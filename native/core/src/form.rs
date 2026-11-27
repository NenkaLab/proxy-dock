//! The block editor as data (Python: `ui/proxies.EditorInterface`, `ui/router.RouterSection`,
//! `ui/relay.RelaySection` - their rows, `_sync` for which rows show, `collect` and `_errors` for saving, and the
//! buttons: key generation, WireGuard devices, group members, rules, lanes).
//!
//! The controller keeps a draft of the block being edited: the window sends the values typed so far, they go into
//! the draft (`apply`), and the form is built again from the draft. Buttons (`act`) change the draft too (add a
//! rule, a device, a member, a key), so list indexes always refer to the draft the window is showing.

use crate::graph::{self, Obj};
use crate::i18n::tr;
use serde_json::{Map, Value, json};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// the router editor's view, shared by every router while the app runs (Python `router.MODE`): false = simple
/// (a row per condition in use), true = advanced (every condition field)
pub static ROUTER_ADVANCED: AtomicBool = AtomicBool::new(false);
/// conditions added in the simple view that are still empty: (rule id, condition key)
static SHOWN: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
/// the network interfaces the outbound editor offers (read again by `refresh_ifaces`)
static IFACES: Mutex<Vec<crate::netinfo::Iface>> = Mutex::new(Vec::new());

/// read the PC's network interfaces again (when an editor opens and on "Refresh the list")
pub fn refresh_ifaces() {
    *IFACES.lock().unwrap() = crate::netinfo::interfaces();
}

/// the interface `name` is up but has no route to the internet (Python `ifaces_unrouted`)
pub fn iface_unrouted(name: &str) -> bool {
    IFACES.lock().unwrap().iter().any(|i| i.name == name && !i.routed)
}

/// the "Network interface" choices: automatic, the ones up (with their IPv4 and a warning when they have no
/// internet route), and the saved one when it is not up now (Python `_fill_ifaces`)
fn iface_choices(current: &str) -> Kind {
    let mut o = vec![(String::new(), tr("Automatic"))];
    // the VPN bypass VMs (Windows: Hyper-V), the shared one always offered
    if cfg!(windows) {
        let store = crate::bypass::Store::load();
        let mut names: Vec<String> = store.instances.iter().map(|i| i.name.clone()).collect();
        if !names.contains(&String::new()) {
            names.insert(0, String::new());
        }
        for n in names {
            let mut label = tr("Outside the VPN ({name})").replace("{name}", &crate::bypass::label(&n));
            if !store.get(&n).is_some_and(|i| i.consented()) {
                label += &format!(" · ⚠ {}", tr("set up in Settings > VPN bypass"));
            }
            o.push((crate::bypass::iface_for(&n), label));
        }
    }
    for i in IFACES.lock().unwrap().iter() {
        let mut label = if i.ip.is_empty() { i.name.clone() } else { format!("{} · {}", i.name, i.ip) };
        if !i.routed {
            label += &format!(" · ⚠ {}", tr("no internet route"));
        }
        o.push((i.name.clone(), label));
    }
    if !current.is_empty() && !o.iter().any(|(v, _)| v == current) {
        let shown = match crate::bypass::instance_of(current) {
            Some(n) => tr("Outside the VPN ({name})").replace("{name}", &crate::bypass::label(&n)),
            None => current.to_string(),
        };
        o.push((current.to_string(), tr("{name} (not connected)").replace("{name}", &shown)));
    }
    Kind::Choice(o)
}

/// the router conditions: (key, label, example) as the Python router editor
const CONDS: [(&str, &str, &str); 8] = [
    ("domain_suffix", "Domain (with subdomains)", "example.com, ads.example.net"),
    ("domain", "Domain (exact)", "www.example.com"),
    ("domain_keyword", "Domain keyword", "google"),
    ("ip_cidr", "IP / range", "10.0.0.0/8, 1.1.1.1"),
    ("geoip", "GeoIP country", "kr, jp, us"),
    ("geosite", "Geosite category", "google, netflix, category-ads-all"),
    ("port", "Destination port", "443, 8000-9000"),
    ("process_name", "Program", "chrome.exe, game.exe"),
];

