//! pd-agent - runs inside the VPN-bypass VM (started by /init, restarted if it exits).
//!
//! * control port (vsock 20548): line-JSON requests from ProxyDock - `status`, `net` (DHCP or a fixed address),
//!   `socks_auth`, `poweroff` - each answered with one JSON line.
//! * data port (vsock 20549): every stream is relayed to the sing-box SOCKS5 server inside the VM (127.0.0.1
//!   only), which leaves through the VM's own network card. ProxyDock's sing-box reaches it as a `socks` outbound
//!   with UDP over TCP, so UDP travels over these streams too.
//!
//! The VM's network card is bridged to a physical one, so its traffic never passes the host's IP stack and the
//! host's VPN. Hyper-V sockets go over VMBus, not IP, so the host's VPN (or its "block LAN" option) does not see
//! the host<->VM link either.
//!
//! `--tcp` listens on 127.0.0.1 TCP ports instead of vsock and `--no-net` leaves the network alone: for tests in an
//! ordinary Linux container.

mod net;
mod sock;

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const CONTROL_PORT: u32 = 20548;
pub const DATA_PORT: u32 = 20549;

pub struct Opts {
    pub tcp: bool,
    pub no_net: bool,
    pub singbox: String,
    pub socks_port: u16,
    pub run_dir: String,
}

impl Opts {
    fn parse() -> Opts {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.to_string());
        Opts {
            tcp: args.iter().any(|a| a == "--tcp"),
            no_net: args.iter().any(|a| a == "--no-net"),
            singbox: env("PD_SINGBOX", "/usr/bin/sing-box"),
            socks_port: env("PD_SOCKS_PORT", "1080").parse().unwrap_or(1080),
            run_dir: env("PD_RUN_DIR", "/run/proxydock"),
        }
    }
}

pub struct Agent {
    opts: Opts,
    started: Instant,
    net: Mutex<net::Net>,
    singbox_up: AtomicBool,
    /// the account the SOCKS5 server asks for (set by ProxyDock before it opens the relay on the host, so no other
    /// program on the host can use this way out)
    auth: Mutex<Option<(String, String)>>,
    /// sing-box's pid while it runs (socks_auth restarts it)
    singbox_pid: Mutex<Option<u32>>,
    /// a restart was asked for: start again at once
    restart: AtomicBool,
}

