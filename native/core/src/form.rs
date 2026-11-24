//! The block editor as data (Python: `ui/proxies.EditorInterface` - its rows, `_sync` for which rows show, `collect`
//! and `_errors` for saving). The window draws the fields; whenever a field marked `rerender` changes it asks for
//! the form again with the values typed so far, so the visible fields follow the type / security / mode chosen.

use crate::graph::{self, Obj};
use crate::i18n::tr;
use serde_json::{json, Map, Value};

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Text,
    /// shown masked
    Secret,
    Int { min: i64, max: i64 },
    Bool,
    /// (value, label)
    Choice(Vec<(String, String)>),
}

#[derive(Clone, Debug)]
pub struct Field {
    /// "name", "autostart", "inbound.port", "outbound.sni", ...
    pub key: String,
    pub label: String,
    pub desc: String,
    pub kind: Kind,
    pub value: Value,
    pub visible: bool,
    /// the form changes when this one does
    pub rerender: bool,
}

impl Field {
    pub fn to_json(&self) -> Value {
        let (kind, extra) = match &self.kind {
            Kind::Text => ("text", json!(null)),
            Kind::Secret => ("secret", json!(null)),
            Kind::Int { min, max } => ("int", json!([min, max])),
            Kind::Bool => ("bool", json!(null)),
            Kind::Choice(o) => ("choice", json!(o.iter().map(|(v, l)| json!([v, l])).collect::<Vec<_>>())),
        };
        json!({"key": self.key, "label": self.label, "desc": self.desc, "kind": kind, "options": extra,
               "value": self.value, "visible": self.visible, "rerender": self.rerender})
    }
}

fn choice(pairs: &[(&str, String)]) -> Kind {
    Kind::Choice(pairs.iter().map(|(v, l)| (v.to_string(), l.clone())).collect())
}

fn plain(values: &[&str]) -> Kind {
    Kind::Choice(values.iter().map(|v| (v.to_string(), v.to_string())).collect())
}

struct Builder<'a> {
    node: &'a Value,
    prefix: &'a str,
    out: Vec<Field>,
}

impl Builder<'_> {
    fn add(&mut self, key: &str, label: &str, desc: &str, kind: Kind, visible: bool) -> &mut Field {
        let full = if key == "name" || key == "autostart" || key == "auto_restart" { key.to_string() } else { format!("{}.{key}", self.prefix) };
        let value = match full.split_once('.') {
            Some((a, b)) => self.node[a][b].clone(),
            None => self.node[key].clone(),
        };
        self.out.push(Field { key: full, label: tr(label), desc: if desc.is_empty() { String::new() } else { tr(desc) }, kind, value, visible, rerender: false });
        self.out.last_mut().unwrap()
    }
}

fn inbound_types() -> Kind {
    choice(&[
        ("socks5", "SOCKS5".into()),
        ("socks4", "SOCKS4".into()),
        ("http", "HTTP".into()),
        ("https", "HTTPS".into()),
        ("shadowsocks", "Shadowsocks".into()),
        ("hysteria2", "Hysteria2".into()),
        ("wireguard", "WireGuard".into()),
        ("forward", tr("Port forward")),
        ("tun", "TUN".into()),
    ])
}

fn outbound_types() -> Kind {
    let types = ["direct", "http", "https", "socks4", "socks5", "shadowsocks", "hysteria2", "wireguard", "vless", "vmess", "trojan", "tuic", "subscription", "group", "block"];
    Kind::Choice(types.iter().map(|t| (t.to_string(), graph::outbound_label(t))).collect())
}

fn ss_methods() -> Kind {
    plain(&crate::checks::SS_METHODS.iter().map(|m| m.0).collect::<Vec<_>>())
}

