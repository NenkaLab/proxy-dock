//! The core process's controller (Python: the non-UI half of `ui/window.py`): reads the settings, runs the cards
//! through the manager (autostart, auto-restart with back-off), keeps the recent log, and serves the window
//! process over ipc. Everything happens on one controller thread fed by a channel.

use crate::{canvas, ipc};
use proxydock_core::i18n::tr;
use proxydock_core::log::{Entry, Level};
use proxydock_core::manager::{self, Events, Manager, State};
use proxydock_core::{config, edit, form, graph, log};
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet, VecDeque};

/// a canvas block's port: the block's key and the port's index (auto ports)
type PortRef = (String, usize);
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
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
    /// a command from the tray menu (start at login, the PC block)
    Tray(Value),
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
    /// start again: the whole app (true) or only the window (false: a new app scale)
    pub restart: Arc<dyn Fn(bool) + Send + Sync>,
    /// the tray icon's tooltip (cards running, rates, connections), once a second when it changes
    pub tooltip: Notify,
}

impl Default for Hooks {
    fn default() -> Self {
        Hooks { notify: Arc::new(|_| {}), quit: Arc::new(|| {}), restart: Arc::new(|_| {}), tooltip: Arc::new(|_| {}) }
    }
}

/// seconds of throughput the statistics graph shows (Python `stats.POINTS`)
pub const STAT_POINTS: usize = 60;

/// the first automatic update check after starting (Python: 10 s)
const UPDATE_FIRST: Duration = Duration::from_secs(10);

/// updates (Python `ui/updater.py`): the newer release found, what the About page says, a download in progress
#[derive(Default)]
struct UpdateState {
    release: Option<proxydock_core::update::Release>,
    status: String,
    checking: bool,
    downloading: bool,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    /// the startup check (once, when due)
    next_check: Option<Instant>,
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

/// a hand-picked group for the tray: (id, name, [(member id, name)], the member in use)
pub type QuickGroup = (String, String, Vec<(String, String)>, String);

/// What the tray menu can change without the window (Python `_tray_quick_actions`)
#[derive(Clone, Debug, Default)]
pub struct Quick {
    /// IF blocks in switch mode: (id, name, on)
    pub switches: Vec<(String, String, bool)>,
    /// groups that use the member picked by hand: (id, name, [(member id, name)], the member in use)
    pub groups: Vec<QuickGroup>,
    /// ProxyDock's own switch (home page)
    pub app_on: bool,
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
    pc: Arc<Mutex<Option<String>>>,
    quick: Arc<Mutex<Quick>>,
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

    /// a warning in the log (from the tray process)
    pub fn warn(&self, message: &str) {
        let m = message.to_string();
        let _ = self.tx.send(Msg::Work(Box::new(move |c: &mut Controller| c.app_log(Level::Warn, &m))));
    }

    /// the tray menu: "startup" {on}, "startup_hidden" {on}, "pc_off"
    pub fn tray(&self, cmd: Value) {
        let _ = self.tx.send(Msg::Tray(cmd));
    }

    /// the card the PC block uses as the system proxy (its name), if any
    pub fn pc_card(&self) -> Option<String> {
        self.pc.lock().unwrap().clone()
    }

    /// the IF switches and hand-picked groups for the tray menu
    pub fn quick(&self) -> Quick {
        self.quick.lock().unwrap().clone()
    }

    /// close the window only (it keeps its place; the tray and the cards go on)
    pub fn close_window(&self) {
        let _ = self.tx.send(Msg::Work(Box::new(|c: &mut Controller| c.broadcast(&json!({"ev": "close_window"})))));
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
    let pc = Arc::new(Mutex::new(None));
    let quick = Arc::new(Mutex::new(Quick::default()));
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
        tooltip: String::new(),
        drafts: HashMap::new(),
        flow_last: HashMap::new(),
        flow_sent: vec![],
        busy: std::collections::HashSet::new(),
        sub_busy: std::collections::HashSet::new(),
        next_sub_check: Instant::now() + SUB_FIRST,
        probe_text: HashMap::new(),
        hooks,
        shared_glob: shared_glob.clone(),
        pc_card: pc.clone(),
        quick: quick.clone(),
        firewall: None,
        bypass_reason: Some(String::new()),
        bypass_checked: false,
        app_on: true,
        bypass_adapters: vec![],
        bypass_working: false,
        bypass_progress: String::new(),
        fw_busy: false,
        engines: None,
        update: UpdateState { next_check: Some(Instant::now() + UPDATE_FIRST), ..Default::default() },
        engine_text: String::new(),
        latency: HashMap::new(),
        latency_busy: std::collections::HashSet::new(),
        latency_round: None,
        stat_last: HashMap::new(),
        stat_last_t: Instant::now(),
        stat_hist: HashMap::new(),
        exit_last: HashMap::new(),
        exit_hist: HashMap::new(),
        conn_running: false,
        conn_ok: vec![],
        conn_total: 0,
        file_log: None,
        undo: vec![],
        redo: vec![],
        undo_base: Value::Null,
        undo_open: false,
        undo_quiet: false,
    };
    c.load();
    std::thread::Builder::new().name("controller".into()).spawn(move || c.run(rx))?;
    Ok(Handle { tx, port, token, cards, glob: shared_glob, pc, quick })
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
    /// the tray tooltip last sent
    tooltip: String,
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
    /// the PC block's card (its name) for the tray menu
    pc_card: Arc<Mutex<Option<String>>>,
    quick: Arc<Mutex<Quick>>,
    /// the firewall block rule: None while it is being checked or changed
    firewall: Option<bool>,
    /// VPN bypass (Settings > VPN bypass): why it cannot run here (None = it can), the physical network cards,
    /// a VM being prepared / removed and what it is doing
    bypass_reason: Option<String>,
    /// the first check after startup is done (kept-on VMs start once)
    bypass_checked: bool,
    /// ProxyDock's own switch (home page): off = every card refuses all connections
    app_on: bool,
    bypass_adapters: Vec<String>,
    bypass_working: bool,
    bypass_progress: String,
    fw_busy: bool,
    /// the About page's engines: [name, program, version] (None while they are being asked)
    engines: Option<Vec<Value>>,
    update: UpdateState,
    /// the settings page's sing-box line (path and version)
    engine_text: String,
    /// latency of outbound blocks: id -> (ok, ms, error, what it depends on)
    latency: HashMap<String, (bool, Option<i64>, String, Value)>,
    latency_busy: std::collections::HashSet<String>,
    /// the running test: (total, done, ok)
    latency_round: Option<(usize, usize, usize)>,
    /// statistics: each card's up / down bytes at the last tick, and the last minute per second of each card
    /// ("" = all of them: up, down) and of each outbound block (up + down)
    stat_last: HashMap<String, (f64, f64)>,
    stat_last_t: Instant,
    stat_hist: HashMap<String, VecDeque<(f64, f64)>>,
    exit_last: HashMap<String, (f64, f64)>,
    exit_hist: HashMap<String, VecDeque<f64>>,
    /// a connection test runs; the times of the sites that answered and how many sites
    conn_running: bool,
    conn_ok: Vec<f64>,
    conn_total: usize,
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
    json!({"time": e.time, "level": e.level.tag(), "message": e.message, "name": e.proxy_name, "id": e.proxy_id})
}

/// The client and destination of a connection log line: "who -> host:port …" or "who UDP session ended …"
pub fn conn_of_line(message: &str) -> Option<(String, Option<String>)> {
    if let Some((who, rest)) = message.split_once(" -> ") {
        let target = rest.split_whitespace().next()?.to_string();
        return Some((who.trim().to_string(), Some(target)));
    }
    let (who, _) = message.split_once(" UDP ")?;
    (!who.contains(' ')).then(|| (who.to_string(), None))
}

impl Controller {
    fn app_log(&mut self, level: Level, message: &str) {
        let e = Entry {
            time: log::now_text(),
            level,
            message: log::clean(message),
            proxy_id: String::new(),
            proxy_name: String::new(),
        };
        self.add_log(e);
    }

