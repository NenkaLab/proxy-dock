//! The controller with a settings file of two cards, talked to like the window does (needs bin/<platform>/sing-box).

use proxydock::{controller, ipc};
use serde_json::{Value, json};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

/// the run folder is process-wide: the tests of this file take turns
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// One test at a time, and the registry keys of start at login and the system proxy pointed at a test key
/// (never the real settings).
fn serial() -> std::sync::MutexGuard<'static, ()> {
    let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let base = format!(r"Software\ProxyDockTest\ipc{}", std::process::id());
    proxydock_core::platform::set_test_keys(&format!(r"{base}\Run"), &format!(r"{base}\Inet"));
    // (macOS / Linux: the login items in a temporary folder)
    proxydock_core::platform::set_test_dir(
        &std::env::temp_dir().join(format!("proxydock-ipc-login-{}", std::process::id())),
    );
    g
}

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
    let _serial = serial();
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
    assert!(
        expect(&rx, 10, |v| v["ev"] == "card" && v["card"]["id"] == "b2" && v["card"]["state"] == "running").is_some()
    );
    assert!(TcpStream::connect(("127.0.0.1", p2)).is_ok());
    assert!(
        expect(&rx, 5, |v| v["ev"] == "log"
            && v["entries"][0]["name"] == "manual"
            && v["entries"][0]["message"].as_str().unwrap().starts_with("시작: "))
        .is_some()
    );
    assert!(expect(&rx, 3, |v| v["ev"] == "stats" && v["stats"]["b2"].is_object()).is_some());
    ipc::send(&mut w, &json!({"cmd": "toggle", "id": "b2"})).unwrap();
    assert!(
        expect(&rx, 10, |v| v["ev"] == "card" && v["card"]["id"] == "b2" && v["card"]["state"] == "stopped").is_some()
    );

    // stop all, then quit: nothing listening
    ipc::send(&mut w, &json!({"cmd": "stop_all"})).unwrap();
    assert!(
        expect(&rx, 10, |v| v["ev"] == "card" && v["card"]["id"] == "a1" && v["card"]["state"] == "stopped").is_some()
    );
    h.quit(Duration::from_secs(10));
    assert!(TcpStream::connect(("127.0.0.1", p1)).is_err() && TcpStream::connect(("127.0.0.1", p2)).is_err());
    let _ = std::fs::remove_dir_all(&home);
}

fn saved(path: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn editing_saves_and_applies() {
    let _serial = serial();
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
    assert!(
        expect(&rx, 10, |v| v["ev"] == "card"
            && v["card"]["id"] == "a1"
            && v["card"]["state"] == "running"
            && v["card"]["summary"].as_str().unwrap().contains(&format!("127.0.0.1:{up}")))
        .is_some()
    );

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
    assert!(
        expect(&rx, 10, |v| v["ev"] == "card" && v["card"]["id"] == "a1" && v["card"]["state"] == "stopped").is_some()
    );

    // the saved file reads back with the same password
    let l = proxydock_core::config::load_from(&path).unwrap();
    assert_eq!(l.graph["inbounds"][0]["inbound"]["password"], "secret pw");
    h.quit(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn block_editor() {
    let _serial = serial();
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
    let field =
        |form: &Value, key: &str| form["fields"].as_array().unwrap().iter().find(|f| f["key"] == key).cloned().unwrap();

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
    ipc::send(&mut w, &json!({"cmd": "edit_save", "id": "a1", "values": {"inbound.auth_enabled": false, "inbound.port": p2.to_string(), "name": " renamed "}})).unwrap();
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
    ipc::send(&mut w, &json!({"cmd": "edit_open", "id": "r"})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    ipc::send(&mut w, &json!({"cmd": "edit_save", "id": "r", "values": {"rules.0.match.port": "443, 99999"}})).unwrap();
    let e = expect(&rx, 5, |v| v["ev"] == "form_errors").expect("errors");
    assert!(e["errors"].to_string().contains("99999"), "{e}");
    // (the bad port is still in the draft: corrected with the new values)
    ipc::send(
        &mut w,
        &json!({"cmd": "edit_action", "id": "r", "action": "rule.add",
        "values": {"rules.0.match.port": "", "rules.0.match.geoip": "kr, jp", "rules.0.match.domain_suffix": ""}}),
    )
    .unwrap();
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    assert!(f["fields"].as_array().unwrap().iter().any(|x| x["key"] == "rules.1.target"));
    ipc::send(&mut w, &json!({"cmd": "edit_save", "id": "r", "values": {"rules.1.match.domain_keyword": "ads", "rules.1.target": "d"}})).unwrap();
    let r = expect(&rx, 10, |v| v["ev"] == "form_saved" || v["ev"] == "form_errors").expect("answer");
    assert_eq!(r["ev"], "form_saved", "{r}");
    let f: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(f["graph"]["routers"][0]["rules"][0]["match"], json!({"geoip": ["kr", "jp"]}));
    assert_eq!(f["graph"]["routers"][0]["rules"][1]["match"], json!({"domain_keyword": ["ads"]}));
    assert_eq!(f["graph"]["routers"][0]["rules"][1]["target"], "d");

    // the card becomes a WireGuard server: a key is there, devices get addresses and configs
    ipc::send(&mut w, &json!({"cmd": "edit_open", "id": "a1"})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    ipc::send(&mut w, &json!({"cmd": "edit_preview", "id": "a1", "values": {"inbound.type": "wireguard"}})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    ipc::send(&mut w, &json!({"cmd": "edit_action", "id": "a1", "action": "gen.inbound.wg_private_key", "values": {}}))
        .unwrap();
    expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    ipc::send(
        &mut w,
        &json!({"cmd": "edit_action", "id": "a1", "action": "wgdev.add", "values": {"wg_new_name": "phone"}}),
    )
    .unwrap();
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    assert_eq!(field(&f, "wgdev.0")["value"], "10.66.0.2/32");
    ipc::send(&mut w, &json!({"cmd": "edit_action", "id": "a1", "action": "wgdev.conf.0", "values": {}})).unwrap();
    let t = expect(&rx, 5, |v| v["ev"] == "show_text").expect("config");
    assert!(t["text"].as_str().unwrap().contains("Address = 10.66.0.2/32"));
    expect(&rx, 5, |v| v["ev"] == "form").expect("form after the config");
    ipc::send(&mut w, &json!({"cmd": "edit_close"})).unwrap();

    // an outbound: the type decides the fields; a VLESS block needs a UUID
    ipc::send(&mut w, &json!({"cmd": "edit_open", "id": "d"})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "form").expect("form");
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

/// one HTTP response on a local port (a subscription server); returns the URL
fn serve_once(body: String) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/sub", l.local_addr().unwrap().port());
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        if let Ok((mut s, _)) = l.accept() {
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        }
    });
    url
}

/// a TLS server for one handshake, with a self-signed certificate for `name`; returns its port
fn tls_once(name: &str) -> u16 {
    use std::sync::Arc;
    let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
    let cert = rcgen::CertificateParams::new(vec![name.to_string()]).unwrap().self_signed(&key).unwrap();
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()))
        .unwrap();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let cfg = Arc::new(cfg);
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let mut c = rustls::ServerConnection::new(cfg).unwrap();
            while c.is_handshaking() && c.complete_io(&mut s).is_ok() {}
        }
    });
    port
}

