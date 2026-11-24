//! One proxy card run by its own sing-box process (Python: `singbox/process.py`, `SingBoxInstance`).
//!
//! Threads instead of an event loop: one reads sing-box's output (stdout and stderr each), one polls the Clash
//! API once a second (traffic, open connections, idle TCP timeout, per-IP limit). The owner gets log entries and
//! a "died" notice through the hooks it passes in.

use super::compile::{self as sbc, BuildEnv, Reason};
use super::guard;
use crate::config;
use crate::i18n::tr;
use crate::log::{fmt_size, Level};
use crate::net::host_port;
use regex_lite::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const START_TIMEOUT: Duration = Duration::from_secs(10);
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

// "LEVEL[0001] [1234567 5ms] message" (the bracket with the connection id is only on connection lines)
static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Z]+)\[\d+\]\s+(?:\[(\d+)\s[^\]]*\]\s+)?(.*)$").unwrap());
static FROM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:inbound|endpoint)/[\w-]+\[[^\]]*\]: (?:inbound )?(?:packet )?connection from (\S+)$").unwrap()
});
static TO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:inbound|endpoint)/[\w-]+\[[^\]]*\]: (?:\[[^\]]*\] )?inbound (packet )?connection to (\S+)$").unwrap()
});
static PROC_ERR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:inbound|endpoint)/[\w-]+\[[^\]]*\]: process connection from (\S+): (.*)$").unwrap()
});
static AUTH_USER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"authentication failed, username=(.*?), password=").unwrap());
static OPEN_FAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^connection: open (?:packet )?connection to (\S+) using outbound/[\w-]+\[[^\]]*\]: (.*)$").unwrap()
});
static REJECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^router: match\[(\d+)\].*=> reject$").unwrap());
static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new("\x1b\\[[0-9;]*m").unwrap());
static SECRET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(password|secret)=\S+").unwrap());
const BENIGN: [&str; 11] = [
    "use of closed network connection", "wsasend", "wsarecv", "EOF", "connection reset", "broken pipe",
    "forcibly closed", "aborted", "i/o timeout", "context canceled", "no Proxy-Authorization header",
];

fn hide_secrets(text: &str) -> String {
    SECRET.replace_all(text, "$1=***").into_owned()
}

/// Python `repr()` of a string
fn py_repr(s: &str) -> String {
    let q = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::from(q);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == q => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(q);
    out
}

fn ip_of(addr: &str) -> String {
    crate::net::split_host_port(addr, Some(0)).map(|(h, _)| h).unwrap_or_else(|_| addr.to_string())
}

/// Traffic and connection counters of a card (Python `common.Stats`), kept across restarts by the owner.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub tcp_active: u64,
    pub udp_active: u64,
    pub total: u64,
    pub failed: u64,
    pub blocked: u64,
    pub up: u64,
    pub down: u64,
}

/// Why a card did not start.
#[derive(Debug, Clone, PartialEq)]
pub enum StartError {
    PortInUse(String),
    AccessDenied(String),
    AddressNotAvailable(String),
    /// sing-box missing, the config cannot be built, the files cannot be written, ...
    Other(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            StartError::PortInUse(t) | StartError::AccessDenied(t) | StartError::AddressNotAvailable(t) | StartError::Other(t) => {
                f.write_str(t)
            }
        }
    }
}

pub(crate) fn start_error(text: &str) -> StartError {
    let t = text.to_lowercase();
    if t.contains("only one usage of each socket address") || t.contains("address already in use") {
        StartError::PortInUse(text.into())
    } else if t.contains("forbidden by its access permissions") || t.contains("permission denied") {
        StartError::AccessDenied(text.into())
    } else if t.contains("requested address is not valid") || t.contains("cannot assign requested address") {
        StartError::AddressNotAvailable(text.into())
    } else {
        StartError::Other(format!("sing-box: {}", hide_secrets(text)))
    }
}

/// What the owner of a running card hears from it.
#[derive(Clone)]
pub struct Hooks {
    /// a log entry of this card (level, message)
    pub log: Arc<dyn Fn(Level, &str) + Send + Sync>,
    /// sing-box exited on its own (the message says how)
    pub died: Arc<dyn Fn(String) + Send + Sync>,
}

