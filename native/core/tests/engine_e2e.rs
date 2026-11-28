//! Cards on the real sing-box (bin/<platform>/ of the repository; skipped when it is not there, like the Python
//! tests). Everything runs on loopback in a temporary run folder.

use proxydock_core::log::Level;
use proxydock_core::singbox::process::{Engine, EngineEnv, Hooks, StartError};
use proxydock_core::singbox::{self, guard};
use proxydock_core::{config, graph};
use serde_json::{Value, json};
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

/// The run folder (and the bans file in it) is process-wide: the tests of this file take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
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
        helper: None,
        bypass: vec![],
        app_on: true,
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
    let _serial = serial();
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
    assert!(
        log.wait_for(|(lv, m)| *lv == Level::Tcp && m.contains(&format!("-> 127.0.0.1:{echo}")), 5),
        "{:?}",
        log.0.lock().unwrap()
    );

    // the way it took: the outbound sing-box chose, mapped to the exit block (a log line -> the canvas)
    let who = log
        .0
        .lock()
        .unwrap()
        .iter()
        .find(|(lv, m)| *lv == Level::Tcp && m.contains(&format!("-> 127.0.0.1:{echo}")))
        .map(|(_, m)| m.split(" -> ").next().unwrap().to_string())
        .unwrap();
    let target = format!("127.0.0.1:{echo}");
    let until = Instant::now() + Duration::from_secs(5);
    while engine.route_of(&who, Some(&target)).is_none() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(engine.route_of(&who, Some(&target)).map(|w| w.0).as_deref(), Some("out1"));
    assert_eq!(engine.route_of(&who, None).map(|w| w.0).as_deref(), Some("out1"));
    assert_eq!(engine.route_of("10.9.9.9:1", None), None);

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
    let _serial = serial();
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
    kill_by_command_line(&pid_filter);
    let until = Instant::now() + Duration::from_secs(5);
    while died.lock().unwrap().is_empty() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(died.lock().unwrap().len(), 1, "{:?}", log.0.lock().unwrap());
    assert!(died.lock().unwrap()[0].contains("sing-box"));
    drop(engine);
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

fn card_graph(g: Value) -> Value {
    Value::Object(graph::compile(&graph::validate(&g)).remove(0))
}

#[test]
fn https_card_with_self_signed_cert() {
    let _serial = serial();
    if exe().is_none() {
        eprintln!("sing-box not found: skipped");
        return;
    }
    let dir = temp_dir("https");
    let port = free_port();
    let p = card(json!({"type": "https", "host": "127.0.0.1", "port": port}), json!({"type": "direct"}));
    let cert = proxydock_core::tls::server_cert_paths(&p, &dir.join("certs")).unwrap();
    assert!(cert.0.ends_with("proxy-card1.crt"));
    let glob = config::validate_global(&json!({}));
    let mut e = env(&dir);
    e.cert = Some(cert);
    let (hooks, log, _) = Log::hooks();
    let mut engine =
        Engine::start(&p, &glob, &e, hooks).unwrap_or_else(|err| panic!("{err:?} {:?}", log.0.lock().unwrap()));
    assert!(TcpStream::connect(("127.0.0.1", port)).is_ok());
    engine.stop();
}

