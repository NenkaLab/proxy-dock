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
    if t == "wireguard":       # a sing-box endpoint that other devices connect to (goes into "endpoints")
        from ..config import wg_peers
        return {"type": "wireguard", "tag": IN_TAG, "listen_port": ib["port"], "address": [ib["wg_address"].strip()],
                "private_key": ib["wg_private_key"].strip(), "mtu": ib.get("wg_mtu") or 1408,
                "peers": [{"public_key": x["public_key"].strip(), "allowed_ips": [x["address"]]} for x in wg_peers(ib)]}
    if t == "tun":
        return {"type": "tun", "tag": IN_TAG, "address": TUN_ADDRESS, "mtu": 9000, "auto_route": True,
                "strict_route": bool(ib.get("tun_strict_route")), "stack": ib.get("tun_stack", "mixed")}
    base = {"tag": IN_TAG, "listen": _listen(ib["host"]), "listen_port": ib["port"]}
    if t == "forward":          # sing-box direct inbound: every connection goes to the fixed destination
        d = {"type": "direct", **base, "override_address": ib["fwd_host"].strip().strip("[]"),
             "override_port": ib["fwd_port"]}
        if ib.get("fwd_network", "tcp") in ("tcp", "udp"):
            d["network"] = ib["fwd_network"]
        return d
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
        d = {"type": "hysteria2", **base, "users": [{"password": ib["hy2_password"]}],
             "tls": {"enabled": True, "certificate_path": cert[0], "key_path": cert[1]}}
        if ib.get("hy2_obfs_password"):
            d["obfs"] = {"type": "salamander", "password": ib["hy2_obfs_password"]}
        if ib.get("hy2_up_mbps"):
            d["up_mbps"] = ib["hy2_up_mbps"]
        if ib.get("hy2_down_mbps"):
            d["down_mbps"] = ib["hy2_down_mbps"]
        return d
    raise ValueError(f"unsupported inbound {t}")


def _tls(ob):
    t = {"enabled": True, "server_name": ob.get("sni") or ob["host"], "insecure": not ob.get("tls_verify", True)}
    if ob.get("pin_sha256") and ob.get("type") in ("https", "hysteria2"):
        t["certificate_sha256"] = [pin_base64(ob["pin_sha256"])]     # sing-box 1.15+ (engine_for checks)
    return t


def pin_base64(pin):
    """pin_sha256 (hex SHA-256 of the whole DER certificate, colons allowed) as base64, for certificate_sha256."""
    import base64
    h = pin.replace(":", "").lower()
    try:
        if len(h) != 64:
            raise ValueError
        return base64.b64encode(bytes.fromhex(h)).decode()
    except ValueError:
        raise ValueError(f"invalid certificate fingerprint {pin}") from None


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
        d = {"type": "hysteria2", **srv, "password": ob["hy2_password"], "tls": _tls(ob)}
        if ob.get("hy2_obfs_password"):
            d["obfs"] = {"type": "salamander", "password": ob["hy2_obfs_password"]}
        if ob.get("hy2_up_mbps"):           # Brutal (both needed; 0 = BBR)
            d["up_mbps"] = ob["hy2_up_mbps"]
        if ob.get("hy2_down_mbps"):
            d["down_mbps"] = ob["hy2_down_mbps"]
        if (ob.get("hy2_ports") or "").strip():
            from ..config import hop_ports
            ports = hop_ports(ob["hy2_ports"])
            if ports:
                d["server_ports"] = ports
                d["hop_interval"] = f"{ob.get('hy2_hop_interval') or 30}s"
        return d
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
    if t == "vmess":
        d = {"type": "vmess", **srv, "uuid": ob["uuid"].strip(), "security": ob.get("vmess_cipher") or "auto",
             "alter_id": ob.get("vmess_alter_id", 0), "packet_encoding": "xudp"}
        if ob.get("security", "none") == "tls":
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
        if ob["type"] == "subscription":
            out += subscription_outbounds(ob, f"{prefix}{i}")
            break
        if ob["type"] == "group":
            out += group_outbounds(ob, f"{prefix}{i}")
            break
        d = outbound(ob, f"{prefix}{i}")
        if ob.get("via"):
            d["detour"] = f"{prefix}{i + 1}"
        elif ob.get("interface"):
            d["bind_interface"] = ob["interface"]
        out.append(d)
        ob, i = ob.get("via"), i + 1
    return out