/// "Add rule"'s templates (Python `router.templates`): menu label, rule name, conditions, target block type
type Template = (String, Option<String>, Value, Option<&'static str>);

fn rule_templates() -> Vec<Template> {
    vec![
        (tr("Empty rule"), None, json!({}), None),
        (tr("Programs..."), Some(tr("Programs")), json!({"process_name": []}), None),
        (tr("Ads and trackers (Geosite)"), Some(tr("Ads")), json!({"geosite": ["category-ads-all"]}), Some("block")),
        (
            tr("Local network (LAN, private IPs)"),
            Some(tr("Local network")),
            json!({"ip_private": true}),
            Some("direct"),
        ),
        (
            tr("Sites of one country (GeoIP + Geosite)"),
            Some(tr("Country")),
            json!({"geoip": ["kr"], "geosite": ["kr"]}),
            None,
        ),
    ]
}

/// the outbound blocks a group can still take as members (not itself, groups, Block or ones it has)
fn addable_members(g: &Obj, node: &Value) -> Vec<(String, String)> {
    let id = s(node, "id");
    let members: Vec<&str> = node["members"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    list(g, "outbounds")
        .filter(|x| {
            s(x, "id") != id
                && !members.contains(&s(x, "id"))
                && !matches!(x["outbound"]["type"].as_str(), Some("group" | "block"))
        })
        .map(|x| (s(x, "id").into(), s(x, "name").into()))
        .collect()
}

/// the conditions a rule shows in the simple view: the ones in use and the ones just added (a new rule: domain)
fn simple_conds(r: &Value) -> Vec<&'static str> {
    let rid = s(r, "id");
    let shown = SHOWN.lock().unwrap();
    let on = |k: &str| shown.iter().any(|(id, c)| id == rid && c == k);
    let mut out: Vec<&'static str> = CONDS
        .iter()
        .map(|c| c.0)
        .filter(|k| r["match"][*k].as_array().is_some_and(|a| !a.is_empty()) || on(k))
        .collect();
    if r["match"]["ip_private"] == json!(true) || on("ip_private") {
        out.push("ip_private");
    }
    if out.is_empty() {
        out.push("domain_suffix");
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    Text,
    /// shown masked
    Secret,
    Int {
        min: i64,
        max: i64,
    },
    Bool,
    /// (value, label)
    Choice(Vec<(String, String)>),
    /// a button; the field's key is the action
    Button,
    /// a line of a list: label and value text, with buttons (action, label)
    Item(Vec<(String, String)>),
    /// read-only text that can be selected (a public key)
    Info,
    /// a heading between groups of fields
    Heading,
}

#[derive(Clone, Debug)]
pub struct Field {
    /// "name", "inbound.port", "overrides.timeout", "rules.0.target", or an action for buttons
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
            Kind::Button => ("button", json!(null)),
            Kind::Item(b) => ("item", json!(b.iter().map(|(v, l)| json!([v, l])).collect::<Vec<_>>())),
            Kind::Info => ("info", json!(null)),
            Kind::Heading => ("heading", json!(null)),
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

fn field(key: &str, label: &str, desc: &str, kind: Kind, value: Value, visible: bool) -> Field {
    Field {
        key: key.into(),
        label: if label.is_empty() { String::new() } else { tr(label) },
        desc: if desc.is_empty() { String::new() } else { tr(desc) },
        kind,
        value,
        visible,
        rerender: false,
    }
}

struct Builder<'a> {
    node: &'a Value,
    prefix: &'a str,
    out: Vec<Field>,
}

impl Builder<'_> {
    fn add(&mut self, key: &str, label: &str, desc: &str, kind: Kind, visible: bool) -> &mut Field {
        let full = if matches!(key, "name" | "autostart" | "auto_restart") {
            key.to_string()
        } else {
            format!("{}.{key}", self.prefix)
        };
        let value = match full.split_once('.') {
            Some((a, b)) => self.node[a][b].clone(),
            None => self.node[key].clone(),
        };
        self.out.push(field(&full, label, desc, kind, value, visible));
        self.out.last_mut().unwrap()
    }

    fn push(&mut self, f: Field) -> &mut Field {
        self.out.push(f);
        self.out.last_mut().unwrap()
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn name_of(g: &Obj, id: &str) -> String {
    graph::find(g, id).map(|(_, n)| s(n, "name").to_string()).unwrap_or_default()
}

/// "Wireless #3" / "Pass-through · lane 2" for a pass-through input lane (Python `lane_label`)
fn lane_choices(g: &Obj) -> Vec<(String, String)> {
    let mut out = vec![];
    for r in g.get("relays").and_then(Value::as_array).into_iter().flatten() {
        if r.get("broken").is_some_and(|b| !b.is_null()) {
            continue;
        }
        for (i, x) in r["inputs"].as_array().into_iter().flatten().enumerate() {
            let label = if r["wireless"] == json!(true) {
                tr("Wireless {name}").replace("{name}", &format!("#{}", r["number"].as_i64().unwrap_or(0)))
            } else {
                tr("{name} · lane {n}").replace("{name}", s(r, "name")).replace("{n}", &(i + 1).to_string())
            };
            out.push((s(x, "id").to_string(), label));
        }
    }
    out
}

fn list<'a>(g: &'a Obj, key: &str) -> impl Iterator<Item = &'a Value> {
    g.get(key).and_then(Value::as_array).into_iter().flatten()
}

/// where an inbound block can send its traffic (Python `_after_fill`)
fn inbound_targets(g: &Obj, own: &str) -> Kind {
    let mut o = vec![(String::new(), tr("(not connected)"))];
    o.extend(list(g, "outbounds").map(|x| (s(x, "id").into(), s(x, "name").into())));
    o.extend(list(g, "routers").map(|x| (s(x, "id").into(), tr("Router {name}").replace("{name}", s(x, "name")))));
    o.extend(
        list(g, "inbounds")
            .filter(|x| s(x, "id") != own)
            .map(|x| (s(x, "id").into(), tr("relay → {name}").replace("{name}", s(x, "name")))),
    );
    o.extend(lane_choices(g));
    Kind::Choice(o)
}

/// where a router output or a pass-through output can go: outbound blocks, other routers, pass-through lanes
fn router_targets(g: &Obj, own: &str) -> Kind {
    let mut o = vec![(String::new(), tr("(not connected)"))];
    o.extend(list(g, "outbounds").map(|x| (s(x, "id").into(), s(x, "name").into())));
    o.extend(
        list(g, "routers")
            .filter(|x| s(x, "id") != own)
            .map(|x| (s(x, "id").into(), tr("Router {name}").replace("{name}", s(x, "name")))),
    );
    o.extend(lane_choices(g).into_iter().filter(|(id, _)| {
        !own.is_empty()
            && !graph::find(g, own)
                .is_some_and(|(_, n)| n["inputs"].as_array().is_some_and(|a| a.iter().any(|x| x["id"] == json!(id))))
    }));
    Kind::Choice(o)
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
    let types = [
        "direct",
        "http",
        "https",
        "socks4",
        "socks5",
        "shadowsocks",
        "hysteria2",
        "wireguard",
        "vless",
        "vmess",
        "trojan",
        "tuic",
        "subscription",
        "group",
        "block",
    ];
    Kind::Choice(types.iter().map(|t| (t.to_string(), graph::outbound_label(t))).collect())
}

fn ss_methods() -> Kind {
    plain(&crate::checks::SS_METHODS.iter().map(|m| m.0).collect::<Vec<_>>())
}

/// the keys a card can set for itself instead of the global settings (Python `OVERRIDABLE`)
pub const OVERRIDABLE: [(&str, &str, &str); 7] = [
    ("allowed_domains", "Allowed domains", "Comma separated · * = all · *.example.com = including subdomains"),
    ("block_private", "Block private network destinations", "Refuse connections to loopback and private IPs"),
    ("timeout", "Idle timeout (s)", ""),
    ("log_connections", "Log connections", ""),
    ("log_close", "Log connection close details", ""),
    (
        "dns_server",
        "DNS server",
        "Empty = the PC's DNS. Otherwise 1.1.1.1 (UDP), tcp://, tls:// (DoT), https://…/dns-query (DoH), quic:// or h3://. Used where ProxyDock looks names up itself (Direct, IP rules, TUN). Needs the sing-box engine",
    ),
    // (several servers in the same field: the extra ones go to dns_extra)
    (
        "dns_via_outbound",
        "DNS through the outbound",
        "The card's own lookups go through its tunnel or upstream instead of leaving this PC directly (needs a DNS server above)",
    ),
];

fn peers(ib: &Value) -> Vec<Value> {
    match serde_json::from_str::<Value>(if s(ib, "wg_peers").is_empty() { "[]" } else { s(ib, "wg_peers") }) {
        Ok(Value::Array(a)) => a.into_iter().filter(|x| x.is_object() && x["public_key"].is_string()).collect(),
        _ => vec![],
    }
}

/// The fields of an inbound block (a card).
pub fn inbound_form(g: &Obj, glob: &Map<String, Value>, node: &Value) -> Vec<Field> {
    let ib = &node["inbound"];
    let it = ib["type"].as_str().unwrap_or("socks5");
    let can_auth = matches!(it, "socks5" | "http" | "https");
    let auth = ib["auth_enabled"] == json!(true);
    let mut b = Builder { node, prefix: "inbound", out: vec![] };
    b.add("name", "Name", "", Kind::Text, true);
    b.add("autostart", "Start with the app", "Starts this proxy when ProxyDock starts", Kind::Bool, true);
    b.add("auto_restart", "Restart automatically", "If it stops on its own or fails to start (engine exit, port busy), try again after 3 s, then less often up to every minute", Kind::Bool, true);
    b.push(field("", "Inbound (how this PC accepts connections)", "", Kind::Heading, json!(null), true));
    b.add("type", "Protocol", "", inbound_types(), true).rerender = true;
    b.add(
        "host",
        "Listen address · port",
        "127.0.0.1 = this PC only, 0.0.0.0 = allow other PCs (check authentication and firewall)",
        Kind::Text,
        it != "tun" && it != "wireguard",
    );
    b.add(
        "port",
        if it == "wireguard" { "Listen address · port" } else { "" },
        "",
        Kind::Int { min: 1, max: 65535 },
        it != "tun",
    );
    b.push(field("port.check", "Check availability", "", Kind::Button, json!(null), it != "tun"));
    b.add(
        "tun_stack",
        "Network stack",
        "How sing-box handles the captured packets. Try another one if some apps misbehave",
        choice(&[("mixed", tr("mixed (recommended)")), ("system", "system".into()), ("gvisor", "gvisor".into())]),
        it == "tun",
    );
    b.add(
        "tun_strict_route",
        "Strict route",
        "Also stops traffic that tries to go around the TUN. Can break IPv6 on Windows; leave off unless needed",
        Kind::Bool,
        it == "tun",
    );
    b.add("tun_fakeip", "Fake-IP DNS", "Apps get placeholder addresses (198.18.x.x) at once and the real name is looked up where the traffic leaves (e.g. at the VPN server), so names do not leak to the local network. Needs the sing-box engine", Kind::Bool, it == "tun");
    b.add("fwd_host", "Forward to", "Every connection to this port goes to this address, through the outbound below (e.g. a remote desktop or SSH server behind the VPN)", Kind::Text, it == "forward");
    b.add("fwd_port", "", "", Kind::Int { min: 0, max: 65535 }, it == "forward");
    b.add(
        "fwd_network",
        "",
        "",
        choice(&[("tcp", "TCP".into()), ("udp", "UDP".into()), ("both", "TCP + UDP".into())]),
        it == "forward",
    );
    // WireGuard server: key, network, devices
    let wg = it == "wireguard";
    b.add("wg_private_key", "Server private key", "Generated automatically. Keep it secret", Kind::Secret, wg);
    b.push(field("gen.inbound.wg_private_key", "Generate", "", Kind::Button, json!(null), wg));
    b.add("wg_address", "Tunnel network · address for devices", "Devices get addresses from this network. The second box is the address devices use to reach this PC (empty = this PC's LAN IP; for the internet use your public IP or DDNS name and forward the UDP port on the router)", Kind::Text, wg);
    b.add("wg_endpoint", "", "", Kind::Text, wg);
    let devices = peers(ib);
    b.push(field(
        "",
        "Devices",
        "Each phone or PC gets its own key. Copy or save its config and import it in the WireGuard app on that device",
        Kind::Heading,
        json!(null),
        wg,
    ));
    for (i, p) in devices.iter().enumerate() {
        let buttons = vec![
            (format!("wgdev.conf.{i}"), tr("Copy config")),
            (format!("wgdev.save.{i}"), tr("Save .conf")),
            (format!("wgdev.del.{i}"), tr("Remove")),
        ];
        let name = p.get("name").map_or_else(|| "?".to_string(), crate::singbox::compile::py_str);
        b.push(Field {
            key: format!("wgdev.{i}"),
            label: name,
            desc: String::new(),
            kind: Kind::Item(buttons),
            value: p["address"].clone(),
            visible: wg,
            rerender: false,
        });
    }
    b.push(field("wg_new_name", "Add device", "", Kind::Text, json!(""), wg));
    b.push(field("wgdev.add", "Add", "", Kind::Button, json!(null), wg));
    // accounts and secrets
    b.add("auth_enabled", "User authentication", "", Kind::Bool, can_auth).rerender = true;
    b.add("username", "Account", "", Kind::Text, can_auth && auth);
    b.add("password", "", "", Kind::Secret, can_auth && auth);
    let tls_files = matches!(it, "https" | "hysteria2");
    b.add(
        "tls_cert",
        "TLS certificate",
        "PEM format. If empty, a self-signed certificate is generated (clients must skip certificate verification)",
        Kind::Text,
        tls_files,
    );
    b.push(field("ui.browse.inbound.tls_cert", "Browse", "", Kind::Button, json!(null), tls_files));
    b.add("tls_key", "TLS private key", "", Kind::Text, tls_files);
    b.push(field("ui.browse.inbound.tls_key", "Browse", "", Kind::Button, json!(null), tls_files));
    b.add("ss_method", "Cipher", "Must match the client", ss_methods(), it == "shadowsocks");
    b.add(
        "ss_password",
        "Shadowsocks password",
        "2022 ciphers use a base64 key - click 'Generate' after changing the cipher",
        Kind::Secret,
        it == "shadowsocks",
    );
    b.push(field("gen.inbound.ss_password", "Generate", "", Kind::Button, json!(null), it == "shadowsocks"));
    b.add("hy2_password", "Hysteria2 password", "At least 8 characters", Kind::Secret, it == "hysteria2");
    b.push(field("gen.inbound.hy2_password", "Generate", "", Kind::Button, json!(null), it == "hysteria2"));
    b.add(
        "hy2_obfs_password",
        "Salamander obfuscation",
        "Makes the traffic look like random data. Clients need the same password. Needs the sing-box engine",
        Kind::Secret,
        it == "hysteria2",
    );
    b.add(
        "hy2_up_mbps",
        "Speed limit up · down (Mbps)",
        "Upper limit per client. 0 = no limit",
        Kind::Int { min: 0, max: 100000 },
        it == "hysteria2",
    );
    b.add("hy2_down_mbps", "", "", Kind::Int { min: 0, max: 100000 }, it == "hysteria2");
    // where it goes
    b.push(field("", "Connect to", "", Kind::Heading, json!(null), true));
    let id = s(node, "id");
    b.push(field(
        "target",
        "Outbound block",
        "Where this inbound sends its traffic. Choosing another inbound relays into it. Also editable on the canvas.",
        inbound_targets(g, id),
        json!(node["target"].as_str().unwrap_or("")),
        true,
    ));
    // this card's own settings instead of the global ones
    b.push(field("", "Check connection", "", Kind::Heading, json!(null), true));
    b.push(field("probe", "Check connection", "Tests the current, unsaved settings: connects to an HTTPS site through the outbound and checks that the inbound port can be opened (including the certificate).", Kind::Button, json!(null), true));
    b.push(field(
        "",
        "Local settings (uncheck to use a value just for this proxy)",
        "",
        Kind::Heading,
        json!(null),
        true,
    ));
    for (key, label, desc) in OVERRIDABLE {
        let own = &node["overrides"][key];
        let follow = own.is_null();
        // the switch is Python's "Use global" check box
        let f = b.push(field(&format!("follow.{key}"), label, desc, Kind::Bool, json!(follow), true));
        f.rerender = true;
        f.desc = if f.desc.is_empty() { tr("Use global") } else { format!("{} · {}", tr("Use global"), f.desc) };
        if key == "dns_server" {
            f.desc = format!(
                "{} {}",
                f.desc,
                tr("Several servers (separated by commas): all are asked at once and the first answer is used.")
            );
        }
        let value = if follow { glob.get(key).cloned().unwrap_or(Value::Null) } else { own.clone() };
        let (kind, value) = match key {
            "allowed_domains" => (
                Kind::Text,
                json!(
                    value
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                        .unwrap_or_default()
                ),
            ),
            "timeout" => (Kind::Int { min: 10, max: 86400 }, value),
            // the main DNS server and the extra ones (they race it) in one field
            "dns_server" => {
                let extra = if follow {
                    glob.get("dns_extra").cloned().unwrap_or(Value::Null)
                } else {
                    node["overrides"]["dns_extra"].clone()
                };
                (Kind::Text, json!(crate::config::dns_list_text(value.as_str().unwrap_or(""), &extra)))
            }
            _ => (Kind::Bool, value),
        };
        b.push(field(&format!("overrides.{key}"), "", "", kind, value, !follow));
    }
    b.out
}

/// The fields of an outbound block.
pub fn outbound_form(g: &Obj, node: &Value) -> Vec<Field> {
    let ob = &node["outbound"];
    let st = |k: &str| ob[k].as_str().unwrap_or("").to_string();
    let ot = st("type");
    let ot = ot.as_str();
    let sec = st("security");
    let remote = !matches!(ot, "direct" | "block" | "subscription" | "group");
    let tls =
        matches!(ot, "https" | "hysteria2" | "trojan" | "tuic") || (matches!(ot, "vless" | "vmess") && sec != "none");
    let has_via = node["via"].is_string() && !matches!(ot, "direct" | "hysteria2" | "block");
    let fp = (matches!(ot, "vless" | "vmess" | "trojan") && sec != "none") || ot == "tuic";
    let tr_kind = st("transport");
    let gm = st("group_mode");
    let id = s(node, "id");
    let members: Vec<String> =
        node["members"].as_array().into_iter().flatten().filter_map(|m| m.as_str().map(String::from)).collect();
    let mut b = Builder { node, prefix: "outbound", out: vec![] };
    b.add("name", "Name", "", Kind::Text, true);
    b.add(
        "type",
        "Type",
        "Direct = connect straight from this PC. The others go through another proxy/server",
        outbound_types(),
        true,
    )
    .rerender = true;
    b.push(field("link.paste", "Paste share link", "", Kind::Button, json!(null), true));
    b.add("host", "Server address · port", "", Kind::Text, remote);
    b.add("port", "", "", Kind::Int { min: 0, max: 65535 }, remote);
    // through another block
    let mut vias = vec![(String::new(), tr("(directly)"))];
    vias.extend(
        list(g, "outbounds")
            .filter(|x| s(x, "id") != id && !matches!(x["outbound"]["type"].as_str(), Some("direct" | "group")))
            .map(|x| (s(x, "id").into(), s(x, "name").into())),
    );
    b.push(field("via", "Through", "Connect to this server through another outbound block (e.g. a tunnel through the company proxy). Also editable on the canvas.",
        Kind::Choice(vias), json!(node["via"].as_str().unwrap_or("")), !matches!(ot, "direct" | "hysteria2" | "block" | "subscription" | "group"))).rerender = true;
    b.add(
        "bind",
        "Source address",
        "Connect from this IP of the PC (choose a network card / VPN). Empty = automatic",
        Kind::Text,
        ot == "direct",
    );
    let iface_shown = !matches!(ot, "block" | "group") && !has_via;
    b.add("interface", "Network interface", "Leave through this network card, e.g. Tailscale, WireGuard, a VPN app or Wi-Fi, without changing the PC's routes. Needs the sing-box engine.", iface_choices(&st("interface")), iface_shown).rerender = true;
    b.push(field("iface.refresh", "Refresh the list", "", Kind::Button, json!(null), iface_shown));
    let bypass_note = iface_shown && crate::bypass::instance_of(&st("interface")).is_some();
    b.push(field(
        "bypass_note",
        "Outside the VPN",
        "",
        Kind::Info,
        json!(tr("This exit leaves outside the VPN running on this PC: the sites it reaches and your internet provider see your real IP address, and the VPN's kill switch and DNS protection do not apply to it.")),
        bypass_note,
    ));
    let user_shown = matches!(ot, "http" | "https" | "socks4" | "socks5");
    b.add("username", "Upstream proxy account", "", Kind::Text, user_shown);
    b.add(
        "password",
        if user_shown { "" } else { "Password" },
        "",
        Kind::Secret,
        matches!(ot, "http" | "https" | "socks5" | "trojan" | "tuic"),
    );
    // subscription
    let servers: Vec<(String, String)> = crate::singbox::compile::servers_of(ob)
        .iter()
        .filter_map(|x| x["name"].as_str().map(|n| (n.to_string(), n.to_string())))
        .collect();
    let updated = ob["sub_updated"].as_i64().unwrap_or(0);
    let when = if updated > 0 { crate::log::local_minute(updated) } else { "-".into() };
    b.add("sub_url", "Subscription address", "", Kind::Secret, ot == "subscription").desc =
        tr("{count} servers · updated {when}").replace("{count}", &servers.len().to_string()).replace("{when}", &when);
    b.push(field("sub.update", "Update now", "", Kind::Button, json!(null), ot == "subscription"));
    b.add(
        "sub_mode",
        "Server",
        "Fastest automatically measures the servers every few minutes and moves away from ones that stop working",
        choice(&[("auto", tr("Fastest automatically")), ("select", tr("Pick a server"))]),
        ot == "subscription",
    )
    .rerender = true;
    let servers = if servers.is_empty() { vec![(String::new(), tr("(no servers yet)"))] } else { servers };
    b.add("sub_selected", "", "", Kind::Choice(servers), ot == "subscription" && st("sub_mode") == "select");
    b.add(
        "sub_interval",
        "Update every (hours)",
        "0 = only when you click 'Update now'",
        Kind::Int { min: 0, max: 720 },
        ot == "subscription",
    );
    // group: mode and members
    let grp = ot == "group";
    b.add("group_mode", "Choose", match gm.as_str() {
        "failover" => "Uses the first member that works; when it stops working, the next one. It stays there until that one fails too",
        "select" => "Uses the member you choose. Switch it here or from the block's right-click menu (running cards restart)",
        _ => "Uses the member that answers fastest, and moves away from members that stop working",
    }, choice(&[("auto", tr("Fastest automatically")), ("failover", tr("Failover in order")), ("select", tr("Manual (choose one)"))]), grp).rerender = true;
    b.add(
        "group_selected",
        "",
        "",
        Kind::Choice(members.iter().map(|m| (m.clone(), name_of(g, m))).collect()),
        grp && gm == "select",
    );
    b.push(field("", "Members", "Outbound blocks this group uses, first one on top. You can also connect them on the canvas from the group's 'members' output.", Kind::Heading, json!(null), grp));
    for (i, m) in members.iter().enumerate() {
        let buttons = vec![
            (format!("member.up.{i}"), "↑".into()),
            (format!("member.down.{i}"), "↓".into()),
            (format!("member.del.{i}"), tr("Remove")),
        ];
        b.push(Field {
            key: format!("member.{i}"),
            label: format!("{}. {}", i + 1, name_of(g, m)),
            desc: String::new(),
            kind: Kind::Item(buttons),
            value: json!(""),
            visible: grp,
            rerender: false,
        });
    }
    let addable = addable_members(g, node);
    let first = addable.first().map_or(String::new(), |a| a.0.clone());
    b.push(field("member_new", "Add a member", "", Kind::Choice(addable), json!(first), grp));
    b.push(field("member.add", "Add", "", Kind::Button, json!(null), grp));
    b.add(
        "group_test_url",
        "Test address · interval (s)",
        "Each member is measured by loading this address through it",
        Kind::Text,
        grp && gm != "select",
    );
    b.add("group_interval", "", "", Kind::Int { min: 10, max: 3600 }, grp && gm != "select");
    b.add(
        "group_tolerance",
        "Tolerance (ms)",
        "Switch only when another member is at least this much faster (avoids switching back and forth)",
        Kind::Int { min: 0, max: 5000 },
        grp && gm == "auto",
    );
    // VMess / VLESS / TUIC / Trojan
    b.add(
        "vmess_cipher",
        "Encryption · alterId",
        "Use what the server expects. alterId 0 = current servers (AEAD)",
        Kind::Choice(
            crate::checks::VMESS_CIPHERS
                .iter()
                .enumerate()
                .map(|(i, c)| (c.to_string(), if i == 0 { tr("auto (recommended)") } else { c.to_string() }))
                .collect(),
        ),
        ot == "vmess",
    );
    b.add("vmess_alter_id", "", "", Kind::Int { min: 0, max: 65535 }, ot == "vmess");
    b.add("uuid", "User UUID", "", Kind::Text, matches!(ot, "vless" | "vmess" | "tuic"));
    let secs: Vec<(&str, String)> = if ot == "vmess" {
        vec![("none", tr("None")), ("tls", "TLS".into())]
    } else {
        vec![("none", tr("None")), ("tls", "TLS".into()), ("reality", "REALITY".into())]
    };
    b.add(
        "security",
        "Security · flow",
        if ot == "vless" {
            "Use what the server is set up with (REALITY usually with xtls-rprx-vision)"
        } else {
            "Use what the server is set up with"
        },
        choice(&secs),
        matches!(ot, "vless" | "vmess"),
    )
    .rerender = true;
    b.add(
        "vless_flow",
        "",
        "",
        choice(&[("", tr("(no flow)")), ("xtls-rprx-vision", "xtls-rprx-vision".into())]),
        ot == "vless",
    );
    b.add(
        "reality_public_key",
        "REALITY",
        "The server's public key and short ID. SNI = the site the server imitates",
        Kind::Text,
        ot == "vless" && sec == "reality",
    );
    b.add("reality_short_id", "", "", Kind::Text, ot == "vless" && sec == "reality");
    let fps = ["", "chrome", "firefox", "safari", "edge", "ios", "android", "random", "randomized"];
    b.add(
        "fingerprint",
        "Fingerprint · ALPN",
        "Look like this browser's TLS (uTLS). Empty ALPN = default",
        Kind::Choice(
            fps.iter().map(|f| (f.to_string(), if f.is_empty() { tr("(none)") } else { f.to_string() })).collect(),
        ),
        fp && ot != "tuic",
    );
    b.add("alpn", if fp && ot != "tuic" { "" } else { "Fingerprint · ALPN" }, "", Kind::Text, fp);
    b.add(
        "transport",
        "Transport",
        "WebSocket and gRPC are used behind CDNs. Path for WebSocket, service name for gRPC",
        choice(&[("tcp", "TCP".into()), ("ws", "WebSocket".into()), ("grpc", "gRPC".into())]),
        matches!(ot, "vless" | "vmess" | "trojan"),
    )
    .rerender = true;
    b.add("transport_path", "", "", Kind::Text, matches!(ot, "vless" | "vmess" | "trojan") && tr_kind != "tcp");
    b.add("transport_host", "", "", Kind::Text, matches!(ot, "vless" | "vmess" | "trojan") && tr_kind == "ws");
    b.add(
        "tuic_congestion",
        "Congestion control · UDP relay",
        "Use what the TUIC server expects",
        plain(&["bbr", "cubic", "new_reno"]),
        ot == "tuic",
    );
    b.add("tuic_udp_relay", "", "", plain(&["native", "quic"]), ot == "tuic");
    // TLS
    b.add(
        "tls_verify",
        "Verify server certificate",
        "Turn off for servers with self-signed certificates (pinning the fingerprint is recommended)",
        Kind::Bool,
        tls && !(ot == "vless" && sec == "reality"),
    );
    b.add("sni", "SNI (server name)", "Servers that check the name (e.g. the official Hysteria2 server) refuse a wrong one. Connecting by IP? Click 'Detect from server'.", Kind::Text, tls);
    b.push(field(
        "sni.detect",
        "Detect from server",
        "Read the server's certificate and use its name",
        Kind::Button,
        json!(null),
        tls,
    ));
    b.add(
        "pin_sha256",
        "Pin certificate fingerprint",
        "If set, only a certificate with this fingerprint is accepted",
        Kind::Text,
        ot == "hysteria2",
    );
    // Shadowsocks / Hysteria2
    b.add("ss_method", "Cipher", "Must match the server", ss_methods(), ot == "shadowsocks");
    b.add("ss_password", "Shadowsocks password", "", Kind::Secret, ot == "shadowsocks");
    b.add("hy2_password", "Hysteria2 password", "", Kind::Secret, ot == "hysteria2");
    b.add(
        "hy2_obfs_password",
        "Salamander obfuscation",
        "Only if the server uses it (obfs-password in the link)",
        Kind::Secret,
        ot == "hysteria2",
    );
    b.add(
        "hy2_up_mbps",
        "Bandwidth up · down (Mbps)",
        "Brutal: enter your real line speed to push through lossy networks. 0 = automatic (BBR)",
        Kind::Int { min: 0, max: 100000 },
        ot == "hysteria2",
    );
    b.add("hy2_down_mbps", "", "", Kind::Int { min: 0, max: 100000 }, ot == "hysteria2");
    b.add(
        "hy2_ports",
        "Port hopping · interval (s)",
        "Ports the server listens on, e.g. 20000-30000 or 443,5000-6000. Empty = off",
        Kind::Text,
        ot == "hysteria2",
    );
    b.add("hy2_hop_interval", "", "", Kind::Int { min: 5, max: 3600 }, ot == "hysteria2");
    // WireGuard
    let wg = ot == "wireguard";
    b.push(field(
        "ui.wg.import",
        "Import .conf…",
        "Fill everything in from a .conf file from your VPN provider or WireGuard server",
        Kind::Button,
        json!(null),
        wg,
    ));
    b.add("wg_private_key", "Private key", "", Kind::Secret, wg);
    b.push(field("gen.outbound.wg_private_key", "Generate", "", Kind::Button, json!(null), wg));
    b.push(field("wg_public", "Public key", "", Kind::Info, json!(crate::wg::public_of(&st("wg_private_key"))), wg));
    b.add(
        "wg_address",
        "Tunnel address",
        "This side's address inside the tunnel (Address in the .conf)",
        Kind::Text,
        wg,
    );
    b.add("wg_peer_public_key", "Server public key", "PublicKey of the [Peer]", Kind::Text, wg);
    b.add("wg_preshared_key", "Preshared key", "", Kind::Secret, wg);
    b.add(
        "wg_allowed_ips",
        "Allowed IPs",
        "Destinations sent into the tunnel. 0.0.0.0/0, ::/0 = everything",
        Kind::Text,
        wg,
    );
    b.add(
        "wg_mtu",
        "MTU · keepalive (s)",
        "Keepalive 0 = off. 25 helps behind NAT",
        Kind::Int { min: 576, max: 9200 },
        wg,
    );
    b.add("wg_keepalive", "", "", Kind::Int { min: 0, max: 3600 }, wg);
    b.push(field("", "Check connection", "", Kind::Heading, json!(null), true));
    b.push(field(
        "probe",
        "Check connection",
        "Tests the current, unsaved settings: connects to an HTTPS site through this outbound.",
        Kind::Button,
        json!(null),
        true,
    ));
    b.out
}

/// The fields of a router / IF block: its name, then per rule its name, conditions and target; the default.
pub fn router_form(g: &Obj, node: &Value) -> Vec<Field> {
    let id = s(node, "id");
    let is_if = node["style"] == json!("if");
    let switch = is_if && node["switch"] == json!(true);
    let targets = router_targets(g, id);
    let mut out = vec![field("name", "Name", "", Kind::Text, node["name"].clone(), true)];
    if is_if {
        out.push(field(
            "switch",
            "Manual switch",
            "No condition: switched on, everything goes to 'then'; off, to 'else' (a kill switch)",
            Kind::Bool,
            json!(node["switch"] == json!(true)),
            true,
        ));
        out.last_mut().unwrap().rerender = true;
        out.push(field("switch_on", "Switched on", "", Kind::Bool, json!(node["switch_on"] == json!(true)), switch));
    }
    let advanced = ROUTER_ADVANCED.load(Ordering::Relaxed);
    // Simple / Advanced (Python's segmented switch above the rules); advanced shows the long note
    let mut view = field(
        "router_view",
        if is_if { "Condition" } else { "Rules (checked from the top; the first match wins)" },
        "",
        choice(&[("simple", tr("Simple")), ("advanced", tr("Advanced"))]),
        json!(if advanced { "advanced" } else { "simple" }),
        !switch,
    );
    if advanced {
        view.desc = tr(
            "Inside one rule, any one of the address conditions (domains, IPs, GeoIP, Geosite, private network) is enough; port and program must also match. Separate several values with commas. Program rules work for connections from this PC. GeoIP and Geosite data is downloaded once a day.",
        );
    }
    view.rerender = true;
    out.push(view);
    let rules: Vec<&Value> = node["rules"].as_array().into_iter().flatten().collect();
    for (i, r) in rules.iter().enumerate() {
        let base = format!("rules.{i}");
        let title = if is_if { "IF".to_string() } else { tr("Rule {n}").replace("{n}", &(i + 1).to_string()) };
        let buttons = if is_if {
            vec![]
        } else {
            vec![
                (format!("rule.up.{i}"), "↑".into()),
                (format!("rule.down.{i}"), "↓".into()),
                (format!("rule.del.{i}"), tr("Remove")),
            ]
        };
        out.push(Field {
            key: format!("rule.{i}"),
            label: title,
            desc: String::new(),
            kind: Kind::Item(buttons),
            value: json!(""),
            visible: true,
            rerender: false,
        });
        if !is_if {
            out.push(field(&format!("{base}.name"), "Name", "", Kind::Text, r["name"].clone(), true));
        }
        let text_of = |k: &str| {
            r["match"][k]
                .as_array()
                .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
                .unwrap_or_default()
        };
        let private = json!(r["match"]["ip_private"] == json!(true));
        if advanced {
            for (k, label, example) in CONDS {
                out.push(Field {
                    key: format!("{base}.match.{k}"),
                    label: tr(label),
                    desc: example.into(),
                    kind: Kind::Text,
                    value: json!(text_of(k)),
                    visible: !switch,
                    rerender: false,
                });
                if k == "process_name" {
                    out.push(field(
                        &format!("ui.pick.{base}.match.process_name"),
                        "Choose...",
                        "",
                        Kind::Button,
                        json!(null),
                        !switch,
                    ));
                }
            }
            out.push(field(
                &format!("{base}.match.ip_private"),
                "Private network addresses",
                "",
                Kind::Bool,
                private,
                !switch,
            ));
        } else {
            // a row per condition in use, each with its remove button (Python `CondRow`)
            let used = simple_conds(r);
            for k in &used {
                if *k == "ip_private" {
                    out.push(field(
                        &format!("{base}.match.ip_private"),
                        "Private network addresses",
                        "LAN, loopback and other private IPs",
                        Kind::Bool,
                        private.clone(),
                        !switch,
                    ));
                } else {
                    let (_, label, example) = CONDS.iter().find(|c| c.0 == *k).copied().unwrap();
                    out.push(Field {
                        key: format!("{base}.match.{k}"),
                        label: tr(label),
                        desc: example.into(),
                        kind: Kind::Text,
                        value: json!(text_of(k)),
                        visible: !switch,
                        rerender: false,
                    });
                    if *k == "process_name" {
                        out.push(field(
                            &format!("ui.pick.{base}.match.process_name"),
                            "Choose...",
                            "",
                            Kind::Button,
                            json!(null),
                            !switch,
                        ));
                    }
                }
                out.push(field(
                    &format!("rule.cond.del.{i}.{k}"),
                    "Remove condition",
                    "",
                    Kind::Button,
                    json!(null),
                    !switch,
                ));
            }
            let free: Vec<(String, String)> = CONDS
                .iter()
                .filter(|c| !used.contains(&c.0))
                .map(|c| (c.0.to_string(), tr(c.1)))
                .chain(
                    (!used.contains(&"ip_private"))
                        .then(|| ("ip_private".to_string(), tr("Private network addresses"))),
                )
                .collect();
            if !free.is_empty() {
                let first = free[0].0.clone();
                out.push(field(
                    &format!("{base}.cond_new"),
                    "Add condition",
                    "One matching website / IP / country condition is enough; ports and programs must also match.",
                    Kind::Choice(free),
                    json!(first),
                    !switch,
                ));
                out.push(field(&format!("rule.cond.add.{i}"), "Add", "", Kind::Button, json!(null), !switch));
            }
        }
        out.push(field(
            &format!("{base}.target"),
            if is_if { "then" } else { "then →" },
            "",
            targets.clone(),
            json!(r["target"].as_str().unwrap_or("")),
            true,
        ));
    }
    if !is_if {
        let templates: Vec<(String, String)> =
            rule_templates().into_iter().enumerate().map(|(i, t)| (i.to_string(), t.0)).collect();
        out.push(field("rule_template", "Add rule", "", Kind::Choice(templates), json!("0"), true));
        out.push(field("rule.add", "Add", "", Kind::Button, json!(null), true));
    }
    out.push(field(
        "default",
        if is_if { "else" } else { "Default" },
        "Where traffic goes when no rule matches",
        targets,
        json!(node["default"].as_str().unwrap_or("")),
        true,
    ));
    out
}

/// The fields of a pass-through block (Python `RelaySection`): lanes, wiring and where each output goes; a
/// wireless pair: its titles and number.
pub fn relay_form(g: &Obj, node: &Value) -> Vec<Field> {
    let id = s(node, "id");
    let wireless = node["wireless"] == json!(true);
    let routed = node["routed"] == json!(true);
    let ins: Vec<&Value> = node["inputs"].as_array().into_iter().flatten().collect();
    let outs: Vec<&Value> = node["outputs"].as_array().into_iter().flatten().collect();
    let mut out = vec![field(
        "name",
        "Name",
        if wireless { "The name above is the input block's title." } else { "" },
        Kind::Text,
        node["name"].clone(),
        true,
    )];
    if wireless {
        out.push(field("name_out", "Output block title", "", Kind::Text, node["name_out"].clone(), true));
        out.push(field("number", "Pair number", "", Kind::Int { min: 1, max: 999 }, node["number"].clone(), true));
    } else {
        out.push(field(
            "routed",
            "Wiring",
            "",
            choice(&[("false", tr("Straight")), ("true", tr("Routed (wire lanes freely)"))]),
            json!(routed.to_string()),
            true,
        ));
        out.last_mut().unwrap().rerender = true;
        out.push(field(
            "lanes_in",
            "Inputs",
            "",
            Kind::Int { min: 1, max: graph::MAX_LANES as i64 },
            json!(ins.len()),
            true,
        ));
        out.last_mut().unwrap().rerender = true;
        out.push(field(
            "lanes_out",
            "Outputs",
            "",
            Kind::Int { min: 1, max: graph::MAX_LANES as i64 },
            json!(outs.len()),
            routed,
        ));
        out.last_mut().unwrap().rerender = true;
        if routed {
            let mut wiring = vec![(String::new(), tr("(not wired)"))];
            wiring.extend(
                outs.iter()
                    .enumerate()
                    .map(|(k, o)| (s(o, "id").to_string(), tr("→ output {n}").replace("{n}", &(k + 1).to_string()))),
            );
            for (i, x) in ins.iter().enumerate() {
                out.push(field(
                    &format!("in.{i}.out"),
                    "",
                    "",
                    Kind::Choice(wiring.clone()),
                    json!(x["out"].as_str().unwrap_or("")),
                    true,
                ));
                out.last_mut().unwrap().label = tr("Input {n}").replace("{n}", &(i + 1).to_string());
            }
        }
    }
    let targets = router_targets(g, id);
    for (k, o) in outs.iter().enumerate() {
        let mut f =
            field(&format!("out.{k}.target"), "", "", targets.clone(), json!(o["target"].as_str().unwrap_or("")), true);
        f.label = tr("Output {n}").replace("{n}", &(k + 1).to_string());
        out.push(f);
    }
    out
}

/// The editor fields of a block of the given kind.
pub fn form(g: &Obj, glob: &Map<String, Value>, kind: &str, node: &Value) -> Vec<Field> {
    match kind {
        "inbound" => inbound_form(g, glob, node),
        "outbound" => outbound_form(g, node),
        "router" => router_form(g, node),
        _ => relay_form(g, node),
    }
}

/// a typed value in the field's type (numbers from text boxes, booleans from switches)
fn typed(old: &Value, new: &Value) -> Value {
    match (old, new) {
        (Value::Number(_), Value::String(t)) => crate::py::int(t).map_or_else(|| old.clone(), |n| json!(n)),
        (Value::Bool(_), Value::String(t)) => json!(t == "true"),
        _ => new.clone(),
    }
}

fn opt_id(v: &Value) -> Value {
    match v.as_str() {
        Some(t) if !t.is_empty() => json!(t),
        _ => Value::Null,
    }
}

fn truth(v: &Value) -> bool {
    v == &json!(true) || v == &json!("true")
}

/// Python `graph.set_lane_count`: grow or shrink a side (straight blocks keep both sides equal)
pub fn set_lane_count(n: &mut Value, side: &str, count: usize) {
    let count = if n["wireless"] == json!(true) { 1 } else { count.clamp(1, graph::MAX_LANES) };
    let routed = n["routed"] == json!(true);
    let sides: Vec<&str> = if routed { vec![side] } else { vec!["inputs", "outputs"] };
    for sd in sides {
        let key = if sd == "inputs" { "out" } else { "target" };
        let arr = n[sd].as_array_mut().unwrap();
        while arr.len() < count {
            arr.push(Value::Object(graph::new_lane(key)));
        }
        arr.truncate(count);
    }
    let out_ids: Vec<Value> = n["outputs"].as_array().unwrap().iter().map(|o| o["id"].clone()).collect();
    let outs = n["outputs"].clone();
    for (i, x) in n["inputs"].as_array_mut().unwrap().iter_mut().enumerate() {
        if !routed {
            x["out"] = outs[i]["id"].clone();
        } else if !out_ids.contains(&x["out"]) {
            x["out"] = Value::Null;
        }
    }
    if !routed {
        let gaps: Vec<Value> = n["inputs"].as_array().unwrap().iter().map(|x| x["gap"].clone()).collect();
        for (o, gap) in n["outputs"].as_array_mut().unwrap().iter_mut().zip(gaps) {
            o["gap"] = gap;
        }
    }
}

/// The draft with the values typed so far put in (Python `collect`), validated like a loaded block; links stay.
pub fn apply(kind: &str, node: &Value, values: &Map<String, Value>) -> Value {
    let mut n = node.clone();
    let strip_keys = [
        "host",
        "tls_cert",
        "tls_key",
        "ss_password",
        "fwd_host",
        "wg_private_key",
        "wg_address",
        "wg_endpoint",
        "sni",
        "pin_sha256",
        "hy2_ports",
        "bind",
        "interface",
        "group_test_url",
        "wg_peer_public_key",
        "wg_preshared_key",
        "wg_allowed_ips",
        "uuid",
        "reality_public_key",
        "reality_short_id",
        "alpn",
        "transport_path",
        "transport_host",
        "sub_url",
    ];
    let mut follows: Vec<(String, bool)> = vec![];
    let mut lanes: Vec<(&str, usize)> = vec![];
    for (key, v) in values {
        let parts: Vec<&str> = key.split('.').collect();
        match parts.as_slice() {
            ["name"] => n["name"] = json!(v.as_str().unwrap_or("").trim()),
            ["name_out"] => n["name_out"] = json!(crate::py::head(v.as_str().unwrap_or("").trim(), 60)),
            [k @ ("autostart" | "auto_restart" | "switch" | "switch_on")] => n[*k] = json!(truth(v)),
            ["number"] => n["number"] = typed(&json!(0), v),
            ["routed"] => {
                let routed = truth(v);
                if routed != (n["routed"] == json!(true)) {
                    n["routed"] = json!(routed);
                    if !routed {
                        let k = n["inputs"].as_array().map_or(1, Vec::len);
                        if let Some(o) = n["outputs"].as_array_mut() {
                            o.truncate(k);
                        }
                    }
                }
            }
            ["router_view"] => ROUTER_ADVANCED.store(v.as_str() == Some("advanced"), Ordering::Relaxed),
            ["lanes_in"] => lanes.push(("inputs", typed(&json!(0), v).as_u64().unwrap_or(1) as usize)),
            ["lanes_out"] => lanes.push(("outputs", typed(&json!(0), v).as_u64().unwrap_or(1) as usize)),
            [k @ ("target" | "via" | "default")] => n[*k] = opt_id(v),
            ["in", i, "out"] => {
                if let Some(x) = i.parse::<usize>().ok().and_then(|i| n["inputs"].get_mut(i)) {
                    x["out"] = opt_id(v);
                }
            }
            ["out", i, "target"] => {
                if let Some(x) = i.parse::<usize>().ok().and_then(|i| n["outputs"].get_mut(i)) {
                    x["target"] = opt_id(v);
                }
            }
            [section @ ("inbound" | "outbound"), k] => {
                let mut v = typed(&n[*section][*k], v);
                if strip_keys.contains(k)
                    && let Some(t) = v.as_str()
                {
                    v = json!(crate::py::strip(t));
                }
                n[*section][*k] = v;
            }
            ["follow", k] => follows.push((k.to_string(), truth(v))),
            ["overrides", k] => {
                let value = match *k {
                    "allowed_domains" => {
                        let items = crate::singbox::compile::split(v.as_str().unwrap_or(""));
                        if items.is_empty() { json!(["*"]) } else { json!(items) }
                    }
                    "timeout" => typed(&json!(0), v),
                    "dns_server" => {
                        let text = v.as_str().unwrap_or("");
                        match crate::config::parse_dns_list(text) {
                            Ok((main, extra)) => {
                                n["overrides"]["dns_extra"] = json!(extra);
                                json!(main)
                            }
                            // (kept as typed: the check names it)
                            Err(_) => {
                                n["overrides"]["dns_extra"] = json!([]);
                                json!(crate::py::strip(text))
                            }
                        }
                    }
                    _ => json!(truth(v)),
                };
                n["overrides"][*k] = value;
            }
            ["rules", i, "name"] => {
                if let Some(r) = i.parse::<usize>().ok().and_then(|i| n["rules"].get_mut(i)) {
                    r["name"] = json!(v.as_str().unwrap_or("").trim());
                }
            }
            ["rules", i, "target"] => {
                if let Some(r) = i.parse::<usize>().ok().and_then(|i| n["rules"].get_mut(i)) {
                    r["target"] = opt_id(v);
                }
            }
            ["rules", i, "match", k] => {
                let Some(r) = i.parse::<usize>().ok().and_then(|i| n["rules"].get_mut(i)) else { continue };
                if *k == "ip_private" {
                    r["match"][*k] = json!(truth(v));
                } else {
                    r["match"][*k] = json!(crate::singbox::compile::split(v.as_str().unwrap_or("")));
                }
            }
            _ => {}
        }
    }
    // a card following the global value has null; switching "use global" off starts from the shown value
    for (k, follow) in follows {
        if follow {
            n["overrides"][&k] = Value::Null;
            if k == "dns_server" {
                n["overrides"]["dns_extra"] = Value::Null;
            }
        } else if n["overrides"][&k].is_null() {
            n["overrides"][&k] = values.get(&format!("overrides.{k}")).cloned().unwrap_or(json!(""));
            if k == "dns_server" && n["overrides"]["dns_extra"].is_null() {
                n["overrides"]["dns_extra"] = json!([]);
            }
        }
    }
    for (side, count) in lanes {
        set_lane_count(&mut n, side, count);
    }
    match kind {
        "inbound" => {
            let ib_type = s(&n["inbound"], "type").to_string();
            if !matches!(ib_type.as_str(), "socks5" | "http" | "https") {
                n["inbound"]["auth_enabled"] = json!(false);
            }
            let target = n["target"].clone();
            let mut v = Value::Object(graph::validate_inbound_node(&n));
            v["target"] = target;
            v
        }
        "outbound" => {
            let ot = s(&n["outbound"], "type").to_string();
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
        _ => {
            let (name, name_out) = (n["name"].clone(), n["name_out"].clone());
            let mut v = Value::Object(graph::validate_relay_node(&n));
            v["name"] = name;
            if v["wireless"] == json!(true) {
                v["name_out"] = name_out;
            }
            v
        }
    }
}

/// a random password for Hysteria2 (Python `secrets.token_urlsafe(18)`)
fn token() -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(crate::singbox::process::random_bytes(18))
}

/// Python `ss_generate_password`: a base64 key for 2022 ciphers, a random password otherwise
fn ss_password(method: &str) -> String {
    use base64::Engine;
    match crate::checks::SS_METHODS.iter().find(|m| m.0 == method) {
        Some((_, len, true)) => {
            base64::engine::general_purpose::STANDARD.encode(crate::singbox::process::random_bytes(*len))
        }
        _ => token(),
    }
}

/// Fill in what Python fills in when it opens a card: a Shadowsocks / Hysteria2 password, a WireGuard key.
pub fn on_open(kind: &str, node: &mut Value) {
    if kind != "inbound" {
        return;
    }
    let ib = &mut node["inbound"];
    match ib["type"].as_str() {
        Some("shadowsocks") if s(ib, "ss_password").is_empty() => {
            ib["ss_password"] = json!(ss_password(s(ib, "ss_method")))
        }
        Some("hysteria2") if s(ib, "hy2_password").is_empty() => ib["hy2_password"] = json!(token()),
        Some("wireguard") if s(ib, "wg_private_key").is_empty() => ib["wg_private_key"] = json!(crate::wg::keypair().0),
        _ => {}
    }
}

/// What a button did: the changed draft, text to show (a device's config), a message (ok, title, text).
pub struct Acted {
    pub node: Value,
    pub show: Option<(String, String)>,
    pub notice: Option<(bool, String, String)>,
    /// a file for the window to save: suggested name, text (a WireGuard device's .conf)
    pub save: Option<(String, String)>,
    /// the window opens the program picker for this field (the "Programs..." rule template)
    pub pick: Option<String>,
}

/// "Paste share link": the outbound from the link (the subscription's servers stay), the link's name when the
/// block still has an empty or type name (Python `_paste_link`)
fn paste_link(n: &mut Value, clipboard: &str) -> (bool, String, String) {
    let (fields, name) = match crate::links::parse(clipboard) {
        Ok(x) => x,
        Err(e) => {
            let text = format!(
                "{e}
{}",
                tr("Copy a vless://, vmess://, trojan://, tuic://, ss:// or hysteria2:// link first")
            );
            return (false, tr("Cannot use the clipboard"), text);
        }
    };
    let old = n["outbound"].clone();
    let mut ob = crate::config::outbound_defaults();
    for k in ["sub_servers", "sub_updated"] {
        if let Some(v) = old.get(k) {
            ob.insert(k.into(), v.clone());
        }
    }
    for (k, v) in fields {
        ob.insert(k, v);
    }
    n["outbound"] = Value::Object(ob);
    let current = py_strip(s(n, "name"));
    let Kind::Choice(types) = outbound_types() else { unreachable!() };
    if current.is_empty() || types.iter().any(|(_, label)| current.starts_with(label.as_str())) {
        n["name"] = json!(crate::py::head(&name, 60));
    }
    (true, tr("Filled in from the link"), name)
}

fn py_strip(t: &str) -> String {
    crate::py::strip(t).to_string()
}

/// A button of the editor (`action` = the button field's key), on the draft with the typed values already in.
pub fn act(g: &Obj, kind: &str, node: &Value, action: &str, values: &Map<String, Value>) -> Result<Acted, String> {
    let mut n = node.clone();
    let mut show = None;
    let mut notice = None;
    let mut save = None;
    let mut pick = None;
    let parts: Vec<&str> = action.split('.').collect();
    let idx = |p: &str| p.parse::<usize>().ok();
    match parts.as_slice() {
        ["gen", section, k] => {
            let v = match *k {
                "ss_password" => ss_password(s(&n[*section], "ss_method")),
                "hy2_password" => token(),
                _ => crate::wg::keypair().0,
            };
            n[*section][*k] = json!(v);
        }
        ["wgdev", "add"] => {
            let ib = &n["inbound"];
            let mut devs = peers(ib);
            let used: Vec<String> = devs.iter().filter_map(|x| x["address"].as_str().map(String::from)).collect();
            let Some(addr) = crate::wg::next_peer_address(s(ib, "wg_address"), &used) else {
                return Err(tr("WireGuard server: the tunnel address must be like 10.66.0.1/24"));
            };
            let (private, public) = crate::wg::keypair();
            let typed_name =
                crate::py::head(values.get("wg_new_name").and_then(Value::as_str).unwrap_or("").trim(), 40);
            let name = if typed_name.is_empty() {
                tr("Device {n}").replace("{n}", &(devs.len() + 1).to_string())
            } else {
                typed_name
            };
            devs.push(json!({"name": name, "private_key": private, "public_key": public, "address": addr}));
            n["inbound"]["wg_peers"] = json!(serde_json::to_string(&devs).unwrap());
        }
        ["wgdev", "del", i] => {
            let mut devs = peers(&n["inbound"]);
            if let Some(i) = idx(i).filter(|i| *i < devs.len()) {
                devs.remove(i);
            }
            n["inbound"]["wg_peers"] =
                json!(if devs.is_empty() { String::new() } else { serde_json::to_string(&devs).unwrap() });
        }
        ["wgdev", "conf", i] => {
            let devs = peers(&n["inbound"]);
            let Some(p) = idx(i).and_then(|i| devs.get(i)) else { return Err(String::new()) };
            let endpoint = match s(&n["inbound"], "wg_endpoint") {
                "" => crate::wg::local_lan_ip(),
                e => e.to_string(),
            };
            let title = p.get("name").map_or_else(|| "?".to_string(), crate::singbox::compile::py_str);
            show = Some((title, crate::wg::client_conf(&n["inbound"], p, &endpoint)));
        }
        ["wgdev", "save", i] => {
            let devs = peers(&n["inbound"]);
            let Some(p) = idx(i).and_then(|i| devs.get(i)) else { return Err(String::new()) };
            let endpoint = match s(&n["inbound"], "wg_endpoint") {
                "" => crate::wg::local_lan_ip(),
                e => e.to_string(),
            };
            // (Python `_wgi_save`: the device name with only letters, digits, - and _)
            let name = p.get("name").map_or_else(String::new, crate::singbox::compile::py_str);
            let safe: String =
                name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
            let safe = if safe.is_empty() { "device".to_string() } else { safe };
            save = Some((format!("{safe}.conf"), crate::wg::client_conf(&n["inbound"], p, &endpoint)));
        }
        ["wg", "import"] => {
            let text = values.get("conf_text").and_then(Value::as_str).unwrap_or("");
            let file = values.get("conf_name").and_then(Value::as_str).unwrap_or("");
            match crate::wg::parse_conf(text.strip_prefix('\u{feff}').unwrap_or(text)) {
                Ok((fields, notes)) => {
                    for (k, v) in fields {
                        n["outbound"][&k] = v;
                    }
                    // a new block still named after its type takes the file's name (Python `_wg_import`)
                    let current = py_strip(s(&n, "name"));
                    if current.is_empty() || current.starts_with(&graph::outbound_label("wireguard")) {
                        let stem = std::path::Path::new(file)
                            .file_stem()
                            .map(|x| x.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        n["name"] = json!(crate::py::head(&stem, 60));
                    }
                    let text = if notes.is_empty() { file.to_string() } else { notes.join("\n") };
                    notice = Some((true, tr("Imported"), text));
                }
                Err(e) => notice = Some((false, tr("Cannot import"), e)),
            }
        }
        ["iface", "refresh"] => refresh_ifaces(),
        ["rule", "cond", "add", i] => {
            let Some(r) = idx(i).and_then(|i| n["rules"].get_mut(i)) else { return Err(String::new()) };
            // (untouched, the list shows its first condition not in use)
            let used = simple_conds(r);
            let first_free = CONDS.iter().map(|c| c.0).chain(["ip_private"]).find(|c| !used.contains(c)).unwrap_or("");
            let k =
                values.get(&format!("rules.{i}.cond_new")).and_then(Value::as_str).unwrap_or(first_free).to_string();
            if k == "ip_private" {
                r["match"]["ip_private"] = json!(true);
            } else if CONDS.iter().any(|c| c.0 == k) {
                // the rule's current rows stay shown even while empty
                let rid = s(r, "id").to_string();
                let current = simple_conds(r);
                let mut shown = SHOWN.lock().unwrap();
                for c in current {
                    if !shown.contains(&(rid.clone(), c.to_string())) {
                        shown.push((rid.clone(), c.to_string()));
                    }
                }
                shown.push((rid, k));
            }
        }
        ["rule", "cond", "del", i, k] => {
            let Some(r) = idx(i).and_then(|i| n["rules"].get_mut(i)) else { return Err(String::new()) };
            r["match"][*k] = if *k == "ip_private" { json!(false) } else { json!([]) };
            let rid = s(r, "id").to_string();
            SHOWN.lock().unwrap().retain(|(id, c)| !(*id == rid && c == k));
        }
        ["member", "add"] => {
            // (untouched, the list shows the first block it can add)
            let first = addable_members(g, &n).first().map_or(String::new(), |a| a.0.clone());
            if let Some(m) = values
                .get("member_new")
                .and_then(Value::as_str)
                .or(Some(first.as_str()))
                .filter(|m| !m.is_empty())
                .map(String::from)
            {
                let m = m.as_str();
                let members = n["members"].as_array_mut().unwrap();
                if !members.contains(&json!(m)) {
                    members.push(json!(m));
                }
            }
        }
        [list @ ("member" | "rule"), dir @ ("up" | "down" | "del"), i] => {
            let key = if *list == "member" { "members" } else { "rules" };
            let items = n[key].as_array_mut().unwrap();
            if let Some(i) = idx(i).filter(|i| *i < items.len()) {
                match *dir {
                    "up" if i > 0 => items.swap(i, i - 1),
                    "down" if i + 1 < items.len() => items.swap(i, i + 1),
                    "del" => {
                        items.remove(i);
                    }
                    _ => {}
                }
            }
        }
        ["link", "paste"] => {
            notice = Some(paste_link(&mut n, values.get("clipboard").and_then(Value::as_str).unwrap_or("")));
        }
        ["rule", "add"] => {
            // from the chosen template (Python `RouterSection.add_rule`): its target is the first block of its type
            let k = n["rules"].as_array().map_or(0, Vec::len);
            let pick_t =
                values.get("rule_template").and_then(Value::as_str).and_then(|t| t.parse::<usize>().ok()).unwrap_or(0);
            let (_, name, m, target_type) =
                rule_templates().into_iter().nth(pick_t).unwrap_or_else(|| rule_templates().remove(0));
            let target = target_type
                .and_then(|t| list(g, "outbounds").find(|x| x["outbound"]["type"] == json!(t)))
                .map_or(Value::Null, |x| x["id"].clone());
            let id = crate::ids::new_id();
            if m.get("process_name").is_some() {
                SHOWN.lock().unwrap().push((id.clone(), "process_name".into()));
                pick = Some(format!("rules.{k}.match.process_name"));
            }
            let name = name.unwrap_or_else(|| tr("Rule {n}").replace("{n}", &(k + 1).to_string()));
            n["rules"]
                .as_array_mut()
                .unwrap()
                .push(json!({"id": id, "name": name, "match": m, "target": target, "color": ""}));
        }
        _ => return Err(String::new()),
    }
    let node = apply(kind, &n, &Map::new());
    Ok(Acted { node, show, notice, save, pick })
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
    // links into lanes a pass-through no longer has are cut (Python drop_dead_lane_links)
    if kind == "relay" {
        let lanes = graph::lane_index(&g);
        let blocks: Vec<String> = ["inbounds", "outbounds", "routers"]
            .iter()
            .flat_map(|k| list(&g, k))
            .filter_map(|n| n["id"].as_str().map(String::from))
            .collect();
        let alive = |t: &Value| -> Value {
            match t.as_str() {
                Some(t) if lanes.is_input(t) || blocks.iter().any(|b| b == t) => json!(t),
                _ => Value::Null,
            }
        };
        let fix = |g: &mut Obj, key: &str, f: &dyn Fn(&mut Value)| {
            for x in g.get_mut(key).and_then(Value::as_array_mut).into_iter().flatten() {
                f(x);
            }
        };
        fix(&mut g, "inbounds", &|n| n["target"] = alive(&n["target"]));
        fix(&mut g, "routers", &|r| {
            r["default"] = alive(&r["default"]);
            for x in r["rules"].as_array_mut().into_iter().flatten() {
                x["target"] = alive(&x["target"]);
            }
        });
        fix(&mut g, "relays", &|r| {
            for x in r["outputs"].as_array_mut().into_iter().flatten() {
                x["target"] = alive(&x["target"]);
            }
        });
    }
    g
}

/// What has to be fixed before the edited node can be saved (Python `EditorInterface._errors`).
pub fn errors(g: &Obj, kind: &str, node: &Value) -> Vec<String> {
    errors_for(g, kind, node, false)
}

/// What has to be fixed before the connection check (an unconnected card cannot be checked)
pub fn check_errors(g: &Obj, kind: &str, node: &Value) -> Vec<String> {
    errors_for(g, kind, node, true)
}

/// The proxy the connection check uses for the edited block (Python `_as_proxy`): a card as compiled with the
/// draft, or for an outbound block one that leaves through it (its chain and group members included).
pub fn as_proxy(g: &Obj, kind: &str, node: &Value) -> Option<Value> {
    let g2 = with_node(g, kind, node);
    match kind {
        "inbound" => graph::compile(&g2).into_iter().map(Value::Object).find(|p| p["id"] == node["id"]),
        "outbound" => {
            let mut p = Value::Object(crate::config::validate_proxy(
                &json!({"id": "__edit__", "name": node["name"], "outbound": node["outbound"]}),
            ));
            let resolved = graph::resolve_outbound(&g2, s(node, "id"));
            p["outbound"]["via"] = resolved.get("via").cloned().unwrap_or(Value::Null);
            if let Some(m) = resolved.get("members") {
                p["outbound"]["members"] = m.clone();
            }
            Some(p)
        }
        _ => None,
    }
}

fn errors_for(g: &Obj, kind: &str, node: &Value, for_check: bool) -> Vec<String> {
    let g2 = with_node(g, kind, node);
    let id = s(node, "id");
    match kind {
        "inbound" => {
            let mut errs = vec![];
            if let Some(t) = node["target"].as_str() {
                match graph::find(&g2, t).map(|(_, n)| s(&n["inbound"], "type").to_string()).as_deref() {
                    Some("tun") => {
                        errs.push(tr("A TUN card takes traffic from apps; another inbound cannot relay into it"))
                    }
                    Some("wireguard") => errs.push(tr(
                        "A WireGuard server takes traffic from devices; another inbound cannot relay into it",
                    )),
                    _ => {}
                }
            }
            let compiled: Vec<Value> = graph::compile(&g2).into_iter().map(Value::Object).collect();
            let Some(p) = compiled.iter().find(|p| p["id"] == node["id"]) else { return errs };
            let others: Vec<Value> = compiled.iter().filter(|o| o["id"] != node["id"]).cloned().collect();
            let unlinked = crate::checks::unlinked_error();
            // an unconnected inbound can be saved, it just cannot start
            errs.extend(
                crate::checks::check_proxy(p, &others)
                    .into_iter()
                    .filter(|e| for_check || p["unlinked"] != json!(true) || *e != unlinked),
            );
            errs
        }
        "outbound" => {
            let name = s(node, "name");
            let mut errs = if name.is_empty() { vec![tr("Enter a name")] } else { vec![] };
            if graph::via_loop(&g2, id) {
                let mut names = graph::chain_names(&g2, s(node, "via"));
                names.push(name.to_string());
                errs.push(format!("{}{}", tr("The proxy chain loops: "), names.join(" → ")));
                return errs;
            }
            errs.extend(crate::checks::outbound_errors(&graph::resolve_outbound(&g2, id)));
            errs
        }
        "router" => {
            let is_if = node["style"] == json!("if");
            let mut errs = if s(node, "name").is_empty() { vec![tr("Enter a name")] } else { vec![] };
            if node["default"].is_null() {
                errs.push(tr("Choose the default target (where traffic goes when no rule matches)"));
            }
            for (i, r) in node["rules"].as_array().into_iter().flatten().enumerate() {
                let rname = r["name"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .map(String::from)
                    .unwrap_or_else(|| (i + 1).to_string());
                if s(r, "name").is_empty() && !is_if {
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
        _ => {
            let mut errs = if s(node, "name").is_empty() { vec![tr("Enter a name")] } else { vec![] };
            if node["wireless"] == json!(true) {
                if s(node, "name_out").is_empty() {
                    errs.push(tr("Enter the output block's title"));
                }
                let number = node["number"].clone();
                if list(&g2, "relays")
                    .any(|r| r["wireless"] == json!(true) && r["number"] == number && r["id"] != node["id"])
                {
                    errs.push(
                        tr("#{n} is already used by another wireless pair")
                            .replace("{n}", &crate::singbox::compile::py_str(&number)),
                    );
                }
            }
            let lanes = graph::lane_index(&g2);
            let looped = node["outputs"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|x| x["target"].as_str().is_some_and(|t| lanes.lane_loop(t)))
                || list(&g2, "routers").filter_map(|r| r["id"].as_str()).any(|rid| graph::router_loop(&g2, rid));
            if looped {
                errs.push(tr("The links would go round in a circle"));
            }
            errs
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g() -> Obj {
        graph::validate(&json!({
            "inbounds": [{"id": "a", "name": "A", "inbound": {"type": "socks5", "port": 1080}, "target": "d"}],
            "outbounds": [
                {"id": "d", "name": "Direct", "outbound": {"type": "direct"}},
                {"id": "v", "name": "V", "outbound": {"type": "vless", "host": "h", "port": 443}},
                {"id": "grp", "name": "G", "outbound": {"type": "group"}, "members": ["d"]},
            ],
            "routers": [{"id": "r", "name": "R", "rules": [{"id": "x", "name": "kr", "match": {"geoip": ["kr"]}, "target": "d"}], "default": "d"}],
            "relays": [{"id": "rl", "name": "L", "inputs": [{"id": "L1"}, {"id": "L2"}]}],
        }))
    }

    fn node(g: &Obj, id: &str) -> Value {
        graph::find(g, id).unwrap().1.clone()
    }

    fn f<'a>(fs: &'a [Field], key: &str) -> &'a Field {
        fs.iter().find(|x| x.key == key).unwrap_or_else(|| panic!("{key}"))
    }

    fn vals(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    #[test]
    fn cards_targets_and_overrides() {
        let g = g();
        let glob = crate::config::validate_global(&json!({"timeout": 300}));
        let n = node(&g, "a");
        let fs = inbound_form(&g, &glob, &n);
        assert!(!f(&fs, "inbound.username").visible);
        assert_eq!(f(&fs, "target").value, "d");
        assert!(
            matches!(&f(&fs, "target").kind, Kind::Choice(o) if o.iter().any(|(v, l)| v == "L2" && l.contains("L")))
        );
        assert!(!f(&fs, "overrides.timeout").visible);
        assert_eq!(f(&fs, "overrides.timeout").value, 300);
        // own timeout, then back to the global one
        let n2 = apply(
            "inbound",
            &n,
            &vals(&[("follow.timeout", json!(false)), ("overrides.timeout", json!("600")), ("target", json!("r"))]),
        );
        assert_eq!(n2["overrides"]["timeout"], 600);
        assert_eq!(n2["target"], "r");
        let n3 = apply("inbound", &n2, &vals(&[("follow.timeout", json!(true))]));
        assert!(n3["overrides"]["timeout"].is_null());
        let n4 = apply(
            "inbound",
            &n,
            &vals(&[("follow.allowed_domains", json!(false)), ("overrides.allowed_domains", json!("a.com; *.b.com"))]),
        );
        assert_eq!(n4["overrides"]["allowed_domains"], json!(["a.com", "*.b.com"]));
    }

    #[test]
    fn keys_devices_members_rules() {
        let g = g();
        // a WireGuard server: key on open, two devices with their own addresses and configs
        let mut n = apply(
            "inbound",
            &node(&g, "a"),
            &vals(&[("inbound.type", json!("wireguard")), ("inbound.port", json!("51820"))]),
        );
        on_open("inbound", &mut n);
        assert!(crate::checks::valid_wg_key(n["inbound"]["wg_private_key"].as_str().unwrap()));
        let n = act(&g, "inbound", &n, "wgdev.add", &vals(&[("wg_new_name", json!("phone"))])).unwrap().node;
        let n = act(&g, "inbound", &n, "wgdev.add", &Map::new()).unwrap().node;
        let devs = peers(&n["inbound"]);
        assert_eq!((devs[0]["name"].as_str(), devs[0]["address"].as_str()), (Some("phone"), Some("10.66.0.2/32")));
        assert_eq!(devs[1]["address"], "10.66.0.3/32");
        let conf = act(&g, "inbound", &n, "wgdev.conf.0", &Map::new()).unwrap().show.unwrap().1;
        assert!(
            conf.contains("Address = 10.66.0.2/32")
                && conf.contains(":51820")
                && conf.contains(&crate::wg::public_of(n["inbound"]["wg_private_key"].as_str().unwrap()))
        );
        assert!(errors(&g, "inbound", &n).iter().all(|e| !e.contains("WireGuard")), "{:?}", errors(&g, "inbound", &n));
        let n = act(&g, "inbound", &n, "wgdev.del.0", &Map::new()).unwrap().node;
        assert_eq!(peers(&n["inbound"]).len(), 1);
        // 2022 key of the right length
        let ss = apply(
            "inbound",
            &node(&g, "a"),
            &vals(&[("inbound.type", json!("shadowsocks")), ("inbound.ss_method", json!("2022-blake3-aes-128-gcm"))]),
        );
        let ss = act(&g, "inbound", &ss, "gen.inbound.ss_password", &Map::new()).unwrap().node;
        assert!(
            crate::checks::ss_key_error("2022-blake3-aes-128-gcm", ss["inbound"]["ss_password"].as_str().unwrap())
                .is_none()
        );
        // group members: add, order, remove
        let gn =
            act(&g, "outbound", &node(&g, "grp"), "member.add", &vals(&[("member_new", json!("v"))])).unwrap().node;
        assert_eq!(gn["members"], json!(["d", "v"]));
        let gn = act(&g, "outbound", &gn, "member.up.1", &Map::new()).unwrap().node;
        assert_eq!(gn["members"], json!(["v", "d"]));
        // rules: add, name, target, order, delete
        let r = act(&g, "router", &node(&g, "r"), "rule.add", &Map::new()).unwrap().node;
        let r = apply(
            "router",
            &r,
            &vals(&[("rules.1.match.domain_suffix", json!("ads.example")), ("rules.1.target", json!("v"))]),
        );
        let r = act(&g, "router", &r, "rule.up.1", &Map::new()).unwrap().node;
        assert_eq!(r["rules"][0]["target"], "v");
        assert_eq!(r["rules"][0]["match"]["domain_suffix"], json!(["ads.example"]));
        let r = act(&g, "router", &r, "rule.del.1", &Map::new()).unwrap().node;
        assert_eq!(r["rules"].as_array().unwrap().len(), 1);
        assert!(errors(&g, "router", &r).is_empty(), "{:?}", errors(&g, "router", &r));
    }

    #[test]
    fn rule_templates_simple_view_wg_files() {
        let g = g();
        // "Local network": private addresses to the first Direct block
        let r = act(&g, "router", &node(&g, "r"), "rule.add", &vals(&[("rule_template", json!("3"))])).unwrap().node;
        let last = r["rules"].as_array().unwrap().last().unwrap().clone();
        assert_eq!((last["match"]["ip_private"].clone(), last["target"].clone()), (json!(true), json!("d")));
        // "Programs...": the window is asked to open the picker; the simple view shows the program row
        let n = r["rules"].as_array().unwrap().len();
        let a = act(&g, "router", &r, "rule.add", &vals(&[("rule_template", json!("1"))])).unwrap();
        assert_eq!(a.pick, Some(format!("rules.{n}.match.process_name")));
        let has = |node: &Value, key: &str| router_form(&g, node).iter().any(|x| x.key == key);
        assert!(has(&a.node, &format!("ui.pick.rules.{n}.match.process_name")));
        // a condition added stays while empty, and goes again with its remove button
        let b = act(
            &g,
            "router",
            &a.node,
            &format!("rule.cond.add.{n}"),
            &vals(&[(&format!("rules.{n}.cond_new"), json!("geoip"))]),
        )
        .unwrap()
        .node;
        assert!(has(&b, &format!("rules.{n}.match.geoip")) && has(&b, &format!("rules.{n}.match.process_name")));
        let c = act(&g, "router", &b, &format!("rule.cond.del.{n}.geoip"), &Map::new()).unwrap().node;
        assert!(!has(&c, &format!("rules.{n}.match.geoip")));
        // a .conf fills a WireGuard outbound and names a new block after the file
        let w = apply("outbound", &node(&g, "v"), &vals(&[("outbound.type", json!("wireguard")), ("name", json!(""))]));
        let conf =
            "[Interface]\nPrivateKey = k\nAddress = 10.0.0.2/32\n[Peer]\nPublicKey = p\nEndpoint = vpn.example:51820\n";
        let w = act(
            &g,
            "outbound",
            &w,
            "wg.import",
            &vals(&[("conf_text", json!(conf)), ("conf_name", json!("home-vpn.conf"))]),
        )
        .unwrap();
        assert_eq!(
            (w.node["outbound"]["host"].clone(), w.node["outbound"]["port"].clone(), w.node["name"].clone()),
            (json!("vpn.example"), json!(51820), json!("home-vpn"))
        );
        assert!(w.notice.unwrap().0);
        // a server's device saved as a file, named after the device
        let mut s = apply("inbound", &node(&g, "a"), &vals(&[("inbound.type", json!("wireguard"))]));
        on_open("inbound", &mut s);
        let s = act(&g, "inbound", &s, "wgdev.add", &vals(&[("wg_new_name", json!("my phone"))])).unwrap().node;
        let saved = act(&g, "inbound", &s, "wgdev.save.0", &Map::new()).unwrap().save.unwrap();
        assert_eq!(saved.0, "my_phone.conf");
        assert!(saved.1.starts_with("[Interface]"));
    }

    #[test]
    fn pass_through_lanes() {
        let g = g();
        let n = node(&g, "rl");
        let fs = relay_form(&g, &n);
        assert_eq!(f(&fs, "lanes_in").value, 2);
        assert!(!f(&fs, "lanes_out").visible);
        let n = apply("relay", &n, &vals(&[("lanes_in", json!("3")), ("out.0.target", json!("d"))]));
        assert_eq!(n["inputs"].as_array().unwrap().len(), 3);
        assert_eq!(n["outputs"].as_array().unwrap().len(), 3);
        assert_eq!(n["outputs"][0]["target"], "d");
        let n = apply("relay", &n, &vals(&[("routed", json!("true"))]));
        let n = apply("relay", &n, &vals(&[("lanes_out", json!("1")), ("in.2.out", json!(""))]));
        assert_eq!(n["outputs"].as_array().unwrap().len(), 1);
        assert!(n["inputs"][2]["out"].is_null());
        assert!(errors(&g, "relay", &n).is_empty());
    }
}