/// The fields of an inbound block (a card).
pub fn inbound_form(node: &Value) -> Vec<Field> {
    let ib = &node["inbound"];
    let it = ib["type"].as_str().unwrap_or("socks5");
    let can_auth = matches!(it, "socks5" | "http" | "https");
    let auth = ib["auth_enabled"] == json!(true);
    let mut b = Builder { node, prefix: "inbound", out: vec![] };
    b.add("name", "Name", "", Kind::Text, true);
    b.add("autostart", "Start with the app", "Starts this proxy when ProxyDock starts", Kind::Bool, true);
    b.add("auto_restart", "Restart automatically", "If it stops on its own or fails to start (engine exit, port busy), try again after 3 s, then less often up to every minute", Kind::Bool, true);
    b.add("type", "Protocol", "", inbound_types(), true).rerender = true;
    b.add("host", "Listen address · port", "127.0.0.1 = this PC only, 0.0.0.0 = allow other PCs (check authentication and firewall)", Kind::Text, it != "tun" && it != "wireguard");
    b.add("port", "", "", Kind::Int { min: 1, max: 65535 }, it != "tun");
    b.add("tun_stack", "Network stack", "How sing-box handles the captured packets. Try another one if some apps misbehave",
        choice(&[("mixed", tr("mixed (recommended)")), ("system", "system".into()), ("gvisor", "gvisor".into())]), it == "tun");
    b.add("tun_strict_route", "Strict route", "Also stops traffic that tries to go around the TUN. Can break IPv6 on Windows; leave off unless needed", Kind::Bool, it == "tun");
    b.add("tun_fakeip", "Fake-IP DNS", "Apps get placeholder addresses (198.18.x.x) at once and the real name is looked up where the traffic leaves (e.g. at the VPN server), so names do not leak to the local network. Needs the sing-box engine", Kind::Bool, it == "tun");
    b.add("fwd_host", "Forward to", "Every connection to this port goes to this address, through the outbound below (e.g. a remote desktop or SSH server behind the VPN)", Kind::Text, it == "forward");
    b.add("fwd_port", "", "", Kind::Int { min: 0, max: 65535 }, it == "forward");
    b.add("fwd_network", "", "", choice(&[("tcp", "TCP".into()), ("udp", "UDP".into()), ("both", "TCP + UDP".into())]), it == "forward");
    b.add("wg_private_key", "Server private key", "Generated automatically. Keep it secret", Kind::Secret, it == "wireguard");
    b.add("wg_address", "Tunnel network · address for devices", "Devices get addresses from this network. The second box is the address devices use to reach this PC (empty = this PC's LAN IP; for the internet use your public IP or DDNS name and forward the UDP port on the router)", Kind::Text, it == "wireguard");
    b.add("wg_endpoint", "", "", Kind::Text, it == "wireguard");
    b.add("auth_enabled", "User authentication", "", Kind::Bool, can_auth).rerender = true;
    b.add("username", "Account", "", Kind::Text, can_auth && auth);
    b.add("password", "", "", Kind::Secret, can_auth && auth);
    b.add("tls_cert", "TLS certificate", "PEM format. If empty, a self-signed certificate is generated (clients must skip certificate verification)", Kind::Text, matches!(it, "https" | "hysteria2"));
    b.add("tls_key", "TLS private key", "", Kind::Text, matches!(it, "https" | "hysteria2"));
    b.add("ss_method", "Cipher", "Must match the client", ss_methods(), it == "shadowsocks");
    b.add("ss_password", "Shadowsocks password", "2022 ciphers use a base64 key - click 'Generate' after changing the cipher", Kind::Secret, it == "shadowsocks");
    b.add("hy2_password", "Hysteria2 password", "At least 8 characters", Kind::Secret, it == "hysteria2");
    b.add("hy2_obfs_password", "Salamander obfuscation", "Makes the traffic look like random data. Clients need the same password. Needs the sing-box engine", Kind::Secret, it == "hysteria2");
    b.add("hy2_up_mbps", "Speed limit up · down (Mbps)", "Upper limit per client. 0 = no limit", Kind::Int { min: 0, max: 100000 }, it == "hysteria2");
    b.add("hy2_down_mbps", "", "", Kind::Int { min: 0, max: 100000 }, it == "hysteria2");
    b.out
}

