//! The canvas as the window draws it (Python: the node building in `ui/canvas.py`): the block graph turned into
//! blocks with input / output lanes and links between lanes, plus, for the live traffic view, which links each
//! card's traffic to each exit block runs along.
//!
//! Block kinds: 0 inbound (a card), 1 router / IF, 2 outbound, 3 pass-through (a wireless pair is two halves),
//! 4 PC. Every block has an input lane 0 unless said otherwise; outputs: inbound 1 (its target), outbound 1 when
//! it can go via another block (a group: its members), router one per rule + default, pass-through per lane.

use proxydock_core::graph::LaneIndex;
use proxydock_core::i18n::tr;
use serde_json::{Map, Value, json};
use std::collections::{HashMap, HashSet};

type Obj = Map<String, Value>;

/// (card id, exit block id, by a router rule)
pub type Way = (String, String, bool);

/// the window's default link colour (proxydock-ui.rs `WIRE`)
const DEFAULT_WIRE: &str = "#5aa0f0";

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
    /// pass-through: wired freely (its two sides added to apart)
    pub routed: bool,
    /// pass-through: open as a box (named lanes, wiring edited inside); folded = the small block
    pub open: bool,
    /// "#rrggbb" or "" (automatic)
    pub color: String,
    /// the card this block is (inbound blocks)
    pub card: String,
    /// the lines under the title (Python `update_states`): text and tone (0 normal, 1 warning, 2 muted, 3 via
    /// chain, 4 on); a card's state line is added by the window (it changes without a new canvas)
    pub lines: Vec<(String, i32)>,
    /// the colour its links leave in ("" = the default): its own, or what comes into it (Python `wire_colors`)
    pub swatch: String,
    /// a router / IF: each output's key (rule id / "default"), own colour and the colour its link has
    pub out_colors: Vec<(String, String, String)>,
    /// a router / IF: a manual switch's position (None = not a switch: its rules decide)
    pub switch: Option<bool>,
    /// a router / IF: which rule outputs have a condition (the engine skips the others)
    pub conds: Vec<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub from: String,
    pub out: usize,
    pub to: String,
    pub inp: usize,
    pub color: String,
    /// a mix of several colours (Python draws it dashed)
    pub mixed: bool,
}

#[derive(Default, Debug)]
pub struct View {
    pub nodes: Vec<Node>,
    pub links: Vec<Link>,
    /// (card id, exit block id, sent there by a router rule) -> indexes into links that this card's traffic going
    /// that way runs along (a rule and the default can both lead to one exit, on different links)
    pub paths: HashMap<Way, Vec<usize>>,
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
        routed: false,
        open: false,
        color: String::new(),
        card: String::new(),
        lines: vec![],
        swatch: String::new(),
        out_colors: vec![],
        switch: None,
        conds: vec![],
    }
}

// --- the blocks' info lines (Python `ui/canvas.update_states`, `graph.users / used_by / lane_sources`,
// `ui/proxies.outbound_summary`, `ui/router.match_summary`) ---

const NORMAL: i32 = 0;
const WARN: i32 = 1;
const MUTED: i32 = 2;
const VIA: i32 = 3;
const ON: i32 = 4;

fn name_of(g: &Obj, id: &str) -> Option<String> {
    proxydock_core::graph::find(g, id).and_then(|(_, n)| n["name"].as_str().map(String::from))
}

/// inbound blocks whose traffic goes to `nid` (straight or through pass-through lanes)
fn users(g: &Obj, idx: &LaneIndex, nid: &str) -> Vec<String> {
    list(g, "inbounds")
        .filter(|n| idx.follow(n.get("target").and_then(Value::as_str)).as_deref() == Some(nid))
        .map(|n| s(n, "name").to_string())
        .collect()
}

