//! The canvas as the window draws it (Python: the node building in `ui/canvas.py`): the block graph turned into
//! blocks with input / output lanes and links between lanes, plus, for the live traffic view, which links each
//! card's traffic to each exit block runs along.
//!
//! Block kinds: 0 inbound (a card), 1 router / IF, 2 outbound, 3 pass-through (a wireless pair is two halves),
//! 4 PC. Every block has an input lane 0 unless said otherwise; outputs: inbound 1 (its target), outbound 1 when
//! it can go via another block (a group: its members), router one per rule + default, pass-through per lane.

use proxydock_core::i18n::tr;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};

type Obj = Map<String, Value>;

/// blocks that end a chain (their server connection dials out itself): no "via" output
const NO_VIA: [&str; 5] = ["direct", "hysteria2", "block", "subscription", "group"];

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    /// block id ("<id>/out" for the output half of a wireless pair, "pc" for the PC block)
    pub key: String,
    pub kind: i32,
    pub title: String,
    pub detail: String,
    pub x: f64,
    pub y: f64,
    pub ins: usize,
    pub outs: usize,
    pub in_labels: Vec<String>,
    pub out_labels: Vec<String>,
    /// pass-through: input lane -> output lane
    pub wires: Vec<(usize, usize)>,
    /// "#rrggbb" or "" (automatic)
    pub color: String,
    /// the card this block is (inbound blocks)
    pub card: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub from: String,
    pub out: usize,
    pub to: String,
    pub inp: usize,
    pub color: String,
}

#[derive(Default, Debug)]
pub struct View {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    /// (card id, exit block id) -> indexes into links that this card's traffic to that exit runs along
    pub paths: HashMap<(String, String), Vec<usize>>,
}

fn list<'a>(g: &'a Obj, k: &str) -> impl Iterator<Item = &'a Obj> {
    g.get(k).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object)
}

fn s<'a>(o: &'a Obj, k: &str) -> &'a str {
    o.get(k).and_then(Value::as_str).unwrap_or("")
}

fn pos(v: Option<&Value>) -> (f64, f64) {
    let a = v.and_then(Value::as_array);
    let n = |i: usize| a.and_then(|a| a.get(i)).and_then(Value::as_f64).unwrap_or(0.0);
    (n(0), n(1))
}

fn node(key: &str, kind: i32, title: &str, detail: String, p: (f64, f64)) -> Node {
    Node {
        key: key.into(),
        kind,
        title: title.into(),
        detail,
        x: p.0,
        y: p.1,
        ins: 1,
        outs: 1,
        in_labels: vec![],
        out_labels: vec![],
        wires: vec![],
        color: String::new(),
        card: String::new(),
    }
}

/// what a link to `target` lands on: (block key, input lane)
fn landing(target: &str, lanes: &HashMap<String, (String, usize)>, blocks: &HashSet<String>) -> Option<(String, usize)> {
    if let Some(l) = lanes.get(target) {
        return Some(l.clone());
    }
    blocks.contains(target).then(|| (target.to_string(), 0))
}