/// What this card knows about a connection between two polls (close details, idle timeout).
struct Live {
    who: String,
    target: String,
    udp: bool,
    up: u64,
    down: u64,
    start: f64,
    change: Instant,
    reason: Option<String>,
}

struct State {
    proxy: Value,
    cfg: Value,
    reasons: Vec<Option<Reason>>,
    stats: Stats,
    last_error: String,
    last_reason: String,
    /// sing-box connection id -> client "ip:port" / destination "host:port"
    clients: HashMap<String, String>,
    targets: HashMap<String, String>,
    last_totals: (u64, u64),
    live: HashMap<String, Live>,
    /// outbound tag a connection left through (chains[0]) -> [up, down] so far / in this poll only
    tag_bytes: HashMap<String, (f64, f64)>,
    tick_tags: HashMap<String, (f64, f64)>,
    raw_tail: VecDeque<String>,
}

struct Shared {
    id: String,
    hooks: Hooks,
    state: Mutex<State>,
    stopping: AtomicBool,
}

impl Shared {
    fn log(&self, level: Level, message: &str) {
        let message = crate::log::clean(message);
        if level.is_problem() {
            self.state.lock().unwrap().last_error = message.clone();
        }
        (self.hooks.log)(level, &message);
    }

    fn log_conn(&self, level: Level, message: &str) {
        let on = self.state.lock().unwrap().cfg["log_connections"].as_bool().unwrap_or(true);
        if on {
            self.log(level, message);
        }
    }
}

/// Where a card's sing-box runs from and the files it needs.
#[derive(Clone)]
pub struct EngineEnv {
    pub exe: PathBuf,
    pub run_dir: PathBuf,
    pub bans_path: PathBuf,
    /// rule-set tag -> local file (GeoIP / Geosite)
    pub rulesets: Vec<(String, String)>,
    /// (certificate, key) for https / hysteria2 inbounds
    pub cert: Option<(String, String)>,
    /// ProxyDock's own engines: a TUN card routes them around itself
    pub exclude_paths: Vec<String>,
    /// run sing-box through the administrator helper (a TUN card while ProxyDock is not elevated)
    pub helper: Option<super::helper::Launch>,
}

/// sing-box started directly, or by the administrator helper
enum Proc {
    Local(Child),
    Helper(super::helper::Conn),
}

impl Proc {
    fn kill_and_wait(&mut self) {
        match self {
            Proc::Local(c) => {
                let _ = c.kill();
                let _ = c.wait();
            }
            Proc::Helper(h) => {
                h.kill();
                let until = Instant::now() + Duration::from_secs(5);
                while h.code().is_none() && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(50));
                }
                h.close(); // ends the reader if the helper did not answer
            }
        }
    }

    /// the exit code once the process ended (its output is closed)
    fn exit_code(&mut self) -> Option<i32> {
        match self {
            Proc::Local(c) => c.wait().ok().and_then(|s| s.code()),
            Proc::Helper(h) => h.code(),
        }
    }
}

static TUN_OWNER: Mutex<Option<String>> = Mutex::new(None);

/// A running card.
pub struct Engine {
    shared: Arc<Shared>,
    child: Arc<Mutex<Option<Proc>>>,
    config_path: PathBuf,
    api: (u16, String),
    threads: Vec<std::thread::JoinHandle<()>>,
}

fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

pub(crate) fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
        BCryptGenRandom(std::ptr::null_mut(), buf.as_mut_ptr(), buf.len() as u32, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
    }
    #[cfg(not(windows))]
    {
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let _ = f.read_exact(&mut buf);
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(windows)]
pub(crate) fn attach_to_job(child: &Child) {
    // a job object that kills its processes when ProxyDock's handle closes (even if ProxyDock crashes)
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::JobObjects::*;
    static JOB: LazyLock<usize> = LazyLock::new(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        job as usize
    });
    unsafe {
        AssignProcessToJobObject(*JOB as _, child.as_raw_handle() as _);
    }
}

