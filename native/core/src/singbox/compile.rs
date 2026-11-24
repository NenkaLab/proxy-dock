//! ProxyDock proxy (`graph::compile()` output) -> sing-box config (Python: `singbox/compile.py`). Pure
//! functions, no I/O (certificate and rule-set paths come in).

use crate::config::dns_server_spec;
use crate::net::{is_ip_literal, strip_scope};
use crate::secret::truthy;
use serde_json::{json, Map, Value};
use std::net::IpAddr;

pub const IN_TAG: &str = "in";
pub const BANS_TAG: &str = "bans";
pub const TUN_DIRECT: &str = "tun-direct";
pub const TUN_ADDRESS: [&str; 2] = ["172.19.0.1/30", "fdfe:dcba:9876::1/126"];
pub const DNS_TAG: &str = "dns";
pub const LOCAL_DNS: &str = "local";
pub const FAKEIP_TAG: &str = "fakeip";
pub const FAKEIP_V4: &str = "198.18.0.0/15";
pub const FAKEIP_V6: &str = "fc00::/18";
/// ms: urltest never trades the current member for a faster one -> member order wins
pub const FAILOVER_TOLERANCE: i64 = 30000;
/// router conditions that need the destination IP
pub const IP_KEYS: [&str; 3] = ["ip_cidr", "ip_private", "geoip"];
/// internal ranges sing-box's ip_is_private does not cover
const EXTRA_INTERNAL: [&str; 9] = [
    "0.0.0.0/8", "100.64.0.0/10", "192.0.0.0/24", "198.18.0.0/15", "224.0.0.0/4", "240.0.0.0/4", "ff00::/8",
    "64:ff9b:1::/48", "2001:db8::/32",
];
const TEST_URL: &str = "https://www.gstatic.com/generate_204";

/// Why a config cannot be built. `Missing` and `Unsupported` are what a subscription server may have (that server
/// is skipped, Python KeyError / ValueError); anything else stops the build.
#[derive(Debug, Clone, PartialEq)]
pub enum BuildError {
    /// a required field is not there
    Missing(String),
    /// a type ProxyDock cannot turn into sing-box, or a value it cannot read
    Unsupported(String),
}

type R<T> = Result<T, BuildError>;
type Obj = Map<String, Value>;

/// What route rule i blocks: used to explain sing-box "router: match[n] => reject" log lines.
#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    Banned,
    SelfPort,
    Allow,
    Private,
    /// a router rule (or the router default) that sends traffic to the Block outbound
    Rule(String),
}

impl Reason {
    /// the shape the Python app uses: "banned" / "self" / "allow" / "private" / ["rule", name]
    pub fn to_json(r: &Option<Reason>) -> Value {
        match r {
            None => Value::Null,
            Some(Reason::Banned) => json!("banned"),
            Some(Reason::SelfPort) => json!("self"),
            Some(Reason::Allow) => json!("allow"),
            Some(Reason::Private) => json!("private"),
            Some(Reason::Rule(n)) => json!(["rule", n]),
        }
    }
}

fn get<'a>(o: &'a Value, k: &str) -> R<&'a Value> {
    o.get(k).ok_or_else(|| BuildError::Missing(k.into()))
}

fn opt<'a>(o: &'a Value, k: &str) -> Option<&'a Value> {
    o.get(k).filter(|v| truthy(v))
}

fn st(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

/// Python `str(v)` for the values a setting can hold
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "True".into() } else { "False".into() },
        Value::Null => "None".into(),
        Value::Number(n) if n.is_f64() => {
            let s = format!("{:?}", n.as_f64().unwrap());
            match s.split_once('e') {
                Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
                _ => s,
            }
        }
        other => other.to_string(),
    }
}

fn py_int(s: &str) -> R<i64> {
    crate::py::int(s).ok_or_else(|| BuildError::Unsupported(format!("not a number: {s}")))
}

fn strip(s: &str) -> String {
    crate::py::strip(s).to_string()
}

/// "a, b; c" -> ["a", "b", "c"] (Python `wgconf.split`)
pub fn split(text: &str) -> Vec<String> {
    text.replace(';', ",").split(',').map(crate::py::strip).filter(|x| !x.is_empty()).map(String::from).collect()
}

fn host_cidr(x: &str) -> R<String> {
    let a: IpAddr = strip_scope(x).parse().map_err(|_| BuildError::Unsupported(format!("not an IP: {x}")))?;
    Ok(format!("{a}/{}", if a.is_ipv4() { 32 } else { 128 }))
}

/// Python `ipaddress.ip_network(x, strict=False)` / a single address -> "net/prefix"
fn cidr(x: &str) -> R<String> {
    let Some((addr, prefix)) = x.split_once('/') else { return host_cidr(x) };
    let bad = || BuildError::Unsupported(format!("not an IP range: {x}"));
    let a: IpAddr = addr.parse().map_err(|_| bad())?;
    let p: u32 = prefix.parse().map_err(|_| bad())?;
    match a {
        IpAddr::V4(v4) if p <= 32 => {
            let mask = if p == 0 { 0 } else { u32::MAX << (32 - p) };
            Ok(format!("{}/{p}", std::net::Ipv4Addr::from(u32::from(v4) & mask)))
        }
        IpAddr::V6(v6) if p <= 128 => {
            let mask = if p == 0 { 0 } else { u128::MAX << (128 - p) };
            Ok(format!("{}/{p}", std::net::Ipv6Addr::from(u128::from(v6) & mask)))
        }
        _ => Err(bad()),
    }
}

