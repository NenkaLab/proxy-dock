//! Block graph (Python: `proxydock/graph.py`): inbound, outbound, router (and IF) and pass-through blocks plus
//! the PC block. This part is `validate()` - normalizing a graph read from the settings file: every node gets
//! its defaults, broken links and loops are cut, wireless pairs get unique numbers.
//!
//! See the Python module's docstring for what each kind of block means. The JSON shape is the same.

use crate::config;
use crate::i18n::tr;
use crate::ids::new_id;
use crate::net::host_port;
use crate::py;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

/// end a chain (their servers dial out themselves)
pub const NO_VIA: [&str; 5] = ["direct", "hysteria2", "block", "subscription", "group"];
/// cannot be a group member
pub const NO_MEMBER: [&str; 2] = ["group", "block"];
pub const MATCH_LISTS: [&str; 8] =
    ["domain", "domain_suffix", "domain_keyword", "ip_cidr", "port", "process_name", "geoip", "geosite"];
pub const LOWER_KEYS: [&str; 5] = ["domain", "domain_suffix", "domain_keyword", "geoip", "geosite"];
pub const MAX_LANES: usize = 24;
pub const MAX_GAP: i64 = 4;
pub const MAX_WIRELESS: i64 = 9999;

pub type Obj = Map<String, Value>;

pub fn empty() -> Obj {
    match json!({"inbounds": [], "outbounds": [], "routers": [], "relays": [],
                 "pc": {"inbound": null, "pos": [460, -200], "backup": null, "applied": false, "server": ""}}) {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

/// a non-empty string (Python `isinstance(v, str) and v`)
fn text(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

fn id_or_new(v: Option<&Value>) -> String {
    text(v).unwrap_or_else(new_id)
}

fn opt(s: Option<String>) -> Value {
    s.map(Value::String).unwrap_or(Value::Null)
}

/// Python `isinstance(x, (int, float))` (bool counts) -> float
fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// Python `_pos`: a valid [x, y] becomes floats; otherwise the default exactly as given (ints stay ints)
fn pos(v: Option<&Value>, default: Value) -> Value {
    if let Some(Value::Array(a)) = v {
        if a.len() == 2 {
            if let (Some(x), Some(y)) = (num(&a[0]), num(&a[1])) {
                return json!([x, y]);
            }
        }
    }
    default
}

/// Python `x + d` for a JSON number (int stays int)
fn plus(x: &Value, d: i64) -> Value {
    match x.as_i64() {
        Some(i) if !x.is_f64() => json!(i + d),
        _ => json!(x.as_f64().unwrap_or(0.0) + d as f64),
    }
}

fn name(v: Option<&Value>, default: impl FnOnce() -> String) -> String {
    match v {
        Some(Value::String(s)) if !py::strip(s).is_empty() => py::head(py::strip(s), 60),
        _ => default(),
    }
}

/// '#rrggbb' (lower case) or '' (no colour: the default)
pub fn valid_color(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) if s.chars().count() == 7 && s.starts_with('#') && py::int16_ok(&s[1..]) => s.to_lowercase(),
        _ => String::new(),
    }
}

/// Python `type(v) is int` (not bool) within lo..=hi
fn int_in(v: Option<&Value>, lo: i64, hi: i64) -> Option<i64> {
    match v {
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => n.as_i64().filter(|x| (lo..=hi).contains(x)),
        _ => None,
    }
}

pub fn outbound_label(t: &str) -> String {
    match t {
        "direct" => "Direct".into(),
        "http" => tr("HTTP proxy"),
        "https" => tr("HTTPS proxy"),
        "socks4" => tr("SOCKS4 proxy"),
        "socks5" => tr("SOCKS5 proxy"),
        "shadowsocks" => tr("Shadowsocks tunnel"),
        "hysteria2" => tr("Hysteria2 tunnel"),
        "wireguard" => tr("WireGuard tunnel"),
        "vless" => tr("VLESS tunnel"),
        "vmess" => tr("VMess tunnel"),
        "trojan" => tr("Trojan tunnel"),
        "tuic" => tr("TUIC tunnel"),
        "subscription" => tr("Subscription"),
        "group" => tr("Group"),
        "block" => tr("Block"),
        other => other.into(),
    }
}

fn s<'a>(m: &'a Obj, k: &str) -> &'a str {
    m.get(k).and_then(Value::as_str).unwrap_or("")
}

pub fn default_outbound_name(ob: &Obj) -> String {
    match s(ob, "type") {
        "block" => tr("Block"),
        "subscription" => tr("Subscription"),
        "group" => tr("Group"),
        "direct" => {
            let wh = if !s(ob, "interface").is_empty() { s(ob, "interface") } else { s(ob, "bind") };
            if wh.is_empty() { "Direct".into() } else { format!("Direct ({wh})") }
        }
        t => {
            let label = outbound_label(t);
            if s(ob, "host").is_empty() {
                label
            } else {
                format!("{label} {}", host_port(s(ob, "host"), ob["port"].as_i64().unwrap_or(0)))
            }
        }
    }
}