/// Build the canvas of a validated graph. `summary(card id)` gives an inbound block's detail line.
pub fn build(g: &Obj, summary: &dyn Fn(&str) -> String) -> View {
    let mut v = View::default();
    // relay input lane id -> (key, lane) it lands on; relay output lane id -> (key, lane) it leaves from
    let mut in_lanes: HashMap<String, (String, usize)> = HashMap::new();
    let mut out_lanes: Vec<(String, usize, Option<String>)> = vec![];

    for b in list(g, "inbounds") {
        let id = s(b, "id");
        let mut n = node(id, 0, s(b, "name"), summary(id), pos(b.get("pos")));
        n.color = s(b, "color").into();
        n.card = id.into();
        v.nodes.push(n);
    }
    for b in list(g, "outbounds") {
        let id = s(b, "id");
        let ob = b.get("outbound").and_then(Value::as_object).cloned().unwrap_or_default();
        let t = s(&ob, "type");
        let detail = match t {
            "direct" | "block" | "group" | "subscription" => proxydock_core::graph::outbound_label(t),
            _ => format!("{} {}", proxydock_core::graph::outbound_label(t), proxydock_core::net::host_port(s(&ob, "host"), ob.get("port").and_then(Value::as_i64).unwrap_or(0))),
        };
        let mut n = node(id, 2, s(b, "name"), detail, pos(b.get("pos")));
        n.outs = if NO_VIA.contains(&t) && t != "group" { 0 } else { 1 };
        if t == "group" {
            n.out_labels = vec![tr("Members")];
        }
        n.color = s(b, "color").into();
        v.nodes.push(n);
    }
    for b in list(g, "routers") {
        let id = s(b, "id");
        let is_if = s(b, "style") == "if";
        let rules: Vec<&Obj> = list(b, "rules").collect();
        let mut n = node(id, 1, s(b, "name"), String::new(), pos(b.get("pos")));
        n.outs = rules.len() + 1;
        n.out_labels = if is_if {
            vec![tr("then"), tr("else")]
        } else {
            rules.iter().map(|r| s(r, "name").to_string()).chain([tr("Default")]).collect()
        };
        n.detail = if is_if { "IF".into() } else { "Router".into() };
        n.color = s(b, "color").into();
        v.nodes.push(n);
    }
    for b in list(g, "relays") {
        let id = s(b, "id");
        let ins: Vec<&Obj> = list(b, "inputs").collect();
        let outs: Vec<&Obj> = list(b, "outputs").collect();
        if b.get("wireless") == Some(&json!(true)) {
            // two halves; the pair is known by its number
            let number = b.get("number").and_then(Value::as_i64).unwrap_or(0);
            let broken = s(b, "broken");
            let mut a = node(id, 3, s(b, "name"), format!("#{number}"), pos(b.get("pos")));
            a.outs = 0;
            let out_key = format!("{id}/out");
            let mut z = node(&out_key, 3, s(b, "name_out"), format!("#{number}"), pos(b.get("pos_out")));
            z.ins = 0;
            if let Some(l) = ins.first() {
                in_lanes.insert(s(l, "id").into(), (id.into(), 0));
            }
            if let Some(l) = outs.first() {
                out_lanes.push((out_key.clone(), 0, l.get("target").and_then(Value::as_str).map(String::from)));
            }
            if broken != "in" {
                v.nodes.push(a);
            }
            if broken != "out" {
                v.nodes.push(z);
            }
            continue;
        }
        let mut n = node(id, 3, s(b, "name"), String::new(), pos(b.get("pos")));
        n.ins = ins.len();
        n.outs = outs.len();
        n.in_labels = (1..=ins.len()).map(|k| k.to_string()).collect();
        n.out_labels = (1..=outs.len()).map(|k| k.to_string()).collect();
        let out_index: HashMap<&str, usize> = outs.iter().enumerate().map(|(k, o)| (s(o, "id"), k)).collect();
        for (i, l) in ins.iter().enumerate() {
            in_lanes.insert(s(l, "id").into(), (id.into(), i));
            if let Some(o) = l.get("out").and_then(Value::as_str).and_then(|o| out_index.get(o)) {
                n.wires.push((i, *o));
            }
        }
        for (k, o) in outs.iter().enumerate() {
            out_lanes.push((id.into(), k, o.get("target").and_then(Value::as_str).map(String::from)));
        }
        v.nodes.push(n);
    }
    if let Some(pc) = g.get("pc").and_then(Value::as_object) {
        let mut n = node("pc", 4, &tr("PC (system proxy)"), String::new(), pos(pc.get("pos")));
        n.ins = 0;
        v.nodes.push(n);
    }

    let blocks: HashSet<String> = v.nodes.iter().map(|n| n.key.clone()).collect();
    let color_of: HashMap<String, String> = v.nodes.iter().map(|n| (n.key.clone(), n.color.clone())).collect();
    let link = |from: &str, out: usize, target: Option<&str>, color: &str, v: &mut View| {
        let Some((to, inp)) = target.and_then(|t| landing(t, &in_lanes, &blocks)) else { return };
        if !blocks.contains(from) {
            return;
        }
        let c = if color.is_empty() { color_of.get(from).cloned().unwrap_or_default() } else { color.to_string() };
        v.links.push(Link { from: from.into(), out, to, inp, color: c });
    };
    for b in list(g, "inbounds") {
        link(s(b, "id"), 0, b.get("target").and_then(Value::as_str), "", &mut v);
    }
    for b in list(g, "outbounds") {
        link(s(b, "id"), 0, b.get("via").and_then(Value::as_str), "", &mut v);
        for m in b.get("members").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            link(s(b, "id"), 0, Some(m), "", &mut v);
        }
    }
    for b in list(g, "routers") {
        let rules: Vec<&Obj> = list(b, "rules").collect();
        for (i, r) in rules.iter().enumerate() {
            link(s(b, "id"), i, r.get("target").and_then(Value::as_str), s(r, "color"), &mut v);
        }
        link(s(b, "id"), rules.len(), b.get("default").and_then(Value::as_str), s(b, "default_color"), &mut v);
    }
    for (key, k, target) in &out_lanes {
        link(key, *k, target.as_deref(), "", &mut v);
    }
    if let Some(pc) = g.get("pc").and_then(Value::as_object) {
        link("pc", 0, pc.get("inbound").and_then(Value::as_str), "", &mut v);
    }
    v.paths = paths(&v);
    v
}