fn listen(host: &str) -> String {
    if host.to_lowercase() == "localhost" { "127.0.0.1".into() } else { host.into() }
}

/// a JSON object from key/value pairs, in order
fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
}

fn wg_peers(ib: &Value) -> Vec<Value> {
    let text = ib.get("wg_peers").and_then(Value::as_str).filter(|s| !s.is_empty()).unwrap_or("[]");
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(a)) => a.into_iter().filter(|x| x.get("public_key").is_some_and(Value::is_string)).collect(),
        _ => vec![],
    }
}

/// The card's inbound. cert: (certificate path, key path) for https / hysteria2.
pub fn inbound(ib: &Value, cert: Option<(&str, &str)>) -> R<Value> {
    let t = st(get(ib, "type")?);
    let cert_tls = || -> R<Value> {
        let (c, k) = cert.ok_or_else(|| BuildError::Missing("certificate".into()))?;
        Ok(json!({"enabled": true, "certificate_path": c, "key_path": k}))
    };
    if t == "wireguard" {
        let peers = wg_peers(ib)
            .iter()
            .map(|x| Ok(json!({"public_key": strip(st(&x["public_key"])), "allowed_ips": [get(x, "address")?]})))
            .collect::<R<Vec<Value>>>()?;
        return Ok(obj(vec![
            ("type", json!("wireguard")),
            ("tag", json!(IN_TAG)),
            ("listen_port", get(ib, "port")?.clone()),
            ("address", json!([strip(st(get(ib, "wg_address")?))])),
            ("private_key", json!(strip(st(get(ib, "wg_private_key")?)))),
            ("mtu", opt(ib, "wg_mtu").cloned().unwrap_or(json!(1408))),
            ("peers", Value::Array(peers)),
        ]));
    }
    if t == "tun" {
        return Ok(obj(vec![
            ("type", json!("tun")),
            ("tag", json!(IN_TAG)),
            ("address", json!(TUN_ADDRESS)),
            ("mtu", json!(9000)),
            ("auto_route", json!(true)),
            ("strict_route", json!(ib.get("tun_strict_route").is_some_and(truthy))),
            ("stack", ib.get("tun_stack").cloned().unwrap_or(json!("mixed"))),
        ]));
    }
    let base = |kind: &str| -> R<Vec<(&'static str, Value)>> {
        Ok(vec![
            ("type", json!(kind)),
            ("tag", json!(IN_TAG)),
            ("listen", json!(listen(st(get(ib, "host")?)))),
            ("listen_port", get(ib, "port")?.clone()),
        ])
    };
    let users = || -> R<Value> { Ok(json!([{"username": get(ib, "username")?, "password": get(ib, "password")?}])) };
    match t {
        "forward" => {
            let mut d = base("direct")?;
            let host = strip(st(get(ib, "fwd_host")?));
            d.push(("override_address", json!(host.trim_matches(['[', ']']))));
            d.push(("override_port", get(ib, "fwd_port")?.clone()));
            let net = ib.get("fwd_network").cloned().unwrap_or(json!("tcp"));
            if net == json!("tcp") || net == json!("udp") {
                d.push(("network", net));
            }
            Ok(obj(d))
        }
        "socks5" | "socks4" => {
            let mut d = base("socks")?;
            if t == "socks5" && truthy(get(ib, "auth_enabled")?) {
                d.push(("users", users()?));
            }
            Ok(obj(d))
        }
        "http" | "https" => {
            let mut d = base("http")?;
            if truthy(get(ib, "auth_enabled")?) {
                d.push(("users", users()?));
            }
            if t == "https" {
                d.push(("tls", cert_tls()?));
            }
            Ok(obj(d))
        }
        "shadowsocks" => {
            let mut d = base("shadowsocks")?;
            d.push(("method", get(ib, "ss_method")?.clone()));
            d.push(("password", get(ib, "ss_password")?.clone()));
            Ok(obj(d))
        }
        "hysteria2" => {
            let mut d = base("hysteria2")?;
            d.push(("users", json!([{"password": get(ib, "hy2_password")?}])));
            d.push(("tls", cert_tls()?));
            if let Some(p) = opt(ib, "hy2_obfs_password") {
                d.push(("obfs", json!({"type": "salamander", "password": p})));
            }
            if let Some(v) = opt(ib, "hy2_up_mbps") {
                d.push(("up_mbps", v.clone()));
            }
            if let Some(v) = opt(ib, "hy2_down_mbps") {
                d.push(("down_mbps", v.clone()));
            }
            Ok(obj(d))
        }
        _ => Err(BuildError::Unsupported(format!("unsupported inbound {t}"))),
    }
}

