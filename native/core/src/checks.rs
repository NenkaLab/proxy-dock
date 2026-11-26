//! What has to be fixed before a block is saved or a card starts (Python: `config.check_proxy`,
//! `config.outbound_errors`, `config.wg_server_errors`, `ui/router.match_errors`). Messages as the Python app.

use crate::config::dns_server_spec;
use crate::i18n::tr;
use crate::py;
use serde_json::Value;
use std::net::IpAddr;

/// cipher -> (key length, 2022)
pub const SS_METHODS: [(&str, usize, bool); 6] = [
    ("2022-blake3-aes-128-gcm", 16, true),
    ("2022-blake3-aes-256-gcm", 32, true),
    ("2022-blake3-chacha20-poly1305", 32, true),
    ("chacha20-ietf-poly1305", 32, false),
    ("aes-256-gcm", 32, false),
    ("aes-128-gcm", 16, false),
];
pub const VMESS_CIPHERS: [&str; 5] = ["auto", "aes-128-gcm", "chacha20-poly1305", "none", "zero"];
const NEEDS_UDP_VIA: [&str; 2] = ["wireguard", "tuic"];
const UDP_VIA_OK: [&str; 9] =
    ["shadowsocks", "hysteria2", "wireguard", "socks5", "vless", "vmess", "trojan", "tuic", "subscription"];

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn n(v: &Value, k: &str) -> i64 {
    v[k].as_i64().unwrap_or(0)
}

fn t1(text: &str, key: &str, val: &str) -> String {
    tr(text).replace(&format!("{{{key}}}"), val)
}

/// Python `ss_make_key` for the checks: Err(message) when the password does not make a key
pub fn ss_key_error(method: &str, password: &str) -> Option<String> {
    let (_, key_len, v2022) = SS_METHODS.iter().find(|m| m.0 == method).copied().unwrap_or(("", 32, false));
    if !v2022 {
        return None;
    }
    let Some(psk) = py::b64decode_strict(py::strip(password)) else {
        return Some(tr("The 2022 password must be a base64 key (use the 'Generate' button)"));
    };
    (psk.len() != key_len).then(|| {
        tr("The 2022 key must be {key_len} bytes (currently {len} bytes)")
            .replace("{key_len}", &key_len.to_string())
            .replace("{len}", &psk.len().to_string())
    })
}

/// `wgconf.valid_key`: base64 of 32 bytes
pub fn valid_wg_key(text: &str) -> bool {
    py::b64decode_strict(py::strip(text)).is_some_and(|k| k.len() == 32)
}

/// `wgconf.valid_cidrs`: a list of IP ranges ("a, b; c"), each with a prefix
pub fn valid_cidrs(text: &str) -> bool {
    let items = crate::singbox::compile::split(text);
    !items.is_empty() && items.iter().all(|x| x.contains('/') && py::ip_interface(x).is_some())
}

/// `uuid.UUID(text)`: 32 hex digits, with or without dashes, braces, "urn:uuid:"
pub fn valid_uuid(text: &str) -> bool {
    let h = text.replace("urn:", "").replace("uuid:", "");
    let h = h.trim_matches(['{', '}']).replace('-', "");
    h.chars().count() == 32 && py::int16_ok(&h)
}

fn valid_listen_host(h: &str) -> bool {
    h.eq_ignore_ascii_case("localhost") || py::ip_address(h).is_some()
}

fn bind_addrs(host: &str) -> Vec<IpAddr> {
    if host.eq_ignore_ascii_case("localhost") {
        return vec!["127.0.0.1".parse().unwrap(), "::1".parse().unwrap()];
    }
    py::ip_address(host).into_iter().collect()
}

