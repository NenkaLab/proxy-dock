"""ProxyDock proxy (graph.compile() output) -> sing-box config dict. Pure functions, no I/O (cert paths come in)."""
import ipaddress

from ..common import is_ip_literal, strip_scope

IN_TAG = "in"
BANS_TAG = "bans"

# what each route rule blocks (index in route.rules -> reason code), used to explain "router: match[n] => reject" lines
R_BANNED, R_SELF, R_ALLOW, R_PRIVATE = "banned", "self", "allow", "private"
# a router rule that sends traffic to the Block outbound: reasons[i] = ("rule", rule name)
R_RULE = "rule"
IP_KEYS = ("ip_cidr", "ip_private", "geoip")      # router conditions that need the destination IP

# is_internal_ip() ranges that sing-box's ip_is_private does not cover
_EXTRA_INTERNAL = ["0.0.0.0/8", "100.64.0.0/10", "192.0.0.0/24", "198.18.0.0/15", "224.0.0.0/4", "240.0.0.0/4",
                   "ff00::/8", "64:ff9b:1::/48", "2001:db8::/32"]


def _listen(host):
    return "127.0.0.1" if host.lower() == "localhost" else host


TUN_ADDRESS = ["172.19.0.1/30", "fdfe:dcba:9876::1/126"]
TUN_DIRECT = "tun-direct"           # a TUN card's own Direct: ProxyDock's engines leave through it, not the TUN


def inbound(ib, cert=None):
    """cert: (cert_path, key_path) for https / hysteria2."""
    t = ib["type"]
    if t == "tun":
        return {"type": "tun", "tag": IN_TAG, "address": TUN_ADDRESS, "mtu": 9000, "auto_route": True,
                "strict_route": bool(ib.get("tun_strict_route")), "stack": ib.get("tun_stack", "mixed")}
    base = {"tag": IN_TAG, "listen": _listen(ib["host"]), "listen_port": ib["port"]}
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


def _tls_full(ob, default_alpn=None):
    """TLS for VLESS / Trojan / TUIC: SNI, verification, ALPN, uTLS fingerprint, REALITY."""
    from ..wgconf import split
    t = _tls(ob)
    alpn = split(ob.get("alpn", "")) or default_alpn
    if alpn:
        t["alpn"] = alpn
    if ob.get("fingerprint") and ob["type"] != "tuic":      # (uTLS does not apply to QUIC)
        t["utls"] = {"enabled": True, "fingerprint": ob["fingerprint"]}
    if ob.get("security") == "reality" and ob["type"] == "vless":
        t["reality"] = {"enabled": True, "public_key": ob["reality_public_key"].strip(),
                        "short_id": ob.get("reality_short_id", "").strip()}
        t.pop("insecure", None)
    return t


def _transport(ob):
    t = ob.get("transport", "tcp")
    if t == "ws":
        d = {"type": "ws", "path": ob.get("transport_path") or "/"}
        if ob.get("transport_host"):
            d["headers"] = {"Host": ob["transport_host"]}
        return d
    if t == "grpc":
        return {"type": "grpc", "service_name": ob.get("transport_path", "")}
    return None


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
    if t == "vless":
        d = {"type": "vless", **srv, "uuid": ob["uuid"].strip(), "packet_encoding": "xudp"}
        if ob.get("vless_flow"):
            d["flow"] = ob["vless_flow"]
        if ob.get("security", "tls") != "none":
            d["tls"] = _tls_full(ob)
        tr = _transport(ob)
        if tr:
            d["transport"] = tr
        return d
    if t == "trojan":
        d = {"type": "trojan", **srv, "password": ob["password"], "tls": _tls_full(ob)}
        tr = _transport(ob)
        if tr:
            d["transport"] = tr
        return d
    if t == "tuic":
        return {"type": "tuic", **srv, "uuid": ob["uuid"].strip(), "password": ob["password"],
                "congestion_control": ob.get("tuic_congestion") or "bbr",
                "udp_relay_mode": ob.get("tuic_udp_relay") or "native", "tls": _tls_full(ob, ["h3"])}
    if t == "wireguard":            # a sing-box endpoint (goes into "endpoints", used like an outbound)
        from ..wgconf import split
        peer = {"address": ob["host"], "port": ob["port"], "public_key": ob["wg_peer_public_key"].strip(),
                "allowed_ips": split(ob["wg_allowed_ips"])}
        if ob.get("wg_preshared_key"):
            peer["pre_shared_key"] = ob["wg_preshared_key"].strip()
        if ob.get("wg_keepalive"):
            peer["persistent_keepalive_interval"] = ob["wg_keepalive"]
        return {"type": "wireguard", "tag": tag, "address": split(ob["wg_address"]),
                "private_key": ob["wg_private_key"].strip(), "mtu": ob.get("wg_mtu") or 1408, "peers": [peer]}
    raise ValueError(f"unsupported outbound {t}")


