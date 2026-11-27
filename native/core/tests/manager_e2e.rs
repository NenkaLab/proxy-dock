//! The card manager on the real sing-box (skipped without bin/<platform>/sing-box). All tests of this file share
//! one run folder: the run folder and the ban list are process-wide.

use proxydock_core::log::{Entry, Level};
use proxydock_core::manager::{Events, Folders, Manager, State};
use proxydock_core::{config, graph, singbox};
use serde_json::{Value, json};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn exe() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    singbox::find(singbox::EXE, "", &[root])
}

fn glob(extra: Value) -> serde_json::Map<String, Value> {
    let mut g = json!({"singbox_path": exe().unwrap().display().to_string()});
    for (k, v) in extra.as_object().unwrap() {
        g[k] = v.clone();
    }
    config::validate_global(&g)
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn card(id: &str, port: u16) -> Value {
    let g = graph::validate(&json!({
        "inbounds": [{"id": id, "name": format!("card {id}"), "inbound": {"type": "http", "host": "127.0.0.1", "port": port}, "target": "o"}],
        "outbounds": [{"id": "o", "outbound": {"type": "direct"}}],
    }));
    Value::Object(graph::compile(&g).remove(0))
}

struct Seen {
    states: Arc<Mutex<Vec<(String, State)>>>,
    logs: Arc<Mutex<Vec<Entry>>>,
}

impl Seen {
    fn states_of(&self, id: &str) -> Vec<State> {
        self.states.lock().unwrap().iter().filter(|(i, _)| i == id).map(|(_, s)| s.clone()).collect()
    }
    fn logged(&self, f: impl Fn(&Entry) -> bool) -> bool {
        self.logs.lock().unwrap().iter().any(f)
    }
}

fn manager() -> (Manager, Seen) {
    manager_with(false, None)
}

fn helper_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_tun_helper"))
}