#[test]
fn share_links_and_subscriptions() {
    let _serial = serial();
    let home = std::env::temp_dir().join(format!("proxydock-links-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let settings = json!({
        "version": 3,
        "graph": {
            "inbounds": [],
            "outbounds": [{"id": "o", "name": "Direct", "outbound": {"type": "direct"}},
                          {"id": "s", "name": "Sub", "outbound": {"type": "subscription"}}],
        },
    });
    let path = home.join("config.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    let h = controller::spawn(path.clone()).unwrap();
    let (mut w, rx) = connect(&h, &h.token);
    expect(&rx, 5, |v| v["ev"] == "hello").expect("hello");
    let field =
        |form: &Value, key: &str| form["fields"].as_array().unwrap().iter().find(|f| f["key"] == key).cloned().unwrap();

    // a share link fills the outbound and, over a type name, the block's name
    ipc::send(&mut w, &json!({"cmd": "edit_open", "id": "o"})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    ipc::send(
        &mut w,
        &json!({"cmd": "edit_action", "id": "o", "action": "link.paste", "values": {"clipboard": "not a link"}}),
    )
    .unwrap();
    let n = expect(&rx, 5, |v| v["ev"] == "notice").expect("notice");
    assert_eq!(n["ok"], false);
    let link = "trojan://secret@example.com:8443?sni=a.example.com&type=ws&path=%2Fws#My%20Trojan";
    ipc::send(&mut w, &json!({"cmd": "edit_action", "id": "o", "action": "link.paste", "values": {"clipboard": link}}))
        .unwrap();
    let n = expect(&rx, 5, |v| v["ev"] == "notice").expect("notice");
    assert_eq!((n["ok"].as_bool(), n["text"].as_str()), (Some(true), Some("My Trojan")));
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    assert_eq!(field(&f, "outbound.type")["value"], "trojan");
    assert_eq!(field(&f, "outbound.transport_path")["value"], "/ws");
    assert_eq!(field(&f, "name")["value"], "My Trojan");
    ipc::send(&mut w, &json!({"cmd": "edit_save", "id": "o", "values": {}})).unwrap();
    expect(&rx, 10, |v| v["ev"] == "form_saved").expect("saved");
    assert_eq!(saved(&path)["graph"]["outbounds"][0]["outbound"]["sni"], "a.example.com");

    // "Detect from server": the name in the server's certificate becomes the SNI
    let tls = tls_once("proxy.example.net");
    ipc::send(&mut w, &json!({"cmd": "edit_open", "id": "o"})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    ipc::send(&mut w, &json!({"cmd": "edit_action", "id": "o", "action": "sni.detect",
                              "values": {"outbound.host": "127.0.0.1", "outbound.port": tls.to_string(), "outbound.sni": ""}})).unwrap();
    let n = expect(&rx, 15, |v| v["ev"] == "notice").expect("notice");
    assert_eq!(n["ok"], true, "{n}");
    assert!(n["text"].as_str().unwrap().contains("proxy.example.net"), "{n}");
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    assert_eq!(field(&f, "outbound.sni")["value"], "proxy.example.net");
    ipc::send(&mut w, &json!({"cmd": "edit_close"})).unwrap();

    // "Update now": the button is busy until the list arrives, then the servers are in the draft
    use base64::Engine;
    let list = "trojan://pw@a.com:443#A\nvless://u@b.com:1?type=kcp#bad\nss://YWVzLTI1Ni1nY206cGFzcw@1.2.3.4:8388#C\n";
    let url = serve_once(base64::engine::general_purpose::STANDARD.encode(list));
    ipc::send(&mut w, &json!({"cmd": "edit_open", "id": "s"})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    ipc::send(
        &mut w,
        &json!({"cmd": "edit_action", "id": "s", "action": "sub.update", "values": {"outbound.sub_url": url}}),
    )
    .unwrap();
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("busy form");
    assert_eq!(field(&f, "sub.update")["busy"], true);
    let n = expect(&rx, 25, |v| v["ev"] == "notice").expect("notice");
    assert_eq!(n["ok"], true, "{n}");
    assert!(n["text"].as_str().unwrap().contains('2') && n["text"].as_str().unwrap().contains('1'), "{n}");
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    assert!(field(&f, "sub.update").get("busy").is_none());
    assert!(field(&f, "outbound.sub_url")["desc"].as_str().unwrap().starts_with('2'));
    ipc::send(&mut w, &json!({"cmd": "edit_save", "id": "s", "values": {}})).unwrap();
    expect(&rx, 10, |v| v["ev"] == "form_saved").expect("saved");
    // (the servers are saved sealed, like passwords)
    let g = proxydock_core::config::load_from(&path).unwrap().graph;
    let servers: Value = serde_json::from_str(g["outbounds"][1]["outbound"]["sub_servers"].as_str().unwrap()).unwrap();
    assert_eq!(servers.as_array().unwrap().iter().map(|s| s["name"].as_str().unwrap()).collect::<Vec<_>>(), ["A", "C"]);
    h.quit(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn connection_check_and_port_check() {
    let _serial = serial();
    let Some(exe) = singbox() else {
        eprintln!("sing-box not found: skipped");
        return;
    };
    let home = std::env::temp_dir().join(format!("proxydock-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let busy = TcpListener::bind("127.0.0.1:0").unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    let dead = free_port();
    let settings = json!({
        "version": 3,
        "global": {"singbox_path": exe.display().to_string()},
        "graph": {
            "inbounds": [{"id": "a1", "name": "card", "inbound": {"type": "socks5", "host": "127.0.0.1", "port": busy_port}, "target": "u"}],
            "outbounds": [{"id": "u", "name": "Up", "outbound": {"type": "socks5", "host": "127.0.0.1", "port": dead}},
                          {"id": "b", "name": "Nothing", "outbound": {"type": "block"}}],
        },
    });
    let path = home.join("config.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    proxydock_core::singbox::guard::set_run_dir(home.join("run"));
    let h = controller::spawn(path.clone()).unwrap();
    let (mut w, rx) = connect(&h, &h.token);
    expect(&rx, 5, |v| v["ev"] == "hello").expect("hello");
    let field =
        |form: &Value, key: &str| form["fields"].as_array().unwrap().iter().find(|f| f["key"] == key).cloned().unwrap();

    // the card's port is taken by another program
    ipc::send(&mut w, &json!({"cmd": "edit_open", "id": "a1"})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    ipc::send(&mut w, &json!({"cmd": "edit_action", "id": "a1", "action": "port.check", "values": {}})).unwrap();
    let n = expect(&rx, 5, |v| v["ev"] == "notice").expect("notice");
    assert_eq!(n["ok"], false, "{n}");
    expect(&rx, 5, |v| v["ev"] == "form").expect("form after the port check");
    // the check: the upstream does not answer, and the inbound port is reported too
    ipc::send(&mut w, &json!({"cmd": "edit_action", "id": "a1", "action": "probe", "values": {}})).unwrap();
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("busy form");
    assert_eq!(field(&f, "probe")["busy"], true);
    let n = expect(&rx, 40, |v| v["ev"] == "notice").expect("notice");
    assert_eq!(n["ok"], false, "{n}");
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    let probe = field(&f, "probe");
    assert_eq!(probe["tone"], 2);
    let text = probe["desc"].as_str().unwrap();
    assert!(text.contains(&busy_port.to_string()) && text.lines().count() >= 2, "{text}");
    drop(busy);

    // a Block outbound: nothing to check
    ipc::send(&mut w, &json!({"cmd": "edit_open", "id": "b"})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    ipc::send(&mut w, &json!({"cmd": "edit_action", "id": "b", "action": "probe", "values": {}})).unwrap();
    let f = expect(&rx, 5, |v| v["ev"] == "form").expect("form");
    assert!(field(&f, "probe")["desc"].as_str().unwrap().contains("Block"));
    h.quit(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn undo_rename_duplicate_and_notices() {
    let _serial = serial();
    let home = std::env::temp_dir().join(format!("proxydock-undo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let port = free_port();
    // the card cannot start (no sing-box there): with no window open, the tray says so
    let settings = json!({
        "version": 3,
        "global": {"log_to_file": true, "singbox_path": home.join("missing.exe").display().to_string()},
        "graph": {
            // a plain-text password: saved sealed while loading, which is not an undo step
            "inbounds": [{"id": "c1", "name": "card", "autostart": true, "inbound": {"type": "socks5", "host": "127.0.0.1", "port": port, "auth_enabled": true, "username": "u", "password": "pw"}, "target": "d"}],
            "outbounds": [{"id": "d", "name": "Direct", "outbound": {"type": "direct"}}],
        },
    });
    let path = home.join("config.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    proxydock_core::singbox::guard::set_run_dir(home.join("run"));
    let tray: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let notify: controller::Notify = {
        let tray = tray.clone();
        std::sync::Arc::new(move |m: &str| tray.lock().unwrap().push(m.to_string()))
    };
    let h = controller::spawn_with(path.clone(), controller::Hooks { notify, ..Default::default() }).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while !tray.lock().unwrap().iter().any(|m| m.starts_with("'card' failed to start: ")) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        tray.lock().unwrap().iter().any(|m| m.starts_with("'card' failed to start: ")),
        "{:?}",
        tray.lock().unwrap()
    );

    let (mut w, rx) = connect(&h, &h.token);
    let hello = expect(&rx, 5, |v| v["ev"] == "hello").expect("hello");
    assert_eq!((hello["undo"].clone(), hello["redo"].clone()), (json!(false), json!(false)));
    let names = |p: &std::path::Path| -> Vec<String> {
        let g = saved(p)["graph"].clone();
        ["inbounds", "outbounds"]
            .iter()
            .flat_map(|k| g[*k].as_array().cloned().unwrap_or_default())
            .map(|n| n["name"].as_str().unwrap().to_string())
            .collect()
    };

    // rename: a settings change, shown in the open window (not the tray) and saved; one undo step
    let before = tray.lock().unwrap().len();
    ipc::send(&mut w, &json!({"cmd": "rename", "key": "d", "name": "  Exit  "})).unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "info" && v["message"] == "Saved proxy 'Exit'").is_some());
    assert!(expect(&rx, 5, |v| v["ev"] == "undo" && v["undo"] == true && v["redo"] == false).is_some());
    assert_eq!(names(&path), ["card", "Exit"]);
    assert_eq!(tray.lock().unwrap().len(), before);
    // the same name again: nothing changes, the window gets the canvas back
    ipc::send(&mut w, &json!({"cmd": "rename", "key": "d", "name": "Exit"})).unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "graph").is_some());

    // duplicate two blocks in one message: one step
    ipc::send(&mut w, &json!({"cmd": "duplicate", "keys": ["c1", "d"]})).unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "graph" && v["cards"].as_array().unwrap().len() == 2).is_some());
    assert_eq!(names(&path), ["card", "card copy", "Exit", "Exit copy"]);
    let copy = saved(&path)["graph"]["inbounds"][1].clone();
    assert_eq!(
        (copy["inbound"]["port"].clone(), copy["autostart"].clone(), copy["target"].clone()),
        (json!(port + 1), json!(false), Value::Null)
    );

    // undo twice, redo once
    ipc::send(&mut w, &json!({"cmd": "undo"})).unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "info" && v["message"] == "Undone").is_some());
    assert!(expect(&rx, 5, |v| v["ev"] == "undo" && v["undo"] == true && v["redo"] == true).is_some());
    assert_eq!(names(&path), ["card", "Exit"]);
    ipc::send(&mut w, &json!({"cmd": "undo"})).unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "undo" && v["undo"] == false && v["redo"] == true).is_some());
    assert_eq!(names(&path), ["card", "Direct"]);
    ipc::send(&mut w, &json!({"cmd": "redo"})).unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "info" && v["message"] == "Redone").is_some());
    assert!(expect(&rx, 5, |v| v["ev"] == "undo" && v["undo"] == true && v["redo"] == true).is_some());
    assert_eq!(names(&path), ["card", "Exit"]);
    // a new change drops what could be redone
    ipc::send(&mut w, &json!({"cmd": "delete", "keys": [], "links": [{"from": "c1", "out": 0, "to": "d", "in": 0}]}))
        .unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "undo" && v["undo"] == true && v["redo"] == false).is_some());
    assert_eq!(saved(&path)["graph"]["inbounds"][0]["target"], Value::Null);

    // "Save the log to a file": the settings changes are in it, one line each
    h.quit(Duration::from_secs(10));
    let text = std::fs::read_to_string(home.join("logs").join("proxydock.log")).unwrap();
    assert!(text.lines().any(|l| l.ends_with("] SET   Saved proxy 'Exit'")), "{text}");
    assert!(text.lines().any(|l| l.ends_with("] SET   Undone")), "{text}");
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn settings_page_and_latency() {
    let _serial = serial();
    let Some(exe) = singbox() else {
        eprintln!("sing-box not found: skipped");
        return;
    };
    let home = std::env::temp_dir().join(format!("proxydock-settings-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let settings = json!({
        "version": 3,
        "global": {"singbox_path": exe.display().to_string()},
        "graph": {
            "outbounds": [
                {"id": "d", "name": "Direct", "outbound": {"type": "direct"}},
                {"id": "x", "name": "Refuse", "outbound": {"type": "block"}},
                {"id": "s", "name": "Dead", "outbound": {"type": "socks5", "host": "127.0.0.1", "port": free_port()}},
            ],
        },
    });
    let path = home.join("config.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    proxydock_core::singbox::guard::set_run_dir(home.join("run"));
    let h = controller::spawn(path.clone()).unwrap();
    let (mut w, rx) = connect(&h, &h.token);
    let hello = expect(&rx, 5, |v| v["ev"] == "hello").expect("hello");
    let st = &hello["settings"];
    assert_eq!(st["values"]["allowed_domains"], "*");
    assert_eq!(st["values"]["ui_scale"], 1);
    assert_eq!(st["languages"][1], "한국어");
    assert!(st["engine"].as_str().unwrap().starts_with("sing-box "), "{}", st["engine"]);

    // latency: every outbound block but Block, "…" while measuring, then a time or ✕
    ipc::send(&mut w, &json!({"cmd": "latency", "keys": []})).unwrap();
    let busy = expect(&rx, 5, |v| v["ev"] == "latency" && !v["latency"]["progress"].as_str().unwrap().is_empty())
        .expect("busy");
    assert_eq!(busy["latency"]["badges"]["d"][0], "…");
    assert_eq!(busy["latency"]["badges"]["s"][0], "…");
    assert!(busy["latency"]["badges"].get("x").is_none());
    // (a warning when some did not respond, as Python's bar)
    let done = expect(&rx, 60, |v| {
        (v["ev"] == "info" || v["ev"] == "warn") && v["message"].as_str().unwrap().contains("not responding")
    })
    .expect("finished");
    assert_eq!(done["ev"], "warn", "the SOCKS5 block has no server: one did not respond");
    eprintln!("{}", done["message"]);
    let last = expect(&rx, 5, |v| v["ev"] == "latency").expect("badges");
    assert_eq!(last["latency"]["progress"], "");
    assert_eq!(last["latency"]["badges"]["s"], json!(["✕", 3]));
    let d = last["latency"]["badges"]["d"][0].as_str().unwrap().to_string();
    eprintln!("Direct: {d}");
    assert!(d.ends_with(" ms") || d == "✕");
    // the block changed: its result is no longer shown
    ipc::send(&mut w, &json!({"cmd": "edit_open", "id": "s"})).unwrap();
    ipc::send(&mut w, &json!({"cmd": "edit_save", "id": "s", "values": {"outbound.port": "1"}})).unwrap();
    let after = expect(&rx, 5, |v| v["ev"] == "latency").expect("after");
    assert_eq!(after["latency"]["badges"]["d"], last["latency"]["badges"]["d"]);
    assert!(after["latency"]["badges"].get("s").is_none(), "{after}");
    // refused: no allowed domain, a DNS server that is not one
    let mut vals = st["values"].clone();
    vals["allowed_domains"] = json!(" ; ");
    ipc::send(&mut w, &json!({"cmd": "settings_save", "values": vals})).unwrap();
    assert!(
        expect(&rx, 5, |v| v["ev"] == "error" && v["message"].as_str().unwrap().starts_with("Allowed domains: "))
            .is_some()
    );
    vals["allowed_domains"] = json!("a.com; *.b.com ,");
    vals["dns_server"] = json!("ftp://x");
    ipc::send(&mut w, &json!({"cmd": "settings_save", "values": vals})).unwrap();
    assert!(
        expect(&rx, 5, |v| v["ev"] == "error" && v["message"].as_str().unwrap().starts_with("DNS server: ")).is_some()
    );

    // saved: Korean, a bigger window, quit with X
    vals["dns_server"] = json!(" 1.1.1.1 ,tls://dns.google");
    vals["language"] = json!(1);
    vals["ui_scale"] = json!(3);
    vals["close_exits"] = json!(true);
    vals["timeout"] = json!(300);
    ipc::send(&mut w, &json!({"cmd": "settings_save", "values": vals})).unwrap();
    assert!(
        expect(&rx, 5, |v| v["ev"] == "info" && v["message"] == "Settings saved (applies after restart: App scale)")
            .is_some()
    );
    assert!(expect(&rx, 5, |v| v["ev"] == "language" && v["language"] == "ko").is_some());
    let s2 = expect(&rx, 5, |v| v["ev"] == "settings").expect("settings");
    assert_eq!(s2["settings"]["values"]["allowed_domains"], "a.com, *.b.com");
    assert_eq!(s2["settings"]["values"]["dns_server"], "1.1.1.1, tls://dns.google");
    assert_eq!(s2["settings"]["themes"][0], "기기 설정");
    let g = saved(&path)["global"].clone();
    assert_eq!(
        (
            g["language"].clone(),
            g["ui_scale"].clone(),
            g["close_exits"].clone(),
            g["timeout"].clone(),
            g["dns_server"].clone()
        ),
        (json!("ko"), json!(125), json!(true), json!(300), json!("1.1.1.1"))
    );
    assert_eq!(saved(&path)["global"]["dns_extra"], json!(["tls://dns.google"]));
    assert_eq!(h.setting("ui_scale"), json!(125));

    h.quit(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn statistics_and_connection_test() {
    let _serial = serial();
    let Some(exe) = singbox() else {
        eprintln!("sing-box not found: skipped");
        return;
    };
    let home = std::env::temp_dir().join(format!("proxydock-conn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let port = free_port();
    let settings = json!({
        "version": 3,
        "global": {"singbox_path": exe.display().to_string()},
        "graph": {
            "inbounds": [{"id": "c", "name": "card", "autostart": true, "inbound": {"type": "http", "host": "127.0.0.1", "port": port}, "target": "d"}],
            "outbounds": [{"id": "d", "name": "Direct", "outbound": {"type": "direct"}}],
        },
    });
    let path = home.join("config.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    proxydock_core::singbox::guard::set_run_dir(home.join("run"));
    let h = controller::spawn(path.clone()).unwrap();
    let (mut w, rx) = connect(&h, &h.token);
    let hello = expect(&rx, 5, |v| v["ev"] == "hello").expect("hello");
    assert_eq!(hello["conn"]["sites"].as_array().unwrap().len(), 21);
    assert_eq!(hello["conn"]["sites"][0], json!(["Global", "Google", "www.google.com", false]));
    assert_eq!(hello["history"]["meta"]["exits"][0], json!(["d", "Direct", ""]));

    // sites: refused, added, already there, removed
    ipc::send(&mut w, &json!({"cmd": "test_site_add", "text": "localhost"})).unwrap();
    assert!(
        expect(&rx, 5, |v| v["ev"] == "error" && v["message"].as_str().unwrap().starts_with("Cannot add: ")).is_some()
    );
    ipc::send(&mut w, &json!({"cmd": "test_site_add", "text": "https://Example.COM/x"})).unwrap();
    let sites = expect(&rx, 5, |v| v["ev"] == "conn_sites").expect("sites");
    assert_eq!(sites["conn"]["sites"][21], json!(["Added", "example.com", "example.com", true]));
    assert_eq!(saved(&path)["global"]["test_sites"], json!(["example.com"]));
    ipc::send(&mut w, &json!({"cmd": "test_site_add", "text": "www.google.com"})).unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "info" && v["message"] == "Already in the list: www.google.com").is_some());
    ipc::send(&mut w, &json!({"cmd": "test_site_remove", "host": "example.com"})).unwrap();
    let sites = expect(&rx, 5, |v| v["ev"] == "conn_sites").expect("sites");
    assert_eq!(sites["conn"]["sites"].as_array().unwrap().len(), 21);

    // the card runs: each second a point with every card, the total and the exit blocks
    let until = Instant::now() + Duration::from_secs(10);
    while !h.cards().iter().any(|c| c.state == proxydock_core::manager::State::Running) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    let st = expect(&rx, 5, |v| v["ev"] == "stats").expect("stats");
    assert!(st["point"]["cards"]["c"].is_array() && st["point"]["cards"][""].is_array(), "{st}");
    assert!(st["stats"][""]["tcp"].is_number());

    // the connection test through the card: every site answers or fails, then the summary
    ipc::send(&mut w, &json!({"cmd": "conn_test", "id": "c"})).unwrap();
    let start = expect(&rx, 10, |v| v["ev"] == "conn_start").expect("start");
    assert_eq!(start["total"], 21);
    let mut rows = std::collections::HashSet::new();
    let done = loop {
        let v = expect(&rx, 60, |v| v["ev"] == "conn_result" || v["ev"] == "conn_done").expect("results");
        if v["ev"] == "conn_done" {
            break v;
        }
        rows.insert(v["row"].as_u64().unwrap());
    };
    assert_eq!(rows.len(), 21);
    assert_eq!(done["error"], "");
    eprintln!("{}", done["summary"]);
    assert!(done["summary"].as_str().unwrap().starts_with("Succeeded "));
    h.quit(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&home);
}

#[cfg(windows)]
#[test]
fn pc_block_and_start_at_login() {
    use proxydock_core::platform;
    let _serial = serial();
    let Some(exe) = singbox() else {
        eprintln!("sing-box not found: skipped");
        return;
    };
    let home = std::env::temp_dir().join(format!("proxydock-pc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let port = free_port();
    // the PC's own settings before ProxyDock
    let mut before = serde_json::Map::new();
    before.insert("ProxyEnable".into(), json!(0));
    before.insert("ProxyServer".into(), json!("old.example:3128"));
    platform::set_system_proxy(&before).unwrap();
    let settings = json!({
        "version": 3,
        "global": {"singbox_path": exe.display().to_string()},
        "graph": {
            "inbounds": [
                {"id": "h", "name": "web", "autostart": true, "inbound": {"type": "http", "host": "127.0.0.1", "port": port}, "target": "d"},
                {"id": "s", "name": "socks", "inbound": {"type": "socks5", "host": "127.0.0.1", "port": free_port()}, "target": "d"},
            ],
            "outbounds": [{"id": "d", "name": "Direct", "outbound": {"type": "direct"}}],
            "pc": {"inbound": "h"},
        },
    });
    let path = home.join("config.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    proxydock_core::singbox::guard::set_run_dir(home.join("run"));
    let h = controller::spawn(path.clone()).unwrap();
    let (mut w, rx) = connect(&h, &h.token);
    expect(&rx, 5, |v| v["ev"] == "hello").expect("hello");

    // the card runs: the system proxy is ours, the PC's own values kept as the backup
    let server = format!("127.0.0.1:{port}");
    let until = Instant::now() + Duration::from_secs(10);
    while platform::get_system_proxy().unwrap()["ProxyServer"] != json!(server) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    let now = platform::get_system_proxy().unwrap();
    assert_eq!((now["ProxyEnable"].clone(), now["ProxyServer"].clone()), (json!(1), json!(server)));
    assert_eq!(now["ProxyOverride"], json!("localhost;127.*;<local>"));
    // (the file is saved just after the registry changes)
    let until = Instant::now() + Duration::from_secs(5);
    while saved(&path)["graph"]["pc"]["applied"] != json!(true) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    let pc = saved(&path)["graph"]["pc"].clone();
    assert_eq!((pc["applied"].clone(), pc["backup"]["ProxyServer"].clone()), (json!(true), json!("old.example:3128")));
    assert_eq!(h.pc_card().as_deref(), Some("web"));

    // the card stops: the PC's own settings come back
    ipc::send(&mut w, &json!({"cmd": "stop", "id": "h"})).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while platform::get_system_proxy().unwrap()["ProxyServer"] != json!("old.example:3128") && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(platform::get_system_proxy().unwrap()["ProxyEnable"], json!(0));
    let until = Instant::now() + Duration::from_secs(5);
    while saved(&path)["graph"]["pc"]["applied"] != json!(false) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(saved(&path)["graph"]["pc"]["applied"], json!(false));

    // a SOCKS5 card cannot be the system proxy: said so
    ipc::send(&mut w, &json!({"cmd": "connect", "from": "pc", "out": 0, "to": "s", "in": 0})).unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "error" && v["message"].as_str().unwrap().contains("HTTP")).is_some());
    // "turn off": disconnected and the proxy off
    ipc::send(&mut w, &json!({"cmd": "pc_off"})).unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "log" && v["entries"][0]["message"] == "System proxy turned off").is_some());
    assert!(expect(&rx, 5, |v| v["ev"] == "graph").is_some());
    assert_eq!(saved(&path)["graph"]["pc"]["inbound"], Value::Null);

    // start at login: on, hidden, off (the test Run key)
    ipc::send(&mut w, &json!({"cmd": "startup", "on": true})).unwrap();
    let st = expect(&rx, 5, |v| v["ev"] == "settings" && v["settings"]["startup"]["registered"] == true)
        .expect("registered");
    assert_eq!(st["settings"]["startup"]["hidden"], false);
    let me = std::env::current_exe().unwrap();
    assert_eq!(platform::startup_command(), Some(format!("\"{}\"", me.display())));
    ipc::send(&mut w, &json!({"cmd": "startup_hidden", "on": true})).unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "settings" && v["settings"]["startup"]["hidden"] == true).is_some());
    assert!(platform::startup_command().unwrap().ends_with(" --hidden"));
    ipc::send(&mut w, &json!({"cmd": "startup", "on": false})).unwrap();
    assert!(expect(&rx, 5, |v| v["ev"] == "settings" && v["settings"]["startup"]["registered"] == false).is_some());
    assert_eq!(platform::startup_command(), None);

    h.quit(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&home);
    unsafe {
        let w: Vec<u16> = r"Software\ProxyDockTest".encode_utf16().chain(Some(0)).collect();
        windows_sys::Win32::System::Registry::RegDeleteTreeW(
            windows_sys::Win32::System::Registry::HKEY_CURRENT_USER,
            w.as_ptr(),
        );
    }
}

#[test]
fn tray_switches_and_group_members() {
    let _serial = serial();
    let home = std::env::temp_dir().join(format!("proxydock-quick-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let settings = json!({
        "version": 3,
        "graph": {
            "inbounds": [{"id": "c", "name": "card", "inbound": {"type": "socks5", "host": "127.0.0.1", "port": free_port()}, "target": "k"}],
            "routers": [{"id": "k", "name": "Kill", "style": "if", "switch": true, "switch_on": false, "rules": [{"name": "then", "match": {}, "target": "g"}], "default": "d1"}],
            "outbounds": [
                {"id": "d1", "name": "One", "outbound": {"type": "direct"}},
                {"id": "d2", "name": "Two", "outbound": {"type": "direct"}},
                {"id": "g", "name": "Pick", "outbound": {"type": "group", "group_mode": "select"}, "members": ["d1", "d2"]},
            ],
        },
    });
    let path = home.join("config.json");
    std::fs::write(&path, settings.to_string()).unwrap();
    let h = controller::spawn(path.clone()).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while h.quick().switches.is_empty() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    let q = h.quick();
    assert_eq!(q.switches, vec![("k".to_string(), "Kill".to_string(), false)]);
    assert_eq!(q.groups.len(), 1);
    assert_eq!((q.groups[0].1.as_str(), q.groups[0].2.len(), q.groups[0].3.as_str()), ("Pick", 2, "d1"));

    // from the tray: the switch on, the other member
    h.tray(json!({"cmd": "set_switch", "id": "k", "on": true}));
    h.tray(json!({"cmd": "select_member", "group": "g", "member": "d2"}));
    let until = Instant::now() + Duration::from_secs(5);
    while (h.quick().switches.first().map(|s| s.2) != Some(true) || h.quick().groups[0].3 != "d2")
        && Instant::now() < until
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    let g = saved(&path)["graph"].clone();
    assert_eq!(g["routers"][0]["switch_on"], json!(true));
    assert_eq!(g["outbounds"][2]["outbound"]["group_selected"], json!("d2"));
    // not a member: nothing changes
    h.tray(json!({"cmd": "select_member", "group": "g", "member": "c"}));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(saved(&path)["graph"]["outbounds"][2]["outbound"]["group_selected"], json!("d2"));
    h.quit(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn settings_v2_become_a_graph() {
    let _serial = serial();
    let home = std::env::temp_dir().join(format!("proxydock-v2-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    // two proxies through the same upstream: one shared outbound block
    let up = json!({"type": "socks5", "host": "10.0.0.1", "port": 1080});
    let v2 = json!({"version": 2, "global": {"language": "en"}, "proxies": [
        {"id": "aaaaaaaa", "name": "first", "autostart": false, "inbound": {"type": "socks5", "port": free_port()}, "outbound": up},
        {"id": "bbbbbbbb", "name": "second", "inbound": {"type": "http", "port": free_port()}, "outbound": up},
    ]});
    let path = home.join("config.json");
    std::fs::write(&path, v2.to_string()).unwrap();
    let h = controller::spawn(path.clone()).unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while saved(&path)["version"] != json!(3) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    let s = saved(&path);
    assert_eq!(s["version"], json!(3));
    let g = &s["graph"];
    assert_eq!(g["inbounds"].as_array().unwrap().len(), 2);
    assert_eq!(g["outbounds"].as_array().unwrap().len(), 1);
    let shared = g["outbounds"][0]["id"].clone();
    assert_eq!(
        (g["inbounds"][0]["id"].clone(), g["inbounds"][0]["target"].clone()),
        (json!("aaaaaaaa"), shared.clone())
    );
    assert_eq!(g["inbounds"][1]["target"], shared);
    // the v2 file kept for an older ProxyDock
    let kept: Value = serde_json::from_str(&std::fs::read_to_string(home.join("config.v2.json")).unwrap()).unwrap();
    assert_eq!(kept["version"], json!(2));
    assert_eq!(h.cards().len(), 2);
    h.quit(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn update_check_finds_a_newer_release() {
    // (only debug builds read the release from PROXYDOCK_UPDATE_URL; a release build asks GitHub itself)
    if !cfg!(debug_assertions) {
        eprintln!("release build: skipped");
        return;
    }
    let _serial = serial();
    let home = std::env::temp_dir().join(format!("proxydock-upd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let path = home.join("config.json");
    // no automatic check at startup in this test (switched off)
    std::fs::write(&path, json!({"version": 3, "global": {"check_updates": false}, "graph": {}}).to_string()).unwrap();
    let release = json!({"tag_name": "v99.0.0", "name": "ProxyDock 99", "body": "**New**", "html_url": "https://github.com/x/releases/v99",
                         "assets": [{"name": "ProxyDock-99.0.0-windows-x64-setup.exe", "browser_download_url": "https://dl/x", "size": 5242880}]});
    // (debug builds read the release from here instead of GitHub)
    unsafe { std::env::set_var("PROXYDOCK_UPDATE_URL", serve_once(release.to_string())) };
    let h = controller::spawn(path.clone()).unwrap();
    let (mut w, rx) = connect(&h, &h.token);
    let hello = expect(&rx, 5, |v| v["ev"] == "hello").expect("hello");
    assert_eq!(hello["update"]["release"], Value::Null);
    ipc::send(&mut w, &json!({"cmd": "update_check"})).unwrap();
    let u = expect(&rx, 10, |v| v["ev"] == "update" && v["update"]["release"].is_object()).expect("release");
    assert_eq!(u["update"]["release"]["version"], "99.0.0");
    assert_eq!(u["update"]["status"], "Version 99.0.0 is available");
    // this copy was not installed by the setup program: the release page instead of a download
    assert_eq!(u["update"]["release"]["asset"], Value::Null);
    assert!(expect(&rx, 5, |v| v["ev"] == "update_dialog").is_some());
    assert!(saved(&path)["global"]["update_checked"].as_i64().unwrap() > 0);
    unsafe { std::env::remove_var("PROXYDOCK_UPDATE_URL") };
    h.quit(Duration::from_secs(10));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn unreadable_settings_are_kept_aside() {
    // (Python `config.load`): a broken config.json is copied to config.json.broken and the app starts with defaults
    let _serial = serial();
    let home = std::env::temp_dir().join(format!("proxydock-broken-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let path = home.join("config.json");
    std::fs::write(&path, "{ this is not json").unwrap();
    proxydock_core::singbox::guard::set_run_dir(home.join("run"));
    let h = controller::spawn(path.clone()).unwrap();
    let (mut w, rx) = connect(&h, &h.token);
    let hello = expect(&rx, 5, |v| v["ev"] == "hello").expect("hello");
    assert!(
        hello["logs"].as_array().unwrap().iter().any(|l| l["message"].as_str().unwrap_or("").contains(".broken")),
        "{}",
        hello["logs"]
    );
    assert_eq!(std::fs::read_to_string(home.join("config.json.broken")).unwrap(), "{ this is not json");
    // it can save again: a new block is written over the broken file (its copy stays)
    ipc::send(&mut w, &json!({"cmd": "add", "what": "outbound", "type": "direct", "x": 0, "y": 0})).unwrap();
    expect(&rx, 5, |v| v["ev"] == "graph").expect("graph");
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["graph"]["outbounds"].as_array().unwrap().len(), 1);
    h.quit(std::time::Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&home);
}
