//! Clash / Clash.Meta (mihomo) subscription YAML -> outbound settings (Python: `clash.py`).
//!
//! Only the "proxies:" list is read. The YAML is read the way PyYAML's `safe_load` reads it (YAML 1.1 plain
//! scalars: yes / no / on / off are booleans, 0o-less octal, 1:30 sexagesimal, `<<` merge keys, anchors), because
//! the values go through Python's `str()` in the Python app.

use crate::i18n::tr;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::sync::LazyLock;
use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser, Tag};
use yaml_rust2::scanner::{Marker, TScalarStyle};

/// a PyYAML value
#[derive(Clone, Debug)]
pub enum Y {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(String),
    Seq(Vec<Y>),
    /// in order; a later equal key wins (`get` looks from the end)
    Map(Vec<(Y, Y)>),
}

impl Y {
    pub fn get(&self, key: &str) -> Option<&Y> {
        match self {
            Y::Map(m) => m.iter().rev().find(|(k, _)| matches!(k, Y::Str(s) if s == key)).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Python truthiness
    fn truthy(&self) -> bool {
        match self {
            Y::Null => false,
            Y::Bool(b) => *b,
            Y::Int(i) => *i != 0,
            Y::Float(f) => *f != 0.0,
            Y::Str(s) => !s.is_empty(),
            Y::Seq(v) => !v.is_empty(),
            Y::Map(m) => !m.is_empty(),
        }
    }

    /// Python `str()` (lists and maps roughly as Python prints them)
    fn py_str(&self) -> String {
        match self {
            Y::Null => "None".into(),
            Y::Bool(b) => if *b { "True" } else { "False" }.into(),
            Y::Int(i) => i.to_string(),
            Y::Float(f) if f.is_nan() => "nan".into(),
            Y::Float(f) if f.is_infinite() => if *f > 0.0 { "inf" } else { "-inf" }.into(),
            Y::Float(f) => crate::py::float_repr(*f),
            Y::Str(s) => s.clone(),
            Y::Seq(v) => format!("[{}]", v.iter().map(Y::py_repr).collect::<Vec<_>>().join(", ")),
            Y::Map(m) => format!(
                "{{{}}}",
                m.iter().map(|(k, v)| format!("{}: {}", k.py_repr(), v.py_repr())).collect::<Vec<_>>().join(", ")
            ),
        }
    }

    fn py_repr(&self) -> String {
        match self {
            Y::Str(s) => crate::py::repr(s),
            other => other.py_str(),
        }
    }

    /// Python `int(v)` (None if Python raises)
    fn py_int(&self) -> Option<i128> {
        match self {
            Y::Bool(b) => Some(*b as i128),
            Y::Int(i) => Some(*i),
            Y::Float(f) if f.is_finite() => Some(f.trunc() as i128),
            Y::Str(s) => crate::py::int(s).map(i128::from),
            _ => None,
        }
    }
}

/// `_s(v)`: `"" if v is None else str(v).strip()`
fn s(v: Option<&Y>) -> String {
    match v {
        None | Some(Y::Null) => String::new(),
        Some(v) => crate::py::strip(&v.py_str()).to_string(),
    }
}

/// `_b(v)`
fn b(v: Option<&Y>) -> bool {
    matches!(v, Some(Y::Bool(true))) || matches!(s(v).to_lowercase().as_str(), "true" | "1" | "yes")
}

fn truthy(v: Option<&Y>) -> bool {
    v.is_some_and(Y::truthy)
}

/// `a or b`
fn or<'a>(a: Option<&'a Y>, b: Option<&'a Y>) -> Option<&'a Y> {
    if truthy(a) { a } else { b }
}

// --- reading the YAML like PyYAML ---

static BOOL: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
    regex_lite::Regex::new(r"^(?:yes|Yes|YES|no|No|NO|true|True|TRUE|false|False|FALSE|on|On|ON|off|Off|OFF)$").unwrap()
});
static FLOAT: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
    regex_lite::Regex::new(r"^(?:[-+]?(?:[0-9][0-9_]*)\.[0-9_]*(?:[eE][-+][0-9]+)?|\.[0-9][0-9_]*(?:[eE][-+][0-9]+)?|[-+]?[0-9][0-9_]*(?::[0-5]?[0-9])+\.[0-9_]*|[-+]?\.(?:inf|Inf|INF)|\.(?:nan|NaN|NAN))$").unwrap()
});
static INT: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
    regex_lite::Regex::new(r"^(?:[-+]?0b[0-1_]+|[-+]?0[0-7_]+|[-+]?(?:0|[1-9][0-9_]*)|[-+]?0x[0-9a-fA-F_]+|[-+]?[1-9][0-9_]*(?::[0-5]?[0-9])+)$").unwrap()
});