fn tls(ob: &Value) -> R<Value> {
    let sni = match opt(ob, "sni") {
        Some(v) => v.clone(),
        None => get(ob, "host")?.clone(),
    };
    let verify = ob.get("tls_verify").map(truthy).unwrap_or(true);
    let mut t = json!({"enabled": true, "server_name": sni, "insecure": !verify});
    let kind = ob.get("type").and_then(Value::as_str).unwrap_or("");
    if let (Some(pin), true) = (opt(ob, "pin_sha256"), kind == "https" || kind == "hysteria2") {
        t["certificate_sha256"] = json!([pin_base64(st(pin))?]);
    }
    Ok(t)
}

/// ProxyDock's pin_sha256 (hex SHA-256 of the whole DER certificate, colons allowed, as in Hysteria2 share links)
/// as sing-box 1.15 certificate_sha256 wants it: base64 of the same 32 bytes
pub fn pin_base64(pin: &str) -> R<String> {
    use base64::Engine;
    let h: String = pin.chars().filter(|c| *c != ':').collect::<String>().to_ascii_lowercase();
    let bytes: Option<Vec<u8>> = (h.len() == 64 && h.bytes().all(|c| c.is_ascii_hexdigit()))
        .then(|| (0..32).map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).ok()).collect())
        .flatten();
    match bytes {
        Some(b) => Ok(base64::engine::general_purpose::STANDARD.encode(b)),
        None => Err(BuildError::Unsupported(format!("invalid certificate fingerprint {pin}"))),
    }
}

/// TLS for VLESS / VMess / Trojan / TUIC: SNI, verification, ALPN, uTLS fingerprint, REALITY
fn tls_full(ob: &Value, default_alpn: Option<Vec<&str>>) -> R<Value> {
    let mut t = tls(ob)?;
    let alpn = split(ob.get("alpn").and_then(Value::as_str).unwrap_or(""));
    if !alpn.is_empty() {
        t["alpn"] = json!(alpn);
    } else if let Some(d) = default_alpn {
        t["alpn"] = json!(d);
    }
    let kind = st(get(ob, "type")?);
    if let Some(fp) = opt(ob, "fingerprint") {
        if kind != "tuic" {
            t["utls"] = json!({"enabled": true, "fingerprint": fp});
        }
    }
    if ob.get("security") == Some(&json!("reality")) && kind == "vless" {
        let pk = strip(st(get(ob, "reality_public_key")?));
        let sid = strip(ob.get("reality_short_id").and_then(Value::as_str).unwrap_or(""));
        t["reality"] = json!({"enabled": true, "public_key": pk, "short_id": sid});
        t.as_object_mut().unwrap().shift_remove("insecure");
    }
    Ok(t)
}

fn transport(ob: &Value) -> Option<Value> {
    match ob.get("transport").and_then(Value::as_str).unwrap_or("tcp") {
        "ws" => {
            let path = opt(ob, "transport_path").cloned().unwrap_or(json!("/"));
            let mut d = json!({"type": "ws", "path": path});
            if let Some(h) = opt(ob, "transport_host") {
                d["headers"] = json!({"Host": h});
            }
            Some(d)
        }
        "grpc" => Some(json!({"type": "grpc", "service_name": ob.get("transport_path").cloned().unwrap_or(json!(""))})),
        _ => None,
    }
}