#[test]
fn router_geoip_rule_downloads_its_rule_set() {
    let _serial = serial();
    if exe().is_none() {
        eprintln!("sing-box not found: skipped");
        return;
    }
    let dir = temp_dir("geo");
    let port = free_port();
    let p = card_graph(json!({
        "inbounds": [{"id": "card1", "name": "geo", "inbound": {"type": "http", "host": "127.0.0.1", "port": port}, "target": "r1"}],
        "outbounds": [{"id": "out1", "outbound": {"type": "direct"}}, {"id": "out2", "outbound": {"type": "block"}}],
        "routers": [{"id": "r1", "rules": [{"id": "x", "match": {"geoip": ["cn"]}, "target": "out2"}], "default": "out1"}],
    }));
    let need = proxydock_core::singbox::rulesets::needed(&p);
    assert_eq!(need, vec![("geoip".to_string(), "cn".to_string())]);
    let rs_dir = dir.join("rulesets");
    let rs = match proxydock_core::singbox::rulesets::ensure_in(
        &rs_dir,
        &need,
        &proxydock_core::singbox::rulesets::http_get,
        &|_| {},
    ) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("no network ({err}): skipped");
            return;
        }
    };
    assert_eq!(rs[0].0, "geoip-cn");
    assert!(std::fs::read(&rs[0].1).unwrap().starts_with(b"SRS"));
    let glob = config::validate_global(&json!({}));
    let mut e = env(&dir);
    e.rulesets = rs;
    let (hooks, log, _) = Log::hooks();
    let mut engine =
        Engine::start(&p, &glob, &e, hooks).unwrap_or_else(|err| panic!("{err:?} {:?}", log.0.lock().unwrap()));
    let conf = std::fs::read_to_string(dir.join("card1.json")).unwrap();
    assert!(conf.contains("geoip-cn"), "{conf}");
    engine.stop();
}

/// sing-box 1.15+ for the pinning test: PROXYDOCK_SINGBOX_115 (a path), else none (skipped)
fn exe_115() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("PROXYDOCK_SINGBOX_115")?);
    let v = singbox::version::of(&p)?;
    v.has_certificate_pin().then_some(p)
}

fn der_sha256_hex(pem_path: &str) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};
    let pem = std::fs::read_to_string(pem_path).unwrap();
    let b64: String = pem.lines().filter(|l| !l.starts_with("-----")).collect();
    let der = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
    Sha256::digest(der).iter().map(|b| format!("{b:02x}")).collect()
}

/// Hysteria2 server card with a self-signed certificate; a SOCKS5 client card whose Hysteria2 outbound pins it.
/// Ok when a relayed connection through the client card worked.
fn pinned_hy2(exe: &std::path::Path, pin: &str, verify: bool) -> Result<(), String> {
    let dir = temp_dir(&format!("pin-{}-{verify}", &pin[..4]));
    let (sport, cport, echo) = (free_udp_port(), free_port(), echo_server());
    let server = card(
        json!({"type": "hysteria2", "host": "127.0.0.1", "port": sport, "hy2_password": "hpw"}),
        json!({"type": "direct"}),
    );
    let cert = proxydock_core::tls::server_cert_paths(&server, &dir.join("certs")).unwrap();
    let pin = if pin == "RIGHT" { der_sha256_hex(&cert.0) } else { pin.to_string() };
    let glob = config::validate_global(&json!({}));
    let mut e = env(&dir);
    e.exe = exe.to_path_buf();
    e.cert = Some(cert);
    let (h1, l1, _) = Log::hooks();
    let mut s =
        Engine::start(&server, &glob, &e, h1).map_err(|x| format!("server {x:?} {:?}", l1.0.lock().unwrap()))?;
    let client = card_graph(json!({
        "inbounds": [{"id": "card2", "name": "c", "inbound": {"type": "socks5", "host": "127.0.0.1", "port": cport}, "target": "o"}],
        "outbounds": [{"id": "o", "outbound": {"type": "hysteria2", "host": "127.0.0.1", "port": sport, "hy2_password": "hpw",
            "sni": "localhost", "pin_sha256": pin, "tls_verify": verify}}],
    }));
    let mut e2 = env(&dir);
    e2.exe = exe.to_path_buf();
    let (h2, l2, _) = Log::hooks();
    let mut c = Engine::start(&client, &glob, &e2, h2).map_err(|x| format!("client {x:?}"))?;
    let conf = std::fs::read_to_string(dir.join("card2.json")).unwrap();
    assert!(conf.contains("certificate_sha256"), "{conf}");
    let r = (|| -> Result<(), String> {
        let mut st = socks5_noauth(cport, echo).map_err(|x| x.to_string())?;
        st.write_all(b"hi").map_err(|x| x.to_string())?;
        let mut got = [0u8; 8];
        st.read_exact(&mut got).map_err(|x| x.to_string())?;
        (&got == b"echo: hi").then_some(()).ok_or("wrong reply".into())
    })();
    let _ = l2;
    c.stop();
    s.stop();
    r
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn socks5_noauth(proxy: u16, dest_port: u16) -> std::io::Result<TcpStream> {
    let mut s = TcpStream::connect(("127.0.0.1", proxy))?;
    s.set_read_timeout(Some(Duration::from_secs(8)))?;
    s.write_all(&[5, 1, 0])?;
    let mut r = [0u8; 2];
    s.read_exact(&mut r)?;
    s.write_all(&[5, 1, 0, 1, 127, 0, 0, 1, (dest_port >> 8) as u8, dest_port as u8])?;
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep)?;
    if rep[1] != 0 {
        return Err(std::io::Error::other(format!("connect refused ({})", rep[1])));
    }
    Ok(s)
}