fn manager_with(
    force_hysteria: bool,
    force_helper: Option<proxydock_core::singbox::helper::Launch>,
) -> (Manager, Seen) {
    let dir = std::env::temp_dir().join(format!("proxydock-mgr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (states, logs) = (Arc::new(Mutex::new(vec![])), Arc::new(Mutex::new(vec![])));
    let (s, l) = (states.clone(), logs.clone());
    let events = Events {
        status: Arc::new(move |id, st| s.lock().unwrap().push((id.to_string(), st.clone()))),
        log: Arc::new(move |e| l.lock().unwrap().push(e)),
    };
    let folders = Folders {
        run_dir: dir.join("run"),
        cert_dir: dir.join("certs"),
        ruleset_dir: dir.join("rulesets"),
        force_hysteria,
        force_helper,
    };
    (Manager::with_folders(events, folders), Seen { states, logs })
}

fn wait(cond: impl Fn() -> bool, secs: u64) -> bool {
    let until = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < until {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    cond()
}

fn listening(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_ok()
}

#[test]
fn start_apply_restart_stop() {
    if exe().is_none() {
        return;
    }
    let (m, seen) = manager();
    let port = free_port();
    let p = card("m1", port);
    let g = glob(json!({}));
    assert!(m.start(&p, &g).recv().unwrap());
    assert_eq!(m.state("m1"), State::Running);
    assert_eq!(seen.states_of("m1"), [State::Starting, State::Running]);
    assert!(listening(port));
    assert!(seen.logged(|e| e.proxy_id == "m1" && e.level == Level::Info && e.message.contains("HTTP 127.0.0.1:")));

    // a change that applies without a restart: the logging switch
    m.apply(&p, &glob(json!({"log_connections": false}))).recv().unwrap();
    assert_eq!(seen.states_of("m1").len(), 2, "no restart for a logging switch");
    // a setting that lives in the sing-box config: restart
    m.apply(&p, &glob(json!({"timeout": 300}))).recv().unwrap();
    assert_eq!(seen.states_of("m1")[2..], [State::Starting, State::Running]);
    // a new port: restart on it
    let port2 = free_port();
    m.apply(&card("m1", port2), &g).recv().unwrap();
    assert_eq!(m.state("m1"), State::Running);
    assert!(listening(port2) && !listening(port));

    m.stop("m1").recv().unwrap();
    assert_eq!(m.state("m1"), State::Stopped);
    assert!(!listening(port2));
    assert!(seen.logged(|e| e.proxy_id == "m1" && e.message == proxydock_core::i18n::tr("Stopped")));
    // applying to a stopped card does nothing
    m.apply(&p, &g).recv().unwrap();
    assert_eq!(m.state("m1"), State::Stopped);
}

#[test]
fn busy_port_and_death() {
    if exe().is_none() {
        return;
    }
    let (m, seen) = manager();
    let busy = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = busy.local_addr().unwrap().port();
    assert!(!m.start(&card("m2", port), &glob(json!({}))).recv().unwrap());
    match m.state("m2") {
        State::Error(msg) => assert!(msg.contains(&port.to_string()), "{msg}"),
        other => panic!("{other:?}"),
    }
    assert!(seen.logged(|e| e.proxy_id.is_empty() && e.level == Level::Error && e.message.contains("card m2")));
    drop(busy);

    // killed from outside: the card goes to error
    let port = free_port();
    assert!(m.start(&card("m3", port), &glob(json!({}))).recv().unwrap());
    kill_by_command_line("m3.json");
    assert!(wait(|| matches!(m.state("m3"), State::Error(_)), 5), "{:?}", m.state("m3"));
    assert!(!m.is_running("m3"));
    // it can be started again
    assert!(m.start(&card("m3", port), &glob(json!({}))).recv().unwrap());
    m.stop_all(Duration::from_secs(5));
    assert_eq!(m.state("m3"), State::Stopped);
}

#[test]
fn pinned_card_on_old_sing_box_cannot_run() {
    let Some(exe) = exe() else { return };
    if singbox::version::of(&exe).unwrap().has_certificate_pin() {
        return;
    }
    let (m, _) = manager();
    let g = graph::validate(&json!({
        "inbounds": [{"id": "m4", "name": "pinned", "inbound": {"type": "socks5", "host": "127.0.0.1", "port": free_port()}, "target": "o"}],
        "outbounds": [{"id": "o", "outbound": {"type": "hysteria2", "host": "192.0.2.1", "port": 443, "hy2_password": "x", "pin_sha256": "ab".repeat(32)}}],
    }));
    let p = Value::Object(graph::compile(&g).remove(0));
    assert!(!m.start(&p, &glob(json!({}))).recv().unwrap());
    assert_eq!(
        m.state("m4"),
        State::Error(proxydock_core::i18n::tr("certificate pinning needs sing-box 1.15 or later"))
    );
}

fn echo_server() -> u16 {
    use std::io::{Read, Write};
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            std::thread::spawn(move || {
                let mut s = s;
                let mut buf = [0u8; 4096];
                while let Ok(n) = s.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let mut out = b"echo: ".to_vec();
                    out.extend_from_slice(&buf[..n]);
                    if s.write_all(&out).is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

/// "hi" through a SOCKS5 proxy (no auth) to the echo server
fn through_socks(proxy: u16, dest: u16) -> Result<(), String> {
    use std::io::{Read, Write};
    let run = || -> std::io::Result<Vec<u8>> {
        let mut s = TcpStream::connect(("127.0.0.1", proxy))?;
        s.set_read_timeout(Some(Duration::from_secs(8)))?;
        s.write_all(&[5, 1, 0])?;
        let mut r = [0u8; 2];
        s.read_exact(&mut r)?;
        s.write_all(&[5, 1, 0, 1, 127, 0, 0, 1, (dest >> 8) as u8, dest as u8])?;
        let mut rep = [0u8; 10];
        s.read_exact(&mut rep)?;
        if rep[1] != 0 {
            return Err(std::io::Error::other(format!("refused ({})", rep[1])));
        }
        s.write_all(b"hi")?;
        let mut got = [0u8; 8];
        s.read_exact(&mut got)?;
        Ok(got.to_vec())
    };
    match run() {
        Ok(g) if g == b"echo: hi" => Ok(()),
        Ok(g) => Err(format!("{g:?}")),
        Err(e) => Err(e.to_string()),
    }
}

#[test]
fn card_through_the_administrator_helper() {
    if exe().is_none() || !helper_exe().is_file() {
        eprintln!("sing-box or the tun_helper example missing: skipped");
        return;
    }
    let launch = proxydock_core::singbox::helper::Launch {
        program: helper_exe(),
        args: vec!["--tun-helper".into()],
        elevate: false,
    };
    let (m, seen) = manager_with(false, Some(launch));
    let port = free_port();
    let p = card("h1", port);
    assert!(m.start(&p, &glob(json!({}))).recv().unwrap(), "{:?}", m.state("h1"));
    assert!(listening(port));
    assert!(seen.logged(|e| e.proxy_id == "h1"
        && e.message == proxydock_core::i18n::tr("Asking for administrator permission to start the TUN…")));
    m.stop("h1").recv().unwrap();
    assert!(wait(|| !listening(port), 5), "sing-box still listening after stop");

    // sing-box ends under the helper: the card hears it through the helper
    assert!(m.start(&p, &glob(json!({}))).recv().unwrap());
    kill_by_command_line("h1.json");
    assert!(wait(|| matches!(m.state("h1"), State::Error(_)), 8), "{:?}", m.state("h1"));
    // no helper left behind
    assert!(wait(|| running("tun_helper") == 0, 5), "{} tun_helper left", running("tun_helper"));
}

#[test]
fn hysteria2_server_card_on_the_official_core() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    if exe().is_none() || singbox::find(singbox::HY_EXE, "", std::slice::from_ref(&root)).is_none() {
        eprintln!("sing-box or hysteria missing: skipped");
        return;
    }
    let (m, seen) = manager_with(true, None);
    let g = glob(json!({"hysteria_path": singbox::find(singbox::HY_EXE, "", &[root]).unwrap().display().to_string()}));
    let sport = std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let server = Value::Object(graph::compile(&graph::validate(&json!({
        "inbounds": [{"id": "hs", "name": "hy server", "inbound": {"type": "hysteria2", "host": "127.0.0.1", "port": sport, "hy2_password": "right"}, "target": "o"}],
        "outbounds": [{"id": "o", "outbound": {"type": "direct"}}],
    }))).remove(0));
    assert!(m.start(&server, &g).recv().unwrap(), "{:?}", m.state("hs"));
    assert!(seen.logged(|e| e.proxy_id == "hs" && e.message.ends_with("· Hysteria2 + sing-box")));
    let echo = echo_server();
    let client = |id: &str, pw: &str| {
        let port = free_port();
        let p = Value::Object(graph::compile(&graph::validate(&json!({
            "inbounds": [{"id": id, "name": id, "inbound": {"type": "socks5", "host": "127.0.0.1", "port": port}, "target": "o"}],
            "outbounds": [{"id": "o", "outbound": {"type": "hysteria2", "host": "127.0.0.1", "port": sport, "hy2_password": pw, "tls_verify": false}}],
        }))).remove(0));
        (p, port)
    };
    let (good, gport) = client("hc1", "right");
    assert!(m.start(&good, &g).recv().unwrap());
    assert_eq!(through_socks(gport, echo), Ok(()));
    assert!(wait(|| m.stats("hs").up > 0, 5), "{:?}", m.stats("hs"));

    let (bad, bport) = client("hc2", "wrong");
    assert!(m.start(&bad, &g).recv().unwrap());
    assert!(through_socks(bport, echo).is_err());
    assert!(wait(
        || seen.logged(|e| e.proxy_id == "hs" && e.level == Level::Auth && e.message.contains("127.0.0.1")),
        5
    ));
    assert!(m.stats("hs").blocked >= 1);
    assert!(m.last_error("hs").contains("127.0.0.1"));
    m.stop_all(Duration::from_secs(10));
    assert_eq!(m.state("hs"), State::Stopped);
}

/// "Check connection" goes through the whole chain: the outbound's "via" block is used (an HTTP proxy that
/// records the CONNECT it gets), and a failure comes back with what sing-box logged
#[test]
fn connection_check_uses_the_chain() {
    if exe().is_none() {
        eprintln!("sing-box not found: skipped");
        return;
    }
    let (m, _seen) = manager();
    let dead = free_port();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let hop = l.local_addr().unwrap().port();
    let asked = Arc::new(Mutex::new(Vec::<String>::new()));
    let a = asked.clone();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for s in l.incoming().take(3).flatten() {
            let mut s = s;
            let mut buf = [0u8; 1024];
            let n = s.read(&mut buf).unwrap_or(0);
            a.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).lines().next().unwrap_or("").to_string());
            let _ = s.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n");
        }
    });
    let g = graph::validate(&json!({
        "inbounds": [{"id": "c", "name": "card", "inbound": {"type": "socks5", "host": "127.0.0.1", "port": free_port()}, "target": "s"}],
        "outbounds": [{"id": "s", "outbound": {"type": "socks5", "host": "127.0.0.1", "port": dead}, "via": "h"},
                      {"id": "h", "outbound": {"type": "http", "host": "127.0.0.1", "port": hop}}],
    }));
    let card = Value::Object(graph::compile(&g).remove(0));
    let glob = glob(json!({}));
    let out = proxydock_core::probe::check_outbound(&m.remote(), &card, &glob, "__check_t__");
    assert_eq!(out["ok"], false, "{out:?}");
    let asked = asked.lock().unwrap().clone();
    assert!(
        asked.iter().any(|l| l.starts_with(&format!("CONNECT 127.0.0.1:{dead}"))),
        "the chain's first hop was not used: {asked:?} {out:?}"
    );
    assert!(!out["detail"].as_str().unwrap_or("").is_empty(), "{out:?}");
    assert_eq!(m.state("__check_t__"), State::Stopped);
    let (ok, text) = proxydock_core::probe::result_text("socks5", &out, None);
    assert!(!ok && text.contains('\n'), "{text}");

    // straight out to the internet: ok when this PC is online (not required)
    let direct = card_direct();
    let out = proxydock_core::probe::check_outbound(&m.remote(), &direct, &glob, "__check_d__");
    eprintln!("direct check: {}", proxydock_core::probe::result_text("direct", &out, None).1);
}

fn card_direct() -> Value {
    card("cd", free_port())
}

/// End the processes whose command line contains `pattern` (a sing-box killed from outside)
fn kill_by_command_line(pattern: &str) {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", &format!(
                "Get-CimInstance Win32_Process -Filter \"Name='sing-box.exe'\" | Where-Object {{ $_.CommandLine -like '*{}*' }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}",
                pattern.replace('\'', "''"))])
            .output()
            .unwrap();
        assert!(out.status.success());
    }
    // (Linux: /proc/<pid>/cmdline, arguments separated by NULs)
    #[cfg(target_os = "linux")]
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(cmd) = std::fs::read(entry.path().join("cmdline")) else { continue };
        // (sing-box only, as on Windows: the helper's command line names the same config)
        let comm = std::fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        if comm.trim() == "sing-box" && String::from_utf8_lossy(&cmd).contains(pattern) {
            let ok = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status().unwrap();
            assert!(ok.success());
        }
    }
    #[cfg(target_os = "macos")]
    {
        let ok = std::process::Command::new("pkill")
            .args(["-9", "-f", &format!("sing-box run.*{pattern}")])
            .status()
            .unwrap();
        assert!(ok.success());
    }
}

