"""Block graph: inbound blocks, outbound blocks and the PC (system proxy) block.

graph = {
  "inbounds":  [{"id", "name", "autostart", "auto_restart", "inbound": {...}, "overrides": {...}, "target": id|None, "pos": [x, y]}],
  "outbounds": [{"id", "name", "outbound": {...}, "via": id|None, "members": [id, ...], "pos": [x, y]}],
  "routers":   [{"id", "name", "style": "router"|"if", "rules": [{"id", "name", "match": {...}, "target": id|None}],
                 "default": id|None, "pos": [x, y]}],
  "relays":    [{"id", "name", "routed": bool, "inputs": [{"id", "gap", "out": output lane id|None}],
                 "outputs": [{"id", "gap", "target": id|None}], "pos": [x, y]}],
  "pc":        {"inbound": id|None, "pos": [x, y], "backup": {...}|None, "applied": bool, "server": str},
}

An inbound's target is either an outbound block (traffic leaves through it) or another inbound block
(relay: traffic is handed to that local inbound). Several inbounds can share one outbound block.
An outbound block's "via" is another outbound block its own server connection goes through
(e.g. Shadowsocks via the company HTTP proxy); Direct and Hysteria2 (QUIC/UDP) cannot have a via.
A group outbound ("group") uses its member blocks (node["members"], in order) as one: the fastest one, or the
first one that works (failover). Members are outbound blocks other than groups and Block; nothing goes "via" a group.
An inbound can also target a router block: its rules (checked top to bottom) send matching traffic to an outbound
block, the Block outbound or another router; everything else goes to the router's default. Routers need the
sing-box engine (they become sing-box route rules). An IF block is a router with exactly one rule: its "then"
output is the rule's target, its "else" output the default. An IF in switch mode ("switch": True) has no condition:
"switch_on" sends everything to "then", off to "else" (a manual switch / kill switch).
A pass-through block ("relays") only tidies the wiring: a link can go to one of its input lanes, and each input
lane is wired inside the block to an output lane, whose link goes on to an outbound block, a router or another
pass-through lane. Straight blocks wire lane i to lane i; routed ones ("routed": True) can cross, merge and have
different input and output counts. "gap" = empty rows above a lane on the canvas. compile() follows the lanes
(without_relays) so the runtime never sees them.
Every block can have a "color" ("#rrggbb", "" = automatic): the canvas draws the links leaving it in that colour.
A block without one follows the links coming into it (their mix where different colours meet, drawn dashed), and
a pass-through does the same lane by lane - see wire_colors().
The runtime still works with one "proxy" per inbound (inbound + resolved outbound + overrides, plus "routes" for a
router: [{"match": [match, ...], "outbound": resolved chain, "name": rule name}]); compile() builds them.
"""
import copy
import json
import uuid

from .common import INBOUND_TYPES, OUTBOUND_TYPES, host_port
from .i18n import _t


NO_VIA = ("direct", "hysteria2", "block", "subscription", "group")   # end a chain (their servers dial out themselves)
NO_MEMBER = ("group", "block")      # cannot be a group member

# router rule conditions: lists of strings, plus ip_private (bool). Inside one rule, sing-box ORs the address
# conditions (domain*, ip_cidr, ip_private, geoip, geosite) and ANDs the rest (port, process_name).
MATCH_LISTS = ("domain", "domain_suffix", "domain_keyword", "ip_cidr", "port", "process_name", "geoip", "geosite")
LOWER_KEYS = ("domain", "domain_suffix", "domain_keyword", "geoip", "geosite")


def new_id():
    return uuid.uuid4().hex[:8]


def empty():
    return {"inbounds": [], "outbounds": [], "routers": [], "relays": [],
            "pc": {"inbound": None, "pos": [460, -200], "backup": None, "applied": False, "server": ""}}


def _pos(v, default=(0, 0)):
    if isinstance(v, (list, tuple)) and len(v) == 2 and all(isinstance(x, (int, float)) for x in v):
        return [float(v[0]), float(v[1])]
    return list(default)


def _name(v, default):
    return v.strip()[:60] if isinstance(v, str) and v.strip() else default


def valid_color(v):
    """'#rrggbb' (lower case) or '' (no colour: the default)."""
    if isinstance(v, str) and len(v) == 7 and v[0] == "#":
        try:
            int(v[1:], 16)
            return v.lower()
        except ValueError:
            pass
    return ""


