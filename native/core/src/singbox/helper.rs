//! Administrator helper for TUN cards (Python: `singbox/helper.py`, same protocol).
//!
//! A TUN device needs administrator rights, but ProxyDock itself should not run elevated. When a TUN card starts
//! and ProxyDock is not elevated, it starts `<ProxyDock> --tun-helper <port> <token> <config> <dir>` through UAC
//! ("runas"). The helper connects back to 127.0.0.1:<port>, proves itself with <token>, runs the card's sing-box
//! (only the bundled binary, never a path from the unelevated settings or a downloaded one) and streams its output
//! back. "STOP" or a closed connection (ProxyDock quit or crashed) stops sing-box, and the helper exits with it;
//! the last line is EXIT_MARK + sing-box's exit code.

use crate::i18n::tr;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const EXIT_MARK: &[u8] = b"\x00PROXYDOCK-EXIT ";
/// the user may take a while to answer the UAC prompt
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
/// the first command line argument of the helper mode
pub const ARG: &str = "--tun-helper";

/// How to start the helper: `program args... <port> <token> <config> <dir>`.
#[derive(Clone, Debug)]
pub struct Launch {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// through UAC; false for tests (same protocol, no prompt)
    pub elevate: bool,
}

impl Launch {
    /// this program in helper mode, elevated
    pub fn this_program() -> Option<Launch> {
        Some(Launch { program: std::env::current_exe().ok()?, args: vec![ARG.into()], elevate: true })
    }
}

