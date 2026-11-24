//! Settings file (Python: `proxydock/config.py`): defaults, the lenient type check every field goes through,
//! global settings validation and reading `config.json` (v3: `{"version", "global", "graph"}`).
//!
//! Values are kept as JSON maps for now, in the same shape as the Python app, so both apps read and write the
//! same file while the port is going on.

use crate::net::{is_ip_literal, split_host_port, valid_domain};
use crate::secret::{self, Undecryptable};
use serde_json::{json, Map, Value};

pub const LANGUAGE_CODES: [&str; 8] = ["en", "ko", "ja", "zh_CN", "zh_TW", "de", "fr", "es"];
pub const THEMES: [&str; 3] = ["auto", "light", "dark"];
pub const UI_SCALES: [i64; 5] = [90, 100, 110, 125, 150];
pub const TEXT_SIZES: [i64; 4] = [100, 115, 130, 150];
pub const ENGINES: [&str; 2] = ["sing-box", "python"];
pub const MAX_TEST_SITES: usize = 30;
pub const OVERRIDABLE: [&str; 7] =
    ["allowed_domains", "block_private", "timeout", "log_connections", "log_close", "dns_server", "dns_via_outbound"];
pub const VMESS_CIPHERS: [&str; 5] = ["auto", "aes-128-gcm", "chacha20-poly1305", "none", "zero"];
pub const INBOUND_TYPES: [&str; 9] =
    ["socks5", "socks4", "http", "https", "shadowsocks", "hysteria2", "wireguard", "forward", "tun"];
pub const OUTBOUND_TYPES: [&str; 15] = [
    "direct", "http", "https", "socks4", "socks5", "shadowsocks", "hysteria2", "wireguard", "vless", "vmess", "trojan",
    "tuic", "subscription", "group", "block",
];
pub const TUN_STACKS: [&str; 3] = ["mixed", "system", "gvisor"];
pub const SS_METHODS: [&str; 6] = [
    "2022-blake3-aes-128-gcm", "2022-blake3-aes-256-gcm", "2022-blake3-chacha20-poly1305", "chacha20-ietf-poly1305",
    "aes-256-gcm", "aes-128-gcm",
];

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

pub fn global_defaults() -> Map<String, Value> {
    obj(json!({
        "language": "en", "theme": "auto", "ui_scale": 100, "text_size": 115, "ui_font": "", "mono_font": "",
        "disable_ligatures": false, "log_font_size": 10, "close_exits": false, "log_to_file": false,
        "engine": "sing-box", "run_as_admin": false, "singbox_path": "", "hysteria_path": "", "test_sites": [],
        "check_updates": true, "update_checked": 0,
        "allowed_domains": ["*"], "block_private": false, "timeout": 120, "log_connections": true,
        "log_close": false, "dns_server": "", "dns_via_outbound": false,
    }))
}

pub fn inbound_defaults() -> Map<String, Value> {
    obj(json!({
        "type": "socks5", "host": "127.0.0.1", "port": 30307, "auth_enabled": false, "username": "", "password": "",
        "tls_cert": "", "tls_key": "", "ss_method": "2022-blake3-aes-256-gcm", "ss_password": "", "hy2_password": "",
        "hy2_obfs_password": "", "hy2_up_mbps": 0, "hy2_down_mbps": 0, "tun_stack": "mixed",
        "tun_strict_route": false, "tun_fakeip": false, "wg_private_key": "", "wg_address": "10.66.0.1/24",
        "wg_peers": "", "wg_endpoint": "", "wg_mtu": 1408, "fwd_host": "", "fwd_port": 0, "fwd_network": "tcp",
    }))
}

pub fn outbound_defaults() -> Map<String, Value> {
    obj(json!({
        "type": "direct", "host": "", "port": 0, "username": "", "password": "", "tls_verify": true, "sni": "",
        "pin_sha256": "", "ss_method": "2022-blake3-aes-256-gcm", "ss_password": "", "hy2_password": "",
        "hy2_obfs_password": "", "hy2_up_mbps": 0, "hy2_down_mbps": 0, "hy2_ports": "", "hy2_hop_interval": 30,
        "bind": "", "interface": "", "wg_private_key": "", "wg_address": "", "wg_peer_public_key": "",
        "wg_preshared_key": "", "wg_allowed_ips": "0.0.0.0/0, ::/0", "wg_mtu": 1408, "wg_keepalive": 0,
        "uuid": "", "security": "tls", "vless_flow": "", "vmess_cipher": "auto", "vmess_alter_id": 0,
        "reality_public_key": "", "reality_short_id": "", "fingerprint": "chrome", "alpn": "", "transport": "tcp",
        "transport_path": "", "transport_host": "", "tuic_congestion": "bbr", "tuic_udp_relay": "native",
        "sub_url": "", "sub_servers": "", "sub_mode": "auto", "sub_selected": "", "sub_interval": 24,
        "sub_updated": 0, "sub_test_url": "https://www.gstatic.com/generate_204",
        "group_mode": "auto", "group_test_url": "https://www.gstatic.com/generate_204", "group_interval": 180,
        "group_tolerance": 50, "group_selected": "",
    }))
}