/// Python `_binds_overlap`: the two addresses cannot be bound on the same port at once
fn binds_overlap(h1: &str, h2: &str, udp: bool) -> bool {
    let addrs = |h: &str| {
        let mut out = bind_addrs(h);
        if udp && out.iter().any(|a| a.is_ipv6() && a.is_unspecified()) {
            out.push("0.0.0.0".parse().unwrap());
        }
        out
    };
    let (a, b) = (addrs(h1), addrs(h2));
    a.iter()
        .any(|x| b.iter().any(|y| x.is_ipv4() == y.is_ipv4() && (x == y || x.is_unspecified() || y.is_unspecified())))
}

fn transports(ib: &Value) -> Vec<&'static str> {
    match s(ib, "type") {
        "forward" => match s(ib, "fwd_network") {
            "both" => vec!["tcp", "udp"],
            "udp" => vec!["udp"],
            _ => vec!["tcp"],
        },
        "socks5" | "shadowsocks" => vec!["tcp", "udp"],
        "hysteria2" | "wireguard" => vec!["udp"],
        _ => vec!["tcp"],
    }
}

/// WireGuard server inbound (Python `wg_server_errors`)
pub fn wg_server_errors(ib: &Value) -> Vec<String> {
    let mut errs = vec![];
    if !valid_wg_key(s(ib, "wg_private_key")) {
        errs.push(tr("WireGuard server: generate the server key"));
    }
    let net = py::ip_interface(py::strip(s(ib, "wg_address"))).filter(|(a, p)| *p < if a.is_ipv4() { 31 } else { 127 });
    if net.is_none() {
        errs.push(tr("WireGuard server: the tunnel address must be like 10.66.0.1/24"));
    }
    let peers: Vec<Value> =
        match serde_json::from_str::<Value>(if s(ib, "wg_peers").is_empty() { "[]" } else { s(ib, "wg_peers") }) {
            Ok(Value::Array(a)) => a.into_iter().filter(|x| x.is_object() && x["public_key"].is_string()).collect(),
            _ => vec![],
        };
    if peers.is_empty() {
        errs.push(tr("WireGuard server: add at least one device"));
    }
    let in_net = |a: IpAddr| -> bool {
        let Some((n, p)) = net else { return true };
        match (n, a) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let m = if p == 0 { 0 } else { u32::MAX << (32 - p) };
                u32::from(n) & m == u32::from(a) & m
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let m = if p == 0 { 0 } else { u128::MAX << (128 - p) };
                u128::from(n) & m == u128::from(a) & m
            }
            _ => false,
        }
    };
    let mut seen = vec![];
    for x in &peers {
        let name = x.get("name").map_or_else(|| "?".to_string(), crate::singbox::compile::py_str);
        if !valid_wg_key(x["public_key"].as_str().unwrap_or("")) {
            errs.push(t1("WireGuard server: '{name}' has no valid public key", "name", &name));
        }
        let addr = x["address"].as_str().and_then(py::ip_interface).map(|(a, _)| a);
        let ok = addr.is_some_and(|a| in_net(a) && net.is_none_or(|(n, _)| a != n) && !seen.contains(&a));
        if let Some(a) = addr.filter(|_| ok) {
            seen.push(a);
        } else {
            errs.push(
                tr("WireGuard server: '{name}' needs its own address inside {net}")
                    .replace("{name}", &name)
                    .replace("{net}", s(ib, "wg_address")),
            );
        }
    }
    let mtu = n(ib, "wg_mtu");
    if !(576..=9000).contains(&mtu) {
        errs.push(tr("WireGuard: MTU must be 576 to 9000"));
    }
    errs
}

