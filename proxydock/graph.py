"""Block graph: inbound blocks, outbound blocks and the PC (system proxy) block.

graph = {
  "inbounds":  [{"id", "name", "autostart", "inbound": {...}, "overrides": {...}, "target": id|None, "pos": [x, y]}],
  "outbounds": [{"id", "name", "outbound": {...}, "via": id|None, "pos": [x, y]}],
  "routers":   [{"id", "name", "rules": [{"id", "name", "match": {...}, "target": id|None}], "default": id|None,
                 "pos": [x, y]}],
  "pc":        {"inbound": id|None, "pos": [x, y], "backup": {...}|None, "applied": bool, "server": str},
}

An inbound's target is either an outbound block (traffic leaves through it) or another inbound block
(relay: traffic is handed to that local inbound). Several inbounds can share one outbound block.
An outbound block's "via" is another outbound block its own server connection goes through
(e.g. Shadowsocks via the company HTTP proxy); Direct and Hysteria2 (QUIC/UDP) cannot have a via.
An inbound can also target a router block: its rules (checked top to bottom) send matching traffic to an outbound
block, the Block outbound or another router; everything else goes to the router's default. Routers need the
sing-box engine (they become sing-box route rules).
The runtime still works with one "proxy" per inbound (inbound + resolved outbound + overrides, plus "routes" for a
router: [{"match": [match, ...], "outbound": resolved chain, "name": rule name}]); compile() builds them.
"""
import copy
import json
import uuid

from .common import INBOUND_TYPES, OUTBOUND_TYPES, host_port
from .i18n import _t


NO_VIA = ("direct", "hysteria2", "block")   # Direct/Block end a chain; Hysteria2 uses QUIC (UDP), cannot be tunnelled

# router rule conditions: lists of strings, plus ip_private (bool). Inside one rule, sing-box ORs the address
# conditions (domain*, ip_cidr, ip_private, geoip, geosite) and ANDs the rest (port, process_name).
MATCH_LISTS = ("domain", "domain_suffix", "domain_keyword", "ip_cidr", "port", "process_name", "geoip", "geosite")
LOWER_KEYS = ("domain", "domain_suffix", "domain_keyword", "geoip", "geosite")


def new_id():
    return uuid.uuid4().hex[:8]


def empty():
    return {"inbounds": [], "outbounds": [], "routers": [],
            "pc": {"inbound": None, "pos": [460, -200], "backup": None, "applied": False, "server": ""}}


def _pos(v, default=(0, 0)):
    if isinstance(v, (list, tuple)) and len(v) == 2 and all(isinstance(x, (int, float)) for x in v):
        return [float(v[0]), float(v[1])]
    return list(default)


def _name(v, default):
    return v.strip()[:60] if isinstance(v, str) and v.strip() else default


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
    return {"id": raw.get("id") if isinstance(raw.get("id"), str) and raw.get("id") else new_id(),
            "name": _name(raw.get("name"), default_outbound_name(ob)), "outbound": ob,
            "via": via if isinstance(via, str) and via and ob["type"] not in NO_VIA else None,
            "pos": _pos(raw.get("pos"))}


def validate_inbound_node(raw):
    from . import config
    raw = raw if isinstance(raw, dict) else {}
    p = config.validate_proxy(raw)
    target = raw.get("target")
    return {"id": p["id"], "name": p["name"], "autostart": p["autostart"], "inbound": p["inbound"],
            "overrides": p["overrides"], "target": target if isinstance(target, str) and target else None,
            "pos": _pos(raw.get("pos"))}


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
    return {"id": raw.get("id") if isinstance(raw.get("id"), str) and raw.get("id") else new_id(),
            "name": _name(raw.get("name"), "Router"), "rules": rules,
            "default": d if isinstance(d, str) and d else None, "pos": _pos(raw.get("pos"))}


def router_targets(r):
    """(rule id or "default", target id) for every link out of router r."""
    return [(x["id"], x["target"]) for x in r["rules"] if x["target"]] + ([("default", r["default"])] if r["default"] else [])


def router_loop(graph, rid):
    """True if following router links from router rid comes back to it."""
    routers = {r["id"]: r for r in graph.get("routers", [])}
    stack, seen = ([t for _, t in router_targets(routers[rid])] if rid in routers else []), set()
    while stack:
        cur = stack.pop()
        if cur == rid:
            return True
        if cur in seen or cur not in routers:
            continue
        seen.add(cur)
        stack += [t for _, t in router_targets(routers[cur])]
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
    ids = {n["id"] for n in g["inbounds"]} | {n["id"] for n in g["outbounds"]} | {n["id"] for n in g["routers"]}
    for n in g["inbounds"]:
        if n["target"] not in ids or n["target"] == n["id"]:
            n["target"] = None
    rule_ids = {n["id"] for n in g["outbounds"]} | {n["id"] for n in g["routers"]}
    router_ids = {n["id"] for n in g["routers"]}
    for r in g["routers"]:                  # router links go to outbound blocks or other routers
        for x in r["rules"]:
            if x["target"] not in rule_ids or x["target"] == r["id"]:
                x["target"] = None
        if r["default"] not in rule_ids or r["default"] == r["id"]:
            r["default"] = None
    for r in g["routers"]:                  # a router loop cannot be used: cut it
        while router_loop(g, r["id"]):
            key = next(k for k, t in reversed(router_targets(r)) if t in router_ids)
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
    """('inbound'|'outbound'|'router', node) or (None, None)."""
    for kind in ("inbounds", "outbounds", "routers"):
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
    """Inbound blocks whose target is nid."""
    return [n for n in graph["inbounds"] if n["target"] == nid]


def default_outbound_name(ob):
    if ob["type"] == "block":
        return _t("Block")
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
        return _t("Router {name}").format(name=t["name"])
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
        elif kind == "inbound":
            ob = chain_outbound(t["inbound"])
        else:
            ob = {"type": "direct"}
        p = config.validate_proxy({"id": n["id"], "name": n["name"], "autostart": n["autostart"],
                                   "inbound": copy.deepcopy(n["inbound"]), "outbound": ob,
                                   "overrides": copy.deepcopy(n["overrides"])})
        p["outbound"]["via"] = ob.get("via")
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


def add_router(graph, name=None, pos=None):
    n = validate_router_node({"id": new_id(), "name": name, "pos": pos or [230, 0],
                              "rules": [{"name": _t("Rule {n}").format(n=1), "match": {}}]})
    graph.setdefault("routers", []).append(n)
    return n


def remove(graph, nid):
    graph["inbounds"] = [n for n in graph["inbounds"] if n["id"] != nid]
    graph["outbounds"] = [n for n in graph["outbounds"] if n["id"] != nid]
    graph["routers"] = [n for n in graph.get("routers", []) if n["id"] != nid]
    for r in graph["routers"]:
        for x in r["rules"]:
            if x["target"] == nid:
                x["target"] = None
        if r["default"] == nid:
            r["default"] = None
    for n in graph["inbounds"]:
        if n["target"] == nid:
            n["target"] = None
    for n in graph["outbounds"]:
        if n["via"] == nid:
            n["via"] = None
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