/// The fields of an outbound block. `members`: (id, name) of a group's members (for its manual choice).
pub fn outbound_form(node: &Value, members: &[(String, String)]) -> Vec<Field> {
    let ob = &node["outbound"];
    let s = |k: &str| ob[k].as_str().unwrap_or("").to_string();
    let ot = s("type");
    let ot = ot.as_str();
    let sec = s("security");
    let remote = !matches!(ot, "direct" | "block" | "subscription" | "group");
    let tls = matches!(ot, "https" | "hysteria2" | "trojan" | "tuic") || (matches!(ot, "vless" | "vmess") && sec != "none");
    let has_via = node["via"].is_string() && !matches!(ot, "direct" | "hysteria2" | "block");
    let fp = (matches!(ot, "vless" | "vmess" | "trojan") && sec != "none") || ot == "tuic";
    let tr_kind = s("transport");
    let gm = s("group_mode");
    let mut b = Builder { node, prefix: "outbound", out: vec![] };
    b.add("name", "Name", "", Kind::Text, true);
    b.add("type", "Type", "Direct = connect straight from this PC. The others go through another proxy/server", outbound_types(), true).rerender = true;
    b.add("host", "Server address · port", "", Kind::Text, remote);
    b.add("port", "", "", Kind::Int { min: 0, max: 65535 }, remote);
    b.add("bind", "Source address", "Connect from this IP of the PC (choose a network card / VPN). Empty = automatic", Kind::Text, ot == "direct");
    b.add("interface", "Network interface", "Leave through this network card, e.g. Tailscale, WireGuard, a VPN app or Wi-Fi, without changing the PC's routes. Needs the sing-box engine.", Kind::Text, !matches!(ot, "block" | "group") && !has_via);
    b.add("username", "Upstream proxy account", "", Kind::Text, matches!(ot, "http" | "https" | "socks4" | "socks5"));
    let user_shown = matches!(ot, "http" | "https" | "socks4" | "socks5");
    b.add("password", if user_shown { "" } else { "Password" }, "", Kind::Secret, matches!(ot, "http" | "https" | "socks5" | "trojan" | "tuic"));
    // subscription
    b.add("sub_url", "Subscription address", "", Kind::Secret, ot == "subscription");
    b.add("sub_mode", "Server", "Fastest automatically measures the servers every few minutes and moves away from ones that stop working",
        choice(&[("auto", tr("Fastest automatically")), ("select", tr("Pick a server"))]), ot == "subscription").rerender = true;
    let servers: Vec<(String, String)> = crate::singbox::compile::servers_of(ob)
        .iter()
        .filter_map(|x| x["name"].as_str().map(|n| (n.to_string(), n.to_string())))
        .collect();
    b.add("sub_selected", "", "", Kind::Choice(servers), ot == "subscription" && s("sub_mode") == "select");
    b.add("sub_interval", "Update every (hours)", "0 = only when you click 'Update now'", Kind::Int { min: 0, max: 720 }, ot == "subscription");
    // group
    b.add("group_mode", "Choose", match gm.as_str() {
        "failover" => "Uses the first member that works; when it stops working, the next one. It stays there until that one fails too",
        "select" => "Uses the member you choose. Switch it here or from the block's right-click menu (running cards restart)",
        _ => "Uses the member that answers fastest, and moves away from members that stop working",
    }, choice(&[("auto", tr("Fastest automatically")), ("failover", tr("Failover in order")), ("select", tr("Manual (choose one)"))]), ot == "group").rerender = true;
    b.add("group_selected", "", "", Kind::Choice(members.to_vec()), ot == "group" && gm == "select");
    b.add("group_test_url", "Test address · interval (s)", "Each member is measured by loading this address through it", Kind::Text, ot == "group" && gm != "select");
    b.add("group_interval", "", "", Kind::Int { min: 10, max: 3600 }, ot == "group" && gm != "select");
    b.add("group_tolerance", "Tolerance (ms)", "Switch only when another member is at least this much faster (avoids switching back and forth)", Kind::Int { min: 0, max: 5000 }, ot == "group" && gm == "auto");
    // VMess / VLESS / TUIC / Trojan
    b.add("vmess_cipher", "Encryption · alterId", "Use what the server expects. alterId 0 = current servers (AEAD)",
        Kind::Choice(crate::checks::VMESS_CIPHERS.iter().enumerate().map(|(i, c)| (c.to_string(), if i == 0 { tr("auto (recommended)") } else { c.to_string() })).collect()), ot == "vmess");
    b.add("vmess_alter_id", "", "", Kind::Int { min: 0, max: 65535 }, ot == "vmess");
    b.add("uuid", "User UUID", "", Kind::Text, matches!(ot, "vless" | "vmess" | "tuic"));
    let secs: Vec<(&str, String)> = if ot == "vmess" {
        vec![("none", tr("None")), ("tls", "TLS".into())]
    } else {
        vec![("none", tr("None")), ("tls", "TLS".into()), ("reality", "REALITY".into())]
    };
    b.add("security", "Security · flow", if ot == "vless" { "Use what the server is set up with (REALITY usually with xtls-rprx-vision)" } else { "Use what the server is set up with" },
        choice(&secs), matches!(ot, "vless" | "vmess")).rerender = true;
    b.add("vless_flow", "", "", choice(&[("", tr("(no flow)")), ("xtls-rprx-vision", "xtls-rprx-vision".into())]), ot == "vless");
    b.add("reality_public_key", "REALITY", "The server's public key and short ID. SNI = the site the server imitates", Kind::Text, ot == "vless" && sec == "reality");
    b.add("reality_short_id", "", "", Kind::Text, ot == "vless" && sec == "reality");
    let fps = ["", "chrome", "firefox", "safari", "edge", "ios", "android", "random", "randomized"];
    b.add("fingerprint", "Fingerprint · ALPN", "Look like this browser's TLS (uTLS). Empty ALPN = default",
        Kind::Choice(fps.iter().map(|f| (f.to_string(), if f.is_empty() { tr("(none)") } else { f.to_string() })).collect()), fp && ot != "tuic");
    b.add("alpn", if fp && ot != "tuic" { "" } else { "Fingerprint · ALPN" }, "", Kind::Text, fp);
    b.add("transport", "Transport", "WebSocket and gRPC are used behind CDNs. Path for WebSocket, service name for gRPC",
        choice(&[("tcp", "TCP".into()), ("ws", "WebSocket".into()), ("grpc", "gRPC".into())]), matches!(ot, "vless" | "vmess" | "trojan")).rerender = true;
    b.add("transport_path", "", "", Kind::Text, matches!(ot, "vless" | "vmess" | "trojan") && tr_kind != "tcp");
    b.add("transport_host", "", "", Kind::Text, matches!(ot, "vless" | "vmess" | "trojan") && tr_kind == "ws");
    b.add("tuic_congestion", "Congestion control · UDP relay", "Use what the TUIC server expects", plain(&["bbr", "cubic", "new_reno"]), ot == "tuic");
    b.add("tuic_udp_relay", "", "", plain(&["native", "quic"]), ot == "tuic");
    // TLS
    b.add("tls_verify", "Verify server certificate", "Turn off for servers with self-signed certificates (pinning the fingerprint is recommended)", Kind::Bool, tls && !(ot == "vless" && sec == "reality"));
    b.add("sni", "SNI (server name)", "Servers that check the name (e.g. the official Hysteria2 server) refuse a wrong one. Connecting by IP? Click 'Detect from server'.", Kind::Text, tls);
    b.add("pin_sha256", "Pin certificate fingerprint", "If set, only a certificate with this fingerprint is accepted", Kind::Text, ot == "hysteria2");
    // Shadowsocks / Hysteria2
    b.add("ss_method", "Cipher", "Must match the server", ss_methods(), ot == "shadowsocks");
    b.add("ss_password", "Shadowsocks password", "", Kind::Secret, ot == "shadowsocks");
    b.add("hy2_password", "Hysteria2 password", "", Kind::Secret, ot == "hysteria2");
    b.add("hy2_obfs_password", "Salamander obfuscation", "Only if the server uses it (obfs-password in the link)", Kind::Secret, ot == "hysteria2");
    b.add("hy2_up_mbps", "Bandwidth up · down (Mbps)", "Brutal: enter your real line speed to push through lossy networks. 0 = automatic (BBR)", Kind::Int { min: 0, max: 100000 }, ot == "hysteria2");
    b.add("hy2_down_mbps", "", "", Kind::Int { min: 0, max: 100000 }, ot == "hysteria2");
    b.add("hy2_ports", "Port hopping · interval (s)", "Ports the server listens on, e.g. 20000-30000 or 443,5000-6000. Empty = off", Kind::Text, ot == "hysteria2");
    b.add("hy2_hop_interval", "", "", Kind::Int { min: 5, max: 3600 }, ot == "hysteria2");
    // WireGuard
    let wg = ot == "wireguard";
    b.add("wg_private_key", "Private key", "", Kind::Secret, wg);
    b.add("wg_address", "Tunnel address", "This side's address inside the tunnel (Address in the .conf)", Kind::Text, wg);
    b.add("wg_peer_public_key", "Server public key", "PublicKey of the [Peer]", Kind::Text, wg);
    b.add("wg_preshared_key", "Preshared key", "", Kind::Secret, wg);
    b.add("wg_allowed_ips", "Allowed IPs", "Destinations sent into the tunnel. 0.0.0.0/0, ::/0 = everything", Kind::Text, wg);
    b.add("wg_mtu", "MTU · keepalive (s)", "Keepalive 0 = off. 25 helps behind NAT", Kind::Int { min: 576, max: 9200 }, wg);
    b.add("wg_keepalive", "", "", Kind::Int { min: 0, max: 3600 }, wg);
    b.out
}