fn sexagesimal(parts: &str) -> f64 {
    parts.split(':').fold(0.0, |acc, p| acc * 60.0 + p.parse::<f64>().unwrap_or(0.0))
}

/// PyYAML `construct_yaml_int`
fn to_int(text: &str) -> Y {
    let v = text.replace('_', "");
    let (neg, v) = match v.strip_prefix('-') {
        Some(r) => (true, r.to_string()),
        None => (false, v.strip_prefix('+').unwrap_or(&v).to_string()),
    };
    let n: Option<i128> = if v == "0" {
        Some(0)
    } else if let Some(b) = v.strip_prefix("0b") {
        i128::from_str_radix(b, 2).ok()
    } else if let Some(h) = v.strip_prefix("0x") {
        i128::from_str_radix(h, 16).ok()
    } else if v.starts_with('0') {
        i128::from_str_radix(&v, 8).ok()
    } else if v.contains(':') {
        Some(v.split(':').fold(0i128, |acc, p| acc.saturating_mul(60).saturating_add(p.parse().unwrap_or(0))))
    } else {
        v.parse().ok()
    };
    match n {
        Some(n) => Y::Int(if neg { -n } else { n }),
        None => Y::Str(text.to_string()),
    }
}

/// PyYAML `construct_yaml_float`
fn to_float(text: &str) -> Y {
    let v = text.replace('_', "").to_lowercase();
    let (sign, v) = match v.strip_prefix('-') {
        Some(r) => (-1.0, r.to_string()),
        None => (1.0, v.strip_prefix('+').unwrap_or(&v).to_string()),
    };
    let f = if v == ".inf" {
        f64::INFINITY
    } else if v == ".nan" {
        f64::NAN
    } else if v.contains(':') {
        sexagesimal(&v)
    } else {
        // Python float() takes "1." and ".5" like Rust
        v.parse().unwrap_or(0.0)
    };
    Y::Float(sign * f)
}

/// the implicit types of a plain scalar (PyYAML's resolver)
fn plain(text: &str) -> Y {
    if matches!(text, "" | "~" | "null" | "Null" | "NULL") {
        Y::Null
    } else if BOOL.is_match(text) {
        Y::Bool(matches!(text.to_lowercase().as_str(), "yes" | "true" | "on"))
    } else if INT.is_match(text) {
        to_int(text)
    } else if FLOAT.is_match(text) {
        to_float(text)
    } else {
        Y::Str(text.to_string())
    }
}

fn tagged(text: String, tag: &Tag) -> Result<Y, String> {
    if tag.handle == "!" && tag.suffix.is_empty() {
        return Ok(Y::Str(text)); // the non-specific tag
    }
    if tag.handle != "tag:yaml.org,2002:" {
        return Err(format!("could not determine a constructor for the tag '{}{}'", tag.handle, tag.suffix));
    }
    Ok(match tag.suffix.as_str() {
        "str" => Y::Str(text),
        "null" => Y::Null,
        "bool" => Y::Bool(matches!(text.to_lowercase().as_str(), "yes" | "true" | "on")),
        "int" => to_int(&text),
        "float" => to_float(&text),
        other => return Err(format!("could not determine a constructor for the tag 'tag:yaml.org,2002:{other}'")),
    })
}

enum Frame {
    Seq(usize, Vec<Y>),
    /// anchor, pairs, a key waiting for its value, merged pairs
    Map(usize, Vec<(Y, Y)>, Option<(Y, bool)>, Vec<Vec<(Y, Y)>>),
}

#[derive(Default)]
struct Builder {
    stack: Vec<Frame>,
    anchors: HashMap<usize, Y>,
    doc: Option<Y>,
    error: Option<String>,
}