#[test]
fn certificate_pin_on_sing_box_115() {
    let _serial = serial();
    let Some(exe) = exe_115() else {
        eprintln!("no sing-box 1.15 (PROXYDOCK_SINGBOX_115): skipped");
        return;
    };
    let wrong = "00".repeat(32);
    for verify in [true, false] {
        // the pin replaces CA checks (a self-signed certificate passes) and holds even with verification off
        assert_eq!(pinned_hy2(&exe, "RIGHT", verify), Ok(()), "right pin, tls_verify={verify}");
        assert!(pinned_hy2(&exe, &wrong, verify).is_err(), "wrong pin accepted, tls_verify={verify}");
    }
}

#[test]
fn pinned_cards_need_sing_box_115() {
    let _serial = serial();
    let Some(old) = exe() else { return };
    let pinned = json!({"type": "hysteria2", "host": "192.0.2.1", "port": 443, "hy2_password": "x", "pin_sha256": "ab".repeat(32)});
    // pinned in the card's own chain, and only behind a router rule
    let own = card(json!({"type": "socks5", "port": 1080}), pinned.clone());
    let routed = card_graph(json!({
        "inbounds": [{"id": "card1", "inbound": {"type": "socks5", "port": 1080}, "target": "r"}],
        "outbounds": [{"id": "d", "outbound": {"type": "direct"}}, {"id": "p", "outbound": pinned}],
        "routers": [{"id": "r", "rules": [{"id": "x", "match": {"domain_suffix": ["example.com"]}, "target": "p"}], "default": "d"}],
    }));
    let plain = card(json!({"type": "socks5", "port": 1080}), json!({"type": "direct"}));
    let glob = |path: &PathBuf| config::validate_global(&json!({"singbox_path": path.display().to_string()}));
    let has_pin = singbox::version::of(&old).unwrap().has_certificate_pin();
    for p in [&own, &routed] {
        assert!(singbox::uses_certificate_pin(p));
        assert_eq!(singbox::engine_for(p, &glob(&old)).is_ok(), has_pin);
    }
    assert!(!singbox::uses_certificate_pin(&plain));
    assert!(singbox::engine_for(&plain, &glob(&old)).is_ok());
    if let Some(new) = exe_115() {
        assert_eq!(
            singbox::engine_for(&routed, &glob(&new)).map(|(k, e)| (k, e == new)),
            Ok((singbox::Kind::SingBox, true))
        );
    }
}

/// "Detect from server" over QUIC: a Hysteria2 server card's self-signed certificate is read (names, fingerprint)
#[test]
fn detect_sni_over_quic() {
    let _serial = serial();
    let Some(exe) = exe() else {
        eprintln!("sing-box not found: skipped");
        return;
    };
    let dir = temp_dir("detect");
    let sport = free_udp_port();
    let server = card(
        json!({"type": "hysteria2", "host": "127.0.0.1", "port": sport, "hy2_password": "hpw"}),
        json!({"type": "direct"}),
    );
    let cert = proxydock_core::tls::server_cert_paths(&server, &dir.join("certs")).unwrap();
    let want = der_sha256_hex(&cert.0);
    let mut e = env(&dir);
    e.exe = exe;
    e.cert = Some(cert);
    let (h, _, _) = Log::hooks();
    let mut s = Engine::start(&server, &config::validate_global(&json!({})), &e, h).expect("server");
    let d =
        proxydock_core::certinfo::detect(&json!({"type": "hysteria2", "host": "127.0.0.1", "port": sport, "sni": ""}));
    s.stop();
    let d = d.expect("certificate");
    assert_eq!(d.names, ["localhost"]);
    assert_eq!(d.sni, "localhost");
    assert_eq!(d.sha256, want);
}

