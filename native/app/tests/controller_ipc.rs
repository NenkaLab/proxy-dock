//! The controller with a settings file of two cards, talked to like the window does (needs bin/<platform>/sing-box).

use proxydock::{controller, ipc};
use serde_json::{json, Value};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

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