/// Problems of a resolved outbound (with its via chain and group members), Python `outbound_errors`.
pub fn outbound_errors(ob: &Value) -> Vec<String> {
    let mut errs = vec![];
    let t = s(ob, "type");
    if let Some(via) = ob.get("via").filter(|v| v.is_object()) {
        if t == "hysteria2" {
            errs.push(tr("Hysteria2 uses QUIC (UDP), so it cannot go through another outbound"));
        }
        if NEEDS_UDP_VIA.contains(&t) && !UDP_VIA_OK.contains(&s(via, "type")) {
            errs.push(
                tr("{type} needs UDP, so it cannot go through an HTTP or SOCKS4 proxy")
                    .replace("{type}", &crate::graph::outbound_label(t)),
            );
        }
        errs.extend(outbound_errors(via));
    }
    match t {
        "block" => {}
        "group" => {
            let members = ob["members"].as_array().cloned().unwrap_or_default();
            if members.is_empty() {
                errs.push(tr("Connect at least one outbound block to the group's 'members' output"));
            }
            let url = s(ob, "group_test_url").trim().to_lowercase();
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                errs.push(tr("Group: the test address must start with http:// or https://"));
            }
            if !(10..=3600).contains(&n(ob, "group_interval")) {
                errs.push(tr("Group: the interval must be 10 to 3600 seconds"));
            }
            if !(0..=5000).contains(&n(ob, "group_tolerance")) {
                errs.push(tr("Group: the tolerance must be 0 to 5000 ms"));
            }
            for m in &members {
                errs.extend(outbound_errors(&m["outbound"]).into_iter().map(|e| format!("{}: {e}", s(m, "name"))));
            }
        }
        "subscription" => {
            let url = s(ob, "sub_url").trim().to_lowercase();
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                errs.push(tr("Enter the subscription address (https://...)"));
            }
            if crate::singbox::compile::servers_of(ob).is_empty() {
                errs.push(tr("The subscription has no servers yet - click 'Update now'"));
            }
        }
        "direct" => {
            if !s(ob, "bind").is_empty() && !valid_listen_host(s(ob, "bind")) {
                errs.push(tr("The source address must be an IP of this PC"));
            }
        }
        _ => {
            if py::strip(s(ob, "host")).is_empty() || !(1..=65535).contains(&n(ob, "port")) {
                errs.push(tr("Enter the outbound server address and port"));
            }
            if t == "shadowsocks" {
                let e = ss_key_error(s(ob, "ss_method"), s(ob, "ss_password"))
                    .or_else(|| s(ob, "ss_password").is_empty().then(|| tr("Enter the Shadowsocks password")));
                if let Some(e) = e {
                    errs.push(t1("Outbound: {e}", "e", &e));
                }
            }
            if t == "hysteria2" {
                if s(ob, "hy2_password").is_empty() {
                    errs.push(tr("Enter the outbound Hysteria2 password"));
                }
                if !py::strip(s(ob, "hy2_ports")).is_empty() {
                    if crate::config::hop_ports(s(ob, "hy2_ports")).is_none() {
                        errs.push(tr("Port hopping: use ports or ranges like 20000-30000 or 443,5000-6000"));
                    }
                    if !(5..=3600).contains(&n(ob, "hy2_hop_interval")) {
                        errs.push(tr("Port hopping: the interval must be 5 to 3600 seconds"));
                    }
                }
                if !((0..=100000).contains(&n(ob, "hy2_up_mbps")) && (0..=100000).contains(&n(ob, "hy2_down_mbps"))) {
                    errs.push(tr("Bandwidth must be 0 to 100000 Mbps"));
                }
            }
            if t == "vmess" {
                if !["none", "tls"].contains(&s(ob, "security")) {
                    errs.push(tr("VMess security: choose none or TLS"));
                }
                if !VMESS_CIPHERS.contains(&s(ob, "vmess_cipher")) {
                    errs.push(tr("Choose the VMess encryption"));
                }
                if !(0..=65535).contains(&n(ob, "vmess_alter_id")) {
                    errs.push(tr("VMess alterId must be 0 to 65535"));
                }
            }
            if ["vless", "vmess", "tuic"].contains(&t) && !valid_uuid(py::strip(s(ob, "uuid"))) {
                errs.push(tr("Enter the user UUID (e.g. 0b5b8d9e-...)"));
            }
            if ["trojan", "tuic"].contains(&t) && s(ob, "password").is_empty() {
                errs.push(tr("Enter the password"));
            }
            if t == "vless" {
                if !["none", "tls", "reality"].contains(&s(ob, "security")) {
                    errs.push(tr("Choose the VLESS security (none, TLS or REALITY)"));
                }
                if s(ob, "security") == "reality" {
                    let pbk = format!("{}==", py::strip(s(ob, "reality_public_key")));
                    if !pbk.is_ascii() || py::urlsafe_b64decode(&pbk).is_none_or(|k| k.len() != 32) {
                        errs.push(tr("REALITY: enter the server's public key"));
                    }
                    let sid = py::strip(s(ob, "reality_short_id"));
                    if sid.chars().count() > 16
                        || sid.chars().count() % 2 == 1
                        || !sid.chars().all(|c| c.is_ascii_hexdigit())
                    {
                        errs.push(tr("REALITY: the short ID is up to 16 hex digits"));
                    }
                    if py::strip(s(ob, "sni")).is_empty() {
                        errs.push(tr("REALITY: enter the server name (SNI) of the site the server imitates"));
                    }
                    if s(ob, "fingerprint").is_empty() {
                        errs.push(tr("REALITY needs a browser fingerprint"));
                    }
                }
            }
            if ["vless", "vmess", "trojan"].contains(&t) && !["tcp", "ws", "grpc"].contains(&s(ob, "transport")) {
                errs.push(tr("Choose the transport (TCP, WebSocket or gRPC)"));
            }
            if t == "wireguard" {
                if !valid_wg_key(s(ob, "wg_private_key")) {
                    errs.push(tr("WireGuard: enter a valid private key (base64, or click Generate)"));
                }
                if !valid_wg_key(s(ob, "wg_peer_public_key")) {
                    errs.push(tr("WireGuard: enter the server's public key (base64)"));
                }
                if !s(ob, "wg_preshared_key").is_empty() && !valid_wg_key(s(ob, "wg_preshared_key")) {
                    errs.push(tr("WireGuard: the preshared key is not valid (base64, 32 bytes)"));
                }
                if !valid_cidrs(s(ob, "wg_address")) {
                    errs.push(tr("WireGuard: enter this side's tunnel address, e.g. 10.0.0.2/32"));
                }
                if !valid_cidrs(s(ob, "wg_allowed_ips")) {
                    errs.push(tr("WireGuard: allowed IPs must be IP ranges, e.g. 0.0.0.0/0, ::/0"));
                }
                if !(576..=9200).contains(&n(ob, "wg_mtu")) {
                    errs.push(tr("WireGuard: MTU must be between 576 and 9200"));
                }
            }
            if t == "socks4" && !s(ob, "password").is_empty() {
                errs.push(tr("SOCKS4 does not support passwords (user ID only)"));
            }
        }
    }
    errs
}