pub fn validate_outbound_node(raw: &Value) -> Obj {
    let empty = Obj::new();
    let r = raw.as_object().unwrap_or(&empty);
    let ob = config::validate_outbound(r.get("outbound").unwrap_or(&Value::Null));
    let id = id_or_new(r.get("id"));
    let t = s(&ob, "type").to_string();
    let mut members: Vec<Value> = vec![];
    if t == "group" {
        if let Some(Value::Array(ms)) = r.get("members") {
            for m in ms {
                if let Value::String(x) = m {
                    if !x.is_empty() && !members.contains(m) {
                        members.push(Value::String(x.clone()));
                    }
                }
            }
        }
    }
    let via = text(r.get("via")).filter(|_| !NO_VIA.contains(&t.as_str()));
    let mut n = Obj::new();
    n.insert("id".into(), json!(id));
    n.insert("name".into(), json!(name(r.get("name"), || default_outbound_name(&ob))));
    n.insert("outbound".into(), Value::Object(ob));
    n.insert("via".into(), opt(via));
    n.insert("members".into(), Value::Array(members));
    n.insert("pos".into(), pos(r.get("pos"), json!([0, 0])));
    n.insert("color".into(), json!(valid_color(r.get("color"))));
    n
}

pub fn validate_inbound_node(raw: &Value) -> Obj {
    let empty = Obj::new();
    let r = raw.as_object().unwrap_or(&empty);
    let p = config::validate_proxy(raw);
    let mut n = Obj::new();
    for k in ["id", "name", "autostart", "auto_restart", "inbound", "overrides"] {
        n.insert(k.into(), p[k].clone());
    }
    n.insert("target".into(), opt(text(r.get("target"))));
    n.insert("pos".into(), pos(r.get("pos"), json!([0, 0])));
    n.insert("color".into(), json!(valid_color(r.get("color"))));
    // (key order as Python: inbound before overrides)
    let order = ["id", "name", "autostart", "auto_restart", "inbound", "overrides", "target", "pos", "color"];
    order.iter().map(|k| (k.to_string(), n[*k].clone())).collect()
}

pub fn validate_match(raw: Option<&Value>) -> Obj {
    let empty = Obj::new();
    let r = raw.and_then(Value::as_object).unwrap_or(&empty);
    let mut m = Obj::new();
    for k in MATCH_LISTS {
        if let Some(Value::Array(v)) = r.get(k) {
            let mut items: Vec<String> = v
                .iter()
                .filter_map(|x| x.as_str().map(|t| py::strip(t).to_string()).filter(|t| !t.is_empty()))
                .collect();
            if LOWER_KEYS.contains(&k) {
                items = items.into_iter().map(|x| x.to_lowercase()).collect();
            }
            if !items.is_empty() {
                m.insert(k.into(), json!(items));
            }
        }
    }
    if r.get("ip_private") == Some(&Value::Bool(true)) {
        m.insert("ip_private".into(), json!(true));
    }
    m
}

pub fn validate_router_node(raw: &Value) -> Obj {
    let empty = Obj::new();
    let r = raw.as_object().unwrap_or(&empty);
    let mut rules: Vec<Obj> = vec![];
    let mut seen = HashSet::new();
    if let Some(Value::Array(items)) = r.get("rules") {
        for (i, x) in items.iter().enumerate() {
            let Value::Object(x) = x else { continue };
            let mut rid = id_or_new(x.get("id"));
            if seen.contains(&rid) {
                rid = new_id();
            }
            seen.insert(rid.clone());
            let mut rule = Obj::new();
            rule.insert("id".into(), json!(rid));
            rule.insert("name".into(), json!(name(x.get("name"), || tr("Rule {n}").replace("{n}", &(i + 1).to_string()))));
            rule.insert("match".into(), Value::Object(validate_match(x.get("match"))));
            rule.insert("target".into(), opt(text(x.get("target"))));
            rule.insert("color".into(), json!(valid_color(x.get("color"))));
            rules.push(rule);
        }
    }
    let style = if r.get("style") == Some(&json!("if")) { "if" } else { "router" };
    let mut color = valid_color(r.get("color"));
    let mut default_color = valid_color(r.get("default_color"));
    if style == "if" {
        rules.truncate(1);
        if rules.is_empty() {
            let mut rule = Obj::new();
            rule.insert("id".into(), json!(new_id()));
            rule.insert("name".into(), json!("IF"));
            rule.insert("match".into(), json!({}));
            rule.insert("target".into(), Value::Null);
            rule.insert("color".into(), json!(""));
            rules.push(rule);
        }
        if !color.is_empty() {
            if rules[0]["color"] == json!("") {
                rules[0].insert("color".into(), json!(color));
            }
            if default_color.is_empty() {
                default_color = color.clone();
            }
            color = String::new();
        }
    }
    let mut n = Obj::new();
    n.insert("id".into(), json!(id_or_new(r.get("id"))));
    n.insert("name".into(), json!(name(r.get("name"), || if style == "if" { "IF".into() } else { "Router".into() })));
    n.insert("style".into(), json!(style));
    n.insert("rules".into(), Value::Array(rules.into_iter().map(Value::Object).collect()));
    n.insert("switch".into(), json!(style == "if" && r.get("switch") == Some(&Value::Bool(true))));
    n.insert("switch_on".into(), json!(r.get("switch_on") == Some(&Value::Bool(true))));
    n.insert("default".into(), opt(text(r.get("default"))));
    n.insert("pos".into(), pos(r.get("pos"), json!([0, 0])));
    n.insert("color".into(), json!(color));
    n.insert("default_color".into(), json!(default_color));
    n
}