/// One outbound (not following via). WireGuard comes out as a sing-box endpoint.
pub fn outbound(ob: &Value, tag: &str) -> R<Value> {
    let t = st(get(ob, "type")?).to_string();
    if t == "direct" {
        let mut d = obj(vec![("type", json!("direct")), ("tag", json!(tag))]);
        let bind = ob.get("bind").filter(|v| truthy(v)).and_then(Value::as_str).unwrap_or("");
        if !bind.is_empty() && is_ip_literal(bind) {
            let key = if bind.contains(':') { "inet6_bind_address" } else { "inet4_bind_address" };
            d[key] = json!(strip_scope(bind));
        }
        return Ok(d);
    }
    let srv = |kind: &str| -> R<Vec<(&'static str, Value)>> {
        Ok(vec![
            ("type", json!(kind)),
            ("tag", json!(tag)),
            ("server", get(ob, "host")?.clone()),
            ("server_port", get(ob, "port")?.clone()),
        ])
    };
    let login = |d: &mut Vec<(&'static str, Value)>, with_password: bool| {
        if let Some(u) = opt(ob, "username") {
            d.push(("username", u.clone()));
            if with_password {
                d.push(("password", ob.get("password").cloned().unwrap_or(json!(""))));
            }
        }
    };
    let with_transport = |mut d: Vec<(&'static str, Value)>| {
        if let Some(tr) = transport(ob) {
            d.push(("transport", tr));
        }
        obj(d)
    };
    match t.as_str() {
        "http" | "https" => {
            let mut d = srv("http")?;
            login(&mut d, true);
            if t == "https" {
                d.push(("tls", tls(ob)?));
            }
            Ok(obj(d))
        }
        "socks5" => {
            let mut d = srv("socks")?;
            d.push(("version", json!("5")));
            login(&mut d, true);
            Ok(obj(d))
        }
        "socks4" => {
            let mut d = srv("socks")?;
            d.push(("version", json!("4a")));
            login(&mut d, false);
            Ok(obj(d))
        }
        "shadowsocks" => {
            let mut d = srv("shadowsocks")?;
            d.push(("method", get(ob, "ss_method")?.clone()));
            d.push(("password", get(ob, "ss_password")?.clone()));
            Ok(obj(d))
        }
        "hysteria2" => {
            let mut d = srv("hysteria2")?;
            d.push(("password", get(ob, "hy2_password")?.clone()));
            d.push(("tls", tls(ob)?));
            if let Some(p) = opt(ob, "hy2_obfs_password") {
                d.push(("obfs", json!({"type": "salamander", "password": p})));
            }
            if let Some(v) = opt(ob, "hy2_up_mbps") {
                d.push(("up_mbps", v.clone()));
            }
            if let Some(v) = opt(ob, "hy2_down_mbps") {
                d.push(("down_mbps", v.clone()));
            }
            let ports_text = ob.get("hy2_ports").filter(|v| truthy(v)).and_then(Value::as_str).unwrap_or("");
            if !crate::py::strip(ports_text).is_empty() {
                if let Some(ports) = crate::config::hop_ports(ports_text) {
                    d.push(("server_ports", json!(ports)));
                    let interval = opt(ob, "hy2_hop_interval").cloned().unwrap_or(json!(30));
                    d.push(("hop_interval", json!(format!("{}s", py_str(&interval)))));
                }
            }
            Ok(obj(d))
        }
        "vless" => {
            let mut d = srv("vless")?;
            d.push(("uuid", json!(strip(st(get(ob, "uuid")?)))));
            d.push(("packet_encoding", json!("xudp")));
            if let Some(f) = opt(ob, "vless_flow") {
                d.push(("flow", f.clone()));
            }
            if ob.get("security").cloned().unwrap_or(json!("tls")) != json!("none") {
                d.push(("tls", tls_full(ob, None)?));
            }
            Ok(with_transport(d))
        }
        "vmess" => {
            let mut d = srv("vmess")?;
            d.push(("uuid", json!(strip(st(get(ob, "uuid")?)))));
            d.push(("security", opt(ob, "vmess_cipher").cloned().unwrap_or(json!("auto"))));
            d.push(("alter_id", ob.get("vmess_alter_id").cloned().unwrap_or(json!(0))));
            d.push(("packet_encoding", json!("xudp")));
            if ob.get("security").cloned().unwrap_or(json!("none")) == json!("tls") {
                d.push(("tls", tls_full(ob, None)?));
            }
            Ok(with_transport(d))
        }
        "trojan" => {
            let mut d = srv("trojan")?;
            d.push(("password", get(ob, "password")?.clone()));
            d.push(("tls", tls_full(ob, None)?));
            Ok(with_transport(d))
        }
        "tuic" => {
            let mut d = srv("tuic")?;
            d.push(("uuid", json!(strip(st(get(ob, "uuid")?)))));
            d.push(("password", get(ob, "password")?.clone()));
            d.push(("congestion_control", opt(ob, "tuic_congestion").cloned().unwrap_or(json!("bbr"))));
            d.push(("udp_relay_mode", opt(ob, "tuic_udp_relay").cloned().unwrap_or(json!("native"))));
            d.push(("tls", tls_full(ob, Some(vec!["h3"]))?));
            Ok(obj(d))
        }
        "wireguard" => {
            let mut peer = obj(vec![
                ("address", get(ob, "host")?.clone()),
                ("port", get(ob, "port")?.clone()),
                ("public_key", json!(strip(st(get(ob, "wg_peer_public_key")?)))),
                ("allowed_ips", json!(split(st(get(ob, "wg_allowed_ips")?)))),
            ]);
            if let Some(k) = opt(ob, "wg_preshared_key") {
                peer["pre_shared_key"] = json!(strip(st(k)));
            }
            if let Some(k) = opt(ob, "wg_keepalive") {
                peer["persistent_keepalive_interval"] = k.clone();
            }
            Ok(obj(vec![
                ("type", json!("wireguard")),
                ("tag", json!(tag)),
                ("address", json!(split(st(get(ob, "wg_address")?)))),
                ("private_key", json!(strip(st(get(ob, "wg_private_key")?)))),
                ("mtu", opt(ob, "wg_mtu").cloned().unwrap_or(json!(1408))),
                ("peers", json!([peer])),
            ]))
        }
        _ => Err(BuildError::Unsupported(format!("unsupported outbound {t}"))),
    }
}

/// The via chain as sing-box outbounds: <prefix>0 (the card's outbound) -detour-> <prefix>1 -> ... A Block outbound
/// has none (it becomes a reject rule). A network interface applies to the hop that dials out itself.
pub fn outbounds(ob: &Value, prefix: &str) -> R<Vec<Value>> {
    let mut out = vec![];
    let mut i = 0;
    let mut cur = ob;
    while truthy(cur) && get(cur, "type")? != &json!("block") {
        let tag = format!("{prefix}{i}");
        match st(&cur["type"]) {
            "subscription" => {
                out.extend(subscription_outbounds(cur, &tag)?);
                break;
            }
            "group" => {
                out.extend(group_outbounds(cur, &tag)?);
                break;
            }
            _ => {}
        }
        let mut d = outbound(cur, &tag)?;
        if cur.get("via").is_some_and(truthy) {
            d["detour"] = json!(format!("{prefix}{}", i + 1));
        } else if let Some(iface) = opt(cur, "interface") {
            d["bind_interface"] = iface.clone();
        }
        out.push(d);
        cur = cur.get("via").unwrap_or(&Value::Null);
        i += 1;
    }
    Ok(out)
}