impl Builder {
    fn push_value(&mut self, v: Y, anchor: usize, is_merge_key: bool) {
        if anchor > 0 {
            self.anchors.insert(anchor, v.clone());
        }
        match self.stack.last_mut() {
            None => {
                if self.doc.is_none() {
                    self.doc = Some(v);
                }
            }
            Some(Frame::Seq(_, items)) => items.push(v),
            Some(Frame::Map(_, pairs, pending, merges)) => match pending.take() {
                None => *pending = Some((v, is_merge_key)),
                Some((_, true)) => {
                    // `<<: *base` or `<<: [*a, *b]`, in the order listed (PyYAML 6.0.4's flatten_mapping)
                    match v {
                        Y::Map(m) => merges.push(m),
                        Y::Seq(list) => {
                            for x in list {
                                match x {
                                    Y::Map(m) => merges.push(m),
                                    _ => self.error = Some("expected a mapping for merging".into()),
                                }
                            }
                        }
                        _ => self.error = Some("expected a mapping or list of mappings for merging".into()),
                    }
                }
                Some((k, false)) => {
                    if matches!(k, Y::Seq(_) | Y::Map(_)) {
                        self.error = Some("found unhashable key".into());
                    }
                    pairs.push((k, v));
                }
            },
        }
    }
}

impl MarkedEventReceiver for Builder {
    fn on_event(&mut self, ev: Event, _mark: Marker) {
        if self.error.is_some() {
            return;
        }
        match ev {
            Event::Scalar(text, style, anchor, tag) => {
                let merge_key = style == TScalarStyle::Plain
                    && tag.is_none()
                    && text == "<<"
                    && matches!(self.stack.last(), Some(Frame::Map(_, _, None, _)));
                let v = match (&tag, style) {
                    (Some(t), _) => match tagged(text, t) {
                        Ok(v) => v,
                        Err(e) => {
                            self.error = Some(e);
                            return;
                        }
                    },
                    (None, TScalarStyle::Plain) => plain(&text),
                    (None, _) => Y::Str(text),
                };
                self.push_value(v, anchor, merge_key);
            }
            Event::Alias(id) => match self.anchors.get(&id).cloned() {
                Some(v) => self.push_value(v, 0, false),
                None => self.error = Some("found undefined alias".into()),
            },
            Event::SequenceStart(anchor, _) => self.stack.push(Frame::Seq(anchor, vec![])),
            Event::MappingStart(anchor, _) => self.stack.push(Frame::Map(anchor, vec![], None, vec![])),
            Event::SequenceEnd => {
                if let Some(Frame::Seq(anchor, items)) = self.stack.pop() {
                    self.push_value(Y::Seq(items), anchor, false);
                }
            }
            Event::MappingEnd => {
                if let Some(Frame::Map(anchor, pairs, _, merges)) = self.stack.pop() {
                    // merged pairs first, then the own ones; a later equal key wins. PyYAML 6.0.4 drops a merged
                    // pair whose key string is the same object as an earlier merged one's: CPython shares the
                    // strings of one character below U+0100 (and ""), so for those the first merged one wins.
                    let mut all: Vec<(Y, Y)> = vec![];
                    for (k, v) in merges.into_iter().flatten() {
                        let shared =
                            matches!(&k, Y::Str(t) if t.chars().count() <= 1 && t.chars().all(|c| (c as u32) < 0x100));
                        if shared && all.iter().any(|(a, _)| matches!((a, &k), (Y::Str(x), Y::Str(y)) if x == y)) {
                            continue;
                        }
                        all.push((k, v));
                    }
                    all.extend(pairs);
                    self.push_value(Y::Map(all), anchor, false);
                }
            }
            _ => {}
        }
    }
}

/// `yaml.safe_load(text)` (the first document)
pub fn safe_load(text: &str) -> Result<Y, String> {
    let mut b = Builder::default();
    Parser::new_from_str(text).load(&mut b, false).map_err(|e| e.to_string())?;
    if let Some(e) = b.error {
        return Err(e);
    }
    Ok(b.doc.unwrap_or(Y::Null))
}

// --- clash.py ---