fn lanes(items: Option<&Value>, key: &str) -> Vec<Obj> {
    let mut out = vec![];
    let mut seen = HashSet::new();
    let Some(Value::Array(items)) = items else { return out };
    for x in items.iter().take(MAX_LANES) {
        let Value::Object(x) = x else { continue };
        let mut lid = id_or_new(x.get("id"));
        if seen.contains(&lid) {
            lid = new_id();
        }
        seen.insert(lid.clone());
        let mut lane = Obj::new();
        lane.insert("id".into(), json!(lid));
        lane.insert("gap".into(), json!(int_in(x.get("gap"), 0, MAX_GAP).unwrap_or(0)));
        lane.insert(key.into(), opt(text(x.get(key))));
        out.push(lane);
    }
    out
}

pub fn new_lane(key: &str) -> Obj {
    let mut lane = Obj::new();
    lane.insert("id".into(), json!(new_id()));
    lane.insert("gap".into(), json!(0));
    lane.insert(key.into(), Value::Null);
    lane
}

fn valid_wireless_number(v: Option<&Value>) -> Option<i64> {
    int_in(v, 1, MAX_WIRELESS)
}

/// "#123" (1-4 digits) -> 123: the name a wireless pair had before 2.4.1
fn old_wireless_name(n: &str) -> Option<i64> {
    let d = n.strip_prefix('#')?;
    (!d.is_empty() && d.len() <= 4 && d.chars().all(|c| c.is_ascii_digit())).then(|| d.parse().unwrap())
}

pub fn validate_relay_node(raw: &Value) -> Obj {
    let empty = Obj::new();
    let r = raw.as_object().unwrap_or(&empty);
    let wireless = r.get("wireless") == Some(&Value::Bool(true));
    let routed = r.get("routed") == Some(&Value::Bool(true)) && !wireless;
    let mut ins = lanes(r.get("inputs"), "out");
    if ins.is_empty() {
        ins.push(new_lane("out"));
    }
    let mut outs = lanes(r.get("outputs"), "target");
    if outs.is_empty() {
        outs.push(new_lane("target"));
    }
    if wireless {
        ins.truncate(1);
        outs.truncate(1);
        ins[0].insert("gap".into(), json!(0));
        outs[0].insert("gap".into(), json!(0));
    }
    if routed {
        let out_ids: HashSet<Value> = outs.iter().map(|o| o["id"].clone()).collect();
        for i in ins.iter_mut() {
            if !out_ids.contains(&i["out"]) {
                i.insert("out".into(), Value::Null);
            }
        }
    } else {
        while outs.len() < ins.len() {
            outs.push(new_lane("target"));
        }
        outs.truncate(ins.len());
        for (i, o) in ins.iter_mut().zip(outs.iter_mut()) {
            o.insert("gap".into(), i["gap"].clone());
            i.insert("out".into(), o["id"].clone());
        }
    }
    let mut n = Obj::new();
    n.insert("id".into(), json!(id_or_new(r.get("id"))));
    n.insert(
        "name".into(),
        json!(name(r.get("name"), || if wireless { tr("Wireless in") } else { tr("Pass-through") })),
    );
    n.insert("routed".into(), json!(routed));
    n.insert("inputs".into(), Value::Null);
    n.insert("outputs".into(), Value::Null);
    let p = pos(r.get("pos"), json!([0, 0]));
    n.insert("pos".into(), p.clone());
    n.insert("color".into(), json!(""));
    n.insert("wireless".into(), json!(wireless));
    if wireless {
        let mut number = r.get("number").cloned();
        if let Some(old) = old_wireless_name(s(&n, "name")) {
            n.insert("name".into(), json!(tr("Wireless in")));
            if valid_wireless_number(number.as_ref()).is_none() {
                number = Some(json!(old));
            }
        }
        n.insert("number".into(), json!(valid_wireless_number(number.as_ref()).unwrap_or(0)));
        n.insert("name_out".into(), json!(name(r.get("name_out"), || tr("Wireless out"))));
        n.insert("pos_out".into(), pos(r.get("pos_out"), json!([plus(&p[0], 300), plus(&p[1], 120)])));
        let broken = match r.get("broken") {
            Some(Value::String(b)) if b == "in" || b == "out" => Value::String(b.clone()),
            _ => Value::Null,
        };
        if !broken.is_null() {
            ins[0].insert("out".into(), Value::Null);
            outs[0].insert("target".into(), Value::Null);
        }
        n.insert("broken".into(), broken);
    }
    n.insert("inputs".into(), Value::Array(ins.into_iter().map(Value::Object).collect()));
    n.insert("outputs".into(), Value::Array(outs.into_iter().map(Value::Object).collect()));
    n
}