def validate_outbound_node(raw):
    from . import config
    from .ss import SS_METHODS
    raw = raw if isinstance(raw, dict) else {}
    ob = config._typed(config.OUTBOUND_DEFAULTS, raw.get("outbound"))
    if ob["type"] not in OUTBOUND_TYPES:
        ob["type"] = "direct"
    if ob["ss_method"] not in SS_METHODS:
        ob["ss_method"] = config.OUTBOUND_DEFAULTS["ss_method"]
    if not 0 <= ob["port"] <= 65535:
        ob["port"] = 0
    via = raw.get("via")
    members = []
    if ob["type"] == "group":
        for m in raw.get("members") or []:
            if isinstance(m, str) and m and m not in members:
                members.append(m)
    return {"id": raw.get("id") if isinstance(raw.get("id"), str) and raw.get("id") else new_id(),
            "name": _name(raw.get("name"), default_outbound_name(ob)), "outbound": ob,
            "via": via if isinstance(via, str) and via and ob["type"] not in NO_VIA else None,
            "members": members, "pos": _pos(raw.get("pos")), "color": valid_color(raw.get("color"))}


def validate_inbound_node(raw):
    from . import config
    raw = raw if isinstance(raw, dict) else {}
    p = config.validate_proxy(raw)
    target = raw.get("target")
    return {"id": p["id"], "name": p["name"], "autostart": p["autostart"], "auto_restart": p["auto_restart"],
            "inbound": p["inbound"],
            "overrides": p["overrides"], "target": target if isinstance(target, str) and target else None,
            "pos": _pos(raw.get("pos")), "color": valid_color(raw.get("color"))}


def validate_match(raw):
    raw = raw if isinstance(raw, dict) else {}
    m = {}
    for k in MATCH_LISTS:
        v = raw.get(k)
        if isinstance(v, list):
            items = [x.strip() for x in v if isinstance(x, str) and x.strip()]
            if k in LOWER_KEYS:
                items = [x.lower() for x in items]
            if items:
                m[k] = items
    if raw.get("ip_private") is True:
        m["ip_private"] = True
    return m


def validate_router_node(raw):
    raw = raw if isinstance(raw, dict) else {}
    rules, seen = [], set()
    for i, r in enumerate(raw.get("rules") or []):
        if not isinstance(r, dict):
            continue
        rid = r.get("id") if isinstance(r.get("id"), str) and r.get("id") else new_id()
        if rid in seen:
            rid = new_id()
        seen.add(rid)
        t = r.get("target")
        rules.append({"id": rid, "name": _name(r.get("name"), _t("Rule {n}").format(n=i + 1)),
                      "match": validate_match(r.get("match")), "target": t if isinstance(t, str) and t else None})
    d = raw.get("default")
    style = "if" if raw.get("style") == "if" else "router"
    if style == "if":           # exactly one condition set
        rules = rules[:1] or [{"id": new_id(), "name": "IF", "match": {}, "target": None}]
    return {"id": raw.get("id") if isinstance(raw.get("id"), str) and raw.get("id") else new_id(),
            "name": _name(raw.get("name"), "IF" if style == "if" else "Router"), "style": style, "rules": rules,
            "switch": style == "if" and raw.get("switch") is True, "switch_on": raw.get("switch_on") is True,
            "default": d if isinstance(d, str) and d else None, "pos": _pos(raw.get("pos")),
            "color": valid_color(raw.get("color"))}


MAX_LANES = 24
MAX_GAP = 4


def _lanes(items, key):
    out, seen = [], set()
    for x in (items if isinstance(items, list) else [])[:MAX_LANES]:
        if not isinstance(x, dict):
            continue
        lid = x.get("id") if isinstance(x.get("id"), str) and x.get("id") else new_id()
        if lid in seen:
            lid = new_id()
        seen.add(lid)
        gap = x.get("gap")
        v = x.get(key)
        out.append({"id": lid, "gap": gap if type(gap) is int and 0 <= gap <= MAX_GAP else 0,
                    key: v if isinstance(v, str) and v else None})
    return out


def new_lane(key):
    return {"id": new_id(), "gap": 0, key: None}


