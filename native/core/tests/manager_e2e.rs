//! The card manager on the real sing-box (skipped without bin/<platform>/sing-box). All tests of this file share
//! one run folder: the run folder and the ban list are process-wide.

use proxydock_core::log::{Entry, Level};
use proxydock_core::manager::{Events, Folders, Manager, State};
use proxydock_core::{config, graph, singbox};
use serde_json::{json, Value};
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
    let dir = std::env::temp_dir().join(format!("proxydock-mgr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (states, logs) = (Arc::new(Mutex::new(vec![])), Arc::new(Mutex::new(vec![])));
    let (s, l) = (states.clone(), logs.clone());
    let events = Events {
        status: Arc::new(move |id, st| s.lock().unwrap().push((id.to_string(), st.clone()))),
        log: Arc::new(move |e| l.lock().unwrap().push(e)),
    };
    let folders = Folders { run_dir: dir.join("run"), cert_dir: dir.join("certs"), ruleset_dir: dir.join("rulesets") };
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
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command",
            "Get-CimInstance Win32_Process -Filter \"Name='sing-box.exe'\" | Where-Object { $_.CommandLine -like '*m3.json*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }"])
        .output()
        .unwrap();
    assert!(out.status.success());
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
    assert_eq!(m.state("m4"), State::Error(proxydock_core::i18n::tr("certificate pinning needs sing-box 1.15 or later")));
}
