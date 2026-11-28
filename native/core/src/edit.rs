//! Changing the block graph (Python: the editing half of `ui/window.py` and `graph.add_* / remove / cut_links`).
//! Each change keeps the graph valid: a link that would loop or that the blocks cannot carry is refused with the
//! message the user sees, and the graph is left as it was.
//!
//! Canvas keys: a block id; "<id>/out" for the output half of a wireless pair; "pc" for the PC block. Lanes are
//! numbered as the canvas draws them (router outputs: rules then default; pass-through: inputs / outputs).

use crate::graph::{self, Obj};
use crate::i18n::tr;
use serde_json::{Value, json};

/// blocks that end a chain (their server connection dials out itself)
const NO_VIA: [&str; 5] = ["direct", "hysteria2", "block", "subscription", "group"];
const NO_MEMBER: [&str; 2] = ["group", "block"];
/// UDP-based tunnels: only through blocks that carry UDP
const NEEDS_UDP_VIA: [&str; 2] = ["wireguard", "tuic"];
const UDP_VIA_OK: [&str; 9] =
    ["shadowsocks", "hysteria2", "wireguard", "socks5", "vless", "vmess", "trojan", "tuic", "subscription"];

fn cannot(msg: &str) -> Result<(), String> {
    Err(format!("{}: {msg}", tr("Cannot connect")))
}

fn arr<'a>(g: &'a mut Obj, key: &str) -> &'a mut Vec<Value> {
    if !g.get(key).is_some_and(Value::is_array) {
        g.insert(key.into(), json!([]));
    }
    g.get_mut(key).and_then(Value::as_array_mut).unwrap()
}

fn node_mut<'a>(g: &'a mut Obj, key: &str, id: &str) -> Option<&'a mut Value> {
    arr(g, key).iter_mut().find(|n| n["id"] == json!(id))
}

fn kind_of(g: &Obj, id: &str) -> Option<&'static str> {
    graph::find(g, id).map(|(k, _)| k)
}

fn ob_type(g: &Obj, id: &str) -> String {
    graph::find(g, id).and_then(|(_, n)| n["outbound"]["type"].as_str().map(String::from)).unwrap_or_default()
}

/// the block id behind a canvas key, and whether it is a wireless output half
fn block_of(key: &str) -> (&str, bool) {
    match key.strip_suffix("/out") {
        Some(id) => (id, true),
        None => (key, false),
    }
}

/// what a link to input lane `lane` of canvas block `key` points at (a block id or a pass-through lane id)
fn target_id(g: &Obj, key: &str, lane: usize) -> Option<String> {
    let (id, out_half) = block_of(key);
    if out_half {
        return None; // the output half of a wireless pair takes no links
    }
    match graph::find(g, id)? {
        ("relay", r) => r["inputs"].get(lane)?["id"].as_str().map(String::from),
        ("inbound" | "outbound" | "router", _) => Some(id.to_string()),
        _ => None,
    }
}

// --- the Python setters, one per kind of output ---

/// inbound iid sends traffic to tid (Python `set_target`)
pub fn set_target(g: &mut Obj, iid: &str, tid: Option<&str>) -> Result<(), String> {
    let Some(old) = node_mut(g, "inbounds", iid).map(|n| n["target"].clone()) else { return Ok(()) };
    if old.as_str() == tid {
        return Ok(());
    }
    if let Some(t) = tid {
        let tk = kind_of(g, t);
        let is_lane = graph::lane_index(g).is_input(t);
        if t == iid || tk == Some("relay") || (tk.is_none() && !is_lane) {
            return cannot("");
        }
        if tk == Some("inbound") {
            let ty = graph::find(g, t)
                .and_then(|(_, n)| n["inbound"]["type"].as_str().map(String::from))
                .unwrap_or_default();
            if ty == "tun" {
                return cannot(&tr("A TUN card takes traffic from apps; another inbound cannot relay into it"));
            }
            if ty == "wireguard" {
                return cannot(&tr(
                    "A WireGuard server takes traffic from devices; another inbound cannot relay into it",
                ));
            }
        }
    }
    node_mut(g, "inbounds", iid).unwrap()["target"] = tid.map_or(Value::Null, |t| json!(t));
    let proxies: Vec<Value> = graph::compile(g).into_iter().map(Value::Object).collect();
    if let Some(p) = proxies.iter().find(|p| p["id"] == json!(iid))
        && let Some(cycle) = crate::checks::find_cycle(p, &proxies)
    {
        node_mut(g, "inbounds", iid).unwrap()["target"] = old;
        return cannot(&format!("{}{}", tr("The proxy chain loops: "), cycle.join(" → ")));
    }
    Ok(())
}

/// outbound oid connects to its server through vid (Python `set_via`)
pub fn set_via(g: &mut Obj, oid: &str, vid: Option<&str>) -> Result<(), String> {
    let Some(old) = node_mut(g, "outbounds", oid).map(|n| n["via"].clone()) else { return Ok(()) };
    if old.as_str() == vid {
        return Ok(());
    }
    let own = ob_type(g, oid);
    if let Some(v) = vid {
        if kind_of(g, v) != Some("outbound") {
            return cannot("");
        }
        if ob_type(g, v) == "group" {
            return cannot(&tr("Nothing can go through a group; make the block a member of the group instead"));
        }
        if NO_VIA.contains(&own.as_str()) {
            return cannot(&if own == "hysteria2" {
                tr("Hysteria2 uses QUIC (UDP), so it cannot go through another outbound")
            } else {
                tr("Direct is the end of a chain")
            });
        }
        if NEEDS_UDP_VIA.contains(&own.as_str()) && !UDP_VIA_OK.contains(&ob_type(g, v).as_str()) {
            return cannot(
                &tr("{type} needs UDP, so it cannot go through an HTTP or SOCKS4 proxy")
                    .replace("{type}", &graph::outbound_label(&own)),
            );
        }
    }
    node_mut(g, "outbounds", oid).unwrap()["via"] = vid.map_or(Value::Null, |v| json!(v));
    if graph::via_loop(g, oid) {
        node_mut(g, "outbounds", oid).unwrap()["via"] = old;
        let mut names = graph::chain_names(g, vid.unwrap_or(""));
        names.push(graph::find(g, oid).and_then(|(_, n)| n["name"].as_str().map(String::from)).unwrap_or_default());
        return cannot(&format!("{}{}", tr("The proxy chain loops: "), names.join(" → ")));
    }
    Ok(())
}

/// group gid gets mid as its last member (Python `add_member`)
pub fn add_member(g: &mut Obj, gid: &str, mid: &str) -> Result<(), String> {
    if kind_of(g, mid) != Some("outbound") || mid == gid || NO_MEMBER.contains(&ob_type(g, mid).as_str()) {
        return cannot(&tr("A group's members can only be outbound blocks (not another group or Block)"));
    }
    let n = node_mut(g, "outbounds", gid).unwrap();
    let members = n["members"].as_array_mut().unwrap();
    if !members.contains(&json!(mid)) {
        members.push(json!(mid));
    }
    Ok(())
}

fn may_receive_from_router_or_lane(g: &Obj, tid: &str) -> bool {
    matches!(kind_of(g, tid), Some("outbound" | "router")) || graph::lane_index(g).is_input(tid)
}

/// router rid's output `lane` (rules, then default) sends traffic to tid (Python `set_rule_target`)
pub fn set_rule_target(g: &mut Obj, rid: &str, lane: usize, tid: Option<&str>) -> Result<(), String> {
    if tid.is_some_and(|t| !may_receive_from_router_or_lane(g, t)) {
        return cannot(&tr("A router rule can only go to an outbound block, another router or a pass-through"));
    }
    let Some(r) = node_mut(g, "routers", rid) else { return Ok(()) };
    let rules = r["rules"].as_array().map_or(0, Vec::len);
    let slot = if lane < rules { &mut r["rules"][lane]["target"] } else { &mut r["default"] };
    let old = slot.clone();
    *slot = tid.map_or(Value::Null, |t| json!(t));
    if graph::router_loop(g, rid) {
        let r = node_mut(g, "routers", rid).unwrap();
        let slot = if lane < rules { &mut r["rules"][lane]["target"] } else { &mut r["default"] };
        *slot = old;
        return cannot(&tr("The routers point at each other in a loop"));
    }
    Ok(())
}