def validate_relay_node(raw):
    raw = raw if isinstance(raw, dict) else {}
    routed = raw.get("routed") is True
    ins = _lanes(raw.get("inputs"), "out") or [new_lane("out")]
    outs = _lanes(raw.get("outputs"), "target") or [new_lane("target")]
    if routed:
        out_ids = {o["id"] for o in outs}
        for i in ins:
            if i["out"] not in out_ids:
                i["out"] = None
    else:                       # straight: lane i -> lane i, the output side mirrors the input side
        while len(outs) < len(ins):
            outs.append(new_lane("target"))
        outs = outs[:len(ins)]
        for i, o in zip(ins, outs):
            o["gap"] = i["gap"]
            i["out"] = o["id"]
    return {"id": raw.get("id") if isinstance(raw.get("id"), str) and raw.get("id") else new_id(),
            "name": _name(raw.get("name"), _t("Pass-through")), "routed": routed,
            "inputs": ins, "outputs": outs, "pos": _pos(raw.get("pos")), "color": valid_color(raw.get("color"))}


def lane_index(graph):
    """({input lane id: (relay, lane)}, {output lane id: (relay, lane)})."""
    ins, outs = {}, {}
    for r in graph.get("relays", ()):
        for x in r["inputs"]:
            ins[x["id"]] = (r, x)
        for x in r["outputs"]:
            outs[x["id"]] = (r, x)
    return ins, outs


def _walk(graph, tid, index=None):
    """(block id at the end of the pass-through lanes from tid or None, True if they loop)."""
    ins, outs = index or lane_index(graph)
    seen = set()
    while tid in ins:
        if tid in seen:
            return None, True
        seen.add(tid)
        o = outs.get(ins[tid][1]["out"])
        tid = o[1]["target"] if o else None
    return tid, False


def follow(graph, tid, index=None):
    """Where a link to tid really goes: through pass-through lanes to the block behind them (None = nowhere)."""
    return _walk(graph, tid, index)[0]


def lane_loop(graph, tid):
    """True if following pass-through lanes from tid comes back around."""
    return _walk(graph, tid)[1]


def is_lane(graph, tid):
    return tid in lane_index(graph)[0]


def lane_label(graph, tid):
    """'Pass-through name · 3' for an input lane id (None if it is not one)."""
    hit = lane_index(graph)[0].get(tid)
    if hit is None:
        return None
    r, lane = hit
    return _t("{name} · lane {n}").format(name=r["name"], n=r["inputs"].index(lane) + 1)


def lane_choices(graph):
    """[(input lane id, label)] of every pass-through lane, for target pickers."""
    return [(x["id"], lane_label(graph, x["id"])) for r in graph.get("relays", ()) for x in r["inputs"]]


DEFAULT_LINK = "#af5f1e"         # the canvas's default link colour (NodeGraphQt's pipe colour)


def _srgb_to_oklab(hexc):
    def lin(c):
        c /= 255
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = (lin(int(hexc[i:i + 2], 16)) for i in (1, 3, 5))
    l_ = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b) ** (1 / 3)
    m_ = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b) ** (1 / 3)
    s_ = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b) ** (1 / 3)
    return (0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
            1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
            0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_)


def _oklab_to_srgb(lab):
    L, A, B = lab
    l_ = (L + 0.3963377774 * A + 0.2158037573 * B) ** 3
    m_ = (L - 0.1055613458 * A - 0.0638541728 * B) ** 3
    s_ = (L - 0.0894841775 * A - 1.2914855480 * B) ** 3
    rgb = (4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_,
           -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_,
           -0.0041960863 * l_ - 0.7034186147 * m_ + 1.7076147010 * s_)

    def enc(c):
        c = min(max(c, 0.0), 1.0)
        return round(255 * (12.92 * c if c <= 0.0031308 else 1.055 * c ** (1 / 2.4) - 0.055))
    return "#%02x%02x%02x" % tuple(enc(c) for c in rgb)


def mix_colors(colors):
    """Even mix of '#rrggbb' colours ('' = the default link colour) in OKLCh: lightness and chroma are averaged
    and the hue is the circular mean, so red + blue gives a vivid magenta instead of a muddy grey-purple."""
    import math
    labs = [_srgb_to_oklab(c or DEFAULT_LINK) for c in colors]
    if not labs:
        return ""
    n = len(labs)
    L = sum(x[0] for x in labs) / n
    C = sum(math.hypot(x[1], x[2]) for x in labs) / n
    ux = sum(x[1] for x in labs)            # hue: mean direction weighted by chroma (a grey hardly pulls it)
    uy = sum(x[2] for x in labs)
    if math.hypot(ux, uy) < 1e-3 * n:       # hues cancel out (e.g. opposite colours): plain average
        return _oklab_to_srgb((L, sum(x[1] for x in labs) / n, sum(x[2] for x in labs) / n))
    h = math.atan2(uy, ux)
    return _oklab_to_srgb((L, C * math.cos(h), C * math.sin(h)))