/// Python `isinstance(v, int) and not isinstance(v, bool)`: a JSON integer (5, not 5.0)
fn is_int(v: &Value) -> bool {
    matches!(v, Value::Number(n) if n.is_i64() || n.is_u64())
}

/// Every key of `defaults`, taking the value from `raw` only when it has the default's type (bool / int / str /
/// list of str); list items are trimmed and empty ones dropped. Keys `raw` has beyond the defaults are dropped.
/// (Python `config._typed`.)
pub fn typed(defaults: &Map<String, Value>, raw: &Value) -> Map<String, Value> {
    let mut out = defaults.clone();
    let Value::Object(raw) = raw else { return out };
    for (key, default) in defaults {
        let Some(v) = raw.get(key) else { continue };
        let ok = match default {
            Value::Bool(_) => v.is_boolean(),
            Value::Number(_) => is_int(v),
            Value::Array(_) => v.as_array().is_some_and(|a| a.iter().all(Value::is_string)),
            _ => v.is_string(),
        };
        if !ok {
            continue;
        }
        let v = match v {
            Value::Array(a) => Value::Array(
                a.iter()
                    .filter_map(|x| {
                        let t = x.as_str().unwrap().trim();
                        (!t.is_empty()).then(|| Value::String(t.to_string()))
                    })
                    .collect(),
            ),
            other => other.clone(),
        };
        out.insert(key.clone(), v);
    }
    out
}

const DNS_DEFAULT_PORTS: [(&str, u16); 6] =
    [("udp", 53), ("tcp", 53), ("tls", 853), ("https", 443), ("quic", 853), ("h3", 443)];

/// '1.1.1.1', 'tls://dns.google', 'https://cloudflare-dns.com/dns-query', ... -> sing-box DNS server fields.
/// Some(empty map) = the PC's DNS, None = not valid. (Python `config.dns_server_spec`.)
pub fn dns_server_spec(text: &str) -> Option<Map<String, Value>> {
    let t = text.trim();
    if t.is_empty() {
        return Some(Map::new());
    }
    let (scheme, rest) = match t.split_once("://") {
        Some((s, r)) => (s.to_lowercase(), r),
        None => ("udp".to_string(), t),
    };
    let default_port = DNS_DEFAULT_PORTS.iter().find(|(s, _)| *s == scheme)?.1;
    let (hostport, path) = match rest.split_once('/') {
        Some((h, p)) => (h, Some(p)),
        None => (rest, None),
    };
    let (host, port) = split_host_port(hostport, Some(default_port)).ok()?;
    let host = host.trim().to_lowercase();
    if host.is_empty() || !(is_ip_literal(&host) || valid_domain(&host)) || host.contains(['?', '#', '@']) {
        return None;
    }
    let mut d = Map::new();
    d.insert("type".into(), json!(scheme));
    d.insert("server".into(), json!(host));
    if port != default_port {
        d.insert("server_port".into(), json!(port));
    }
    let path = path.filter(|p| !p.is_empty());
    if scheme == "https" || scheme == "h3" {
        d.insert("path".into(), json!(path.map(|p| format!("/{p}")).unwrap_or_else(|| "/dns-query".into())));
    } else if path.is_some() {
        return None;
    }
    Some(d)
}

/// Host name for the connection test from what the user typed ('https://Example.com/x' -> 'example.com'), or
/// None if it is no usable host. (Python `config.test_site_host`.)
pub fn test_site_host(text: &str) -> Option<String> {
    let mut t = text.trim().to_lowercase();
    // drop a scheme: [a-z][a-z0-9+.-]*://
    if let Some(i) = t.find("://") {
        let s = &t[..i];
        let mut cs = s.chars();
        if cs.next().is_some_and(|c| c.is_ascii_lowercase())
            && cs.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+.-".contains(c))
        {
            t = t[i + 3..].to_string();
        }
    }
    let mut t = t.split('/').next().unwrap_or("").split('?').next().unwrap_or("").to_string();
    if t.matches(':').count() == 1 {
        t = t.split(':').next().unwrap().to_string();
    }
    let t = t.trim_matches('.').to_string();
    if t.is_empty() || t.chars().count() > 253 || !t.contains('.') {
        return None;
    }
    // Python: t.encode("idna") must work (no empty labels, labels up to 63) and [\w.-]+ must match the whole host
    if t.split('.').any(|l| l.is_empty() || l.chars().count() > 63) {
        return None;
    }
    if !t.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.' || c == '-') {
        return None;
    }
    Some(t)
}