// --- lookups over a validated graph ---

fn list<'a>(g: &'a Obj, key: &str) -> &'a [Value] {
    g.get(key).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}

/// Input lane id -> output lane id it is wired to, output lane id -> its target. A broken wireless pair takes
/// no links (its inputs are left out).
pub struct LaneIndex {
    ins: HashMap<String, Option<String>>,
    outs: HashMap<String, Option<String>>,
}

pub fn lane_index(g: &Obj) -> LaneIndex {
    let mut ins = HashMap::new();
    let mut outs = HashMap::new();
    for r in list(g, "relays") {
        let broken = r.get("broken").is_some_and(crate::secret::truthy);
        for x in r["inputs"].as_array().unwrap() {
            if !broken {
                ins.insert(s(x.as_object().unwrap(), "id").to_string(), x["out"].as_str().map(String::from));
            }
        }
        for x in r["outputs"].as_array().unwrap() {
            outs.insert(s(x.as_object().unwrap(), "id").to_string(), x["target"].as_str().map(String::from));
        }
    }
    LaneIndex { ins, outs }
}

impl LaneIndex {
    pub fn is_input(&self, id: &str) -> bool {
        self.ins.contains_key(id)
    }

    /// (block id at the end of the pass-through lanes from tid or None, true if they loop)
    fn walk(&self, tid: Option<&str>) -> (Option<String>, bool) {
        let mut tid = tid.map(String::from);
        let mut seen = HashSet::new();
        while let Some(t) = tid.clone().filter(|t| self.ins.contains_key(t)) {
            if !seen.insert(t.clone()) {
                return (None, true);
            }
            tid = self.ins[&t].as_ref().and_then(|o| self.outs.get(o)).and_then(|x| x.clone());
        }
        (tid, false)
    }

    /// where a link to tid really goes: through pass-through lanes to the block behind them
    pub fn follow(&self, tid: Option<&str>) -> Option<String> {
        self.walk(tid).0
    }

    pub fn lane_loop(&self, tid: &str) -> bool {
        self.walk(Some(tid)).1
    }
}

/// (rule id or "default", target id) for every link out of router r
pub fn router_targets(r: &Value) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = r["rules"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|x| x["target"].as_str().map(|t| (x["id"].as_str().unwrap().to_string(), t.to_string())))
        .collect();
    if let Some(d) = r["default"].as_str() {
        out.push(("default".into(), d.to_string()));
    }
    out
}

/// true if following router links from router rid comes back to it
pub fn router_loop(g: &Obj, rid: &str) -> bool {
    let routers: HashMap<&str, &Value> = list(g, "routers").iter().map(|r| (r["id"].as_str().unwrap(), r)).collect();
    let idx = lane_index(g);
    let mut stack: Vec<Option<String>> = match routers.get(rid) {
        Some(r) => router_targets(r).iter().map(|(_, t)| idx.follow(Some(t))).collect(),
        None => vec![],
    };
    let mut seen = HashSet::new();
    while let Some(cur) = stack.pop() {
        let Some(cur) = cur else { continue };
        if cur == rid {
            return true;
        }
        if seen.contains(&cur) || !routers.contains_key(cur.as_str()) {
            continue;
        }
        seen.insert(cur.clone());
        stack.extend(router_targets(routers[cur.as_str()]).iter().map(|(_, t)| idx.follow(Some(t))));
    }
    false
}

/// ("inbound" | "outbound" | "router" | "relay", node); pass-through lane ids are not blocks
pub fn find<'a>(g: &'a Obj, nid: &str) -> Option<(&'static str, &'a Value)> {
    for (key, kind) in [("inbounds", "inbound"), ("outbounds", "outbound"), ("routers", "router"), ("relays", "relay")] {
        if let Some(n) = list(g, key).iter().find(|n| n["id"].as_str() == Some(nid)) {
            return Some((kind, n));
        }
    }
    None
}

/// true if following the via links from outbound nid comes back to it
pub fn via_loop(g: &Obj, nid: &str) -> bool {
    let mut seen = HashSet::new();
    let mut cur = Some(nid.to_string());
    while let Some(c) = cur {
        if !seen.insert(c.clone()) {
            return true;
        }
        cur = find(g, &c).and_then(|(_, n)| n.get("via")).and_then(Value::as_str).map(String::from);
    }
    false
}