/// pass-through rid's output lane sends traffic on to tid (Python `set_lane_target`)
pub fn set_lane_target(g: &mut Obj, rid: &str, lane: usize, tid: Option<&str>) -> Result<(), String> {
    let Some(r) = node_mut(g, "relays", rid) else { return Ok(()) };
    if r.get("broken").is_some_and(|b| !b.is_null()) {
        return cannot(&tr("This wireless block has no partner. Recreate the partner first."));
    }
    if tid.is_some_and(|t| !may_receive_from_router_or_lane(g, t)) {
        return cannot(&tr("A pass-through lane can only go to an outbound block, a router or another pass-through"));
    }
    let r = node_mut(g, "relays", rid).unwrap();
    let Some(slot) = r["outputs"].get_mut(lane).map(|o| &mut o["target"]) else { return Ok(()) };
    let old = slot.clone();
    *slot = tid.map_or(Value::Null, |t| json!(t));
    let looped = tid.is_some_and(|t| graph::lane_index(g).lane_loop(t))
        || g["routers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x["id"].as_str())
            .any(|id| graph::router_loop(g, id));
    if looped {
        node_mut(g, "relays", rid).unwrap()["outputs"][lane]["target"] = old;
        return cannot(&tr("The links would go round in a circle"));
    }
    Ok(())
}

// --- canvas operations ---

/// A link drawn on the canvas from output `out` of block `from` to input `inp` of block `to`.
pub fn connect(g: &mut Obj, from: &str, out: usize, to: &str, inp: usize) -> Result<(), String> {
    let Some(tid) = target_id(g, to, inp) else { return cannot("") };
    if from == "pc" {
        return if kind_of(g, &tid) == Some("inbound") {
            g.get_mut("pc").unwrap()["inbound"] = json!(tid);
            Ok(())
        } else {
            cannot("")
        };
    }
    let (id, _) = block_of(from);
    match kind_of(g, id) {
        Some("inbound") => set_target(g, id, Some(&tid)),
        Some("outbound") if ob_type(g, id) == "group" => add_member(g, id, &tid),
        Some("outbound") => set_via(g, id, Some(&tid)),
        Some("router") => set_rule_target(g, id, out, Some(&tid)),
        Some("relay") => set_lane_target(g, id, out, Some(&tid)),
        _ => cannot(""),
    }
}

/// The link from output `out` of `from` to input `inp` of `to` is removed.
pub fn disconnect(g: &mut Obj, from: &str, out: usize, to: &str, inp: usize) -> Result<(), String> {
    if from == "pc" {
        g.get_mut("pc").unwrap()["inbound"] = Value::Null;
        return Ok(());
    }
    let (id, _) = block_of(from);
    match kind_of(g, id) {
        Some("inbound") => set_target(g, id, None),
        Some("outbound") if ob_type(g, id) == "group" => {
            if let Some(tid) = target_id(g, to, inp) {
                node_mut(g, "outbounds", id).unwrap()["members"].as_array_mut().unwrap().retain(|m| m != &json!(tid));
            }
            Ok(())
        }
        Some("outbound") => set_via(g, id, None),
        Some("router") => set_rule_target(g, id, out, None),
        Some("relay") => set_lane_target(g, id, out, None),
        _ => Ok(()),
    }
}

/// A block (or a wireless half, or the PC block) was moved to x, y.
pub fn move_block(g: &mut Obj, key: &str, x: f64, y: f64) {
    if key == "pc" {
        if let Some(pc) = g.get_mut("pc") {
            pc["pos"] = json!([x, y]);
        }
        return;
    }
    let (id, out_half) = block_of(key);
    for list in ["inbounds", "outbounds", "routers", "relays"] {
        if let Some(n) = node_mut(g, list, id) {
            n[if out_half { "pos_out" } else { "pos" }] = json!([x, y]);
            return;
        }
    }
}

// --- canvas groups (a box around blocks; display only: the wiring does not change) ---

fn groups_mut(g: &mut Obj) -> &mut Vec<Value> {
    if !g.get("groups").is_some_and(Value::is_array) {
        g.insert("groups".into(), json!([]));
    }
    g.get_mut("groups").and_then(Value::as_array_mut).unwrap()
}

fn group_mut<'a>(g: &'a mut Obj, gid: &str) -> Option<&'a mut Value> {
    g.get_mut("groups").and_then(Value::as_array_mut).and_then(|a| a.iter_mut().find(|x| x["id"] == json!(gid)))
}

/// A group around blocks `keys` (taken out of any group they were in); its id. Needs two blocks or more.
pub fn group_blocks(g: &mut Obj, keys: &[&str]) -> Option<String> {
    let valid = graph::canvas_keys(g);
    let keys: Vec<String> = keys.iter().filter(|k| valid.contains(**k)).map(|k| k.to_string()).collect();
    if keys.len() < 2 {
        return None;
    }
    let n = g.get("groups").and_then(Value::as_array).map_or(0, Vec::len) + 1;
    for x in groups_mut(g).iter_mut() {
        if let Some(m) = x["members"].as_array_mut() {
            m.retain(|k| !keys.iter().any(|key| k == key));
        }
    }
    let id = crate::ids::new_id();
    let name = tr("Group {n}").replace("{n}", &n.to_string());
    groups_mut(g).push(json!({"id": id, "name": name, "color": "", "members": keys, "collapsed": false}));
    graph::clean_groups(g);
    Some(id)
}

/// block `key` into group `gid` (out of any other group); false when it is already there or either is unknown
pub fn add_to_group(g: &mut Obj, gid: &str, key: &str) -> bool {
    if !graph::canvas_keys(g).contains(key) || group_of(g, key).as_deref() == Some(gid) {
        return false;
    }
    if !g.get("groups").and_then(Value::as_array).is_some_and(|a| a.iter().any(|x| x["id"] == json!(gid))) {
        return false;
    }
    for x in groups_mut(g).iter_mut() {
        let target = x["id"] == json!(gid);
        if let Some(m) = x["members"].as_array_mut() {
            m.retain(|k| k != key);
            if target {
                m.push(json!(key));
            }
        }
    }
    graph::clean_groups(g);
    true
}