/// Python `config.find_cycle`: following outbounds that point at the local inbound of a card of this app, do we
/// come back to p? (the names on the way)
pub fn find_cycle(p: &Value, proxies: &[Value]) -> Option<Vec<String>> {
    let local = |h: &str| -> bool {
        let h = h.trim().to_lowercase();
        let h = h.trim_end_matches('.');
        h == "localhost"
            || h.ends_with(".localhost")
            || py::ip_address(h.trim_matches(['[', ']'])).is_some_and(|a| a.is_loopback() || a.is_unspecified())
    };
    let next = |cur: &Value| -> Option<Value> {
        let ob = &cur["outbound"];
        let oh = s(ob, "host").trim().trim_matches(['[', ']']).to_string();
        if s(ob, "type") == "direct" || oh.is_empty() {
            return None;
        }
        proxies
            .iter()
            .filter(|q| q["id"] != p["id"])
            .chain(std::iter::once(p))
            .filter(|q| q["inbound"]["port"] == ob["port"])
            .find(|q| {
                let ih = s(&q["inbound"], "host");
                oh.to_lowercase() == ih.to_lowercase() || (local(&oh) && local(ih))
            })
            .cloned()
    };
    let mut path = vec![s(p, "name").to_string()];
    let mut seen = vec![p["id"].clone()];
    let mut cur = p.clone();
    loop {
        let nx = next(&cur)?;
        path.push(s(&nx, "name").to_string());
        if nx["id"] == p["id"] {
            return Some(path);
        }
        if seen.contains(&nx["id"]) {
            return None;
        }
        seen.push(nx["id"].clone());
        cur = nx;
    }
}