/// the Clash API: one request, the JSON body (None when empty)
fn api(port: u16, secret: &str, method: &str, path: &str) -> std::io::Result<Option<Value>> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(3))?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    write!(s, "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {secret}\r\n\r\n")?;
    let mut data = vec![];
    s.read_to_end(&mut data)?;
    let body = match data.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(i) => &data[i + 4..],
        None => &[][..],
    };
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice(body).map(Some).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Clash API "start" (RFC 3339 with nanoseconds) -> epoch seconds (now if it cannot be read)
pub fn started_at(text: &str) -> f64 {
    fn now() -> f64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
    }
    let re = Regex::new(r"^(\d{4})-(\d\d)-(\d\d)T(\d\d):(\d\d):(\d\d)(?:\.(\d+))?(Z|[+-]\d\d:\d\d)?$").unwrap();
    let Some(c) = re.captures(text) else { return now() };
    let n = |i: usize| c.get(i).unwrap().as_str().parse::<i64>().unwrap();
    let (y, mo, d, h, mi, s) = (n(1), n(2), n(3), n(4), n(5), n(6));
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || s > 59 {
        return now();
    }
    // days from civil (Howard Hinnant)
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    let frac = c.get(7).map(|f| {
        let six: String = f.as_str().chars().take(6).collect();
        format!("{six:0<6}").parse::<f64>().unwrap() / 1e6
    });
    let offset = match c.get(8).map(|m| m.as_str()) {
        None | Some("Z") => 0,
        Some(tz) => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            sign * (tz[1..3].parse::<i64>().unwrap() * 3600 + tz[4..6].parse::<i64>().unwrap() * 60)
        }
    };
    (days * 86400 + h * 3600 + mi * 60 + s - offset) as f64 + frac.unwrap_or(0.0)
}