pub fn looks_like_clash(text: &str) -> bool {
    static RE: LazyLock<regex_lite::Regex> = LazyLock::new(|| regex_lite::Regex::new(r"(?m)^proxies\s*:").unwrap());
    RE.is_match(text)
}

fn load(text: &str) -> Result<Vec<Y>, String> {
    let doc = safe_load(text).map_err(|e| {
        tr("The Clash subscription is not valid YAML: {e}").replace("{e}", e.lines().next().unwrap_or(""))
    })?;
    match doc.get("proxies") {
        Some(Y::Seq(list)) if matches!(doc, Y::Map(_)) => Ok(list.clone()),
        _ => Err(tr("The Clash subscription has no 'proxies' list")),
    }
}

/// `_mbps`: '100 Mbps', '1 Gbps', 100 -> Mbps (0 if unknown); None where Python raises
fn mbps(v: Option<&Y>) -> Option<i64> {
    static RE: LazyLock<regex_lite::Regex> =
        LazyLock::new(|| regex_lite::Regex::new(r"^\s*([\d.]+)\s*([kmg]?)(?:bps|b)?\s*$").unwrap());
    let text = s(v).to_lowercase();
    let Some(m) = RE.captures(&text) else { return Some(0) };
    let n: f64 = m[1].parse().ok()?;
    let n = n * match &m[2] {
        "k" => 0.001,
        "g" => 1000.0,
        _ => 1.0,
    };
    if !n.is_finite() {
        return None;
    }
    Some((n.trunc() as i64).clamp(0, 100000))
}

type Fields = Map<String, Value>;

fn tls(p: &Y, sni_key: &str) -> Fields {
    let mut out = Fields::new();
    let sni = s(or(or(p.get(sni_key), p.get("sni")), p.get("servername")));
    if !sni.is_empty() {
        out.insert("sni".into(), json!(sni));
    }
    if b(p.get("skip-cert-verify")) {
        out.insert("tls_verify".into(), json!(false));
    }
    if let Some(Y::Seq(alpn)) = p.get("alpn")
        && !alpn.is_empty()
    {
        out.insert(
            "alpn".into(),
            json!(alpn.iter().map(|x| s(Some(x))).filter(|x| !x.is_empty()).collect::<Vec<_>>().join(",")),
        );
    }
    let fp = s(p.get("client-fingerprint"));
    if !fp.is_empty() {
        out.insert("fingerprint".into(), json!(fp));
    }
    out
}

fn dict(v: Option<&Y>) -> Option<&Y> {
    v.filter(|v| matches!(v, Y::Map(_)))
}

fn transport(p: &Y) -> Option<Fields> {
    let net = {
        let n = s(p.get("network"));
        if n.is_empty() { "tcp".into() } else { n.to_lowercase() }
    };
    let v = match net.as_str() {
        "ws" => {
            let o = dict(p.get("ws-opts"));
            let headers = o.and_then(|o| dict(o.get("headers")));
            let path = s(o.and_then(|o| o.get("path")));
            let host = s(headers.and_then(|h| or(h.get("Host"), h.get("host"))));
            json!({"transport": "ws", "transport_path": if path.is_empty() { "/".into() } else { path }, "transport_host": host})
        }
        "grpc" => {
            json!({"transport": "grpc", "transport_path": s(dict(p.get("grpc-opts")).and_then(|o| o.get("grpc-service-name")))})
        }
        "tcp" => json!({"transport": "tcp"}),
        _ => return None,
    };
    Some(v.as_object().unwrap().clone())
}

fn merged(mut a: Fields, b: Fields) -> Fields {
    for (k, v) in b {
        a.insert(k, v);
    }
    a
}

fn obj(v: Value) -> Fields {
    v.as_object().unwrap().clone()
}