/// The fields of a router / IF block: its name, and per rule a name and the conditions (one text per list).
pub fn router_form(node: &Value) -> Vec<Field> {
    let mut out = vec![Field { key: "name".into(), label: tr("Name"), desc: String::new(), kind: Kind::Text, value: node["name"].clone(), visible: true, rerender: false }];
    let is_if = node["style"] == json!("if");
    let switch = node["switch"] == json!(true);
    // (key, label, example) as the Python router editor
    let conds = [
        ("domain_suffix", "Domain (with subdomains)", "example.com, ads.example.net"),
        ("domain", "Domain (exact)", "www.example.com"),
        ("domain_keyword", "Domain keyword", "google"),
        ("ip_cidr", "IP / range", "10.0.0.0/8, 1.1.1.1"),
        ("geoip", "GeoIP country", "kr, jp, us"),
        ("geosite", "Geosite category", "google, netflix, category-ads-all"),
        ("port", "Destination port", "443, 8000-9000"),
        ("process_name", "Program", "chrome.exe, game.exe"),
    ];
    for (i, r) in node["rules"].as_array().into_iter().flatten().enumerate() {
        let base = format!("rules.{i}");
        if !is_if {
            out.push(Field { key: format!("{base}.name"), label: tr("Rule {n}").replace("{n}", &(i + 1).to_string()), desc: String::new(), kind: Kind::Text, value: r["name"].clone(), visible: true, rerender: false });
        }
        for (k, label, example) in conds {
            let text = r["match"][k].as_array().map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", ")).unwrap_or_default();
            out.push(Field { key: format!("{base}.match.{k}"), label: tr(label), desc: example.into(), kind: Kind::Text, value: json!(text), visible: !switch, rerender: false });
        }
        out.push(Field { key: format!("{base}.match.ip_private"), label: tr("Local network"), desc: String::new(), kind: Kind::Bool, value: json!(r["match"]["ip_private"] == json!(true)), visible: !switch, rerender: false });
    }
    out
}