/// the blocks that send traffic to `nid`: inbounds, groups (members) and routers (rule targets)
fn used_by(g: &Obj, idx: &LaneIndex, nid: &str) -> Vec<String> {
    let mut names = users(g, idx, nid);
    names.extend(
        list(g, "outbounds")
            .filter(|n| n.get("members").and_then(Value::as_array).is_some_and(|m| m.iter().any(|x| x == nid)))
            .map(|n| s(n, "name").to_string()),
    );
    names.extend(
        list(g, "routers")
            .filter(|r| {
                proxydock_core::graph::router_targets(&Value::Object((*r).clone()))
                    .iter()
                    .any(|(_, t)| idx.follow(Some(t)).as_deref() == Some(nid))
            })
            .map(|r| s(r, "name").to_string()),
    );
    names
}

/// the blocks linked straight into input lane `lane`
fn lane_sources(g: &Obj, lane: &str) -> Vec<String> {
    let mut names: Vec<String> =
        list(g, "inbounds").filter(|n| s(n, "target") == lane).map(|n| s(n, "name").to_string()).collect();
    names.extend(
        list(g, "routers")
            .filter(|r| {
                proxydock_core::graph::router_targets(&Value::Object((*r).clone())).iter().any(|(_, t)| t == lane)
            })
            .map(|r| s(r, "name").to_string()),
    );
    names.extend(list(g, "relays").filter(|r| list(r, "outputs").any(|x| s(x, "target") == lane)).map(|r| {
        if r.get("wireless") == Some(&json!(true)) { s(r, "name_out").to_string() } else { s(r, "name").to_string() }
    }));
    names
}

fn used_line(names: &[String]) -> (String, i32) {
    if names.is_empty() {
        (tr("not used"), MUTED)
    } else {
        (tr("used by: {names}").replace("{names}", &names.join(", ")), MUTED)
    }
}

fn group_mode(ob: &Obj) -> String {
    match s(ob, "group_mode") {
        "failover" => tr("Failover in order"),
        "select" => tr("Manual"),
        _ => tr("Fastest"),
    }
}

/// Python `outbound_summary` (not a group)
fn outbound_summary(ob: &Obj) -> String {
    let t = s(ob, "type");
    if t == "block" {
        return tr("Refuses every connection (use it as a router target)");
    }
    let iface = if s(ob, "interface").is_empty() { String::new() } else { format!(" · ⇢ {}", s(ob, "interface")) };
    if t == "direct" {
        let bind = if s(ob, "bind").is_empty() { String::new() } else { format!(" · {}", s(ob, "bind")) };
        return tr("Direct from this PC") + &bind + &iface;
    }
    let server = if s(ob, "host").is_empty() {
        tr("server not set")
    } else {
        proxydock_core::net::host_port(s(ob, "host"), ob.get("port").and_then(Value::as_i64).unwrap_or(0))
    };
    format!("{} · {server}{iface}", proxydock_core::graph::outbound_label(t))
}

/// Python `match_summary`: a rule's conditions in short, e.g. "Program: game.exe + GeoIP country: kr"
pub fn match_summary(m: &Value, limit: usize) -> String {
    const FIELDS: [(&str, &str); 8] = [
        ("domain_suffix", "Domain (with subdomains)"),
        ("domain", "Domain (exact)"),
        ("domain_keyword", "Domain keyword"),
        ("ip_cidr", "IP / range"),
        ("geoip", "GeoIP country"),
        ("geosite", "Geosite category"),
        ("port", "Destination port"),
        ("process_name", "Program"),
    ];
    let mut parts: Vec<String> = FIELDS
        .iter()
        .filter_map(|(k, label)| {
            let vals: Vec<&str> = m[*k].as_array()?.iter().filter_map(Value::as_str).collect();
            (!vals.is_empty()).then(|| format!("{}: {}", tr(label), vals.join(", ")))
        })
        .collect();
    if m["ip_private"] == json!(true) {
        parts.push(tr("Private network addresses"));
    }
    let text = parts.join(" + ");
    if text.chars().count() <= limit { text } else { text.chars().take(limit - 1).collect::<String>() + "…" }
}