    fn add_log(&mut self, e: Entry) {
        if cfg!(debug_assertions) {
            eprintln!(
                "[{}] {:<5} {}{}",
                e.time,
                e.level.tag(),
                if e.proxy_name.is_empty() { String::new() } else { format!("[{}] ", e.proxy_name) },
                e.message
            );
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
        let domains: Vec<&str> =
            g["allowed_domains"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        json!({
            "values": {
                "allowed_domains": domains.join(", "), "block_private": g["block_private"], "timeout": g["timeout"],
                "log_connections": g["log_connections"], "log_close": g["log_close"],
                "dns_server": config::dns_list_text(g["dns_server"].as_str().unwrap_or(""), &g["dns_extra"]),
                "dns_via_outbound": g["dns_via_outbound"], "log_to_file": g["log_to_file"],
                "language": index(&codes, &g["language"]), "theme": index(&config::THEMES, &g["theme"]), "ui_scale": scale,
                "theme_color": g["theme_color"],
                "log_font_size": g["log_font_size"], "close_exits": g["close_exits"], "singbox_latest": g["singbox_latest"],
                "run_as_admin": g["run_as_admin"], "check_updates": g["check_updates"],
                "text_size": config::TEXT_SIZES.iter().position(|x| Some(*x) == g["text_size"].as_i64()).unwrap_or(1),
                "ui_font": g["ui_font"], "mono_font": g["mono_font"],
            },
            "languages": proxydock_core::i18n::LANGUAGES.iter().map(|(_, n)| *n).collect::<Vec<_>>(),
            "themes": [tr("System"), tr("Light"), tr("Dark")],
            "text_sizes": config::TEXT_SIZES.iter().map(|s| format!("{s}%") + &if *s == 115 { tr(" (default)") } else { String::new() }).collect::<Vec<_>>(),
            "scales": config::UI_SCALES.iter().map(|s| format!("{s}%") + &if *s == 100 { tr(" (default)") } else { String::new() }).collect::<Vec<_>>(),
            "engine": self.engine_text,
            "windows": proxydock_core::platform::supported(),
            "startup_supported": proxydock_core::platform::startup_supported(),
            "startup": {"registered": proxydock_core::platform::startup_state().0, "hidden": proxydock_core::platform::startup_state().1},
            "firewall": self.firewall,
            "log_dir": self.log_dir().display().to_string(),
            "bypass": self.bypass_json(),
            "app_on": self.app_on,
        })
    }

    /// Settings > Direct line: the VMs the user made, the choices and what is going on
    fn bypass_json(&self) -> Value {
        use proxydock_core::bypass;
        let store = bypass::Store::load();
        // (the hub is busy while a VM starts: the counts are left out then)
        let hub = bypass::hub::hub().try_lock().ok();
        let items: Vec<Value> = store
            .instances
            .iter()
            .map(|i| {
                json!({
                    "name": i.name, "label": bypass::label(&i.name), "adapter": i.adapter,
                    "memory_mb": i.memory_mb, "ip_mode": i.ip_mode, "address": i.address, "gateway": i.gateway,
                    "dns": i.dns, "ready": i.consented(), "enabled": i.enabled,
                    "in_use": hub.as_ref().map_or(0, |h| h.in_use(&i.name)),
                    "running": hub.as_ref().is_some_and(|h| h.running(&i.name)),
                })
            })
            .collect();
        json!({
            "available": self.bypass_reason.is_none(),
            "reason": self.bypass_reason.clone().unwrap_or_default(),
            "adapters": self.bypass_adapters,
            "memories": BYPASS_MEMORY,
            "keep_running": store.keep_running,
            "working": self.bypass_working,
            "progress": self.bypass_progress,
            "items": items,
        })
    }

    /// whether Hyper-V is there and the physical network cards, asked in the background
    fn refresh_bypass(&mut self) {
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let reason = proxydock_core::bypass::availability().err();
            let adapters = proxydock_core::bypass::physical_adapters();
            let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| {
                let first = !c.bypass_checked;
                c.bypass_checked = true;
                let ok = reason.is_none();
                c.bypass_reason = reason;
                c.bypass_adapters = adapters;
                c.send_settings();
                if first && ok {
                    c.start_kept_vms();
                }
            })));
        });
    }

    /// At startup, with "Keep the VMs running" on: the VMs that are switched on start (they would otherwise wait
    /// for a card; off, no administrator prompt comes at startup)
    fn start_kept_vms(&mut self) {
        use proxydock_core::bypass;
        let store = bypass::Store::load();
        if !store.keep_running {
            return;
        }
        let names: Vec<String> =
            store.instances.iter().filter(|i| i.enabled && i.consented()).map(|i| i.name.clone()).collect();
        if names.is_empty() {
            return;
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            for n in names {
                let r = bypass::hub::hub().lock().unwrap().hold(&n);
                let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| {
                    if let Err(e) = r {
                        c.app_log(Level::Error, &format!("{}: {e}", bypass::label(&n)));
                    }
                    c.send_settings();
                })));
            }
        });
    }

    /// Settings > VPN bypass actions
    fn bypass_command(&mut self, v: &Value) {
        use proxydock_core::bypass::{self, Consent, Instance, Store};
        let name = v["name"].as_str().unwrap_or("").to_string();
        if !name.is_empty() && !bypass::valid_name(&name) {
            self.broadcast(&json!({"ev": "error", "message": tr("A name: up to 24 letters, digits, spaces, - or _")}));
            return;
        }
        let mut store = Store::load();
        let first_adapter = self.bypass_adapters.first().cloned().unwrap_or_default();
        let mut inst = store.get(&name).cloned().unwrap_or_else(|| Instance::new(&name, &first_adapter));
        let cmd = v["cmd"].as_str().unwrap_or("");
        let error = |c: &mut Controller, m: String| c.broadcast(&json!({"ev": "error", "message": m}));
        match cmd {
            "bypass_refresh" => {
                self.refresh_bypass();
                return;
            }
            // the VM's own switch: on = started now and usable; off = its cards stop, then the VM
            "bypass_enable" => {
                let on = v["on"] == json!(true);
                if self.bypass_working || store.get(&name).is_none() {
                    return;
                }
                inst.enabled = on;
                store.put(inst.clone());
                if let Err(e) = store.save() {
                    return error(self, format!("{}: {e}", tr("Save failed")));
                }
                let label = bypass::label(&name);
                let users: Vec<String> = self
                    .proxies
                    .iter()
                    .filter(|p| bypass::instances_in(p).contains(&name))
                    .filter_map(|p| p["id"].as_str().map(String::from))
                    .filter(|id| matches!(self.manager.state(id), State::Running | State::Starting))
                    .collect();
                if !on {
                    for id in &users {
                        self.restart_at.remove(id);
                    }
                }
                self.bypass_working = true;
                self.bypass_progress =
                    tr(if on { "Starting the VM \"{name}\"..." } else { "Stopping the VM \"{name}\"..." })
                        .replace("{name}", &label);
                self.send_settings();
                let remote = self.manager.remote();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let r = if on {
                        bypass::hub::hub().lock().unwrap().hold(&name)
                    } else {
                        for id in &users {
                            let _ = remote.stop(id).recv();
                        }
                        bypass::hub::hub().lock().unwrap().force_stop(&name);
                        Ok(())
                    };
                    let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| {
                        c.bypass_working = false;
                        c.bypass_progress = String::new();
                        match r {
                            Ok(()) => {
                                let m = tr(if on {
                                    "Direct line VM \"{name}\" switched on"
                                } else {
                                    "Direct line VM \"{name}\" switched off"
                                })
                                .replace("{name}", &label);
                                c.app_log(Level::Setting, &m);
                                if !on && !users.is_empty() {
                                    let m =
                                        tr("Stopped {n} cards that use it").replace("{n}", &users.len().to_string());
                                    c.app_log(Level::Setting, &m);
                                }
                            }
                            Err(e) => {
                                c.app_log(Level::Error, &format!("{label}: {e}"));
                                c.broadcast(&json!({"ev": "error", "message": e}));
                            }
                        }
                        c.send_settings();
                    })));
                });
                return;
            }
            "bypass_keep" => {
                store.keep_running = v["on"] == json!(true);
            }
            "bypass_add" => {
                if name.is_empty() {
                    return error(self, tr("A name: up to 24 letters, digits, spaces, - or _"));
                }
                if store.get(&name).is_some() {
                    return error(self, tr("That name is already used"));
                }
                store.put(inst);
            }
            "bypass_set" => {
                if self.bypass_working || bypass::hub::hub().try_lock().map_or(true, |h| h.in_use(&name) > 0) {
                    return error(self, tr("Stop the cards that use this direct line first"));
                }
                if let Some(a) = v["adapter"].as_str() {
                    inst.adapter = a.to_string();
                }
                if let Some(m) = v["memory_mb"].as_u64() {
                    inst.memory_mb = bypass::clamp_memory(m as u32);
                }
                if let Some(mode) = v["ip_mode"].as_str() {
                    let text = |k: &str| v[k].as_str().unwrap_or("").trim().to_string();
                    if mode == "static" {
                        let probe = json!({"op": "net", "mode": "static", "address": text("address"),
                            "gateway": text("gateway"), "dns": text("dns").split([',', ' ']).filter(|d| !d.is_empty()).collect::<Vec<_>>()});
                        if let Err(e) = check_static(&probe) {
                            return error(self, e);
                        }
                        inst.address = text("address");
                        inst.gateway = text("gateway");
                        inst.dns = text("dns");
                    }
                    inst.ip_mode = if mode == "static" { "static".into() } else { "dhcp".into() };
                }
                store.put(inst);
            }
            "bypass_prepare" => {
                if self.bypass_working {
                    return;
                }
                if store.get(&name).is_none() {
                    return error(self, tr("Add the VM first"));
                }
                if v["consent"] == json!(true) {
                    inst.consent = Some(Consent { time: unix_now(), adapter: inst.adapter.clone() });
                    let msg = tr("Direct line: consent given for \"{name}\" on {adapter}")
                        .replace("{name}", &bypass::label(&name))
                        .replace("{adapter}", &inst.adapter);
                    self.app_log(Level::Setting, &msg);
                }
                if !inst.consented() {
                    // the window asks first (the warning with its check box)
                    self.broadcast(&json!({"ev": "bypass_consent", "name": name, "adapter": inst.adapter,
                        "memory_mb": inst.memory_mb}));
                    return;
                }
                store.put(inst.clone());
                if let Err(e) = store.save() {
                    return error(self, format!("{}: {e}", tr("Save failed")));
                }
                self.bypass_working = true;
                self.bypass_progress = tr("Preparing...");
                self.send_settings();
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let ptx = tx.clone();
                    let progress = move |m: &str| {
                        let m = m.to_string();
                        let _ = ptx.send(Msg::Work(Box::new(move |c: &mut Controller| {
                            c.bypass_progress = m;
                            c.send_settings();
                        })));
                    };
                    let r = bypass::hub::hub().lock().unwrap().prepare(&inst, &progress);
                    let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| {
                        c.bypass_working = false;
                        c.bypass_progress = String::new();
                        let label = bypass::label(&inst.name);
                        match r {
                            Ok(_) => c.app_log(
                                Level::Setting,
                                &tr("Direct line \"{name}\" is ready").replace("{name}", &label),
                            ),
                            Err(e) => {
                                let m = tr("Direct line \"{name}\" could not be prepared: {e}")
                                    .replace("{name}", &label)
                                    .replace("{e}", &e);
                                c.app_log(Level::Error, &m);
                                c.broadcast(&json!({"ev": "error", "message": m}));
                            }
                        }
                        c.send_settings();
                    })));
                });
                return;
            }
            "bypass_remove" => {
                if self.bypass_working {
                    return;
                }
                let had_vm = inst.consent.is_some();
                store.remove(&name);
                if let Err(e) = store.save() {
                    return error(self, format!("{}: {e}", tr("Save failed")));
                }
                if had_vm {
                    self.bypass_working = true;
                    self.bypass_progress = tr("Removing the VM...");
                    let tx = self.tx.clone();
                    std::thread::spawn(move || {
                        let r = bypass::hub::hub().lock().unwrap().remove(&name);
                        let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| {
                            c.bypass_working = false;
                            c.bypass_progress = String::new();
                            if let Err(e) = r {
                                c.app_log(Level::Error, &format!("{}: {e}", tr("Direct line")));
                            }
                            c.send_settings();
                        })));
                    });
                }
                self.send_settings();
                return;
            }
            _ => return,
        }
        if let Err(e) = store.save() {
            return error(self, format!("{}: {e}", tr("Save failed")));
        }
        self.send_settings();
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
        // the main DNS server and the extra ones that race it
        let Ok((dns_main, dns_extra)) = config::parse_dns_list(&text("dns_server")) else {
            let msg = json!({"ev": "error", "message": tr("DNS server: use an address like 1.1.1.1, tls://dns.google or https://cloudflare-dns.com/dns-query")});
            self.broadcast(&msg);
            return;
        };
        if !self.can_save {
            self.broadcast(&json!({"ev": "error", "message": tr("Save failed")}));
            return;
        }
        let pick = |list: &[&str], k: &str| {
            list.get(vals[k].as_u64().unwrap_or(0) as usize).copied().unwrap_or(list[0]).to_string()
        };
        let codes: Vec<&str> = proxydock_core::i18n::LANGUAGES.iter().map(|(c, _)| *c).collect();
        let mut g = self.glob.clone();
        for k in [
            "block_private",
            "log_connections",
            "log_close",
            "dns_via_outbound",
            "log_to_file",
            "close_exits",
            "singbox_latest",
            "run_as_admin",
            "check_updates",
        ] {
            g.insert(k.into(), json!(vals[k] == json!(true)));
        }
        for k in ["timeout", "log_font_size"] {
            if let Some(n) = vals[k].as_i64() {
                g.insert(k.into(), json!(n));
            }
        }
        g.insert("allowed_domains".into(), json!(domains));
        g.insert("dns_server".into(), json!(dns_main));
        g.insert("dns_extra".into(), json!(dns_extra));
        g.insert("language".into(), json!(pick(&codes, "language")));
        g.insert("theme".into(), json!(pick(&config::THEMES, "theme")));
        g.insert("theme_color".into(), json!(vals["theme_color"].as_str().unwrap_or("")));
        let size = config::TEXT_SIZES.get(vals["text_size"].as_u64().unwrap_or(1) as usize).copied().unwrap_or(115);
        g.insert("text_size".into(), json!(size));
        for k in ["ui_font", "mono_font"] {
            g.insert(k.into(), json!(proxydock_core::py::strip(vals[k].as_str().unwrap_or(""))));
        }
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
        if !pending.is_empty() {
            // (Python: a warning bar with "Restart now")
            let text = format!(
                "{}\n{}",
                tr("Restart required"),
                tr("Applies after a restart: {join}").replace("{join}", &pending.join(", "))
            );
            self.broadcast(&json!({"ev": "restart_needed", "message": text}));
        }
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
            self.send_conn_sites();
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
                        let msg = tr("Downloaded sing-box {version}; each proxy uses it from its next start")
                            .replace("{version}", &v.to_string());
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
        let mut ids: Vec<String> = keys
            .iter()
            .map(|k| k.strip_suffix("/out").unwrap_or(k).to_string())
            .filter(|k| targets.contains(k))
            .collect();
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
            .map(|id| {
                (
                    id.clone(),
                    proxydock_core::probe::outbound_proxy(&self.graph, id),
                    proxydock_core::probe::latency_key(&self.graph, id),
                )
            })
            .collect();
        self.latency_busy.extend(ids.iter().cloned());
        self.latency_round = Some((ids.len(), 0, 0));
        self.send_latency();
        let queue = Arc::new(Mutex::new(jobs));
        for _ in 0..LATENCY_WORKERS.min(ids.len()) {
            let (queue, tx, remote, glob) = (queue.clone(), self.tx.clone(), self.manager.remote(), self.glob.clone());
            std::thread::spawn(move || {
                loop {
                    let Some((id, p, key)) = queue.lock().unwrap().pop() else { break };
                    let r = proxydock_core::probe::check_outbound(&remote, &p, &glob, &format!("__latency_{id}__"));
                    let ok = r.get("ok") == Some(&json!(true));
                    let ms =
                        if ok { r.get("total_ms").and_then(Value::as_f64).map(|m| m.round() as i64) } else { None };
                    let pick = |k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    let error = if !pick("detail").is_empty() { pick("detail") } else { pick("error") };
                    let _ =
                        tx.send(Msg::Work(Box::new(move |c: &mut Controller| c.on_latency(id, (ok, ms, error, key)))));
                }
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
            let text = tr("{ok} working, {failed} not responding")
                .replace("{ok}", &oks.to_string())
                .replace("{failed}", &(total - oks).to_string());
            // (Python: a success bar, a warning one when some did not respond)
            let ev = if oks == total { "info" } else { "warn" };
            let msg = json!({"ev": ev, "message": format!("{}
{text}", tr("Latency test finished"))});
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
                    Some((ok, ms, _, key)) if *key == proxydock_core::probe::latency_key(&self.graph, id) => {
                        Some(match (ok, ms) {
                            (true, Some(ms)) => (
                                format!("{ms} ms"),
                                if *ms < 400 {
                                    1
                                } else if *ms < 1000 {
                                    2
                                } else {
                                    3
                                },
                            ),
                            _ => ("✕".to_string(), 3),
                        })
                    }
                    _ => None,
                }
            };
            if let Some((text, tone)) = badge {
                badges.insert(id.into(), json!([text, tone]));
            }
        }
        let progress = match self.latency_round {
            Some((total, done, _)) => tr("Testing… {done}/{total}")
                .replace("{done}", &done.to_string())
                .replace("{total}", &total.to_string()),
            None => String::new(),
        };
        json!({"badges": badges, "progress": progress})
    }

    fn send_latency(&mut self) {
        let msg = json!({"ev": "latency", "latency": self.latency_json()});
        self.broadcast(&msg);
    }

    /// The PC block's state (Python `pc_status`): (off | waiting | applied | error | unsupported, text)
    fn pc_status(&self) -> (&'static str, String) {
        let pc = &self.graph["pc"];
        if !proxydock_core::platform::system_proxy_supported() {
            return ("unsupported", tr("This desktop has no system proxy setting ProxyDock can change"));
        }
        let Some(p) = pc["inbound"].as_str().and_then(|id| self.proxy(id)) else {
            return ("off", tr("Not connected - the PC uses its own settings"));
        };
        if let Err(e) = proxydock_core::platform::system_proxy_server(&p["inbound"]) {
            return ("error", e);
        }
        if pc["applied"] == json!(true) {
            let text = tr("In use: {name} ({server})")
                .replace("{name}", &name_of(p))
                .replace("{server}", pc["server"].as_str().unwrap_or(""));
            return ("applied", text);
        }
        ("waiting", tr("Applied while '{name}' is running").replace("{name}", &name_of(p)))
    }

    /// The system proxy follows the PC block (Python `_sync_pc`): set while its card runs, put back otherwise.
    /// True when it changed (the caller saves).
    fn sync_pc(&mut self) -> bool {
        use proxydock_core::platform;
        if !platform::system_proxy_supported() {
            return false;
        }
        let card = self.graph["pc"]["inbound"].as_str().and_then(|id| self.proxy(id)).cloned();
        let server = card
            .filter(|p| self.manager.state(p["id"].as_str().unwrap_or("")) == State::Running)
            .and_then(|p| platform::system_proxy_server(&p["inbound"]).ok());
        let applied = self.graph["pc"]["applied"] == json!(true);
        let result = match server {
            Some(server) => {
                if applied && self.graph["pc"]["server"] == json!(server) {
                    return false;
                }
                let backup = if applied {
                    Ok(self.graph["pc"]["backup"].clone())
                } else {
                    platform::get_system_proxy().map(Value::Object)
                };
                backup.and_then(|b| {
                    platform::set_system_proxy(&platform::proxy_values_for(&server, &b))?;
                    let pc = self.graph.get_mut("pc").unwrap();
                    pc["backup"] = b;
                    pc["applied"] = json!(true);
                    pc["server"] = json!(server);
                    Ok(tr("System proxy set to {server}").replace("{server}", &server))
                })
            }
            None if applied => {
                self.pc_restore_backup();
                Ok(tr("System proxy restored to the previous settings"))
            }
            None => return false,
        };
        match result {
            Ok(msg) => {
                self.app_log(Level::Setting, &msg);
                true
            }
            Err(e) => {
                self.app_log(Level::Error, &tr("Could not change the system proxy: {e}").replace("{e}", &e));
                false
            }
        }
    }

    /// the PC's settings from before ProxyDock changed them (Python `_pc_restore_backup`)
    fn pc_restore_backup(&mut self) {
        let backup = self.graph["pc"]["backup"].clone();
        if let Value::Object(b) = &backup
            && let Err(e) = proxydock_core::platform::set_system_proxy(b)
        {
            self.app_log(Level::Error, &tr("Could not change the system proxy: {e}").replace("{e}", &e));
        }
        let pc = self.graph.get_mut("pc").unwrap();
        pc["applied"] = json!(false);
        pc["server"] = json!("");
    }

    /// start at login, the PC block's off / restore, the firewall rule (from the window or the tray)
    fn platform_command(&mut self, v: &Value) {
        use proxydock_core::platform;
        let on = v["on"] == json!(true);
        match v["cmd"].as_str().unwrap_or("") {
            // Python `set_startup` / `set_hidden`
            "startup" | "startup_hidden" => {
                let (registered, hidden) = platform::startup_state();
                let startup = v["cmd"] == json!("startup");
                if !startup && !registered {
                    self.send_settings();
                    return;
                }
                let exe = std::env::current_exe().unwrap_or_default();
                let (reg, hid) = if startup { (on, hidden) } else { (true, on) };
                match platform::set_startup(&exe, reg, hid) {
                    Ok(()) => {
                        let msg = if startup {
                            tr(if on { "Start at login" } else { "Removed from startup" })
                        } else {
                            tr("Hide startup notification {value}").replace("{value}", if on { "ON" } else { "OFF" })
                        };
                        self.app_log(Level::Setting, &msg);
                    }
                    Err(e) => self.app_log(Level::Error, &tr("Startup registration error: {e}").replace("{e}", &e)),
                }
                self.send_settings();
            }
            // Python `pc_off`: the PC block disconnected and the system proxy turned off
            "pc_off" | "pc_restore" => {
                let off = v["cmd"] == json!("pc_off");
                let mut values = Map::new();
                values.insert("ProxyEnable".into(), json!(0));
                let backup = self.graph["pc"]["backup"].clone();
                let r = if off {
                    platform::set_system_proxy(&values).map(|()| tr("System proxy turned off"))
                } else if let Value::Object(b) = &backup {
                    platform::set_system_proxy(b).map(|()| tr("System proxy restored to the previous settings"))
                } else {
                    Ok(String::new())
                };
                match r {
                    Ok(msg) if !msg.is_empty() => self.app_log(Level::Setting, &msg),
                    Ok(_) => {}
                    Err(e) => {
                        self.app_log(Level::Error, &tr("Could not change the system proxy: {e}").replace("{e}", &e))
                    }
                }
                let pc = self.graph.get_mut("pc").unwrap();
                pc["inbound"] = Value::Null;
                pc["applied"] = json!(false);
                pc["server"] = json!("");
                pc["backup"] = Value::Null;
                self.graph_changed();
            }
            // Python `set_switch`: an IF block in switch mode, everything to "then" (on) or "else" (off)
            "set_switch" => {
                let id = v["id"].as_str().unwrap_or("").to_string();
                let Some(r) = self
                    .graph
                    .get_mut("routers")
                    .and_then(Value::as_array_mut)
                    .and_then(|a| a.iter_mut().find(|r| r["id"] == json!(id)))
                else {
                    return;
                };
                if r["switch"] != json!(true) || r["switch_on"] == json!(on) {
                    return;
                }
                r["switch_on"] = json!(on);
                let msg = tr("Switch '{name}' turned {state}")
                    .replace("{name}", r["name"].as_str().unwrap_or(""))
                    .replace("{state}", &tr(if on { "on" } else { "off" }));
                self.app_log(Level::Setting, &msg);
                self.graph_changed();
            }
            // Python `set_flag`: a card's options that need no restart (its block's chips)
            "set_flag" => {
                let id = v["id"].as_str().unwrap_or("").to_string();
                let key = v["key"].as_str().unwrap_or("");
                if !matches!(key, "autostart" | "auto_restart") {
                    return;
                }
                let Some(n) = self
                    .graph
                    .get_mut("inbounds")
                    .and_then(Value::as_array_mut)
                    .and_then(|a| a.iter_mut().find(|n| n["id"] == json!(id)))
                else {
                    return;
                };
                if n[key] == json!(on) {
                    return;
                }
                n[key] = json!(on);
                if key == "auto_restart" && !on {
                    self.restart_at.remove(&id);
                }
                self.graph_changed();
            }
            // Python `select_member`: a hand-picked group uses another member (its cards restart with it)
            "select_member" => {
                let (gid, mid) = (v["group"].as_str().unwrap_or("").to_string(), v["member"].clone());
                let member = graph::find(&self.graph, mid.as_str().unwrap_or(""))
                    .map(|(_, m)| name_of(m))
                    .unwrap_or_else(|| "?".into());
                let Some(g) = self
                    .graph
                    .get_mut("outbounds")
                    .and_then(Value::as_array_mut)
                    .and_then(|a| a.iter_mut().find(|n| n["id"] == json!(gid)))
                else {
                    return;
                };
                let is_member = g["members"].as_array().is_some_and(|m| m.contains(&mid));
                if !is_member || g["outbound"]["group_selected"] == mid {
                    return;
                }
                g["outbound"]["group_selected"] = mid;
                let msg = tr("Group '{group}' now uses '{name}'")
                    .replace("{group}", g["name"].as_str().unwrap_or(""))
                    .replace("{name}", &member);
                self.app_log(Level::Setting, &msg);
                self.graph_changed();
            }
            // ProxyDock's own switch, from the tray menu
            "home_power" => self.home_power(on),
            // Python `firewall`: the rule changes through UAC, in the background
            "firewall" => {
                if self.fw_busy || !platform::supported() {
                    return;
                }
                self.fw_busy = true;
                self.firewall = None;
                self.send_settings();
                let mut programs: Vec<std::path::PathBuf> = std::env::current_exe().into_iter().collect();
                programs.extend(proxydock_core::singbox::find_binary(&self.glob));
                programs.extend(proxydock_core::singbox::find_hysteria(&self.glob));
                let tx = self.tx.clone();
                std::thread::spawn(move || {
                    let r = platform::firewall_apply(on, &programs);
                    let exists = platform::firewall_rule_exists();
                    let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| {
                        c.fw_busy = false;
                        c.firewall = Some(exists);
                        match r {
                            Ok(msg) => c.app_log(Level::Setting, &tr("Firewall: {msg}").replace("{msg}", &msg)),
                            Err(e) => {
                                let msg =
                                    json!({"ev": "error", "message": format!("{}: {e}", tr("Firewall change failed"))});
                                c.broadcast(&msg);
                            }
                        }
                        c.send_settings();
                    })));
                });
            }
            _ => {}
        }
    }

    /// whether the firewall rule exists, checked in the background (Python `refresh_firewall`)
    fn refresh_firewall(&mut self) {
        if !proxydock_core::platform::supported() {
            return;
        }
        self.firewall = None;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let exists = proxydock_core::platform::firewall_rule_exists();
            let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| {
                c.firewall = Some(exists);
                c.send_settings();
            })));
        });
    }

    /// the sing-box and Hysteria programs and their versions, asked in the background (Python `probe_cores`)
    fn refresh_engines(&mut self) {
        let glob = self.glob.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut out = vec![];
            for (name, exe) in [
                ("sing-box", proxydock_core::singbox::find_binary(&glob)),
                ("Hysteria2", proxydock_core::singbox::find_hysteria(&glob)),
            ] {
                let version = exe.as_deref().and_then(proxydock_core::singbox::core_version);
                out.push(json!([name, exe.map(|e| e.display().to_string()), version]));
            }
            let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| {
                c.engines = Some(out);
                let msg = json!({"ev": "about", "about": c.about_json()});
                c.broadcast(&msg);
            })));
        });
    }

    /// the About page (Python `AboutInterface`)
    fn about_json(&self) -> Value {
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "app_dir": self.config_path.parent().map(|d| d.display().to_string()).unwrap_or_default(),
            "engines": self.engines,
        })
    }

    /// what the window shows about updates: the status, and the newer release with what installing it does
    fn update_json(&self) -> Value {
        use proxydock_core::update;
        let u = &self.update;
        let release = u.release.as_ref().map(|r| {
            let exe = std::env::current_exe().unwrap_or_default();
            let (os, arch) = update::platform_key();
            let asset = update::install_kind(&exe).and_then(|k| update::pick_asset(r, os, arch, k));
            let size = asset.as_ref().and_then(|a| r.assets.get(a)).map_or(0, |a| a.1);
            json!({"version": r.version, "notes": r.notes, "url": r.url, "asset": asset, "size": size})
        });
        json!({"status": u.status, "busy": u.checking || u.downloading, "release": release, "current": update::current_version()})
    }

    fn send_update(&mut self) {
        let msg = json!({"ev": "update", "update": self.update_json()});
        self.broadcast(&msg);
    }

    /// Look for a newer release (Python `Updater.check`); `manual` from the About page's button
    fn check_update(&mut self, manual: bool) {
        if self.update.checking {
            return;
        }
        self.update.checking = true;
        self.update.status = tr("Checking for updates…");
        self.send_update();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = proxydock_core::update::check();
            let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| c.on_update_checked(r, manual))));
        });
    }

    fn on_update_checked(&mut self, r: Result<Option<proxydock_core::update::Release>, String>, manual: bool) {
        self.update.checking = false;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        self.glob.insert("update_checked".into(), json!(now));
        self.save();
        match r {
            Err(e) => {
                self.update.status = tr("Update check failed");
                self.app_log(Level::Warn, &tr("Update check failed: {e}").replace("{e}", &e));
                if manual {
                    self.broadcast(&json!({"ev": "error", "message": format!("{}: {e}", tr("Update check failed"))}));
                }
            }
            Ok(None) => {
                self.update.release = None;
                self.update.status = tr("This is the latest version");
                if manual {
                    let text = tr("{APP_NAME} {version} is the latest version")
                        .replace("{APP_NAME}", "ProxyDock")
                        .replace("{version}", proxydock_core::update::current_version());
                    self.broadcast(&json!({"ev": "info", "message": text}));
                }
            }
            Ok(Some(rel)) => {
                let text = tr("Version {version} is available").replace("{version}", &rel.version);
                self.update.status = text.clone();
                self.update.release = Some(rel);
                self.app_log(Level::Info, &text);
                if !manual {
                    // the window shows it (with a way to the details); with no window, the tray
                    self.notify_user(&text);
                }
            }
        }
        // (the release first, then the dialog that shows it)
        self.send_update();
        if manual && self.update.release.is_some() {
            self.broadcast(&json!({"ev": "update_dialog"}));
        }
    }

    /// Download the release file, check it, then the window asks before installing (Python `start_download`)
    fn download_update(&mut self) {
        use proxydock_core::update;
        let Some(rel) = self.update.release.clone() else { return };
        let exe = std::env::current_exe().unwrap_or_default();
        let (os, arch) = update::platform_key();
        let Some(kind) = update::install_kind(&exe) else { return };
        let Some(name) = update::pick_asset(&rel, os, arch, kind) else { return };
        if self.update.downloading {
            return;
        }
        self.update.downloading = true;
        self.update.cancel.store(false, std::sync::atomic::Ordering::SeqCst);
        self.app_log(Level::Info, &tr("Downloading {name}").replace("{name}", &name));
        self.send_update();
        let (tx, cancel) = (self.tx.clone(), self.update.cancel.clone());
        std::thread::spawn(move || {
            let folder = std::env::temp_dir().join(format!("proxydock-update-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&folder);
            let tx2 = tx.clone();
            let progress = move |done: u64, total: u64| {
                let _ = tx2.send(Msg::Work(Box::new(move |c: &mut Controller| {
                    let msg = json!({"ev": "update_progress", "done": done, "total": total});
                    c.broadcast(&msg);
                })));
            };
            let r = update::fetch_verified(&rel, &name, &folder, &progress, &|| {
                cancel.load(std::sync::atomic::Ordering::SeqCst)
            });
            let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| c.on_update_downloaded(r))));
        });
    }

    fn on_update_downloaded(&mut self, r: Result<std::path::PathBuf, proxydock_core::update::FetchError>) {
        use proxydock_core::update::FetchError;
        self.update.downloading = false;
        self.send_update();
        match r {
            Ok(path) => {
                self.app_log(
                    Level::Info,
                    &tr("Downloaded and verified (SHA-256): {path}").replace("{path}", &path.display().to_string()),
                );
                let msg = json!({"ev": "update_ready", "path": path.display().to_string()});
                self.broadcast(&msg);
            }
            Err(FetchError::Cancelled) => {
                self.app_log(Level::Info, &tr("Update download cancelled"));
                self.broadcast(&json!({"ev": "update_ready", "path": ""}));
            }
            Err(FetchError::Failed(e)) => {
                self.app_log(Level::Error, &tr("Update download failed: {e}").replace("{e}", &e));
                self.broadcast(&json!({"ev": "update_ready", "path": ""}));
                self.broadcast(&json!({"ev": "error", "message": format!("{}: {e}", tr("Update download failed"))}));
            }
        }
    }

    /// Install the downloaded update (Python `install`): Windows runs the installer and quits (it closes this
    /// program, updates it and starts it again); macOS opens the disk image (the user drags the app over the old
    /// one); Linux puts the new AppImage in place of this one and restarts
    fn install_update(&mut self, path: &str) {
        let setup = std::path::Path::new(path);
        let app_dir =
            std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())).unwrap_or_default();
        let kind = proxydock_core::update::install_kind(&std::env::current_exe().unwrap_or_default());
        if !setup.is_file() || kind.is_none() {
            return;
        }
        let fail = |c: &mut Self, e: String| {
            c.app_log(Level::Error, &tr("Update failed: {e}").replace("{e}", &e));
            c.broadcast(&json!({"ev": "error", "message": format!("{}: {e}", tr("Update failed"))}));
        };
        match kind {
            Some("dmg") => {
                if let Err(e) = std::process::Command::new("open").arg(setup).spawn() {
                    fail(self, e.to_string());
                }
                return;
            }
            Some("appimage") => {
                let target = std::env::var_os("APPIMAGE").map(std::path::PathBuf::from).unwrap_or_default();
                match proxydock_core::update::replace_appimage(setup, &target) {
                    Ok(()) => (self.hooks.restart)(true),
                    Err(e) => fail(self, e.to_string()),
                }
                return;
            }
            _ => {}
        }
        let cmd = proxydock_core::update::installer_command(setup, &app_dir, &proxydock_core::i18n::current());
        self.app_log(Level::Info, &tr("Starting the installer: {path}").replace("{path}", path));
        let mut c = std::process::Command::new(&cmd[0]);
        c.args(&cmd[1..]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0000_0008 | 0x0000_0200); // detached, its own group
        }
        match c.spawn() {
            Ok(_) => (self.hooks.quit)(),
            Err(e) => fail(self, e.to_string()),
        }
    }

    fn send_settings(&mut self) {
        let msg = json!({"ev": "settings", "settings": self.settings_json()});
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
                // no settings yet: the previous version's (SOCKS5 Proxy), if it was used (Python `migrate_from_old`)
                let old = proxydock_core::paths::old_app_dir();
                if let Some((g, graph_old, n)) = old.as_deref().and_then(config::migrate_from_old) {
                    self.glob = g;
                    proxydock_core::i18n::set_language(self.glob["language"].as_str().unwrap_or("en"));
                    self.graph = graph::validate(&Value::Object(graph_old));
                    let msg = tr("Imported settings from the previous version ({old_dir}) - {len} proxies")
                        .replace("{old_dir}", &old.unwrap_or_default().display().to_string())
                        .replace("{len}", &n.to_string());
                    self.app_log(Level::Info, &msg);
                }
                self.save();
            }
            Err(e) => {
                // (Python `config.load`): the unreadable file is kept next to it, the app starts with defaults
                let backup = std::path::PathBuf::from(format!("{}.broken", self.config_path.display()));
                let e = match &e {
                    config::LoadError::Unreadable(m) => m.clone(),
                    other => format!("{other:?}"),
                };
                let msg = match std::fs::copy(&self.config_path, &backup) {
                    Ok(_) => tr("Could not read the settings file, starting with defaults ({e}). The original was kept as {backup}")
                        .replace("{e}", &e)
                        .replace("{backup}", &backup.display().to_string()),
                    Err(_) => tr("Could not read the settings file, starting with defaults: {e}").replace("{e}", &e),
                };
                self.app_log(Level::Warn, &msg);
                self.glob = config::validate_global(&json!({}));
                self.graph = graph::validate(&json!({}));
                self.can_save = true;
            }
        }
        if self.graph["pc"]["applied"] == json!(true) {
            // left applied by a crash: the PC's own settings come back first (Python `_pc_restore_backup`)
            self.pc_restore_backup();
        }
        self.rebuild();
        self.undo_base = self.snapshot();
        *self.shared_glob.lock().unwrap() = self.glob.clone();
        self.refresh_firewall();
        self.refresh_bypass();
        self.refresh_engines();
        self.engine_text = engine_text(&self.glob);
        if self.glob["singbox_latest"] == json!(true) {
            self.update_latest_singbox();
        }
        // (before the cards start: off, they start refusing)
        self.load_app_on();
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
        let unlinked: std::collections::HashSet<String> = self
            .proxies
            .iter()
            .filter(|p| p["unlinked"] == json!(true))
            .map(|p| p["id"].as_str().unwrap_or("").to_string())
            .collect();
        self.view = canvas::build(&self.graph, &|id| summaries.get(id).cloned().unwrap_or_default(), &|id| {
            unlinked.contains(id)
        });
        let (_, pc_text) = self.pc_status();
        if let Some(n) = self.view.nodes.iter_mut().find(|n| n.key == "pc") {
            n.detail = pc_text;
        }
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
        edit::drop_empty_groups(&mut self.graph);
        graph::clean_groups(&mut self.graph);
        graph::clean_fills(&mut self.graph);
        graph::clean_relay_ui(&mut self.graph);
        self.rebuild();
        let mut alive = std::collections::HashSet::new();
        for p in self.proxies.clone() {
            let id = p["id"].as_str().unwrap_or("").to_string();
            alive.insert(id.clone());
            if !matches!(self.manager.state(&id), State::Running | State::Starting) {
                continue;
            }
            let same = old.get(&id).is_some_and(|b| {
                ["name", "inbound", "outbound", "overrides", "routes"].iter().all(|k| b.get(*k) == p.get(*k))
            });
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
        self.sync_pc();
        self.save();
        self.send_canvas();
        self.send_latency();
    }

    /// the canvas for the window, each block with its right-click menu
    fn canvas_json(&self) -> Value {
        let mut canvas = canvas::to_json(&self.view, &|id| state_name(&self.manager.state(id)));
        canvas["groups"] = self.graph.get("groups").cloned().unwrap_or(json!([]));
        canvas["fills"] = self.graph.get("fills").cloned().unwrap_or(json!({}));
        // the list page's PC bar (Python `PcBar`): its state and text
        let (state, text) = self.pc_status();
        canvas["pc_status"] = json!([state, text]);
        for n in canvas["nodes"].as_array_mut().into_iter().flatten() {
            let key = n["key"].as_str().unwrap_or("").to_string();
            n["menu"] = edit::block_menu(&self.graph, &key);
            // a card's options (its block's chips)
            if let Some((_, b)) = graph::find(&self.graph, &key).filter(|(k, _)| *k == "inbound") {
                n["autostart"] = json!(b["autostart"] == json!(true));
                n["auto_restart"] = json!(b["auto_restart"] == json!(true));
            }
            // an IF in switch mode: its switch (the list page shows it on the block's card)
            if let Some((_, r)) = graph::find(&self.graph, &key)
                .filter(|(k, r)| *k == "router" && r["style"] == json!("if") && r["switch"] == json!(true))
            {
                n["switch"] = json!(r["switch_on"] == json!(true));
            }
        }
        canvas
    }

    /// what the canvas's "add" menus offer: the inbound and outbound types with their names
    fn add_menu_json(&self) -> Value {
        let ib: Vec<Value> =
            proxydock_core::config::INBOUND_TYPES.iter().map(|t| json!([t, edit::inbound_label(t)])).collect();
        let ob: Vec<Value> =
            proxydock_core::config::OUTBOUND_TYPES.iter().map(|t| json!([t, graph::outbound_label(t)])).collect();
        let templates: Vec<Value> = edit::templates().into_iter().map(|(n, d, _, _)| json!([n, d])).collect();
        json!({"inbounds": ib, "outbounds": ob, "templates": templates})
    }

    fn send_canvas(&mut self) {
        let cards: Vec<Value> = self.proxies.iter().map(|p| self.card_json(p)).collect();
        let canvas = self.canvas_json();
        self.broadcast(&json!({"ev": "graph", "cards": cards, "canvas": canvas, "stat_meta": self.stat_meta()}));
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
            if kind == "outbound" {
                form::refresh_ifaces();
            }
            self.drafts.insert(client, (kind, node));
            self.probe_text.remove(&client);
        }
        let Some((kind, draft)) = self.drafts.get(&client).cloned() else { return };
        let mut node = form::apply(kind, &draft, &values);
        let id = node["id"].as_str().unwrap_or("").to_string();
        // a card picked that has no default route (e.g. a Hyper-V / VMware switch): only its own network is reachable
        let iface = node["outbound"]["interface"].as_str().unwrap_or("").to_string();
        if kind == "outbound"
            && !iface.is_empty()
            && draft["outbound"]["interface"] != json!(iface)
            && form::iface_unrouted(&iface)
        {
            let text = tr("'{name}' has no gateway, so it only reaches its own network (e.g. VMs or a LAN). Connections to the internet through it will fail.").replace("{name}", &iface);
            reply(self, json!({"ev": "notice", "ok": false, "title": tr("No internet route"), "text": text}));
        }
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
            match form::act(&self.graph, kind, &node, action, &values) {
                Ok(done) => {
                    node = done.node;
                    if let Some((name, text)) = done.save {
                        reply(self, json!({"ev": "save_file", "name": name, "text": text}));
                    }
                    if let Some(key) = done.pick {
                        reply(self, json!({"ev": "pick_programs", "key": key}));
                    }
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
        self.graph = form::with_node(&self.graph, kind, &node);
        self.drafts.remove(&client);
        let name = node["name"].as_str().unwrap_or("").to_string();
        self.app_log(Level::Setting, &tr("Saved proxy '{name}'").replace("{name}", &name));
        self.graph_changed();
        if kind == "inbound" {
            let ib = &node["inbound"];
            let host = ib["host"].as_str().unwrap_or("");
            let local = host.eq_ignore_ascii_case("localhost") || proxydock_core::singbox::guard::is_loopback_ip(host);
            if !local
                && matches!(ib["type"].as_str(), Some("socks5" | "http" | "https"))
                && ib["auth_enabled"] != json!(true)
            {
                // (Python: a warning bar over the window as the editor closes; sent after the "saved" message so it stays)
                let text = format!(
                    "{}\n{}",
                    tr("Exposed without authentication"),
                    tr("Anyone on the same network can use this proxy. Turning on user authentication is recommended.")
                );
                reply(self, json!({"ev": "warn", "message": text}));
            }
        }
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
                if f.key == "probe"
                    && let Some((_, text, tone)) = self.probe_text.get(&client).filter(|(b, _, _)| *b == id)
                {
                    j["desc"] = json!(text);
                    j["tone"] = json!(tone);
                }
                j
            })
            .collect();
        let title =
            graph::find(&self.graph, &id).and_then(|(_, n)| n["name"].as_str().map(String::from)).unwrap_or_default();
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
                    self.notice(
                        client,
                        false,
                        &tr("Subscription"),
                        &tr("Enter the subscription address (https://...)"),
                    );
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
                if proxydock_core::py::strip(ob["host"].as_str().unwrap_or("")).is_empty()
                    || ob["port"].as_i64().unwrap_or(0) == 0
                {
                    self.notice(
                        client,
                        false,
                        &tr("Detect from server"),
                        &tr("Enter the server address and port first"),
                    );
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
                    self.notice(
                        client,
                        false,
                        &tr("Please fix first"),
                        &errs.iter().take(4).cloned().collect::<Vec<_>>().join("\n"),
                    );
                    return None;
                }
                let p = form::as_proxy(&self.graph, kind, node)?;
                let inbound = kind == "inbound";
                // the card runs on these settings already: its port is in use by itself
                let running = inbound
                    && self.manager.state(&id) == State::Running
                    && graph::find(&self.graph, &id).is_some_and(|(_, saved)| {
                        saved["inbound"]["host"] == node["inbound"]["host"]
                            && saved["inbound"]["port"] == node["inbound"]["port"]
                    });
                self.probe_text.insert(client, (id.clone(), tr("Connecting through the outbound\u{2026}"), 0));
                let (remote, glob, ot) = (
                    self.manager.remote(),
                    self.glob.clone(),
                    p["outbound"]["type"].as_str().unwrap_or("direct").to_string(),
                );
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
        let saved_same = graph::find(&self.graph, id).is_some_and(|(_, saved)| {
            saved["inbound"]["port"] == json!(port) && saved["inbound"]["host"] == json!(host)
        });
        if self.manager.state(id) == State::Running && saved_same {
            let title = tr("Port {port}").replace("{port}", &port.to_string());
            self.notice(client, true, &title, &tr("This proxy is running on this port."));
            return;
        }
        let (ok, msg) = proxydock_core::probe::check_port(&host, port, ib["type"] == json!("hysteria2"));
        let title =
            tr("Port {port} ").replace("{port}", &port.to_string()) + &tr(if ok { "available" } else { "unavailable" });
        self.notice(client, ok, &title, &msg);
    }

    /// change the window's draft if it still edits block `id`
    fn with_draft(&mut self, client: u64, id: &str, f: impl FnOnce(&mut Value)) {
        if let Some((_, n)) = self.drafts.get_mut(&client)
            && n["id"] == json!(id)
        {
            f(n);
        }
    }

    /// Refresh the subscription blocks whose interval has passed, in the background (Python
    /// `update_due_subscriptions`): changed servers apply to the cards, the same ones only move the update time.
    fn update_due_subscriptions(&mut self) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
        let due: Vec<(String, String)> = self
            .graph
            .get("outbounds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|n| n["outbound"]["type"] == json!("subscription"))
            .filter(|n| {
                let ob = &n["outbound"];
                let every = ob["sub_interval"].as_i64().unwrap_or(0);
                every > 0 && now - ob["sub_updated"].as_i64().unwrap_or(0) >= every * 3600
            })
            .map(|n| {
                (
                    n["id"].as_str().unwrap_or("").to_string(),
                    n["outbound"]["sub_url"].as_str().unwrap_or("").to_string(),
                )
            })
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
        let Some(name) = graph::find(&self.graph, id)
            .filter(|(k, _)| *k == "outbound")
            .map(|(_, n)| n["name"].as_str().unwrap_or("").to_string())
        else {
            return;
        };
        let (fields, note) = match result {
            Ok(x) => x,
            Err(e) => {
                let msg = tr("Subscription '{name}' could not be updated: {note}")
                    .replace("{name}", &name)
                    .replace("{note}", &e);
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
                    edit::move_block(
                        &mut self.graph,
                        key,
                        m["x"].as_f64().unwrap_or(0.0),
                        m["y"].as_f64().unwrap_or(0.0),
                    );
                }
                // positions only: saved (the window already shows them)
                self.rebuild();
                self.save();
                return;
            }
            "connect" => {
                let r = edit::connect(&mut self.graph, &s("from"), n("out"), &s("to"), n("in"));
                if r.is_ok()
                    && s("from") == "pc"
                    && let Some(Err(e)) = graph::find(&self.graph, &s("to"))
                        .map(|(_, b)| proxydock_core::platform::system_proxy_server(&b["inbound"]))
                {
                    self.broadcast(&json!({"ev": "error", "message": format!("{}: {e}", tr("PC (system proxy)"))}));
                }
                r
            }
            "disconnect" => edit::disconnect(&mut self.graph, &s("from"), n("out"), &s("to"), n("in")),
            "lane_map" => {
                let out = v["out"].as_u64().map(|x| x as usize);
                match edit::set_lane_map(&mut self.graph, &s("id"), n("in"), out) {
                    Ok(false) => return,
                    r => r.map(|_| ()),
                }
            }
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
            // a wireless pair's number (its key is "number:<id>")
            "rename" if s("key").starts_with("number:") => {
                match edit::set_wireless_number(&mut self.graph, &s("key")["number:".len()..], &s("name")) {
                    Ok(true) => Ok(()),
                    Ok(false) => return,
                    Err(e) => Err(format!("{}\n{e}", tr("Cannot change the number"))),
                }
            }
            // a pass-through lane's name (its key is "lane:<relay id>:<inputs|outputs>:<index>")
            "rename" if s("key").starts_with("lane:") => {
                let key = s("key");
                let mut it = key["lane:".len()..].splitn(3, ':');
                let (rid, side, i) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
                let side = if side == "outputs" { "outputs" } else { "inputs" };
                if !edit::rename_lane(&mut self.graph, rid, side, i.parse().unwrap_or(usize::MAX), &s("name")) {
                    self.send_canvas();
                    return;
                }
                Ok(())
            }
            // a group port's name (it may repeat)
            "rename" if edit::port_of(&self.graph, &s("key")).is_some() => {
                if !edit::set_port_name(&mut self.graph, &s("key"), &s("name")) {
                    self.send_canvas();
                    return;
                }
                Ok(())
            }
            // a canvas group's title (its key is "group:<id>")
            "rename" if s("key").starts_with("group:") => {
                if !edit::rename_group(&mut self.graph, &s("key")["group:".len()..], &s("name")) {
                    self.send_canvas();
                    return;
                }
                Ok(())
            }
            // canvas groups: a box around blocks (display only)
            "group" => {
                let keys: Vec<String> =
                    v["keys"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
                let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
                let Some(gid) = edit::group_blocks(&mut self.graph, &refs) else {
                    self.broadcast(&json!({"ev": "error", "message": tr("Select two blocks or more")}));
                    return;
                };
                self.auto_ports(&gid);
                Ok(())
            }
            // a port added to a group's edge (from its header): "in" / "out", where the window placed it
            "group_port_add" => {
                if edit::add_group_port(&mut self.graph, &s("id"), &s("side"), f("x"), f("y")).is_none() {
                    return;
                }
                Ok(())
            }
            // blocks' fill colour ("" = by kind)
            "fill" => {
                let keys: Vec<String> =
                    v["keys"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
                let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
                if !edit::set_fill(&mut self.graph, &refs, &s("color")) {
                    return;
                }
                Ok(())
            }
            // a block dropped onto a group's box
            "group_add" => {
                if !edit::add_to_group(&mut self.graph, &s("id"), &s("key")) {
                    return;
                }
                self.auto_ports(&s("id"));
                Ok(())
            }
            "ungroup" | "group_color" | "group_collapse" => {
                let id = s("id");
                let changed = match cmd {
                    "ungroup" => edit::ungroup(&mut self.graph, &id),
                    "group_color" => edit::set_group_color(&mut self.graph, &id, &s("color")),
                    _ => edit::set_group_collapsed(&mut self.graph, &id, v["collapsed"] == json!(true)),
                };
                if !changed {
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
            // a block's colour, or one router output's ("out" = rule id / "default"); "" = automatic
            "color" => {
                let key = s("key");
                let changed = match v["out"].as_str() {
                    Some(out) => {
                        proxydock_core::colors::output_color(&self.graph, &key, out) != s("color")
                            && proxydock_core::colors::set_output_color(&mut self.graph, &key, out, &s("color"))
                    }
                    None => proxydock_core::colors::set_color(
                        &mut self.graph,
                        key.strip_suffix("/out").unwrap_or(&key),
                        &s("color"),
                    ),
                };
                if !changed {
                    return;
                }
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

    /// A new block from the canvas's menus or toolbar: {"what": "inbound" | "outbound" | "router" | "if" | "relay" |
    /// "wireless" | "link", "type", "text" (a share link), "x", "y"}; older form {"kind"}. Returns the block to open
    /// in the editor, if any.
    fn add_block(&mut self, v: &Value) -> Option<String> {
        let f = |k: &str| v[k].as_f64().unwrap_or(0.0);
        let (x, y) = (f("x"), f("y"));
        let what = v["what"].as_str().unwrap_or("");
        let t = v["type"].as_str().unwrap_or("");
        let mut open = None;
        let id = match what {
            "inbound" if proxydock_core::config::INBOUND_TYPES.contains(&t) => {
                edit::add_inbound(&mut self.graph, t, x, y)
            }
            "outbound" if proxydock_core::config::OUTBOUND_TYPES.contains(&t) => {
                let id = edit::add_outbound(&mut self.graph, t, None, None, x, y);
                if t != "direct" {
                    open = Some(id.clone());
                }
                id
            }
            "router" | "if" => {
                let id = edit::add_router(&mut self.graph, what, x, y);
                open = Some(id.clone());
                id
            }
            "relay" => edit::add_relay(&mut self.graph, x, y),
            // the list page's "New proxy": an inbound (and its outbound); the outbound opens, else the inbound
            "template" => {
                let (id, fresh) = edit::add_template(&mut self.graph, v["index"].as_u64().unwrap_or(0) as usize)?;
                let name = graph::find(&self.graph, &id).map(|(_, n)| name_of(n)).unwrap_or_default();
                self.app_log(Level::Setting, &tr("Created proxy '{name}'").replace("{name}", &name));
                self.graph_changed();
                return Some(fresh.unwrap_or(id));
            }
            "wireless" => edit::add_wireless(&mut self.graph, x, y),
            "link" => match proxydock_core::links::parse(v["text"].as_str().unwrap_or("").trim()) {
                Ok((fields, name)) => {
                    let id = edit::add_outbound(&mut self.graph, "", Some(&fields), Some(&name), x, y);
                    open = Some(id.clone());
                    id
                }
                Err(e) => {
                    // (Python `add_from_link`'s message)
                    let msg = format!(
                        "{}: {e}\n{}",
                        tr("Cannot use the clipboard"),
                        tr("Copy a vless://, vmess://, trojan://, tuic://, ss:// or hysteria2:// link first")
                    );
                    self.broadcast(&json!({"ev": "error", "message": msg}));
                    return None;
                }
            },
            "" => edit::add(&mut self.graph, v["kind"].as_i64().unwrap_or(2) as i32, x, y),
            _ => return None,
        };
        // (a wireless pair is named by its number: Python `graph.wireless_label`)
        let name: Option<String> = graph::find(&self.graph, &id).map(|(_, n)| {
            if n["wireless"] == json!(true) {
                tr("Wireless {name}").replace("{name}", &format!("#{}", n["number"].as_i64().unwrap_or(0)))
            } else {
                name_of(n)
            }
        });
        if let Some(name) = name {
            self.app_log(Level::Setting, &tr("Created block '{name}'").replace("{name}", &name));
        }
        self.graph_changed();
        open
    }

    /// An entry of a block's right-click menu (`edit::block_menu`) chosen: {"key", "action"}
    fn node_action(&mut self, n: u64, v: &Value) {
        let key = v["key"].as_str().unwrap_or("").to_string();
        let action = v["action"].as_str().unwrap_or("").to_string();
        let (bid, _) = key.split_once("/out").map(|(b, _)| (b.to_string(), true)).unwrap_or((key.clone(), false));
        let changed = match action.as_str() {
            "edit" => {
                self.on_command(n, &json!({"cmd": "edit_open", "id": key}));
                return;
            }
            "toggle" => {
                self.toggle(&bid);
                return;
            }
            "copy" => {
                self.on_command(n, &json!({"cmd": "copy_client", "id": bid}));
                return;
            }
            "pc" => {
                self.edit("connect", &json!({"from": "pc", "out": 0, "to": bid, "in": 0}));
                return;
            }
            "pc.restore" | "pc.off" => {
                let cmd = if action == "pc.off" { "pc_off" } else { "pc_restore" };
                self.platform_command(&json!({"cmd": cmd}));
                return;
            }
            "latency" => {
                self.test_latency(&[bid]);
                return;
            }
            "duplicate" | "delete" => {
                self.edit(&action, &json!({"keys": [key]}));
                return;
            }
            "disconnect" => match graph::find(&self.graph, &bid).map(|(k, _)| k) {
                Some("inbound") => edit::set_target(&mut self.graph, &bid, None).is_ok(),
                Some("outbound") => edit::set_via(&mut self.graph, &bid, None).is_ok(),
                _ => false,
            },
            "cut" => edit::cut_links(&mut self.graph, &key),
            "wireless.repair" => edit::repair_wireless(&mut self.graph, &bid),
            a if a.starts_with("lane.add.") => edit::add_lane(&mut self.graph, &bid, &a["lane.add.".len()..]),
            "lane.mode.straight" => edit::set_relay_routed(&mut self.graph, &bid, false),
            "lane.mode.routed" => edit::set_relay_routed(&mut self.graph, &bid, true),
            "relay.fold" => edit::set_relay_folded(&mut self.graph, &bid, true),
            "relay.open" => edit::set_relay_folded(&mut self.graph, &bid, false),
            a if a.starts_with("lane.del.") => edit::remove_lane(&mut self.graph, &bid, &a["lane.del.".len()..]),
            a if a.starts_with("member.") => {
                self.platform_command(&json!({"cmd": "select_member", "group": bid, "member": &a["member.".len()..]}));
                return;
            }
            _ => false,
        };
        if changed {
            self.graph_changed();
        }
    }

    /// Every link crossing group gid's edge gets a port (Rust app only): links into one inner input share a port,
    /// named after the block inside; a link that cannot run through a pass-through (an outbound's via, a group
    /// outbound's member) stays as it is.
    fn auto_ports(&mut self, gid: &str) {
        self.rebuild();
        let Some(row) =
            self.graph.get("groups").and_then(Value::as_array).and_then(|a| a.iter().find(|x| x["id"] == json!(gid)))
        else {
            return;
        };
        let strs = |v: &Value| -> HashSet<String> {
            v.as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect()
        };
        let members = strs(&row["members"]);
        let mut ports = strs(&row["ports"]["in"]);
        ports.extend(strs(&row["ports"]["out"]));
        let inside = |k: &str| members.contains(k) || ports.contains(k);
        let node = |k: &str| self.view.nodes.iter().find(|n| n.key == k);
        let inner: Vec<&canvas::Node> = self.view.nodes.iter().filter(|n| members.contains(&n.key)).collect();
        if inner.is_empty() {
            return;
        }
        let x0 = inner.iter().map(|n| n.x).fold(f64::MAX, f64::min);
        let x1 = inner
            .iter()
            .map(|n| {
                n.x + if n.kind == 1 {
                    280.0
                } else if n.open {
                    400.0
                } else {
                    260.0
                }
            })
            .fold(f64::MIN, f64::max);
        // (inner input -> outside sources), (inner output -> outside input)
        let mut incoming: Vec<(PortRef, Vec<PortRef>)> = vec![];
        let mut outgoing: Vec<(PortRef, PortRef)> = vec![];
        for l in &self.view.links {
            let (a, b) = (inside(&l.from), inside(&l.to));
            if !a && b && !ports.contains(&l.to) {
                let k = (l.to.clone(), l.inp);
                match incoming.iter_mut().find(|(t, _)| *t == k) {
                    Some((_, srcs)) => srcs.push((l.from.clone(), l.out)),
                    None => incoming.push((k, vec![(l.from.clone(), l.out)])),
                }
            } else if a && !b && !ports.contains(&l.from) {
                outgoing.push(((l.from.clone(), l.out), (l.to.clone(), l.inp)));
            }
        }
        // a port's name: the inner block, and its port when it has several
        let label = |key: &str, k: usize, input: bool| -> String {
            let Some(n) = node(key) else { return String::new() };
            let (count, labels) = if input { (n.ins, &n.in_labels) } else { (n.outs, &n.out_labels) };
            match labels.get(k).filter(|_| count > 1) {
                Some(l) => format!("{}: {l}", n.title),
                None => n.title.clone(),
            }
        };
        let todo: Vec<(&str, PortRef, Vec<PortRef>, PortRef)> = incoming
            .into_iter()
            .map(|(to, srcs)| ("in", to.clone(), srcs, to))
            .chain(outgoing.into_iter().map(|(from, to)| ("out", from.clone(), vec![from], to)))
            .collect();
        // the input ports take the blocks' place on the left (the blocks move right to make room), so the group's
        // box does not grow over what feeds it; the output ports go right of the blocks
        let positions: Vec<(String, f64, f64)> = inner.iter().map(|n| (n.key.clone(), n.x, n.y)).collect();
        let mut shift = 0.0;
        for (side, (key, k), srcs, (to, inp)) in todo {
            let name = label(&key, k, side == "in");
            let y = node(&key).map_or(0.0, |n| n.y) + 56.0 + k as f64 * 28.0;
            let x = if side == "in" { x0 } else { x1 + shift + 60.0 };
            if side == "in" && shift == 0.0 {
                shift = 240.0;
                for (key, x, y) in &positions {
                    edit::move_block(&mut self.graph, key, x + shift, *y);
                }
            }
            let before = self.graph.clone();
            let Some(p) = edit::add_group_port(&mut self.graph, gid, side, x, y) else { continue };
            if !name.is_empty() {
                edit::set_port_name(&mut self.graph, &p, &name);
            }
            let ok = srcs.iter().all(|(f, o)| edit::connect(&mut self.graph, f, *o, &p, 0).is_ok())
                && edit::connect(&mut self.graph, &p, 0, &to, inp).is_ok();
            if !ok {
                self.graph = before;
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
        let pc = self.graph["pc"]["inbound"].as_str().and_then(|id| self.proxy(id)).map(name_of);
        *self.pc_card.lock().unwrap() = pc;
        let list = |k: &str| self.graph.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
        let st = |v: &Value| v.as_str().unwrap_or("").to_string();
        let switches = list("routers")
            .iter()
            .filter(|r| r["style"] == json!("if") && r["switch"] == json!(true))
            .map(|r| (st(&r["id"]), st(&r["name"]), r["switch_on"] == json!(true)))
            .collect();
        let groups = list("outbounds")
            .iter()
            .filter(|n| n["outbound"]["type"] == json!("group") && n["outbound"]["group_mode"] == json!("select"))
            .filter_map(|n| {
                let members: Vec<(String, String)> = n["members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|m| m.as_str())
                    .filter_map(|m| graph::find(&self.graph, m).map(|(_, b)| (m.to_string(), st(&b["name"]))))
                    .collect();
                let first = members.first()?.0.clone();
                let sel = st(&n["outbound"]["group_selected"]);
                let cur = if members.iter().any(|(m, _)| *m == sel) { sel } else { first };
                Some((st(&n["id"]), st(&n["name"]), members, cur))
            })
            .collect();
        *self.quick.lock().unwrap() = Quick { switches, groups, app_on: self.app_on };
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
                Msg::Tray(v) => self.platform_command(&v),
                Msg::Quit(done) => {
                    self.restart_at.clear();
                    self.manager.stop_all(Duration::from_secs(10));
                    if self.graph["pc"]["applied"] == json!(true) {
                        self.pc_restore_backup();
                        self.app_log(Level::Setting, &tr("System proxy restored to the previous settings"));
                        self.save();
                    }
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
                let canvas = self.canvas_json();
                let hello = json!({"ev": "hello", "language": self.glob["language"], "cards": cards, "logs": logs, "stats": self.stats_json(), "canvas": canvas, "undo": !self.undo.is_empty(), "redo": !self.redo.is_empty(),
                    "add_menu": self.add_menu_json(),
                    "settings": self.settings_json(), "latency": self.latency_json(),
                    "history": self.stat_history_json(), "conn": self.conn_sites_json(), "about": self.about_json(),
                    "update": self.update_json()});
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
            "route_of" => self.route_of(n, id, v["message"].as_str().unwrap_or("")),
            // the log page's Clear: one card's lines ("" = all of them) are forgotten
            "log_clear" => {
                if id.is_empty() {
                    self.logs.clear();
                } else {
                    self.logs.retain(|e| e.proxy_id != id);
                }
            }
            // the address or link a client app needs for card `id` (Python `proxies.copy_url`)
            "copy_client" => {
                let url = self
                    .proxy(id)
                    .map(|p| proxydock_core::links::client_url(p, &proxydock_core::wg::local_lan_ip()))
                    .unwrap_or_default();
                let tun = self.proxy(id).is_some_and(|p| p["inbound"]["type"] == "tun");
                let msg = if url.is_empty() {
                    let why = if tun {
                        tr("A TUN card captures apps on this PC directly; there is no address for clients")
                    } else {
                        tr("Nothing to copy")
                    };
                    json!({"ev": "error", "message": why})
                } else {
                    json!({"ev": "show_text", "title": "", "text": url})
                };
                self.send_to(n, &msg);
            }
            "move" | "connect" | "disconnect" | "lane_map" | "delete" | "duplicate" | "rename" | "color" | "fill"
            | "group" | "ungroup" | "group_color" | "group_collapse" | "group_add" | "group_port_add" => {
                self.edit(cmd, v)
            }
            "add" => {
                // (Python opens the editor for a new outbound other than Direct, and for a router / IF)
                if let Some(open) = self.add_block(v) {
                    self.on_command(n, &json!({"cmd": "edit_open", "id": open}));
                }
            }
            "node_action" => self.node_action(n, v),
            "settings_save" => self.save_settings(v),
            "latency" => {
                let keys: Vec<String> =
                    v["keys"].as_array().into_iter().flatten().filter_map(Value::as_str).map(String::from).collect();
                self.test_latency(&keys);
            }
            "quit" => (self.hooks.quit)(),
            "update_check" => self.check_update(true),
            "update_download" => self.download_update(),
            "update_cancel" => self.update.cancel.store(true, std::sync::atomic::Ordering::SeqCst),
            "update_install" => {
                let path = v["path"].as_str().unwrap_or("").to_string();
                self.install_update(&path);
            }
            "startup" | "startup_hidden" | "pc_off" | "pc_restore" | "firewall" | "set_switch" | "set_flag"
            | "select_member" => self.platform_command(v),
            "bypass_refresh" | "bypass_keep" | "bypass_add" | "bypass_set" | "bypass_prepare" | "bypass_remove"
            | "bypass_enable" => self.bypass_command(v),
            "home_power" => self.home_power(v["on"] == json!(true)),
            // "Restart now" (a new app scale: the window opens again) / the whole app
            "restart" => {
                let app = v["app"] == json!(true);
                if !app {
                    // the window closes itself (keeping its place), not the app ("Quit with the X button" or not)
                    self.broadcast(&json!({"ev": "close_window"}));
                }
                (self.hooks.restart)(app);
            }
            "conn_test" => self.conn_test(id),
            "test_site_add" => {
                let text = v["text"].as_str().unwrap_or("").to_string();
                self.add_test_site(&text);
            }
            "test_site_remove" => {
                if !self.conn_running {
                    let host = v["host"].clone();
                    let list: Vec<Value> = self.glob["test_sites"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|h| *h != host)
                        .collect();
                    self.set_test_sites(list);
                }
            }
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

    /// ProxyDock's own switch (the home page): off = every card refuses all connections while keeping its own
    /// on / off (a kill switch for the whole app), on = back to normal. Kept in home.json for the next start.
    fn home_power(&mut self, on: bool) {
        if self.app_on == on {
            return;
        }
        self.app_on = on;
        self.quick.lock().unwrap().app_on = on;
        let path = self.config_path.with_file_name("home.json");
        let _ = std::fs::write(&path, json!({"app_on": on}).to_string());
        let _ = self.manager.set_app_on(on);
        self.app_log(
            Level::Setting,
            &tr(if on { "ProxyDock switched on" } else { "ProxyDock switched off: every connection is refused" }),
        );
        self.send_settings();
    }

    /// the switch as home.json says (on when there is none)
    fn load_app_on(&mut self) {
        let path = self.config_path.with_file_name("home.json");
        self.app_on = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["app_on"].as_bool())
            .unwrap_or(true);
        self.quick.lock().unwrap().app_on = self.app_on;
        if !self.app_on {
            let _ = self.manager.set_app_on(false);
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
        if self.graph["pc"]["inbound"] == json!(id) && self.sync_pc() {
            self.save();
            self.rebuild();
            self.send_canvas();
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
        let mut sum = [0.0f64; 2];
        let mut counts = [0u64; 5];
        for p in &self.proxies {
            let id = p["id"].as_str().unwrap_or("");
            let s = self.manager.stats(id);
            sum[0] += s.up as f64;
            sum[1] += s.down as f64;
            for (c, v) in counts.iter_mut().zip([s.tcp_active, s.udp_active, s.total, s.failed, s.blocked]) {
                *c += v;
            }
            out.insert(id.into(), json!({"up": s.up, "down": s.down, "tcp": s.tcp_active, "udp": s.udp_active, "total": s.total, "failed": s.failed, "blocked": s.blocked}));
        }
        // "" = every card together (Python `Stats.sum`)
        out.insert("".into(), json!({"up": sum[0], "down": sum[1], "tcp": counts[0], "udp": counts[1], "total": counts[2], "failed": counts[3], "blocked": counts[4]}));
        Value::Object(out)
    }

    /// Once a second (Python `_tick` / `_exit_rates`): each card's and every exit block's rate since the last tick,
    /// kept for the last minute. Returns this second's point for the window.
    fn stat_tick(&mut self) -> Value {
        let now = Instant::now();
        let dt = now.duration_since(self.stat_last_t).as_secs_f64().max(1e-3);
        self.stat_last_t = now;
        let mut cards = Map::new();
        let (mut up_all, mut down_all) = (0.0, 0.0);
        let mut seen = std::collections::HashSet::new();
        let mut exits_now: HashMap<String, (f64, f64)> = HashMap::new();
        for p in &self.proxies {
            let id = p["id"].as_str().unwrap_or("").to_string();
            let s = self.manager.stats(&id);
            let (u, d) = (s.up as f64, s.down as f64);
            let (lu, ld) = self.stat_last.get(&id).copied().unwrap_or((u, d));
            let (ru, rd) = ((u - lu).max(0.0) / dt, (d - ld).max(0.0) / dt);
            self.stat_last.insert(id.clone(), (u, d));
            up_all += ru;
            down_all += rd;
            push_point(self.stat_hist.entry(id.clone()).or_default(), (ru, rd));
            cards.insert(id.clone(), json!([ru, rd]));
            for (exit, (eu, ed)) in self.manager.exit_totals(&id) {
                let e = exits_now.entry(exit).or_default();
                e.0 += eu;
                e.1 += ed;
            }
            seen.insert(id);
        }
        self.stat_last.retain(|k, _| seen.contains(k));
        self.stat_hist.retain(|k, _| k.is_empty() || seen.contains(k));
        push_point(self.stat_hist.entry(String::new()).or_default(), (up_all, down_all));
        cards.insert(String::new(), json!([up_all, down_all]));
        let mut exits = Map::new();
        for (oid, (u, d)) in &exits_now {
            let (lu, ld) = self.exit_last.get(oid).copied().unwrap_or((*u, *d));
            let r = ((u - lu).max(0.0) + (d - ld).max(0.0)) / dt;
            exits.insert(oid.clone(), json!(r));
        }
        for (oid, h) in self.exit_hist.iter_mut() {
            if !exits_now.contains_key(oid) {
                push_point(h, 0.0);
            }
        }
        for (oid, r) in &exits {
            push_point(self.exit_hist.entry(oid.clone()).or_default(), r.as_f64().unwrap_or(0.0));
        }
        self.exit_last = exits_now;
        json!({"cards": cards, "exits": exits})
    }

    /// the last minute for a newly opened window, and the names and colours the graph needs
    fn stat_history_json(&self) -> Value {
        let cards: Map<String, Value> = self
            .stat_hist
            .iter()
            .map(|(k, h)| (k.clone(), json!(h.iter().map(|(u, d)| [*u, *d]).collect::<Vec<_>>())))
            .collect();
        let exits: Map<String, Value> = self.exit_hist.iter().map(|(k, h)| (k.clone(), json!(h))).collect();
        json!({"cards": cards, "exits": exits, "meta": self.stat_meta()})
    }

    /// cards and outbound blocks for the graph: [id, name, colour]
    fn stat_meta(&self) -> Value {
        let colour = |id: &str| {
            graph::find(&self.graph, id).and_then(|(_, n)| n["color"].as_str().map(String::from)).unwrap_or_default()
        };
        let cards: Vec<Value> = self
            .proxies
            .iter()
            .map(|p| {
                let id = p["id"].as_str().unwrap_or("");
                json!([id, p["name"], colour(id)])
            })
            .collect();
        let exits: Vec<Value> = self
            .graph
            .get("outbounds")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|n| n["outbound"]["type"] != "block")
            .map(|n| json!([n["id"], n["name"], n["color"].as_str().unwrap_or("")]))
            .collect();
        json!({"cards": cards, "exits": exits})
    }

    /// The connection test's sites and whether it runs (Python `ConnTestInterface.sites`)
    fn conn_sites_json(&self) -> Value {
        let sites: Vec<Value> =
            proxydock_core::probe::test_sites(&self.glob).into_iter().map(|(g, n, h, a)| json!([g, n, h, a])).collect();
        json!({"sites": sites, "running": self.conn_running, "workers": proxydock_core::probe::PROBE_WORKERS,
               "cooldown": proxydock_core::probe::PROBE_COOLDOWN, "timeout": 8})
    }

    fn send_conn_sites(&mut self) {
        let msg = json!({"ev": "conn_sites", "conn": self.conn_sites_json()});
        self.broadcast(&msg);
    }

    /// "Add" on the connection test page (Python `ConnTestInterface.add_site` / `set_test_sites`)
    fn add_test_site(&mut self, text: &str) {
        if self.conn_running {
            return;
        }
        let Some(host) = config::test_site_host(text) else {
            let msg = json!({"ev": "error", "message": format!("{}: {}", tr("Cannot add"), tr("Enter a site name like example.com"))});
            self.broadcast(&msg);
            return;
        };
        if proxydock_core::probe::test_sites(&self.glob).iter().any(|(_, _, h, _)| *h == host) {
            let msg = json!({"ev": "info", "message": format!("{}: {host}", tr("Already in the list"))});
            self.broadcast(&msg);
            return;
        }
        let mut list: Vec<Value> = self.glob["test_sites"].as_array().cloned().unwrap_or_default();
        if list.len() >= config::MAX_TEST_SITES {
            let msg = json!({"ev": "error", "message": format!("{}: {}", tr("Cannot add"), tr("Up to {n} added sites").replace("{n}", &config::MAX_TEST_SITES.to_string()))});
            self.broadcast(&msg);
            return;
        }
        list.push(json!(host));
        self.set_test_sites(list);
    }

    fn set_test_sites(&mut self, hosts: Vec<Value>) {
        let mut g = self.glob.clone();
        g.insert("test_sites".into(), Value::Array(hosts));
        let g = config::validate_global(&Value::Object(g));
        if g["test_sites"] != self.glob["test_sites"] {
            self.glob = g;
            *self.shared_glob.lock().unwrap() = self.glob.clone();
            self.save();
        }
        self.send_conn_sites();
    }

    /// "Start test": a temporary SOCKS5 bridge to the running card, then every site over HTTPS through it, 6 at
    /// a time; each result as it comes (Python `ConnTestInterface._start` / `_on_result` / `_on_done`)
    fn conn_test(&mut self, id: &str) {
        if self.conn_running {
            return;
        }
        let Some(proxy) = self.proxy(id).cloned() else { return };
        if self.manager.state(id) != State::Running {
            return;
        }
        let hosts: Vec<String> =
            proxydock_core::probe::test_sites(&self.glob).into_iter().map(|(_, _, h, _)| h).collect();
        self.conn_running = true;
        self.conn_ok.clear();
        self.conn_total = hosts.len();
        let name = name_of(&proxy);
        let msg = tr("Connection test started: {name} ({len} sites)")
            .replace("{name}", &name)
            .replace("{len}", &hosts.len().to_string());
        self.app_log(Level::Info, &msg);
        let start = json!({"ev": "conn_start", "name": name, "total": hosts.len()});
        self.broadcast(&start);
        let (tx, remote, glob) = (self.tx.clone(), self.manager.remote(), self.glob.clone());
        std::thread::spawn(move || {
            use proxydock_core::probe;
            let port = probe::free_local_port();
            let started = remote
                .start(&probe::bridge_for(&proxy, port), &glob)
                .recv_timeout(Duration::from_secs(15))
                .unwrap_or(false);
            let mut err = String::new();
            if started {
                // the waterfall's zero: every site's start is measured from here
                let t_test = Instant::now();
                // (taken from the end: reversed, so the first site starts first)
                let queue = Arc::new(Mutex::new(hosts.iter().cloned().enumerate().rev().collect::<Vec<_>>()));
                let workers: Vec<_> = (0..probe::PROBE_WORKERS.min(hosts.len()))
                    .map(|_| {
                        let (queue, tx) = (queue.clone(), tx.clone());
                        std::thread::spawn(move || {
                            loop {
                                let next = queue.lock().unwrap().pop();
                                let Some((row, host)) = next else { break };
                                let begin = t_test.elapsed().as_secs_f64() * 1000.0;
                                let mut res = probe::probe_site(&host, port);
                                res.insert("start_ms".into(), json!(begin));
                                let _ =
                                    tx.send(Msg::Work(Box::new(move |c: &mut Controller| c.on_conn_result(row, res))));
                            }
                        })
                    })
                    .collect();
                for w in workers {
                    let _ = w.join();
                }
            } else {
                err = match remote.state(probe::BRIDGE_ID) {
                    State::Error(m) if !m.is_empty() => m,
                    _ => tr("Could not create the test connection"),
                };
            }
            let _ = remote.stop(probe::BRIDGE_ID).recv_timeout(Duration::from_secs(5));
            let _ = tx.send(Msg::Work(Box::new(move |c: &mut Controller| c.on_conn_done(err))));
        });
    }

    fn on_conn_result(&mut self, row: usize, res: Map<String, Value>) {
        let (text, conn, total, ok) = proxydock_core::probe::site_result(&res);
        if ok {
            self.conn_ok.push(res.get("total_ms").and_then(Value::as_f64).unwrap_or(0.0));
        }
        // the waterfall: when the site started and the end of each phase (ms; null = not reached)
        let at = |k: &str| res.get(k).cloned().unwrap_or(Value::Null);
        let timing = json!({"start": at("start_ms"), "tcp": at("tcp_ms"), "connect": at("connect_ms"), "tls": at("tls_ms"),
            "first_byte": at("first_byte_ms"), "total": at("total_ms")});
        let msg = json!({"ev": "conn_result", "row": row, "result": [text, conn, total, ok], "timing": timing});
        self.broadcast(&msg);
    }

    fn on_conn_done(&mut self, err: String) {
        self.conn_running = false;
        let summary = if !err.is_empty() {
            self.app_log(Level::Fail, &tr("Connection test failed: {err}").replace("{err}", &err));
            tr("Could not start the test: {err}").replace("{err}", &err)
        } else {
            let n = self.conn_ok.len();
            let mut msg = tr("Succeeded {len} / {len2}")
                .replace("{len}", &n.to_string())
                .replace("{len2}", &self.conn_total.to_string());
            if n > 0 {
                let avg = self.conn_ok.iter().sum::<f64>() / n as f64;
                msg += &tr(" · average {avg:.0f} ms").replace("{avg:.0f}", &format!("{avg:.0}"));
            }
            self.app_log(Level::Info, &tr("Connection test finished: {msg}").replace("{msg}", &msg));
            let clock = log::now_text();
            msg + &tr("  ·  finished at {strftime}").replace("{strftime}", clock.get(11..).unwrap_or(""))
        };
        let msg = json!({"ev": "conn_done", "error": err, "summary": summary});
        self.broadcast(&msg);
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
            if p["auto_restart"] == json!(true) && !matches!(self.manager.state(&id), State::Running | State::Starting)
            {
                let _ = self.manager.start(&p, &self.glob);
            }
        }
        if self.update.next_check.is_some_and(|t| now >= t) {
            self.update.next_check = None;
            let unix =
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
            if proxydock_core::update::due(&self.glob, unix) {
                self.check_update(false);
            }
        }
        let point = self.stat_tick();
        // the tray icon's tooltip (Python `_tick`)
        let running = self
            .proxies
            .iter()
            .filter(|p| self.manager.state(p["id"].as_str().unwrap_or("")) == State::Running)
            .count();
        let (up, down) = (point["cards"][""][0].as_f64().unwrap_or(0.0), point["cards"][""][1].as_f64().unwrap_or(0.0));
        let conns: u64 = self
            .proxies
            .iter()
            .map(|p| {
                let s = self.manager.stats(p["id"].as_str().unwrap_or(""));
                s.tcp_active + s.udp_active
            })
            .sum();
        let tip = tr("{APP_NAME}\n{running} proxies running\n↓ {fmt_rate}  ↑ {fmt_rate2}  · connections {value}")
            .replace("{APP_NAME}", "ProxyDock")
            .replace("{running}", &running.to_string())
            .replace("{fmt_rate2}", &format!("{}/s", proxydock_core::log::fmt_size(up)))
            .replace("{fmt_rate}", &format!("{}/s", proxydock_core::log::fmt_size(down)))
            .replace("{value}", &conns.to_string());
        let tip = if self.app_on {
            tip
        } else {
            format!(
                "{tip}
{}",
                tr("ProxyDock is off")
            )
        };
        if tip != self.tooltip {
            (self.hooks.tooltip)(&tip);
            self.tooltip = tip;
        }
        if self.clients.values().any(|c| c.ready) {
            let stats = self.stats_json();
            self.broadcast(&json!({"ev": "stats", "stats": stats, "point": point}));
            self.send_flow();
        }
    }
}

impl Controller {
    /// The canvas links a logged connection of card `id` went along (answered to the asking window only):
    /// the exit from the engine's recent connections, or the card's only exit.
    fn route_of(&mut self, client: u64, id: &str, message: &str) {
        let exits: Vec<&String> = self.view.paths.keys().filter(|(card, _)| card == id).map(|(_, exit)| exit).collect();
        let exit = conn_of_line(message)
            .and_then(|(who, target)| self.manager.route_of(id, &who, target.as_deref()))
            .or_else(|| (exits.len() == 1).then(|| exits[0].clone()));
        let links: Vec<usize> =
            exit.and_then(|e| self.view.paths.get(&(id.to_string(), e)).cloned()).unwrap_or_default();
        let error = if links.is_empty() {
            tr("This connection's way is no longer known (only recent connections of running cards are)")
        } else {
            String::new()
        };
        self.send_to(client, &json!({"ev": "route", "links": links, "error": error}));
    }

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

/// one more second in a last-minute history
fn push_point<T: Default + Clone>(h: &mut VecDeque<T>, v: T) {
    if h.is_empty() {
        h.extend(std::iter::repeat_n(T::default(), STAT_POINTS));
    }
    h.push_back(v);
    while h.len() > STAT_POINTS {
        h.pop_front();
    }
}

fn name_of(p: &Value) -> String {
    p["name"].as_str().unwrap_or("").to_string()
}

/// the memory choices of a VPN bypass VM (MB; at least bypass::MIN_MEMORY_MB)
pub const BYPASS_MEMORY: [u32; 7] = [200, 256, 384, 512, 768, 1024, 2048];

/// a fixed address as pd-agent will check it (the same rules, so the VM never gets one it refuses)
fn check_static(req: &Value) -> Result<(), String> {
    use std::net::{IpAddr, Ipv4Addr};
    let s = |k: &str| req[k].as_str().unwrap_or("").to_string();
    let cidr = s("address");
    let (ip, prefix) = cidr.split_once('/').ok_or_else(|| tr("Address: like 192.168.0.50/24"))?;
    let ip: Ipv4Addr = ip.parse().map_err(|_| tr("Address: like 192.168.0.50/24"))?;
    let prefix: u32 =
        prefix.parse().ok().filter(|p| (1..=32).contains(p)).ok_or_else(|| tr("Address: like 192.168.0.50/24"))?;
    let gw: Ipv4Addr = s("gateway").parse().map_err(|_| tr("Gateway: like 192.168.0.1"))?;
    let mask = u32::MAX << (32 - prefix);
    if u32::from(ip) & mask != u32::from(gw) & mask {
        return Err(tr("The gateway must be in the address's network"));
    }
    let dns: Vec<&str> = req["dns"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    if dns.is_empty() || dns.iter().any(|d| d.parse::<IpAddr>().is_err()) {
        return Err(tr("DNS servers: addresses like 1.1.1.1, 8.8.8.8"));
    }
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod route_tests {
    use super::conn_of_line;

    #[test]
    fn connection_lines_give_client_and_destination() {
        assert_eq!(
            conn_of_line("127.0.0.1:5000 -> example.com:443"),
            Some(("127.0.0.1:5000".into(), Some("example.com:443".into())))
        );
        assert_eq!(
            conn_of_line("127.0.0.1:5000 -> 1.1.1.1:53 UDP"),
            Some(("127.0.0.1:5000".into(), Some("1.1.1.1:53".into())))
        );
        assert_eq!(
            conn_of_line("[::1]:5000 -> example.com:443 closed · normal close · ↑ 1 KB ↓ 2 KB · 1.0s"),
            Some(("[::1]:5000".into(), Some("example.com:443".into())))
        );
        assert_eq!(
            conn_of_line("127.0.0.1:5000 UDP session ended · ↑ 1 B ↓ 2 B · 0.5s"),
            Some(("127.0.0.1:5000".into(), None))
        );
        assert_eq!(conn_of_line("Card started"), None);
        assert_eq!(conn_of_line("sing-box: something UDP failed"), None);
    }
}
