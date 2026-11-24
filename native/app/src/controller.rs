//! The core process's controller (Python: the non-UI half of `ui/window.py`): reads the settings, runs the cards
//! through the manager (autostart, auto-restart with back-off), keeps the recent log, and serves the window
//! process over ipc. Everything happens on one controller thread fed by a channel.

use crate::ipc;
use proxydock_core::i18n::tr;
use proxydock_core::log::{Entry, Level};
use proxydock_core::manager::{self, Events, Manager, State};
use proxydock_core::{config, graph, log};
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
}

/// a card as the tray menu shows it
#[derive(Clone, Debug)]
pub struct CardInfo {
    pub id: String,
    pub name: String,
    pub state: State,
}

/// What the tray holds: commands to the controller and the window's connection details.
#[derive(Clone)]
pub struct Handle {
    tx: Sender<Msg>,
    pub port: u16,
    pub token: String,
    cards: Arc<Mutex<Vec<CardInfo>>>,
}

impl Handle {
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
    };
    c.load();
    std::thread::Builder::new().name("controller".into()).spawn(move || c.run(rx))?;
    Ok(Handle { tx, port, token, cards })
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
        self.logs.push_back(e);
        while self.logs.len() > LOG_KEEP {
            self.logs.pop_front();
        }
    }

    /// settings file -> global settings and the compiled cards; then the autostart cards start
    fn load(&mut self) {
        match config::load_from(&self.config_path) {
            Ok(l) => {
                self.glob = l.global;
                proxydock_core::i18n::set_language(self.glob["language"].as_str().unwrap_or("en"));
                let g = graph::validate(&Value::Object(l.graph));
                self.proxies = graph::compile(&g).into_iter().map(Value::Object).collect();
            }
            Err(config::LoadError::Missing) => {}
            Err(e) => {
                let msg = format!("{}: {e:?}", self.config_path.display());
                self.app_log(Level::Error, &msg);
            }
        }
        self.publish_cards();
        let auto: Vec<Value> = self.proxies.iter().filter(|p| p["autostart"] == json!(true)).cloned().collect();
        for p in auto {
            let _ = self.manager.start(&p, &self.glob);
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
                Msg::Toggle(id) => self.toggle(&id),
                Msg::StopAll => self.stop_all(),
                Msg::Quit(done) => {
                    self.restart_at.clear();
                    self.manager.stop_all(Duration::from_secs(10));
                    let _ = done.send(());
                    break;
                }
            }
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
                let hello = json!({"ev": "hello", "language": self.glob["language"], "cards": cards, "logs": logs, "stats": self.stats_json()});
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
        // (a card that fails without auto-restart shows its error on the card; tray notifications come later)
        if let (State::Error(_), Some(p)) = (&st, self.proxy(id).cloned()) {
            if p["auto_restart"] == json!(true) {
                self.schedule_restart(&p);
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
        }
    }
}

fn name_of(p: &Value) -> String {
    p["name"].as_str().unwrap_or("").to_string()
}