/// The editor of a block: (kind, fields). kind: "inbound" / "outbound" / "router", None for other blocks.
pub fn form_of(g: &Obj, id: &str) -> Option<(&'static str, Vec<Field>)> {
    let (kind, node) = graph::find(g, id)?;
    Some(match kind {
        "inbound" => ("inbound", inbound_form(node)),
        "outbound" => {
            let members: Vec<(String, String)> = node["members"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|m| (m.to_string(), graph::find(g, m).and_then(|(_, n)| n["name"].as_str().map(String::from)).unwrap_or_default()))
                .collect();
            ("outbound", outbound_form(node, &members))
        }
        "router" => ("router", router_form(node)),
        _ => return None,
    })
}

/// a typed value in the field's type (numbers from text boxes, booleans from switches)
fn typed(old: &Value, new: &Value) -> Value {
    match (old, new) {
        (Value::Number(_), Value::String(t)) => crate::py::int(t).map_or_else(|| old.clone(), |n| json!(n)),
        (Value::Bool(_), Value::String(t)) => json!(t == "true"),
        _ => new.clone(),
    }
}

/// The node with the edited values put in (Python `collect`), validated like a loaded one; links stay.
pub fn apply(g: &Obj, id: &str, values: &Map<String, Value>) -> Option<(&'static str, Value)> {
    let (kind, node) = graph::find(g, id)?;
    let mut n = node.clone();
    let strip_keys = ["host", "tls_cert", "tls_key", "ss_password", "fwd_host", "wg_private_key", "wg_address", "wg_endpoint", "sni", "pin_sha256",
        "hy2_ports", "bind", "interface", "group_test_url", "wg_peer_public_key", "wg_preshared_key", "wg_allowed_ips", "uuid",
        "reality_public_key", "reality_short_id", "alpn", "transport_path", "transport_host", "sub_url"];
    for (key, v) in values {
        let parts: Vec<&str> = key.split('.').collect();
        match parts.as_slice() {
            [k] => {
                n[*k] = if *k == "name" { json!(v.as_str().unwrap_or("").trim()) } else { typed(&n[*k], v) };
            }
            [section @ ("inbound" | "outbound"), k] => {
                let mut v = typed(&n[*section][*k], v);
                if strip_keys.contains(k) {
                    if let Some(t) = v.as_str() {
                        v = json!(crate::py::strip(t));
                    }
                }
                n[*section][*k] = v;
            }
            ["rules", i, "name"] => {
                if let Some(r) = n["rules"].get_mut(i.parse::<usize>().ok()?) {
                    r["name"] = json!(v.as_str().unwrap_or("").trim());
                }
            }
            ["rules", i, "match", k] => {
                let Some(r) = n["rules"].get_mut(i.parse::<usize>().ok()?) else { continue };
                if *k == "ip_private" {
                    r["match"][*k] = json!(v == &json!(true) || v == &json!("true"));
                } else {
                    let items = crate::singbox::compile::split(v.as_str().unwrap_or(""));
                    r["match"][*k] = json!(items);
                }
            }
            _ => {}
        }
    }
    let out = match kind {
        "inbound" => {
            let ib_type = n["inbound"]["type"].as_str().unwrap_or("").to_string();
            if !matches!(ib_type.as_str(), "socks5" | "http" | "https") {
                n["inbound"]["auth_enabled"] = json!(false);
            }
            let target = n["target"].clone();
            let mut v = Value::Object(graph::validate_inbound_node(&n));
            v["target"] = target;
            v
        }
        "outbound" => {
            let ot = n["outbound"]["type"].as_str().unwrap_or("").to_string();
            if ot == "socks4" {
                n["outbound"]["password"] = json!("");
            }
            if ot != "direct" {
                n["outbound"]["bind"] = json!("");
            }
            if matches!(ot.as_str(), "block" | "group") {
                n["outbound"]["interface"] = json!("");
            }
            if ["direct", "hysteria2", "block", "subscription", "group"].contains(&ot.as_str()) {
                n["via"] = Value::Null;
            }
            if ot != "group" {
                n["members"] = json!([]);
            }
            if n["via"].is_string() {
                n["outbound"]["interface"] = json!("");
            }
            Value::Object(graph::validate_outbound_node(&n))
        }
        "router" => {
            // empty names stay, so the check can report them
            let names: Vec<Value> = n["rules"].as_array().into_iter().flatten().map(|r| r["name"].clone()).collect();
            let name = n["name"].clone();
            let mut v = Value::Object(graph::validate_router_node(&n));
            v["name"] = name;
            for (r, nm) in v["rules"].as_array_mut().into_iter().flatten().zip(names) {
                r["name"] = nm;
            }
            v
        }
        _ => return None,
    };
    Some((kind, out))
}