def wire_colors(graph):
    """Link colours on the canvas: {block id or pass-through lane id: (colour, parts)}.
    parts = the colours that make it up ('' = the default colour); len(parts) > 1 = a mix (drawn dashed).
    - A block with its own colour uses it.
    - A block without one follows the links coming into it: their one colour, or the mix where different
      colours come in; with nothing coming in, the default colour.
    - A pass-through lane hands on what comes into it the same way; a lane with nothing coming in uses the
      pass-through's own colour (parts = ())."""
    ins, outs = lane_index(graph)
    feeds = {}                  # block id / input lane id -> [("b", source block id) | ("o", output lane id)]

    def feed(target, source):
        if target:
            feeds.setdefault(target, []).append(source)
    for n in graph["inbounds"]:
        feed(n["target"], ("b", n["id"]))
    for r in graph.get("routers", []):
        for _, t in router_targets(r):
            feed(t, ("b", r["id"]))
    for n in graph["outbounds"]:
        feed(n.get("via"), ("b", n["id"]))
        for m in n.get("members", ()):
            feed(m, ("b", n["id"]))
    for r in graph.get("relays", []):
        for x in r["outputs"]:
            feed(x["target"], ("o", x["id"]))
        for x in r["inputs"]:               # a pass-through block (its swatch) shows what flows through it
            feed(r["id"], ("i", x["id"]))
    blocks = {n["id"]: n for kind in ("inbounds", "outbounds", "routers", "relays") for n in graph.get(kind, [])}
    memo = {}

    def source_parts(src, busy):
        kind, v = src
        if kind == "b":
            return block_parts(v, busy)
        if kind == "o":
            return out_parts(v, busy)
        return in_parts(v, busy)

    def incoming(target, busy):
        got = set()
        for src in feeds.get(target, ()):
            got |= set(source_parts(src, busy))
        return got

    def block_parts(bid, busy):
        n = blocks.get(bid)
        if n is None:
            return ()
        if n.get("color"):
            return (n["color"],)
        if bid in busy:                     # (a loop: cannot be used anyway)
            return ()
        got = incoming(bid, busy | {bid})
        if n in graph.get("relays", ()):    # a pass-through with nothing flowing through: its own (default) colour
            return tuple(sorted(got))
        return tuple(sorted(got)) or ("",)

    def in_parts(lid, busy):
        if lid in busy:
            return ()
        return tuple(sorted(incoming(lid, busy | {lid})))

    def out_parts(oid, busy):
        if oid in memo:
            return memo[oid]
        if oid in busy:
            return ()
        busy = busy | {oid}
        r, _ = outs[oid]
        got = set()
        for x in r["inputs"]:
            if x["out"] == oid:
                got |= set(in_parts(x["id"], busy))
        memo[oid] = tuple(sorted(got))
        return memo[oid]

    def colour(parts, own=""):
        if not parts:
            return own
        return parts[0] if len(parts) == 1 else mix_colors(parts)
    out = {}
    for bid, n in blocks.items():
        parts = block_parts(bid, frozenset())
        out[bid] = (colour(parts, n.get("color", "")), parts)
    for oid, (r, _) in outs.items():
        parts = out_parts(oid, frozenset())
        out[oid] = (colour(parts, r.get("color", "")), parts)
    for lid, (r, _) in ins.items():
        parts = in_parts(lid, frozenset())
        out[lid] = (colour(parts, r.get("color", "")), parts)
    return out


def without_relays(graph):
    """The graph with every link into a pass-through lane replaced by where the lanes lead (and no relays)."""
    if not graph.get("relays"):
        return graph
    idx = lane_index(graph)
    return dict(graph, relays=[],
                inbounds=[dict(n, target=follow(graph, n["target"], idx)) for n in graph["inbounds"]],
                routers=[dict(r, default=follow(graph, r["default"], idx),
                              rules=[dict(x, target=follow(graph, x["target"], idx)) for x in r["rules"]])
                         for r in graph.get("routers", [])])


def router_targets(r):
    """(rule id or "default", target id) for every link out of router r."""
    return [(x["id"], x["target"]) for x in r["rules"] if x["target"]] + ([("default", r["default"])] if r["default"] else [])


def router_loop(graph, rid):
    """True if following router links from router rid comes back to it."""
    routers = {r["id"]: r for r in graph.get("routers", [])}
    idx = lane_index(graph)
    stack, seen = ([follow(graph, t, idx) for _, t in router_targets(routers[rid])] if rid in routers else []), set()
    while stack:
        cur = stack.pop()
        if cur == rid:
            return True
        if cur in seen or cur not in routers:
            continue
        seen.add(cur)
        stack += [follow(graph, t, idx) for _, t in router_targets(routers[cur])]
    return False