fn main() {
    let opts = Opts::parse();
    let _ = std::fs::create_dir_all(&opts.run_dir);
    let agent = Arc::new(Agent {
        net: Mutex::new(net::Net::new(opts.no_net)),
        opts,
        started: Instant::now(),
        singbox_up: AtomicBool::new(false),
        auth: Mutex::new(None),
        singbox_pid: Mutex::new(None),
        restart: AtomicBool::new(false),
    });
    log(&format!("pd-agent {VERSION} starting"));
    if !agent.opts.no_net {
        let r = agent.net.lock().unwrap().apply(&net::Mode::Dhcp);
        if let Err(e) = r {
            log(&format!("network: {e}"));
        }
    }
    let a = agent.clone();
    std::thread::spawn(move || supervise_singbox(&a));

    let data = sock::Listener::bind(agent.opts.tcp, DATA_PORT).unwrap_or_else(|e| fatal(&format!("data port: {e}")));
    let socks_port = agent.opts.socks_port;
    std::thread::spawn(move || {
        loop {
            match data.accept() {
                Ok(c) => {
                    std::thread::spawn(move || relay(c, socks_port));
                }
                Err(e) => {
                    log(&format!("data accept: {e}"));
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    });

    let control =
        sock::Listener::bind(agent.opts.tcp, CONTROL_PORT).unwrap_or_else(|e| fatal(&format!("control port: {e}")));
    loop {
        match control.accept() {
            Ok(c) => {
                let a = agent.clone();
                std::thread::spawn(move || serve_control(&a, c));
            }
            Err(e) => {
                log(&format!("control accept: {e}"));
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn fatal(msg: &str) -> ! {
    log(msg);
    std::process::exit(1)
}

pub fn log(msg: &str) {
    eprintln!("pd-agent: {msg}");
}

/// The sing-box config inside the VM: a SOCKS5 server on 127.0.0.1 (only the relay reaches it) that sends
/// everything out directly. UDP over TCP from ProxyDock's socks outbound is handled by sing-box itself.
pub fn singbox_config(socks_port: u16, auth: Option<&(String, String)>) -> Value {
    let mut inbound = json!({"type": "socks", "tag": "in", "listen": "127.0.0.1", "listen_port": socks_port});
    if let Some((user, pass)) = auth {
        inbound["users"] = json!([{"username": user, "password": pass}]);
    }
    json!({
        "log": {"level": "warn"},
        "dns": {"servers": [{"type": "local", "tag": "local"}]},
        "inbounds": [inbound],
        "outbounds": [{"type": "direct", "tag": "direct"}],
        "route": {"final": "direct", "default_domain_resolver": "local"}
    })
}

/// an account part ProxyDock sends: 8-64 letters and digits (it goes into a JSON file, nothing else)
fn valid_secret(s: &str) -> bool {
    (8..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn supervise_singbox(agent: &Agent) {
    let dir = &agent.opts.run_dir;
    let cfg = format!("{dir}/sing-box.json");
    let mut delay = Duration::from_secs(1);
    loop {
        let conf = singbox_config(agent.opts.socks_port, agent.auth.lock().unwrap().as_ref());
        if let Err(e) = std::fs::write(&cfg, conf.to_string()) {
            fatal(&format!("cannot write {cfg}: {e}"));
        }
        let started = Instant::now();
        let child: std::io::Result<Child> =
            Command::new(&agent.opts.singbox).args(["run", "-c", &cfg, "-D", dir, "--disable-color"]).spawn();
        match child {
            Ok(mut c) => {
                *agent.singbox_pid.lock().unwrap() = Some(c.id());
                agent.singbox_up.store(true, Ordering::SeqCst);
                let st = c.wait();
                agent.singbox_up.store(false, Ordering::SeqCst);
                *agent.singbox_pid.lock().unwrap() = None;
                if agent.restart.swap(false, Ordering::SeqCst) {
                    delay = Duration::from_millis(100);
                    std::thread::sleep(delay);
                    continue;
                }
                log(&format!("sing-box exited ({st:?}), restarting"));
            }
            Err(e) => log(&format!("cannot start sing-box: {e}")),
        }
        // back off while it keeps failing right away, start over once it ran for a while
        delay = if started.elapsed() > Duration::from_secs(30) { Duration::from_secs(1) } else { delay * 2 };
        delay = delay.min(Duration::from_secs(30));
        std::thread::sleep(delay);
    }
}

/// Copy both ways between a host stream and the VM's SOCKS5 server until both sides are done.
fn relay(host: Box<dyn sock::Conn>, socks_port: u16) {
    let up = match TcpStream::connect(("127.0.0.1", socks_port)) {
        Ok(s) => s,
        Err(e) => {
            log(&format!("relay: sing-box not reachable: {e}"));
            return;
        }
    };
    let _ = up.set_nodelay(true);
    let (mut host_r, mut up_w) = match (host.try_clone_conn(), up.try_clone()) {
        (Ok(h), Ok(u)) => (h, u),
        _ => return,
    };
    let t = std::thread::spawn(move || {
        let _ = std::io::copy(&mut host_r, &mut up_w);
        let _ = up_w.shutdown(std::net::Shutdown::Write);
    });
    let mut up_r = up;
    let mut host_w = host;
    let _ = copy(&mut up_r, &mut *host_w);
    host_w.shutdown_write();
    let _ = t.join();
}

fn copy(r: &mut impl Read, w: &mut dyn sock::Conn) -> std::io::Result<u64> {
    let mut buf = vec![0u8; 64 * 1024];
    let mut n = 0;
    loop {
        let k = match r.read(&mut buf) {
            Ok(0) => return Ok(n),
            Ok(k) => k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        w.write_all(&buf[..k])?;
        n += k as u64;
    }
}

fn serve_control(agent: &Agent, conn: Box<dyn sock::Conn>) {
    let Ok(mut w) = conn.try_clone_conn() else { return };
    let reader = BufReader::new(conn);
    for line in reader.lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(req) => handle(agent, &req),
            Err(e) => json!({"ok": false, "error": format!("bad request: {e}")}),
        };
        let mut out = reply.to_string();
        out.push('\n');
        if w.write_all(out.as_bytes()).is_err() {
            return;
        }
    }
}

pub fn handle(agent: &Agent, req: &Value) -> Value {
    let op = req.get("op").and_then(Value::as_str).unwrap_or("");
    let result: Result<(), String> = match op {
        "status" => Ok(()),
        "net" => net::Mode::from_request(req).and_then(|m| agent.net.lock().unwrap().apply(&m)),
        "socks_auth" => {
            let s = |k: &str| req.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let (user, pass) = (s("user"), s("pass"));
            if !valid_secret(&user) || !valid_secret(&pass) {
                Err("user and pass: 8-64 letters and digits".into())
            } else {
                *agent.auth.lock().unwrap() = Some((user, pass));
                restart_singbox(agent);
                Ok(())
            }
        }
        "poweroff" => {
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_millis(200));
                unsafe { libc::sync() };
                unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_POWER_OFF) };
            });
            Ok(())
        }
        _ => Err(format!("unknown op {op:?}")),
    };
    let mut reply = json!({"ok": result.is_ok(), "status": status(agent)});
    if let Err(e) = result {
        reply["error"] = json!(e);
    }
    reply
}

/// stop sing-box; the supervisor starts it again at once with the current config
fn restart_singbox(agent: &Agent) {
    if let Some(pid) = *agent.singbox_pid.lock().unwrap() {
        agent.restart.store(true, Ordering::SeqCst);
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
    }
}

fn status(agent: &Agent) -> Value {
    let n = agent.net.lock().unwrap();
    let mut s = n.status();
    s["version"] = json!(VERSION);
    s["uptime_s"] = json!(agent.started.elapsed().as_secs());
    s["singbox"] = json!(agent.singbox_up.load(Ordering::SeqCst));
    s["auth"] = json!(agent.auth.lock().unwrap().is_some());
    let (total, avail) = meminfo();
    s["mem_total_kb"] = json!(total);
    s["mem_available_kb"] = json!(avail);
    s
}

fn meminfo() -> (u64, u64) {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let get = |k: &str| {
        text.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    };
    (get("MemTotal:"), get("MemAvailable:"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_has_the_account_only_when_set() {
        let c = singbox_config(1080, None);
        assert!(c["inbounds"][0].get("users").is_none());
        let a = ("user1234".to_string(), "pass5678".to_string());
        let c = singbox_config(1080, Some(&a));
        assert_eq!(c["inbounds"][0]["users"][0]["username"], "user1234");
        assert_eq!(c["inbounds"][0]["listen"], "127.0.0.1");
        assert!(valid_secret("abcDEF12"));
        assert!(!valid_secret("short1"));
        assert!(!valid_secret("has space 123"));
        assert!(!valid_secret(&"a".repeat(65)));
    }
}
