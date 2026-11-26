//! Runs the proxy cards (Python: `runtime.ProxyManager`). One worker thread takes the commands in order, so a
//! start, a stop and a settings change of the same card never overlap and the caller (UI / tray) never waits on
//! sing-box. A sing-box that exits on its own comes back through the same queue.

use crate::i18n::tr;
use crate::log::{Entry, Level, clean, now_text};
use crate::net::host_port;
use crate::singbox::helper::Launch;
use crate::singbox::hysteria::HyEngine;
use crate::singbox::process::{Engine, EngineEnv, Hooks, StartError, Stats};
use crate::singbox::{self, Kind, guard, rulesets};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Glob = Map<String, Value>;

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Stopped,
    Starting,
    Running,
    /// could not start, or its engine stopped on its own (the message says why)
    Error(String),
}

/// What the owner of the manager hears. Both are called on the worker thread (log also on sing-box reader
/// threads): keep them short, hand the work to the UI's own thread.
/// a card's state changed (its id, the state)
pub type OnStatus = Arc<dyn Fn(&str, &State) + Send + Sync>;
/// a log entry
pub type OnLog = Arc<dyn Fn(Entry) + Send + Sync>;

#[derive(Clone)]
pub struct Events {
    pub status: OnStatus,
    pub log: OnLog,
}

enum Cmd {
    Start(Value, Glob, Option<Sender<bool>>),
    Stop(String, Option<Sender<()>>),
    StopAll(Sender<()>),
    Apply(Value, Glob, Option<Sender<()>>),
    ApplyGlobal(Glob, Vec<Value>),
    Died(String, u64, String),
    /// the manager is dropped (the worker holds a sender itself, so the channel never closes on its own)
    Quit,
}

/// the engine of a running card
enum Card {
    // (boxed: the engines are big and differ in size)
    SingBox(Box<Engine>),
    Hysteria(Box<HyEngine>),
}

impl Card {
    fn stats(&self) -> Stats {
        match self {
            Card::SingBox(e) => e.stats(),
            Card::Hysteria(e) => e.stats(),
        }
    }
    fn last_error(&self) -> String {
        match self {
            Card::SingBox(e) => e.last_error(),
            Card::Hysteria(e) => e.last_error(),
        }
    }
    fn last_reason(&self) -> String {
        match self {
            Card::SingBox(e) => e.last_reason(),
            Card::Hysteria(e) => e.last_reason(),
        }
    }
    fn clear_last(&self) {
        match self {
            Card::SingBox(e) => e.clear_last(),
            Card::Hysteria(e) => e.clear_last(),
        }
    }
    /// (a Hysteria server card has one exit: the caller falls back to it)
    fn route_of(&self, who: &str, target: Option<&str>) -> Option<String> {
        match self {
            Card::SingBox(e) => e.route_of(who, target),
            Card::Hysteria(_) => None,
        }
    }
    fn exit_totals(&self) -> HashMap<String, (f64, f64)> {
        match self {
            Card::SingBox(e) => e.exit_totals(),
            Card::Hysteria(e) => e.exit_totals(),
        }
    }
    fn needs_restart(&self, proxy: &Value, glob: &Glob) -> bool {
        match self {
            Card::SingBox(e) => e.needs_restart(proxy, glob),
            Card::Hysteria(e) => e.needs_restart(proxy, glob),
        }
    }
    fn update(&mut self, proxy: &Value, glob: &Glob) {
        match self {
            Card::SingBox(e) => e.update(proxy, glob),
            Card::Hysteria(e) => e.update(proxy, glob),
        }
    }
    fn stop(&mut self) {
        match self {
            Card::SingBox(e) => e.stop(),
            Card::Hysteria(e) => e.stop(),
        }
    }
}

struct Running {
    engine: Card,
    proxy: Value,
    /// what engine_for chose at start (a change of it means a restart)
    engine_choice: (Kind, PathBuf),
    generation: u64,
}

#[derive(Default)]
struct Shared {
    status: HashMap<String, State>,
    /// counters of earlier runs of each card (Python keeps one Stats object across restarts)
    carried: HashMap<String, Stats>,
    running: HashMap<String, Running>,
}

