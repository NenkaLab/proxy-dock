//! Changing the block graph (Python: the editing half of `ui/window.py` and `graph.add_* / remove / cut_links`).
//! Each change keeps the graph valid: a link that would loop or that the blocks cannot carry is refused with the
//! message the user sees, and the graph is left as it was.
//!
//! Canvas keys: a block id; "<id>/out" for the output half of a wireless pair; "pc" for the PC block. Lanes are
//! numbered as the canvas draws them (router outputs: rules then default; pass-through: inputs / outputs).

use crate::graph::{self, Obj};
use crate::i18n::tr;
use serde_json::{json, Value};

/// blocks that end a chain (their server connection dials out itself)
const NO_VIA: [&str; 5] = ["direct", "hysteria2", "block", "subscription", "group"];
const NO_MEMBER: [&str; 2] = ["group", "block"];
/// UDP-based tunnels: only through blocks that carry UDP
const NEEDS_UDP_VIA: [&str; 2] = ["wireguard", "tuic"];
const UDP_VIA_OK: [&str; 9] = ["shadowsocks", "hysteria2", "wireguard", "socks5", "vless", "vmess", "trojan", "tuic", "subscription"];

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
            let ty = graph::find(g, t).and_then(|(_, n)| n["inbound"]["type"].as_str().map(String::from)).unwrap_or_default();
            if ty == "tun" {
                return cannot(&tr("A TUN card takes traffic from apps; another inbound cannot relay into it"));
            }
            if ty == "wireguard" {
                return cannot(&tr("A WireGuard server takes traffic from devices; another inbound cannot relay into it"));
            }
        }
    }
    node_mut(g, "inbounds", iid).unwrap()["target"] = tid.map_or(Value::Null, |t| json!(t));
    let proxies: Vec<Value> = graph::compile(g).into_iter().map(Value::Object).collect();
    if let Some(p) = proxies.iter().find(|p| p["id"] == json!(iid)) {
        if let Some(cycle) = crate::checks::find_cycle(p, &proxies) {
            node_mut(g, "inbounds", iid).unwrap()["target"] = old;
            return cannot(&format!("{}{}", tr("The proxy chain loops: "), cycle.join(" → ")));
        }
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
            return cannot(&tr("{type} needs UDP, so it cannot go through an HTTP or SOCKS4 proxy").replace("{type}", &graph::outbound_label(&own)));
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
        || g["routers"].as_array().into_iter().flatten().filter_map(|x| x["id"].as_str()).any(|id| graph::router_loop(g, id));
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

/// Delete a block (Python `graph.remove`): links into it are cut; links into a deleted pass-through's lanes are
/// spliced to where those lanes led. A wireless half deletes the whole pair.
pub fn delete(g: &mut Obj, key: &str) {
    let (nid, _) = block_of(key);
    let nid = nid.to_string();
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
    if let Some(pc) = g.get_mut("pc") {
        if pc["inbound"] == json!(nid) {
            pc["inbound"] = Value::Null;
        }
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
            let n = graph::validate_outbound_node(&json!({"id": crate::ids::new_id(), "outbound": {"type": "direct"}, "pos": pos}));
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
}
