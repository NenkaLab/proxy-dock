//! A Hysteria2 server card open to other PCs, run by the official Hysteria2 core (Python: `singbox/hysteria.py`).
//!
//! sing-box's Hysteria2 server does not report wrong passwords, so IP bans would not work there. The official core
//! asks an HTTP auth server for every connection with the client address (`auth: type: http`), so ProxyDock
//! answers those requests itself with the usual rules: repeated failures ban the IP (guard, shared with every
//! other card) and at most MAX_QUIC_PER_IP connections per IP.
//!
//! Data path: client -QUIC-> hysteria server -SOCKS5 (127.0.0.1, random credentials)-> this card's own sing-box
//! -> the card's outbound chain. sing-box applies the allow list, private network blocking and chains (UDP
//! included), and its Clash API gives the statistics, so only authentication lives in hysteria + ProxyDock.

use super::guard;
use super::process::{Engine, EngineEnv, Hooks, START_TIMEOUT, StartError, Stats, random_hex, start_error};
use crate::config;
use crate::i18n::tr;
use crate::log::Level;
use crate::net::host_port;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// max concurrent QUIC connections from one IP (loopback excluded)
pub const MAX_QUIC_PER_IP: usize = 64;
const RESTART_KEYS: [&str; 4] = ["allowed_domains", "block_private", "timeout", "log_connections"];

struct Shared {
    hooks: Hooks,
    inbound: Mutex<Value>,
    log_connections: AtomicBool,
    blocked: AtomicU64,
    last_error: Mutex<String>,
    /// client ip -> connected QUIC connections (from hysteria's log)
    active: Mutex<HashMap<String, usize>>,
    stopping: AtomicBool,
}

impl Shared {
    fn log(&self, level: Level, message: &str) {
        let m = crate::log::clean(message);
        if level.is_problem() {
            *self.last_error.lock().unwrap() = m.clone();
        }
        (self.hooks.log)(level, &m);
    }

    fn log_conn(&self, level: Level, message: &str) {
        if self.log_connections.load(Ordering::Relaxed) {
            self.log(level, message);
        }
    }