pub struct Manager {
    tx: Sender<Cmd>,
    shared: Arc<Mutex<Shared>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

/// What another thread needs of the manager (the connection check starts and reads its own card).
#[derive(Clone)]
pub struct Remote {
    tx: Sender<Cmd>,
    shared: Arc<Mutex<Shared>>,
}

impl Remote {
    pub fn start(&self, proxy: &Value, glob: &Glob) -> Receiver<bool> {
        let (t, r) = channel();
        let _ = self.tx.send(Cmd::Start(proxy.clone(), glob.clone(), Some(t)));
        r
    }

    pub fn stop(&self, id: &str) -> Receiver<()> {
        let (t, r) = channel();
        let _ = self.tx.send(Cmd::Stop(id.to_string(), Some(t)));
        r
    }

    pub fn state(&self, id: &str) -> State {
        self.shared.lock().unwrap().status.get(id).cloned().unwrap_or(State::Stopped)
    }

    pub fn last_error(&self, id: &str) -> String {
        self.shared.lock().unwrap().running.get(id).map(|r| r.engine.last_error()).unwrap_or_default()
    }

    pub fn last_reason(&self, id: &str) -> String {
        self.shared.lock().unwrap().running.get(id).map(|r| r.engine.last_reason()).unwrap_or_default()
    }

    pub fn clear_last(&self, id: &str) {
        if let Some(r) = self.shared.lock().unwrap().running.get(id) {
            r.engine.clear_last();
        }
    }
}

/// where the worker finds the engines' folders (replaceable for tests)
#[derive(Clone)]
pub struct Folders {
    pub run_dir: PathBuf,
    pub cert_dir: PathBuf,
    pub ruleset_dir: PathBuf,
    /// tests: run every Hysteria2 server card on the official core, loopback ones too
    pub force_hysteria: bool,
    /// tests: start every sing-box card through this helper (normally only TUN cards without administrator
    /// rights, through UAC)
    pub force_helper: Option<Launch>,
}

impl Default for Folders {
    fn default() -> Self {
        Folders {
            run_dir: guard::run_dir(),
            cert_dir: crate::paths::cert_dir(),
            ruleset_dir: rulesets::ruleset_dir(),
            force_hysteria: false,
            force_helper: None,
        }
    }
}

impl Manager {
    pub fn new(events: Events) -> Manager {
        Manager::with_folders(events, Folders::default())
    }

    pub fn with_folders(events: Events, folders: Folders) -> Manager {
        guard::set_run_dir(folders.run_dir.clone());
        let (tx, rx) = channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let w = Worker { tx: tx.clone(), shared: shared.clone(), events, folders, generation: 0 };
        let worker = std::thread::Builder::new().name("proxy-manager".into()).spawn(move || w.run(rx)).ok();
        Manager { tx, shared, worker }
    }

    pub fn remote(&self) -> Remote {
        Remote { tx: self.tx.clone(), shared: self.shared.clone() }
    }

    /// Start a card (a running one restarts). The receiver says whether it is running now.
    pub fn start(&self, proxy: &Value, glob: &Glob) -> Receiver<bool> {
        let (t, r) = channel();
        let _ = self.tx.send(Cmd::Start(proxy.clone(), glob.clone(), Some(t)));
        r
    }

    pub fn stop(&self, id: &str) -> Receiver<()> {
        let (t, r) = channel();
        let _ = self.tx.send(Cmd::Stop(id.to_string(), Some(t)));
        r
    }

    /// stop every card, waiting at most `timeout`
    pub fn stop_all(&self, timeout: Duration) {
        let (t, r) = channel();
        if self.tx.send(Cmd::StopAll(t)).is_ok() {
            let _ = r.recv_timeout(timeout);
        }
    }

    /// A card's settings changed: a running card restarts when its inbound / outbound / routes, its engine or a
    /// setting that lives in the sing-box config changed; other changes apply at once.
    pub fn apply(&self, proxy: &Value, glob: &Glob) -> Receiver<()> {
        let (t, r) = channel();
        let _ = self.tx.send(Cmd::Apply(proxy.clone(), glob.clone(), Some(t)));
        r
    }