def outbounds(ob, prefix="out"):
    """The via chain as sing-box outbounds: out0 (the card's outbound) -detour-> out1 -> ...
    A Block outbound has no sing-box outbound (it becomes a reject rule). A network interface applies to the hop
    that dials out itself (the end of the chain)."""
    out, i = [], 0
    while ob and ob["type"] != "block":
        d = outbound(ob, f"{prefix}{i}")
        if ob.get("via"):
            d["detour"] = f"{prefix}{i + 1}"
        elif ob.get("interface"):
            d["bind_interface"] = ob["interface"]
        out.append(d)
        ob, i = ob.get("via"), i + 1
    return out


def _cidr(x):
    if "/" in x:
        return str(ipaddress.ip_network(x, strict=False))
    a = ipaddress.ip_address(strip_scope(x))
    return f"{a}/{a.max_prefixlen}"


def match_fields(m):
    """One router rule's conditions -> sing-box rule fields (without the action)."""
    from .rulesets import tag
    d = {}
    if m.get("domain"):
        d["domain"] = [x.rstrip(".") for x in m["domain"]]
    if m.get("domain_suffix"):
        d["domain_suffix"] = [x.lstrip("*").lstrip(".").rstrip(".") for x in m["domain_suffix"]]
    if m.get("domain_keyword"):
        d["domain_keyword"] = list(m["domain_keyword"])
    if m.get("ip_cidr"):
        d["ip_cidr"] = [_cidr(x) for x in m["ip_cidr"]]
    if m.get("ip_private"):
        d["ip_is_private"] = True
    sets = [tag("geoip", x) for x in m.get("geoip", [])] + [tag("geosite", x) for x in m.get("geosite", [])]
    if sets:
        d["rule_set"] = sets
    ports, ranges = [], []
    for x in m.get("port", []):
        if "-" in x:
            a, b = (int(v) for v in x.split("-", 1))
            ranges.append(f"{a}:{b}")
        else:
            ports.append(int(x))
    if ports:
        d["port"] = ports
    if ranges:
        d["port_range"] = ranges
    if m.get("process_name"):
        d["process_name"] = list(m["process_name"])
    return d


def route_rule(conds):
    """Conditions of every router rule on the way (all must match)."""
    if len(conds) == 1:
        return match_fields(conds[0])
    return {"type": "logical", "mode": "and", "rules": [match_fields(m) for m in conds]}


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


