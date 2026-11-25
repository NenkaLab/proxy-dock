//! Share links -> outbound settings: vless://, vmess://, trojan://, tuic://, ss://, hysteria2:// (hy2://)
//! (Python: `links.py`). `parse(link)` -> (outbound fields, name); the fields go over the outbound defaults.

use crate::i18n::tr;
use crate::py;
use serde_json::{json, Map, Value};
use std::collections::HashMap;

pub const SCHEMES: [&str; 7] = ["vless", "vmess", "trojan", "tuic", "ss", "hysteria2", "hy2"];

type Fields = Map<String, Value>;

/// `links._b64`: URL-safe base64 with the padding added, as UTF-8 text
fn b64(text: &str) -> Result<String, String> {
    let text = py::strip(text);
    let padded = format!("{text}{}", "=".repeat((4 - text.chars().count() % 4) % 4));
    let raw = py::urlsafe_b64decode_msg(&padded)?;
    py::utf8(&raw)
}

fn truthy(v: Option<&String>) -> bool {
    matches!(v.map(|s| s.to_lowercase()).as_deref(), Some("1" | "true" | "yes"))
}

fn bad_address() -> String {
    tr("The link has no valid server address and port")
}

/// `port.isdigit() and 0 < int(port) < 65536`
fn port_of(port: &str) -> Option<u16> {
    if port.is_empty() || !port.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let t = port.trim_start_matches('0');
    if t.len() > 5 {
        return None;
    }
    let n: u32 = if t.is_empty() { 0 } else { t.parse().ok()? };
    (1..65536).contains(&n).then_some(n as u16)
}

fn host_port(netloc: &str) -> Result<(String, u16), String> {
    let hp = rsplit_last(netloc, '@');
    let (host, port) = match hp.strip_prefix('[') {
        Some(rest) => {
            let (h, r) = rest.split_once(']').unwrap_or((rest, ""));
            (h, r.trim_start_matches(':'))
        }
        None => hp.rsplit_once(':').unwrap_or(("", hp)),
    };
    match port_of(port) {
        Some(p) if !host.is_empty() => Ok((host.to_string(), p)),
        _ => Err(bad_address()),
    }
}

/// `s.rsplit(sep, 1)[-1]`
fn rsplit_last(s: &str, sep: char) -> &str {
    s.rsplit_once(sep).map_or(s, |(_, b)| b)
}

/// `s.rsplit(sep, 1)[0]`
fn rsplit_first(s: &str, sep: char) -> &str {
    s.rsplit_once(sep).map_or(s, |(a, _)| a)
}

fn tls_fields(q: &HashMap<String, String>) -> Fields {
    let mut out = Fields::new();
    let get = |k: &str| q.get(k).filter(|v| !v.is_empty());
    if let Some(sni) = get("sni").or(get("peer")) {
        out.insert("sni".into(), json!(sni));
    }
    if truthy(q.get("allowInsecure")) || truthy(q.get("insecure")) || truthy(q.get("allow_insecure")) {
        out.insert("tls_verify".into(), json!(false));
    }
    if let Some(a) = get("alpn") {
        out.insert("alpn".into(), json!(a.replace(' ', "")));
    }
    if let Some(fp) = q.get("fp") {
        out.insert("fingerprint".into(), json!(fp));
    }
    out
}

fn unsupported_transport(t: &str) -> String {
    tr("Transport '{value}' is not supported (TCP, WebSocket and gRPC are)").replace("{value}", t)
}

fn transport_fields(q: &HashMap<String, String>) -> Result<Fields, String> {
    let t = q.get("type").filter(|v| !v.is_empty()).map_or("tcp".to_string(), |t| t.to_lowercase());
    let get = |k: &str, d: &str| q.get(k).cloned().unwrap_or_else(|| d.to_string());
    let v = match t.as_str() {
        "ws" => json!({"transport": "ws", "transport_path": get("path", "/"), "transport_host": get("host", "")}),
        "grpc" => {
            let path = q.get("serviceName").filter(|v| !v.is_empty()).cloned().unwrap_or_else(|| get("path", ""));
            json!({"transport": "grpc", "transport_path": path})
        }
        "tcp" | "raw" | "" => json!({"transport": "tcp"}),
        _ => return Err(unsupported_transport(&t)),
    };
    Ok(v.as_object().unwrap().clone())
}

fn merge(a: &mut Fields, b: Fields) {
    for (k, v) in b {
        a.insert(k, v);
    }
}

pub fn not_a_link() -> String {
    tr("Not a share link (vless://, vmess://, trojan://, tuic://, ss:// or hysteria2://)")
}