/// the servers of a subscription block (Python `subscription.servers_of`)
pub fn servers_of(ob: &Value) -> Vec<Value> {
    let text = ob.get("sub_servers").filter(|v| truthy(v)).and_then(Value::as_str).unwrap_or("[]");
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Array(a)) => a.into_iter().filter(|s| s.get("outbound").is_some_and(Value::is_object)).collect(),
        _ => vec![],
    }
}

/// A subscription block: one outbound per server plus a group with the block's tag (selector = the chosen server,
/// urltest = the fastest one).
pub fn subscription_outbounds(ob: &Value, tag: &str) -> R<Vec<Value>> {
    let mut members = vec![];
    let mut tags: Vec<(Value, String)> = vec![];
    for (i, s) in servers_of(ob).iter().enumerate() {
        let mut m = s["outbound"].clone();
        m["via"] = Value::Null;
        let mut d = match outbound(&m, &format!("{tag}_m{i}")) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if let Some(iface) = opt(ob, "interface") {
            d["bind_interface"] = iface.clone();
        }
        let name = get(s, "name")?.clone();
        tags.push((name, st(&d["tag"]).to_string()));
        members.push(d);
    }
    if tags.is_empty() {
        return Ok(vec![json!({"type": "block", "tag": tag})]);
    }
    let all: Vec<&String> = tags.iter().map(|(_, t)| t).collect();
    let group = if ob.get("sub_mode") == Some(&json!("select")) {
        let chosen = ob
            .get("sub_selected")
            .and_then(|sel| tags.iter().find(|(n, _)| n == sel))
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| tags[0].1.clone());
        obj(vec![("type", json!("selector")), ("tag", json!(tag)), ("outbounds", json!(all)), ("default", json!(chosen))])
    } else {
        obj(vec![
            ("type", json!("urltest")),
            ("tag", json!(tag)),
            ("outbounds", json!(all)),
            ("url", opt(ob, "sub_test_url").cloned().unwrap_or(json!(TEST_URL))),
            ("interval", json!("3m")),
            ("tolerance", json!(50)),
        ])
    };
    members.push(group);
    Ok(members)
}

/// Python `max(a, v)`: v only when it is larger (a when equal)
fn py_max(a: i64, v: &Value) -> Value {
    match v.as_f64() {
        Some(f) if f > a as f64 => v.clone(),
        _ => json!(a),
    }
}

/// A group block: every member's chain (first hop <tag>_m<k>_0) plus a urltest / selector with the block's tag.
/// auto = the fastest member; failover = the first member in order that answers (a tolerance larger than any
/// delay keeps the first working member); select = the chosen one.
pub fn group_outbounds(ob: &Value, tag: &str) -> R<Vec<Value>> {
    let mut out = vec![];
    let mut heads = vec![];
    let members: Vec<Value> = opt(ob, "members").and_then(Value::as_array).cloned().unwrap_or_default();
    for (k, m) in members.iter().enumerate() {
        let chain = outbounds(get(m, "outbound")?, &format!("{tag}_m{k}_"))?;
        if !chain.is_empty() {
            out.extend(chain);
            heads.push(format!("{tag}_m{k}_0"));
        }
    }
    if heads.is_empty() {
        return Err(BuildError::Unsupported("group without members".into()));
    }
    if ob.get("group_mode") == Some(&json!("select")) {
        let mut ids = vec![];
        for m in &members {
            if !outbounds(get(m, "outbound")?, "x")?.is_empty() {
                ids.push(m.get("id").cloned().unwrap_or(Value::Null));
            }
        }
        let chosen = ob
            .get("group_selected")
            .and_then(|sel| ids.iter().position(|i| i == sel))
            .map(|i| heads[i].clone())
            .unwrap_or_else(|| heads[0].clone());
        out.push(obj(vec![
            ("type", json!("selector")),
            ("tag", json!(tag)),
            ("outbounds", json!(heads)),
            ("default", json!(chosen)),
            ("interrupt_exist_connections", json!(true)),
        ]));
        return Ok(out);
    }
    let failover = ob.get("group_mode") == Some(&json!("failover"));
    let interval = py_max(10, opt(ob, "group_interval").unwrap_or(&json!(180)));
    let tolerance = if failover {
        json!(FAILOVER_TOLERANCE)
    } else {
        py_max(0, ob.get("group_tolerance").unwrap_or(&json!(50)))
    };
    out.push(obj(vec![
        ("type", json!("urltest")),
        ("tag", json!(tag)),
        ("outbounds", json!(heads)),
        ("url", opt(ob, "group_test_url").cloned().unwrap_or(json!(TEST_URL))),
        ("interval", json!(format!("{}s", py_str(&interval)))),
        ("tolerance", tolerance),
        ("interrupt_exist_connections", json!(true)),
    ]));
    Ok(out)
}

