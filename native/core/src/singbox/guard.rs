//! Protections done from outside sing-box for all sing-box cards together (Python: `singbox/guard.py` and the
//! ban rules of `common.note_auth_failure`):
//! - temporary IP bans after repeated authentication failures, written to a shared local rule-set file that every
//!   sing-box process rejects (sing-box reloads it on change)
//! - the per-IP concurrent connection limit, enforced by closing the newest connections over the limit through
//!   each process's Clash API

use crate::net::strip_scope;
use serde_json::{Value, json};
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// max concurrent connections from one IP (loopback excluded), over all cards
pub const MAX_CONN_PER_IP: usize = 256;
/// this many authentication failures ...
pub const AUTH_FAIL_LIMIT: usize = 10;
/// ... within this many seconds ...
pub const AUTH_FAIL_WINDOW: Duration = Duration::from_secs(60);
/// ... ban that IP for this long
pub const BAN_SECONDS: u64 = 300;

static RUN_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Where the cards' sing-box configs and the shared bans file live (default: <settings folder>/run).
pub fn run_dir() -> PathBuf {
    RUN_DIR.lock().unwrap().clone().unwrap_or_else(|| crate::paths::app_dir().join("run"))
}

/// use another run folder (tests, a portable copy)
pub fn set_run_dir(dir: PathBuf) {
    *RUN_DIR.lock().unwrap() = Some(dir);
}

pub fn bans_path() -> PathBuf {
    run_dir().join("bans.json")
}

#[derive(Default)]
struct State {
    auth_fails: HashMap<String, VecDeque<Instant>>,
    banned: HashMap<String, Instant>,
    /// ban set last written to the bans file
    written: Option<Vec<String>>,
    /// card id -> {source ip: [(start, connection id)]}
    conns: HashMap<String, HashMap<String, Vec<(String, String)>>>,
    last_tick: Option<Instant>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut g = STATE.lock().unwrap();
    f(g.get_or_insert_with(State::default))
}

pub fn is_loopback_ip(ip: &str) -> bool {
    strip_scope(ip).parse::<IpAddr>().is_ok_and(|a| a.is_loopback())
}

fn is_banned_in(s: &mut State, ip: &str) -> bool {
    match s.banned.get(ip) {
        None => false,
        Some(until) if Instant::now() >= *until => {
            s.banned.remove(ip);
            false
        }
        Some(_) => true,
    }
}

pub fn is_banned(ip: &str) -> bool {
    with(|s| is_banned_in(s, ip))
}

/// the banned IPs (sorted, expired ones dropped)
pub fn current_bans() -> Vec<String> {
    with(|s| {
        let ips: Vec<String> = s.banned.keys().cloned().collect();
        let mut out: Vec<String> = ips.into_iter().filter(|ip| is_banned_in(s, ip)).collect();
        out.sort();
        out
    })
}

/// Contents of the shared rule-set file with the banned client IPs (Python `compile.bans_ruleset`).
pub fn bans_ruleset(ips: &[String]) -> Value {
    let mut sorted = ips.to_vec();
    sorted.sort();
    let mut cidrs: Vec<String> = sorted
        .iter()
        .filter_map(|ip| strip_scope(ip).parse::<IpAddr>().ok())
        .map(|a| format!("{a}/{}", if a.is_ipv4() { 32 } else { 128 }))
        .collect();
    if cidrs.is_empty() {
        // a rule-set rule needs at least one item; 255.255.255.255 is never a client address
        cidrs.push("255.255.255.255/32".into());
    }
    json!({"version": 3, "rules": [{"source_ip_cidr": cidrs}]})
}

/// Write the ban list if it changed. force: check the file even if this process already wrote the same set (a card
/// is starting). False if the file could not be replaced (Windows: a running sing-box has it open right now).
pub fn write_bans(force: bool) -> bool {
    let bans = current_bans();
    if !force && with(|s| s.written.as_ref() == Some(&bans)) {
        return true;
    }
    let data = bans_ruleset(&bans).to_string();
    let path = bans_path();
    let _ = std::fs::create_dir_all(run_dir());
    if std::fs::read_to_string(&path).is_ok_and(|old| old == data) {
        // unchanged: do not touch a file other sing-box processes are reading
        with(|s| s.written = Some(bans));
        return true;
    }
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, &data).is_err() {
        return false;
    }
    for i in 0..5 {
        match std::fs::rename(&tmp, &path) {
            Ok(()) => {
                with(|s| s.written = Some(bans));
                return true;
            }
            Err(_) if i < 4 => std::thread::sleep(Duration::from_millis(50 * (i + 1))),
            Err(_) => return false,
        }
    }
    false
}