/// `_one(p)`: None where Python raises ValueError / TypeError (the entry is skipped)
fn one(p: &Y) -> Option<Fields> {
    let t = s(p.get("type")).to_lowercase();
    let host = s(p.get("server"));
    let port = p.get("port").and_then(Y::py_int)?;
    let port = port.clamp(i64::MIN as i128, i64::MAX as i128) as i64;
    let base = obj(json!({"host": host.trim_matches(['[', ']']), "port": port}));
    let with = |extra: Value| merged(base.clone(), obj(extra));
    let int_or0 = |k: &str| -> Option<i64> {
        match p.get(k).filter(|v| v.truthy()) {
            None => Some(0),
            Some(v) => v.py_int().map(|i| i.clamp(i64::MIN as i128, i64::MAX as i128) as i64),
        }
    };
    Some(match t.as_str() {
        "ss" => {
            if truthy(p.get("plugin")) {
                return None; // obfs / v2ray-plugin are not supported
            }
            let method = s(p.get("cipher")).to_lowercase();
            if !crate::checks::SS_METHODS.iter().any(|m| m.0 == method) {
                return None; // (the checks only know these ciphers)
            }
            with(json!({"type": "shadowsocks", "ss_method": method, "ss_password": s(p.get("password"))}))
        }
        "vmess" => {
            let cipher = {
                let c = s(p.get("cipher"));
                if c.is_empty() { "auto".into() } else { c.to_lowercase() }
            };
            let aid = int_or0("alterId")?;
            let tls_on = b(p.get("tls"));
            let mut ob = merged(
                with(json!({"type": "vmess", "uuid": s(p.get("uuid")), "vmess_cipher": cipher, "vmess_alter_id": aid,
                                             "security": if tls_on { "tls" } else { "none" }})),
                transport(p)?,
            );
            if tls_on {
                ob = merged(ob, tls(p, "servername"));
            }
            ob
        }
        "vless" => {
            let reality = dict(p.get("reality-opts")).filter(|r| r.truthy());
            let sec = if reality.is_some() {
                "reality"
            } else if b(p.get("tls")) {
                "tls"
            } else {
                "none"
            };
            let mut ob = merged(
                with(
                    json!({"type": "vless", "uuid": s(p.get("uuid")), "vless_flow": s(p.get("flow")), "security": sec}),
                ),
                transport(p)?,
            );
            if sec != "none" {
                ob = merged(ob, tls(p, "servername"));
            }
            if let Some(r) = reality {
                ob.insert("reality_public_key".into(), json!(s(r.get("public-key"))));
                ob.insert("reality_short_id".into(), json!(s(r.get("short-id"))));
                ob.entry("fingerprint").or_insert(json!("chrome"));
            }
            ob
        }
        "trojan" => merged(
            merged(with(json!({"type": "trojan", "password": s(p.get("password"))})), tls(p, "sni")),
            transport(p)?,
        ),
        "hysteria2" => {
            let mut ob = merged(
                with(json!({"type": "hysteria2", "hy2_password": s(or(p.get("password"), p.get("auth")))})),
                tls(p, "sni"),
            );
            ob.shift_remove("fingerprint");
            ob.shift_remove("alpn");
            let obfs = s(p.get("obfs")).to_lowercase();
            if !obfs.is_empty() && obfs != "salamander" {
                return None;
            }
            if !obfs.is_empty() {
                ob.insert("hy2_obfs_password".into(), json!(s(p.get("obfs-password"))));
            }
            ob.insert("hy2_up_mbps".into(), json!(mbps(p.get("up"))?));
            ob.insert("hy2_down_mbps".into(), json!(mbps(p.get("down"))?));
            let ports = s(p.get("ports"));
            if !ports.is_empty() {
                ob.insert("hy2_ports".into(), json!(ports.replace('/', ",")));
                if truthy(p.get("hop-interval")) {
                    ob.insert("hy2_hop_interval".into(), json!(int_or0("hop-interval")?));
                }
            }
            ob
        }
        "tuic" => {
            let mut ob = merged(
                with(json!({"type": "tuic", "uuid": s(p.get("uuid")), "password": s(p.get("password"))})),
                tls(p, "sni"),
            );
            ob.shift_remove("fingerprint");
            let cc = s(p.get("congestion-controller"));
            if !cc.is_empty() {
                ob.insert("tuic_congestion".into(), json!(cc.to_lowercase()));
            }
            let relay = s(p.get("udp-relay-mode"));
            if !relay.is_empty() {
                ob.insert("tuic_udp_relay".into(), json!(relay.to_lowercase()));
            }
            ob
        }
        "socks5" => {
            if b(p.get("tls")) {
                return None;
            }
            with(json!({"type": "socks5", "username": s(p.get("username")), "password": s(p.get("password"))}))
        }
        "http" => {
            let https = b(p.get("tls"));
            let mut ob = with(
                json!({"type": if https { "https" } else { "http" }, "username": s(p.get("username")), "password": s(p.get("password"))}),
            );
            if https {
                ob = merged(ob, tls(p, "sni"));
                ob.shift_remove("fingerprint");
                ob.shift_remove("alpn");
            }
            ob
        }
        "wireguard" => {
            let addrs: Vec<String> = [s(p.get("ip")), s(p.get("ipv6"))]
                .into_iter()
                .filter(|x| !x.is_empty())
                .map(|a| {
                    if a.contains('/') {
                        a
                    } else if a.contains(':') {
                        format!("{a}/128")
                    } else {
                        format!("{a}/32")
                    }
                })
                .collect();
            let mut ob = with(
                json!({"type": "wireguard", "wg_private_key": s(p.get("private-key")), "wg_address": addrs.join(", "),
                                     "wg_peer_public_key": s(p.get("public-key")), "wg_preshared_key": s(p.get("pre-shared-key"))}),
            );
            if let Some(Y::Seq(allowed)) = p.get("allowed-ips")
                && !allowed.is_empty()
            {
                ob.insert(
                    "wg_allowed_ips".into(),
                    json!(allowed.iter().map(|x| s(Some(x))).collect::<Vec<_>>().join(", ")),
                );
            }
            if truthy(p.get("mtu")) {
                ob.insert("wg_mtu".into(), json!(int_or0("mtu")?));
            }
            ob
        }
        _ => return None,
    })
}