fn clamp_int(m: &mut Map<String, Value>, key: &str, lo: i64, hi: i64) {
    if let Some(v) = m.get(key).and_then(Value::as_i64) {
        m.insert(key.into(), json!(v.clamp(lo, hi)));
    }
}

/// Global settings (Python `config.validate_global`).
pub fn validate_global(raw: &Value) -> Map<String, Value> {
    let defaults = global_defaults();
    let mut g = typed(&defaults, raw);
    let s = |g: &Map<String, Value>, k: &str| g[k].as_str().unwrap_or("").to_string();
    if !LANGUAGE_CODES.contains(&s(&g, "language").as_str()) {
        g.insert("language".into(), json!("en"));
    }
    if !THEMES.contains(&s(&g, "theme").as_str()) {
        g.insert("theme".into(), json!("auto"));
    }
    if !UI_SCALES.contains(&g["ui_scale"].as_i64().unwrap_or(0)) {
        g.insert("ui_scale".into(), json!(100));
    }
    if !TEXT_SIZES.contains(&g["text_size"].as_i64().unwrap_or(0)) {
        g.insert("text_size".into(), defaults["text_size"].clone());
    }
    clamp_int(&mut g, "log_font_size", 8, 20);
    let mut hosts: Vec<String> = vec![];
    for x in g["test_sites"].as_array().unwrap().iter().filter_map(Value::as_str) {
        if let Some(h) = test_site_host(x) {
            if !hosts.contains(&h) {
                hosts.push(h);
            }
        }
    }
    hosts.truncate(MAX_TEST_SITES);
    g.insert("test_sites".into(), json!(hosts));
    clamp_int(&mut g, "timeout", 10, 86400);
    if g["allowed_domains"].as_array().unwrap().is_empty() {
        g.insert("allowed_domains".into(), json!(["*"]));
    }
    if !ENGINES.contains(&s(&g, "engine").as_str()) {
        g.insert("engine".into(), json!("sing-box"));
    }
    if dns_server_spec(&s(&g, "dns_server")).is_none() {
        g.insert("dns_server".into(), json!(""));
    }
    g
}

/// The inbound settings of an inbound block (Python: the inbound part of `config.validate_proxy`).
pub fn validate_inbound(raw: &Value) -> Map<String, Value> {
    let defaults = inbound_defaults();
    let mut ib = typed(&defaults, raw);
    if !INBOUND_TYPES.contains(&ib["type"].as_str().unwrap()) {
        ib.insert("type".into(), json!("socks5"));
    }
    if !SS_METHODS.contains(&ib["ss_method"].as_str().unwrap()) {
        ib.insert("ss_method".into(), defaults["ss_method"].clone());
    }
    if !(1..=65535).contains(&ib["port"].as_i64().unwrap_or(0)) {
        ib.insert("port".into(), defaults["port"].clone());
    }
    if !TUN_STACKS.contains(&ib["tun_stack"].as_str().unwrap()) {
        ib.insert("tun_stack".into(), json!("mixed"));
    }
    ib
}

/// The outbound settings of an outbound block (Python: the outbound part of `graph.validate_outbound_node`).
pub fn validate_outbound(raw: &Value) -> Map<String, Value> {
    let defaults = outbound_defaults();
    let mut ob = typed(&defaults, raw);
    if !OUTBOUND_TYPES.contains(&ob["type"].as_str().unwrap()) {
        ob.insert("type".into(), json!("direct"));
    }
    if !SS_METHODS.contains(&ob["ss_method"].as_str().unwrap()) {
        ob.insert("ss_method".into(), defaults["ss_method"].clone());
    }
    if !(0..=65535).contains(&ob["port"].as_i64().unwrap_or(-1)) {
        ob.insert("port".into(), json!(0));
    }
    ob
}

/// What reading the settings file found besides the settings themselves.
#[derive(Debug, Default)]
pub struct LoadNotes {
    /// a secret was stored unencrypted, or the file is the old v2 format: save it again
    pub needs_save: bool,
    /// secrets that could not be decrypted (block id, field)
    pub undecryptable: Vec<(String, Undecryptable)>,
}

#[derive(Debug)]
pub enum LoadError {
    Missing,
    Unreadable(String),
    /// settings v2 (one inbound + outbound per proxy): not handled here yet
    OldFormat,
}

/// Settings read from `config.json`: validated global settings and the graph with the inbound / outbound settings
/// of each block decrypted and validated. The rest of each block is kept as written (graph validation comes with
/// the graph port).
#[derive(Debug)]
pub struct Loaded {
    pub global: Map<String, Value>,
    pub graph: Map<String, Value>,
    pub notes: LoadNotes,
}