def route(proxy, cfg, bans_path, rulesets=None, route_tags=None, exclude_paths=None):
    """(route dict, reasons) - reasons[i] says what rule i blocks (None for rules that do not reject).
    rulesets: {tag: path} for GeoIP/Geosite; route_tags: block id -> outbound tag prefix of router targets."""
    ib, ob = proxy["inbound"], proxy["outbound"]
    rules, reasons = [], []

    def add(rule, reason=None):
        rules.append(rule)
        reasons.append(reason)

    tun = ib["type"] == "tun"
    if tun:
        # apps connect by IP: sniff TLS/HTTP/QUIC for the domain so domain rules work; answer DNS here
        add({"action": "sniff"})
        add({"protocol": "dns", "action": "hijack-dns"})
        if exclude_paths:       # ProxyDock's own engines (other cards, tunnels) must not loop back into the TUN
            add({"process_path": list(exclude_paths), "action": "route", "outbound": TUN_DIRECT})
    add({"action": "route-options", "udp_timeout": f"{cfg['timeout']}s"})
    add({"rule_set": BANS_TAG, "action": "reject"}, R_BANNED)
    if not tun:     # never connect to this proxy's own port (would loop forever)
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
    routes = proxy.get("routes") or []
    if any(k in m for r in routes for m in r["match"] for k in IP_KEYS):
        # IP / GeoIP conditions need the destination address. Note: after this the outbound connects to the IP,
        # so a tunnel no longer resolves the domain remotely.
        add({"action": "resolve"})
    for r in routes:
        rule = route_rule(r["match"])
        if r["outbound"]["type"] == "block":
            add(dict(rule, action="reject"), (R_RULE, r["name"]))
        else:
            add(dict(rule, action="route", outbound=f"{route_tags[r['block']]}0"))
    if ob["type"] == "block":          # the router's default is Block: refuse everything else
        add({"action": "reject"}, (R_RULE, "default"))
    sets = [{"type": "local", "tag": BANS_TAG, "format": "source", "path": bans_path}]
    sets += [{"type": "local", "tag": t, "format": "binary", "path": p} for t, p in sorted((rulesets or {}).items())]
    r = {"rule_set": sets, "rules": rules, "final": "out0"}
    if tun:
        r["auto_detect_interface"] = True       # outbounds leave through the real network card, not the TUN
        r["default_domain_resolver"] = "local"
    return r, reasons


def build(proxy, cfg, *, bans_path, api_port, api_secret, cert=None, rulesets=None, exclude_paths=None):
    """Full sing-box config. cfg = config.resolve(proxy, glob). Returns (config dict, route rule reasons)."""
    obs = outbounds(proxy["outbound"]) or [{"type": "direct", "tag": "out0"}]   # (Block default: never used)
    route_tags = {}
    for r in proxy.get("routes") or []:     # one outbound chain per target block, shared by its rules
        if r["outbound"]["type"] != "block" and r["block"] not in route_tags:
            route_tags[r["block"]] = prefix = f"r{len(route_tags)}_"
            obs += outbounds(r["outbound"], prefix)
    tun = proxy["inbound"]["type"] == "tun"
    if tun:
        obs.append({"type": "direct", "tag": TUN_DIRECT})
    r, reasons = route(proxy, cfg, bans_path, rulesets, route_tags, exclude_paths)
    endpoints = [o for o in obs if o["type"] == "wireguard"]
    conf = {
        "log": {"level": "debug", "timestamp": False},      # colors are turned off with --disable-color
        "inbounds": [inbound(proxy["inbound"], cert)],
        "outbounds": [o for o in obs if o["type"] != "wireguard"] or [{"type": "direct", "tag": "direct"}],
        "route": r,
        "experimental": {"clash_api": {"external_controller": f"127.0.0.1:{api_port}", "secret": api_secret}},
    }
    if endpoints:
        conf["endpoints"] = endpoints
    if tun:
        conf["dns"] = {"servers": [{"type": "local", "tag": "local"}]}
    return conf, reasons


def bans_ruleset(ips):
    """Contents of the shared local rule-set file with the banned client IPs."""
    cidrs = []
    for ip in sorted(ips):
        a = ipaddress.ip_address(strip_scope(ip))
        cidrs.append(f"{a}/{a.max_prefixlen}")
    # a rule-set rule needs at least one item; 255.255.255.255 is never a client address
    return {"version": 3, "rules": [{"source_ip_cidr": cidrs or ["255.255.255.255/32"]}]}