/// For every card and every exit block its traffic can reach: the links on the way (the first way found, through
/// routers and pass-through lanes as wired), then the exit's own "via" chain.
fn paths(v: &View) -> HashMap<(String, String), Vec<usize>> {
    let kind: HashMap<&str, i32> = v.nodes.iter().map(|n| (n.key.as_str(), n.kind)).collect();
    let wires: HashMap<&str, &Vec<(usize, usize)>> = v.nodes.iter().map(|n| (n.key.as_str(), &n.wires)).collect();
    // links leaving (block, output lane)
    let mut leaving: HashMap<(&str, usize), Vec<usize>> = HashMap::new();
    for (i, l) in v.links.iter().enumerate() {
        leaving.entry((l.from.as_str(), l.out)).or_default().push(i);
    }
    let wireless_out = |key: &str| format!("{key}/out");
    // the via chain after an exit block
    let via_chain = |exit: &str| -> Vec<usize> {
        let mut out = vec![];
        let mut cur = exit.to_string();
        let mut seen = HashSet::new();
        while seen.insert(cur.clone()) {
            let Some(next) = leaving.get(&(cur.as_str(), 0)).and_then(|ls| ls.first()) else { break };
            // a group's member links are not a chain (which member carries the traffic is not known here)
            if v.nodes.iter().any(|n| n.key == cur && n.out_labels == [tr("Members")]) {
                break;
            }
            out.push(*next);
            cur = v.links[*next].to.clone();
        }
        out
    };
    let mut result = HashMap::new();
    for card in v.nodes.iter().filter(|n| n.kind == 0) {
        // depth-first over (block, lane it came in on), remembering the links taken
        let mut stack: Vec<(String, usize, Vec<usize>)> = vec![(card.key.clone(), 0, vec![])];
        let mut seen = HashSet::new();
        while let Some((key, lane, taken)) = stack.pop() {
            if !seen.insert((key.clone(), lane)) {
                continue;
            }
            let outs: Vec<usize> = match kind.get(key.as_str()) {
                Some(0) if key != card.key => {
                    // into another card's local inbound: that card takes it from here
                    result.entry((card.key.clone(), key.clone())).or_insert(taken);
                    continue;
                }
                Some(2) => {
                    let mut p = taken.clone();
                    p.extend(via_chain(&key));
                    result.entry((card.key.clone(), key.clone())).or_insert(p);
                    continue;
                }
                Some(3) => {
                    if v.nodes.iter().any(|n| n.key == wireless_out(&key)) {
                        vec![0] // a wireless pair: on from the other half
                    } else {
                        wires.get(key.as_str()).map(|w| w.iter().filter(|(i, _)| *i == lane).map(|(_, o)| *o).collect()).unwrap_or_default()
                    }
                }
                Some(1) => (0..v.nodes.iter().find(|n| n.key == key).map_or(0, |n| n.outs)).collect(),
                Some(0) => vec![0],
                _ => vec![],
            };
            let from = if v.nodes.iter().any(|n| n.key == wireless_out(&key)) { wireless_out(&key) } else { key.clone() };
            for o in outs.into_iter().rev() {
                for li in leaving.get(&(from.as_str(), o)).into_iter().flatten() {
                    let l = &v.links[*li];
                    let mut t = taken.clone();
                    t.push(*li);
                    stack.push((l.to.clone(), l.inp, t));
                }
            }
        }
    }
    result
}