impl Engine {
    /// Start sing-box for this card and wait until it reports "sing-box started".
    pub fn start(proxy: &Value, glob: &serde_json::Map<String, Value>, env: &EngineEnv, hooks: Hooks) -> Result<Engine, StartError> {
        let id = proxy["id"].as_str().unwrap_or("").to_string();
        if !env.exe.is_file() {
            return Err(StartError::Other(tr("sing-box was not found")));
        }
        if !guard::write_bans(true) && !env.bans_path.exists() {
            return Err(StartError::Other(tr("Could not write {path}").replace("{path}", &env.bans_path.display().to_string())));
        }
        let tun = proxy["inbound"]["type"] == json!("tun");
        if tun {
            let mut owner = TUN_OWNER.lock().unwrap();
            if owner.as_ref().is_some_and(|o| *o != id) {
                return Err(StartError::Other(tr("Another TUN card is already running; only one can run at a time")));
            }
            *owner = Some(id.clone());
        }
        let release_tun = || {
            let mut owner = TUN_OWNER.lock().unwrap();
            if owner.as_deref() == Some(id.as_str()) {
                *owner = None;
            }
        };
        let cfg = Value::Object(config::resolve(proxy, glob));
        let port = free_port().map_err(|e| StartError::Other(e.to_string()))?;
        let secret = random_hex(16);
        let bans = env.bans_path.display().to_string();
        let build_env = BuildEnv {
            bans_path: &bans,
            api_port: port,
            api_secret: &secret,
            cert: env.cert.as_ref().map(|(c, k)| (c.as_str(), k.as_str())),
            rulesets: &env.rulesets,
            exclude_paths: &env.exclude_paths,
        };
        let (conf, reasons) = match sbc::build(proxy, &cfg, &build_env) {
            Ok(x) => x,
            Err(e) => {
                release_tun();
                return Err(StartError::Other(format!("{e:?}")));
            }
        };
        let config_path = env.run_dir.join(format!("{id}.json"));
        if let Err(e) = write_private(&config_path, &serde_json::to_string_pretty(&conf).unwrap()) {
            release_tun();
            return Err(StartError::Other(e.to_string()));
        }
        // both pipes (or the helper's one stream) feed one line handler, in the order lines arrive on each
        let (line_tx, line_rx) = mpsc::channel::<Option<String>>();
        let mut threads = vec![];
        let (proc_, pipes) = if let Some(how) = &env.helper {
            (hooks.log)(Level::Info, &tr("Asking for administrator permission to start the TUN…"));
            let conn = match super::helper::launch(how, &config_path, &env.run_dir) {
                Ok(c) => c,
                Err(e) => {
                    release_tun();
                    let _ = std::fs::remove_file(&config_path);
                    return Err(StartError::Other(e));
                }
            };
            match conn.spawn_reader(line_tx.clone()) {
                Ok(t) => threads.push(t),
                Err(e) => {
                    release_tun();
                    let _ = std::fs::remove_file(&config_path);
                    return Err(StartError::Other(e.to_string()));
                }
            }
            (Proc::Helper(conn), 1)
        } else {
            let mut cmd = Command::new(&env.exe);
            cmd.arg("run").arg("--disable-color").arg("-c").arg(&config_path).arg("-D").arg(&env.run_dir);
            cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    release_tun();
                    let _ = std::fs::remove_file(&config_path);
                    return Err(StartError::Other(e.to_string()));
                }
            };
            #[cfg(windows)]
            attach_to_job(&child);
            let mut n = 0;
            for pipe in [child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)]
                .into_iter()
                .flatten()
            {
                let tx = line_tx.clone();
                n += 1;
                threads.push(std::thread::spawn(move || {
                    for line in BufReader::new(pipe).split(b'\n').map_while(Result::ok) {
                        let _ = tx.send(Some(String::from_utf8_lossy(&line).into_owned()));
                    }
                    let _ = tx.send(None);
                }));
            }
            (Proc::Local(child), n)
        };
        drop(line_tx);
        let shared = Arc::new(Shared {
            id: id.clone(),
            hooks,
            state: Mutex::new(State {
                proxy: proxy.clone(),
                cfg,
                reasons,
                stats: Stats::default(),
                last_error: String::new(),
                last_reason: String::new(),
                clients: HashMap::new(),
                targets: HashMap::new(),
                last_totals: (0, 0),
                live: HashMap::new(),
                tag_bytes: HashMap::new(),
                tick_tags: HashMap::new(),
                raw_tail: VecDeque::new(),
            }),
            stopping: AtomicBool::new(false),
        });
        let child = Arc::new(Mutex::new(Some(proc_)));
        let (started_tx, started_rx) = mpsc::channel::<Result<(), StartError>>();
        {
            let shared = shared.clone();
            let child = child.clone();
            threads.push(std::thread::spawn(move || read_output(&shared, &child, line_rx, pipes, started_tx)));
        }
        let result = started_rx.recv_timeout(START_TIMEOUT).unwrap_or_else(|_| Err(StartError::Other(tr("sing-box did not start in time"))));
        let mut engine = Engine { shared, child, config_path, api: (port, secret), threads };
        if let Err(e) = result {
            engine.stop();
            return Err(e);
        }
        {
            let shared = engine.shared.clone();
            let (port, secret) = engine.api.clone();
            engine.threads.push(std::thread::spawn(move || poll(&shared, port, &secret)));
        }
        Ok(engine)
    }

    pub fn id(&self) -> &str {
        &self.shared.id
    }

    pub fn stats(&self) -> Stats {
        self.shared.state.lock().unwrap().stats.clone()
    }

    pub fn last_error(&self) -> String {
        self.shared.state.lock().unwrap().last_error.clone()
    }

    /// "denied" / "auth" / "net": the kind of the last failure (for the connection check)
    pub fn last_reason(&self) -> String {
        self.shared.state.lock().unwrap().last_reason.clone()
    }

    /// whether new settings need a restart (route settings live in the sing-box config)
    pub fn needs_restart(&self, proxy: &Value, glob: &serde_json::Map<String, Value>) -> bool {
        let st = self.shared.state.lock().unwrap();
        let new = config::resolve(proxy, glob);
        ["allowed_domains", "block_private", "timeout", "dns_server", "dns_via_outbound"]
            .iter()
            .any(|k| new[*k] != st.cfg[*k])
            || proxy.get("routes") != st.proxy.get("routes")
    }

    /// settings that apply without a restart (logging switches, names)
    pub fn update(&self, proxy: &Value, glob: &serde_json::Map<String, Value>) {
        let mut st = self.shared.state.lock().unwrap();
        st.proxy = proxy.clone();
        st.cfg = Value::Object(config::resolve(proxy, glob));
    }

    /// {outbound block id: (up, down) bytes so far} from this card's connections
    pub fn exit_totals(&self) -> HashMap<String, (f64, f64)> {
        let st = self.shared.state.lock().unwrap();
        let mut out: HashMap<String, (f64, f64)> = HashMap::new();
        for (tag, (up, down)) in &st.tag_bytes {
            if let Some(block) = sbc::exit_of_tag(&st.proxy, tag) {
                let e = out.entry(block).or_default();
                e.0 += up;
                e.1 += down;
            }
        }
        out
    }

    pub fn stop(&mut self) {
        self.shared.stopping.store(true, Ordering::SeqCst);
        guard::forget(&self.shared.id);
        {
            let mut owner = TUN_OWNER.lock().unwrap();
            if owner.as_deref() == Some(self.shared.id.as_str()) {
                *owner = None;
            }
        }
        let proc_ = self.child.lock().unwrap().take();
        if let Some(mut c) = proc_ {
            c.kill_and_wait();
        }
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        let _ = std::fs::remove_file(&self.config_path);
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        if !self.threads.is_empty() || self.child.lock().unwrap().is_some() {
            self.stop();
        }
    }
}

fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        return f.write_all(text.as_bytes());
    }
    #[cfg(not(unix))]
    std::fs::write(path, text) // contains passwords; the settings folder is the user's own
}

fn read_output(shared: &Shared, child: &Mutex<Option<Proc>>, lines: mpsc::Receiver<Option<String>>, pipes: usize, started: mpsc::Sender<Result<(), StartError>>) {
    let mut errors: Vec<String> = vec![];
    let mut is_started = false;
    let mut open_pipes = pipes;
    while open_pipes > 0 {
        let Ok(item) = lines.recv() else { break };
        let Some(raw) = item else {
            open_pipes -= 1;
            continue;
        };
        let line = ANSI.replace_all(&raw, "").trim_end().to_string();
        {
            let mut st = shared.state.lock().unwrap();
            st.raw_tail.push_back(line.clone());
            if st.raw_tail.len() > 30 {
                st.raw_tail.pop_front();
            }
        }
        let Some(c) = LINE.captures(&line) else { continue };
        let level = c.get(1).unwrap().as_str().to_string();
        let cid = c.get(2).map(|m| m.as_str().to_string());
        let msg = c.get(3).unwrap().as_str().to_string();
        if !is_started {
            if msg.starts_with("sing-box started") {
                is_started = true;
                let _ = started.send(Ok(()));
                continue;
            }
            if ["FATAL", "PANIC", "ERROR"].contains(&level.as_str()) {
                errors.push(msg.clone());
            }
        }
        handle(shared, &level, cid, &msg);
    }
    // the pipes closed: sing-box ended
    let code = child.lock().unwrap().as_mut().and_then(|c| c.exit_code());
    let code_text = code.map(|c| c.to_string()).unwrap_or_else(|| "?".into());
    if !is_started {
        let text = errors.last().cloned().unwrap_or_else(|| tr("sing-box exited (code {code})").replace("{code}", &code_text));
        let text = text.strip_prefix("start service: ").unwrap_or(&text).to_string();
        let _ = started.send(Err(start_error(&text)));
    } else if !shared.stopping.load(Ordering::SeqCst) {
        let mut text = tr("sing-box exited (code {code})").replace("{code}", &code_text);
        let panic = shared.state.lock().unwrap().raw_tail.iter().find(|x| x.starts_with("panic:") || x.starts_with("fatal error:")).cloned();
        if let Some(p) = panic {
            text = format!("{text}: {p}");
        }
        shared.log(Level::Error, &text);
        (shared.hooks.died)(text);
    }
}