    /// Decide one connection attempt; addr = "ip:port" of the client.
    fn check_auth(&self, addr: &str, password: &str) -> bool {
        let ip = ip_of(addr);
        if guard::is_banned(&ip) {
            self.blocked.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if !guard::is_loopback_ip(&ip) && self.active.lock().unwrap().get(&ip).copied().unwrap_or(0) >= MAX_QUIC_PER_IP
        {
            self.blocked.fetch_add(1, Ordering::Relaxed); // refused without a log line, as before
            return false;
        }
        let want = self.inbound.lock().unwrap()["hy2_password"].as_str().unwrap_or("").to_string();
        if same(password.as_bytes(), want.as_bytes()) {
            return true;
        }
        self.blocked.fetch_add(1, Ordering::Relaxed);
        self.log(Level::Auth, &tr("H2 {client} authentication failed").replace("{client}", addr));
        if let Some(m) = guard::auth_failure(&ip) {
            self.log(Level::Blocked, &m);
        }
        false
    }

    /// one zap JSON log line of hysteria
    fn handle(&self, e: &Map<String, Value>) {
        let s = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let (msg, addr) = (s("msg"), s("addr"));
        match msg.as_str() {
            "client connected" => {
                *self.active.lock().unwrap().entry(ip_of(&addr)).or_default() += 1;
                self.log_conn(
                    Level::Tcp,
                    &tr("H2 {client} connected (Hysteria2 authenticated)").replace("{client}", &addr),
                );
            }
            "client disconnected" => {
                let mut a = self.active.lock().unwrap();
                let ip = ip_of(&addr);
                let n = a.get(&ip).copied().unwrap_or(1).saturating_sub(1);
                if n > 0 {
                    a.insert(ip, n);
                } else {
                    a.remove(&ip);
                }
            }
            "TCP request" => self.log_conn(Level::Tcp, &format!("H2 {addr} -> {}", s("reqAddr"))),
            "UDP request" => self.log_conn(Level::Udp, &format!("H2 {addr} -> {} UDP", s("reqAddr"))),
            "TCP error" | "UDP error" => {
                if self.log_connections.load(Ordering::Relaxed) {
                    self.log(Level::Warn, format!("H2 {addr} -> {} {}", s("reqAddr"), s("error")).trim_end());
                }
            }
            _ => {
                let level = s("level");
                if matches!(level.as_str(), "warn" | "error" | "fatal") {
                    let lv = if level == "warn" { Level::Warn } else { Level::Error };
                    self.log(lv, format!("Hysteria2: {msg} {}", s("error")).trim_end());
                }
            }
        }
    }
}

fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn ip_of(addr: &str) -> String {
    crate::net::split_host_port(addr, Some(0)).map(|(h, _)| h).unwrap_or_else(|_| addr.to_string())
}

/// A running Hysteria2 server card.
pub struct HyEngine {
    shared: Arc<Shared>,
    inner: Engine,
    /// the inner sing-box's SOCKS5 inbound: port, user, password
    inner_login: (u16, String, String),
    cfg: Map<String, Value>,
    child: Arc<Mutex<Option<Child>>>,
    auth_addr: SocketAddr,
    config_path: PathBuf,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// the card as its inner sing-box runs it: a SOCKS5 inbound on loopback with random credentials, connections
/// logged from hysteria's log instead (it knows the real client address)
fn inner_proxy(proxy: &Value, login: &(u16, String, String)) -> Value {
    let mut p = proxy.clone();
    let mut ib = config::inbound_defaults();
    for (k, v) in [
        ("type", json!("socks5")),
        ("host", json!("127.0.0.1")),
        ("port", json!(login.0)),
        ("auth_enabled", json!(true)),
        ("username", json!(login.1)),
        ("password", json!(login.2)),
    ] {
        ib.insert(k.into(), v);
    }
    p["inbound"] = Value::Object(ib);
    let mut ov = p["overrides"].as_object().cloned().unwrap_or_default();
    ov.insert("log_connections".into(), json!(false));
    ov.insert("log_close".into(), json!(false));
    p["overrides"] = Value::Object(ov);
    p
}

/// the hysteria server config (JSON is valid YAML)
fn hysteria_config(
    ib: &Value,
    cert: &(String, String),
    auth_port: u16,
    login: &(u16, String, String),
    timeout: &Value,
) -> Value {
    let host = ib["host"].as_str().unwrap_or("");
    let host = if host.eq_ignore_ascii_case("localhost") { "127.0.0.1" } else { host };
    let mut conf = json!({
        "listen": host_port(host, ib["port"].as_i64().unwrap_or(0)),
        "tls": {"cert": cert.0, "key": cert.1},
        "auth": {"type": "http", "http": {"url": format!("http://127.0.0.1:{auth_port}/auth")}},
        "outbounds": [{"name": "singbox", "type": "socks5", "socks5": {
            "addr": format!("127.0.0.1:{}", login.0), "username": login.1, "password": login.2}}],
        "udpIdleTimeout": format!("{}s", crate::singbox::compile::py_str(timeout)),
    });
    let truthy = crate::secret::truthy;
    if ib.get("hy2_obfs_password").is_some_and(truthy) {
        conf["obfs"] = json!({"type": "salamander", "salamander": {"password": ib["hy2_obfs_password"]}});
    }
    let mut bw = Map::new();
    for (k, name) in [("hy2_up_mbps", "up"), ("hy2_down_mbps", "down")] {
        if let Some(v) = ib.get(k).filter(|v| truthy(v)) {
            bw.insert(name.into(), json!(format!("{} mbps", crate::singbox::compile::py_str(v))));
        }
    }
    if !bw.is_empty() {
        conf["bandwidth"] = Value::Object(bw);
    }
    conf
}

/// answer one request of hysteria's HTTP auth (POST /auth {"addr", "auth"} -> {"ok", "id"})
fn auth_client(shared: &Shared, mut s: TcpStream) {
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let mut r = BufReader::new(match s.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    });
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        match r.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':')
            && k.trim().eq_ignore_ascii_case("content-length")
        {
            length = v.trim().parse().unwrap_or(0);
        }
        if length > 1 << 16 {
            return;
        }
    }
    let mut body = vec![0u8; length.min(1 << 16)];
    if r.read_exact(&mut body).is_err() {
        return;
    }
    let req: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
    let text = |k: &str| match &req[k] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    };
    let ok = shared.check_auth(&text("addr"), &text("auth"));
    let out = json!({"ok": ok, "id": if ok { "user" } else { "" }}).to_string();
    let _ = write!(
        s,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{out}",
        out.len()
    );
}