def validate(raw):
    raw = raw if isinstance(raw, dict) else {}
    g = empty()
    seen = set()
    for item in raw.get("outbounds") or []:
        n = validate_outbound_node(item)
        if n["id"] not in seen:
            seen.add(n["id"])
            g["outbounds"].append(n)
    for item in raw.get("inbounds") or []:
        n = validate_inbound_node(item)
        if n["id"] not in seen:
            seen.add(n["id"])
            g["inbounds"].append(n)
    for item in raw.get("routers") or []:
        n = validate_router_node(item)
        if n["id"] not in seen:
            seen.add(n["id"])
            g["routers"].append(n)
    for item in raw.get("relays") or []:
        n = validate_relay_node(item)
        lane_ids = [x["id"] for x in n["inputs"] + n["outputs"]]
        if n["id"] not in seen and not seen & set(lane_ids):
            seen |= {n["id"], *lane_ids}
            g["relays"].append(n)
    lanes_in = set(lane_index(g)[0])
    ids = {n["id"] for n in g["inbounds"]} | {n["id"] for n in g["outbounds"]} | {n["id"] for n in g["routers"]}
    for n in g["inbounds"]:
        if n["target"] not in ids | lanes_in or n["target"] == n["id"]:
            n["target"] = None
    rule_ids = {n["id"] for n in g["outbounds"]} | {n["id"] for n in g["routers"]}
    router_ids = {n["id"] for n in g["routers"]}
    for r in g["relays"]:                   # lanes lead on to outbound blocks, routers or other lanes
        for x in r["outputs"]:
            if x["target"] not in rule_ids | lanes_in:
                x["target"] = None
    for r in g["relays"]:                   # lanes going round in a circle cannot be used: cut them
        for x in r["outputs"]:
            if x["target"] in lanes_in and lane_loop(g, x["target"]):
                x["target"] = None
    for r in g["routers"]:                  # router links go to outbound blocks, other routers or lanes
        for x in r["rules"]:
            if x["target"] not in rule_ids | lanes_in or x["target"] == r["id"]:
                x["target"] = None
        if r["default"] not in rule_ids | lanes_in or r["default"] == r["id"]:
            r["default"] = None
    for r in g["routers"]:                  # a router loop cannot be used: cut it
        while router_loop(g, r["id"]):
            key = next(k for k, t in reversed(router_targets(r)) if follow(g, t) in router_ids)
            if key == "default":
                r["default"] = None
            else:
                next(x for x in r["rules"] if x["id"] == key)["target"] = None
    out_ids = {n["id"] for n in g["outbounds"]}
    for n in g["outbounds"]:
        if n["via"] not in out_ids or n["via"] == n["id"]:
            n["via"] = None
    for n in g["outbounds"]:            # a via loop cannot be used: cut it
        if via_loop(g, n["id"]):
            n["via"] = None
    by_id = {n["id"]: n for n in g["outbounds"]}
    for n in g["outbounds"]:            # nothing goes through a group; members are plain outbound blocks
        if n["via"] and by_id[n["via"]]["outbound"]["type"] == "group":
            n["via"] = None
        n["members"] = [m for m in n["members"] if m in by_id and m != n["id"]
                        and by_id[m]["outbound"]["type"] not in NO_MEMBER]
    pc = raw.get("pc") if isinstance(raw.get("pc"), dict) else {}
    g["pc"]["pos"] = _pos(pc.get("pos"), g["pc"]["pos"])
    if pc.get("inbound") in {n["id"] for n in g["inbounds"]}:
        g["pc"]["inbound"] = pc["inbound"]
    if isinstance(pc.get("backup"), dict):
        g["pc"]["backup"] = {k: v for k, v in pc["backup"].items()
                             if isinstance(k, str) and (v is None or isinstance(v, (str, int)))}
    g["pc"]["applied"] = pc.get("applied") is True
    g["pc"]["server"] = pc.get("server") if isinstance(pc.get("server"), str) else ""
    return g


# --- lookups ---

def find(graph, nid):
    """('inbound'|'outbound'|'router'|'relay', node) or (None, None). Pass-through lane ids are not blocks."""
    for kind in ("inbounds", "outbounds", "routers", "relays"):
        for n in graph.get(kind, ()):
            if n["id"] == nid:
                return kind[:-1], n
    return None, None