/// `proxies(text)` -> ([(fields, name)], skipped), or an error if it is not a Clash file
pub fn proxies(text: &str) -> Result<(Vec<(Fields, String)>, usize), String> {
    let mut out = vec![];
    let mut skipped = 0;
    for p in load(text)? {
        if !matches!(p, Y::Map(_)) {
            skipped += 1;
            continue;
        }
        match one(&p) {
            Some(fields) => {
                let name = crate::py::head(&s(p.get("name")), 60);
                let name = if name.is_empty() { fields["type"].as_str().unwrap_or("").to_string() } else { name };
                out.push((fields, name));
            }
            None => skipped += 1,
        }
    }
    Ok((out, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pyyaml_scalars() {
        let y = safe_load(
            "a: yes\nb: 0o7\nc: 010\nd: 1:30\ne: 1.5\nf: '1'\ng: ~\nh: 1e5\nbase: &b {x: 1}\nm:\n  <<: *b\n  y: 2\n",
        )
        .unwrap();
        assert!(matches!(y.get("a"), Some(Y::Bool(true))));
        assert!(matches!(y.get("b"), Some(Y::Str(_))));
        assert!(matches!(y.get("c"), Some(Y::Int(8))));
        assert!(matches!(y.get("d"), Some(Y::Int(90))));
        assert!(matches!(y.get("e"), Some(Y::Float(_))));
        assert!(matches!(y.get("f"), Some(Y::Str(_))));
        assert!(matches!(y.get("g"), Some(Y::Null)));
        assert!(matches!(y.get("h"), Some(Y::Str(_))));
        let m = y.get("m").unwrap();
        assert!(matches!(m.get("x"), Some(Y::Int(1))) && matches!(m.get("y"), Some(Y::Int(2))));
    }

    #[test]
    fn clash_proxies() {
        let text = "proxies:\n  - {name: a, type: ss, server: 1.2.3.4, port: 8388, cipher: AES-256-GCM, password: on}\n  - {name: b, type: foo, server: x, port: 1}\n  - {name: c, type: vless, server: h, port: '443', uuid: u, tls: true, reality-opts: {public-key: K}, network: ws, ws-opts: {path: /p, headers: {Host: cdn}}}\n";
        let (list, skipped) = proxies(text).unwrap();
        assert_eq!(skipped, 1);
        assert_eq!(list[0].0["ss_password"], "True");
        assert_eq!(list[0].0["ss_method"], "aes-256-gcm");
        assert_eq!(list[1].0["security"], "reality");
        assert_eq!(list[1].0["transport_host"], "cdn");
        assert!(proxies("proxies: 3").is_err());
    }
}