/// A share link -> (outbound fields, name), or a readable error.
pub fn parse(link: &str) -> Result<(Fields, String), String> {
    let link = py::strip(link);
    let scheme = link.split_once("://").map_or(String::new(), |(s, _)| s.to_lowercase());
    if !SCHEMES.contains(&scheme.as_str()) {
        return Err(not_a_link());
    }
    if scheme == "vmess" {
        return vmess(link);
    }
    let u = py::urlsplit(link)?;
    let mut q: HashMap<String, String> = HashMap::new();
    for (k, v) in py::parse_qsl(&u.query) {
        q.entry(k).or_insert(v);
    }
    let name = py::strip(&py::unquote(&u.fragment)).to_string();
    let user = if u.netloc.contains('@') { py::unquote(rsplit_first(&u.netloc, '@')) } else { String::new() };
    let named = |n: &str| if name.is_empty() { n.to_string() } else { name.clone() };
    let obj = |v: Value| v.as_object().unwrap().clone();

    if scheme == "ss" {
        let (userinfo, host, port) = if !u.netloc.contains('@') {
            // legacy ss://base64(method:password@host:port)
            let decoded = b64(&u.netloc)?;
            let (userinfo, hp) = decoded.rsplit_once('@').unwrap_or(("", &decoded));
            let (h, p) = host_port(hp)?;
            (userinfo.to_string(), h, p)
        } else {
            let (h, p) = host_port(&u.netloc)?;
            let raw = rsplit_first(&u.netloc, '@');
            let plain = py::unquote(raw);
            (if plain.contains(':') { plain } else { b64(raw)? }, h, p)
        };
        let (method, password) = userinfo.split_once(':').unwrap_or((&userinfo, ""));
        if !crate::checks::SS_METHODS.iter().any(|m| m.0 == method) {
            return Err(tr("Shadowsocks cipher '{value}' is not supported").replace("{value}", method));
        }
        return Ok((obj(json!({"type": "shadowsocks", "host": host, "port": port, "ss_method": method, "ss_password": password})), named("Shadowsocks")));
    }

    if scheme == "hysteria2" || scheme == "hy2" {
        // the official format allows port hopping in the address: host:443,5000-6000
        let hp = rsplit_last(&u.netloc, '@');
        let (hosts, mut ports) = match hp.strip_prefix('[') {
            Some(rest) => (rest.split_once(']').map_or(rest, |(a, _)| a).to_string(), hp.split_once(']').map_or("", |(_, b)| b).trim_start_matches(':').to_string()),
            None => match hp.rsplit_once(':') {
                Some((a, b)) => (a.to_string(), b.to_string()),
                None => (String::new(), hp.to_string()),
            },
        };
        let mut hop = q.get("mport").cloned().unwrap_or_default();
        if !ports.is_empty() && !ports.bytes().all(|c| c.is_ascii_digit()) {
            hop = ports.clone();
            ports = ports.replace('-', ",").split(',').next().unwrap_or("").to_string();
        }
        let (host, port) = host_port(&if hosts.contains(':') { format!("[{hosts}]:{ports}") } else { format!("{hosts}:{ports}") })?;
        let mut ob = obj(json!({"type": "hysteria2", "host": host, "port": port, "hy2_password": user}));
        merge(&mut ob, tls_fields(&q));
        ob.shift_remove("fingerprint");
        ob.shift_remove("alpn");
        if let Some(pin) = q.get("pinSHA256").filter(|v| !v.is_empty()) {
            ob.insert("pin_sha256".into(), json!(pin));
        }
        if let Some(obfs) = q.get("obfs").filter(|v| !v.is_empty()) {
            if obfs.to_lowercase() != "salamander" {
                return Err(tr("Hysteria2 obfuscation '{value}' is not supported").replace("{value}", obfs));
            }
            ob.insert("hy2_obfs_password".into(), json!(q.get("obfs-password").cloned().unwrap_or_default()));
        }
        if !hop.is_empty() {
            ob.insert("hy2_ports".into(), json!(hop));
        }
        return Ok((ob, named("Hysteria2")));
    }

    let (host, port) = host_port(&u.netloc)?;

    if scheme == "vless" {
        let sec = q.get("security").filter(|v| !v.is_empty()).map_or("none".to_string(), |s| s.to_lowercase());
        if !matches!(sec.as_str(), "none" | "tls" | "reality") {
            return Err(tr("VLESS security '{value}' is not supported").replace("{value}", &sec));
        }
        let mut ob = obj(json!({"type": "vless", "host": host, "port": port, "uuid": user, "security": sec,
                                "vless_flow": q.get("flow").cloned().unwrap_or_default()}));
        merge(&mut ob, tls_fields(&q));
        merge(&mut ob, transport_fields(&q)?);
        if sec == "reality" {
            ob.insert("reality_public_key".into(), json!(q.get("pbk").cloned().unwrap_or_default()));
            ob.insert("reality_short_id".into(), json!(q.get("sid").cloned().unwrap_or_default()));
            ob.entry("fingerprint").or_insert(json!("chrome"));
        }
        return Ok((ob, named("VLESS")));
    }

    if scheme == "trojan" {
        let mut ob = obj(json!({"type": "trojan", "host": host, "port": port, "password": user}));
        merge(&mut ob, tls_fields(&q));
        merge(&mut ob, transport_fields(&q)?);
        return Ok((ob, named("Trojan")));
    }

    // tuic://uuid:password@host:port
    let (uid, pw) = user.split_once(':').unwrap_or((&user, ""));
    let mut ob = obj(json!({"type": "tuic", "host": host, "port": port, "uuid": uid, "password": pw}));
    merge(&mut ob, tls_fields(&q));
    ob.shift_remove("fingerprint");
    if let Some(c) = q.get("congestion_control").filter(|v| !v.is_empty()) {
        ob.insert("tuic_congestion".into(), json!(c));
    }
    if let Some(r) = q.get("udp_relay_mode").filter(|v| !v.is_empty()) {
        ob.insert("tuic_udp_relay".into(), json!(r));
    }
    Ok((ob, named("TUIC")))
}