def via_loop(graph, nid):
    """True if following the via links from outbound nid comes back to it."""
    seen, cur = set(), nid
    while cur:
        if cur in seen:
            return True
        seen.add(cur)
        _, n = find(graph, cur)
        cur = n["via"] if n is not None and "via" in n else None
    return False


def resolve_outbound(graph, nid):
    """Outbound settings with the via chain nested in ob["via"] (None at the end)."""
    seen, chain, cur = set(), [], nid
    while cur and cur not in seen:
        seen.add(cur)
        kind, n = find(graph, cur)
        if kind != "outbound":
            break
        chain.append(n)
        cur = n["via"]
    ob = None
    for n in reversed(chain):
        d = copy.deepcopy(n["outbound"])
        d["via"] = ob
        if d["type"] == "group":
            d["members"] = [{"id": mid, "name": find(graph, mid)[1]["name"], "outbound": resolve_outbound(graph, mid)}
                            for mid in n.get("members", []) if find(graph, mid)[0] == "outbound"]
        ob = d
    return ob


def chain_names(graph, nid):
    names, seen, cur = [], set(), nid
    while cur and cur not in seen:
        seen.add(cur)
        kind, n = find(graph, cur)
        if kind != "outbound":
            break
        names.append(n["name"])
        cur = n["via"]
    return names


def users(graph, nid):
    """Inbound blocks whose traffic goes to nid (directly or through pass-through lanes)."""
    idx = lane_index(graph)
    return [n for n in graph["inbounds"] if n["target"] and follow(graph, n["target"], idx) == nid]


def used_by(graph, nid):
    """Names of the blocks that send traffic to block nid: inbounds, groups (members) and routers (rule targets),
    looking through pass-through blocks."""
    idx = lane_index(graph)
    names = [n["name"] for n in users(graph, nid)] + [n["name"] for n in groups_of(graph, nid)]
    names += [r["name"] for r in graph.get("routers", []) if any(follow(graph, t, idx) == nid for _, t in router_targets(r))]
    return names


def lane_sources(graph, lane_id):
    """Names of the blocks linked straight into input lane lane_id (inbounds, router outputs, other lanes)."""
    names = [n["name"] for n in graph["inbounds"] if n["target"] == lane_id]
    names += [r["name"] for r in graph.get("routers", []) if any(t == lane_id for _, t in router_targets(r))]
    names += [r["name"] for r in graph.get("relays", []) if any(x["target"] == lane_id for x in r["outputs"])]
    return names


def groups_of(graph, nid):
    """Group blocks that have outbound nid as a member."""
    return [n for n in graph["outbounds"] if nid in n.get("members", ())]


def default_outbound_name(ob):
    if ob["type"] == "block":
        return _t("Block")
    if ob["type"] == "subscription":
        return _t("Subscription")
    if ob["type"] == "group":
        return _t("Group")
    if ob["type"] == "direct":
        where = ob.get("interface") or ob.get("bind")
        return "Direct" + (f" ({where})" if where else "")
    label = OUTBOUND_TYPES[ob["type"]]
    return f"{label} {host_port(ob['host'], ob['port'])}" if ob["host"] else label


def target_label(graph, n):
    kind, t = find(graph, n["target"]) if n["target"] else (None, None)
    if t is None:
        return _t("not connected")
    if kind == "outbound":
        return " ⇢ ".join(chain_names(graph, t["id"]))
    if kind == "router":
        return (_t("IF {name}") if t.get("style") == "if" else _t("Router {name}")).format(name=t["name"])
    return _t("relay → {name}").format(name=t["name"])


# --- compile to runtime proxies ---

def chain_outbound(ib):
    """Outbound that hands traffic to the local inbound ib (relay / connection test bridge)."""
    host = {"0.0.0.0": "127.0.0.1", "::": "::1"}.get(ib["host"], ib["host"])
    t = ib["type"]
    ob = {"type": t, "host": host, "port": ib["port"], "tls_verify": False}
    if t in ("socks5", "http", "https") and ib["auth_enabled"]:
        ob.update(username=ib["username"], password=ib["password"])
    if t == "shadowsocks":
        ob.update(ss_method=ib["ss_method"], ss_password=ib["ss_password"])
    if t == "hysteria2":
        ob.update(hy2_password=ib["hy2_password"])
    return ob