fn strs(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().map(|x| st(x).to_string()).collect()).unwrap_or_default()
}

/// rule-set tag of a GeoIP / Geosite name (Python `rulesets.tag`)
pub fn ruleset_tag(kind: &str, name: &str) -> String {
    format!("{kind}-{name}")
}

/// One router rule's conditions -> sing-box rule fields (without the action).
pub fn match_fields(m: &Value) -> R<Value> {
    let mut d = Obj::new();
    if opt(m, "domain").is_some() {
        d.insert("domain".into(), json!(strs(m.get("domain")).iter().map(|x| x.trim_end_matches('.')).collect::<Vec<_>>()));
    }
    if opt(m, "domain_suffix").is_some() {
        let v: Vec<String> = strs(m.get("domain_suffix"))
            .iter()
            .map(|x| x.trim_start_matches('*').trim_start_matches('.').trim_end_matches('.').to_string())
            .collect();
        d.insert("domain_suffix".into(), json!(v));
    }
    if opt(m, "domain_keyword").is_some() {
        d.insert("domain_keyword".into(), json!(strs(m.get("domain_keyword"))));
    }
    if opt(m, "ip_cidr").is_some() {
        d.insert("ip_cidr".into(), json!(strs(m.get("ip_cidr")).iter().map(|x| cidr(x)).collect::<R<Vec<_>>>()?));
    }
    if opt(m, "ip_private").is_some() {
        d.insert("ip_is_private".into(), json!(true));
    }
    let mut sets: Vec<String> = strs(m.get("geoip")).iter().map(|x| ruleset_tag("geoip", x)).collect();
    sets.extend(strs(m.get("geosite")).iter().map(|x| ruleset_tag("geosite", x)));
    if !sets.is_empty() {
        d.insert("rule_set".into(), json!(sets));
    }
    let (mut ports, mut ranges) = (vec![], vec![]);
    for x in strs(m.get("port")) {
        if let Some((a, b)) = x.split_once('-') {
            ranges.push(format!("{}:{}", py_int(a)?, py_int(b)?));
        } else {
            ports.push(py_int(&x)?);
        }
    }
    if !ports.is_empty() {
        d.insert("port".into(), json!(ports));
    }
    if !ranges.is_empty() {
        d.insert("port_range".into(), json!(ranges));
    }
    if opt(m, "process_name").is_some() {
        d.insert("process_name".into(), json!(strs(m.get("process_name"))));
    }
    Ok(Value::Object(d))
}

/// conditions of every router rule on the way (all must match)
pub fn route_rule(conds: &[Value]) -> R<Value> {
    if conds.len() == 1 {
        return match_fields(&conds[0]);
    }
    Ok(json!({"type": "logical", "mode": "and", "rules": conds.iter().map(match_fields).collect::<R<Vec<_>>>()?}))
}

fn allow_rule(domains: &[String]) -> R<Value> {
    let (mut exact, mut suffix, mut cidrs) = (vec![], vec![], vec![]);
    for d in domains.iter().map(|x| x.to_lowercase().trim_end_matches('.').to_string()) {
        if let Some(s) = d.strip_prefix("*.") {
            suffix.push(s.to_string());
        } else if is_ip_literal(&d) {
            cidrs.push(host_cidr(&d)?);
        } else {
            exact.push(d);
        }
    }
    let mut rule = obj(vec![("invert", json!(true)), ("action", json!("reject"))]);
    if !exact.is_empty() {
        rule["domain"] = json!(exact);
    }
    if !suffix.is_empty() {
        rule["domain_suffix"] = json!(suffix);
    }
    if !cidrs.is_empty() {
        rule["ip_cidr"] = json!(cidrs);
    }
    Ok(rule)
}

/// (sing-box dns section or None, tag of the server used for real lookups). The PC's DNS unless a DNS server is
/// set; through the card's outbound if dns_via_outbound (not Direct / Block). A TUN card with Fake-IP answers
/// apps' A / AAAA queries with fake addresses.
pub fn dns(proxy: &Value, cfg: &Value) -> (Option<Value>, Option<String>) {
    let spec = dns_server_spec(cfg.get("dns_server").and_then(Value::as_str).unwrap_or("")).unwrap_or_default();
    let ib = &proxy["inbound"];
    let tun = ib["type"] == json!("tun");
    let fakeip = tun && ib.get("tun_fakeip").is_some_and(truthy);
    if spec.is_empty() && !tun {
        return (None, None);
    }
    let mut servers = vec![json!({"type": "local", "tag": LOCAL_DNS})];
    let mut real = LOCAL_DNS.to_string();
    if !spec.is_empty() {
        let mut d = Value::Object(spec);
        d["tag"] = json!(DNS_TAG);
        if !is_ip_literal(st(&d["server"])) {
            d["domain_resolver"] = json!(LOCAL_DNS);
        }
        let ob_type = st(&proxy["outbound"]["type"]);
        if cfg.get("dns_via_outbound").is_some_and(truthy) && ob_type != "direct" && ob_type != "block" {
            d["detour"] = json!("out0");
        }
        servers.push(d);
        real = DNS_TAG.into();
    }
    let mut conf = json!({"servers": servers, "final": real});
    if fakeip {
        conf["servers"].as_array_mut().unwrap().push(
            json!({"type": "fakeip", "tag": FAKEIP_TAG, "inet4_range": FAKEIP_V4, "inet6_range": FAKEIP_V6}),
        );
        conf["rules"] = json!([{"query_type": ["A", "AAAA"], "server": FAKEIP_TAG}]);
    }
    (Some(conf), Some(real))
}