/// Problems to fix before saving / starting a card (Python `check_proxy`); `others` = the other cards.
pub fn check_proxy(p: &Value, others: &[Value]) -> Vec<String> {
    let mut errs = vec![];
    let (ib, ob) = (&p["inbound"], &p["outbound"]);
    if py::strip(s(p, "name")).is_empty() {
        errs.push(tr("Enter a name"));
    }
    let dns = p["overrides"]["dns_server"].as_str().filter(|d| !d.is_empty()).unwrap_or("");
    if dns_server_spec(dns).is_none() {
        errs.push(tr(
            "DNS server: use an address like 1.1.1.1, tls://dns.google or https://cloudflare-dns.com/dns-query",
        ));
    }
    if p["unlinked"] == Value::Bool(true) {
        errs.push(unlinked_error());
    }
    let it = s(ib, "type");
    let tun = it == "tun";
    if !tun && !valid_listen_host(s(ib, "host")) {
        errs.push(tr("The inbound address must be an IP (e.g. 127.0.0.1, 0.0.0.0, ::)"));
    }
    if !tun && !(1..=65535).contains(&n(ib, "port")) {
        errs.push(tr("Invalid inbound port"));
    }
    if ib["auth_enabled"] == Value::Bool(true) && ["socks5", "http", "https"].contains(&it) {
        let ok = |k: &str| (1..=255).contains(&s(ib, k).len());
        if !(ok("username") && ok("password")) {
            errs.push(tr("Enter a username and password to enable authentication (255 bytes max each)"));
        }
    }
    if it == "shadowsocks" {
        let method = s(ib, "ss_method");
        let v2022 = SS_METHODS.iter().any(|m| m.0 == method && m.2);
        let e = if !v2022 && s(ib, "ss_password").chars().count() < 8 {
            Some(tr("The Shadowsocks password must be at least 8 characters"))
        } else {
            ss_key_error(method, s(ib, "ss_password"))
        };
        if let Some(e) = e {
            errs.push(t1("Inbound: {e}", "e", &e));
        }
    }
    if it == "wireguard" {
        errs.extend(wg_server_errors(ib));
    }
    if it == "forward" {
        if py::strip(s(ib, "fwd_host")).is_empty() || !(1..=65535).contains(&n(ib, "fwd_port")) {
            errs.push(tr("Port forward: enter the destination address and port"));
        }
        if !["tcp", "udp", "both"].contains(&s(ib, "fwd_network")) {
            errs.push(tr("Port forward: choose TCP, UDP or both"));
        }
    }
    if it == "hysteria2" && s(ib, "hy2_password").chars().count() < 8 {
        errs.push(tr("The Hysteria2 password must be at least 8 characters"));
    }
    if ["https", "hysteria2"].contains(&it) {
        if s(ib, "tls_cert").is_empty() != s(ib, "tls_key").is_empty() {
            errs.push(tr("Set both the certificate and key files, or leave both empty (generated automatically)"));
        }
        for f in ["tls_cert", "tls_key"] {
            if !s(ib, f).is_empty() && !std::path::Path::new(s(ib, f)).is_file() {
                errs.push(t1("File not found: {ib}", "ib", s(ib, f)));
            }
        }
    }
    errs.extend(outbound_errors(ob));
    let mine = transports(ib);
    for other in others.iter().filter(|o| o["id"] != p["id"]) {
        let oi = &other["inbound"];
        if tun || s(oi, "type") == "tun" {
            continue;
        }
        let shared: Vec<&str> = transports(oi).into_iter().filter(|t| mine.contains(t)).collect();
        if oi["port"] != ib["port"] || shared.is_empty() {
            continue;
        }
        if shared.iter().any(|t| binds_overlap(s(ib, "host"), s(oi, "host"), *t == "udp")) {
            errs.push(
                tr("Port {port} conflicts with '{name}'")
                    .replace("{name}", s(other, "name"))
                    .replace("{port}", &n(ib, "port").to_string()),
            );
        }
    }
    let mut all: Vec<Value> = others.iter().filter(|o| o["id"] != p["id"]).cloned().collect();
    all.push(p.clone());
    if let Some(cycle) = find_cycle(p, &all) {
        errs.push(format!(
            "{}{}{}",
            tr("The proxy chain loops: "),
            cycle.join(" → "),
            tr(" (check the outbound address/port)")
        ));
    }
    errs
}