def flatten_router(graph, rid, prefix=(), seen=()):
    """Router rules as a flat list. Returns (routes, default outbound id):
    routes = [(conditions, outbound block id, rule name)], conditions = the match of every router rule on the way
    (all must match). Rules without a target or without any condition are skipped."""
    _, r = find(graph, rid)
    seen = set(seen) | {rid}
    routes = []
    if r.get("switch"):         # manual switch: no condition, everything takes one side
        r = dict(r, rules=[], default=r["rules"][0]["target"] if r.get("switch_on") else r["default"])
    for rule in r["rules"]:
        if not rule["target"] or not rule["match"]:
            continue
        conds = list(prefix) + [rule["match"]]
        kind, t = find(graph, rule["target"])
        if kind == "outbound":
            routes.append((conds, t["id"], rule["name"]))
        elif kind == "router" and t["id"] not in seen:
            sub, sub_default = flatten_router(graph, t["id"], conds, seen)
            routes += [(c, o, f"{rule['name']} › {name}") for c, o, name in sub]
            if sub_default:
                routes.append((conds, sub_default, rule["name"]))
    kind, t = find(graph, r["default"]) if r["default"] else (None, None)
    if kind == "outbound":
        return routes, t["id"]
    if kind == "router" and t["id"] not in seen:
        sub, sub_default = flatten_router(graph, t["id"], prefix, seen)
        return routes + sub, sub_default
    return routes, None


def compile(graph):
    """Runtime proxies (one per inbound block). 'unlinked' = no target, cannot start."""
    from . import config
    graph = without_relays(graph)
    out = []
    for n in graph["inbounds"]:
        kind, t = find(graph, n["target"]) if n["target"] else (None, None)
        routes = []
        if kind == "router":
            flat, default = flatten_router(graph, t["id"])
            routes = [{"match": conds, "outbound": resolve_outbound(graph, oid), "name": name, "block": oid}
                      for conds, oid, name in flat]
            ob = resolve_outbound(graph, default) if default else {"type": "direct"}
            if not default:
                t = None            # no default exit: cannot start
        elif kind == "outbound":
            ob = resolve_outbound(graph, t["id"])
        elif kind == "inbound" and t["inbound"]["type"] not in ("tun", "wireguard"):
            ob = chain_outbound(t["inbound"])
        else:
            ob = {"type": "direct"}
            t = None                # nothing to send to (a TUN card cannot be relayed into)
        p = config.validate_proxy({"id": n["id"], "name": n["name"], "autostart": n["autostart"],
                                   "auto_restart": n.get("auto_restart", False),
                                   "inbound": copy.deepcopy(n["inbound"]), "outbound": ob,
                                   "overrides": copy.deepcopy(n["overrides"])})
        p["outbound"]["via"] = ob.get("via")
        if "members" in ob:
            p["outbound"]["members"] = ob["members"]
        p["routes"] = routes
        p["target"] = n["target"]
        p["target_name"] = target_label(graph, n)
        p["unlinked"] = t is None
        out.append(p)
    return out


# --- building ---

def add_inbound(graph, name, inbound=None, pos=None, target=None, autostart=False):
    from . import config
    n = validate_inbound_node({"id": new_id(), "name": name, "autostart": autostart, "inbound": inbound or {},
                               "target": target, "pos": pos or [0, 0]})
    n["target"] = target
    graph["inbounds"].append(n)
    return n


def add_outbound(graph, outbound=None, name=None, pos=None):
    n = validate_outbound_node({"id": new_id(), "name": name, "outbound": outbound or {}, "pos": pos or [460, 0]})
    graph["outbounds"].append(n)
    return n


def add_router(graph, name=None, pos=None, style="router"):
    n = validate_router_node({"id": new_id(), "name": name, "pos": pos or [230, 0], "style": style,
                              "rules": [{"name": "IF" if style == "if" else _t("Rule {n}").format(n=1), "match": {}}]})
    graph.setdefault("routers", []).append(n)
    return n


def add_relay(graph, name=None, pos=None, lanes=3):
    n = validate_relay_node({"id": new_id(), "name": name, "pos": pos or [230, 0],
                             "inputs": [new_lane("out") for _ in range(lanes)]})
    graph.setdefault("relays", []).append(n)
    return n


def copy_relay(n):
    """Deep copy of relay node n with fresh block and lane ids (the inside wiring kept)."""
    c = copy.deepcopy(n)
    c["id"] = new_id()
    new_out = {}
    for x in c["outputs"]:
        new_out[x["id"]] = x["id"] = new_id()
    for x in c["inputs"]:
        x["id"] = new_id()
        x["out"] = new_out.get(x["out"])
    return c