/// the smallest number no wireless pair uses yet
pub fn next_wireless_number(g: &Obj) -> i64 {
    let used: HashSet<i64> = list(g, "relays")
        .iter()
        .filter(|r| r["wireless"] == json!(true))
        .filter_map(|r| r["number"].as_i64())
        .collect();
    let mut i = 1;
    while used.contains(&i) {
        i += 1;
    }
    i
}

fn arr_mut<'a>(g: &'a mut Obj, key: &str) -> &'a mut Vec<Value> {
    g.get_mut(key).unwrap().as_array_mut().unwrap()
}

fn ids_of(g: &Obj, key: &str) -> HashSet<String> {
    list(g, key).iter().map(|n| n["id"].as_str().unwrap().to_string()).collect()
}

fn keep(target: &Value, allowed: &HashSet<String>, own: Option<&str>) -> bool {
    target.as_str().is_some_and(|t| allowed.contains(t) && Some(t) != own)
}

/// A graph as read from the settings file -> a valid graph (Python `graph.validate`).
pub fn validate(raw: &Value) -> Obj {
    let empty_obj = Obj::new();
    let r = raw.as_object().unwrap_or(&empty_obj);
    let mut g = empty();
    let mut seen: HashSet<String> = HashSet::new();
    let items = |k: &str| r.get(k).and_then(Value::as_array).cloned().unwrap_or_default();
    for item in items("outbounds") {
        let n = validate_outbound_node(&item);
        if seen.insert(s(&n, "id").to_string()) {
            arr_mut(&mut g, "outbounds").push(Value::Object(n));
        }
    }
    for item in items("inbounds") {
        let n = validate_inbound_node(&item);
        if seen.insert(s(&n, "id").to_string()) {
            arr_mut(&mut g, "inbounds").push(Value::Object(n));
        }
    }
    for item in items("routers") {
        let n = validate_router_node(&item);
        if seen.insert(s(&n, "id").to_string()) {
            arr_mut(&mut g, "routers").push(Value::Object(n));
        }
    }
    for item in items("relays") {
        let n = validate_relay_node(&item);
        let lane_ids: Vec<String> = n["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .chain(n["outputs"].as_array().unwrap())
            .map(|x| x["id"].as_str().unwrap().to_string())
            .collect();
        let id = s(&n, "id").to_string();
        if !seen.contains(&id) && !lane_ids.iter().any(|l| seen.contains(l)) {
            seen.insert(id);
            seen.extend(lane_ids);
            arr_mut(&mut g, "relays").push(Value::Object(n));
        }
    }
    // wireless pair numbers: unique, missing ones get a free number
    let mut used = HashSet::new();
    for r in arr_mut(&mut g, "relays") {
        if r["wireless"] == json!(true) {
            let num = r["number"].as_i64().unwrap();
            if used.contains(&num) {
                r["number"] = json!(0);
            }
            used.insert(r["number"].as_i64().unwrap());
        }
    }
    for i in 0..list(&g, "relays").len() {
        let r = &list(&g, "relays")[i];
        if r["wireless"] == json!(true) && r["number"] == json!(0) {
            let n = next_wireless_number(&g);
            arr_mut(&mut g, "relays")[i]["number"] = json!(n);
        }
    }
    let lanes_in: HashSet<String> = lane_index(&g).ins.keys().cloned().collect();
    let mut ids = ids_of(&g, "inbounds");
    ids.extend(ids_of(&g, "outbounds"));
    ids.extend(ids_of(&g, "routers"));
    let ids_lanes: HashSet<String> = ids.union(&lanes_in).cloned().collect();
    for n in arr_mut(&mut g, "inbounds") {
        let own = n["id"].as_str().map(String::from);
        if !keep(&n["target"], &ids_lanes, own.as_deref()) {
            n["target"] = Value::Null;
        }
    }
    let mut rule_ids = ids_of(&g, "outbounds");
    rule_ids.extend(ids_of(&g, "routers"));
    let router_ids = ids_of(&g, "routers");
    let rule_lanes: HashSet<String> = rule_ids.union(&lanes_in).cloned().collect();
    // lanes lead on to outbound blocks, routers or other lanes
    for r in arr_mut(&mut g, "relays") {
        for x in r["outputs"].as_array_mut().unwrap() {
            if !keep(&x["target"], &rule_lanes, None) {
                x["target"] = Value::Null;
            }
        }
    }
    // lanes going round in a circle cannot be used: cut them (one at a time, in order)
    let n_relays = list(&g, "relays").len();
    for ri in 0..n_relays {
        let n_out = list(&g, "relays")[ri]["outputs"].as_array().unwrap().len();
        for oi in 0..n_out {
            let target = list(&g, "relays")[ri]["outputs"][oi]["target"].as_str().map(String::from);
            if let Some(t) = target {
                if lanes_in.contains(&t) && lane_index(&g).lane_loop(&t) {
                    arr_mut(&mut g, "relays")[ri]["outputs"][oi]["target"] = Value::Null;
                }
            }
        }
    }
    // router links go to outbound blocks, other routers or lanes
    for r in arr_mut(&mut g, "routers") {
        let own = r["id"].as_str().unwrap().to_string();
        for x in r["rules"].as_array_mut().unwrap() {
            if !keep(&x["target"], &rule_lanes, Some(&own)) {
                x["target"] = Value::Null;
            }
        }
        if !keep(&r["default"], &rule_lanes, Some(&own)) {
            r["default"] = Value::Null;
        }
    }
    // a router loop cannot be used: cut it (the last link of the router that leads to a router)
    for ri in 0..list(&g, "routers").len() {
        loop {
            let rid = list(&g, "routers")[ri]["id"].as_str().unwrap().to_string();
            if !router_loop(&g, &rid) {
                break;
            }
            let idx = lane_index(&g);
            let key = router_targets(&list(&g, "routers")[ri])
                .into_iter()
                .rev()
                .find(|(_, t)| idx.follow(Some(t)).is_some_and(|f| router_ids.contains(&f)))
                .map(|(k, _)| k)
                .expect("a router loop has a link to a router");
            let r = &mut arr_mut(&mut g, "routers")[ri];
            if key == "default" {
                r["default"] = Value::Null;
            } else {
                for x in r["rules"].as_array_mut().unwrap() {
                    if x["id"] == json!(key) {
                        x["target"] = Value::Null;
                        break;
                    }
                }
            }
        }
    }
    let out_ids = ids_of(&g, "outbounds");
    for n in arr_mut(&mut g, "outbounds") {
        let own = n["id"].as_str().map(String::from);
        if !keep(&n["via"], &out_ids, own.as_deref()) {
            n["via"] = Value::Null;
        }
    }
    // a via loop cannot be used: cut it
    for i in 0..list(&g, "outbounds").len() {
        let id = list(&g, "outbounds")[i]["id"].as_str().unwrap().to_string();
        if via_loop(&g, &id) {
            arr_mut(&mut g, "outbounds")[i]["via"] = Value::Null;
        }
    }
    // nothing goes through a group; members are plain outbound blocks
    let types: HashMap<String, String> = list(&g, "outbounds")
        .iter()
        .map(|n| (n["id"].as_str().unwrap().to_string(), n["outbound"]["type"].as_str().unwrap().to_string()))
        .collect();
    for n in arr_mut(&mut g, "outbounds") {
        if n["via"].as_str().is_some_and(|v| types[v] == "group") {
            n["via"] = Value::Null;
        }
        let own = n["id"].as_str().unwrap().to_string();
        let members: Vec<Value> = n["members"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|m| {
                let m = m.as_str().unwrap();
                types.get(m).is_some_and(|t| !NO_MEMBER.contains(&t.as_str())) && m != own
            })
            .cloned()
            .collect();
        n["members"] = Value::Array(members);
    }
    // the PC block
    let pc_empty = Value::Object(Obj::new());
    let pc = r.get("pc").filter(|v| v.is_object()).unwrap_or(&pc_empty);
    let inbound_ids = ids_of(&g, "inbounds");
    let gpc = g.get_mut("pc").unwrap();
    let default_pos = gpc["pos"].clone();
    gpc["pos"] = pos(pc.get("pos"), default_pos);
    if let Some(Value::String(i)) = pc.get("inbound") {
        if inbound_ids.contains(i) {
            gpc["inbound"] = json!(i);
        }
    }
    if let Some(Value::Object(b)) = pc.get("backup") {
        let kept: Obj = b
            .iter()
            .filter(|(_, v)| match v {
                Value::Null | Value::String(_) | Value::Bool(_) => true,
                Value::Number(n) => n.is_i64() || n.is_u64(),
                _ => false,
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        gpc["backup"] = Value::Object(kept);
    }
    gpc["applied"] = json!(pc.get("applied") == Some(&Value::Bool(true)));
    gpc["server"] = json!(pc.get("server").and_then(Value::as_str).unwrap_or(""));
    g
}

// --- compile to runtime proxies ---

/// The graph with every link into a pass-through lane replaced by where the lanes lead (and no relays).
pub fn without_relays(g: &Obj) -> Obj {
    if list(g, "relays").is_empty() {
        return g.clone();
    }
    let idx = lane_index(g);
    let follow = |v: &Value| opt(idx.follow(v.as_str()));
    let mut out = g.clone();
    out.insert("relays".into(), json!([]));
    for n in arr_mut(&mut out, "inbounds") {
        n["target"] = follow(&n["target"]);
    }
    if let Some(Value::Array(routers)) = out.get_mut("routers") {
        for r in routers {
            r["default"] = follow(&r["default"]);
            for x in r["rules"].as_array_mut().unwrap() {
                x["target"] = follow(&x["target"]);
            }
        }
    }
    out
}

/// Outbound settings with the via chain nested in ob["via"] (null at the end); a group gets its members
/// ({id, name, outbound}) resolved the same way.
pub fn resolve_outbound(g: &Obj, nid: &str) -> Value {
    let mut seen = HashSet::new();
    let mut chain: Vec<&Value> = vec![];
    let mut cur = Some(nid.to_string());
    while let Some(c) = cur.filter(|c| !seen.contains(c)) {
        seen.insert(c.clone());
        match find(g, &c) {
            Some(("outbound", n)) => {
                chain.push(n);
                cur = n["via"].as_str().map(String::from);
            }
            _ => break,
        }
    }
    let mut ob = Value::Null;
    for n in chain.into_iter().rev() {
        let mut d = n["outbound"].clone();
        d["via"] = ob;
        if d["type"] == json!("group") {
            let members: Vec<Value> = n
                .get("members")
                .and_then(Value::as_array)
                .map(|ms| {
                    ms.iter()
                        .filter_map(Value::as_str)
                        .filter_map(|mid| match find(g, mid) {
                            Some(("outbound", m)) => {
                                Some(json!({"id": mid, "name": m["name"], "outbound": resolve_outbound(g, mid)}))
                            }
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            d["members"] = Value::Array(members);
        }
        ob = d;
    }
    ob
}

/// names of the outbound blocks of nid's via chain
pub fn chain_names(g: &Obj, nid: &str) -> Vec<String> {
    let mut names = vec![];
    let mut seen = HashSet::new();
    let mut cur = Some(nid.to_string());
    while let Some(c) = cur.filter(|c| !seen.contains(c)) {
        seen.insert(c.clone());
        match find(g, &c) {
            Some(("outbound", n)) => {
                names.push(n["name"].as_str().unwrap_or("").to_string());
                cur = n["via"].as_str().map(String::from);
            }
            _ => break,
        }
    }
    names
}

/// what an inbound block is connected to, for lists and messages
pub fn target_label(g: &Obj, n: &Value) -> String {
    let found = n["target"].as_str().and_then(|t| find(g, t));
    match found {
        None => tr("not connected"),
        Some(("outbound", t)) => chain_names(g, t["id"].as_str().unwrap()).join(" ⇢ "),
        Some(("router", t)) => {
            let fmt = if t["style"] == json!("if") { tr("IF {name}") } else { tr("Router {name}") };
            fmt.replace("{name}", t["name"].as_str().unwrap_or(""))
        }
        Some((_, t)) => tr("relay → {name}").replace("{name}", t["name"].as_str().unwrap_or("")),
    }
}

/// Outbound that hands traffic to the local inbound ib (relay / connection test bridge).
pub fn chain_outbound(ib: &Value) -> Value {
    let host = match ib["host"].as_str().unwrap_or("") {
        "0.0.0.0" => "127.0.0.1",
        "::" => "::1",
        h => h,
    };
    let t = ib["type"].as_str().unwrap_or("");
    let mut ob = json!({"type": t, "host": host, "port": ib["port"], "tls_verify": false});
    if ["socks5", "http", "https"].contains(&t) && ib["auth_enabled"] == json!(true) {
        ob["username"] = ib["username"].clone();
        ob["password"] = ib["password"].clone();
    }
    if t == "shadowsocks" {
        ob["ss_method"] = ib["ss_method"].clone();
        ob["ss_password"] = ib["ss_password"].clone();
    }
    if t == "hysteria2" {
        ob["hy2_password"] = ib["hy2_password"].clone();
    }
    ob
}

/// One flattened router rule: every condition set on the way (all must match), the outbound block, the name.
pub struct Route {
    pub conds: Vec<Value>,
    pub outbound: String,
    pub name: String,
}

/// Router rules as a flat list plus the default outbound block. Rules without a target or without any condition
/// are skipped; a rule that leads to another router takes that router's rules (and its default) under its own
/// condition. (Python `graph.flatten_router`.)
pub fn flatten_router(g: &Obj, rid: &str, prefix: &[Value], seen: &HashSet<String>) -> (Vec<Route>, Option<String>) {
    let Some((_, r)) = find(g, rid) else { return (vec![], None) };
    let mut seen = seen.clone();
    seen.insert(rid.to_string());
    let mut r = r.clone();
    if r.get("switch").is_some_and(crate::secret::truthy) {
        // manual switch: no condition, everything takes one side
        let side = if r.get("switch_on").is_some_and(crate::secret::truthy) {
            r["rules"][0]["target"].clone()
        } else {
            r["default"].clone()
        };
        r["rules"] = json!([]);
        r["default"] = side;
    }
    let mut routes = vec![];
    for rule in r["rules"].as_array().unwrap() {
        let has_match = rule["match"].as_object().is_some_and(|m| !m.is_empty());
        let Some(target) = rule["target"].as_str().filter(|_| has_match) else { continue };
        let mut conds = prefix.to_vec();
        conds.push(rule["match"].clone());
        let rname = rule["name"].as_str().unwrap_or("").to_string();
        match find(g, target) {
            Some(("outbound", t)) => {
                routes.push(Route { conds, outbound: t["id"].as_str().unwrap().into(), name: rname })
            }
            Some(("router", t)) if !seen.contains(t["id"].as_str().unwrap()) => {
                let (sub, sub_default) = flatten_router(g, t["id"].as_str().unwrap(), &conds, &seen);
                routes.extend(sub.into_iter().map(|x| Route {
                    conds: x.conds,
                    outbound: x.outbound,
                    name: format!("{rname} › {}", x.name),
                }));
                if let Some(d) = sub_default {
                    routes.push(Route { conds, outbound: d, name: rname });
                }
            }
            _ => {}
        }
    }
    match r["default"].as_str().and_then(|d| find(g, d)) {
        Some(("outbound", t)) => (routes, Some(t["id"].as_str().unwrap().into())),
        Some(("router", t)) if !seen.contains(t["id"].as_str().unwrap()) => {
            let (sub, sub_default) = flatten_router(g, t["id"].as_str().unwrap(), prefix, &seen);
            routes.extend(sub);
            (routes, sub_default)
        }
        _ => (routes, None),
    }
}

/// Runtime proxies, one per inbound block (Python `graph.compile`): the inbound, the resolved outbound (via
/// chain, group members), router routes, overrides. "unlinked" = nothing to send to: cannot start.
pub fn compile(graph: &Obj) -> Vec<Obj> {
    let g = without_relays(graph);
    let mut out = vec![];
    for n in list(&g, "inbounds") {
        let found = n["target"].as_str().and_then(|t| find(&g, t));
        let mut routes = vec![];
        let mut exit_id = Value::Null;
        let mut linked = found.is_some();
        let ob = match found {
            Some(("router", t)) => {
                let (flat, default) = flatten_router(&g, t["id"].as_str().unwrap(), &[], &HashSet::new());
                exit_id = opt(default.clone());
                routes = flat
                    .into_iter()
                    .map(|x| {
                        json!({"match": x.conds, "outbound": resolve_outbound(&g, &x.outbound), "name": x.name,
                               "block": x.outbound})
                    })
                    .collect();
                match default {
                    Some(d) => resolve_outbound(&g, &d),
                    None => {
                        linked = false; // no default exit: cannot start
                        json!({"type": "direct"})
                    }
                }
            }
            Some(("outbound", t)) => {
                exit_id = t["id"].clone();
                resolve_outbound(&g, t["id"].as_str().unwrap())
            }
            Some(("inbound", t)) if !["tun", "wireguard"].contains(&t["inbound"]["type"].as_str().unwrap_or("")) => {
                chain_outbound(&t["inbound"])
            }
            _ => {
                linked = false; // nothing to send to (a TUN card cannot be relayed into)
                json!({"type": "direct"})
            }
        };
        let mut p = config::validate_proxy(&json!({
            "id": n["id"], "name": n["name"], "autostart": n["autostart"],
            "auto_restart": n.get("auto_restart").cloned().unwrap_or(json!(false)),
            "inbound": n["inbound"], "outbound": ob, "overrides": n["overrides"],
        }));
        let po = p.get_mut("outbound").unwrap().as_object_mut().unwrap();
        po.insert("via".into(), ob.get("via").cloned().unwrap_or(Value::Null));
        if let Some(m) = ob.get("members") {
            po.insert("members".into(), m.clone());
        }
        p.insert("routes".into(), Value::Array(routes));
        p.insert("target".into(), n["target"].clone());
        p.insert("exit".into(), exit_id);
        p.insert("target_name".into(), json!(target_label(&g, n)));
        p.insert("unlinked".into(), json!(!linked));
        out.push(p);
    }
    out
}

/// Settings v2 (one inbound + outbound per proxy) -> graph; identical outbounds become one shared block (Python
/// `from_proxies` with `add_outbound` / `add_inbound`)
pub fn from_proxies(proxies: &[Value]) -> Obj {
    let mut g = empty();
    let mut shared: Vec<(Value, String)> = vec![];
    for (i, p) in proxies.iter().enumerate() {
        let ob = &p["outbound"];
        let target = match shared.iter().find(|(o, _)| o == ob) {
            Some((_, id)) => id.clone(),
            None => {
                let n = validate_outbound_node(&json!({"id": new_id(), "name": null, "outbound": ob, "pos": [460, shared.len() * 190]}));
                let id = n["id"].as_str().unwrap_or("").to_string();
                g.get_mut("outbounds").and_then(Value::as_array_mut).unwrap().push(Value::Object(n));
                shared.push((ob.clone(), id.clone()));
                id
            }
        };
        let mut n = validate_inbound_node(&json!({
            "id": new_id(), "name": p["name"], "autostart": p["autostart"], "inbound": p["inbound"],
            "target": target, "pos": [0, i * 170],
        }));
        n.insert("target".into(), json!(target));
        n.insert("id".into(), p["id"].clone());
        n.insert("overrides".into(), p["overrides"].clone());
        g.get_mut("inbounds").and_then(Value::as_array_mut).unwrap().push(Value::Object(n));
    }
    g
}