/// Record an authentication failure from ip. Returns the log message when this failure gets the IP banned.
pub fn auth_failure(ip: &str) -> Option<String> {
    if ip.is_empty() || is_loopback_ip(ip) {
        return None;
    }
    let banned_now = with(|s| {
        let now = Instant::now();
        let q = s.auth_fails.entry(ip.to_string()).or_default();
        q.push_back(now);
        while q.front().is_some_and(|t| now.duration_since(*t) > AUTH_FAIL_WINDOW) {
            q.pop_front();
        }
        let hit = q.len() >= AUTH_FAIL_LIMIT;
        if hit {
            q.clear();
            s.banned.insert(ip.to_string(), now + Duration::from_secs(BAN_SECONDS));
        }
        if s.auth_fails.len() > 10000 {
            s.auth_fails.clear();
        }
        hit
    });
    if !banned_now {
        return None;
    }
    write_bans(false);
    Some(
        crate::i18n::tr("Blocking {ip} for {value} min after repeated authentication failures")
            .replace("{ip}", ip)
            .replace("{value}", &(BAN_SECONDS / 60).to_string()),
    )
}

/// conns: the Clash API connection list of one card. Returns connection ids to close (over the per-IP limit).
pub fn report_connections(card: &str, is_tun: bool, conns: &[Value]) -> Vec<String> {
    with(|s| {
        if is_tun {
            // every app of this PC shares the TUN address: no per-IP limit
            s.conns.remove(card);
            return vec![];
        }
        let mut by_ip: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for c in conns {
            let ip = c["metadata"]["sourceIP"].as_str().unwrap_or("");
            if !ip.is_empty() && !is_loopback_ip(ip) {
                let start = c["start"].as_str().unwrap_or("").to_string();
                let id = c["id"].as_str().unwrap_or("").to_string();
                by_ip.entry(ip.to_string()).or_default().push((start, id));
            }
        }
        s.conns.insert(card.to_string(), by_ip.clone());
        let mut close = vec![];
        for (ip, mut mine) in by_ip {
            let total: usize = s.conns.values().map(|other| other.get(&ip).map_or(0, Vec::len)).sum();
            if total > MAX_CONN_PER_IP {
                mine.sort();
                let n = (total - MAX_CONN_PER_IP).min(mine.len());
                close.extend(mine[mine.len() - n..].iter().map(|(_, id)| id.clone()));
            }
        }
        close
    })
}

pub fn forget(card: &str) {
    with(|s| {
        s.conns.remove(card);
    });
}

/// called about once a second by the running cards: expired bans drop out of the file
pub fn tick() {
    let due = with(|s| {
        let now = Instant::now();
        let due = s.last_tick.is_none_or(|t| now.duration_since(t) >= Duration::from_secs(1));
        if due {
            s.last_tick = Some(now);
        }
        due
    });
    if due {
        write_bans(false);
    }
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *STATE.lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruleset_shape() {
        assert_eq!(bans_ruleset(&[]), json!({"version": 3, "rules": [{"source_ip_cidr": ["255.255.255.255/32"]}]}));
        assert_eq!(
            bans_ruleset(&["10.0.0.2".into(), "::1".into()])["rules"][0]["source_ip_cidr"],
            json!(["10.0.0.2/32", "::1/128"])
        );
    }

    #[test]
    fn per_ip_limit_counts_all_cards() {
        reset_for_tests();
        let conn = |i: usize| json!({"id": format!("c{i}"), "start": format!("2026-01-01T00:00:{:02}Z", i % 60), "metadata": {"sourceIP": "192.0.2.7"}});
        let a: Vec<Value> = (0..200).map(conn).collect();
        let b: Vec<Value> = (200..260).map(conn).collect();
        assert!(report_connections("a", false, &a).is_empty());
        assert_eq!(report_connections("b", false, &b).len(), 4);
        let lo: Vec<Value> =
            (0..300).map(|i| json!({"id": format!("l{i}"), "metadata": {"sourceIP": "127.0.0.1"}})).collect();
        assert!(report_connections("c", false, &lo).is_empty());
    }
}