/// vmess://base64(JSON) as v2rayN writes it: add, port, id, aid, scy, net, type, host, path, tls, sni, alpn, fp, ps
fn vmess(link: &str) -> Result<(Fields, String), String> {
    let unreadable = || tr("The vmess:// link is not readable (expected base64 JSON)");
    let body = link.split_once("://").map_or("", |(_, b)| b);
    let body = body.split('#').next().unwrap_or("");
    let text = b64(body).map_err(|_| unreadable())?;
    let d = match py::json_loads(&text) {
        Some(Value::Object(d)) => d,
        _ => return Err(unreadable()),
    };
    let get = |k: &str| match d.get(k) {
        None | Some(Value::Null) => String::new(),
        Some(v) => py::strip(&crate::singbox::compile::py_str(v)).to_string(),
    };
    let (host, port) = (get("add"), get("port"));
    let Some(port) = port_of(&port).filter(|_| !host.is_empty()) else { return Err(bad_address()) };
    let net = { let n = get("net"); if n.is_empty() { "tcp".to_string() } else { n.to_lowercase() } };
    let ty = get("type");
    if net == "tcp" && !matches!(ty.to_lowercase().as_str(), "" | "none") {
        return Err(unsupported_transport(&format!("tcp/{ty}")));
    }
    let path = get("path");
    let mut q = HashMap::new();
    q.insert("type".to_string(), net.clone());
    q.insert("path".to_string(), if path.is_empty() { if net == "ws" { "/".into() } else { String::new() } } else { path.clone() });
    q.insert("host".to_string(), get("host"));
    q.insert("serviceName".to_string(), path);
    let scy = get("scy").to_lowercase();
    let aid = get("aid");
    let tls = get("tls").to_lowercase() == "tls";
    let mut ob = json!({"type": "vmess", "host": host.trim_matches(['[', ']']), "port": port, "uuid": get("id"),
        "vmess_cipher": if scy.is_empty() { "auto".to_string() } else { scy },
        "vmess_alter_id": if !aid.is_empty() && aid.bytes().all(|c| c.is_ascii_digit()) { aid.parse::<i64>().unwrap_or(i64::MAX) } else { 0 },
        "security": if tls { "tls" } else { "none" }}).as_object().unwrap().clone();
    merge(&mut ob, transport_fields(&q)?);
    if tls {
        let mut t = HashMap::new();
        let sni = get("sni");
        t.insert("sni".to_string(), if sni.is_empty() { get("host") } else { sni });
        t.insert("alpn".to_string(), get("alpn"));
        if !get("fp").is_empty() {
            t.insert("fp".to_string(), get("fp"));
        }
        merge(&mut ob, tls_fields(&t));
    }
    let ps = get("ps");
    Ok((ob, if ps.is_empty() { "VMess".into() } else { ps }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        let (ob, name) = parse("vless://11111111-2222-3333-4444-555555555555@example.com:443?security=reality&pbk=KEY&sid=ab&sni=www.apple.com&type=grpc&serviceName=svc#My%20Server").unwrap();
        assert_eq!(name, "My Server");
        assert_eq!(ob["security"], "reality");
        assert_eq!(ob["fingerprint"], "chrome");
        assert_eq!(ob["transport_path"], "svc");
        let (ob, _) = parse("hy2://pw@[2001:db8::1]:443,5000-6000/?sni=a.com&obfs=salamander&obfs-password=x").unwrap();
        assert_eq!((ob["host"].as_str(), ob["port"].as_i64(), ob["hy2_ports"].as_str()), (Some("2001:db8::1"), Some(443), Some("443,5000-6000")));
        let (ob, name) = parse("ss://YWVzLTI1Ni1nY206cGFzcw@1.2.3.4:8388").unwrap();
        assert_eq!((ob["ss_method"].as_str(), ob["ss_password"].as_str(), name.as_str()), (Some("aes-256-gcm"), Some("pass"), "Shadowsocks"));
        assert!(parse("http://x").is_err());
        assert!(parse("trojan://p@host").is_err());
    }
}