fn inbound_lines(b: &Obj, unlinked: bool) -> Vec<(String, i32)> {
    let ib = b.get("inbound").and_then(Value::as_object).cloned().unwrap_or_default();
    let t = s(&ib, "type");
    let port = ib.get("port").and_then(Value::as_i64).unwrap_or(0);
    let where_ = match t {
        "tun" => format!(
            "{} · {}",
            tr("all apps"),
            if s(&ib, "tun_stack").is_empty() { "mixed" } else { s(&ib, "tun_stack") }
        ),
        "wireguard" => format!("UDP {port}"),
        _ => proxydock_core::net::host_port(s(&ib, "host"), port),
    };
    let mut lines = vec![(format!("{} · {where_}", proxydock_core::edit::inbound_label(t)), NORMAL)];
    if unlinked {
        lines.push((tr("not connected"), WARN));
    }
    lines
}

fn outbound_lines(g: &Obj, idx: &LaneIndex, b: &Obj) -> Vec<(String, i32)> {
    let id = s(b, "id");
    let ob = b.get("outbound").and_then(Value::as_object).cloned().unwrap_or_default();
    let members: Vec<String> = b
        .get("members")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|m| name_of(g, m))
        .collect();
    let mut lines = vec![];
    if s(&ob, "type") == "group" {
        let mut mode = group_mode(&ob);
        let ids: Vec<&str> =
            b.get("members").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).collect();
        if s(&ob, "group_mode") == "select" && !ids.is_empty() {
            let sel = if ids.contains(&s(&ob, "group_selected")) { s(&ob, "group_selected") } else { ids[0] };
            mode += &format!(" · {}", name_of(g, sel).unwrap_or_default());
        }
        lines.push((
            format!("{mode} · {}", tr("{count} members").replace("{count}", &members.len().to_string())),
            NORMAL,
        ));
        if members.is_empty() {
            lines.push((tr("no members"), WARN));
        }
    } else {
        lines.push((outbound_summary(&ob), NORMAL));
    }
    if let Some(via) = b.get("via").and_then(Value::as_str) {
        let chain = proxydock_core::graph::chain_names(g, via);
        lines.push((format!("⇢ {}", chain.join(" ⇢ ")), VIA));
    }
    lines.push(used_line(&used_by(g, idx, id)));
    lines
}

fn router_lines(g: &Obj, idx: &LaneIndex, r: &Obj) -> Vec<(String, i32)> {
    let id = s(r, "id");
    // (the default may lead through pass-through lanes: the block at their end is named)
    let d = idx.follow(r.get("default").and_then(Value::as_str)).and_then(|d| name_of(g, &d));
    let mut lines = vec![];
    if r.get("switch") == Some(&json!(true)) {
        let on = r.get("switch_on") == Some(&json!(true));
        lines.push((tr(if on { "Switch: on → then" } else { "Switch: off → else" }), if on { ON } else { MUTED }));
    } else if s(r, "style") == "if" {
        let m = list(r, "rules").next().and_then(|x| x.get("match").cloned()).unwrap_or(json!({}));
        let summary = match_summary(&m, 48);
        lines.push((format!("IF {}", if summary.is_empty() { tr("(no condition)") } else { summary }), NORMAL));
    } else {
        let count = tr("{count} rules").replace("{count}", &list(r, "rules").count().to_string());
        let default = tr("default: {name}").replace("{name}", d.as_deref().unwrap_or("-"));
        lines.push((format!("{count} · {default}"), NORMAL));
    }
    if d.is_none() {
        lines.push((tr("no default target"), WARN));
    }
    lines.push(used_line(&users(g, idx, id)));
    lines
}

/// what a link to `target` lands on: (block key, input lane)
fn landing(
    target: &str,
    lanes: &HashMap<String, (String, usize)>,
    blocks: &HashSet<String>,
) -> Option<(String, usize)> {
    if let Some(l) = lanes.get(target) {
        return Some(l.clone());
    }
    blocks.contains(target).then(|| (target.to_string(), 0))
}

