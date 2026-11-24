//! The controller with a settings file of two cards, talked to like the window does (needs bin/<platform>/sing-box).

use proxydock::{controller, ipc};
use serde_json::{json, Value};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

/// the run folder is process-wide: the tests of this file take turns
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn singbox() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    proxydock_core::singbox::find(proxydock_core::singbox::EXE, "", &[root])
}

/// the window's side: (writer, messages from the core)
fn connect(h: &controller::Handle, token: &str) -> (TcpStream, Receiver<Value>) {
    let mut s = TcpStream::connect(("127.0.0.1", h.port)).unwrap();
    ipc::send(&mut s, &json!({"cmd": "hello", "token": token})).unwrap();
    let (tx, rx) = channel();
    let r = s.try_clone().unwrap();
    std::thread::spawn(move || {
        for v in ipc::messages(r) {
            if tx.send(v).is_err() {
                break;
            }
        }
    });
    (s, rx)
}

/// the next message matching f within secs
fn expect(rx: &Receiver<Value>, secs: u64, f: impl Fn(&Value) -> bool) -> Option<Value> {
    let until = Instant::now() + Duration::from_secs(secs);
    while let Some(left) = until.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(v) if f(&v) => return Some(v),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

#[test]
fn window_protocol() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(exe) = singbox() else {
        eprintln!("sing-box not found: skipped");
        return;
    };
    let home = std::env::temp_dir().join(format!("proxydock-app-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let (p1, p2) = (free_port(), free_port());
    let settings = json!({
        "version": 3,
        "global": {"language": "ko", "singbox_path": exe.display().to_string()},
        "graph": {
            "inbounds": [
                {"id": "a1", "name": "auto", "autostart": true, "inbound": {"type": "http", "host": "127.0.0.1", "port": p1}, "target": "o"},
                {"id": "b2", "name": "manual", "inbound": {"type": "socks5", "host": "127.0.0.1", "port": p2}, "target": "o"},
            ],
            "outbounds": [{"id": "o", "name": "Direct", "outbound": {"type": "direct"}}],
        },
    });
    let path = home.join("config.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    // the run folder and certificates go under the test folder too
    proxydock_core::singbox::guard::set_run_dir(home.join("run"));
    let h = controller::spawn(path).unwrap();

    // a wrong token gets nothing and is dropped
    let (_bad, bad_rx) = connect(&h, "nope");
    assert!(expect(&bad_rx, 2, |_| true).is_none());

    let (mut w, rx) = connect(&h, &h.token);
    let hello = expect(&rx, 5, |v| v["ev"] == "hello").expect("hello");
    assert_eq!(hello["language"], "ko");
    let cards = hello["cards"].as_array().unwrap();
    assert_eq!(cards.iter().map(|c| c["name"].as_str().unwrap()).collect::<Vec<_>>(), ["auto", "manual"]);
    assert!(cards[0]["summary"].as_str().unwrap().starts_with("HTTP 127.0.0.1:"), "{}", cards[0]);

    // the autostart card runs (it may have been running before the hello)
    let until = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("127.0.0.1", p1)).is_err() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(TcpStream::connect(("127.0.0.1", p1)).is_ok(), "autostart card not listening");
    assert!(h.cards().iter().any(|c| c.id == "a1" && c.state == proxydock_core::manager::State::Running));

    // toggle the manual one on and off
    ipc::send(&mut w, &json!({"cmd": "toggle", "id": "b2"})).unwrap();
    assert!(expect(&rx, 10, |v| v["ev"] == "card" && v["card"]["id"] == "b2" && v["card"]["state"] == "running").is_some());
    assert!(TcpStream::connect(("127.0.0.1", p2)).is_ok());
    assert!(expect(&rx, 5, |v| v["ev"] == "log" && v["entries"][0]["name"] == "manual" && v["entries"][0]["message"].as_str().unwrap().starts_with("시작: ")).is_some());
    assert!(expect(&rx, 3, |v| v["ev"] == "stats" && v["stats"]["b2"].is_object()).is_some());
    ipc::send(&mut w, &json!({"cmd": "toggle", "id": "b2"})).unwrap();
    assert!(expect(&rx, 10, |v| v["ev"] == "card" && v["card"]["id"] == "b2" && v["card"]["state"] == "stopped").is_some());

    // stop all, then quit: nothing listening
    ipc::send(&mut w, &json!({"cmd": "stop_all"})).unwrap();
    assert!(expect(&rx, 10, |v| v["ev"] == "card" && v["card"]["id"] == "a1" && v["card"]["state"] == "stopped").is_some());
    h.quit(Duration::from_secs(10));
    assert!(TcpStream::connect(("127.0.0.1", p1)).is_err() && TcpStream::connect(("127.0.0.1", p2)).is_err());
    let _ = std::fs::remove_dir_all(&home);
}

fn saved(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn editing_saves_and_applies() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(exe) = singbox() else {
        eprintln!("sing-box not found: skipped");
        return;
    };
    let home = std::env::temp_dir().join(format!("proxydock-edit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let (p1, up) = (free_port(), free_port());
    let settings = json!({
        "version": 3,
        "global": {"singbox_path": exe.display().to_string()},
        "graph": {
            "inbounds": [{"id": "a1", "name": "card", "autostart": true, "inbound": {"type": "http", "host": "127.0.0.1", "port": p1, "auth_enabled": true, "username": "u", "password": "secret pw"}, "target": "d"}],
            "outbounds": [
                {"id": "d", "name": "Direct", "outbound": {"type": "direct"}},
                {"id": "s", "name": "Up", "outbound": {"type": "socks5", "host": "127.0.0.1", "port": up}},
                {"id": "g", "name": "Group", "outbound": {"type": "group"}},
            ],
        },
    });
    let path = home.join("config.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    proxydock_core::singbox::guard::set_run_dir(home.join("run"));
    let h = controller::spawn(path.clone()).unwrap();
    let (mut w, rx) = connect(&h, &h.token);
    expect(&rx, 5, |v| v["ev"] == "hello").expect("hello");
    let until = Instant::now() + Duration::from_secs(10);
    while !h.cards().iter().any(|c| c.state == proxydock_core::manager::State::Running) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }

    // plain-text password of the hand-written file: sealed on load (Windows)
    let f = saved(&path);
    assert_eq!(f["version"], 3);
    if cfg!(windows) {
        assert!(f["graph"]["inbounds"][0]["inbound"].get("password").is_none());
        assert!(f["graph"]["inbounds"][0]["inbound"]["password_dpapi"].as_str().unwrap().len() > 20);
    }

    // moving: saved
    ipc::send(&mut w, &json!({"cmd": "move", "blocks": [{"key": "a1", "x": 33.5, "y": 44.0}]})).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while saved(&path)["graph"]["inbounds"][0]["pos"] != json!([33.5, 44.0]) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(saved(&path)["graph"]["inbounds"][0]["pos"], json!([33.5, 44.0]));

    // a refused link: Direct ends a chain -> error with the reason, canvas sent again
    ipc::send(&mut w, &json!({"cmd": "connect", "from": "d", "out": 0, "to": "s", "in": 0})).unwrap();
    let e = expect(&rx, 5, |v| v["ev"] == "error").expect("error");
    assert!(e["message"].as_str().unwrap().contains("Direct"), "{e}");
    assert!(expect(&rx, 5, |v| v["ev"] == "graph").is_some());

    // the card now goes to Up: saved, and the running card restarts with it
    ipc::send(&mut w, &json!({"cmd": "connect", "from": "a1", "out": 0, "to": "s", "in": 0})).unwrap();
    let g = expect(&rx, 5, |v| v["ev"] == "graph").expect("graph");
    assert!(g["canvas"]["links"].as_array().unwrap().iter().any(|l| l["from"] == "a1" && l["to"] == "s"));
    assert_eq!(saved(&path)["graph"]["inbounds"][0]["target"], "s");
    assert!(expect(&rx, 10, |v| v["ev"] == "card" && v["card"]["id"] == "a1" && v["card"]["state"] == "running"
        && v["card"]["summary"].as_str().unwrap().contains(&format!("127.0.0.1:{up}"))).is_some());

    // group members, adding and deleting blocks
    ipc::send(&mut w, &json!({"cmd": "connect", "from": "g", "out": 0, "to": "d", "in": 0})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "graph").expect("graph");
    assert_eq!(saved(&path)["graph"]["outbounds"][2]["members"], json!(["d"]));
    ipc::send(&mut w, &json!({"cmd": "add", "kind": 1, "x": 10.0, "y": 20.0})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "graph").expect("graph");
    assert_eq!(saved(&path)["graph"]["routers"][0]["name"], "Router");
    ipc::send(&mut w, &json!({"cmd": "delete", "keys": ["s"]})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "graph").expect("graph");
    let f = saved(&path);
    assert!(f["graph"]["outbounds"].as_array().unwrap().iter().all(|o| o["id"] != "s"));
    assert!(f["graph"]["inbounds"][0]["target"].is_null());
    // left with nothing to send to: the card stops
    assert!(expect(&rx, 10, |v| v["ev"] == "card" && v["card"]["id"] == "a1" && v["card"]["state"] == "stopped").is_some());

    // the saved file reads back with the same password
    let l = proxydock_core::config::load_from(&path).unwrap();
    assert_eq!(l.graph["inbounds"][0]["inbound"]["password"], "secret pw");
    h.quit(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn block_editor() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let Some(exe) = singbox() else {
        eprintln!("sing-box not found: skipped");
        return;
    };
    let home = std::env::temp_dir().join(format!("proxydock-form-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let (p1, p2) = (free_port(), free_port());
    let settings = json!({
        "version": 3,
        "global": {"singbox_path": exe.display().to_string()},
        "graph": {
            "inbounds": [{"id": "a1", "name": "card", "autostart": true, "inbound": {"type": "socks5", "host": "127.0.0.1", "port": p1}, "target": "r"}],
            "outbounds": [{"id": "d", "name": "Direct", "outbound": {"type": "direct"}}],
            "routers": [{"id": "r", "name": "R", "rules": [{"id": "x", "name": "kr", "match": {"domain_suffix": ["kr"]}, "target": "d"}], "default": "d"}],
        },
    });
    let path = home.join("config.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    proxydock_core::singbox::guard::set_run_dir(home.join("run"));
    let h = controller::spawn(path.clone()).unwrap();
    let (mut w, rx) = connect(&h, &h.token);
    expect(&rx, 5, |v| v["ev"] == "hello").expect("hello");
    let field = |form: &Value, key: &str| form["fields"].as_array().unwrap().iter().find(|f| f["key"] == key).cloned().unwrap();

    // a card: account fields appear with authentication on
    ipc::send(&mut w, &json!({"cmd": "edit_open", "id": "a1"})).unwrap();
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    assert_eq!(f["kind"], "inbound");
    assert_eq!(field(&f, "inbound.port")["value"], p1);
    assert_eq!(field(&f, "inbound.username")["visible"], false);
    ipc::send(&mut w, &json!({"cmd": "edit_preview", "id": "a1", "values": {"inbound.auth_enabled": true}})).unwrap();
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    assert_eq!(field(&f, "inbound.username")["visible"], true);
    // saving without an account: the reason, nothing saved
    ipc::send(&mut w, &json!({"cmd": "edit_save", "id": "a1", "values": {"inbound.auth_enabled": true}})).unwrap();
    let e = expect(&rx, 5, |v| v["ev"] == "form_errors").expect("errors");
    assert!(e["errors"][0].as_str().unwrap().contains("username"), "{e}");
    // a new port: saved, the running card restarts on it
    let until = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("127.0.0.1", p1)).is_err() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    ipc::send(&mut w, &json!({"cmd": "edit_save", "id": "a1", "values": {"inbound.port": p2.to_string(), "name": " renamed "}})).unwrap();
    expect(&rx, 10, |v| v["ev"] == "form_saved").expect("saved");
    let f: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(f["graph"]["inbounds"][0]["inbound"]["port"], p2);
    assert_eq!(f["graph"]["inbounds"][0]["name"], "renamed");
    assert_eq!(f["graph"]["inbounds"][0]["target"], "r");
    let until = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("127.0.0.1", p2)).is_err() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(TcpStream::connect(("127.0.0.1", p2)).is_ok() && TcpStream::connect(("127.0.0.1", p1)).is_err());

    // a router rule: a bad port is refused, a good condition saved
    ipc::send(&mut w, &json!({"cmd": "edit_save", "id": "r", "values": {"rules.0.match.port": "443, 99999"}})).unwrap();
    let e = expect(&rx, 5, |v| v["ev"] == "form_errors").expect("errors");
    assert!(e["errors"].to_string().contains("99999"), "{e}");
    ipc::send(&mut w, &json!({"cmd": "edit_save", "id": "r", "values": {"rules.0.match.geoip": "kr, jp", "rules.0.match.domain_suffix": ""}})).unwrap();
    expect(&rx, 10, |v| v["ev"] == "form_saved").expect("saved");
    let f: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(f["graph"]["routers"][0]["rules"][0]["match"], json!({"geoip": ["kr", "jp"]}));

    // an outbound: the type decides the fields; a VLESS block needs a UUID
    ipc::send(&mut w, &json!({"cmd": "edit_preview", "id": "d", "values": {"outbound.type": "vless"}})).unwrap();
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    assert_eq!(field(&f, "outbound.uuid")["visible"], true);
    assert_eq!(field(&f, "outbound.bind")["visible"], false);
    ipc::send(&mut w, &json!({"cmd": "edit_save", "id": "d", "values": {"outbound.type": "vless", "outbound.host": "h", "outbound.port": "443"}})).unwrap();
    let e = expect(&rx, 5, |v| v["ev"] == "form_errors").expect("errors");
    assert!(e["errors"].to_string().contains("UUID"), "{e}");
    h.quit(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&home);
}