def subscription_outbounds(ob, tag):
    """A subscription block: one outbound per server plus a group with the block's tag
    (selector = the chosen server, urltest = the fastest one, switching away from dead servers)."""
    from ..subscription import servers_of
    members, tags = [], []
    for i, s in enumerate(servers_of(ob)):
        m = dict(s["outbound"], via=None)
        try:
            d = outbound(m, f"{tag}_m{i}")
        except (ValueError, KeyError):
            continue
        if ob.get("interface"):
            d["bind_interface"] = ob["interface"]
        members.append(d)
        tags.append((s["name"], d["tag"]))
    if not tags:
        return [{"type": "block", "tag": tag}]    # (cannot happen after validation)
    names = [n for n, _ in tags]
    if ob.get("sub_mode") == "select":
        chosen = tags[names.index(ob["sub_selected"])][1] if ob.get("sub_selected") in names else tags[0][1]
        group = {"type": "selector", "tag": tag, "outbounds": [t for _, t in tags], "default": chosen}
    else:
        group = {"type": "urltest", "tag": tag, "outbounds": [t for _, t in tags],
                 "url": ob.get("sub_test_url") or "https://www.gstatic.com/generate_204",
                 "interval": "3m", "tolerance": 50}
    return members + [group]


FAILOVER_TOLERANCE = 30000     # ms: urltest never trades the current member for a faster one -> member order wins


def group_outbounds(ob, tag):
    """A group block: every member's chain (first hop <tag>_m<k>_0) plus a urltest with the block's tag.
    auto = the fastest member; failover = the first member in order that answers: with a tolerance larger than any
    measured delay urltest keeps the first working member it finds, and leaves it only when it stops answering
    (sing-box 1.14 urltest.Select; see the version notes)."""
    out, heads = [], []
    for k, m in enumerate(ob.get("members") or []):
        chain = outbounds(m["outbound"], f"{tag}_m{k}_")
        if chain:
            out += chain
            heads.append(f"{tag}_m{k}_0")
    if not heads:
        raise ValueError("group without members")
    if ob.get("group_mode") == "select":
        ids = [m.get("id") for m in ob.get("members") or [] if outbounds(m["outbound"], "x")]
        chosen = heads[ids.index(ob["group_selected"])] if ob.get("group_selected") in ids else heads[0]
        return out + [{"type": "selector", "tag": tag, "outbounds": heads, "default": chosen,
                       "interrupt_exist_connections": True}]
    failover = ob.get("group_mode") == "failover"
    group = {"type": "urltest", "tag": tag, "outbounds": heads,
             "url": ob.get("group_test_url") or "https://www.gstatic.com/generate_204",
             "interval": f"{max(10, ob.get('group_interval') or 180)}s",
             "tolerance": FAILOVER_TOLERANCE if failover else max(0, ob.get("group_tolerance", 50)),
             "interrupt_exist_connections": True}
    return out + [group]


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


DNS_TAG, LOCAL_DNS, FAKEIP_TAG = "dns", "local", "fakeip"
FAKEIP_V4, FAKEIP_V6 = "198.18.0.0/15", "fc00::/18"


def dns(proxy, cfg):
    """(sing-box dns section or None, tag of the server used for real lookups).
    The PC's DNS ("local") unless a DNS server is set; through the card's outbound if dns_via_outbound (a tunnel
    or upstream, not Direct/Block). A TUN card with Fake-IP answers apps' A/AAAA queries with fake addresses."""
    from ..config import dns_server_spec
    spec = dns_server_spec(cfg.get("dns_server", "")) or {}
    tun = proxy["inbound"]["type"] == "tun"
    fakeip = tun and proxy["inbound"].get("tun_fakeip")
    if not spec and not tun:
        return None, None
    servers = [{"type": "local", "tag": LOCAL_DNS}]
    real = LOCAL_DNS
    if spec:
        d = dict(spec, tag=DNS_TAG)
        if not is_ip_literal(d["server"]):
            d["domain_resolver"] = LOCAL_DNS        # the DNS server's own name: looked up with the PC's DNS
        if cfg.get("dns_via_outbound") and proxy["outbound"]["type"] not in ("direct", "block"):
            d["detour"] = "out0"
        servers.append(d)
        real = DNS_TAG
    conf = {"servers": servers, "final": real}
    if fakeip:
        servers.append({"type": "fakeip", "tag": FAKEIP_TAG, "inet4_range": FAKEIP_V4, "inet6_range": FAKEIP_V6})
        conf["rules"] = [{"query_type": ["A", "AAAA"], "server": FAKEIP_TAG}]
    return conf, real