/// One sing-box log line (level, connection id, message): connection lines, rejects, authentication failures,
/// failed outbound connections. (Python `SingBoxInstance._handle`.)
fn handle(shared: &Shared, level: &str, cid: Option<String>, msg: &str) {
    let mut cid = cid;
    if cid.is_none() && msg.starts_with("endpoint/") && (FROM.is_match(msg) || TO.is_match(msg)) {
        cid = Some("endpoint".into()); // a WireGuard server's lines have no connection id ("from" is followed by "to")
    }
    let Some(cid) = cid else {
        if ["WARN", "ERROR", "FATAL", "PANIC"].contains(&level) {
            let lv = if level == "WARN" { Level::Warn } else { Level::Error };
            shared.log(lv, &format!("sing-box: {}", hide_secrets(msg)));
        }
        return;
    };
    if let Some(c) = FROM.captures(msg) {
        let mut st = shared.state.lock().unwrap();
        st.clients.insert(cid, c[1].to_string());
        if st.clients.len() > 20000 {
            st.clients.clear();
            st.targets.clear();
        }
        return;
    }
    let who = shared.state.lock().unwrap().clients.get(&cid).cloned().unwrap_or_else(|| "?".into());
    if let Some(c) = TO.captures(msg) {
        let udp = c.get(1).is_some();
        let target = c[2].to_string();
        {
            let mut st = shared.state.lock().unwrap();
            st.targets.insert(cid, target.clone());
            st.stats.total += 1;
        }
        if udp {
            shared.log_conn(Level::Udp, &format!("{who} -> {target} UDP"));
        } else {
            shared.log_conn(Level::Tcp, &format!("{who} -> {target}"));
        }
        return;
    }
    let target = shared.state.lock().unwrap().targets.get(&cid).cloned().unwrap_or_else(|| "?".into());
    if let Some(c) = REJECT.captures(msg) {
        let idx: usize = c[1].parse().unwrap_or(usize::MAX);
        let reason = {
            let mut st = shared.state.lock().unwrap();
            st.stats.blocked += 1;
            st.last_reason = "denied".into();
            st.reasons.get(idx).cloned().flatten()
        };
        let text = match reason {
            Some(Reason::Allow) => Some(tr("{who} -> {target} not in the allow list").replace("{who}", &who).replace("{target}", &target)),
            Some(Reason::Private) => Some(format!("{who} -> {target} {}", tr("private address {host}").replace("{host}", &target))),
            Some(Reason::Banned) => Some(format!("{who} -> {target} {}", tr("blocked (temporarily banned IP)"))),
            Some(Reason::SelfPort) => Some(format!("{who} -> {target} {}", tr("blocked (this proxy's own port)"))),
            Some(Reason::Rule(name)) => {
                let name = if name == "default" { tr("default") } else { name };
                Some(format!("{who} -> {target} {}", tr("blocked by router rule '{name}'").replace("{name}", &name)))
            }
            None => None,
        };
        if let Some(t) = text {
            shared.log(Level::Blocked, &t);
        }
        return;
    }
    if let Some(c) = PROC_ERR.captures(msg) {
        let (client, err) = (c[1].to_string(), c[2].to_string());
        if err.contains("authentication failed") && !err.contains("no Proxy-Authorization header") {
            shared.state.lock().unwrap().stats.blocked += 1;
            match AUTH_USER.captures(&err) {
                Some(u) => shared.log(
                    Level::Auth,
                    &tr("{client} authentication failed (user {user!r})").replace("{client}", &client).replace("{user!r}", &py_repr(&u[1])),
                ),
                None => shared.log(Level::Auth, &format!("{client} {}", tr("authentication failed (wrong key or not Shadowsocks)"))),
            }
            if let Some(m) = guard::auth_failure(&ip_of(&client)) {
                shared.log(Level::Blocked, &m);
            }
        } else if !BENIGN.iter().any(|b| err.contains(b)) {
            shared.log_conn(Level::Fail, &format!("{client} {}", hide_secrets(&err)));
        }
        return;
    }
    if let Some(c) = OPEN_FAIL.captures(msg) {
        let (dest, err) = (c[1].to_string(), c[2].to_string());
        {
            let mut st = shared.state.lock().unwrap();
            st.stats.failed += 1;
            st.last_reason = if err.contains("authentication failed") { "auth".into() } else { "net".into() };
        }
        let hint = if err.contains("no such network interface") {
            format!(" - {}", tr("the chosen network interface does not exist (renamed or removed?)"))
        } else if err.contains("unreachable network") || err.contains("network is unreachable") {
            format!(" - {}", tr("the chosen network interface may be disconnected"))
        } else {
            String::new()
        };
        shared.log(Level::Fail, &format!("{who} -> {dest} {}{hint}", hide_secrets(&err)));
    }
}

