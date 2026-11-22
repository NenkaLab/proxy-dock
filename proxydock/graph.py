"""Block graph: inbound blocks, outbound blocks and the PC (system proxy) block.

graph = {
  "inbounds":  [{"id", "name", "autostart", "inbound": {...}, "overrides": {...}, "target": id|None, "pos": [x, y]}],
  "outbounds": [{"id", "name", "outbound": {...}, "via": id|None, "pos": [x, y]}],
  "pc":        {"inbound": id|None, "pos": [x, y], "backup": {...}|None, "applied": bool, "server": str},
}

An inbound's target is either an outbound block (traffic leaves through it) or another inbound block
(relay: traffic is handed to that local inbound). Several inbounds can share one outbound block.
An outbound block's "via" is another outbound block its own server connection goes through
(e.g. Shadowsocks via the company HTTP proxy); Direct and Hysteria2 (QUIC/UDP) cannot have a via.
The runtime still works with one "proxy" per inbound (inbound + resolved outbound + overrides); compile() builds them.
"""
import copy
import json
import uuid

from .common import INBOUND_TYPES, OUTBOUND_TYPES, host_port
from .i18n import _t


NO_VIA = ("direct", "hysteria2")     # Direct ends a chain; Hysteria2 uses QUIC (UDP) so it cannot be tunnelled


def new_id():
    return uuid.uuid4().hex[:8]


def empty():
    return {"inbounds": [], "outbounds": [],
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
    ids = {n["id"] for n in g["inbounds"]} | {n["id"] for n in g["outbounds"]}
    for n in g["inbounds"]:
        if n["target"] not in ids or n["target"] == n["id"]:
            n["target"] = None
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
    """('inbound'|'outbound', node) or (None, None)."""
    for kind in ("inbounds", "outbounds"):
        for n in graph[kind]:
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
    if ob["type"] == "direct":
        return "Direct" + (f" ({ob['bind']})" if ob.get("bind") else "")
    label = OUTBOUND_TYPES[ob["type"]]
    return f"{label} {host_port(ob['host'], ob['port'])}" if ob["host"] else label


def target_label(graph, n):
    kind, t = find(graph, n["target"]) if n["target"] else (None, None)
    if t is None:
        return _t("not connected")
    if kind == "outbound":
        return " ⇢ ".join(chain_names(graph, t["id"]))
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


def compile(graph):
    """Runtime proxies (one per inbound block). 'unlinked' = no target, cannot start."""
    from . import config
    out = []
    for n in graph["inbounds"]:
        kind, t = find(graph, n["target"]) if n["target"] else (None, None)
        if kind == "outbound":
            ob = resolve_outbound(graph, t["id"])
        elif kind == "inbound":
            ob = chain_outbound(t["inbound"])
        else:
            ob = {"type": "direct"}
        p = config.validate_proxy({"id": n["id"], "name": n["name"], "autostart": n["autostart"],
                                   "inbound": copy.deepcopy(n["inbound"]), "outbound": ob,
                                   "overrides": copy.deepcopy(n["overrides"])})
        p["outbound"]["via"] = ob.get("via")
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


def remove(graph, nid):
    graph["inbounds"] = [n for n in graph["inbounds"] if n["id"] != nid]
    graph["outbounds"] = [n for n in graph["outbounds"] if n["id"] != nid]
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