/// a DNS server on loopback: A queries for `name` get 127.0.0.1, everything else an empty answer
fn fake_dns(name: &str) -> u16 {
    let s = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = s.local_addr().unwrap().port();
    let want: Vec<u8> =
        name.split('.').flat_map(|p| std::iter::once(p.len() as u8).chain(p.bytes())).chain([0]).collect();
    std::thread::spawn(move || {
        let mut buf = [0u8; 1500];
        while let Ok((n, from)) = s.recv_from(&mut buf) {
            let q = &buf[..n];
            let qname_end = 12 + q[12..].iter().position(|b| *b == 0).unwrap_or(0) + 1;
            let qtype = u16::from_be_bytes([q[qname_end], q[qname_end + 1]]);
            let hit = q[12..qname_end].eq_ignore_ascii_case(&want) && qtype == 1;
            let mut out = q[..2].to_vec();
            out.extend_from_slice(&[0x81, 0x80, 0, 1, 0, u8::from(hit), 0, 0, 0, 0]);
            out.extend_from_slice(&q[12..qname_end + 4]);
            if hit {
                out.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 127, 0, 0, 1]);
            }
            let _ = s.send_to(&out, from);
        }
    });
    port
}

/// SOCKS5 CONNECT (no auth) to a domain name
fn socks5_domain(proxy: u16, host: &str, dest_port: u16) -> std::io::Result<TcpStream> {
    let mut s = TcpStream::connect(("127.0.0.1", proxy))?;
    s.set_read_timeout(Some(Duration::from_secs(15)))?;
    s.write_all(&[5, 1, 0])?;
    let mut r = [0u8; 2];
    s.read_exact(&mut r)?;
    let mut req = vec![5, 1, 0, 3, host.len() as u8];
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(&dest_port.to_be_bytes());
    s.write_all(&req)?;
    let mut rep = [0u8; 10];
    s.read_exact(&mut rep)?;
    if rep[1] != 0 {
        return Err(std::io::Error::other(format!("connect refused ({})", rep[1])));
    }
    Ok(s)
}

#[test]
fn several_dns_servers_race() {
    let _serial = serial();
    if exe().is_none() {
        eprintln!("sing-box not found: skipped");
        return;
    }
    let dir = temp_dir("dnsrace");
    let port = free_port();
    let echo = echo_server();
    // the main server never answers; the extra one does
    let silent = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let dead = silent.local_addr().unwrap().port();
    let good = fake_dns("race.test");
    let p = card(json!({"type": "socks5", "host": "127.0.0.1", "port": port}), json!({"type": "direct"}));
    let glob = config::validate_global(
        &json!({"dns_server": format!("127.0.0.1:{dead}"), "dns_extra": [format!("127.0.0.1:{good}")]}),
    );
    assert_eq!(glob["dns_extra"], json!([format!("127.0.0.1:{good}")]));
    let (hooks, log, _) = Log::hooks();
    let mut engine = Engine::start(&p, &glob, &env(&dir), hooks).expect("start");
    let t0 = Instant::now();
    let mut s = socks5_domain(port, "race.test", echo).unwrap_or_else(|e| panic!("{e}: {:?}", log.0.lock().unwrap()));
    s.write_all(b"hi").unwrap();
    let mut got = [0u8; 8];
    s.read_exact(&mut got).unwrap();
    assert_eq!(&got, b"echo: hi");
    // answered by the extra server, not after the main one's timeout
    assert!(t0.elapsed() < Duration::from_secs(5), "{:?}", t0.elapsed());
    engine.stop();
}
