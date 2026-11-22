"""ProxyDock proxy (graph.compile() output) -> sing-box config dict. Pure functions, no I/O (cert paths come in)."""
import ipaddress

from ..common import is_ip_literal, strip_scope

IN_TAG = "in"
BANS_TAG = "bans"

# what each route rule blocks (index in route.rules -> reason code), used to explain "router: match[n] => reject" lines
R_BANNED, R_SELF, R_ALLOW, R_PRIVATE = "banned", "self", "allow", "private"

# is_internal_ip() ranges that sing-box's ip_is_private does not cover
_EXTRA_INTERNAL = ["0.0.0.0/8", "100.64.0.0/10", "192.0.0.0/24", "198.18.0.0/15", "224.0.0.0/4", "240.0.0.0/4",
                   "ff00::/8", "64:ff9b:1::/48", "2001:db8::/32"]


def _listen(host):
    return "127.0.0.1" if host.lower() == "localhost" else host


def inbound(ib, cert=None):
    """cert: (cert_path, key_path) for https / hysteria2."""
    base = {"tag": IN_TAG, "listen": _listen(ib["host"]), "listen_port": ib["port"]}
    t = ib["type"]
    if t in ("socks5", "socks4"):
        d = {"type": "socks", **base}
        if t == "socks5" and ib["auth_enabled"]:
            d["users"] = [{"username": ib["username"], "password": ib["password"]}]
        return d
    if t in ("http", "https"):
        d = {"type": "http", **base}
        if ib["auth_enabled"]:
            d["users"] = [{"username": ib["username"], "password": ib["password"]}]
        if t == "https":
            d["tls"] = {"enabled": True, "certificate_path": cert[0], "key_path": cert[1]}
        return d
    if t == "shadowsocks":
        return {"type": "shadowsocks", **base, "method": ib["ss_method"], "password": ib["ss_password"]}
    if t == "hysteria2":
        return {"type": "hysteria2", **base, "users": [{"password": ib["hy2_password"]}],
                "tls": {"enabled": True, "certificate_path": cert[0], "key_path": cert[1]}}
    raise ValueError(f"unsupported inbound {t}")


def _tls(ob):
    return {"enabled": True, "server_name": ob.get("sni") or ob["host"], "insecure": not ob.get("tls_verify", True)}


def outbound(ob, tag):
    t = ob["type"]
    if t == "direct":
        d = {"type": "direct", "tag": tag}
        bind = ob.get("bind") or ""
        if bind and is_ip_literal(bind):
            d["inet6_bind_address" if ":" in bind else "inet4_bind_address"] = strip_scope(bind)
        return d
    srv = {"tag": tag, "server": ob["host"], "server_port": ob["port"]}
    if t in ("http", "https"):
        d = {"type": "http", **srv}
        if ob.get("username"):
            d.update(username=ob["username"], password=ob.get("password", ""))
        if t == "https":
            d["tls"] = _tls(ob)
        return d
    if t == "socks5":
        d = {"type": "socks", **srv, "version": "5"}
        if ob.get("username"):
            d.update(username=ob["username"], password=ob.get("password", ""))
        return d
    if t == "socks4":
        d = {"type": "socks", **srv, "version": "4a"}
        if ob.get("username"):
            d["username"] = ob["username"]
        return d
    if t == "shadowsocks":
        return {"type": "shadowsocks", **srv, "method": ob["ss_method"], "password": ob["ss_password"]}
    if t == "hysteria2":
        return {"type": "hysteria2", **srv, "password": ob["hy2_password"], "tls": _tls(ob)}
    raise ValueError(f"unsupported outbound {t}")


def outbounds(ob):
    """The via chain as sing-box outbounds: out0 (the card's outbound) -detour-> out1 -> ..."""
    out, i = [], 0
    while ob:
        d = outbound(ob, f"out{i}")
        if ob.get("via"):
            d["detour"] = f"out{i + 1}"
        out.append(d)
        ob, i = ob.get("via"), i + 1
    return out


def _allow_rule(domains):
    exact, suffix, cidr = [], [], []
    for d in (x.lower().rstrip(".") for x in domains):
        if d.startswith("*."):
            suffix.append(d[2:])
        elif is_ip_literal(d):
            a = ipaddress.ip_address(strip_scope(d))
            cidr.append(f"{a}/{a.max_prefixlen}")
        else:
            exact.append(d)
    rule = {"invert": True, "action": "reject"}
    if exact:
        rule["domain"] = exact
    if suffix:
        rule["domain_suffix"] = suffix
    if cidr:
        rule["ip_cidr"] = cidr
    return rule


def route(proxy, cfg, bans_path):
    """(route dict, reasons) - reasons[i] says what rule i blocks (None for rules that do not reject)."""
    ib, ob = proxy["inbound"], proxy["outbound"]
    rules, reasons = [], []

    def add(rule, reason=None):
        rules.append(rule)
        reasons.append(reason)

    add({"action": "route-options", "udp_timeout": f"{cfg['timeout']}s"})
    add({"rule_set": BANS_TAG, "action": "reject"}, R_BANNED)
    # never connect to this proxy's own port (would loop forever)
    add({"domain": ["localhost"], "ip_cidr": ["127.0.0.0/8", "::1/128"] + (
        [f"{ib['host']}/32"] if is_ip_literal(ib["host"]) and ":" not in ib["host"] and ib["host"] != "0.0.0.0" else []),
         "port": [ib["port"]], "action": "reject"}, R_SELF)
    doms = cfg["allowed_domains"]
    if "*" not in doms:
        add(_allow_rule(doms), R_ALLOW)
    if cfg["block_private"]:
        if ob["type"] == "direct":
            # Direct resolves here, so check the resolved address too. Through an upstream/tunnel the upstream
            # resolves domains, so (as in the Python engine) only literal IPs and localhost are checked.
            add({"action": "resolve"})
        add({"domain_suffix": ["localhost"], "action": "reject"}, R_PRIVATE)
        add({"ip_is_private": True, "action": "reject"}, R_PRIVATE)
        add({"ip_cidr": _EXTRA_INTERNAL, "action": "reject"}, R_PRIVATE)
    r = {"rule_set": [{"type": "local", "tag": BANS_TAG, "format": "source", "path": bans_path}],
         "rules": rules, "final": "out0"}
    return r, reasons


def build(proxy, cfg, *, bans_path, api_port, api_secret, cert=None):
    """Full sing-box config. cfg = config.resolve(proxy, glob). Returns (config dict, route rule reasons)."""
    r, reasons = route(proxy, cfg, bans_path)
    conf = {
        "log": {"level": "debug", "timestamp": False},      # colors are turned off with --disable-color
        "inbounds": [inbound(proxy["inbound"], cert)],
        "outbounds": outbounds(proxy["outbound"]),
        "route": r,
        "experimental": {"clash_api": {"external_controller": f"127.0.0.1:{api_port}", "secret": api_secret}},
    }
    return conf, reasons


def bans_ruleset(ips):
    """Contents of the shared local rule-set file with the banned client IPs."""
    cidrs = []
    for ip in sorted(ips):
        a = ipaddress.ip_address(strip_scope(ip))
        cidrs.append(f"{a}/{a.max_prefixlen}")
    # a rule-set rule needs at least one item; 255.255.255.255 is never a client address
    return {"version": 3, "rules": [{"source_ip_cidr": cidrs or ["255.255.255.255/32"]}]}