/// {router target block id: its outbound tag prefix}, in order (one chain per target block, shared by its rules)
pub fn route_exit_tags(proxy: &Value) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = vec![];
    for r in proxy.get("routes").and_then(Value::as_array).into_iter().flatten() {
        let block = st(&r["block"]).to_string();
        if r["outbound"]["type"] != json!("block") && !tags.iter().any(|(b, _)| *b == block) {
            let prefix = format!("r{}_", tags.len());
            tags.push((block, prefix));
        }
    }
    tags
}

/// outbound block id a connection leaving through outbound `tag` (sing-box chains[0]) belongs to
pub fn exit_of_tag(proxy: &Value, tag: &str) -> Option<String> {
    for (block, prefix) in route_exit_tags(proxy) {
        if tag.starts_with(&prefix) {
            return Some(block);
        }
    }
    if tag.starts_with("out") { proxy.get("exit").and_then(Value::as_str).map(String::from) } else { None }
}

/// (route section, reasons): reasons[i] says what rule i blocks.
pub fn route(
    proxy: &Value,
    cfg: &Value,
    bans_path: &str,
    rulesets: &[(String, String)],
    route_tags: &[(String, String)],
    exclude_paths: &[String],
) -> R<(Value, Vec<Option<Reason>>)> {
    let ib = &proxy["inbound"];
    let ob = &proxy["outbound"];
    let mut rules: Vec<Value> = vec![];
    let mut reasons: Vec<Option<Reason>> = vec![];
    let (_, resolver) = dns(proxy, cfg);
    let resolve = match &resolver {
        Some(r) => json!({"action": "resolve", "server": r}),
        None => json!({"action": "resolve"}),
    };
    let tun = ib["type"] == json!("tun");
    let fakeip = tun && ib.get("tun_fakeip").is_some_and(truthy);
    let mut add = |rule: Value, reason: Option<Reason>| {
        rules.push(rule);
        reasons.push(reason);
    };
    if tun {
        // apps connect by IP: sniff for the domain so domain rules work; answer DNS here
        add(json!({"action": "sniff"}), None);
        add(json!({"protocol": "dns", "action": "hijack-dns"}), None);
        if !exclude_paths.is_empty() {
            add(json!({"process_path": exclude_paths, "action": "route", "outbound": TUN_DIRECT}), None);
        }
    }
    add(json!({"action": "route-options", "udp_timeout": format!("{}s", py_str(get(cfg, "timeout")?))}), None);
    add(json!({"rule_set": BANS_TAG, "action": "reject"}), Some(Reason::Banned));
    if !tun && ib["type"] != json!("wireguard") {
        // never connect to this proxy's own port (would loop forever)
        let host = st(&ib["host"]);
        let mut cidrs = vec![json!("127.0.0.0/8"), json!("::1/128")];
        if is_ip_literal(host) && !host.contains(':') && host != "0.0.0.0" {
            cidrs.push(json!(format!("{host}/32")));
        }
        add(
            json!({"domain": ["localhost"], "ip_cidr": cidrs, "port": [ib["port"]], "action": "reject"}),
            Some(Reason::SelfPort),
        );
    }
    let doms = strs(cfg.get("allowed_domains"));
    if !doms.iter().any(|d| d == "*") {
        add(allow_rule(&doms)?, Some(Reason::Allow));
    }
    if cfg.get("block_private").is_some_and(truthy) {
        if ob["type"] == json!("direct") {
            // Direct resolves here, so check the resolved address too
            add(resolve.clone(), None);
        }
        add(json!({"domain_suffix": ["localhost"], "action": "reject"}), Some(Reason::Private));
        add(json!({"ip_is_private": true, "action": "reject"}), Some(Reason::Private));
        let extra: Vec<&str> = EXTRA_INTERNAL.iter().copied().filter(|x| !(fakeip && *x == FAKEIP_V4)).collect();
        add(json!({"ip_cidr": extra, "action": "reject"}), Some(Reason::Private));
    }
    let routes: Vec<Value> = proxy.get("routes").and_then(Value::as_array).cloned().unwrap_or_default();
    let needs_ip = routes.iter().any(|r| {
        r["match"].as_array().into_iter().flatten().any(|m| IP_KEYS.iter().any(|k| m.get(*k).is_some()))
    });
    if needs_ip {
        // IP / GeoIP conditions need the destination address (a tunnel then connects to the IP, not the domain)
        add(resolve.clone(), None);
    }
    for r in &routes {
        let mut rule = route_rule(r["match"].as_array().map(Vec::as_slice).unwrap_or(&[]))?;
        if r["outbound"]["type"] == json!("block") {
            rule["action"] = json!("reject");
            add(rule, Some(Reason::Rule(st(&r["name"]).into())));
        } else {
            let block = st(&r["block"]);
            let prefix = &route_tags.iter().find(|(b, _)| b == block).ok_or(BuildError::Missing(block.into()))?.1;
            rule["action"] = json!("route");
            rule["outbound"] = json!(format!("{prefix}0"));
            add(rule, None);
        }
    }
    if ob["type"] == json!("block") {
        // the router's default is Block: refuse everything else
        add(json!({"action": "reject"}), Some(Reason::Rule("default".into())));
    }
    let mut sets = vec![json!({"type": "local", "tag": BANS_TAG, "format": "source", "path": bans_path})];
    let mut sorted = rulesets.to_vec();
    sorted.sort();
    sets.extend(sorted.iter().map(|(t, p)| json!({"type": "local", "tag": t, "format": "binary", "path": p})));
    let mut r = json!({"rule_set": sets, "rules": rules, "final": "out0"});
    if tun {
        r["auto_detect_interface"] = json!(true);
    }
    if let Some(res) = resolver {
        r["default_domain_resolver"] = json!(res);
    }
    Ok((r, reasons))
}

