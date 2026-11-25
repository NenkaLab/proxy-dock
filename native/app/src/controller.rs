//! The core process's controller (Python: the non-UI half of `ui/window.py`): reads the settings, runs the cards
//! through the manager (autostart, auto-restart with back-off), keeps the recent log, and serves the window
//! process over ipc. Everything happens on one controller thread fed by a channel.

use crate::{canvas, ipc};
use proxydock_core::i18n::tr;
use proxydock_core::log::{Entry, Level};
use proxydock_core::manager::{self, Events, Manager, State};
use proxydock_core::{config, edit, form, graph, log};
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const LOG_KEEP: usize = 5000;
/// log lines a newly opened window gets
pub const LOG_SEND: usize = 2000;
/// seconds before restart attempt 1, 2, 3, ... (then every 60 s)
pub const RESTART_DELAYS: [u64; 5] = [3, 6, 12, 30, 60];

enum Msg {
    Status(String, State),
    Log(Entry),
    Client(u64, TcpStream),
    Command(u64, Value),
    ClientGone(u64),
    Tick,
    Toggle(String),
    StopAll,
    Quit(Sender<()>),
    /// the result of work done on another thread (a network check, a download), applied on the controller's
    Work(Box<dyn FnOnce(&mut Controller) + Send>),
}

/// work for another thread: it returns what to do with its result on the controller's thread
type Job = Box<dyn FnOnce() -> Box<dyn FnOnce(&mut Controller) + Send> + Send>;

/// editor buttons that need the network (their work runs on another thread)
const NETWORK_ACTIONS: [&str; 3] = ["sub.update", "sni.detect", "probe"];

/// undo steps kept (Python `UNDO_LIMIT`)
const UNDO_LIMIT: usize = 100;
/// the PC block's live state: never undone (Python `PC_RUNTIME`)
const PC_RUNTIME: [&str; 3] = ["backup", "applied", "server"];

/// shows a message in the tray when no window is open (Python `on_notify` -> `tray.showMessage`)
pub type Notify = Arc<dyn Fn(&str) + Send + Sync>;

/// What the tray process does for the controller.
#[derive(Clone)]
pub struct Hooks {
    pub notify: Notify,
    /// quit the whole app (the window closed with "Quit with the X button" on)
    pub quit: Arc<dyn Fn() + Send + Sync>,
}

impl Default for Hooks {
    fn default() -> Self {
        Hooks { notify: Arc::new(|_| {}), quit: Arc::new(|| {}) }
    }
}

/// outbound blocks measured at the same time (Python: a pool of 4)
const LATENCY_WORKERS: usize = 4;

/// the first subscription check after starting, then how often (Python `update_due_subscriptions` timers)
const SUB_FIRST: Duration = Duration::from_secs(20);
const SUB_EVERY: Duration = Duration::from_secs(15 * 60);

/// the label of an editor button while its work runs
fn busy_label(action: &str) -> &'static str {
    match action {
        "sub.update" => "Updating…",
        "sni.detect" => "Connecting…",
        _ => "Checking…",
    }
}

/// a card as the tray menu shows it
#[derive(Clone, Debug)]
pub struct CardInfo {
    pub id: String,
    pub name: String,
    pub state: State,
    pub autostart: bool,
}

/// What the tray holds: commands to the controller and the window's connection details.
#[derive(Clone)]
pub struct Handle {
    tx: Sender<Msg>,
    pub port: u16,
    pub token: String,
    cards: Arc<Mutex<Vec<CardInfo>>>,
    glob: Arc<Mutex<Map<String, Value>>>,
}

impl Handle {
    /// a global setting as saved (the window's scale, ...)
    pub fn setting(&self, key: &str) -> Value {
        self.glob.lock().unwrap().get(key).cloned().unwrap_or(Value::Null)
    }

    pub fn cards(&self) -> Vec<CardInfo> {
        self.cards.lock().unwrap().clone()
    }

    /// start a stopped card, stop a running one
    pub fn toggle(&self, id: &str) {
        let _ = self.tx.send(Msg::Toggle(id.to_string()));
    }

    pub fn stop_all(&self) {
        let _ = self.tx.send(Msg::StopAll);
    }

    /// stop every card and the controller (waits at most `timeout`)
    pub fn quit(&self, timeout: Duration) {
        let (t, r) = channel();
        if self.tx.send(Msg::Quit(t)).is_ok() {
            let _ = r.recv_timeout(timeout);
        }
    }

    /// PROXYDOCK_IPC for the window process
    pub fn ipc_env(&self) -> String {
        format!("{} {}", self.port, self.token)
    }
}

pub fn state_name(s: &State) -> &'static str {
    match s {
        State::Stopped => "stopped",
        State::Starting => "starting",
        State::Running => "running",
        State::Error(_) => "error",
    }
}

/// Read the settings at `config_path` and start the controller (autostart cards start right away).
pub fn spawn(config_path: PathBuf) -> std::io::Result<Handle> {
    spawn_with(config_path, Hooks::default())
}

/// `spawn` with what the tray does for it (notifications, quitting)
pub fn spawn_with(config_path: PathBuf, hooks: Hooks) -> std::io::Result<Handle> {
    let (tx, rx) = channel();
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    let token = proxydock_core::singbox::process::random_hex(16);
    {
        let tx = tx.clone();
        std::thread::Builder::new().name("ipc-accept".into()).spawn(move || {
            for (n, s) in listener.incoming().flatten().enumerate() {
                if tx.send(Msg::Client(n as u64, s)).is_err() {
                    break;
                }
            }
        })?;
    }
    {
        let tx = tx.clone();
        std::thread::Builder::new().name("tick".into()).spawn(move || {
            while tx.send(Msg::Tick).is_ok() {
                std::thread::sleep(Duration::from_secs(1));
            }
        })?;
    }
    let cards = Arc::new(Mutex::new(vec![]));
    let shared_glob = Arc::new(Mutex::new(config::validate_global(&json!({}))));
    let events = {
        let (a, b) = (tx.clone(), tx.clone());
        Events {
            status: Arc::new(move |id, st| {
                let _ = a.send(Msg::Status(id.to_string(), st.clone()));
            }),
            log: Arc::new(move |e| {
                let _ = b.send(Msg::Log(e));
            }),
        }
    };
    let manager = Manager::new(events);
    let mut c = Controller {
        tx: tx.clone(),
        token: token.clone(),
        manager,
        glob: config::validate_global(&json!({})),
        proxies: vec![],
        logs: VecDeque::new(),
        clients: HashMap::new(),
        restart_at: HashMap::new(),
        restart_count: HashMap::new(),
        running_since: HashMap::new(),
        cards: cards.clone(),
        config_path,
        view: canvas::View::default(),
        graph: graph::empty(),
        can_save: false,
        drafts: HashMap::new(),
        flow_last: HashMap::new(),
        flow_sent: vec![],
        busy: std::collections::HashSet::new(),
        sub_busy: std::collections::HashSet::new(),
        next_sub_check: Instant::now() + SUB_FIRST,
        probe_text: HashMap::new(),
        hooks,
        shared_glob: shared_glob.clone(),
        engine_text: String::new(),
        latency: HashMap::new(),
        latency_busy: std::collections::HashSet::new(),
        latency_round: None,
        file_log: None,
        undo: vec![],
        redo: vec![],
        undo_base: Value::Null,
        undo_open: false,
        undo_quiet: false,
    };
    c.load();
    std::thread::Builder::new().name("controller".into()).spawn(move || c.run(rx))?;
    Ok(Handle { tx, port, token, cards, glob: shared_glob })
}