/// the group goes, its blocks stay where they are; its ports go too, what ran through them joined up
pub fn ungroup(g: &mut Obj, gid: &str) -> bool {
    let ports: Vec<String> = group_mut(g, gid)
        .map(|x| {
            ["in", "out"]
                .iter()
                .flat_map(|k| x["ports"][k].as_array().cloned().unwrap_or_default())
                .filter_map(|p| p.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    for p in &ports {
        delete(g, p);
    }
    let before = g.get("groups").and_then(Value::as_array).map_or(0, Vec::len);
    groups_mut(g).retain(|x| x["id"] != json!(gid));
    graph::clean_groups(g);
    g.get("groups").and_then(Value::as_array).map_or(0, Vec::len) != before
}

/// A new port on side `side` ("in" / "out") of group gid at x, y: a pass-through with one lane named
/// "Input n" / "Output n"; its id.
pub fn add_group_port(g: &mut Obj, gid: &str, side: &str, x: f64, y: f64) -> Option<String> {
    let side = if side == "out" { "out" } else { "in" };
    let count = group_mut(g, gid)?["ports"][side].as_array().map_or(0, Vec::len);
    let name = tr(if side == "in" { "Input {n}" } else { "Output {n}" }).replace("{n}", &(count + 1).to_string());
    let n = graph::validate_relay_node(&json!({
        "id": crate::ids::new_id(), "name": name, "pos": [x, y], "inputs": [Value::Object(graph::new_lane("out"))]
    }));
    let id = push(g, "relays", n);
    let row = group_mut(g, gid)?;
    if !row["ports"].is_object() {
        row["ports"] = json!({"in": [], "out": []});
    }
    if !row["ports"][side].is_array() {
        row["ports"][side] = json!([]);
    }
    row["ports"][side].as_array_mut().unwrap().push(json!(id));
    graph::clean_groups(g);
    Some(id)
}

/// A group port's name (names may repeat, unlike blocks'); false when empty or the same.
pub fn set_port_name(g: &mut Obj, rid: &str, name: &str) -> bool {
    let name = crate::py::head(crate::py::strip(name), 40);
    match node_mut(g, "relays", rid) {
        Some(n) if !name.is_empty() && n["name"] != json!(name) => {
            n["name"] = json!(name);
            true
        }
        _ => false,
    }
}

/// the group whose port relay `rid` is, and its side ("in" / "out")
pub fn port_of(g: &Obj, rid: &str) -> Option<(String, &'static str)> {
    g.get("groups").and_then(Value::as_array).into_iter().flatten().find_map(|x| {
        let has = |k: &str| x["ports"][k].as_array().is_some_and(|a| a.iter().any(|p| p == rid));
        let side = if has("in") { "in" } else if has("out") { "out" } else { return None };
        Some((x["id"].as_str()?.to_string(), side))
    })
}

pub fn rename_group(g: &mut Obj, gid: &str, name: &str) -> bool {
    let name = crate::py::head(crate::py::strip(name), 60);
    match group_mut(g, gid) {
        Some(x) if !name.is_empty() && x["name"] != json!(name) => {
            x["name"] = json!(name);
            true
        }
        _ => false,
    }
}

pub fn set_group_color(g: &mut Obj, gid: &str, color: &str) -> bool {
    let color = graph::valid_color(Some(&json!(color)));
    match group_mut(g, gid) {
        Some(x) if x["color"] != json!(color) => {
            x["color"] = json!(color);
            true
        }
        _ => false,
    }
}

pub fn set_group_collapsed(g: &mut Obj, gid: &str, collapsed: bool) -> bool {
    match group_mut(g, gid) {
        Some(x) if x["collapsed"] != json!(collapsed) => {
            x["collapsed"] = json!(collapsed);
            true
        }
        _ => false,
    }
}

/// the fill colour of blocks `keys` ("" = by kind); false when nothing changed
pub fn set_fill(g: &mut Obj, keys: &[&str], color: &str) -> bool {
    let color = graph::valid_color(Some(&json!(color)));
    let valid = graph::canvas_keys(g);
    if !g.get("fills").is_some_and(Value::is_object) {
        g.insert("fills".into(), json!({}));
    }
    let fills = g.get_mut("fills").and_then(Value::as_object_mut).unwrap();
    let mut changed = false;
    for k in keys.iter().filter(|k| valid.contains(**k)) {
        let before = fills.get(*k).cloned().unwrap_or(json!(""));
        if before != json!(color) {
            changed = true;
            if color.is_empty() {
                fills.remove(*k);
            } else {
                fills.insert(k.to_string(), json!(color));
            }
        }
    }
    graph::clean_fills(g);
    changed
}

/// Name lane `index` of side `side` ("inputs" / "outputs") of pass-through rid ("" = its number again).
pub fn rename_lane(g: &mut Obj, rid: &str, side: &str, index: usize, name: &str) -> bool {
    let Some(lane) = graph::find(g, rid)
        .filter(|(k, n)| *k == "relay" && n["wireless"] != json!(true))
        .and_then(|(_, n)| n[side].get(index)?["id"].as_str().map(String::from))
    else {
        return false;
    };
    let name = crate::py::head(crate::py::strip(name), 40);
    if !g.get("lane_names").is_some_and(Value::is_object) {
        g.insert("lane_names".into(), json!({}));
    }
    let names = g.get_mut("lane_names").and_then(Value::as_object_mut).unwrap();
    let before = names.get(&lane).cloned().unwrap_or(json!(""));
    if before == json!(name) {
        return false;
    }
    if name.is_empty() {
        names.remove(&lane);
    } else {
        names.insert(lane, json!(name));
    }
    graph::clean_relay_ui(g);
    true
}

/// Fold pass-through rid to its small block (false: open it as a box with its lanes and inside wiring).
pub fn set_relay_folded(g: &mut Obj, rid: &str, folded: bool) -> bool {
    if !graph::find(g, rid).is_some_and(|(k, n)| k == "relay" && n["wireless"] != json!(true)) {
        return false;
    }
    if !g.get("relays_folded").is_some_and(Value::is_array) {
        g.insert("relays_folded".into(), json!([]));
    }
    let list = g.get_mut("relays_folded").and_then(Value::as_array_mut).unwrap();
    let has = list.iter().any(|x| x == rid);
    if has == folded {
        graph::clean_relay_ui(g);
        return false;
    }
    if folded {
        list.push(json!(rid));
    } else {
        list.retain(|x| x != rid);
    }
    graph::clean_relay_ui(g);
    true
}

/// the group a block is in
pub fn group_of(g: &Obj, key: &str) -> Option<String> {
    g.get("groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|x| x["members"].as_array().is_some_and(|m| m.iter().any(|k| k == key)))
        .and_then(|x| x["id"].as_str().map(String::from))
}

/// Delete a block (Python `graph.remove`): links into it are cut; links into a deleted pass-through's lanes are
/// spliced to where those lanes led. A wireless half deletes the whole pair.
pub fn delete(g: &mut Obj, key: &str) {
    let (nid, out_half) = block_of(key);
    let nid = nid.to_string();
    // one half of an intact wireless pair: the pair breaks, the other half stays (Python `delete_wireless_half`)
    if break_wireless(g, &nid, if out_half { "out" } else { "in" }) {
        return;
    }
    let Some((kind, gone)) = graph::find(g, &nid).map(|(k, n)| (k, n.clone())) else { return };
    let mut replace: std::collections::HashMap<String, Value> = Default::default();
    if kind == "relay" {
        let outs: std::collections::HashMap<String, Value> = gone["outputs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|o| (o["id"].as_str().unwrap_or("").to_string(), o["target"].clone()))
            .collect();
        let own: Vec<Value> = gone["inputs"].as_array().into_iter().flatten().map(|x| x["id"].clone()).collect();
        for x in gone["inputs"].as_array().into_iter().flatten() {
            let t = x["out"].as_str().and_then(|o| outs.get(o)).cloned().unwrap_or(Value::Null);
            replace.insert(x["id"].as_str().unwrap_or("").to_string(), if own.contains(&t) { Value::Null } else { t });
        }
    }
    for list in ["inbounds", "outbounds", "routers", "relays"] {
        arr(g, list).retain(|n| n["id"] != json!(nid));
    }
    let fix = |t: &Value| -> Value {
        match t.as_str() {
            Some(s) if s == nid => Value::Null,
            Some(s) => replace.get(s).cloned().unwrap_or_else(|| t.clone()),
            None => t.clone(),
        }
    };
    for r in arr(g, "routers").iter_mut() {
        for x in r["rules"].as_array_mut().into_iter().flatten() {
            x["target"] = fix(&x["target"]);
        }
        r["default"] = fix(&r["default"]);
    }
    let relays = arr(g, "relays").len();
    for i in 0..relays {
        let outs = arr(g, "relays")[i]["outputs"].as_array().map_or(0, Vec::len);
        for k in 0..outs {
            let t = fix(&arr(g, "relays")[i]["outputs"][k]["target"]);
            arr(g, "relays")[i]["outputs"][k]["target"] = t.clone();
            if t.as_str().is_some_and(|t| graph::lane_index(g).lane_loop(t)) {
                arr(g, "relays")[i]["outputs"][k]["target"] = Value::Null;
            }
        }
    }
    for n in arr(g, "inbounds").iter_mut() {
        n["target"] = fix(&n["target"]);
        if n["target"] == n["id"] {
            n["target"] = Value::Null;
        }
    }
    // splicing can close a router loop: cut it at the last output that leads to a router
    let router_ids: Vec<String> = arr(g, "routers").iter().filter_map(|r| r["id"].as_str().map(String::from)).collect();
    for rid in router_ids {
        while graph::router_loop(g, &rid) {
            let r = graph::find(g, &rid).unwrap().1.clone();
            let lanes = graph::lane_index(g);
            let key = graph::router_targets(&r)
                .into_iter()
                .rev()
                .find(|(_, t)| lanes.follow(Some(t)).is_some_and(|b| kind_of(g, &b) == Some("router")))
                .map(|(k, _)| k);
            let r = node_mut(g, "routers", &rid).unwrap();
            match key.as_deref() {
                Some("default") | None => r["default"] = Value::Null,
                Some(k) => {
                    for x in r["rules"].as_array_mut().into_iter().flatten() {
                        if x["id"] == json!(k) {
                            x["target"] = Value::Null;
                        }
                    }
                }
            }
        }
    }
    for n in arr(g, "outbounds").iter_mut() {
        if n["via"] == json!(nid) {
            n["via"] = Value::Null;
        }
        if let Some(m) = n["members"].as_array_mut() {
            m.retain(|x| x != &json!(nid));
        }
    }
    if let Some(pc) = g.get_mut("pc")
        && pc["inbound"] == json!(nid)
    {
        pc["inbound"] = Value::Null;
    }
}

fn names(g: &Obj) -> std::collections::HashSet<String> {
    ["inbounds", "outbounds", "routers", "relays"]
        .iter()
        .flat_map(|k| g.get(*k).and_then(Value::as_array).into_iter().flatten())
        .filter(|n| n["wireless"] != json!(true))
        .filter_map(|n| n["name"].as_str().map(String::from))
        .collect()
}

/// Python `_unique_name`: base, else "base 2", "base 3", ...
pub fn unique_name(g: &Obj, base: &str) -> String {
    let taken = names(g);
    if !taken.contains(base) {
        return base.to_string();
    }
    (2..).map(|i| format!("{base} {i}")).find(|n| !taken.contains(n)).unwrap()
}

/// A new block at x, y: kind 0 an inbound (SOCKS5 on a free local port), 1 a router, 2 an outbound (Direct),
/// 3 a pass-through with three lanes. Returns its id.
pub fn add(g: &mut Obj, kind: i32, x: f64, y: f64) -> String {
    let pos = json!([x, y]);
    match kind {
        0 => {
            let used: std::collections::HashSet<i64> =
                arr(g, "inbounds").iter().filter_map(|n| n["inbound"]["port"].as_i64()).collect();
            let port = (1080..).find(|p| !used.contains(p)).unwrap();
            let name = unique_name(g, "SOCKS5");
            let n = graph::validate_inbound_node(&json!({
                "id": crate::ids::new_id(), "name": name, "inbound": {"type": "socks5", "port": port}, "pos": pos,
            }));
            let id = n["id"].as_str().unwrap().to_string();
            arr(g, "inbounds").push(Value::Object(n));
            id
        }
        1 => {
            let name = unique_name(g, "Router");
            let n = graph::validate_router_node(&json!({
                "id": crate::ids::new_id(), "name": name, "pos": pos, "style": "router",
                "rules": [{"name": tr("Rule {n}").replace("{n}", "1"), "match": {}}],
            }));
            let id = n["id"].as_str().unwrap().to_string();
            arr(g, "routers").push(Value::Object(n));
            id
        }
        3 => {
            let lanes: Vec<Value> = (0..3).map(|_| Value::Object(graph::new_lane("out"))).collect();
            let n = graph::validate_relay_node(&json!({"id": crate::ids::new_id(), "pos": pos, "inputs": lanes}));
            let id = n["id"].as_str().unwrap().to_string();
            arr(g, "relays").push(Value::Object(n));
            id
        }
        _ => {
            let n = graph::validate_outbound_node(
                &json!({"id": crate::ids::new_id(), "outbound": {"type": "direct"}, "pos": pos}),
            );
            let id = n["id"].as_str().unwrap().to_string();
            let name = unique_name(g, n["name"].as_str().unwrap_or("Direct"));
            let mut n = Value::Object(n);
            n["name"] = json!(name);
            arr(g, "outbounds").push(n);
            id
        }
    }
}

/// Rename a block from its title on the canvas (Python `rename_node`, `set_wireless_title`). Returns the name the
/// settings log shows, or None when nothing changes (an empty or the same name, the PC block). A wireless half
/// keeps its title as typed (titles may repeat); other blocks get a free name ("base 2", ...).
/// Python `set_wireless_number`: the pair number of wireless pair `rid` (1..9999, not another pair's). Ok(true):
/// changed; Ok(false): the same; Err: why not.
pub fn set_wireless_number(g: &mut Obj, rid: &str, text: &str) -> Result<bool, String> {
    let rid = block_of(rid).0;
    let number = crate::py::strip(text)
        .trim_start_matches('#')
        .parse::<i64>()
        .ok()
        .filter(|n| (1..=graph::MAX_WIRELESS).contains(n));
    let Some(number) = number else {
        return Err(tr("Enter a number from 1 to {max}").replace("{max}", &graph::MAX_WIRELESS.to_string()));
    };
    let taken = arr(g, "relays")
        .iter()
        .any(|r| r["id"] != json!(rid) && r["wireless"] == json!(true) && r["number"] == json!(number));
    if taken {
        return Err(tr("#{n} is already used by another wireless pair").replace("{n}", &number.to_string()));
    }
    let Some(r) = arr(g, "relays").iter_mut().find(|r| r["id"] == json!(rid) && r["wireless"] == json!(true)) else {
        return Ok(false);
    };
    if r["number"] == json!(number) {
        return Ok(false);
    }
    r["number"] = json!(number);
    Ok(true)
}

pub fn rename(g: &mut Obj, key: &str, name: &str) -> Option<String> {
    let (id, out_half) = block_of(key);
    let name: String = crate::py::strip(name).chars().take(60).collect();
    let (kind, n) = graph::find(g, id).map(|(k, n)| (k, n.clone()))?;
    if name.is_empty() || !matches!(kind, "inbound" | "outbound" | "router" | "relay") {
        return None;
    }
    if kind == "relay" && n["wireless"] == json!(true) {
        let field = if out_half { "name_out" } else { "name" };
        if n[field] == json!(name) {
            return None;
        }
        let label = tr("Wireless {name}").replace("{name}", &format!("#{}", n["number"].as_i64().unwrap_or(0)));
        node_mut(g, "relays", id)?[field] = json!(name);
        return Some(format!("{label} · {name}"));
    }
    if n["name"] == json!(name) {
        return None;
    }
    let list = format!("{kind}s");
    // (its own old name does not count as taken)
    node_mut(g, &list, id)?["name"] = json!("");
    let free = unique_name(g, &name);
    node_mut(g, &list, id)?["name"] = json!(free);
    Some(free)
}

/// A copy of each block (Python `duplicate_node`), 40 px down and right, with nothing linked to or from it (a
/// pass-through keeps its inside wiring; a wireless pair gets a number of its own). A card copy gets the next free
/// port and does not start by itself. Returns the new ids.
pub fn duplicate(g: &mut Obj, keys: &[&str]) -> Vec<String> {
    let mut ids: Vec<&str> = vec![];
    for k in keys {
        let (id, _) = block_of(k);
        if *k != "pc" && !ids.contains(&id) {
            ids.push(id);
        }
    }
    let mut made = vec![];
    for id in ids {
        let Some((kind, src)) = graph::find(g, id).map(|(k, n)| (k, n.clone())) else { continue };
        if !matches!(kind, "inbound" | "outbound" | "router" | "relay") {
            continue;
        }
        let mut n = src.clone();
        let new_id = crate::ids::new_id();
        n["id"] = json!(new_id);
        if kind == "relay" {
            // fresh lane ids, the inside wiring kept (Python `copy_relay`)
            let mut new_out: std::collections::HashMap<String, String> = Default::default();
            for x in n["outputs"].as_array_mut().into_iter().flatten() {
                let fresh = crate::ids::new_id();
                new_out.insert(x["id"].as_str().unwrap_or("").to_string(), fresh.clone());
                x["id"] = json!(fresh);
            }
            for x in n["inputs"].as_array_mut().into_iter().flatten() {
                x["id"] = json!(crate::ids::new_id());
                x["out"] = x["out"].as_str().and_then(|o| new_out.get(o)).map_or(Value::Null, |o| json!(o));
            }
        }
        let base = format!("{}{}", src["name"].as_str().unwrap_or(""), tr(" copy"));
        n["name"] = json!(unique_name(g, &base));
        let shifted = |p: &Value| json!([p[0].as_f64().unwrap_or(0.0) + 40.0, p[1].as_f64().unwrap_or(0.0) + 40.0]);
        n["pos"] = shifted(&src["pos"]);
        match kind {
            "inbound" => {
                let used: std::collections::HashSet<i64> =
                    arr(g, "inbounds").iter().filter_map(|x| x["inbound"]["port"].as_i64()).collect();
                let start = src["inbound"]["port"].as_i64().unwrap_or(1080) + 1;
                n["inbound"]["port"] = json!((start..).find(|p| !used.contains(p)).unwrap());
                n["autostart"] = json!(false);
                n["target"] = Value::Null;
            }
            "outbound" => {
                n["via"] = Value::Null;
                n["members"] = json!([]);
            }
            "router" => {
                n["default"] = Value::Null;
                for x in n["rules"].as_array_mut().into_iter().flatten() {
                    x["id"] = json!(crate::ids::new_id());
                    x["target"] = Value::Null;
                }
            }
            _ => {
                for x in n["outputs"].as_array_mut().into_iter().flatten() {
                    x["target"] = Value::Null;
                }
                if n["wireless"] == json!(true) {
                    n["number"] = json!(graph::next_wireless_number(g));
                    n["pos_out"] = shifted(&src["pos_out"]);
                    n["broken"] = Value::Null;
                    let out0 = n["outputs"][0]["id"].clone();
                    n["inputs"][0]["out"] = out0;
                }
            }
        }
        arr(g, &format!("{kind}s")).push(n);
        made.push(new_id);
    }
    made
}

// --- blocks by type, wireless pairs, cutting links, lanes (Python `ui/canvas.add_inbound`, `window.new_*`,
// `graph.add_wireless / break_wireless / repair_wireless / cut_links / remove_lane`) ---

/// the name of an inbound type as menus and new blocks show it (Python `INBOUND_TYPES`)
pub fn inbound_label(t: &str) -> String {
    match t {
        "socks5" => "SOCKS5".into(),
        "socks4" => "SOCKS4".into(),
        "http" => "HTTP".into(),
        "https" => "HTTPS".into(),
        "shadowsocks" => "Shadowsocks".into(),
        "hysteria2" => "Hysteria2".into(),
        "wireguard" => "WireGuard".into(),
        "forward" => tr("Port forward"),
        "tun" => "TUN".into(),
        other => other.into(),
    }
}

/// the port a new inbound of type `t` starts from (Python `canvas.DEFAULT_PORTS`; a TUN card has no port: any
/// free number keeps the port checks quiet)
fn default_port(t: &str) -> i64 {
    match t {
        "socks4" => 1084,
        "http" => 8080,
        "https" | "hysteria2" => 8443,
        "shadowsocks" => 8388,
        "wireguard" => 51820,
        "forward" => 2222,
        "tun" => 30300,
        _ => 1080,
    }
}

fn push(g: &mut Obj, list: &str, n: crate::graph::Obj) -> String {
    let id = n["id"].as_str().unwrap_or("").to_string();
    arr(g, list).push(Value::Object(n));
    id
}

/// A new inbound block of type `t` at x, y, not connected (Python `canvas.add_inbound`): servers open to the LAN
/// (Shadowsocks, Hysteria2, WireGuard), the others local, on the first free port from the type's usual one.
pub fn add_inbound(g: &mut Obj, t: &str, x: f64, y: f64) -> String {
    let used: std::collections::HashSet<i64> =
        arr(g, "inbounds").iter().filter_map(|n| n["inbound"]["port"].as_i64()).collect();
    let port = (default_port(t)..).find(|p| !used.contains(p)).unwrap();
    let host = if matches!(t, "shadowsocks" | "hysteria2" | "wireguard") { "0.0.0.0" } else { "127.0.0.1" };
    let name = unique_name(g, &inbound_label(t));
    let n = graph::validate_inbound_node(&json!({
        "id": crate::ids::new_id(), "name": name, "inbound": {"type": t, "host": host, "port": port}, "pos": [x, y],
    }));
    push(g, "inbounds", n)
}

/// A new outbound block of type `t` (or with `fields` from a share link, named `name`) at x, y
pub fn add_outbound(
    g: &mut Obj,
    t: &str,
    fields: Option<&serde_json::Map<String, Value>>,
    name: Option<&str>,
    x: f64,
    y: f64,
) -> String {
    let ob = match fields {
        Some(f) => Value::Object(f.clone()),
        None => json!({"type": t}),
    };
    let mut n = graph::validate_outbound_node(&json!({"id": crate::ids::new_id(), "outbound": ob, "pos": [x, y]}));
    let base = name
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| n["name"].as_str().unwrap_or("").to_string());
    n.insert("name".into(), json!(unique_name(g, &crate::py::head(&base, 60))));
    push(g, "outbounds", n)
}

/// A new router (`style` "router": several rules) or IF ("if": one condition, then / else) at x, y
pub fn add_router(g: &mut Obj, style: &str, x: f64, y: f64) -> String {
    let is_if = style == "if";
    let name = unique_name(g, if is_if { "IF" } else { "Router" });
    let rule = if is_if { "IF".to_string() } else { tr("Rule {n}").replace("{n}", "1") };
    let n = graph::validate_router_node(&json!({
        "id": crate::ids::new_id(), "name": name, "pos": [x, y], "style": if is_if { "if" } else { "router" },
        "rules": [{"name": rule, "match": {}}],
    }));
    push(g, "routers", n)
}

/// A new pass-through with three lanes at x, y
pub fn add_relay(g: &mut Obj, x: f64, y: f64) -> String {
    let lanes: Vec<Value> = (0..3).map(|_| Value::Object(graph::new_lane("out"))).collect();
    let name = unique_name(g, &tr("Pass-through"));
    let n =
        graph::validate_relay_node(&json!({"id": crate::ids::new_id(), "name": name, "pos": [x, y], "inputs": lanes}));
    push(g, "relays", n)
}

/// A new wireless pair: the input half at x, y, the output half to its right (Python `graph.add_wireless`)
pub fn add_wireless(g: &mut Obj, x: f64, y: f64) -> String {
    let n = graph::validate_relay_node(&json!({
        "id": crate::ids::new_id(), "pos": [x, y], "wireless": true, "number": graph::next_wireless_number(g),
        "pos_out": [x + 300.0, y], "inputs": [graph::new_lane("out")],
    }));
    push(g, "relays", n)
}

/// One half ("in" / "out") of an intact wireless pair is deleted: every link of the pair is cut and the other
/// half stays, disabled (Python `graph.break_wireless`). False if `rid` is no intact pair.
pub fn break_wireless(g: &mut Obj, rid: &str, half: &str) -> bool {
    let Some((kind, r)) = graph::find(g, rid).map(|(k, n)| (k, n.clone())) else { return false };
    if kind != "relay"
        || r["wireless"] != json!(true)
        || r["broken"].as_str().is_some_and(|b| !b.is_empty())
        || !matches!(half, "in" | "out")
    {
        return false;
    }
    let lane_in = r["inputs"][0]["id"].clone();
    let cut = |t: &mut Value| {
        if *t == lane_in {
            *t = Value::Null;
        }
    };
    for n in arr(g, "inbounds").iter_mut() {
        cut(&mut n["target"]);
    }
    for x in arr(g, "routers").iter_mut() {
        cut(&mut x["default"]);
        for rule in x["rules"].as_array_mut().into_iter().flatten() {
            cut(&mut rule["target"]);
        }
    }
    for x in arr(g, "relays").iter_mut() {
        for o in x["outputs"].as_array_mut().into_iter().flatten() {
            cut(&mut o["target"]);
        }
    }
    if let Some(r) = node_mut(g, "relays", rid) {
        r["inputs"][0]["out"] = Value::Null;
        r["outputs"][0]["target"] = Value::Null;
        r["broken"] = json!(half);
    }
    true
}

/// Make the missing half of a broken wireless pair again, next to the one left, with no links (Python
/// `graph.repair_wireless`)
pub fn repair_wireless(g: &mut Obj, rid: &str) -> bool {
    let Some(r) = node_mut(g, "relays", rid) else { return false };
    let broken = r["broken"].as_str().unwrap_or("").to_string();
    if broken.is_empty() {
        return false;
    }
    let f = |v: &Value, i: usize| v[i].as_f64().unwrap_or(0.0);
    if broken == "in" {
        r["pos"] = json!([f(&r["pos_out"], 0) - 300.0, f(&r["pos_out"], 1)]);
    } else {
        r["pos_out"] = json!([f(&r["pos"], 0) + 300.0, f(&r["pos"], 1)]);
    }
    r["broken"] = Value::Null;
    r["inputs"][0]["out"] = r["outputs"][0]["id"].clone();
    true
}

/// Cut every link into and out of the block behind `key` (the block stays; a pass-through keeps its inside
/// wiring; for a wireless half only that half's links). Python `graph.cut_links`. True if anything changed.
pub fn cut_links(g: &mut Obj, key: &str) -> bool {
    let (bid, out_half) = block_of(key);
    let bid = bid.to_string();
    let Some((kind, b)) = graph::find(g, &bid).map(|(k, n)| (k, n.clone())) else { return false };
    let half = if b["wireless"] == json!(true) { Some(if out_half { "out" } else { "in" }) } else { None };
    let take_in = half != Some("out");
    let take_out = half != Some("in");
    let into: Vec<Value> = if !take_in {
        vec![]
    } else if kind == "relay" {
        b["inputs"].as_array().into_iter().flatten().map(|x| x["id"].clone()).collect()
    } else {
        vec![json!(bid)]
    };
    let before = Value::Object(g.clone());
    let cut = |t: &Value| if into.contains(t) { Value::Null } else { t.clone() };
    for n in arr(g, "inbounds").iter_mut() {
        n["target"] = if take_out && n["id"] == json!(bid) { Value::Null } else { cut(&n["target"]) };
    }
    for r in arr(g, "routers").iter_mut() {
        let mine = take_out && r["id"] == json!(bid);
        r["default"] = if mine { Value::Null } else { cut(&r["default"]) };
        for x in r["rules"].as_array_mut().into_iter().flatten() {
            x["target"] = if mine { Value::Null } else { cut(&x["target"]) };
        }
    }
    for r in arr(g, "relays").iter_mut() {
        let mine = take_out && r["id"] == json!(bid);
        for x in r["outputs"].as_array_mut().into_iter().flatten() {
            x["target"] = if mine { Value::Null } else { cut(&x["target"]) };
        }
    }
    for n in arr(g, "outbounds").iter_mut() {
        if take_out && n["id"] == json!(bid) {
            n["via"] = Value::Null;
            n["members"] = json!([]);
            continue;
        }
        n["via"] = cut(&n["via"]);
        if let Some(m) = n["members"].as_array_mut() {
            m.retain(|x| !into.contains(x));
        }
    }
    if take_out && g.get("pc").and_then(|p| p.get("inbound")) == Some(&json!(bid)) {
        g.get_mut("pc").unwrap()["inbound"] = Value::Null;
    }
    Value::Object(g.clone()) != before
}

/// Add a lane to a pass-through (`side` "inputs", or "outputs" for an output lane of a routed one)
pub fn add_lane(g: &mut Obj, rid: &str, side: &str) -> bool {
    let Some(mut n) = graph::find(g, rid).filter(|(k, _)| *k == "relay").map(|(_, n)| n.clone()) else { return false };
    if n["wireless"] == json!(true) {
        return false;
    }
    let count = n[side].as_array().map_or(0, Vec::len) + 1;
    if count > graph::MAX_LANES {
        return false;
    }
    crate::form::set_lane_count(&mut n, side, count);
    *g = crate::form::with_node(g, "relay", &n);
    true
}

/// Wire input lane `input` of pass-through rid inside the block to output lane `output` (None = to nothing),
/// as dragged on the block (Python `set_lane_map`). A straight block that gets a crossing wire becomes routed,
/// its other lanes kept as they are. Ok(false): nothing changed.
pub fn set_lane_map(g: &mut Obj, rid: &str, input: usize, output: Option<usize>) -> Result<bool, String> {
    let Some(before) = node_mut(g, "relays", rid).map(|n| n.clone()) else { return Ok(false) };
    if before["wireless"] == json!(true) {
        return Ok(false);
    }
    let outs: Vec<Value> = before["outputs"].as_array().into_iter().flatten().map(|o| o["id"].clone()).collect();
    let Some(in_id) = before["inputs"].get(input).and_then(|x| x["id"].as_str()).map(str::to_string) else {
        return Ok(false);
    };
    let new_out = match output {
        Some(o) => match outs.get(o) {
            Some(id) => id.clone(),
            None => return Ok(false),
        },
        None => Value::Null,
    };
    if before["inputs"][input]["out"] == new_out {
        return Ok(false);
    }
    let n = node_mut(g, "relays", rid).unwrap();
    if n["routed"] != json!(true) {
        // straight: lane i goes to lane i; keep that for the others
        n["routed"] = json!(true);
        for (i, x) in n["inputs"].as_array_mut().into_iter().flatten().enumerate() {
            x["out"] = outs.get(i).cloned().unwrap_or(Value::Null);
        }
    }
    n["inputs"][input]["out"] = new_out;
    let looped = graph::lane_index(g).lane_loop(&in_id)
        || g["routers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|x| x["id"].as_str())
            .any(|id| graph::router_loop(g, id));
    if looped {
        *node_mut(g, "relays", rid).unwrap() = before;
        cannot(&tr("The links would go round in a circle"))?;
    }
    Ok(true)
}

/// A pass-through wired straight (lane i to lane i, both sides as many as its inputs) or freely (its wiring kept)
/// - the editor's "Wiring" from the canvas. False when nothing changed.
pub fn set_relay_routed(g: &mut Obj, rid: &str, routed: bool) -> bool {
    let Some(mut n) = graph::find(g, rid).filter(|(k, _)| *k == "relay").map(|(_, n)| n.clone()) else { return false };
    if n["wireless"] == json!(true) || (n["routed"] == json!(true)) == routed {
        return false;
    }
    n["routed"] = json!(routed);
    if !routed {
        let count = n["inputs"].as_array().map_or(1, Vec::len);
        crate::form::set_lane_count(&mut n, "inputs", count);
    }
    *g = crate::form::with_node(g, "relay", &n);
    true
}

/// Remove one lane (an input or output lane id) of a pass-through (Python `graph.remove_lane`): a straight block
/// loses it on both sides, a routed one only on that side; one lane per side stays. Links into lanes that are
/// gone are cut.
pub fn remove_lane(g: &mut Obj, rid: &str, lane: &str) -> bool {
    let Some(mut n) = graph::find(g, rid).filter(|(k, _)| *k == "relay").map(|(_, n)| n.clone()) else { return false };
    if n["wireless"] == json!(true) {
        return false;
    }
    let has = |side: &str| n[side].as_array().is_some_and(|a| a.iter().any(|x| x["id"] == json!(lane)));
    let side = if has("inputs") {
        "inputs"
    } else if has("outputs") {
        "outputs"
    } else {
        return false;
    };
    if n[side].as_array().map_or(0, Vec::len) <= 1 {
        return false;
    }
    let i = n[side].as_array().unwrap().iter().position(|x| x["id"] == json!(lane)).unwrap();
    let sides: Vec<&str> = if n["routed"] == json!(true) { vec![side] } else { vec!["inputs", "outputs"] };
    for s in sides {
        if let Some(a) = n[s].as_array_mut().filter(|a| i < a.len()) {
            a.remove(i);
        }
    }
    let outs: Vec<Value> = n["outputs"].as_array().into_iter().flatten().map(|o| o["id"].clone()).collect();
    for x in n["inputs"].as_array_mut().into_iter().flatten() {
        if !outs.contains(&x["out"]) {
            x["out"] = Value::Null;
        }
    }
    *g = crate::form::with_node(g, "relay", &n);
    true
}

fn item(action: &str, title: String) -> Value {
    json!({"action": action, "title": title})
}

/// The right-click menu of the canvas block behind `key` (Python `CanvasInterface.context_menu` on a block), in the
/// sections the window draws: `head` (the block's own entries), one submenu (`sub_title`, `sub`), then "Disconnect
/// all links" (`cut`) and Duplicate / Delete (`tail`). Each entry's action goes back to the core (`node_action`);
/// "toggle" is titled Run / Stop by the window (from the card's live state).
pub fn block_menu(g: &Obj, key: &str) -> Value {
    let menu = |head: Vec<Value>, sub_title: String, sub: Vec<Value>, cut: bool, tail: bool| json!({"head": head, "sub_title": sub_title, "sub": sub, "cut": cut, "tail": tail});
    if key == "pc" {
        return menu(
            vec![item("pc.restore", tr("Restore previous")), item("pc.off", tr("Off (default)")), item("fill", tr("Block colour"))],
            String::new(),
            vec![],
            false,
            false,
        );
    }
    let (bid, out_half) = block_of(key);
    let Some((kind, b)) = graph::find(g, bid) else { return menu(vec![], String::new(), vec![], false, false) };
    // a group's port: its colour, cut its links, delete it (double-click renames it)
    if port_of(g, bid).is_some() {
        let head = vec![item("fill", tr("Block colour")), item("delete", tr("Delete"))];
        return menu(head, String::new(), vec![], true, false);
    }
    let mut head = vec![item("edit", tr("Edit"))];
    if b["wireless"] == json!(true) {
        if b["broken"].as_str().is_some_and(|s| !s.is_empty()) {
            head.push(item("wireless.repair", tr("Recreate the partner")));
        } else {
            head.push(item(
                if out_half { "wireless.partner.in" } else { "wireless.partner.out" },
                tr("Go to the partner"),
            ));
        }
        head.push(item("fill", tr("Block colour")));
        return menu(head, String::new(), vec![], false, true);
    }
    let (mut sub_title, mut sub) = (String::new(), vec![]);
    match kind {
        "inbound" => {
            head.push(item("toggle", String::new()));
            head.push(item("copy", tr("Copy address/link for clients")));
            if crate::platform::PC_INBOUND_TYPES.contains(&b["inbound"]["type"].as_str().unwrap_or("")) {
                head.push(item("pc", tr("Use as the PC proxy")));
            }
            if !b["target"].is_null() {
                head.push(item("disconnect", tr("Disconnect")));
            }
        }
        "relay" => {
            let routed = b["routed"] == json!(true);
            head.push(item("lane.add.inputs", tr("Add a lane")));
            if routed {
                head.push(item("lane.add.outputs", tr("Add an output lane")));
            }
            // (the editor's "Wiring": the other one)
            head.push(if routed {
                item("lane.mode.straight", format!("{}: {}", tr("Wiring"), tr("Straight")))
            } else {
                item("lane.mode.routed", format!("{}: {}", tr("Wiring"), tr("Routed (wire lanes freely)")))
            });
            for (side, label) in [("inputs", "Input {n}"), ("outputs", "Output {n}")] {
                let lanes = b[side].as_array().cloned().unwrap_or_default();
                if lanes.len() > 1 && (routed || side == "inputs") {
                    for (i, l) in lanes.iter().enumerate() {
                        let title =
                            if routed { tr(label) } else { tr("Lane {n}") }.replace("{n}", &(i + 1).to_string());
                        sub.push(item(&format!("lane.del.{}", l["id"].as_str().unwrap_or("")), title));
                    }
                }
            }
            if !sub.is_empty() {
                sub_title = tr("Remove a lane");
            }
        }
        "outbound" => {
            let t = b["outbound"]["type"].as_str().unwrap_or("");
            if t != "block" {
                head.push(item("latency", tr("Test latency")));
            }
            if !b["via"].is_null() {
                head.push(item("disconnect", tr("Disconnect")));
            }
            if t == "group" && b["outbound"]["group_mode"] == json!("select") {
                let sel = b["outbound"]["group_selected"].as_str().unwrap_or("");
                let members: Vec<&str> =
                    b["members"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
                let cur = if members.contains(&sel) { sel } else { members.first().copied().unwrap_or("") };
                for mid in &members {
                    if let Some((_, n)) = graph::find(g, mid) {
                        let name = n["name"].as_str().unwrap_or("");
                        let mut e = item(&format!("member.{mid}"), name.to_string());
                        if *mid == cur {
                            e["checked"] = json!(true);
                        }
                        sub.push(e);
                    }
                }
                if !sub.is_empty() {
                    sub_title = tr("Use member");
                }
            }
        }
        _ => {}
    }
    head.push(item("fill", tr("Block colour")));
    head.push(item("group.make", tr("Group blocks")));
    menu(head, sub_title, sub, true, true)
}

// --- the list page's "New proxy" templates (Python `ui/proxies.TEMPLATES`, `window.new_proxy / new_inbound`) ---

/// (name, what it is for, inbound, outbound)
pub fn templates() -> Vec<(String, String, Value, Value)> {
    vec![
        (
            tr("Local SOCKS5"),
            tr("SOCKS5 proxy for this PC"),
            json!({"type": "socks5", "port": 1080}),
            json!({"type": "direct"}),
        ),
        (
            tr("Local HTTP"),
            tr("For HTTP proxy settings in browsers and apps"),
            json!({"type": "http", "port": 8080}),
            json!({"type": "direct"}),
        ),
        (
            tr("HTTPS proxy"),
            tr("HTTP proxy wrapped in TLS (self-signed certificate generated)"),
            json!({"type": "https", "port": 8443}),
            json!({"type": "direct"}),
        ),
        (
            "SOCKS4".into(),
            tr("For older programs (no authentication)"),
            json!({"type": "socks4", "port": 1084}),
            json!({"type": "direct"}),
        ),
        (
            tr("Shadowsocks server"),
            tr("Shadowsocks apps on other devices connect to it"),
            json!({"type": "shadowsocks", "host": "0.0.0.0", "port": 8388}),
            json!({"type": "direct"}),
        ),
        (
            tr("Hysteria2 server"),
            tr("Hysteria2 apps on other devices connect to it (QUIC)"),
            json!({"type": "hysteria2", "host": "0.0.0.0", "port": 8443}),
            json!({"type": "direct"}),
        ),
        (
            tr("Local → upstream proxy"),
            tr("Local SOCKS5, forwarded to a company/other HTTP or SOCKS proxy"),
            json!({"type": "socks5", "port": 1080}),
            json!({"type": "http"}),
        ),
        (
            tr("Local → Shadowsocks tunnel"),
            tr("Local SOCKS5, encrypted to a remote Shadowsocks server"),
            json!({"type": "socks5", "port": 1080}),
            json!({"type": "shadowsocks"}),
        ),
        (
            tr("Local → Hysteria2 tunnel"),
            tr("Local SOCKS5, to a remote Hysteria2 server (QUIC)"),
            json!({"type": "socks5", "port": 1080}),
            json!({"type": "hysteria2"}),
        ),
    ]
}

/// Template `i`: an inbound on the first free port from the template's, below the other inbounds; with an
/// outbound other than Direct, a new outbound block linked to it (Python `new_inbound`). Returns (the inbound's
/// id, the new outbound's id).
pub fn add_template(g: &mut Obj, i: usize) -> Option<(String, Option<String>)> {
    let (name, _, ib, ob) = templates().into_iter().nth(i)?;
    let used: std::collections::HashSet<i64> =
        arr(g, "inbounds").iter().filter_map(|n| n["inbound"]["port"].as_i64()).collect();
    let mut ib = ib;
    let port = (ib["port"].as_i64().unwrap_or(1080)..).find(|p| !used.contains(p)).unwrap();
    ib["port"] = json!(port);
    // below the inbounds there are (Python `next_free_pos`)
    let y = arr(g, "inbounds")
        .iter()
        .filter_map(|n| n["pos"][1].as_f64())
        .fold(None, |m: Option<f64>, y| Some(m.map_or(y, |m| m.max(y))))
        .map_or(0.0, |y| y + 170.0);
    let fresh = if ob["type"] != json!("direct") {
        Some(add_outbound(g, ob["type"].as_str().unwrap_or("direct"), None, None, 460.0, y))
    } else {
        None
    };
    let name = unique_name(g, &name);
    let n = graph::validate_inbound_node(&json!({
        "id": crate::ids::new_id(), "name": name, "inbound": ib, "pos": [0.0, y], "target": fresh.clone(),
    }));
    let mut n = Value::Object(n);
    n["target"] = json!(fresh.clone());
    let id = n["id"].as_str().unwrap_or("").to_string();
    arr(g, "inbounds").push(n);
    Some((id, fresh))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Obj {
        graph::validate(&json!({
            "inbounds": [
                {"id": "a", "name": "A", "inbound": {"type": "socks5", "port": 1080}},
                {"id": "b", "name": "B", "inbound": {"type": "http", "port": 8080}},
            ],
            "outbounds": [
                {"id": "d", "name": "Direct", "outbound": {"type": "direct"}},
                {"id": "s", "name": "S", "outbound": {"type": "socks5", "host": "127.0.0.1", "port": 1080}},
                {"id": "h", "name": "H", "outbound": {"type": "http", "host": "p", "port": 3128}},
                {"id": "w", "name": "W", "outbound": {"type": "wireguard", "host": "x", "port": 51820}},
                {"id": "grp", "name": "G", "outbound": {"type": "group"}},
            ],
            "routers": [{"id": "r", "name": "R", "rules": [{"id": "x", "name": "x", "match": {}}]}],
            "relays": [{"id": "rl", "name": "L", "inputs": [{"id": "L1"}, {"id": "L2"}]}],
        }))
    }

    #[test]
    fn relay_wiring_mode() {
        let mut g = sample();
        let rl = |g: &Obj| graph::find(g, "rl").unwrap().1.clone();
        assert!(set_relay_routed(&mut g, "rl", true));
        assert!(!set_relay_routed(&mut g, "rl", true));
        // routed: an output lane added on its own, input 1 wired to output 3
        assert!(add_lane(&mut g, "rl", "outputs"));
        assert_eq!(set_lane_map(&mut g, "rl", 0, Some(2)), Ok(true));
        assert_eq!(rl(&g)["outputs"].as_array().unwrap().len(), 3);
        // straight again: as many outputs as inputs, lane i to lane i
        assert!(set_relay_routed(&mut g, "rl", false));
        let n = rl(&g);
        let (ins, outs) = (n["inputs"].as_array().unwrap(), n["outputs"].as_array().unwrap());
        assert_eq!((ins.len(), outs.len()), (2, 2));
        assert!(ins.iter().zip(outs).all(|(i, o)| i["out"] == o["id"]));
    }

    #[test]
    fn lane_map_on_the_block() {
        let mut g = sample();
        let rl = |g: &Obj| graph::find(g, "rl").unwrap().1.clone();
        let outs: Vec<Value> = rl(&g)["outputs"].as_array().unwrap().iter().map(|o| o["id"].clone()).collect();
        // the same wire: nothing changes; a crossing wire makes the block routed, the other lane kept
        assert_eq!(set_lane_map(&mut g, "rl", 0, Some(0)), Ok(false));
        assert_eq!(set_lane_map(&mut g, "rl", 0, Some(1)), Ok(true));
        let n = rl(&g);
        assert_eq!(n["routed"], json!(true));
        assert_eq!((n["inputs"][0]["out"].clone(), n["inputs"][1]["out"].clone()), (outs[1].clone(), outs[1].clone()));
        // to nothing, and lanes that do not exist
        assert_eq!(set_lane_map(&mut g, "rl", 1, None), Ok(true));
        assert_eq!(rl(&g)["inputs"][1]["out"], Value::Null);
        assert_eq!(set_lane_map(&mut g, "rl", 5, Some(0)), Ok(false));
        assert_eq!(set_lane_map(&mut g, "rl", 0, Some(9)), Ok(false));
    }

    #[test]
    fn wireless_numbers() {
        let mut g = sample();
        let a = add_wireless(&mut g, 0.0, 0.0);
        let b = add_wireless(&mut g, 0.0, 100.0);
        assert_eq!(set_wireless_number(&mut g, &a, "#7"), Ok(true));
        assert_eq!(set_wireless_number(&mut g, &format!("{a}/out"), "7"), Ok(false));
        assert!(set_wireless_number(&mut g, &b, "7").unwrap_err().contains('7'));
        assert!(set_wireless_number(&mut g, &b, "0").is_err());
        assert!(set_wireless_number(&mut g, &b, "x").is_err());
    }

    #[test]
    fn canvas_groups() {
        let mut g = sample();
        assert!(!g.contains_key("groups"), "no groups: no key (Python's graphs stay the same)");
        assert!(group_blocks(&mut g, &["a"]).is_none(), "one block is not a group");
        let id = group_blocks(&mut g, &["a", "d", "nope"]).unwrap();
        assert_eq!(g["groups"][0]["members"], json!(["a", "d"]));
        // a block is in one group: grouping it again takes it out of the first
        let id2 = group_blocks(&mut g, &["d", "s"]).unwrap();
        assert_eq!(group_of(&g, "d"), Some(id2.clone()));
        // the first group lost "d" and is left with one block: still a group (it was made with two)
        assert_eq!(g["groups"][0]["members"], json!(["a"]));
        assert!(rename_group(&mut g, &id, "Local"));
        assert!(set_group_color(&mut g, &id, "#57ab5a"));
        assert!(set_group_collapsed(&mut g, &id, true));
        // kept through validation (load / save); a deleted block leaves its group, an empty group goes
        let v = graph::validate(&Value::Object(g.clone()));
        assert_eq!(v["groups"][0]["name"], "Local");
        assert_eq!(v["groups"][0]["collapsed"], true);
        delete(&mut g, "a");
        graph::clean_groups(&mut g);
        assert_eq!(g["groups"].as_array().unwrap().len(), 1);
        // dropped onto a group: in it (and out of the other)
        assert!(add_to_group(&mut g, &id2, "b"));
        assert_eq!(group_of(&g, "b"), Some(id2.clone()));
        assert!(!add_to_group(&mut g, &id2, "b"), "already in it");
        assert!(ungroup(&mut g, &id2));
        assert!(!g.contains_key("groups"));
    }

    #[test]
    fn links_by_kind() {
        let mut g = sample();
        connect(&mut g, "a", 0, "r", 0).unwrap();
        assert_eq!(graph::find(&g, "a").unwrap().1["target"], "r");
        connect(&mut g, "r", 1, "rl", 1).unwrap(); // default -> lane 2
        assert_eq!(graph::find(&g, "r").unwrap().1["default"], "L2");
        let out_lane = graph::find(&g, "rl").unwrap().1["outputs"][1]["id"].clone();
        assert!(out_lane.is_string());
        connect(&mut g, "rl", 1, "d", 0).unwrap();
        connect(&mut g, "grp", 0, "d", 0).unwrap();
        connect(&mut g, "grp", 0, "h", 0).unwrap();
        assert_eq!(graph::find(&g, "grp").unwrap().1["members"], json!(["d", "h"]));
        disconnect(&mut g, "grp", 0, "d", 0).unwrap();
        assert_eq!(graph::find(&g, "grp").unwrap().1["members"], json!(["h"]));
        move_block(&mut g, "a", 12.5, -3.0);
        assert_eq!(graph::find(&g, "a").unwrap().1["pos"], json!([12.5, -3.0]));
    }

    #[test]
    fn refused_links_leave_the_graph_alone() {
        let mut g = sample();
        // Direct ends a chain; nothing goes through a group; WireGuard needs UDP
        assert!(connect(&mut g, "d", 0, "h", 0).is_err() || graph::find(&g, "d").unwrap().1["via"].is_null());
        assert!(set_via(&mut g, "h", Some("grp")).is_err());
        assert!(set_via(&mut g, "w", Some("h")).is_err());
        // a via loop
        set_via(&mut g, "s", Some("h")).unwrap();
        assert!(set_via(&mut g, "h", Some("s")).is_err());
        assert!(graph::find(&g, "h").unwrap().1["via"].is_null());
        // card A relays into its own port through S (S points at 127.0.0.1:1080 = A): a loop
        set_via(&mut g, "s", None).unwrap();
        let e = set_target(&mut g, "a", Some("s")).unwrap_err();
        assert!(e.contains("A → A"), "{e}");
        assert!(graph::find(&g, "a").unwrap().1["target"].is_null());
        // routers pointing at each other
        let r2 = add(&mut g, 1, 0.0, 0.0);
        set_rule_target(&mut g, "r", 0, Some(&r2)).unwrap();
        assert!(set_rule_target(&mut g, &r2, 0, Some("r")).is_err());
    }

    #[test]
    fn delete_splices_pass_through_lanes() {
        let mut g = sample();
        connect(&mut g, "rl", 0, "d", 0).unwrap();
        connect(&mut g, "r", 0, "rl", 0).unwrap();
        connect(&mut g, "a", 0, "r", 0).unwrap();
        delete(&mut g, "rl");
        assert_eq!(graph::find(&g, "r").unwrap().1["rules"][0]["target"], "d");
        delete(&mut g, "r");
        assert!(graph::find(&g, "a").unwrap().1["target"].is_null());
        let id = add(&mut g, 0, 0.0, 0.0);
        let n = graph::find(&g, &id).unwrap().1;
        assert_eq!(n["inbound"]["port"], 1081); // 1080 is taken by A
        assert_eq!(unique_name(&g, "A"), "A 2");
    }

    #[test]
    fn group_ports_run_through_and_join_up_on_ungroup() {
        let mut g = sample();
        connect(&mut g, "a", 0, "h", 0).unwrap();
        let gid = group_blocks(&mut g, &["h", "w"]).unwrap();
        // a port: a one-lane pass-through, named, not a member; the link from outside runs through it
        let p = add_group_port(&mut g, &gid, "in", 0.0, 0.0).unwrap();
        assert_eq!(graph::find(&g, &p).unwrap().1["name"], json!(tr("Input {n}").replace("{n}", "1")));
        assert_eq!(port_of(&g, &p), Some((gid.clone(), "in")));
        assert!(!g["groups"][0]["members"].as_array().unwrap().contains(&json!(p)));
        connect(&mut g, "a", 0, &p, 0).unwrap();
        connect(&mut g, &p, 0, "h", 0).unwrap();
        let lane = graph::find(&g, &p).unwrap().1["inputs"][0]["id"].clone();
        assert_eq!(graph::find(&g, "a").unwrap().1["target"], lane);
        // the runtime sees the link as it was (no cost)
        assert_eq!(graph::without_relays(&g)["inbounds"][0]["target"], json!("h"));
        // an empty port is fine; names may repeat
        let q = add_group_port(&mut g, &gid, "out", 0.0, 0.0).unwrap();
        assert!(set_port_name(&mut g, &q, "X") && set_port_name(&mut g, &p, "X"));
        // ungrouped: the ports go, what ran through them joined up
        assert!(ungroup(&mut g, &gid));
        assert!(graph::find(&g, &p).is_none() && graph::find(&g, &q).is_none());
        assert_eq!(graph::find(&g, "a").unwrap().1["target"], json!("h"));
    }

    #[test]
    fn fills_and_lane_names() {
        let mut g = sample();
        assert!(set_fill(&mut g, &["a", "rl", "nope"], "#e5534b"));
        assert!(!set_fill(&mut g, &["a"], "#e5534b"));
        assert_eq!(g["fills"], json!({"a": "#e5534b", "rl": "#e5534b"}));
        assert!(set_fill(&mut g, &["a", "rl"], ""));
        assert!(!g.contains_key("fills"), "no fills: no key");
        // lane names and folding, cleaned with the blocks
        assert!(rename_lane(&mut g, "rl", "inputs", 1, "web"));
        assert!(!rename_lane(&mut g, "rl", "inputs", 9, "x"));
        assert!(set_relay_folded(&mut g, "rl", true) && !set_relay_folded(&mut g, "rl", true));
        let again = graph::validate(&Value::Object(g.clone()));
        assert_eq!(again["lane_names"].as_object().unwrap().len(), 1);
        assert_eq!(again["relays_folded"], json!(["rl"]));
        delete(&mut g, "rl");
        graph::clean_relay_ui(&mut g);
        assert!(!g.contains_key("lane_names") && !g.contains_key("relays_folded"));
    }
}