def route(proxy, cfg, bans_path, rulesets=None, route_tags=None, exclude_paths=None):
    """(route dict, reasons) - reasons[i] says what rule i blocks (None for rules that do not reject).
    rulesets: {tag: path} for GeoIP/Geosite; route_tags: block id -> outbound tag prefix of router targets."""
    ib, ob = proxy["inbound"], proxy["outbound"]
    rules, reasons = [], []
    _, resolver = dns(proxy, cfg)
    resolve = {"action": "resolve", "server": resolver} if resolver else {"action": "resolve"}
    fakeip = ib["type"] == "tun" and ib.get("tun_fakeip")

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
    if not tun and ib["type"] != "wireguard":     # never connect to this proxy's own port (would loop forever)
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
            add(dict(resolve))
        add({"domain_suffix": ["localhost"], "action": "reject"}, R_PRIVATE)
        add({"ip_is_private": True, "action": "reject"}, R_PRIVATE)
        add({"ip_cidr": [x for x in _EXTRA_INTERNAL if not (fakeip and x == FAKEIP_V4)], "action": "reject"}, R_PRIVATE)
    routes = proxy.get("routes") or []
    if any(k in m for r in routes for m in r["match"] for k in IP_KEYS):
        # IP / GeoIP conditions need the destination address. Note: after this the outbound connects to the IP,
        # so a tunnel no longer resolves the domain remotely.
        add(dict(resolve))
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
    if resolver:
        r["default_domain_resolver"] = resolver
    return r, reasons


def route_exit_tags(proxy):
    """{router target block id: its outbound tag prefix} (one chain per target block, shared by its rules)."""
    tags = {}
    for r in proxy.get("routes") or []:
        if r["outbound"]["type"] != "block" and r["block"] not in tags:
            tags[r["block"]] = f"r{len(tags)}_"
    return tags


def exit_of_tag(proxy, tag):
    """Outbound block id a connection leaving through outbound tag (sing-box 'chains'[0]) belongs to, or None."""
    for block, prefix in route_exit_tags(proxy).items():
        if tag.startswith(prefix):
            return block
    return proxy.get("exit") if tag.startswith("out") else None


def build(proxy, cfg, *, bans_path, api_port, api_secret, cert=None, rulesets=None, exclude_paths=None):
    """Full sing-box config. cfg = config.resolve(proxy, glob). Returns (config dict, route rule reasons)."""
    obs = outbounds(proxy["outbound"]) or [{"type": "direct", "tag": "out0"}]   # (Block default: never used)
    route_tags = route_exit_tags(proxy)     # one outbound chain per target block, shared by its rules
    for r in proxy.get("routes") or []:
        if r["outbound"]["type"] != "block" and route_tags.get(r["block"]) and \
                not any(o["tag"].startswith(route_tags[r["block"]]) for o in obs):
            obs += outbounds(r["outbound"], route_tags[r["block"]])
    tun = proxy["inbound"]["type"] == "tun"
    if tun:
        obs.append({"type": "direct", "tag": TUN_DIRECT})
    r, reasons = route(proxy, cfg, bans_path, rulesets, route_tags, exclude_paths)
    endpoints = [o for o in obs if o["type"] == "wireguard"]
    ib_conf = inbound(proxy["inbound"], cert)
    if ib_conf["type"] == "wireguard":
        endpoints.insert(0, ib_conf)
    conf = {
        "log": {"level": "debug", "timestamp": False},      # colors are turned off with --disable-color
        "inbounds": [ib_conf] if ib_conf["type"] != "wireguard" else [],
        "outbounds": [o for o in obs if o["type"] != "wireguard"] or [{"type": "direct", "tag": "direct"}],
        "route": r,
        "experimental": {"clash_api": {"external_controller": f"127.0.0.1:{api_port}", "secret": api_secret}},
    }
    if endpoints:
        conf["endpoints"] = endpoints
    dns_conf, _ = dns(proxy, cfg)
    if dns_conf:
        conf["dns"] = dns_conf
    return conf, reasons


def bans_ruleset(ips):
    """Contents of the shared local rule-set file with the banned client IPs."""
    cidrs = []
    for ip in sorted(ips):
        a = ipaddress.ip_address(strip_scope(ip))
        cidrs.append(f"{a}/{a.max_prefixlen}")
    # a rule-set rule needs at least one item; 255.255.255.255 is never a client address
    return {"version": 3, "rules": [{"source_ip_cidr": cidrs or ["255.255.255.255/32"]}]}