impl HyEngine {
    /// Start the card's inner sing-box, the auth server and the Hysteria2 core (`hysteria`), and wait until the
    /// core reports "server up and running". env.cert is the certificate the core serves.
    pub fn start(
        proxy: &Value,
        glob: &Map<String, Value>,
        env: &EngineEnv,
        hooks: Hooks,
        hysteria: &Path,
    ) -> Result<HyEngine, StartError> {
        let id = proxy["id"].as_str().unwrap_or("").to_string();
        let cfg = config::resolve(proxy, glob);
        let cert = env.cert.clone().ok_or_else(|| StartError::Other("no certificate".into()))?;
        let shared = Arc::new(Shared {
            hooks: hooks.clone(),
            inbound: Mutex::new(proxy["inbound"].clone()),
            log_connections: AtomicBool::new(cfg["log_connections"] == json!(true)),
            blocked: AtomicU64::new(0),
            last_error: Mutex::new(String::new()),
            active: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
        });
        let port = TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .map_err(|e| StartError::Other(e.to_string()))?;
        let login = (port, random_hex(8), random_hex(16));
        let inner_hooks = {
            let s = shared.clone();
            Hooks { log: Arc::new(move |lv, m| s.log(lv, m)), died: hooks.died.clone() }
        };
        let inner_env = EngineEnv { cert: None, helper: None, ..env.clone() };
        let mut inner = Engine::start(&inner_proxy(proxy, &login), glob, &inner_env, inner_hooks)?;
        let fail = |inner: &mut Engine, e: StartError| {
            inner.stop();
            Err(e)
        };

        // auth server
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(e) => return fail(&mut inner, StartError::Other(e.to_string())),
        };
        let auth_addr = listener.local_addr().unwrap();
        let mut threads = vec![];
        {
            let shared = shared.clone();
            threads.push(std::thread::spawn(move || {
                for s in listener.incoming() {
                    if shared.stopping.load(Ordering::SeqCst) {
                        break;
                    }
                    if let Ok(s) = s {
                        let shared = shared.clone();
                        std::thread::spawn(move || auth_client(&shared, s));
                    }
                }
            }));
        }