fn poll(shared: &Shared, port: u16, secret: &str) {
    while !shared.stopping.load(Ordering::SeqCst) {
        // sleep in short steps so stop() does not wait a whole interval
        let until = Instant::now() + POLL_INTERVAL;
        while Instant::now() < until {
            if shared.stopping.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        guard::tick();
        let Ok(Some(d)) = api(port, secret, "GET", "/connections") else { continue };
        let conns: Vec<Value> = d["connections"].as_array().cloned().unwrap_or_default();
        let (idle, is_tun) = {
            let mut st = shared.state.lock().unwrap();
            let tcp = conns.iter().filter(|c| c["metadata"]["network"] == json!("tcp")).count() as u64;
            st.stats.tcp_active = tcp;
            st.stats.udp_active = conns.len() as u64 - tcp;
            let up = d["uploadTotal"].as_u64().unwrap_or(0);
            let down = d["downloadTotal"].as_u64().unwrap_or(0);
            let (lu, ld) = st.last_totals;
            let (du, dd) = (up.saturating_sub(lu), down.saturating_sub(ld));
            st.stats.up += du;
            st.stats.down += dd;
            st.last_totals = (up, down);
            let is_tun = st.proxy["inbound"]["type"] == json!("tun");
            let idle = track(&mut st, &conns, shared);
            attribute(&mut st, du as f64, dd as f64);
            (idle, is_tun)
        };
        let over = guard::report_connections(&shared.id, is_tun, &conns);
        if !over.is_empty() {
            shared.state.lock().unwrap().stats.blocked += over.len() as u64;
            shared.log(
                Level::Blocked,
                &tr("Closed {count} connections over the per-IP limit ({limit})")
                    .replace("{count}", &over.len().to_string())
                    .replace("{limit}", &guard::MAX_CONN_PER_IP.to_string()),
            );
        }
        for cid in over.iter().chain(idle.iter()) {
            let _ = api(port, secret, "DELETE", &format!("/connections/{cid}"));
        }
    }
}

/// Follow the open connections between polls: returns the TCP connections idle longer than the timeout (to
/// close) and logs a close line for connections that are gone (amounts as of the last poll).
fn track(st: &mut State, conns: &[Value], shared: &Shared) -> Vec<String> {
    let now = Instant::now();
    let timeout = Duration::from_secs(st.cfg["timeout"].as_u64().unwrap_or(120));
    let mut seen = HashSet::new();
    let mut idle = vec![];
    for c in conns {
        let Some(cid) = c["id"].as_str().filter(|s| !s.is_empty()) else { continue };
        seen.insert(cid.to_string());
        let up = c["upload"].as_u64().unwrap_or(0);
        let down = c["download"].as_u64().unwrap_or(0);
        let tag = c["chains"].as_array().and_then(|a| a.first()).and_then(Value::as_str).unwrap_or("").to_string();
        let (pu, pd) = st.live.get(cid).map(|e| (e.up, e.down)).unwrap_or((0, 0));
        let got = st.tick_tags.entry(tag).or_default();
        got.0 += up.saturating_sub(pu) as f64;
        got.1 += down.saturating_sub(pd) as f64;
        let e = st.live.entry(cid.to_string()).or_insert_with(|| {
            let md = &c["metadata"];
            let port = md["destinationPort"].as_str().and_then(|p| p.parse::<i64>().ok()).or_else(|| md["destinationPort"].as_i64()).unwrap_or(0);
            let src_port = md["sourcePort"].as_str().map(String::from).or_else(|| md["sourcePort"].as_i64().map(|p| p.to_string())).unwrap_or("?".into());
            let src = md["sourceIP"].as_str().unwrap_or("?");
            let host = md["host"].as_str().filter(|h| !h.is_empty()).or(md["destinationIP"].as_str()).unwrap_or("?");
            Live {
                who: if src.contains(':') { format!("[{src}]:{src_port}") } else { format!("{src}:{src_port}") },
                target: host_port(host, port),
                udp: md["network"] == json!("udp"),
                up,
                down,
                start: started_at(c["start"].as_str().unwrap_or("")),
                change: now,
                reason: None,
            }
        });
        if (up, down) != (e.up, e.down) {
            e.up = up;
            e.down = down;
            e.change = now;
        }
        if !e.udp && e.reason.is_none() && now.duration_since(e.change) >= timeout {
            e.reason = Some(tr("idle timeout"));
            idle.push(cid.to_string());
        }
    }
    let gone: Vec<String> = st.live.keys().filter(|k| !seen.contains(*k)).cloned().collect();
    for cid in gone {
        let e = st.live.remove(&cid).unwrap();
        closed(st, &e, shared);
    }
    idle
}

fn closed(st: &State, e: &Live, shared: &Shared) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
    let secs = (now - e.start).max(0.0);
    let log_close = st.cfg["log_close"].as_bool().unwrap_or(false);
    let log_conn = st.cfg["log_connections"].as_bool().unwrap_or(true);
    let text = if e.udp {
        (log_close || log_conn).then(|| {
            (Level::Udp, tr("{who} UDP session ended · ↑ {fmt_size} ↓ {fmt_size2} · {value:.1f}s{extra}")
                .replace("{who}", &e.who)
                .replace("{fmt_size}", &fmt_size(e.up as f64))
                .replace("{fmt_size2}", &fmt_size(e.down as f64))
                .replace("{value:.1f}", &format!("{secs:.1}"))
                .replace("{extra}", ""))
        })
    } else {
        log_close.then(|| {
            (Level::Tcp, tr("{who} -> {target} closed · {reason} · ↑ {fmt_size} ↓ {fmt_size2} · {value:.1f}s")
                .replace("{who}", &e.who)
                .replace("{target}", &e.target)
                .replace("{reason}", &e.reason.clone().unwrap_or_else(|| tr("normal close")))
                .replace("{fmt_size}", &fmt_size(e.up as f64))
                .replace("{fmt_size2}", &fmt_size(e.down as f64))
                .replace("{value:.1f}", &format!("{secs:.1}")))
        })
    };
    if let Some((level, t)) = text {
        // (logged without the state lock: the hook may call back into the owner)
        let message = crate::log::clean(&t);
        (shared.hooks.log)(level, &message);
    }
}