/// Build the canvas of a validated graph. `summary(card id)` gives an inbound block's detail line,
/// `unlinked(card id)` whether its traffic reaches no outbound.
pub fn build(g: &Obj, summary: &dyn Fn(&str) -> String, unlinked: &dyn Fn(&str) -> bool) -> View {
    let mut v = View::default();
    let idx = proxydock_core::graph::lane_index(g);
    let wc = proxydock_core::colors::wire_colors(g);
    // (a mix is shown with the window's default link colour standing for "default", not Python's brown)
    let wire = |key: &str| {
        let (c, parts) = wc.get(key).cloned().unwrap_or_default();
        if parts.len() > 1 { (proxydock_core::colors::mix_with(&parts, DEFAULT_WIRE), parts) } else { (c, parts) }
    };
    // relay input lane id -> (key, lane) it lands on; relay output lane id -> (key, lane) it leaves from
    let mut in_lanes: HashMap<String, (String, usize)> = HashMap::new();
    // (key, lane, target, output lane id)
    let mut out_lanes: Vec<(String, usize, Option<String>, String)> = vec![];

    for b in list(g, "inbounds") {
        let id = s(b, "id");
        let mut n = node(id, 0, s(b, "name"), summary(id), pos(b.get("pos")));
        n.color = s(b, "color").into();
        n.card = id.into();
        n.lines = inbound_lines(b, unlinked(id));
        n.out_labels = vec![tr("Connect to")];
        v.nodes.push(n);
    }
    for b in list(g, "outbounds") {
        let id = s(b, "id");
        let ob = b.get("outbound").and_then(Value::as_object).cloned().unwrap_or_default();
        let t = s(&ob, "type");
        let detail = match t {
            "direct" | "block" | "group" | "subscription" => proxydock_core::graph::outbound_label(t),
            _ => format!(
                "{} {}",
                proxydock_core::graph::outbound_label(t),
                proxydock_core::net::host_port(s(&ob, "host"), ob.get("port").and_then(Value::as_i64).unwrap_or(0))
            ),
        };
        let mut n = node(id, 2, s(b, "name"), detail, pos(b.get("pos")));
        n.outs = if NO_VIA.contains(&t) && t != "group" { 0 } else { 1 };
        n.in_labels = vec![tr("Input")];
        n.out_labels = if t == "group" {
            vec![tr("Members")]
        } else if n.outs == 1 {
            vec![tr("via")]
        } else {
            vec![]
        };
        n.lines = outbound_lines(g, &idx, b);
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
        n.switch = (b.get("switch") == Some(&json!(true))).then(|| b.get("switch_on") == Some(&json!(true)));
        n.conds =
            rules.iter().map(|r| r.get("match").and_then(Value::as_object).is_some_and(|m| !m.is_empty())).collect();
        n.in_labels = vec![tr("Input")];
        n.lines = router_lines(g, &idx, b);
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
            a.in_labels = vec![tr("Input")];
            z.out_labels = vec![tr("Connect to")];
            // where it goes / where it comes from (Python: the halves' info text)
            if broken.is_empty() {
                let lane = ins.first().map(|l| s(l, "id")).unwrap_or("");
                let src = lane_sources(g, lane);
                let dest = idx.follow(Some(lane)).and_then(|t| name_of(g, &t)).unwrap_or_else(|| tr("not connected"));
                a.lines = vec![(format!("→ {dest}"), NORMAL)];
                z.lines =
                    vec![(format!("← {}", if src.is_empty() { "-".to_string() } else { src.join(", ") }), NORMAL)];
            } else {
                let text = tr("partner deleted");
                a.lines = vec![(text.clone(), WARN)];
                z.lines = vec![(text, WARN)];
            }
            if let Some(l) = ins.first() {
                in_lanes.insert(s(l, "id").into(), (id.into(), 0));
            }
            if let Some(l) = outs.first() {
                out_lanes.push((
                    out_key.clone(),
                    0,
                    l.get("target").and_then(Value::as_str).map(String::from),
                    s(l, "id").to_string(),
                ));
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
        n.routed = b["routed"] == json!(true);
        n.open = !g.get("relays_folded").and_then(Value::as_array).is_some_and(|f| f.iter().any(|x| x == id));
        // a lane's own name, else its number
        let lane_name = |l: &Obj, k: usize| {
            g.get("lane_names")
                .and_then(|m| m.get(s(l, "id")))
                .and_then(Value::as_str)
                .map_or_else(|| k.to_string(), String::from)
        };
        n.in_labels = ins.iter().enumerate().map(|(k, l)| lane_name(l, k + 1)).collect();
        n.out_labels = outs.iter().enumerate().map(|(k, l)| lane_name(l, k + 1)).collect();
        let out_index: HashMap<&str, usize> = outs.iter().enumerate().map(|(k, o)| (s(o, "id"), k)).collect();
        for (i, l) in ins.iter().enumerate() {
            in_lanes.insert(s(l, "id").into(), (id.into(), i));
            if let Some(o) = l.get("out").and_then(Value::as_str).and_then(|o| out_index.get(o)) {
                n.wires.push((i, *o));
            }
        }
        for (k, o) in outs.iter().enumerate() {
            out_lanes.push((
                id.into(),
                k,
                o.get("target").and_then(Value::as_str).map(String::from),
                s(o, "id").to_string(),
            ));
        }
        v.nodes.push(n);
    }
    if let Some(pc) = g.get("pc").and_then(Value::as_object) {
        let mut n = node("pc", 4, &tr("PC (system proxy)"), String::new(), pos(pc.get("pos")));
        n.ins = 0;
        n.out_labels = vec![tr("Connect to")];
        v.nodes.push(n);
    }

    // each block's colour, and a router's outputs' (their key, own colour, link colour)
    for n in v.nodes.iter_mut() {
        let base = n.key.strip_suffix("/out").unwrap_or(&n.key).to_string();
        n.swatch = wire(&base).0;
        if n.kind == 1
            && let Some((_, r)) = proxydock_core::graph::find(g, &n.key)
        {
            let mut keys: Vec<String> = r["rules"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|x| x["id"].as_str().map(String::from))
                .collect();
            keys.push("default".into());
            n.out_colors = keys
                .into_iter()
                .map(|k| {
                    let own = proxydock_core::colors::output_color(g, &n.key, &k);
                    let eff = wire(&proxydock_core::colors::router_out_key(&n.key, &k)).0;
                    (k, own, eff)
                })
                .collect();
        }
    }
    let blocks: HashSet<String> = v.nodes.iter().map(|n| n.key.clone()).collect();
    // a link takes the colour of what leaves there: a block, a router output, a pass-through output lane
    let link = |from: &str, out: usize, target: Option<&str>, colour_key: &str, v: &mut View| {
        let Some((to, inp)) = target.and_then(|t| landing(t, &in_lanes, &blocks)) else { return };
        if !blocks.contains(from) {
            return;
        }
        let (color, parts) = wire(colour_key);
        v.links.push(Link { from: from.into(), out, to, inp, color, mixed: parts.len() > 1 });
    };
    for b in list(g, "inbounds") {
        link(s(b, "id"), 0, b.get("target").and_then(Value::as_str), s(b, "id"), &mut v);
    }
    for b in list(g, "outbounds") {
        link(s(b, "id"), 0, b.get("via").and_then(Value::as_str), s(b, "id"), &mut v);
        for m in b.get("members").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            link(s(b, "id"), 0, Some(m), s(b, "id"), &mut v);
        }
    }
    for b in list(g, "routers") {
        let rules: Vec<&Obj> = list(b, "rules").collect();
        let rid = s(b, "id");
        for (i, r) in rules.iter().enumerate() {
            link(
                rid,
                i,
                r.get("target").and_then(Value::as_str),
                &proxydock_core::colors::router_out_key(rid, s(r, "id")),
                &mut v,
            );
        }
        link(
            rid,
            rules.len(),
            b.get("default").and_then(Value::as_str),
            &proxydock_core::colors::router_out_key(rid, "default"),
            &mut v,
        );
    }
    for (key, k, target, lane) in &out_lanes {
        link(key, *k, target.as_deref(), lane, &mut v);
    }
    if let Some(pc) = g.get("pc").and_then(Value::as_object) {
        link("pc", 0, pc.get("inbound").and_then(Value::as_str), "", &mut v);
    }
    v.paths = paths(&v);
    v
}