/// Inputs of a card's sing-box config that come from outside the graph.
pub struct BuildEnv<'a> {
    pub bans_path: &'a str,
    pub api_port: u16,
    pub api_secret: &'a str,
    pub cert: Option<(&'a str, &'a str)>,
    /// rule-set tag -> local file
    pub rulesets: &'a [(String, String)],
    /// ProxyDock's own engines: a TUN card routes them around itself
    pub exclude_paths: &'a [String],
}

/// Full sing-box config of a card. cfg = the card's effective proxy settings (config resolve). Returns the config
/// and what each route rule blocks.
pub fn build(proxy: &Value, cfg: &Value, env: &BuildEnv) -> R<(Value, Vec<Option<Reason>>)> {
    let mut obs = outbounds(&proxy["outbound"], "out")?;
    if obs.is_empty() {
        obs.push(json!({"type": "direct", "tag": "out0"})); // (Block default: never used)
    }
    let route_tags = route_exit_tags(proxy);
    for r in proxy.get("routes").and_then(Value::as_array).into_iter().flatten() {
        if r["outbound"]["type"] == json!("block") {
            continue;
        }
        let block = st(&r["block"]);
        if let Some((_, prefix)) = route_tags.iter().find(|(b, _)| b == block) {
            if !obs.iter().any(|o| st(&o["tag"]).starts_with(prefix.as_str())) {
                obs.extend(outbounds(&r["outbound"], prefix)?);
            }
        }
    }
    let tun = proxy["inbound"]["type"] == json!("tun");
    if tun {
        obs.push(json!({"type": "direct", "tag": TUN_DIRECT}));
    }
    let (r, reasons) = route(proxy, cfg, env.bans_path, env.rulesets, &route_tags, env.exclude_paths)?;
    let mut endpoints: Vec<Value> = obs.iter().filter(|o| o["type"] == json!("wireguard")).cloned().collect();
    let ib_conf = inbound(&proxy["inbound"], env.cert)?;
    let ib_is_wg = ib_conf["type"] == json!("wireguard");
    if ib_is_wg {
        endpoints.insert(0, ib_conf.clone());
    }
    let mut plain: Vec<Value> = obs.into_iter().filter(|o| o["type"] != json!("wireguard")).collect();
    if plain.is_empty() {
        plain.push(json!({"type": "direct", "tag": "direct"}));
    }
    let mut conf = obj(vec![
        ("log", json!({"level": "debug", "timestamp": false})),
        ("inbounds", if ib_is_wg { json!([]) } else { json!([ib_conf]) }),
        ("outbounds", Value::Array(plain)),
        ("route", r),
        (
            "experimental",
            json!({"clash_api": {"external_controller": format!("127.0.0.1:{}", env.api_port), "secret": env.api_secret}}),
        ),
    ]);
    if !endpoints.is_empty() {
        conf["endpoints"] = Value::Array(endpoints);
    }
    if let (Some(d), _) = dns(proxy, cfg) {
        conf["dns"] = d;
    }
    Ok((conf, reasons))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_like_python() {
        assert_eq!(cidr("10.1.2.3/8").unwrap(), "10.0.0.0/8");
        assert_eq!(cidr("2001:DB8::1/32").unwrap(), "2001:db8::/32");
        assert_eq!(cidr("192.168.0.1").unwrap(), "192.168.0.1/32");
        assert!(cidr("10.0.0.0/33").is_err());
    }

    #[test]
    fn python_numbers() {
        assert_eq!(py_int(" +8_0 ").unwrap(), 80);
        assert!(py_int("8__0").is_err());
        assert_eq!(py_str(&json!(30.0)), "30.0");
        assert_eq!(py_str(&json!(1e16)), "1e+16");
        assert_eq!(py_str(&json!(30)), "30");
    }
}