/// Add this poll's bytes (the card's exact totals du / dd) to tag_bytes. The Clash API lists only open
/// connections: a card without router rules has one exit and gets the whole amount; with rules, the unseen rest is
/// split like the seen bytes of this poll (or goes to the default exit when none was seen).
fn attribute(st: &mut State, du: f64, dd: f64) {
    let mut tick = std::mem::take(&mut st.tick_tags);
    if sbc::route_exit_tags(&st.proxy).is_empty() {
        tick = HashMap::from([("out0".to_string(), (du, dd))]);
    } else {
        let su: f64 = tick.values().map(|v| v.0).sum();
        let sd: f64 = tick.values().map(|v| v.1).sum();
        let (ru, rd) = ((du - su).max(0.0), (dd - sd).max(0.0));
        if ru > 0.0 || rd > 0.0 {
            let mut weights: Vec<(String, f64)> = tick.iter().filter(|(_, v)| v.0 + v.1 > 0.0).map(|(t, v)| (t.clone(), v.0 + v.1)).collect();
            let mut total: f64 = weights.iter().map(|w| w.1).sum();
            if total == 0.0 {
                weights = vec![("out0".into(), 1.0)];
                total = 1.0;
            }
            for (t, w) in weights {
                let got = tick.entry(t).or_default();
                got.0 += ru * w / total;
                got.1 += rd * w / total;
            }
        }
    }
    for (t, (u, d)) in tick {
        let got = st.tag_bytes.entry(t).or_default();
        got.0 += u;
        got.1 += d;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_patterns() {
        let c = LINE.captures("INFO[0001] [3245104543 0ms] inbound/socks[in]: inbound connection from 127.0.0.1:51000").unwrap();
        assert_eq!(&c[1], "INFO");
        assert_eq!(&c[2], "3245104543");
        assert_eq!(FROM.captures(&c[3]).unwrap()[1].to_string(), "127.0.0.1:51000");
        assert!(TO.is_match("inbound/socks[in]: inbound packet connection to 1.1.1.1:53"));
        assert!(REJECT.is_match("router: match[3] => reject"));
        let e = PROC_ERR.captures("inbound/socks[in]: process connection from 10.0.0.9:5000: socks5: authentication failed, username=bob, password=x").unwrap();
        assert_eq!(&e[1], "10.0.0.9:5000");
        assert_eq!(&AUTH_USER.captures(&e[2]).unwrap()[1], "bob");
        assert_eq!(hide_secrets("a password=hunter2 b"), "a password=*** b");
        assert_eq!(py_repr("bob"), "'bob'");
        assert_eq!(py_repr("o'k"), "\"o'k\"");
    }

    #[test]
    fn start_errors() {
        assert!(matches!(start_error("listen tcp 127.0.0.1:1080: bind: Only one usage of each socket address"), StartError::PortInUse(_)));
        assert!(matches!(start_error("decode config: password=abc bad"), StartError::Other(t) if t == "sing-box: decode config: password=*** bad"));
    }

    #[test]
    fn clash_start_times() {
        assert_eq!(started_at("2026-11-24T00:00:00Z"), 1795478400.0);
        assert_eq!(started_at("2026-11-24T09:00:00.5+09:00"), 1795478400.5);
    }
}