struct Client {
    stream: TcpStream,
    /// said hello with the right token
    ready: bool,
}

struct Controller {
    tx: Sender<Msg>,
    token: String,
    manager: Manager,
    glob: Map<String, Value>,
    proxies: Vec<Value>,
    logs: VecDeque<Entry>,
    clients: HashMap<u64, Client>,
    restart_at: HashMap<String, Instant>,
    restart_count: HashMap<String, usize>,
    running_since: HashMap<String, Instant>,
    cards: Arc<Mutex<Vec<CardInfo>>>,
    config_path: PathBuf,
    /// the canvas of the loaded graph
    view: canvas::View,
    /// the block graph (validated; secrets in plain text, sealed when saved)
    graph: Map<String, Value>,
    /// false when the settings file could not be read: it is never overwritten then
    can_save: bool,
    /// the block each window is editing: (kind, draft)
    drafts: HashMap<u64, (&'static str, Value)>,
    /// per (card, exit block): up + down bytes at the last tick (for the per-link rates)
    flow_last: HashMap<(String, String), f64>,
    /// the per-link rates last sent (bytes/s; all zero = nothing flowing)
    flow_sent: Vec<f64>,
    /// editor buttons whose work is running: (window, action)
    busy: std::collections::HashSet<(u64, String)>,
    /// subscription blocks being refreshed in the background
    sub_busy: std::collections::HashSet<String>,
    next_sub_check: Instant,
    /// the last connection check of each window: (block, text, tone 0 / 1 ok / 2 failed)
    probe_text: HashMap<u64, (String, String, i32)>,
    hooks: Hooks,
    /// the global settings as the tray reads them
    shared_glob: Arc<Mutex<Map<String, Value>>>,
    /// the settings page's sing-box line (path and version)
    engine_text: String,
    /// latency of outbound blocks: id -> (ok, ms, error, what it depends on)
    latency: HashMap<String, (bool, Option<i64>, String, Value)>,
    latency_busy: std::collections::HashSet<String>,
    /// the running test: (total, done, ok)
    latency_round: Option<(usize, usize, usize)>,
    /// "Save the log to a file" (global setting `log_to_file`)
    file_log: Option<log::FileLog>,
    /// undo / redo: the graph before each change; the changes one message makes are one step
    undo: Vec<Value>,
    redo: Vec<Value>,
    undo_base: Value,
    /// this message already made a step (further changes join it)
    undo_open: bool,
    /// background changes (a subscription refresh) are not steps
    undo_quiet: bool,
}

fn entry_json(e: &Entry) -> Value {
    json!({"time": e.time, "level": e.level.tag(), "message": e.message, "name": e.proxy_name})
}

impl Controller {
    fn app_log(&mut self, level: Level, message: &str) {
        let e = Entry { time: log::now_text(), level, message: log::clean(message), proxy_id: String::new(), proxy_name: String::new() };
        self.add_log(e);
    }

    fn add_log(&mut self, e: Entry) {
        if cfg!(debug_assertions) {
            eprintln!("[{}] {:<5} {}{}", e.time, e.level.tag(), if e.proxy_name.is_empty() { String::new() } else { format!("[{}] ", e.proxy_name) }, e.message);
        }
        self.broadcast(&json!({"ev": "log", "entries": [entry_json(&e)]}));
        if let Some(f) = self.file_log.as_mut() {
            let _ = f.write(&log::format_line(&e));
        }
        // a settings change is shown to the user too (Python `notify_hook`)
        if e.level == Level::Setting {
            self.notify_user(&e.message);
        }
        self.logs.push_back(e);
        while self.logs.len() > LOG_KEEP {
            self.logs.pop_front();
        }
    }

    /// A message for the user (Python `on_notify`): in the open window, else in the tray.
    fn notify_user(&mut self, message: &str) {
        if self.clients.values().any(|c| c.ready) {
            self.broadcast(&json!({"ev": "info", "message": message}));
        } else {
            (self.hooks.notify)(message);
        }
    }

    /// "Save the log to a file" on or off (Python `_apply_file_logging`)
    fn apply_file_logging(&mut self, enabled: bool) {
        if enabled && self.file_log.is_none() {
            let dir = self.config_path.parent().map(|d| d.join("logs")).unwrap_or_else(proxydock_core::paths::log_dir);
            match log::FileLog::open(&dir) {
                Ok(f) => self.file_log = Some(f),
                Err(e) => {
                    let msg = tr("Cannot open the log file: {e}").replace("{e}", &e.to_string());
                    self.app_log(Level::Error, &msg);
                }
            }
        } else if !enabled {
            self.file_log = None;
        }
    }

    /// the graph as undo keeps it (without the PC block's live state)
    fn snapshot(&self) -> Value {
        let mut g = self.graph.clone();
        if let Some(pc) = g.get_mut("pc").and_then(Value::as_object_mut) {
            for k in PC_RUNTIME {
                pc.remove(k);
            }
        }
        Value::Object(g)
    }

    /// After a save (Python `_note_change`): if the graph changed, the state before it becomes an undo step (one
    /// per message, however many changes it makes).
    fn note_change(&mut self) {
        let snap = self.snapshot();
        if snap == self.undo_base {
            return;
        }
        // the save while loading (an older file sealed again, a new file): the starting point, not a step
        if self.undo_base.is_null() {
            self.undo_base = snap;
            return;
        }
        if !self.undo_quiet && !self.undo_open {
            let before = std::mem::replace(&mut self.undo_base, Value::Null);
            self.undo.push(before);
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
            self.redo.clear();
            self.undo_open = true;
        }
        self.undo_base = snap;
        self.send_undo_state();
    }

    fn send_undo_state(&mut self) {
        let msg = json!({"ev": "undo", "undo": !self.undo.is_empty(), "redo": !self.redo.is_empty()});
        self.broadcast(&msg);
    }