/// For every card and every exit block its traffic can reach, by its routers' defaults and by a rule: the links
/// on the way (the first way found, through routers and pass-through lanes as wired), then the exit's own "via"
/// chain. As the engine flattens routers: a switch sends everything one side, a rule without a condition is
/// skipped, and traffic that took a rule output anywhere counts as sent by a rule.
fn paths(v: &View) -> HashMap<Way, Vec<usize>> {
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
        // depth-first over (block, lane it came in on, by a rule so far), remembering the links taken
        let mut stack: Vec<(String, usize, bool, Vec<usize>)> = vec![(card.key.clone(), 0, false, vec![])];
        let mut seen = HashSet::new();
        while let Some((key, lane, rule, taken)) = stack.pop() {
            if !seen.insert((key.clone(), lane, rule)) {
                continue;
            }
            // (output lane, whether it is a rule's)
            let outs: Vec<(usize, bool)> = match kind.get(key.as_str()) {
                Some(0) if key != card.key => {
                    // into another card's local inbound: that card takes it from here
                    result.entry((card.key.clone(), key.clone(), rule)).or_insert(taken);
                    continue;
                }
                Some(2) => {
                    let mut p = taken.clone();
                    p.extend(via_chain(&key));
                    result.entry((card.key.clone(), key.clone(), rule)).or_insert(p);
                    continue;
                }
                Some(3) => {
                    if v.nodes.iter().any(|n| n.key == wireless_out(&key)) {
                        vec![(0, false)] // a wireless pair: on from the other half
                    } else {
                        wires
                            .get(key.as_str())
                            .map(|w| w.iter().filter(|(i, _)| *i == lane).map(|(_, o)| (*o, false)).collect())
                            .unwrap_or_default()
                    }
                }
                Some(1) => match v.nodes.iter().find(|n| n.key == key) {
                    // a switch: its side (then = output 0, else = the default, the last)
                    Some(n) if n.switch.is_some() => {
                        vec![(if n.switch == Some(true) { 0 } else { n.outs.saturating_sub(1) }, false)]
                    }
                    // the rules with a condition, then the default
                    Some(n) => (0..n.outs)
                        .filter_map(|o| {
                            if o + 1 == n.outs {
                                Some((o, false))
                            } else {
                                n.conds.get(o).copied().unwrap_or(false).then_some((o, true))
                            }
                        })
                        .collect(),
                    None => vec![],
                },
                Some(0) => vec![(0, false)],
                _ => vec![],
            };
            let from =
                if v.nodes.iter().any(|n| n.key == wireless_out(&key)) { wireless_out(&key) } else { key.clone() };
            for (o, by_rule) in outs.into_iter().rev() {
                for li in leaving.get(&(from.as_str(), o)).into_iter().flatten() {
                    let l = &v.links[*li];
                    let mut t = taken.clone();
                    t.push(*li);
                    stack.push((l.to.clone(), l.inp, rule || by_rule, t));
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
                "wires": n.wires.iter().map(|(i, o)| json!([i, o])).collect::<Vec<_>>(), "routed": n.routed, "open": n.open, "color": n.color,
                "card": n.card, "state": if n.card.is_empty() { "" } else { states(&n.card) },
                "lines": n.lines.iter().map(|(t, tone)| json!([t, tone])).collect::<Vec<_>>(),
                "swatch": n.swatch,
                "out_colors": n.out_colors.iter().map(|(k, own, eff)| json!([k, own, eff])).collect::<Vec<_>>(),
            })
        })
        .collect();
    let links: Vec<Value> = v
        .links
        .iter()
        .map(|l| json!({"from": l.from, "out": l.out, "to": l.to, "in": l.inp, "color": l.color, "mixed": l.mixed}))
        .collect();
    json!({"nodes": nodes, "links": links})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_colours_info_lines_and_port_names() {
        let g = proxydock_core::graph::validate(&json!({
            "inbounds": [
                {"id": "a", "name": "A", "inbound": {"type": "socks5", "port": 1}, "target": "o1", "color": "#e5534b"},
                {"id": "b", "name": "B", "inbound": {"type": "socks5", "port": 2}, "target": "o1", "color": "#4f8ef7"}
            ],
            "outbounds": [
                {"id": "o1", "name": "Hop", "outbound": {"type": "socks5", "host": "h", "port": 1}, "via": "o2"},
                {"id": "o2", "name": "Exit", "outbound": {"type": "socks5", "host": "x", "port": 2}}
            ]
        }));
        let v = build(&g, &|_| String::new(), &|id| id == "b");
        let hop = v.links.iter().find(|l| l.from == "o1").unwrap();
        assert!(hop.mixed, "two colours come into Hop: its link is a mix");
        assert!(!v.links.iter().find(|l| l.from == "a").unwrap().mixed);
        assert_eq!(v.links.iter().find(|l| l.from == "a").unwrap().color, "#e5534b");
        let node = |k: &str| v.nodes.iter().find(|n| n.key == k).unwrap();
        assert_eq!(node("o1").lines.last().unwrap().0, tr("used by: {names}").replace("{names}", "A, B"));
        assert_eq!(node("o1").lines[1].0, "⇢ Exit");
        assert_eq!(node("o2").out_labels, vec![tr("via")]);
        assert_eq!(node("a").out_labels, vec![tr("Connect to")]);
        assert!(node("b").lines.iter().any(|(_, tone)| *tone == WARN));
    }

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
        let v = build(&g, &|_| "SOCKS5".into(), &|_| false);
        let kinds: Vec<(String, i32, usize, usize)> =
            v.nodes.iter().map(|n| (n.key.clone(), n.kind, n.ins, n.outs)).collect();
        assert!(kinds.contains(&("r".into(), 1, 1, 2)));
        assert!(kinds.contains(&("d".into(), 2, 1, 0)));
        assert!(kinds.contains(&("s".into(), 2, 1, 1)));
        assert!(kinds.contains(&("rl".into(), 3, 1, 1)));
        let has = |from: &str, out: usize, to: &str, inp: usize| {
            v.links.iter().position(|l| l.from == from && l.out == out && l.to == to && l.inp == inp)
        };
        let (a, b, c, d, e) = (
            has("in1", 0, "r", 0).unwrap(),
            has("r", 0, "d", 0).unwrap(),
            has("r", 1, "rl", 0).unwrap(),
            has("rl", 0, "s", 0).unwrap(),
            has("s", 0, "hp", 0).unwrap(),
        );
        let way = |exit: &str, rule: bool| v.paths.get(&("in1".to_string(), exit.to_string(), rule)).cloned();
        assert_eq!(way("d", true), Some(vec![a, b]));
        assert_eq!(way("s", false), Some(vec![a, c, d, e]));
        assert_eq!(way("d", false), None);
    }

    #[test]
    fn a_rule_and_the_default_to_one_exit_are_apart() {
        // IF cn -> a switch (on: then) -> the exit; else -> the same exit straight
        let g = proxydock_core::graph::validate(&json!({
            "inbounds": [{"id": "in1", "name": "card", "inbound": {"type": "socks5", "port": 1080}, "target": "r"}],
            "outbounds": [
                {"id": "x", "name": "Exit", "outbound": {"type": "direct"}},
                {"id": "b", "name": "Block", "outbound": {"type": "block"}},
            ],
            "routers": [
                {"id": "r", "name": "CN", "style": "if", "rules": [{"id": "c1", "name": "IF", "match": {"geoip": ["cn"]}, "target": "s"}], "default": "x"},
                {"id": "s", "name": "Switch", "style": "if", "switch": true, "switch_on": true, "rules": [{"id": "c2", "name": "IF", "match": {}, "target": "x"}], "default": "b"},
            ],
        }));
        let v = build(&g, &|_| String::new(), &|_| false);
        let has = |from: &str, out: usize| v.links.iter().position(|l| l.from == from && l.out == out).unwrap();
        let way = |exit: &str, rule: bool| v.paths.get(&("in1".to_string(), exit.to_string(), rule)).cloned();
        let into = has("in1", 0);
        assert_eq!(way("x", false), Some(vec![into, has("r", 1)]));
        assert_eq!(way("x", true), Some(vec![into, has("r", 0), has("s", 0)]));
        // the switch's other side carries nothing
        assert_eq!(way("b", true), None);
        assert_eq!(way("b", false), None);
    }
}