        let config_path = env.run_dir.join(format!("{id}-hysteria.yaml"));
        let conf = hysteria_config(&proxy["inbound"], &cert, auth_addr.port(), &login, &cfg["timeout"]);
        let mut hy = HyEngine {
            shared: shared.clone(),
            inner,
            inner_login: login,
            cfg: cfg.clone(),
            child: Arc::new(Mutex::new(None)),
            auth_addr,
            config_path: config_path.clone(),
            threads,
        };
        if let Err(e) = std::fs::create_dir_all(&env.run_dir)
            .and_then(|()| std::fs::write(&config_path, serde_json::to_string_pretty(&conf).unwrap()))
        {
            hy.stop();
            return Err(StartError::Other(e.to_string()));
        }
        let mut cmd = Command::new(hysteria);
        cmd.args(["server", "-c"]).arg(&config_path).arg("--disable-update-check").current_dir(&env.run_dir);
        cmd.env("HYSTERIA_LOG_LEVEL", if shared.log_connections.load(Ordering::Relaxed) { "debug" } else { "info" });
        cmd.env("HYSTERIA_LOG_FORMAT", "json");
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                hy.stop();
                return Err(StartError::Other(e.to_string()));
            }
        };
        #[cfg(windows)]
        super::process::attach_to_job(&child);
        let (tx, rx) = mpsc::channel::<Option<String>>();
        for pipe in [
            child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
            child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let tx = tx.clone();
            hy.threads.push(std::thread::spawn(move || {
                for line in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
                    let _ = tx.send(Some(String::from_utf8_lossy(&line).into_owned()));
                }
                let _ = tx.send(None);
            }));
        }
        drop(tx);
        *hy.child.lock().unwrap() = Some(child);
        let (started_tx, started_rx) = mpsc::channel::<Result<(), StartError>>();
        {
            let (shared, child) = (shared.clone(), hy.child.clone());
            hy.threads.push(std::thread::spawn(move || read_output(&shared, &child, rx, started_tx)));
        }
        let result = started_rx
            .recv_timeout(START_TIMEOUT)
            .unwrap_or_else(|_| Err(StartError::Other(tr("The Hysteria2 core did not start in time"))));
        if let Err(e) = result {
            hy.stop();
            return Err(e);
        }
        Ok(hy)
    }

    /// ProxyDock's switch: the inner sing-box refuses what the Hysteria2 core hands it
    pub fn set_app_on(&self, on: bool) -> Result<(), String> {
        self.inner.set_app_on(on)
    }

    pub fn stats(&self) -> Stats {
        let mut s = self.inner.stats();
        s.blocked += self.shared.blocked.load(Ordering::Relaxed);
        s
    }

    pub fn last_error(&self) -> String {
        self.shared.last_error.lock().unwrap().clone()
    }

    pub fn last_reason(&self) -> String {
        self.inner.last_reason()
    }

    pub fn clear_last(&self) {
        self.shared.last_error.lock().unwrap().clear();
        self.inner.clear_last();
    }

    pub fn needs_restart(&self, proxy: &Value, glob: &Map<String, Value>) -> bool {
        let new = config::resolve(proxy, glob);
        RESTART_KEYS.iter().any(|k| new[*k] != self.cfg[*k])
            || self.inner.needs_restart(&inner_proxy(proxy, &self.inner_login), glob)
    }

    pub fn update(&mut self, proxy: &Value, glob: &Map<String, Value>) {
        *self.shared.inbound.lock().unwrap() = proxy["inbound"].clone();
        self.cfg = config::resolve(proxy, glob);
        self.shared.log_connections.store(self.cfg["log_connections"] == json!(true), Ordering::Relaxed);
        self.inner.update(&inner_proxy(proxy, &self.inner_login), glob);
    }

    pub fn exit_totals(&self) -> HashMap<String, (f64, f64)> {
        self.inner.exit_totals()
    }

    pub fn exit_conns(&self) -> HashMap<String, u64> {
        self.inner.exit_conns()
    }

    pub fn stop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        let c = self.child.lock().unwrap().take();
        if let Some(mut c) = c {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = TcpStream::connect_timeout(&self.auth_addr, Duration::from_secs(1)); // wakes the auth server's accept
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        self.inner.stop();
        let _ = std::fs::remove_file(&self.config_path);
    }
}

impl Drop for HyEngine {
    fn drop(&mut self) {
        if !self.threads.is_empty() {
            self.stop();
        }
    }
}