/// The parent's side of a running helper.
pub struct Conn {
    stream: TcpStream,
    code: Arc<Mutex<Option<i32>>>,
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Start the helper for this config and wait until it connected back with the token.
pub fn launch(how: &Launch, config_path: &Path, workdir: &Path) -> Result<Conn, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = super::process::random_hex(16);
    let mut args = how.args.clone();
    args.extend([port.to_string(), token.clone(), config_path.display().to_string(), workdir.display().to_string()]);
    if how.elevate {
        runas(&how.program, &args)?;
    } else {
        let mut cmd = Command::new(&how.program);
        cmd.args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        cmd.spawn().map_err(|e| e.to_string())?;
    }
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let until = Instant::now() + CONNECT_TIMEOUT;
    while Instant::now() < until {
        match listener.accept() {
            Ok((s, _)) => {
                let _ = s.set_nonblocking(false);
                let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
                let mut line = vec![];
                let mut r = BufReader::new(&s);
                if r.read_until(b'\n', &mut line).is_ok() && same(line.trim_ascii(), token.as_bytes()) {
                    // nothing else was sent yet: the helper waits for its sing-box first, so the buffer is empty
                    let _ = s.set_read_timeout(None);
                    return Ok(Conn { stream: s, code: Arc::new(Mutex::new(None)) });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(tr("The administrator helper did not connect"))
}

impl Conn {
    /// a thread that sends the helper's output lines to `tx` (None at the end) and records the exit code
    pub fn spawn_reader(&self, tx: Sender<Option<String>>) -> std::io::Result<std::thread::JoinHandle<()>> {
        let s = self.stream.try_clone()?;
        let code = self.code.clone();
        Ok(std::thread::spawn(move || {
            for line in BufReader::new(s).split(b'\n') {
                let Ok(line) = line else { break };
                if let Some(rest) = line.strip_prefix(EXIT_MARK) {
                    *code.lock().unwrap() = Some(String::from_utf8_lossy(rest).trim().parse().unwrap_or(1));
                    break;
                }
                let _ = tx.send(Some(String::from_utf8_lossy(&line).into_owned()));
            }
            code.lock().unwrap().get_or_insert(-1); // connection lost
            let _ = tx.send(None);
        }))
    }

    /// ask the helper to stop sing-box; the reader ends within a few seconds either way
    pub fn kill(&mut self) {
        let _ = self.stream.write_all(b"STOP\n");
        let _ = self.stream.set_read_timeout(Some(Duration::from_secs(5)));
    }

    /// sing-box's exit code once the reader saw the end (-1: the connection was lost)
    pub fn code(&self) -> Option<i32> {
        *self.code.lock().unwrap()
    }

    pub fn close(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Windows command line quoting (the rules CommandLineToArgvW reads back)
pub fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '\n', '\x0b', '"']) {
        return a.to_string();
    }
    let mut out = String::from('"');
    let mut slashes = 0;
    for c in a.chars() {
        match c {
            '\\' => slashes += 1,
            '"' => {
                out.push_str(&"\\".repeat(slashes * 2 + 1));
                out.push('"');
                slashes = 0;
            }
            c => {
                out.push_str(&"\\".repeat(slashes));
                out.push(c);
                slashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

/// Start program elevated (UAC) without a window. Err when the user said no.
#[cfg(windows)]
pub fn runas(program: &Path, args: &[String]) -> Result<(), String> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
    use windows_sys::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let verb = wide("runas");
    let file = wide(&program.display().to_string());
    let params = wide(&args.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" "));
    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb = verb.as_ptr();
    info.lpFile = file.as_ptr();
    info.lpParameters = params.as_ptr();
    info.nShow = 0; // SW_HIDE
    if unsafe { ShellExecuteExW(&mut info) } == 0 {
        let err = unsafe { GetLastError() };
        if err == 1223 {
            // ERROR_CANCELLED
            return Err(tr("Administrator permission was not given (the UAC prompt was cancelled)"));
        }
        return Err(tr("Could not start the administrator helper (error {code})").replace("{code}", &err.to_string()));
    }
    if !info.hProcess.is_null() {
        unsafe { CloseHandle(info.hProcess) };
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn runas(_program: &Path, _args: &[String]) -> Result<(), String> {
    Err(tr("A TUN card needs ProxyDock to run as root"))
}

/// The helper side (runs elevated): `args` = port, token, config, dir. Returns sing-box's exit code.
pub fn main(args: &[String]) -> i32 {
    let [port, token, config_path, workdir, ..] = args else { return 2 };
    let Ok(port) = port.parse::<u16>() else { return 2 };
    let Ok(mut sock) = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(10)) else {
        return 1;
    };
    let _ = sock.write_all(format!("{token}\n").as_bytes());
    let Some(exe) = super::find_bundled(super::EXE) else {
        let mut msg = b"FATAL[0000] sing-box was not found\n".to_vec();
        msg.extend_from_slice(EXIT_MARK);
        msg.extend_from_slice(b"1\n");
        let _ = sock.write_all(&msg);
        return 1;
    };
    let mut cmd = Command::new(&exe);
    cmd.args(["run", "--disable-color", "-c", config_path, "-D", workdir]);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let mut msg = format!("FATAL[0000] {e}\n").into_bytes();
            msg.extend_from_slice(EXIT_MARK);
            msg.extend_from_slice(b"1\n");
            let _ = sock.write_all(&msg);
            return 1;
        }
    };
    #[cfg(windows)]
    super::process::attach_to_job(&child); // sing-box also ends if this helper is killed
    let out = Arc::new(Mutex::new(sock.try_clone().expect("socket")));
    let mut forwarders = vec![];
    for pipe in [
        child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
        child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let out = out.clone();
        forwarders.push(std::thread::spawn(move || {
            for line in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
                let mut l = line;
                l.push(b'\n');
                if out.lock().unwrap().write_all(&l).is_err() {
                    break;
                }
            }
        }));
    }
    let child = Arc::new(Mutex::new(child));
    {
        // STOP or a closed connection ends sing-box
        let child = child.clone();
        let mut ctl = sock.try_clone().expect("socket");
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match ctl.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if buf.windows(4).any(|w| w == b"STOP") {
                            break;
                        }
                    }
                }
            }
            let _ = child.lock().unwrap().kill();
        });
    }
    let code = loop {
        if let Some(st) = child.lock().unwrap().try_wait().ok().flatten() {
            break st.code().unwrap_or(1);
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    for f in forwarders {
        let _ = f.join();
    }
    let mut msg = EXIT_MARK.to_vec();
    msg.extend_from_slice(format!("{code}\n").as_bytes());
    let _ = out.lock().unwrap().write_all(&msg);
    let _ = sock.shutdown(std::net::Shutdown::Both);
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_round_trips_windows_rules() {
        assert_eq!(quote_arg("plain"), "plain");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg(r"C:\Program Files\x"), r#""C:\Program Files\x""#);
        assert_eq!(quote_arg(r"dir\ "), r#""dir\ ""#);
        assert_eq!(quote_arg(r"end\\"), r"end\\");
        assert_eq!(quote_arg(r"a b\"), r#""a b\\""#);
        assert_eq!(quote_arg(r#"say "hi""#), r#""say \"hi\"""#);
    }
}
