//! Link colours on the canvas (Python `graph.mix_colors / wire_colors / output_color / set_output_color`): a block
//! with its own colour uses it, one without follows the links coming in (their one colour, or an even mix in
//! OKLCh where different ones come in), pass-through lanes hand on what comes into them.

use crate::graph::{self, Obj};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};

/// the canvas's default link colour in mixes (Python `DEFAULT_LINK`, NodeGraphQt's pipe colour)
pub const DEFAULT_LINK: &str = "#af5f1e";

fn srgb_to_oklab(hexc: &str) -> (f64, f64, f64) {
    let lin = |c: f64| {
        let c = c / 255.0;
        if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    };
    let ch = |i: usize| lin(u8::from_str_radix(hexc.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0) as f64);
    let (r, g, b) = (ch(1), ch(3), ch(5));
    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    (
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    )
}

fn oklab_to_srgb((ll, a, b): (f64, f64, f64)) -> String {
    let l = (ll + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let m = (ll - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let s = (ll - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    let rgb = [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
    ];
    let enc = |c: f64| {
        let c = c.clamp(0.0, 1.0);
        // (Python round(): halves to even)
        let v = 255.0 * if c <= 0.0031308 { 12.92 * c } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 };
        let r = v.round();
        (if (v - v.trunc() - 0.5).abs() < 1e-12 && r % 2.0 != 0.0 { r - 1.0 } else { r }) as u8
    };
    format!("#{:02x}{:02x}{:02x}", enc(rgb[0]), enc(rgb[1]), enc(rgb[2]))
}

/// Even mix of "#rrggbb" colours ("" = the default link colour) in OKLCh: lightness and chroma averaged, the hue
/// the circular mean, so red + blue gives a vivid magenta instead of a muddy grey-purple.
pub fn mix_colors(colors: &[String]) -> String {
    mix_with(colors, DEFAULT_LINK)
}

/// `mix_colors` with "" standing for `default` (the window's own default link colour)
pub fn mix_with(colors: &[String], default: &str) -> String {
    let labs: Vec<(f64, f64, f64)> = colors.iter().map(|c| srgb_to_oklab(if c.is_empty() { default } else { c })).collect();
    if labs.is_empty() {
        return String::new();
    }
    let n = labs.len() as f64;
    let l = labs.iter().map(|x| x.0).sum::<f64>() / n;
    let c = labs.iter().map(|x| x.1.hypot(x.2)).sum::<f64>() / n;
    let (ux, uy) = (labs.iter().map(|x| x.1).sum::<f64>(), labs.iter().map(|x| x.2).sum::<f64>());
    if ux.hypot(uy) < 1e-3 * n {
        return oklab_to_srgb((l, ux / n, uy / n));
    }
    let h = uy.atan2(ux);
    oklab_to_srgb((l, c * h.cos(), c * h.sin()))
}

/// the colours key of router `rid`'s output `key` (rule id or "default")
pub fn router_out_key(rid: &str, key: &str) -> String {
    format!("{rid}/{key}")
}

type Parts = BTreeSet<String>;

enum Src {
    Block(String),
    Out(String),
    Rout(String),
    In(String),
}

struct Ctx<'a> {
    feeds: HashMap<String, Vec<Src>>,
    blocks: HashMap<String, (&'a Value, bool)>,
    /// output lane id -> its relay; input lane id -> output lane id it is wired to
    out_relay: HashMap<String, &'a Value>,
    rule_color: HashMap<String, (String, String)>,
    memo: std::cell::RefCell<HashMap<String, Parts>>,
}

impl Ctx<'_> {
    fn source(&self, src: &Src, busy: &HashSet<String>) -> Parts {
        match src {
            Src::Block(b) => self.block(b, busy),
            Src::Out(o) => self.out(o, busy),
            Src::Rout(k) => self.rout(k, busy),
            Src::In(l) => self.input(l, busy),
        }
    }

    fn incoming(&self, target: &str, busy: &HashSet<String>) -> Parts {
        let mut got = Parts::new();
        for s in self.feeds.get(target).into_iter().flatten() {
            got.extend(self.source(s, busy));
        }
        got
    }

    fn block(&self, bid: &str, busy: &HashSet<String>) -> Parts {
        let Some((n, relay)) = self.blocks.get(bid) else { return Parts::new() };
        let own = n["color"].as_str().unwrap_or("");
        if !own.is_empty() && !relay {
            return [own.to_string()].into();
        }
        if busy.contains(bid) {
            return Parts::new();
        }
        let mut b = busy.clone();
        b.insert(bid.to_string());
        let got = self.incoming(bid, &b);
        if *relay || !got.is_empty() { got } else { [String::new()].into() }
    }

    fn input(&self, lid: &str, busy: &HashSet<String>) -> Parts {
        if busy.contains(lid) {
            return Parts::new();
        }
        let mut b = busy.clone();
        b.insert(lid.to_string());
        self.incoming(lid, &b)
    }

    fn rout(&self, key: &str, busy: &HashSet<String>) -> Parts {
        let Some((own, rid)) = self.rule_color.get(key) else { return Parts::new() };
        if own.is_empty() { self.block(rid, busy) } else { [own.clone()].into() }
    }

    fn out(&self, oid: &str, busy: &HashSet<String>) -> Parts {
        if let Some(p) = self.memo.borrow().get(oid) {
            return p.clone();
        }
        if busy.contains(oid) {
            return Parts::new();
        }
        let mut b = busy.clone();
        b.insert(oid.to_string());
        let mut got = Parts::new();
        if let Some(r) = self.out_relay.get(oid) {
            for x in r["inputs"].as_array().into_iter().flatten() {
                if x["out"].as_str() == Some(oid) {
                    got.extend(self.input(x["id"].as_str().unwrap_or(""), &b));
                }
            }
        }
        self.memo.borrow_mut().insert(oid.to_string(), got.clone());
        got
    }
}

/// (colour, parts) of one wire: parts = the colours that make it up ("" = the default); more than one = a mix
pub type Wire = (String, Vec<String>);

fn colour(parts: &Parts, own: &str) -> String {
    match parts.len() {
        0 => own.to_string(),
        1 => parts.iter().next().unwrap().clone(),
        _ => mix_colors(&parts.iter().cloned().collect::<Vec<_>>()),
    }
}

/// Python `wire_colors`: {block id / pass-through lane id / router output key: (colour, parts)}
pub fn wire_colors(g: &Obj) -> HashMap<String, Wire> {
    let list = |k: &str| g.get(k).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let s = |v: &Value, k: &str| v[k].as_str().unwrap_or("").to_string();
    let mut feeds: HashMap<String, Vec<Src>> = HashMap::new();
    let mut feed = |t: &Value, src: Src| {
        if let Some(t) = t.as_str() {
            feeds.entry(t.to_string()).or_default().push(src);
        }
    };
    for n in list("inbounds") {
        feed(&n["target"], Src::Block(s(n, "id")));
    }
    for r in list("routers") {
        for (key, t) in graph::router_targets(r) {
            feed(&json!(t), Src::Rout(router_out_key(&s(r, "id"), &key)));
        }
    }
    for n in list("outbounds") {
        feed(&n["via"], Src::Block(s(n, "id")));
        for m in n["members"].as_array().into_iter().flatten() {
            feed(m, Src::Block(s(n, "id")));
        }
    }
    for r in list("relays") {
        for x in r["outputs"].as_array().into_iter().flatten() {
            feed(&x["target"], Src::Out(s(x, "id")));
        }
        for x in r["inputs"].as_array().into_iter().flatten() {
            feed(&r["id"], Src::In(s(x, "id")));
        }
    }
    let mut blocks = HashMap::new();
    for kind in ["inbounds", "outbounds", "routers", "relays"] {
        for n in list(kind) {
            blocks.insert(s(n, "id"), (n, kind == "relays"));
        }
    }
    let mut out_relay = HashMap::new();
    let mut in_lanes = vec![];
    for r in list("relays") {
        for x in r["outputs"].as_array().into_iter().flatten() {
            out_relay.insert(s(x, "id"), r);
        }
        for x in r["inputs"].as_array().into_iter().flatten() {
            in_lanes.push(s(x, "id"));
        }
    }
    let mut rule_color = HashMap::new();
    for r in list("routers") {
        let rid = s(r, "id");
        for x in r["rules"].as_array().into_iter().flatten() {
            rule_color.insert(router_out_key(&rid, &s(x, "id")), (s(x, "color"), rid.clone()));
        }
        rule_color.insert(router_out_key(&rid, "default"), (s(r, "default_color"), rid.clone()));
    }
    let ctx = Ctx { feeds, blocks, out_relay, rule_color, memo: Default::default() };
    let none = HashSet::new();
    let wire = |p: Parts, own: &str| (colour(&p, own), p.into_iter().collect::<Vec<_>>());
    let mut out = HashMap::new();
    for (bid, (n, relay)) in &ctx.blocks {
        let own = if *relay { String::new() } else { s(n, "color") };
        out.insert(bid.clone(), wire(ctx.block(bid, &none), &own));
    }
    for oid in ctx.out_relay.keys() {
        out.insert(oid.clone(), wire(ctx.out(oid, &none), ""));
    }
    for lid in &in_lanes {
        out.insert(lid.clone(), wire(ctx.input(lid, &none), ""));
    }
    for key in ctx.rule_color.keys() {
        out.insert(key.clone(), wire(ctx.rout(key, &none), ""));
    }
    out
}

/// own colour of router `bid`'s output `key` (rule id or "default"); "" for anything else
pub fn output_color(g: &Obj, bid: &str, key: &str) -> String {
    match graph::find(g, bid) {
        Some(("router", n)) if key == "default" => n["default_color"].as_str().unwrap_or("").into(),
        Some(("router", n)) => n["rules"].as_array().into_iter().flatten().find(|x| x["id"] == json!(key)).and_then(|x| x["color"].as_str()).unwrap_or("").into(),
        _ => String::new(),
    }
}

/// Give output `key` of router `bid` its own colour ("" = none); false when there is no such output
pub fn set_output_color(g: &mut Obj, bid: &str, key: &str, color: &str) -> bool {
    let color = graph::valid_color(Some(&json!(color)));
    let Some(r) = g.get_mut("routers").and_then(Value::as_array_mut).and_then(|a| a.iter_mut().find(|r| r["id"] == json!(bid))) else { return false };
    if key == "default" {
        r["default_color"] = json!(color);
        return true;
    }
    match r["rules"].as_array_mut().and_then(|a| a.iter_mut().find(|x| x["id"] == json!(key))) {
        Some(x) => {
            x["color"] = json!(color);
            true
        }
        None => false,
    }
}

/// a block's own colour ("" = automatic); pass-throughs have none (their lanes follow what comes in)
pub fn set_color(g: &mut Obj, bid: &str, color: &str) -> bool {
    let color = graph::valid_color(Some(&json!(color)));
    for kind in ["inbounds", "outbounds", "routers"] {
        if let Some(n) = g.get_mut(kind).and_then(Value::as_array_mut).and_then(|a| a.iter_mut().find(|n| n["id"] == json!(bid))) {
            if n["color"] == json!(color) {
                return false;
            }
            n["color"] = json!(color);
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixes_like_python() {
        // (values from Python graph.mix_colors)
        assert_eq!(mix_colors(&["#e5534b".into(), "#4f8ef7".into()]), PY_RED_BLUE);
        assert_eq!(mix_colors(&["#ff0000".into()]), "#ff0000");
        assert_eq!(mix_colors(&[]), "");
    }

    const PY_RED_BLUE: &str = "#c561c4";

    #[test]
    fn follows_and_mixes() {
        let g = graph::validate(&json!({
            "inbounds": [
                {"id": "a", "name": "A", "inbound": {"type": "socks5", "port": 1}, "target": "r", "color": "#e5534b"},
                {"id": "b", "name": "B", "inbound": {"type": "socks5", "port": 2}, "target": "d", "color": "#4f8ef7"}
            ],
            "outbounds": [{"id": "d", "name": "D", "outbound": {"type": "direct"}}],
            "routers": [{"id": "r", "name": "R", "rules": [{"id": "x", "name": "x", "match": {"domain": ["a"]}, "target": "d", "color": "#57ab5a"}], "default": "d"}]
        }));
        let w = wire_colors(&g);
        assert_eq!(w["a"].0, "#e5534b");
        assert_eq!(w["r"].0, "#e5534b");
        assert_eq!(w[&router_out_key("r", "x")].0, "#57ab5a");
        assert_eq!(w[&router_out_key("r", "default")].0, "#e5534b");
        // D gets red (the router default), green (the rule) and blue (B): a mix
        assert_eq!(w["d"].1.len(), 3);
        assert_eq!(w["d"].0, "#d95f9a");
    }
}