/// How many processes of this program name run
fn running(name: &str) -> usize {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process {name} -ErrorAction SilentlyContinue | Measure-Object).Count"),
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_dir("/proc")
            .unwrap()
            .flatten()
            .filter(|e| std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == name))
            .count()
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("pgrep").args(["-x", name]).output().unwrap();
        String::from_utf8_lossy(&out.stdout).lines().count()
    }
}

#[test]
fn app_switch_refuses_everything_and_lets_through_again() {
    if exe().is_none() {
        return;
    }
    let (m, seen) = manager();
    let dest = echo_server();
    let socks_card = |id: &str, port: u16| {
        let g = graph::validate(&json!({
            "inbounds": [{"id": id, "name": id, "inbound": {"type": "socks5", "host": "127.0.0.1", "port": port}, "target": "o"}],
            "outbounds": [{"id": "o", "outbound": {"type": "direct"}}],
        }));
        Value::Object(graph::compile(&g).remove(0))
    };
    let (p1, p2) = (free_port(), free_port());
    let g = glob(json!({}));
    assert!(m.start(&socks_card("k1", p1), &g).recv().unwrap());
    assert!(through_socks(p1, dest).is_ok());
    // off: the card keeps running (its own state untouched) but refuses everything
    m.set_app_on(false).recv().unwrap();
    assert_eq!(m.state("k1"), State::Running);
    assert!(listening(p1));
    assert!(through_socks(p1, dest).is_err());
    assert!(wait(|| seen.logged(|e| e.proxy_id == "k1" && e.message.contains("switched off")), 5));
    // a card started while off starts refusing too
    assert!(m.start(&socks_card("k2", p2), &g).recv().unwrap());
    assert!(through_socks(p2, dest).is_err());
    // on: both let traffic through again, without a restart
    m.set_app_on(true).recv().unwrap();
    assert!(through_socks(p1, dest).is_ok());
    assert!(through_socks(p2, dest).is_ok());
    assert_eq!(seen.states_of("k1"), [State::Starting, State::Running]);
    m.stop("k1").recv().unwrap();
    m.stop("k2").recv().unwrap();
}