pub fn parse(text: &str) -> Result<Loaded, LoadError> {
    let raw: Value = serde_json::from_str(text).map_err(|e| LoadError::Unreadable(e.to_string()))?;
    let empty = Map::new();
    let raw = raw.as_object().unwrap_or(&empty);
    if raw.get("version").and_then(Value::as_i64).unwrap_or(2) < 3 {
        return Err(LoadError::OldFormat);
    }
    let global = validate_global(raw.get("global").unwrap_or(&Value::Null));
    let mut graph = raw.get("graph").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut notes = LoadNotes::default();
    for (key, field) in [("inbounds", "inbound"), ("outbounds", "outbound")] {
        let items: Vec<Value> = graph
            .get(key)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter(|x| x.is_object()).cloned().collect())
            .unwrap_or_default();
        let items = items
            .into_iter()
            .map(|mut item| {
                let node = item.as_object_mut().unwrap();
                let id = node.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let (plain, legacy, failed) = secret::unseal(node.get(field).unwrap_or(&Value::Null));
                notes.needs_save |= legacy;
                notes.undecryptable.extend(failed.into_iter().map(|f| (id.clone(), f)));
                let settings = if field == "inbound" { validate_inbound(&plain) } else { validate_outbound(&plain) };
                node.insert(field.into(), Value::Object(settings));
                item
            })
            .collect();
        graph.insert(key.into(), Value::Array(items));
    }
    Ok(Loaded { global, graph, notes })
}

pub fn load_from(path: &std::path::Path) -> Result<Loaded, LoadError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(LoadError::Missing),
        Err(e) => Err(LoadError::Unreadable(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_keeps_only_values_of_the_right_type() {
        let d = inbound_defaults();
        let out = typed(&d, &json!({"port": "80", "auth_enabled": 1, "host": "0.0.0.0", "extra": true, "wg_mtu": 1.5}));
        assert_eq!(out["port"], json!(30307));
        assert_eq!(out["auth_enabled"], json!(false));
        assert_eq!(out["host"], json!("0.0.0.0"));
        assert_eq!(out["wg_mtu"], json!(1408));
        assert!(out.get("extra").is_none());
        assert_eq!(out.keys().next().unwrap(), "type");
        let g = typed(&global_defaults(), &json!({"allowed_domains": [" a.com ", "", "b"]}));
        assert_eq!(g["allowed_domains"], json!(["a.com", "b"]));
    }

    #[test]
    fn dns_specs() {
        assert_eq!(dns_server_spec("").unwrap(), Map::new());
        assert_eq!(Value::Object(dns_server_spec("1.1.1.1").unwrap()), json!({"type": "udp", "server": "1.1.1.1"}));
        assert_eq!(
            Value::Object(dns_server_spec("tls://dns.google:8853").unwrap()),
            json!({"type": "tls", "server": "dns.google", "server_port": 8853})
        );
        assert_eq!(
            Value::Object(dns_server_spec("HTTPS://Cloudflare-DNS.com").unwrap()),
            json!({"type": "https", "server": "cloudflare-dns.com", "path": "/dns-query"})
        );
        assert_eq!(
            Value::Object(dns_server_spec("h3://dns.x/q").unwrap()),
            json!({"type": "h3", "server": "dns.x", "path": "/q"})
        );
        assert!(dns_server_spec("tls://dns.google/x").is_none());
        assert!(dns_server_spec("ftp://x").is_none());
        assert!(dns_server_spec("a@b").is_none());
    }

    #[test]
    fn test_sites() {
        assert_eq!(test_site_host("https://Example.com/x?y").as_deref(), Some("example.com"));
        assert_eq!(test_site_host("example.com:8443").as_deref(), Some("example.com"));
        assert_eq!(test_site_host("한국.kr").as_deref(), Some("한국.kr"));
        assert_eq!(test_site_host("localhost"), None);
        assert_eq!(test_site_host("a..b"), None);
        assert_eq!(test_site_host("a b.com"), None);
    }

    #[test]
    fn global_rules() {
        let g = validate_global(&json!({"language": "xx", "timeout": 5, "log_font_size": 99,
            "test_sites": ["a.com", "A.com", "bad"], "allowed_domains": [], "dns_server": "ftp://x"}));
        assert_eq!(g["language"], json!("en"));
        assert_eq!(g["timeout"], json!(10));
        assert_eq!(g["log_font_size"], json!(20));
        assert_eq!(g["test_sites"], json!(["a.com"]));
        assert_eq!(g["allowed_domains"], json!(["*"]));
        assert_eq!(g["dns_server"], json!(""));
    }
}