    /// The global settings changed: apply them to the running cards among `proxies`.
    pub fn apply_global(&self, glob: &Glob, proxies: &[Value]) {
        let _ = self.tx.send(Cmd::ApplyGlobal(glob.clone(), proxies.to_vec()));
    }

    pub fn state(&self, id: &str) -> State {
        self.shared.lock().unwrap().status.get(id).cloned().unwrap_or(State::Stopped)
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.shared.lock().unwrap().running.contains_key(id)
    }

    /// a card's counters: earlier runs plus the running one
    pub fn stats(&self, id: &str) -> Stats {
        let sh = self.shared.lock().unwrap();
        let mut s = sh.carried.get(id).cloned().unwrap_or_default();
        if let Some(r) = sh.running.get(id) {
            add(&mut s, &r.engine.stats(), true);
        }
        s
    }

    /// the message of the card's last failure (connection check), "" when none or not running
    pub fn last_error(&self, id: &str) -> String {
        self.shared.lock().unwrap().running.get(id).map(|r| r.engine.last_error()).unwrap_or_default()
    }

    pub fn last_reason(&self, id: &str) -> String {
        self.shared.lock().unwrap().running.get(id).map(|r| r.engine.last_reason()).unwrap_or_default()
    }

    /// the exit block a recent connection of a running card took (`Engine::route_of`)
    pub fn route_of(&self, id: &str, who: &str, target: Option<&str>) -> Option<String> {
        self.shared.lock().unwrap().running.get(id).and_then(|r| r.engine.route_of(who, target))
    }

    /// {outbound block id: (up, down)} of a running card
    pub fn exit_totals(&self, id: &str) -> HashMap<String, (f64, f64)> {
        self.shared.lock().unwrap().running.get(id).map(|r| r.engine.exit_totals()).unwrap_or_default()
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        self.stop_all(Duration::from_secs(5));
        let _ = self.tx.send(Cmd::Quit);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
    }
}

fn add(to: &mut Stats, s: &Stats, with_active: bool) {
    to.total += s.total;
    to.failed += s.failed;
    to.blocked += s.blocked;
    to.up += s.up;
    to.down += s.down;
    if with_active {
        to.tcp_active += s.tcp_active;
        to.udp_active += s.udp_active;
    }
}

/// "SOCKS5 127.0.0.1:1080 → Direct" (Python `ProxyInstance.summary`)
pub fn summary(proxy: &Value) -> String {
    summary_of(proxy, Kind::SingBox)
}

fn summary_of(proxy: &Value, kind_: Kind) -> String {
    let ib = &proxy["inbound"];
    let t = ib["type"].as_str().unwrap_or("");
    let kind = match t {
        "socks5" => "SOCKS5".to_string(),
        "socks4" => "SOCKS4".into(),
        "http" => "HTTP".into(),
        "https" => "HTTPS".into(),
        "shadowsocks" => "Shadowsocks".into(),
        "hysteria2" => "Hysteria2".into(),
        "wireguard" => "WireGuard".into(),
        "forward" => tr("Port forward"),
        "tun" => "TUN".into(),
        other => other.into(),
    };
    let port = ib["port"].as_i64().unwrap_or(0);
    let where_ = match t {
        "tun" => format!("{} · {}", tr("all apps"), ib["tun_stack"].as_str().unwrap_or("mixed")),
        "wireguard" => format!("UDP {port}"),
        _ => host_port(ib["host"].as_str().unwrap_or(""), port),
    };
    let mut parts = vec![];
    let mut ob = Some(&proxy["outbound"]);
    while let Some(o) = ob.filter(|o| o.is_object()) {
        let ot = o["type"].as_str().unwrap_or("");
        if ot == "group" {
            let names: Vec<&str> =
                o["members"].as_array().into_iter().flatten().filter_map(|m| m["name"].as_str()).collect();
            parts.push(format!("{} ({})", crate::graph::outbound_label("group"), names.join(", ")));
            break;
        }
        parts.push(match ot {
            "direct" | "block" => crate::graph::outbound_label(ot),
            _ => format!(
                "{} {}",
                crate::graph::outbound_label(ot),
                host_port(o["host"].as_str().unwrap_or(""), o["port"].as_i64().unwrap_or(0))
            ),
        });
        ob = o.get("via").filter(|v| !v.is_null());
    }
    let engine = if kind_ == Kind::Hysteria { " · Hysteria2 + sing-box" } else { "" };
    format!("{kind} {where_} → {}{engine}", parts.join(" ⇢ "))
}

/// may this process create a TUN device (Windows: elevated, elsewhere: root)
pub fn is_admin() -> bool {
    #[cfg(windows)]
    {
        unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
    }
    #[cfg(not(windows))]
    {
        std::env::var("USER").is_ok_and(|u| u == "root")
    }
}

fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || guard::is_loopback_ip(host)
}