pub fn unlinked_error() -> String {
    tr("Not connected to an outbound - connect it on the canvas or choose one in the editor")
}

/// One router rule's conditions (Python `match_errors`).
pub fn match_errors(name: &str, m: &Value) -> Vec<String> {
    let list = |k: &str| {
        m[k].as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect::<Vec<_>>()
    };
    let e = |t: &str, v: &str| tr(t).replace("{name}", name).replace("{value}", v);
    let mut errs = vec![];
    for x in list("ip_cidr") {
        if py::ip_interface(&x).is_none() {
            errs.push(e("Rule '{name}': '{value}' is not an IP or IP range", &x));
        }
    }
    for x in list("port") {
        let (a, b) = x.split_once('-').unwrap_or((&x, ""));
        let lo = py::int(a);
        let hi = if b.is_empty() { lo } else { py::int(b) };
        if !matches!((lo, hi), (Some(lo), Some(hi)) if 1 <= lo && lo <= hi && hi <= 65535) {
            errs.push(e("Rule '{name}': '{value}' is not a port or port range (1-65535)", &x));
        }
    }
    for x in list("geoip") {
        if !crate::singbox::rulesets::valid_name(&x) {
            errs.push(e("Rule '{name}': '{value}' is not a country code (e.g. kr)", &x));
        }
    }
    for x in list("geosite") {
        if !crate::singbox::rulesets::valid_name(&x) {
            errs.push(e("Rule '{name}': '{value}' is not a Geosite category name", &x));
        }
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_parsers() {
        assert!(valid_uuid("0b5b8d9e-1234-4567-89ab-0123456789ab"));
        assert!(valid_uuid("{0B5B8D9E12344567 89ab0123456789ab}".replace(' ', "").as_str()));
        assert!(valid_uuid("urn:uuid:0b5b8d9e-1234-4567-89ab-0123456789ab"));
        assert!(!valid_uuid("0b5b8d9e-1234-4567-89ab-0123456789a"));
        assert!(valid_wg_key("yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk="));
        assert!(!valid_wg_key("yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk"));
        assert!(valid_cidrs("10.0.0.2/32; fd00::2/128"));
        assert!(!valid_cidrs("10.0.0.2"));
        assert_eq!(py::ip_interface("10.1.2.3/255.255.0.0").map(|x| x.1), Some(16));
        assert_eq!(py::ip_interface("10.1.2.3/0.0.0.255").map(|x| x.1), Some(24));
        assert!(py::ip_interface("10.1.2.3/+8").is_none());
        assert!(ss_key_error("2022-blake3-aes-128-gcm", "AAAAAAAAAAAAAAAAAAAAAA==").is_none());
        assert!(ss_key_error("2022-blake3-aes-256-gcm", "AAAAAAAAAAAAAAAAAAAAAA==").unwrap().contains("16"));
    }
}
