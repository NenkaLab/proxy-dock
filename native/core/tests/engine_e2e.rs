//! Cards on the real sing-box (bin/<platform>/ of the repository; skipped when it is not there, like the Python
//! tests). Everything runs on loopback in a temporary run folder.

use proxydock_core::log::Level;
use proxydock_core::singbox::process::{Engine, EngineEnv, Hooks, StartError};
use proxydock_core::singbox::{self, guard};
use proxydock_core::{config, graph};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn exe() -> Option<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    singbox::find(singbox::EXE, "", &[root])
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// a card from a tiny graph: inbound block -> outbound block
fn card(inbound: Value, outbound: Value) -> Value {
    let g = graph::validate(&json!({
        "inbounds": [{"id": "card1", "name": "test", "inbound": inbound, "target": "out1"}],
        "outbounds": [{"id": "out1", "outbound": outbound}],
    }));
    Value::Object(graph::compile(&g).remove(0))
}

struct Log(Arc<Mutex<Vec<(Level, String)>>>);

impl Log {
    fn hooks() -> (Hooks, Log, Arc<Mutex<Vec<String>>>) {
        let entries = Arc::new(Mutex::new(vec![]));
        let died = Arc::new(Mutex::new(vec![]));
        let (e, d) = (entries.clone(), died.clone());
        let hooks = Hooks {
            log: Arc::new(move |lv, m| e.lock().unwrap().push((lv, m.to_string()))),
            died: Arc::new(move |m| d.lock().unwrap().push(m)),
        };
        (hooks, Log(entries), died)
    }

    fn wait_for(&self, what: impl Fn(&(Level, String)) -> bool, secs: u64) -> bool {
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until {
            if self.0.lock().unwrap().iter().any(&what) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }
}

fn env(dir: &std::path::Path) -> EngineEnv {
    guard::set_run_dir(dir.to_path_buf());
    EngineEnv {
        exe: exe().unwrap(),
        run_dir: dir.to_path_buf(),
        bans_path: guard::bans_path(),
        rulesets: vec![],
        cert: None,
        exclude_paths: vec![],
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("proxydock-e2e-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// a TCP server that answers every line with "echo: <line>"
fn echo_server() -> u16 {
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

/// SOCKS5 CONNECT with username / password; Ok(stream) when the proxy accepted
fn socks5(proxy: u16, user: &str, pass: &str, dest_port: u16) -> std::io::Result<TcpStream> {
    let mut s = TcpStream::connect(("127.0.0.1", proxy))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.write_all(&[5, 1, 2])?;
    let mut r = [0u8; 2];
    s.read_exact(&mut r)?;
    let mut auth = vec![1, user.len() as u8];
    auth.extend_from_slice(user.as_bytes());
    auth.push(pass.len() as u8);
    auth.extend_from_slice(pass.as_bytes());
    s.write_all(&auth)?;
    s.read_exact(&mut r)?;
    if r[1] != 0 {
        return Err(std::io::Error::other("auth refused"));
    }
    s.write_all(&[5, 1, 0, 1, 127, 0, 0, 1, (dest_port >> 8) as u8, dest_port as u8])?;
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep)?;
    if rep[1] != 0 {
        return Err(std::io::Error::other(format!("connect refused ({})", rep[1])));
    }
    Ok(s)
}

#[test]
fn socks5_card_relays_counts_and_logs() {
    if exe().is_none() {
        eprintln!("sing-box not found: skipped");
        return;
    }
    let dir = temp_dir("socks");
    let port = free_port();
    let echo = echo_server();
    let p = card(
        json!({"type": "socks5", "host": "127.0.0.1", "port": port, "auth_enabled": true, "username": "u", "password": "pw"}),
        json!({"type": "direct"}),
    );
    let glob = config::validate_global(&json!({"log_close": true}));
    let (hooks, log, died) = Log::hooks();
    let mut engine = Engine::start(&p, &glob, &env(&dir), hooks).expect("start");
    assert!(dir.join("card1.json").is_file());

    // a relayed connection: data both ways, a TCP log line
    let mut s = socks5(port, "u", "pw", echo).expect("socks5 connect");
    s.write_all(&vec![b'x'; 50_000]).unwrap();
    let mut got = vec![0u8; 50_006];
    s.read_exact(&mut got).unwrap();
    assert!(got.starts_with(b"echo: "));
    assert!(log.wait_for(|(lv, m)| *lv == Level::Tcp && m.contains(&format!("-> 127.0.0.1:{echo}")), 5), "{:?}", log.0.lock().unwrap());

    // traffic counted from the Clash API (polled once a second)
    let until = Instant::now() + Duration::from_secs(5);
    while engine.stats().up < 50_000 && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(100));
    }
    let st = engine.stats();
    assert!(st.up >= 50_000 && st.down >= 50_000, "{st:?}");
    assert!(st.total >= 1);
    assert_eq!(engine.exit_totals().get("out1").map(|t| t.0 >= 50_000.0), Some(true), "{:?}", engine.exit_totals());

    // closing it: a close line (log_close)
    drop(s);
    assert!(log.wait_for(|(lv, m)| *lv == Level::Tcp && m.contains("closed"), 5), "{:?}", log.0.lock().unwrap());

    // a wrong password: an AUTH entry naming the user, and the card's last error
    assert!(socks5(port, "mallory", "nope", echo).is_err());
    assert!(log.wait_for(|(lv, m)| *lv == Level::Auth && m.contains("'mallory'"), 5), "{:?}", log.0.lock().unwrap());
    assert!(engine.last_error().contains("mallory"));

    engine.stop();
    assert!(!dir.join("card1.json").exists());
    assert!(TcpStream::connect(("127.0.0.1", port)).is_err(), "port still open after stop");
    assert!(died.lock().unwrap().is_empty(), "a stop is not a death");
}

#[test]
fn port_in_use_and_death_are_reported() {
    if exe().is_none() {
        eprintln!("sing-box not found: skipped");
        return;
    }
    let dir = temp_dir("busy");
    let busy = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = busy.local_addr().unwrap().port();
    let p = card(json!({"type": "http", "host": "127.0.0.1", "port": port}), json!({"type": "direct"}));
    let glob = config::validate_global(&json!({}));
    let (hooks, _, _) = Log::hooks();
    match Engine::start(&p, &glob, &env(&dir), hooks) {
        Err(StartError::PortInUse(_)) => {}
        Err(other) => panic!("expected port in use, got {other:?}"),
        Ok(_) => panic!("started on a busy port"),
    }
    drop(busy);

    // sing-box killed from outside: the died hook hears about it
    let port = free_port();
    let p = card(json!({"type": "http", "host": "127.0.0.1", "port": port}), json!({"type": "direct"}));
    let (hooks, log, died) = Log::hooks();
    let engine = Engine::start(&p, &glob, &env(&dir), hooks).expect("start");
    let cfg = dir.join("card1.json");
    let pid_filter = format!("{}", cfg.display());
    // find this card's sing-box by its config path on the command line and end it
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-Command", &format!(
            "Get-CimInstance Win32_Process -Filter \"Name='sing-box.exe'\" | Where-Object {{ $_.CommandLine -like '*{}*' }} | ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force }}",
            pid_filter.replace('\'', "''"))])
        .output()
        .unwrap();
    assert!(out.status.success());
    let until = Instant::now() + Duration::from_secs(5);
    while died.lock().unwrap().is_empty() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(died.lock().unwrap().len(), 1, "{:?}", log.0.lock().unwrap());
    assert!(died.lock().unwrap()[0].contains("sing-box"));
    drop(engine);
}