/// the parts of a card whose change needs a restart (Python `_net_key`)
fn net_key(p: &Value) -> (&Value, &Value, Option<&Value>) {
    (&p["inbound"], &p["outbound"], p.get("routes"))
}

struct Worker {
    tx: Sender<Cmd>,
    shared: Arc<Mutex<Shared>>,
    events: Events,
    folders: Folders,
    generation: u64,
}

impl Worker {
    fn run(mut self, rx: Receiver<Cmd>) {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Start(p, g, done) => {
                    let ok = self.start(&p, &g);
                    if let Some(d) = done {
                        let _ = d.send(ok);
                    }
                }
                Cmd::Stop(id, done) => {
                    self.stop(&id, false);
                    if let Some(d) = done {
                        let _ = d.send(());
                    }
                }
                Cmd::StopAll(done) => {
                    let ids: Vec<String> = self.shared.lock().unwrap().running.keys().cloned().collect();
                    for id in ids {
                        self.stop(&id, false);
                    }
                    let _ = done.send(());
                }
                Cmd::Apply(p, g, done) => {
                    self.apply(&p, &g);
                    if let Some(d) = done {
                        let _ = d.send(());
                    }
                }
                Cmd::ApplyGlobal(g, proxies) => {
                    for p in proxies {
                        self.apply(&p, &g);
                    }
                }
                Cmd::Died(id, generation, text) => self.died(&id, generation, text),
                Cmd::Quit => break,
            }
        }
        // the manager is gone: nothing may outlive it
        let all: Vec<Running> = self.shared.lock().unwrap().running.drain().map(|(_, r)| r).collect();
        for mut r in all {
            r.engine.stop();
        }
    }

    fn set(&self, id: &str, st: State) {
        self.shared.lock().unwrap().status.insert(id.to_string(), st.clone());
        (self.events.status)(id, &st);
    }

    fn log(&self, level: Level, message: &str, proxy: Option<&Value>) {
        let (pid, name) =
            proxy.map_or(("", ""), |p| (p["id"].as_str().unwrap_or(""), p["name"].as_str().unwrap_or("")));
        (self.events.log)(Entry {
            time: now_text(),
            level,
            message: clean(message),
            proxy_id: pid.into(),
            proxy_name: name.into(),
        });
    }

    /// everything a card's sing-box needs besides its settings: the engine, rule-sets, certificate, TUN exclusions
    fn prepare(&self, proxy: &Value, glob: &Glob) -> Result<(EngineEnv, (Kind, PathBuf)), String> {
        let mut choice = singbox::engine_for(proxy, glob)?;
        let ib_type = proxy["inbound"]["type"].as_str().unwrap_or("");
        if self.folders.force_hysteria && ib_type == "hysteria2" {
            singbox::find_hysteria(glob).ok_or_else(|| tr("The Hysteria2 core was not found"))?;
            choice.0 = Kind::Hysteria;
        }
        let mut helper = self.folders.force_helper.clone();
        if helper.is_none() && ib_type == "tun" && !is_admin() {
            // TUN needs administrator rights: this card's sing-box runs through the UAC helper, which only runs
            // the bundled sing-box (never a configured or downloaded one), so that is the one checked here
            helper = Some(Launch::this_program().ok_or_else(|| tr("sing-box was not found"))?);
            let bundled = singbox::find_bundled(singbox::EXE).ok_or_else(|| tr("sing-box was not found"))?;
            if singbox::uses_certificate_pin(proxy)
                && !singbox::version::of(&bundled).is_some_and(|v| v.has_certificate_pin())
            {
                return Err(tr("certificate pinning needs sing-box 1.15 or later"));
            }
            choice.1 = bundled;
        }
        let need = rulesets::needed(proxy);
        let rulesets = if need.is_empty() {
            vec![]
        } else {
            rulesets::ensure_in(&self.folders.ruleset_dir, &need, &rulesets::http_get, &|m| {
                self.log(Level::Warn, m, Some(proxy))
            })?
        };
        let cert = if matches!(ib_type, "https" | "hysteria2") {
            Some(crate::tls::server_cert_paths(proxy, &self.folders.cert_dir).map_err(|e| e.to_string())?)
        } else {
            None
        };
        let mut exclude_paths = vec![];
        if ib_type == "tun" {
            let mut v: Vec<String> =
                [Some(choice.1.clone()), singbox::find_hysteria(glob), std::env::current_exe().ok()]
                    .into_iter()
                    .flatten()
                    .map(|p| std::path::absolute(&p).unwrap_or(p).display().to_string())
                    .collect();
            v.sort();
            v.dedup();
            exclude_paths = v;
        }
        let env = EngineEnv {
            exe: choice.1.clone(),
            run_dir: self.folders.run_dir.clone(),
            bans_path: guard::bans_path(),
            rulesets,
            cert,
            exclude_paths,
            helper,
        };
        Ok((env, choice))
    }

    /// the engine choice prepare would make (without downloads or certificates)
    fn prepare_choice(&self, proxy: &Value, glob: &Glob) -> Option<(Kind, PathBuf)> {
        let mut c = singbox::engine_for(proxy, glob).ok()?;
        let ib_type = proxy["inbound"]["type"].as_str().unwrap_or("");
        if self.folders.force_hysteria && ib_type == "hysteria2" {
            c.0 = Kind::Hysteria;
        }
        if self.folders.force_helper.is_none() && ib_type == "tun" && !is_admin() {
            c.1 = singbox::find_bundled(singbox::EXE)?;
        }
        Some(c)
    }

    fn start(&mut self, proxy: &Value, glob: &Glob) -> bool {
        let id = proxy["id"].as_str().unwrap_or("").to_string();
        if self.shared.lock().unwrap().running.contains_key(&id) {
            self.stop(&id, true);
        }
        self.set(&id, State::Starting);
        let started = self.prepare(proxy, glob).and_then(|(env, choice)| {
            self.generation += 1;
            let generation = self.generation;
            let (tx, log_events, log_proxy) = (self.tx.clone(), self.events.log.clone(), proxy.clone());
            let died_id = id.clone();
            let hooks = Hooks {
                log: Arc::new(move |lv, m| {
                    (log_events)(Entry {
                        time: now_text(),
                        level: lv,
                        message: clean(m),
                        proxy_id: log_proxy["id"].as_str().unwrap_or("").into(),
                        proxy_name: log_proxy["name"].as_str().unwrap_or("").into(),
                    })
                }),
                died: Arc::new(move |text| {
                    let _ = tx.send(Cmd::Died(died_id.clone(), generation, text));
                }),
            };
            let engine = if choice.0 == Kind::Hysteria {
                let hy = singbox::find_hysteria(glob).ok_or_else(|| tr("The Hysteria2 core was not found"))?;
                HyEngine::start(proxy, glob, &env, hooks, &hy).map(|e| Card::Hysteria(Box::new(e)))
            } else {
                Engine::start(proxy, glob, &env, hooks).map(|e| Card::SingBox(Box::new(e)))
            };
            engine
                .map(|engine| Running { engine, proxy: proxy.clone(), engine_choice: choice, generation })
                .map_err(|e| start_message(&e, proxy))
        });
        match started {
            Ok(r) => {
                let kind = r.engine_choice.0;
                self.shared.lock().unwrap().running.insert(id.clone(), r);
                self.set(&id, State::Running);
                self.log(
                    Level::Info,
                    &tr("Started: {summary}").replace("{summary}", &summary_of(proxy, kind)),
                    Some(proxy),
                );
                self.open_proxy_warnings(proxy);
                true
            }
            Err(msg) => {
                self.set(&id, State::Error(msg.clone()));
                let name = proxy["name"].as_str().unwrap_or("");
                self.log(
                    Level::Error,
                    &tr("[{name}] failed to start: {msg}").replace("{name}", name).replace("{msg}", &msg),
                    None,
                );
                false
            }
        }
    }

    fn open_proxy_warnings(&self, proxy: &Value) {
        let ib = &proxy["inbound"];
        let t = ib["type"].as_str().unwrap_or("");
        let host = ib["host"].as_str().unwrap_or("");
        if t == "tun" || is_loopback_host(host) {
            return;
        }
        if t == "socks4" {
            self.log(
                Level::Warn,
                &tr("SOCKS4 has no password authentication; anyone can use it if it is exposed"),
                Some(proxy),
            );
        } else if matches!(t, "socks5" | "http" | "https") && ib["auth_enabled"] != Value::Bool(true) {
            let hp = host_port(host, ib["port"].as_i64().unwrap_or(0));
            self.log(
                Level::Warn,
                &tr("Listening on {host_port} without authentication (open proxy)").replace("{host_port}", &hp),
                Some(proxy),
            );
        }
    }

    /// take a card out of the running set and stop its engine (counters carried over)
    fn take_and_stop(&self, id: &str) -> Option<Value> {
        let r = self.shared.lock().unwrap().running.remove(id)?;
        let Running { mut engine, proxy, .. } = r;
        engine.stop();
        let s = engine.stats();
        add(self.shared.lock().unwrap().carried.entry(id.to_string()).or_default(), &s, false);
        Some(proxy)
    }

    fn stop(&self, id: &str, quiet: bool) {
        let proxy = self.take_and_stop(id);
        if !quiet {
            if let Some(p) = &proxy {
                self.log(Level::Info, &tr("Stopped"), Some(p));
            }
            self.set(id, State::Stopped);
        }
    }

    fn died(&self, id: &str, generation: u64, text: String) {
        let current = self.shared.lock().unwrap().running.get(id).map(|r| r.generation);
        if current != Some(generation) {
            return; // an older run of the card, already replaced or stopped
        }
        self.take_and_stop(id);
        self.set(id, State::Error(text));
    }

    fn apply(&mut self, proxy: &Value, glob: &Glob) {
        let id = proxy["id"].as_str().unwrap_or("").to_string();
        let restart = {
            let sh = self.shared.lock().unwrap();
            let Some(r) = sh.running.get(&id) else { return };
            net_key(&r.proxy) != net_key(proxy)
                || self.prepare_choice(proxy, glob).as_ref() != Some(&r.engine_choice)
                || r.engine.needs_restart(proxy, glob)
        };
        if restart {
            self.start(proxy, glob);
        } else if let Some(r) = self.shared.lock().unwrap().running.get_mut(&id) {
            r.engine.update(proxy, glob);
            r.proxy = proxy.clone();
        }
    }
}