/// the view as the window gets it
pub fn to_json(v: &View, states: &dyn Fn(&str) -> &'static str) -> Value {
    let nodes: Vec<Value> = v
        .nodes
        .iter()
        .map(|n| {
            json!({
                "key": n.key, "kind": n.kind, "title": n.title, "detail": n.detail, "x": n.x, "y": n.y,
                "ins": n.ins, "outs": n.outs, "in_labels": n.in_labels, "out_labels": n.out_labels,
                "wires": n.wires.iter().map(|(i, o)| json!([i, o])).collect::<Vec<_>>(), "color": n.color,
                "card": n.card, "state": if n.card.is_empty() { "" } else { states(&n.card) },
            })
        })
        .collect();
    let links: Vec<Value> = v
        .links
        .iter()
        .map(|l| json!({"from": l.from, "out": l.out, "to": l.to, "in": l.inp, "color": l.color}))
        .collect();
    json!({"nodes": nodes, "links": links})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_to_canvas_and_paths() {
        let g = proxydock_core::graph::validate(&json!({
            "inbounds": [{"id": "in1", "name": "card", "inbound": {"type": "socks5", "port": 1080}, "target": "r"}],
            "outbounds": [
                {"id": "d", "name": "Direct", "outbound": {"type": "direct"}},
                {"id": "s", "name": "SS", "outbound": {"type": "shadowsocks", "host": "h", "port": 8388, "ss_password": "x"}, "via": "hp"},
                {"id": "hp", "name": "HTTP", "outbound": {"type": "http", "host": "p", "port": 3128}},
            ],
            "routers": [{"id": "r", "name": "R", "rules": [{"id": "x1", "name": "kr", "match": {"geoip": ["kr"]}, "target": "d"}], "default": "L1"}],
            "relays": [{"id": "rl", "name": "lanes", "inputs": [{"id": "L1", "out": "O1"}], "outputs": [{"id": "O1", "target": "s"}]}],
        }));
        let v = build(&g, &|_| "SOCKS5".into());
        let kinds: Vec<(String, i32, usize, usize)> = v.nodes.iter().map(|n| (n.key.clone(), n.kind, n.ins, n.outs)).collect();
        assert!(kinds.contains(&("r".into(), 1, 1, 2)));
        assert!(kinds.contains(&("d".into(), 2, 1, 0)));
        assert!(kinds.contains(&("s".into(), 2, 1, 1)));
        assert!(kinds.contains(&("rl".into(), 3, 1, 1)));
        let has = |from: &str, out: usize, to: &str, inp: usize| v.links.iter().position(|l| l.from == from && l.out == out && l.to == to && l.inp == inp);
        let (a, b, c, d, e) = (has("in1", 0, "r", 0).unwrap(), has("r", 0, "d", 0).unwrap(), has("r", 1, "rl", 0).unwrap(), has("rl", 0, "s", 0).unwrap(), has("s", 0, "hp", 0).unwrap());
        assert_eq!(v.paths[&("in1".to_string(), "d".to_string())], vec![a, b]);
        assert_eq!(v.paths[&("in1".to_string(), "s".to_string())], vec![a, c, d, e]);
    }
}