def set_lane_count(n, side, count):
    """Grow or shrink relay n's 'inputs' or 'outputs' to count lanes (1..MAX_LANES); straight blocks keep both sides equal."""
    count = max(1, min(MAX_LANES, count))
    sides = ("inputs", "outputs") if not n["routed"] else (side,)
    for s in sides:
        key = "out" if s == "inputs" else "target"
        while len(n[s]) < count:
            n[s].append(new_lane(key))
        del n[s][count:]
    out_ids = {o["id"] for o in n["outputs"]}
    for i, x in enumerate(n["inputs"]):
        if not n["routed"]:
            x["out"] = n["outputs"][i]["id"]
            n["outputs"][i]["gap"] = x["gap"]
        elif x["out"] not in out_ids:
            x["out"] = None


def drop_dead_lane_links(graph):
    """Links into pass-through lanes that no longer exist (the block lost lanes) are cut."""
    lanes = set(lane_index(graph)[0])
    blocks = {n["id"] for kind in ("inbounds", "outbounds", "routers") for n in graph.get(kind, [])}

    def alive(t):
        return t if t is None or t in lanes or t in blocks else None
    for n in graph["inbounds"]:
        n["target"] = alive(n["target"])
    for r in graph.get("routers", []):
        r["default"] = alive(r["default"])
        for x in r["rules"]:
            x["target"] = alive(x["target"])
    for r in graph.get("relays", []):
        for x in r["outputs"]:
            x["target"] = alive(x["target"])


def remove(graph, nid):
    kind, gone = find(graph, nid)
    replace = {}                # links into a deleted pass-through's lanes -> where the lanes led (splice)
    if kind == "relay":
        outs = {o["id"]: o["target"] for o in gone["outputs"]}
        own = {x["id"] for x in gone["inputs"]}
        for x in gone["inputs"]:
            t = outs.get(x["out"])
            replace[x["id"]] = t if t not in own else None
    graph["inbounds"] = [n for n in graph["inbounds"] if n["id"] != nid]
    graph["outbounds"] = [n for n in graph["outbounds"] if n["id"] != nid]
    graph["routers"] = [n for n in graph.get("routers", []) if n["id"] != nid]
    graph["relays"] = [n for n in graph.get("relays", []) if n["id"] != nid]

    def fix(t):
        return None if t == nid else replace.get(t, t)
    for r in graph["routers"]:
        for x in r["rules"]:
            x["target"] = fix(x["target"])
        r["default"] = fix(r["default"])
    for r in graph["relays"]:
        for x in r["outputs"]:
            x["target"] = fix(x["target"])
            if x["target"] and lane_loop(graph, x["target"]):
                x["target"] = None
    for n in graph["inbounds"]:
        n["target"] = fix(n["target"])
        if n["target"] == n["id"]:
            n["target"] = None
    for r in graph["routers"]:              # splicing can close a router loop: cut it
        while router_loop(graph, r["id"]):
            key = next(k for k, t in reversed(router_targets(r)) if find(graph, follow(graph, t))[0] == "router")
            if key == "default":
                r["default"] = None
            else:
                next(x for x in r["rules"] if x["id"] == key)["target"] = None
    for n in graph["outbounds"]:
        if n["via"] == nid:
            n["via"] = None
        if nid in n.get("members", ()):
            n["members"].remove(nid)
    if graph["pc"]["inbound"] == nid:
        graph["pc"]["inbound"] = None


def shared_direct(graph):
    """The first plain Direct block (created if missing) - new inbounds connect to it by default."""
    for n in graph["outbounds"]:
        if n["outbound"]["type"] == "direct" and not n["outbound"].get("bind"):
            return n
    return add_outbound(graph, {"type": "direct"}, "Direct", next_free_pos(graph, "outbounds"))


def next_free_pos(graph, kind):
    x = 0 if kind == "inbounds" else 460
    ys = [n["pos"][1] for n in graph[kind]]
    return [x, (max(ys) + 170) if ys else 0]


def from_proxies(proxies):
    """Settings v2 (one inbound + outbound per proxy) -> graph. Identical outbounds become one shared block."""
    g = empty()
    by_key = {}
    for i, p in enumerate(proxies):
        ob = p["outbound"]
        key = json.dumps(ob, sort_keys=True)
        node = by_key.get(key)
        if node is None:
            node = by_key[key] = add_outbound(g, ob, None, [460, len(by_key) * 190])
        add_inbound(g, p["name"], p["inbound"], [0, i * 170], node["id"], p["autostart"])
        g["inbounds"][-1]["id"] = p["id"]
        g["inbounds"][-1]["overrides"] = copy.deepcopy(p["overrides"])
    return g