/// The graph with node replaced (same id).
pub fn with_node(g: &Obj, kind: &str, node: &Value) -> Obj {
    let mut g = g.clone();
    if let Some(list) = g.get_mut(&format!("{kind}s")).and_then(Value::as_array_mut) {
        for x in list.iter_mut() {
            if x["id"] == node["id"] {
                *x = node.clone();
            }
        }
    }
    g
}

/// What has to be fixed before the edited node can be saved (Python `EditorInterface._errors`).
pub fn errors(g: &Obj, kind: &str, node: &Value) -> Vec<String> {
    let g2 = with_node(g, kind, node);
    let id = node["id"].as_str().unwrap_or("");
    match kind {
        "inbound" => {
            let compiled: Vec<Value> = graph::compile(&g2).into_iter().map(Value::Object).collect();
            let Some(p) = compiled.iter().find(|p| p["id"] == node["id"]) else { return vec![] };
            let others: Vec<Value> = compiled.iter().filter(|o| o["id"] != node["id"]).cloned().collect();
            let unlinked = crate::checks::unlinked_error();
            // an unconnected inbound can be saved, it just cannot start
            crate::checks::check_proxy(p, &others).into_iter().filter(|e| p["unlinked"] != json!(true) || *e != unlinked).collect()
        }
        "outbound" => {
            let name = node["name"].as_str().unwrap_or("");
            let mut errs = if name.is_empty() { vec![tr("Enter a name")] } else { vec![] };
            if graph::via_loop(&g2, id) {
                let mut names = graph::chain_names(&g2, node["via"].as_str().unwrap_or(""));
                names.push(name.to_string());
                errs.push(format!("{}{}", tr("The proxy chain loops: "), names.join(" → ")));
                return errs;
            }
            errs.extend(crate::checks::outbound_errors(&graph::resolve_outbound(&g2, id)));
            errs
        }
        "router" => {
            let is_if = node["style"] == json!("if");
            let mut errs = if node["name"].as_str().unwrap_or("").is_empty() { vec![tr("Enter a name")] } else { vec![] };
            if node["default"].is_null() {
                errs.push(tr("Choose the default target (where traffic goes when no rule matches)"));
            }
            for (i, r) in node["rules"].as_array().into_iter().flatten().enumerate() {
                let rname = r["name"].as_str().filter(|n| !n.is_empty()).map(String::from).unwrap_or_else(|| (i + 1).to_string());
                if r["name"].as_str().unwrap_or("").is_empty() && !is_if {
                    errs.push(tr("Rule {n}: enter a name").replace("{n}", &(i + 1).to_string()));
                }
                let empty_match = r["match"].as_object().is_none_or(|m| m.is_empty());
                if is_if && node["switch"] == json!(true) {
                    if r["target"].is_null() {
                        errs.push(tr("IF: choose where 'then' goes"));
                    }
                    continue;
                }
                if is_if && empty_match {
                    errs.push(tr("IF: add at least one condition"));
                } else if is_if && r["target"].is_null() {
                    errs.push(tr("IF: choose where 'then' goes"));
                } else if !r["target"].is_null() && empty_match {
                    errs.push(tr("Rule '{name}': add at least one condition").replace("{name}", &rname));
                }
                errs.extend(crate::checks::match_errors(&rname, &r["match"]));
            }
            if graph::router_loop(&g2, id) {
                errs.push(tr("The routers point at each other in a loop"));
            }
            errs
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> Obj {
        graph::validate(&json!({
            "inbounds": [{"id": "a", "name": "A", "inbound": {"type": "socks5", "port": 1080}, "target": "d"}],
            "outbounds": [{"id": "d", "name": "Direct", "outbound": {"type": "direct"}}, {"id": "v", "name": "V", "outbound": {"type": "vless", "host": "h", "port": 443}}],
            "routers": [{"id": "r", "name": "R", "rules": [{"id": "x", "name": "kr", "match": {"geoip": ["kr"]}, "target": "d"}], "default": "d"}],
        }))
    }

    fn vis(f: &[Field], key: &str) -> bool {
        f.iter().find(|x| x.key == key).unwrap().visible
    }

    #[test]
    fn visibility_follows_values() {
        let g = g();
        let (_, f) = form_of(&g, "a").unwrap();
        assert!(!vis(&f, "inbound.username"));
        let mut v = Map::new();
        v.insert("inbound.auth_enabled".into(), json!(true));
        let (_, n) = apply(&g, "a", &v).unwrap();
        assert!(vis(&inbound_form(&n), "inbound.username"));
        let (_, f) = form_of(&g, "v").unwrap();
        assert!(!vis(&f, "outbound.reality_public_key") && vis(&f, "outbound.uuid") && vis(&f, "outbound.sni"));
        let mut v = Map::new();
        v.insert("outbound.security".into(), json!("reality"));
        let (_, n) = apply(&g, "v", &v).unwrap();
        let f = outbound_form(&n, &[]);
        assert!(vis(&f, "outbound.reality_public_key") && !vis(&f, "outbound.tls_verify"));
    }

    #[test]
    fn apply_and_check() {
        let g = g();
        let mut v = Map::new();
        v.insert("name".into(), json!("  New name "));
        v.insert("inbound.port".into(), json!("2080"));
        v.insert("inbound.auth_enabled".into(), json!(true));
        let (k, n) = apply(&g, "a", &v).unwrap();
        assert_eq!(n["name"], "New name");
        assert_eq!(n["inbound"]["port"], 2080);
        assert_eq!(n["target"], "d");
        assert!(errors(&g, k, &n).iter().any(|e| e.contains("username")));
        let mut v = Map::new();
        v.insert("rules.0.match.port".into(), json!("443, 99999"));
        v.insert("rules.0.name".into(), json!(""));
        let (k, n) = apply(&g, "r", &v).unwrap();
        assert_eq!(n["rules"][0]["match"]["port"], json!(["443", "99999"]));
        let e = errors(&g, k, &n);
        assert!(e.iter().any(|x| x.contains("99999")) && e.iter().any(|x| x.contains("Rule 1")), "{e:?}");
        let (k, n) = apply(&g, "v", &Map::new()).unwrap();
        assert!(errors(&g, k, &n).iter().any(|x| x.contains("UUID")));
    }
}