/// a start error as the card shows it (Python `ProxyManager._start`)
fn start_message(e: &StartError, proxy: &Value) -> String {
    let ib = &proxy["inbound"];
    let port = ib["port"].as_i64().unwrap_or(0).to_string();
    match e {
        StartError::PortInUse(_) => tr("Port {port} is already in use by another program").replace("{port}", &port),
        StartError::AccessDenied(_) => {
            tr("Port {port} cannot be used (reserved by Windows, or no permission)").replace("{port}", &port)
        }
        StartError::AddressNotAvailable(_) => {
            tr("This PC does not have the address {host}").replace("{host}", ib["host"].as_str().unwrap_or(""))
        }
        StartError::Other(t) => t.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn summaries() {
        let p = json!({"inbound": {"type": "socks5", "host": "::1", "port": 1080},
            "outbound": {"type": "socks5", "host": "10.0.0.1", "port": 9, "via": {"type": "direct", "via": null}}});
        assert_eq!(summary(&p), "SOCKS5 [::1]:1080 → SOCKS5 proxy 10.0.0.1:9 ⇢ Direct");
        let p = json!({"inbound": {"type": "wireguard", "host": "0.0.0.0", "port": 51820},
            "outbound": {"type": "group", "members": [{"name": "a"}, {"name": "b"}]}});
        assert_eq!(summary(&p), "WireGuard UDP 51820 → Group (a, b)");
    }
}