    /// Undo (or redo) one step (Python `_step`)
    fn undo_step(&mut self, redo: bool) {
        if !self.can_save {
            return;
        }
        let popped = if redo { self.redo.pop() } else { self.undo.pop() };
        let Some(state) = popped else { return };
        let now = self.snapshot();
        if redo {
            self.undo.push(now);
        } else {
            self.redo.push(now);
        }
        let mut g = graph::validate(&state);
        let live: Vec<(String, Value)> = self
            .graph
            .get("pc")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .filter(|(k, _)| PC_RUNTIME.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        if let Some(pc) = g.get_mut("pc").and_then(Value::as_object_mut) {
            pc.extend(live);
        }
        self.graph = g;
        self.undo_quiet = true;
        self.graph_changed();
        self.undo_quiet = false;
        self.undo_base = self.snapshot();
        self.app_log(Level::Setting, &tr(if redo { "Redone" } else { "Undone" }));
        self.send_undo_state();
    }

    /// The settings page: the values to show, the choices and what the app found (Python `SettingsInterface.load`)
    fn settings_json(&self) -> Value {
        let g = &self.glob;
        let index = |list: &[&str], v: &Value| list.iter().position(|x| Some(*x) == v.as_str()).unwrap_or(0);
        let codes: Vec<&str> = proxydock_core::i18n::LANGUAGES.iter().map(|(c, _)| *c).collect();
        let scale = config::UI_SCALES.iter().position(|x| Some(*x) == g["ui_scale"].as_i64()).unwrap_or(1);
        let domains: Vec<&str> = g["allowed_domains"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        json!({
            "values": {
                "allowed_domains": domains.join(", "), "block_private": g["block_private"], "timeout": g["timeout"],
                "log_connections": g["log_connections"], "log_close": g["log_close"], "dns_server": g["dns_server"],
                "dns_via_outbound": g["dns_via_outbound"], "log_to_file": g["log_to_file"],
                "language": index(&codes, &g["language"]), "theme": index(&config::THEMES, &g["theme"]), "ui_scale": scale,
                "log_font_size": g["log_font_size"], "close_exits": g["close_exits"], "singbox_latest": g["singbox_latest"],
            },
            "languages": proxydock_core::i18n::LANGUAGES.iter().map(|(_, n)| *n).collect::<Vec<_>>(),
            "themes": [tr("System"), tr("Light"), tr("Dark")],
            "scales": config::UI_SCALES.iter().map(|s| format!("{s}%") + &if *s == 100 { tr(" (default)") } else { String::new() }).collect::<Vec<_>>(),
            "engine": self.engine_text,
            "log_dir": self.log_dir().display().to_string(),
        })
    }

    fn log_dir(&self) -> PathBuf {
        self.config_path.parent().map(|d| d.join("logs")).unwrap_or_else(proxydock_core::paths::log_dir)
    }

    /// "Apply" on the settings page (Python `SettingsInterface.apply` + `Controller.apply_global`)
    fn save_settings(&mut self, v: &Value) {
        let vals = &v["values"];
        let text = |k: &str| vals[k].as_str().unwrap_or("").to_string();
        let domains: Vec<String> = text("allowed_domains")
            .replace(';', ",")
            .split(',')
            .map(|d| proxydock_core::py::strip(d).to_string())
            .filter(|d| !d.is_empty())
            .collect();
        if domains.is_empty() {
            let msg = json!({"ev": "error", "message": format!("{}: {}", tr("Allowed domains"), tr("Enter at least one (* allows all)"))});
            self.broadcast(&msg);
            return;
        }
        if config::dns_server_spec(&text("dns_server")).is_none() {
            let msg = json!({"ev": "error", "message": tr("DNS server: use an address like 1.1.1.1, tls://dns.google or https://cloudflare-dns.com/dns-query")});
            self.broadcast(&msg);
            return;
        }
        if !self.can_save {
            self.broadcast(&json!({"ev": "error", "message": tr("Save failed")}));
            return;
        }
        let pick = |list: &[&str], k: &str| list.get(vals[k].as_u64().unwrap_or(0) as usize).copied().unwrap_or(list[0]).to_string();
        let codes: Vec<&str> = proxydock_core::i18n::LANGUAGES.iter().map(|(c, _)| *c).collect();
        let mut g = self.glob.clone();
        for k in ["block_private", "log_connections", "log_close", "dns_via_outbound", "log_to_file", "close_exits", "singbox_latest"] {
            g.insert(k.into(), json!(vals[k] == json!(true)));
        }
        for k in ["timeout", "log_font_size"] {
            if let Some(n) = vals[k].as_i64() {
                g.insert(k.into(), json!(n));
            }
        }
        g.insert("allowed_domains".into(), json!(domains));
        g.insert("dns_server".into(), json!(proxydock_core::py::strip(&text("dns_server"))));
        g.insert("language".into(), json!(pick(&codes, "language")));
        g.insert("theme".into(), json!(pick(&config::THEMES, "theme")));
        let scale = config::UI_SCALES.get(vals["ui_scale"].as_u64().unwrap_or(1) as usize).copied().unwrap_or(100);
        g.insert("ui_scale".into(), json!(scale));
        let g = config::validate_global(&Value::Object(g));
        let old = std::mem::replace(&mut self.glob, g);
        *self.shared_glob.lock().unwrap() = self.glob.clone();
        self.save();
        self.manager.apply_global(&self.glob, &self.proxies);
        let to_file = self.glob["log_to_file"] == json!(true);
        self.apply_file_logging(to_file);
        let mut pending = vec![];
        if self.glob["ui_scale"] != old["ui_scale"] {
            pending.push(tr("App scale"));
        }
        let mut msg = tr("Settings saved");
        if !pending.is_empty() {
            msg += &tr(" (applies after restart: {join})").replace("{join}", &pending.join(", "));
        }
        self.app_log(Level::Setting, &msg);
        if self.glob["singbox_latest"] == json!(true) && old["singbox_latest"] != json!(true) {
            self.update_latest_singbox();
        }
        if self.glob["singbox_latest"] != old["singbox_latest"] {
            self.engine_text = engine_text(&self.glob);
        }
        if self.glob["language"] != old["language"] {
            proxydock_core::i18n::set_language(self.glob["language"].as_str().unwrap_or("en"));
            let lang = json!({"ev": "language", "language": self.glob["language"]});
            self.broadcast(&lang);
            self.rebuild();
            self.send_canvas();
            self.app_log(Level::Setting, &tr("Language changed"));
        }
        let settings = json!({"ev": "settings", "settings": self.settings_json()});
        self.broadcast(&settings);
    }

    /// "Use the latest sing-box": check for a newer release in the background (at most once a day)
    fn update_latest_singbox(&mut self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = proxydock_core::singbox::latest::update(false);
            let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| {
                match result {
                    Ok(proxydock_core::singbox::latest::Outcome::Installed(v)) => {
                        let msg = tr("Downloaded sing-box {version}; each proxy uses it from its next start").replace("{version}", &v.to_string());
                        c.app_log(Level::Setting, &msg);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        let msg = tr("Could not get the latest sing-box: {error}").replace("{error}", &e);
                        c.app_log(Level::Warn, &msg);
                    }
                }
                c.engine_text = engine_text(&c.glob);
                let settings = json!({"ev": "settings", "settings": c.settings_json()});
                c.broadcast(&settings);
            })));
        });
    }

    /// Measure outbound blocks (the given ones, else every one but Block), 4 at a time (Python `test_latency`).
    fn test_latency(&mut self, keys: &[String]) {
        let targets = proxydock_core::probe::latency_targets(&self.graph);
        let mut ids: Vec<String> = keys.iter().map(|k| k.strip_suffix("/out").unwrap_or(k).to_string()).filter(|k| targets.contains(k)).collect();
        if ids.is_empty() {
            ids = targets;
        }
        ids.retain(|i| !self.latency_busy.contains(i));
        ids.dedup();
        if ids.is_empty() {
            return;
        }
        let jobs: Vec<(String, Value, Value)> = ids
            .iter()
            .map(|id| (id.clone(), proxydock_core::probe::outbound_proxy(&self.graph, id), proxydock_core::probe::latency_key(&self.graph, id)))
            .collect();
        self.latency_busy.extend(ids.iter().cloned());
        self.latency_round = Some((ids.len(), 0, 0));
        self.send_latency();
        let queue = Arc::new(Mutex::new(jobs));
        for _ in 0..LATENCY_WORKERS.min(ids.len()) {
            let (queue, tx, remote, glob) = (queue.clone(), self.tx.clone(), self.manager.remote(), self.glob.clone());
            std::thread::spawn(move || loop {
                let Some((id, p, key)) = queue.lock().unwrap().pop() else { break };
                let r = proxydock_core::probe::check_outbound(&remote, &p, &glob, &format!("__latency_{id}__"));
                let ok = r.get("ok") == Some(&json!(true));
                let ms = if ok { r.get("total_ms").and_then(Value::as_f64).map(|m| m.round() as i64) } else { None };
                let pick = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                let error = if !pick("detail").is_empty() { pick("detail") } else { pick("error") };
                let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| c.on_latency(id, (ok, ms, error, key)))));
            });
        }
    }

    fn on_latency(&mut self, id: String, result: (bool, Option<i64>, String, Value)) {
        self.latency_busy.remove(&id);
        let ok = result.0;
        self.latency.insert(id, result);
        let mut finished = None;
        if let Some((total, done, oks)) = self.latency_round.as_mut() {
            *done += 1;
            *oks += usize::from(ok);
            if *done >= *total {
                finished = Some((*total, *oks));
            }
        }
        if let Some((total, oks)) = finished {
            self.latency_round = None;
            let text = tr("{ok} working, {failed} not responding").replace("{ok}", &oks.to_string()).replace("{failed}", &(total - oks).to_string());
            let msg = json!({"ev": "info", "message": format!("{} · {text}", tr("Latency test finished"))});
            self.broadcast(&msg);
        }
        self.send_latency();
    }

    /// The badges on outbound blocks (Python `update_latency`): "…" while testing, the time (tone 1 fast, 2 slow,
    /// 3 very slow) or "✕" (3) for a block that did not respond; a result shows only while its block is unchanged.
    fn latency_json(&self) -> Value {
        let mut badges = Map::new();
        for n in self.graph.get("outbounds").and_then(Value::as_array).into_iter().flatten() {
            let id = n["id"].as_str().unwrap_or("");
            let badge = if self.latency_busy.contains(id) {
                Some(("…".to_string(), 0))
            } else {
                match self.latency.get(id) {
                    Some((ok, ms, _, key)) if *key == proxydock_core::probe::latency_key(&self.graph, id) => Some(match (ok, ms) {
                        (true, Some(ms)) => (format!("{ms} ms"), if *ms < 400 { 1 } else if *ms < 1000 { 2 } else { 3 }),
                        _ => ("✕".to_string(), 3),
                    }),
                    _ => None,
                }
            };
            if let Some((text, tone)) = badge {
                badges.insert(id.into(), json!([text, tone]));
            }
        }
        let progress = match self.latency_round {
            Some((total, done, _)) => tr("Testing… {done}/{total}").replace("{done}", &done.to_string()).replace("{total}", &total.to_string()),
            None => String::new(),
        };
        json!({"badges": badges, "progress": progress})
    }

    fn send_latency(&mut self) {
        let msg = json!({"ev": "latency", "latency": self.latency_json()});
        self.broadcast(&msg);
    }

    /// settings file -> global settings and the compiled cards; then the autostart cards start
    fn load(&mut self) {
        match config::load_from(&self.config_path) {
            Ok(l) => {
                self.glob = l.global;
                proxydock_core::i18n::set_language(self.glob["language"].as_str().unwrap_or("en"));
                self.graph = graph::validate(&Value::Object(l.graph));
                self.can_save = true;
                for (id, field) in &l.notes.undecryptable {
                    let msg = format!("{id}: {field:?}");
                    self.app_log(Level::Warn, &msg);
                }
                let to_file = self.glob["log_to_file"] == json!(true);
                self.apply_file_logging(to_file);
                if l.notes.needs_save {
                    // plain-text passwords of an older version: saved sealed right away (as the Python app does)
                    self.save();
                }
            }
            Err(config::LoadError::Missing) => {
                self.can_save = true;
                self.save();
            }
            Err(e) => {
                // never overwrite a file this version cannot read (a newer or broken one)
                let msg = format!("{}: {e:?}", self.config_path.display());
                self.app_log(Level::Error, &msg);
            }
        }
        self.rebuild();
        self.undo_base = self.snapshot();
        *self.shared_glob.lock().unwrap() = self.glob.clone();
        self.engine_text = engine_text(&self.glob);
        if self.glob["singbox_latest"] == json!(true) {
            self.update_latest_singbox();
        }
        let auto: Vec<Value> = self.proxies.iter().filter(|p| p["autostart"] == json!(true)).cloned().collect();
        for p in auto {
            let _ = self.manager.start(&p, &self.glob);
        }
    }

    /// cards and canvas from the graph
    fn rebuild(&mut self) {
        self.proxies = graph::compile(&self.graph).into_iter().map(Value::Object).collect();
        let summaries: HashMap<String, String> =
            self.proxies.iter().map(|p| (p["id"].as_str().unwrap_or("").to_string(), manager::summary(p))).collect();
        self.view = canvas::build(&self.graph, &|id| summaries.get(id).cloned().unwrap_or_default());
        self.publish_cards();
    }

    fn save(&mut self) {
        if !self.can_save {
            return;
        }
        if let Err(e) = config::save_to(&self.config_path, &self.glob, &self.graph) {
            let msg = tr("Failed to save settings: {e}").replace("{e}", &format!("{e:?}"));
            self.app_log(Level::Warn, &msg);
        }
        self.note_change();
    }

    /// The graph changed (Python `_recompile`): rebuild the cards, apply to running ones that changed (a card
    /// left with nothing to send to stops), stop deleted ones, save, and show the window the new canvas.
    fn graph_changed(&mut self) {
        let old: HashMap<String, Value> =
            self.proxies.iter().map(|p| (p["id"].as_str().unwrap_or("").to_string(), p.clone())).collect();
        self.rebuild();
        let mut alive = std::collections::HashSet::new();
        for p in self.proxies.clone() {
            let id = p["id"].as_str().unwrap_or("").to_string();
            alive.insert(id.clone());
            if !matches!(self.manager.state(&id), State::Running | State::Starting) {
                continue;
            }
            let same = old.get(&id).is_some_and(|b| ["name", "inbound", "outbound", "overrides", "routes"].iter().all(|k| b.get(*k) == p.get(*k)));
            if same {
                continue;
            }
            if p["unlinked"] == json!(true) {
                let _ = self.manager.stop(&id);
            } else {
                let _ = self.manager.apply(&p, &self.glob);
            }
        }
        for id in old.keys().filter(|id| !alive.contains(*id)) {
            self.restart_at.remove(id);
            let _ = self.manager.stop(id);
        }
        self.save();
        self.send_canvas();
        self.send_latency();
    }

    fn send_canvas(&mut self) {
        let cards: Vec<Value> = self.proxies.iter().map(|p| self.card_json(p)).collect();
        let canvas = canvas::to_json(&self.view, &|id| state_name(&self.manager.state(id)));
        self.broadcast(&json!({"ev": "graph", "cards": cards, "canvas": canvas}));
    }

    /// The block editor. open: a draft of the block and its fields; preview: the typed values go into the draft and
    /// the fields come back (which ones show depends on them); action: a button on the draft; save: the draft is
    /// checked and saved like a canvas edit.
    fn editor(&mut self, client: u64, cmd: &str, v: &Value) {
        let reply = |c: &mut Self, msg: Value| {
            if let Some(cl) = c.clients.get_mut(&client) {
                let _ = ipc::send(&mut cl.stream, &msg);
            }
        };
        let values = v["values"].as_object().cloned().unwrap_or_default();
        if cmd == "edit_open" {
            let key = v["id"].as_str().unwrap_or("");
            let id = key.strip_suffix("/out").unwrap_or(key).to_string();
            let Some((kind, node)) = graph::find(&self.graph, &id).map(|(k, n)| (k, n.clone())) else { return };
            if !matches!(kind, "inbound" | "outbound" | "router" | "relay") {
                reply(self, json!({"ev": "error", "message": tr("This block has no settings to edit yet")}));
                return;
            }
            let mut node = node;
            form::on_open(kind, &mut node);
            self.drafts.insert(client, (kind, node));
            self.probe_text.remove(&client);
        }
        let Some((kind, draft)) = self.drafts.get(&client).cloned() else { return };
        let mut node = form::apply(kind, &draft, &values);
        let id = node["id"].as_str().unwrap_or("").to_string();
        let action = v["action"].as_str().unwrap_or("");
        if cmd == "edit_action" && action == "port.check" {
            self.check_port(client, &node);
        }
        if cmd == "edit_action" && NETWORK_ACTIONS.contains(&action) {
            self.drafts.insert(client, (kind, node.clone()));
            self.start_work(client, action, kind, &node);
            self.send_form(client);
            return;
        }
        if cmd == "edit_action" {
            match form::act(kind, &node, action, &values) {
                Ok(done) => {
                    node = done.node;
                    if let Some((title, text)) = done.show {
                        reply(self, json!({"ev": "show_text", "title": title, "text": text}));
                    }
                    if let Some((ok, title, text)) = done.notice {
                        reply(self, json!({"ev": "notice", "ok": ok, "title": title, "text": text}));
                    }
                }
                Err(msg) if !msg.is_empty() => reply(self, json!({"ev": "form_errors", "errors": [msg]})),
                Err(_) => {}
            }
        }
        self.drafts.insert(client, (kind, node.clone()));
        if cmd != "edit_save" {
            self.send_form(client);
            return;
        }
        if !self.can_save {
            reply(self, json!({"ev": "form_errors", "errors": [tr("Save failed")]}));
            return;
        }
        let errs = form::errors(&self.graph, kind, &node);
        if !errs.is_empty() {
            reply(self, json!({"ev": "form_errors", "errors": errs}));
            return;
        }
        if kind == "inbound" {
            let ib = &node["inbound"];
            let host = ib["host"].as_str().unwrap_or("");
            let local = host.eq_ignore_ascii_case("localhost") || proxydock_core::singbox::guard::is_loopback_ip(host);
            if !local && matches!(ib["type"].as_str(), Some("socks5" | "http" | "https")) && ib["auth_enabled"] != json!(true) {
                self.app_log(Level::Warn, &tr("Anyone on the same network can use this proxy. Turning on user authentication is recommended."));
            }
        }
        self.graph = form::with_node(&self.graph, kind, &node);
        self.drafts.remove(&client);
        let name = node["name"].as_str().unwrap_or("").to_string();
        self.app_log(Level::Setting, &tr("Saved proxy '{name}'").replace("{name}", &name));
        self.graph_changed();
        reply(self, json!({"ev": "form_saved", "id": id}));
    }

    /// the editor's fields for the window's draft (buttons whose work runs are shown busy)
    fn send_form(&mut self, client: u64) {
        let Some((kind, node)) = self.drafts.get(&client).cloned() else { return };
        let id = node["id"].as_str().unwrap_or("").to_string();
        let fields: Vec<Value> = form::form(&self.graph, &self.glob, kind, &node)
            .iter()
            .map(|f| {
                let mut j = f.to_json();
                if self.busy.contains(&(client, f.key.clone())) {
                    j["label"] = json!(tr(busy_label(&f.key)));
                    j["busy"] = json!(true);
                }
                if f.key == "probe" {
                    if let Some((_, text, tone)) = self.probe_text.get(&client).filter(|(b, _, _)| *b == id) {
                        j["desc"] = json!(text);
                        j["tone"] = json!(tone);
                    }
                }
                j
            })
            .collect();
        let title = graph::find(&self.graph, &id).and_then(|(_, n)| n["name"].as_str().map(String::from)).unwrap_or_default();
        let msg = json!({"ev": "form", "id": id, "kind": kind, "title": title, "fields": fields});
        self.send_to(client, &msg);
    }

    fn send_to(&mut self, client: u64, msg: &Value) {
        if let Some(cl) = self.clients.get_mut(&client) {
            let _ = ipc::send(&mut cl.stream, msg);
        }
    }

    fn notice(&mut self, client: u64, ok: bool, title: &str, text: &str) {
        self.send_to(client, &json!({"ev": "notice", "ok": ok, "title": title, "text": text}));
    }

    /// An editor button that needs the network: the work runs on its own thread and its result goes into the
    /// window's draft when it is still editing the same block.
    fn start_work(&mut self, client: u64, action: &str, kind: &'static str, node: &Value) {
        let key = (client, action.to_string());
        if self.busy.contains(&key) {
            return;
        }
        let Some(job) = self.network_job(client, action, kind, node) else { return };
        self.busy.insert(key.clone());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let apply = job();
            let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| {
                c.busy.remove(&key);
                apply(c);
                c.send_form(key.0);
            })));
        });
    }

    /// the work of a network button (None: nothing to do, a message was shown)
    fn network_job(&mut self, client: u64, action: &str, kind: &'static str, node: &Value) -> Option<Job> {
        let id = node["id"].as_str().unwrap_or("").to_string();
        match action {
            "sub.update" => {
                let url = proxydock_core::py::strip(node["outbound"]["sub_url"].as_str().unwrap_or("")).to_string();
                if url.is_empty() {
                    self.notice(client, false, &tr("Subscription"), &tr("Enter the subscription address (https://...)"));
                    return None;
                }
                Some(Box::new(move || {
                    let result = proxydock_core::subscription::update(&url);
                    Box::new(move |c: &mut Controller| {
                        let (ok, text) = match result {
                            Ok((fields, note)) => {
                                c.with_draft(client, &id, |n| {
                                    for (k, v) in fields {
                                        n["outbound"][k] = v;
                                    }
                                });
                                (true, note)
                            }
                            Err(e) => (false, e),
                        };
                        c.notice(client, ok, &tr("Subscription"), &text);
                    })
                }))
            }
            "sni.detect" => {
                let ob = node["outbound"].clone();
                if proxydock_core::py::strip(ob["host"].as_str().unwrap_or("")).is_empty() || ob["port"].as_i64().unwrap_or(0) == 0 {
                    self.notice(client, false, &tr("Detect from server"), &tr("Enter the server address and port first"));
                    return None;
                }
                Some(Box::new(move || {
                    let result = proxydock_core::certinfo::detect(&ob);
                    Box::new(move |c: &mut Controller| match result {
                        Ok(d) => {
                            if !d.sni.is_empty() {
                                c.with_draft(client, &id, |n| n["outbound"]["sni"] = json!(d.sni));
                            }
                            let (ok, title, text) = proxydock_core::certinfo::message(&d);
                            c.notice(client, ok, &title, &text);
                        }
                        Err(e) => c.notice(client, false, &tr("Detect from server"), &e),
                    })
                }))
            }
            "probe" => {
                if kind == "outbound" && node["outbound"]["type"] == json!("block") {
                    let text = tr("A Block outbound refuses every connection, so there is nothing to check.");
                    self.probe_text.insert(client, (id, text, 0));
                    return None;
                }
                let errs = form::check_errors(&self.graph, kind, node);
                if !errs.is_empty() {
                    self.notice(client, false, &tr("Please fix first"), &errs.iter().take(4).cloned().collect::<Vec<_>>().join("\n"));
                    return None;
                }
                let p = form::as_proxy(&self.graph, kind, node)?;
                let inbound = kind == "inbound";
                // the card runs on these settings already: its port is in use by itself
                let running = inbound
                    && self.manager.state(&id) == State::Running
                    && graph::find(&self.graph, &id).is_some_and(|(_, saved)| {
                        saved["inbound"]["host"] == node["inbound"]["host"] && saved["inbound"]["port"] == node["inbound"]["port"]
                    });
                self.probe_text.insert(client, (id.clone(), tr("Connecting through the outbound\u{2026}"), 0));
                let (remote, glob, ot) = (self.manager.remote(), self.glob.clone(), p["outbound"]["type"].as_str().unwrap_or("direct").to_string());
                let check_id = format!("__check_{client}__");
                Some(Box::new(move || {
                    let out = proxydock_core::probe::check_outbound(&remote, &p, &glob, &check_id);
                    let inb = inbound.then(|| proxydock_core::probe::check_inbound(&p, running));
                    let (ok, text) = proxydock_core::probe::result_text(&ot, &out, inb);
                    Box::new(move |c: &mut Controller| {
                        if c.drafts.get(&client).is_some_and(|(_, n)| n["id"] == json!(id)) {
                            c.probe_text.insert(client, (id.clone(), text.clone(), if ok { 1 } else { 2 }));
                            let title = tr("Connection check ") + &tr(if ok { "succeeded" } else { "failed" });
                            c.notice(client, ok, &title, text.lines().next().unwrap_or(""));
                        }
                    })
                }))
            }
            _ => None,
        }
    }

    /// "Check availability" next to a card's port (Python `_check_port`)
    fn check_port(&mut self, client: u64, node: &Value) {
        let ib = &node["inbound"];
        let host = match proxydock_core::py::strip(ib["host"].as_str().unwrap_or("")) {
            "" => "127.0.0.1".to_string(),
            h => h.to_string(),
        };
        let port = ib["port"].as_i64().unwrap_or(0).clamp(0, 65535) as u16;
        let id = node["id"].as_str().unwrap_or("");
        let saved_same = graph::find(&self.graph, id).is_some_and(|(_, saved)| saved["inbound"]["port"] == json!(port) && saved["inbound"]["host"] == json!(host));
        if self.manager.state(id) == State::Running && saved_same {
            let title = tr("Port {port}").replace("{port}", &port.to_string());
            self.notice(client, true, &title, &tr("This proxy is running on this port."));
            return;
        }
        let (ok, msg) = proxydock_core::probe::check_port(&host, port, ib["type"] == json!("hysteria2"));
        let title = tr("Port {port} ").replace("{port}", &port.to_string()) + &tr(if ok { "available" } else { "unavailable" });
        self.notice(client, ok, &title, &msg);
    }

    /// change the window's draft if it still edits block `id`
    fn with_draft(&mut self, client: u64, id: &str, f: impl FnOnce(&mut Value)) {
        if let Some((_, n)) = self.drafts.get_mut(&client) {
            if n["id"] == json!(id) {
                f(n);
            }
        }
    }

    /// Refresh the subscription blocks whose interval has passed, in the background (Python
    /// `update_due_subscriptions`): changed servers apply to the cards, the same ones only move the update time.
    fn update_due_subscriptions(&mut self) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let due: Vec<(String, String)> = self.graph.get("outbounds").and_then(Value::as_array).into_iter().flatten()
            .filter(|n| n["outbound"]["type"] == json!("subscription"))
            .filter(|n| {
                let ob = &n["outbound"];
                let every = ob["sub_interval"].as_i64().unwrap_or(0);
                every > 0 && now - ob["sub_updated"].as_i64().unwrap_or(0) >= every * 3600
            })
            .map(|n| (n["id"].as_str().unwrap_or("").to_string(), n["outbound"]["sub_url"].as_str().unwrap_or("").to_string()))
            .filter(|(id, _)| !self.sub_busy.contains(id))
            .collect();
        for (id, url) in due {
            self.sub_busy.insert(id.clone());
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let result = proxydock_core::subscription::update(&url);
                let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| c.on_sub_updated(&id, result))));
            });
        }
    }

    fn on_sub_updated(&mut self, id: &str, result: Result<(Map<String, Value>, String), String>) {
        self.sub_busy.remove(id);
        let Some(name) = graph::find(&self.graph, id).filter(|(k, _)| *k == "outbound").map(|(_, n)| n["name"].as_str().unwrap_or("").to_string()) else { return };
        let (fields, note) = match result {
            Ok(x) => x,
            Err(e) => {
                let msg = tr("Subscription '{name}' could not be updated: {note}").replace("{name}", &name).replace("{note}", &e);
                self.app_log(Level::Warn, &msg);
                return;
            }
        };
        let mut same = false;
        for n in self.graph.get_mut("outbounds").and_then(Value::as_array_mut).into_iter().flatten() {
            if n["id"] == json!(id) {
                same = n["outbound"]["sub_servers"] == fields["sub_servers"];
                for (k, v) in &fields {
                    if !same || k == "sub_updated" {
                        n["outbound"][k] = v.clone();
                    }
                }
            }
        }
        self.undo_quiet = true;
        if same {
            self.save();
        } else {
            let msg = tr("Subscription '{name}' updated: {note}").replace("{name}", &name).replace("{note}", &note);
            self.app_log(Level::Setting, &msg);
            self.graph_changed();
        }
        self.undo_quiet = false;
    }

    /// a canvas edit from the window
    fn edit(&mut self, cmd: &str, v: &Value) {
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        let n = |k: &str| v[k].as_u64().unwrap_or(0) as usize;
        let f = |k: &str| v[k].as_f64().unwrap_or(0.0);
        if !self.can_save {
            self.broadcast(&json!({"ev": "error", "message": tr("Save failed")}));
            self.send_canvas();
            return;
        }
        let result = match cmd {
            "move" => {
                for m in v["blocks"].as_array().into_iter().flatten() {
                    let key = m["key"].as_str().unwrap_or("");
                    edit::move_block(&mut self.graph, key, m["x"].as_f64().unwrap_or(0.0), m["y"].as_f64().unwrap_or(0.0));
                }
                // positions only: saved (the window already shows them)
                self.rebuild();
                self.save();
                return;
            }
            "connect" => edit::connect(&mut self.graph, &s("from"), n("out"), &s("to"), n("in")),
            "disconnect" => edit::disconnect(&mut self.graph, &s("from"), n("out"), &s("to"), n("in")),
            "delete" => {
                // selected links are cut first (a refused one is skipped), then the selected blocks deleted
                for l in v["links"].as_array().into_iter().flatten() {
                    let s = |k: &str| l[k].as_str().unwrap_or("").to_string();
                    let n = |k: &str| l[k].as_u64().unwrap_or(0) as usize;
                    let _ = edit::disconnect(&mut self.graph, &s("from"), n("out"), &s("to"), n("in"));
                }
                for k in v["keys"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                    edit::delete(&mut self.graph, k);
                }
                Ok(())
            }
            "duplicate" => {
                let keys: Vec<&str> = v["keys"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                if edit::duplicate(&mut self.graph, &keys).is_empty() {
                    return;
                }
                Ok(())
            }
            "rename" => match edit::rename(&mut self.graph, &s("key"), &s("name")) {
                Some(name) => {
                    self.app_log(Level::Setting, &tr("Saved proxy '{name}'").replace("{name}", &name));
                    Ok(())
                }
                None => {
                    // nothing to change: the window shows the saved title again
                    self.send_canvas();
                    return;
                }
            },
            "add" => {
                edit::add(&mut self.graph, v["kind"].as_i64().unwrap_or(2) as i32, f("x"), f("y"));
                Ok(())
            }
            _ => return,
        };
        match result {
            Ok(()) => self.graph_changed(),
            Err(msg) => {
                self.broadcast(&json!({"ev": "error", "message": msg}));
                self.send_canvas(); // the refused link disappears again
            }
        }
    }

    fn proxy(&self, id: &str) -> Option<&Value> {
        self.proxies.iter().find(|p| p["id"] == json!(id))
    }

    fn card_json(&self, p: &Value) -> Value {
        let id = p["id"].as_str().unwrap_or("");
        let st = self.manager.state(id);
        let error = if let State::Error(m) = &st { m.clone() } else { String::new() };
        json!({
            "id": id, "name": p["name"], "summary": manager::summary(p), "state": state_name(&st), "error": error,
            "autostart": p["autostart"], "auto_restart": p["auto_restart"],
        })
    }

    fn publish_cards(&self) {
        *self.cards.lock().unwrap() = self
            .proxies
            .iter()
            .map(|p| CardInfo {
                id: p["id"].as_str().unwrap_or("").into(),
                name: p["name"].as_str().unwrap_or("").into(),
                state: self.manager.state(p["id"].as_str().unwrap_or("")),
                autostart: p["autostart"] == json!(true),
            })
            .collect();
    }

    fn broadcast(&mut self, v: &Value) {
        let mut gone = vec![];
        for (id, c) in self.clients.iter_mut().filter(|(_, c)| c.ready) {
            if ipc::send(&mut c.stream, v).is_err() {
                gone.push(*id);
            }
        }
        for id in gone {
            self.clients.remove(&id);
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        while let Ok(m) = rx.recv() {
            match m {
                Msg::Status(id, st) => self.on_status(&id, st),
                Msg::Log(e) => self.add_log(e),
                Msg::Client(n, s) => self.on_client(n, s),
                Msg::Command(n, v) => self.on_command(n, &v),
                Msg::ClientGone(n) => {
                    self.clients.remove(&n);
                }
                Msg::Tick => self.on_tick(),
                Msg::Work(f) => f(&mut self),
                Msg::Toggle(id) => self.toggle(&id),
                Msg::StopAll => self.stop_all(),
                Msg::Quit(done) => {
                    self.restart_at.clear();
                    self.manager.stop_all(Duration::from_secs(10));
                    let _ = done.send(());
                    break;
                }
            }
            // one message, one undo step (Python: one turn of the event loop)
            self.undo_open = false;
        }
    }

    fn on_client(&mut self, n: u64, s: TcpStream) {
        let Ok(reader) = s.try_clone() else { return };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            for v in ipc::messages(reader) {
                if tx.send(Msg::Command(n, v)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::ClientGone(n));
        });
        self.clients.insert(n, Client { stream: s, ready: false });
    }

    fn on_command(&mut self, n: u64, v: &Value) {
        let cmd = v["cmd"].as_str().unwrap_or("");
        let ready = self.clients.get(&n).is_some_and(|c| c.ready);
        if !ready {
            if cmd == "hello" && ipc::same(v["token"].as_str().unwrap_or(""), &self.token) {
                let cards: Vec<Value> = self.proxies.iter().map(|p| self.card_json(p)).collect();
                let skip = self.logs.len().saturating_sub(LOG_SEND);
                let logs: Vec<Value> = self.logs.iter().skip(skip).map(entry_json).collect();
                let canvas = canvas::to_json(&self.view, &|id| state_name(&self.manager.state(id)));
                let hello = json!({"ev": "hello", "language": self.glob["language"], "cards": cards, "logs": logs, "stats": self.stats_json(), "canvas": canvas, "undo": !self.undo.is_empty(), "redo": !self.redo.is_empty(),
                    "settings": self.settings_json(), "latency": self.latency_json()});
                if let Some(c) = self.clients.get_mut(&n) {
                    c.ready = ipc::send(&mut c.stream, &hello).is_ok();
                }
            } else {
                self.clients.remove(&n); // wrong token: drop it
            }
            return;
        }
        let id = v["id"].as_str().unwrap_or("");
        match cmd {
            "start" => {
                if let Some(p) = self.proxy(id).cloned() {
                    self.restart_at.remove(id);
                    let _ = self.manager.start(&p, &self.glob);
                }
            }
            "stop" => {
                self.restart_at.remove(id);
                let _ = self.manager.stop(id);
            }
            "toggle" => self.toggle(id),
            "stop_all" => self.stop_all(),
            "move" | "connect" | "disconnect" | "delete" | "add" | "duplicate" | "rename" => self.edit(cmd, v),
            "settings_save" => self.save_settings(v),
            "latency" => {
                let keys: Vec<String> = v["keys"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
                self.test_latency(&keys);
            }
            "quit" => (self.hooks.quit)(),
            "undo" => self.undo_step(false),
            "redo" => self.undo_step(true),
            "edit_open" | "edit_preview" | "edit_action" | "edit_save" => self.editor(n, cmd, v),
            "edit_close" => {
                self.drafts.remove(&n);
            }
            _ => {}
        }
    }

    fn toggle(&mut self, id: &str) {
        let Some(p) = self.proxy(id).cloned() else { return };
        self.restart_at.remove(id);
        match self.manager.state(id) {
            State::Running | State::Starting => {
                let _ = self.manager.stop(id);
            }
            _ => {
                let _ = self.manager.start(&p, &self.glob);
            }
        }
    }

    fn stop_all(&mut self) {
        self.restart_at.clear();
        let ids: Vec<String> = self.proxies.iter().filter_map(|p| p["id"].as_str().map(String::from)).collect();
        for id in ids {
            let _ = self.manager.stop(&id);
        }
    }

    fn on_status(&mut self, id: &str, st: State) {
        if st == State::Running {
            self.running_since.insert(id.to_string(), Instant::now());
        }
        if let (State::Error(msg), Some(p)) = (&st, self.proxy(id).cloned()) {
            if p["auto_restart"] == json!(true) {
                self.schedule_restart(&p);
            } else {
                let text = tr("'{name}' failed to start: {msg}").replace("{name}", &name_of(&p)).replace("{msg}", msg);
                self.notify_user(&text);
            }
        }
        if let Some(p) = self.proxy(id).cloned() {
            let card = self.card_json(&p);
            self.broadcast(&json!({"ev": "card", "card": card}));
        }
        self.publish_cards();
    }

    fn schedule_restart(&mut self, p: &Value) {
        let id = p["id"].as_str().unwrap_or("").to_string();
        if self.restart_at.contains_key(&id) {
            return;
        }
        if self.running_since.get(&id).is_none_or(|t| t.elapsed() > Duration::from_secs(120)) {
            self.restart_count.insert(id.clone(), 0); // it ran a while (or never): count again
        }
        let n = self.restart_count.get(&id).copied().unwrap_or(0);
        let delay = RESTART_DELAYS[n.min(RESTART_DELAYS.len() - 1)];
        self.restart_count.insert(id.clone(), n + 1);
        let msg = tr("'{name}' stopped; restarting in {sec} s (attempt {n})")
            .replace("{name}", &name_of(p))
            .replace("{sec}", &delay.to_string())
            .replace("{n}", &(n + 1).to_string());
        self.app_log(Level::Warn, &msg);
        self.restart_at.insert(id, Instant::now() + Duration::from_secs(delay));
    }

    fn stats_json(&self) -> Value {
        let mut out = Map::new();
        for p in &self.proxies {
            let id = p["id"].as_str().unwrap_or("");
            let s = self.manager.stats(id);
            out.insert(id.into(), json!({"up": s.up, "down": s.down, "tcp": s.tcp_active, "udp": s.udp_active, "total": s.total, "failed": s.failed, "blocked": s.blocked}));
        }
        Value::Object(out)
    }

    fn on_tick(&mut self) {
        let now = Instant::now();
        if now >= self.next_sub_check {
            self.next_sub_check = now + SUB_EVERY;
            self.update_due_subscriptions();
        }
        let due: Vec<String> = self.restart_at.iter().filter(|(_, t)| **t <= now).map(|(id, _)| id.clone()).collect();
        for id in due {
            self.restart_at.remove(&id);
            let Some(p) = self.proxy(&id).cloned() else { continue };
            if p["auto_restart"] == json!(true) && !matches!(self.manager.state(&id), State::Running | State::Starting) {
                let _ = self.manager.start(&p, &self.glob);
            }
        }
        if self.clients.values().any(|c| c.ready) {
            let stats = self.stats_json();
            self.broadcast(&json!({"ev": "stats", "stats": stats}));
            self.send_flow();
        }
    }
}

impl Controller {
    /// bytes per second on each canvas link over the last tick: each running card's traffic to each exit block
    /// (from its sing-box connections) counted on the links of that card's way to the exit
    fn send_flow(&mut self) {
        let mut rates = vec![0.0f64; self.view.links.len()];
        let mut now: HashMap<(String, String), f64> = HashMap::new();
        for p in &self.proxies {
            let id = p["id"].as_str().unwrap_or("");
            for (exit, (up, down)) in self.manager.exit_totals(id) {
                now.insert((id.to_string(), exit), up + down);
            }
        }
        for (key, total) in &now {
            let before = self.flow_last.get(key).copied().unwrap_or(*total);
            let rate = (total - before).max(0.0);
            if rate > 0.0 {
                for li in self.view.paths.get(key).into_iter().flatten() {
                    rates[*li] += rate;
                }
            }
        }
        self.flow_last = now;
        // nothing flowing now and nothing shown: no message
        if rates.iter().all(|r| *r == 0.0) && self.flow_sent.iter().all(|r| *r == 0.0) {
            return;
        }
        self.broadcast(&json!({"ev": "flow", "links": rates}));
        self.flow_sent = rates;
    }
}

/// the settings page's sing-box line: the program the cards use and its version
fn engine_text(glob: &Map<String, Value>) -> String {
    match proxydock_core::singbox::find_binary(glob) {
        Some(exe) => match proxydock_core::singbox::version::of(&exe) {
            Some(v) => format!("sing-box {v} · {}", exe.display()),
            None => exe.display().to_string(),
        },
        None => tr("sing-box was not found"),
    }
}

fn name_of(p: &Value) -> String {
    p["name"].as_str().unwrap_or("").to_string()
}