fn read_output(
    shared: &Shared,
    child: &Mutex<Option<Child>>,
    lines: mpsc::Receiver<Option<String>>,
    started: mpsc::Sender<Result<(), StartError>>,
) {
    let mut errors: Vec<String> = vec![];
    let mut is_started = false;
    let mut open = 2;
    while open > 0 {
        let Ok(item) = lines.recv() else { break };
        let Some(raw) = item else {
            open -= 1;
            continue;
        };
        let line = raw.trim();
        let Ok(Value::Object(e)) = serde_json::from_str::<Value>(line) else {
            if !line.is_empty() {
                errors.push(line.to_string());
            }
            continue;
        };
        let msg = e.get("msg").and_then(Value::as_str).unwrap_or("");
        let level = e.get("level").and_then(Value::as_str).unwrap_or("");
        if !is_started {
            if msg == "server up and running" {
                is_started = true;
                let _ = started.send(Ok(()));
                continue;
            }
            if matches!(level, "fatal" | "error" | "panic") {
                let err = e.get("error").and_then(Value::as_str).unwrap_or("");
                errors.push(format!("{msg}: {err}").trim_end_matches(": ").to_string());
            }
        }
        shared.handle(&e);
    }
    let code = child.lock().unwrap().as_mut().and_then(|c| c.wait().ok()).and_then(|s| s.code());
    let code_text = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
    let exited = tr("The Hysteria2 core exited (code {code})").replace("{code}", &code_text);
    if !is_started {
        let _ = started.send(Err(start_error(&errors.last().cloned().unwrap_or(exited))));
    } else if !shared.stopping.load(Ordering::SeqCst) {
        shared.log(Level::Error, &exited);
        (shared.hooks.died)(exited);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shared(pw: &str) -> Shared {
        Shared {
            hooks: Hooks { log: Arc::new(|_, _| {}), died: Arc::new(|_| {}) },
            inbound: Mutex::new(json!({"hy2_password": pw})),
            log_connections: AtomicBool::new(true),
            blocked: AtomicU64::new(0),
            last_error: Mutex::new(String::new()),
            active: Mutex::new(HashMap::new()),
            stopping: AtomicBool::new(false),
        }
    }

    #[test]
    fn auth_rules() {
        let s = shared("pw");
        assert!(s.check_auth("198.51.100.7:4000", "pw"));
        assert!(!s.check_auth("198.51.100.7:4000", "nope"));
        assert_eq!(s.blocked.load(Ordering::Relaxed), 1);
        assert!(s.last_error.lock().unwrap().contains("198.51.100.7:4000"));
        // connection count per IP from hysteria's log
        for _ in 0..MAX_QUIC_PER_IP {
            s.handle(json!({"msg": "client connected", "addr": "198.51.100.8:1"}).as_object().unwrap());
        }
        assert!(!s.check_auth("198.51.100.8:2", "pw"));
        s.handle(json!({"msg": "client disconnected", "addr": "198.51.100.8:1"}).as_object().unwrap());
        assert!(s.check_auth("198.51.100.8:2", "pw"));
        // loopback has no limit
        for _ in 0..MAX_QUIC_PER_IP {
            s.handle(json!({"msg": "client connected", "addr": "[::1]:1"}).as_object().unwrap());
        }
        assert!(s.check_auth("[::1]:5", "pw"));
    }

    #[test]
    fn config_shape() {
        let ib = json!({"host": "localhost", "port": 8443, "hy2_obfs_password": "ob", "hy2_up_mbps": 50, "hy2_down_mbps": 0});
        let c = hysteria_config(&ib, &("c.crt".into(), "c.key".into()), 9, &(7, "u".into(), "p".into()), &json!(120));
        assert_eq!(c["listen"], "127.0.0.1:8443");
        assert_eq!(c["auth"]["http"]["url"], "http://127.0.0.1:9/auth");
        assert_eq!(c["outbounds"][0]["socks5"], json!({"addr": "127.0.0.1:7", "username": "u", "password": "p"}));
        assert_eq!(c["obfs"]["salamander"]["password"], "ob");
        assert_eq!(c["bandwidth"], json!({"up": "50 mbps"}));
        assert_eq!(c["udpIdleTimeout"], "120s");
    }
}
