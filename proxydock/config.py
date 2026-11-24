"""Settings: paths, v2 format (global + proxy list), validation, password storage, migration from the
previous version, global/local merge.

config.json
{ "version": 2,
  "global":  { app settings + proxy defaults },
  "proxies": [ { "id", "name", "autostart", "inbound": {...}, "outbound": {...}, "overrides": {...} } ] }

Effective proxy setting = the value in overrides if set (local), otherwise the global default (null).
"""
from .i18n import _t, on_language_change
import base64
import copy
import ctypes
import ipaddress
import json
import os
import shutil
import uuid

from . import graph as _graph

from .common import (
    APP_ID, INBOUND_TYPES, IS_MAC, IS_WIN, LOG_INFO, LOG_WARN, OLD_APP_ID, OUTBOUND_TYPES, log,
)


def app_dir(name=APP_ID):
    home = os.path.expanduser("~")
    if IS_WIN:
        base = os.environ.get("APPDATA") or home
    elif IS_MAC:
        base = os.path.join(home, "Library", "Application Support")
    else:
        base = os.environ.get("XDG_CONFIG_HOME") or os.path.join(home, ".config")
    return os.path.join(base, name)


APP_DIR = app_dir()
CONFIG_PATH = os.path.join(APP_DIR, "config.json")
LOG_DIR = os.path.join(APP_DIR, "logs")
CERT_DIR = os.path.join(APP_DIR, "certs")
WINDOW_STATE_PATH = os.path.join(APP_DIR, "window.json")

THEMES = []


@on_language_change
def _tr_themes():
    THEMES[:] = [("auto", _t("System")), ("light", _t("Light")), ("dark", _t("Dark"))]


UI_SCALES = [90, 100, 110, 125, 150]
TEXT_SIZES = [100, 115, 130, 150]       # % of the Fluent widgets' own text sizes (applied at once)
ENGINES = ("sing-box", "python")

GLOBAL_DEFAULTS = {
    # app only (global only)
    "language": "en",           # i18n.LANGUAGES code (applies after restart)
    "theme": "auto",
    "ui_scale": 100,
    "text_size": 115,
    "ui_font": "",              # "" = built-in Pretendard
    "mono_font": "",            # "" = built-in JetBrains Mono
    "disable_ligatures": False,
    "log_font_size": 10,
    "close_exits": False,
    "log_to_file": False,
    "engine": "sing-box",       # "sing-box" (one sing-box process per card) or "python" (built-in engine)
    "run_as_admin": False,      # Windows: restart ProxyDock elevated (UAC) at startup, so TUN cards need no prompt
    "singbox_path": "",         # "" = bundled sing-box (bin/ folder), then PATH
    "hysteria_path": "",        # "" = bundled official Hysteria2 core (bin/ folder), then PATH
    "singbox_latest": False,    # experimental (Rust app): run the newest sing-box release, alpha / beta included
    "test_sites": [],           # sites the user added to the connection test (host names)
    "check_updates": True,      # ask GitHub for a newer release at startup (at most once a day, update.py)
    "update_checked": 0,        # time.time() of the last update check
    # proxy defaults (a proxy's local settings take precedence)
    "allowed_domains": ["*"],
    "block_private": False,
    "timeout": 120,
    "log_connections": True,
    "log_close": False,
    "dns_server": "",           # "" = the PC's DNS; else 1.1.1.1, tcp://, tls://, https://host/path, quic://, h3:// (sing-box)
    "dns_via_outbound": False,  # send this card's own DNS lookups through its outbound (custom server only)
}
OVERRIDABLE = ("allowed_domains", "block_private", "timeout", "log_connections", "log_close", "dns_server",
               "dns_via_outbound")
VMESS_CIPHERS = ("auto", "aes-128-gcm", "chacha20-poly1305", "none", "zero")
DNS_PRESETS = ["", "https://cloudflare-dns.com/dns-query", "https://dns.google/dns-query", "https://dns.quad9.net/dns-query",
               "tls://1.1.1.1", "1.1.1.1"]
DNS_DEFAULT_PORTS = {"udp": 53, "tcp": 53, "tls": 853, "https": 443, "quic": 853, "h3": 443}


def dns_server_spec(text):
    """'1.1.1.1', 'tls://dns.google', 'https://cloudflare-dns.com/dns-query', ... -> sing-box DNS server fields
    ({} = the PC's DNS, None = not valid)."""
    t = (text or "").strip()
    if not t:
        return {}
    scheme, sep, rest = t.partition("://")
    if not sep:
        scheme, rest = "udp", t
    scheme = scheme.lower()
    if scheme not in DNS_DEFAULT_PORTS:
        return None
    hostport, slash, path = rest.partition("/")
    try:
        from .common import split_host_port
        host, port = split_host_port(hostport, DNS_DEFAULT_PORTS[scheme])
    except ValueError:
        return None
    host = host.strip().lower()
    from .common import is_ip_literal, valid_domain
    if not host or not (is_ip_literal(host) or valid_domain(host)) or any(c in host for c in "?#@"):
        return None
    d = {"type": scheme, "server": host}
    if port != DNS_DEFAULT_PORTS[scheme]:
        d["server_port"] = port
    if scheme in ("https", "h3"):
        d["path"] = "/" + path if slash and path else "/dns-query"
    elif slash and path:
        return None
    return d

INBOUND_DEFAULTS = {
    "type": "socks5",
    "host": "127.0.0.1",
    "port": 30307,
    "auth_enabled": False,
    "username": "",
    "password": "",
    "tls_cert": "",             # "" = generated self-signed certificate
    "tls_key": "",
    "ss_method": "2022-blake3-aes-256-gcm",
    "ss_password": "",
    "hy2_password": "",
    "hy2_obfs_password": "",    # Hysteria2 server: Salamander obfuscation ("" = off; sing-box / official core)
    "hy2_up_mbps": 0,           # Hysteria2 server bandwidth limits (Brutal, 0 = no limit)
    "hy2_down_mbps": 0,
    "tun_stack": "mixed",       # TUN: sing-box network stack (mixed / system / gvisor)
    "tun_strict_route": False,  # TUN: block traffic that would bypass the TUN (can break IPv6 on Windows)
    "tun_fakeip": False,        # TUN: answer apps' DNS with fake IPs (198.18.0.0/15); the real lookup happens at the exit
    # WireGuard server: other devices connect with a WireGuard app (port = UDP listen port)
    "wg_private_key": "",
    "wg_address": "10.66.0.1/24",   # this server's tunnel address; clients get the next addresses of the subnet
    "wg_peers": "",             # JSON [{"name", "private_key", "public_key", "address"}] (client keys kept for their config)
    "wg_endpoint": "",          # address clients use to reach this PC ("" = this PC's LAN IP)
    "wg_mtu": 1408,
    # port forward: everything arriving on the port goes to this fixed destination through the card's outbound
    "fwd_host": "",
    "fwd_port": 0,
    "fwd_network": "tcp",       # "tcp", "udp" or "both"
}
OUTBOUND_DEFAULTS = {
    "type": "direct",
    "host": "",
    "port": 0,
    "username": "",
    "password": "",
    "tls_verify": True,
    "sni": "",
    "pin_sha256": "",
    "ss_method": "2022-blake3-aes-256-gcm",
    "ss_password": "",
    "hy2_password": "",
    "hy2_obfs_password": "",    # Salamander obfuscation, must match the server ("" = off)
    "hy2_up_mbps": 0,           # Brutal: this PC's real upload / download speed in Mbps (0 = BBR, automatic)
    "hy2_down_mbps": 0,
    "hy2_ports": "",            # port hopping: e.g. "20000-30000" or "443,5000-6000" ("" = off)
    "hy2_hop_interval": 30,     # seconds between port hops
    "bind": "",                 # Direct only: source address to connect from ("" = automatic)
    "interface": "",            # network interface to leave from (Tailscale, a VPN, Wi-Fi...; "" = automatic, sing-box)
    # WireGuard tunnel (host/port = the peer's endpoint)
    "wg_private_key": "",
    "wg_address": "",           # this side's tunnel address(es), comma separated, e.g. "10.0.0.2/32, fd00::2/128"
    "wg_peer_public_key": "",
    "wg_preshared_key": "",
    "wg_allowed_ips": "0.0.0.0/0, ::/0",
    "wg_mtu": 1408,
    "wg_keepalive": 0,          # persistent keepalive (s), 0 = off
    # VLESS / Trojan / TUIC (Trojan and TUIC use "password"; tls_verify / sni are shared with HTTPS and Hysteria2)
    "uuid": "",                 # VLESS / TUIC user id
    "security": "tls",          # VLESS: "none", "tls" or "reality"
    "vless_flow": "",           # VLESS: "" or "xtls-rprx-vision"
    "vmess_cipher": "auto",     # VMess: auto / aes-128-gcm / chacha20-poly1305 / none / zero
    "vmess_alter_id": 0,        # VMess: 0 = AEAD (current servers); > 0 only for very old servers
    "reality_public_key": "",
    "reality_short_id": "",
    "fingerprint": "chrome",    # uTLS client fingerprint ("" = Go's own TLS); REALITY needs one
    "alpn": "",                 # comma separated, "" = default (TUIC: h3)
    "transport": "tcp",         # VLESS / Trojan: "tcp", "ws" or "grpc"
    "transport_path": "",       # ws: path, grpc: service name
    "transport_host": "",       # ws: Host header
    "tuic_congestion": "bbr",   # cubic / new_reno / bbr
    "tuic_udp_relay": "native",  # native / quic
    # subscription (a provider's server list from a URL, used as one block)
    "sub_url": "",
    "sub_servers": "",          # JSON [{"name", "outbound"}] of the last update (kept for offline starts)
    "sub_mode": "auto",         # "auto" = fastest server (urltest, fails over), "select" = sub_selected
    "sub_selected": "",
    "sub_interval": 24,         # hours between automatic updates, 0 = only "Update now"
    "sub_updated": 0,           # time of the last update (epoch seconds)
    "sub_test_url": "https://www.gstatic.com/generate_204",     # what "auto" measures
    # group (several outbound blocks used as one; the members are graph links, node["members"])
    "group_mode": "auto",       # "auto" = fastest, "failover" = first member that works (in order), "select" = chosen one
    "group_test_url": "https://www.gstatic.com/generate_204",
    "group_interval": 180,      # seconds between measurements
    "group_tolerance": 50,      # auto: switch only when another member is this many ms faster
    "group_selected": "",       # select: id of the member in use
}
SECRET_FIELDS = ("password", "ss_password", "hy2_password", "hy2_obfs_password", "wg_private_key", "wg_preshared_key", "wg_peers",
                 "sub_url", "sub_servers")      # (subscription URLs carry tokens, the server list has passwords)


# --- validation ---

def _typed(defaults, raw):
    out = dict(defaults)
    if not isinstance(raw, dict):
        return out
    for key, default in defaults.items():
        v = raw.get(key, default)
        if isinstance(default, bool):
            ok = isinstance(v, bool)
        elif isinstance(default, int):
            ok = isinstance(v, int) and not isinstance(v, bool)
        elif isinstance(default, list):
            ok = isinstance(v, list) and all(isinstance(x, str) for x in v)
            if ok:
                v = [x.strip() for x in v if x.strip()]
        else:
            ok = isinstance(v, str)
        if ok:
            out[key] = v
    return out


MAX_TEST_SITES = 30


def test_site_host(text):
    """Host name for the connection test from what the user typed ('https://Example.com/x' -> 'example.com'),
    or None if it is no usable host."""
    import re
    t = (text or "").strip().lower()
    t = re.sub(r"^[a-z][a-z0-9+.-]*://", "", t).split("/", 1)[0].split("?", 1)[0]
    if t.count(":") == 1:                   # (host:port -> host; the test always uses HTTPS on 443)
        t = t.split(":", 1)[0]
    t = t.strip(".")
    if not t or len(t) > 253 or "." not in t:
        return None
    try:
        t.encode("idna")
    except UnicodeError:
        return None
    return t if re.fullmatch(r"[\w.-]+", t) else None


def validate_global(raw):
    from .ss import SS_METHODS  # noqa: F401  (keeps the import order for format checks)
    g = _typed(GLOBAL_DEFAULTS, raw)
    from .i18n import LANGUAGE_CODES
    if g["language"] not in LANGUAGE_CODES:
        g["language"] = "en"
    if g["theme"] not in dict(THEMES):
        g["theme"] = "auto"
    if g["ui_scale"] not in UI_SCALES:
        g["ui_scale"] = 100
    if g["text_size"] not in TEXT_SIZES:
        g["text_size"] = GLOBAL_DEFAULTS["text_size"]
    g["log_font_size"] = min(max(g["log_font_size"], 8), 20)
    hosts = []
    for x in g["test_sites"]:
        h = test_site_host(x)
        if h and h not in hosts:
            hosts.append(h)
    g["test_sites"] = hosts[:MAX_TEST_SITES]
    g["timeout"] = min(max(g["timeout"], 10), 86400)
    if not g["allowed_domains"]:
        g["allowed_domains"] = ["*"]
    if g["engine"] not in ENGINES:
        g["engine"] = "sing-box"
    if dns_server_spec(g["dns_server"]) is None:
        g["dns_server"] = ""
    return g


def validate_proxy(raw):
    from .ss import SS_METHODS
    raw = raw if isinstance(raw, dict) else {}
    p = {
        "id": raw.get("id") if isinstance(raw.get("id"), str) and raw.get("id") else uuid.uuid4().hex[:8],
        "name": (raw.get("name") or _t("Proxy")).strip()[:60] if isinstance(raw.get("name"), str) else _t("Proxy"),
        "autostart": raw.get("autostart") if isinstance(raw.get("autostart"), bool) else False,
        "auto_restart": raw.get("auto_restart") if isinstance(raw.get("auto_restart"), bool) else False,
        "inbound": _typed(INBOUND_DEFAULTS, raw.get("inbound")),
        "outbound": _typed(OUTBOUND_DEFAULTS, raw.get("outbound")),
        "overrides": {},
    }
    ib, ob = p["inbound"], p["outbound"]
    if ib["type"] not in INBOUND_TYPES:
        ib["type"] = "socks5"
    if ob["type"] not in OUTBOUND_TYPES:
        ob["type"] = "direct"
    for d in (ib, ob):
        if d["ss_method"] not in SS_METHODS:
            d["ss_method"] = INBOUND_DEFAULTS["ss_method"]
    if not 1 <= ib["port"] <= 65535:
        ib["port"] = INBOUND_DEFAULTS["port"]
    from .common import TUN_STACKS
    if ib["tun_stack"] not in TUN_STACKS:
        ib["tun_stack"] = "mixed"
    if not 0 <= ob["port"] <= 65535:
        ob["port"] = 0
    ov = raw.get("overrides") if isinstance(raw.get("overrides"), dict) else {}
    typed = _typed({k: GLOBAL_DEFAULTS[k] for k in OVERRIDABLE}, ov)
    for k in OVERRIDABLE:
        p["overrides"][k] = typed[k] if ov.get(k) is not None else None
    if p["overrides"]["timeout"] is not None:
        p["overrides"]["timeout"] = min(max(p["overrides"]["timeout"], 10), 86400)
    return p


def new_proxy(name, inbound=None, outbound=None, autostart=False):
    return validate_proxy({"id": uuid.uuid4().hex[:8], "name": name, "autostart": autostart,
                           "inbound": inbound or {}, "outbound": outbound or {}})


def resolve(proxy, glob):
    """Effective settings of a proxy (local first, otherwise global)."""
    out = {}
    for k in OVERRIDABLE:
        v = proxy["overrides"].get(k)
        out[k] = copy.deepcopy(v if v is not None else glob[k])
    return out


def _valid_listen_host(h):
    if h.lower() == "localhost":
        return True
    try:
        ipaddress.ip_address(h)
        return True
    except ValueError:
        return False


def _bind_addrs(host):
    """Inbound address -> set of (family, address) actually bound. localhost opens both IPv4 and IPv6 loopback."""
    if host.lower() == "localhost":
        return {(4, ipaddress.ip_address("127.0.0.1")), (6, ipaddress.ip_address("::1"))}
    try:
        a = ipaddress.ip_address(host)
    except ValueError:
        return set()
    return {(a.version, a)}


def _binds_overlap(h1, h2, udp=False):
    """True if the two addresses cannot be bound on the same port at once (same address in the same family,
    or one of them is the wildcard address). TCP binds IPv6 as v6-only so :: and 0.0.0.0 can coexist, but a UDP ::
    may also receive IPv4 depending on the OS, so it counts as overlapping."""
    def addrs(h):
        out = _bind_addrs(h)
        if udp and (6, ipaddress.ip_address("::")) in out:
            out.add((4, ipaddress.ip_address("0.0.0.0")))
        return out
    for f1, a1 in addrs(h1):
        for f2, a2 in addrs(h2):
            if f1 == f2 and (a1 == a2 or a1.is_unspecified or a2.is_unspecified):
                return True
    return False


def _is_local_target(host):
    h = host.strip().lower().rstrip(".")
    if h == "localhost" or h.endswith(".localhost"):
        return True
    try:
        a = ipaddress.ip_address(h.strip("[]"))
    except ValueError:
        return False
    return a.is_loopback or a.is_unspecified


def _next_hop(p, proxies):
    """If p's outbound points at the inbound of another (or the same) proxy in this app, return that proxy."""
    ob = p["outbound"]
    oh = ob["host"].strip().strip("[]")
    if ob["type"] == "direct" or not oh:
        return None
    for q in proxies:
        ih = q["inbound"]["host"]
        if q["inbound"]["port"] != ob["port"]:
            continue
        if oh.lower() == ih.lower() or (_is_local_target(oh) and _is_local_target(ih)):
            return q
    return None


def find_cycle(p, proxies):
    """Follow outbounds starting at p; if we come back to p return the path (list of names), else None."""
    others = [q for q in proxies if q["id"] != p["id"]] + [p]
    path, cur, seen = [p["name"]], p, {p["id"]}
    while True:
        nxt = _next_hop(cur, others)
        if nxt is None:
            return None
        path.append(nxt["name"])
        if nxt["id"] == p["id"]:
            return path
        if nxt["id"] in seen:
            return None
        seen.add(nxt["id"])
        cur = nxt


def _transports(t, ib=None):
    if t == "forward":
        net = (ib or {}).get("fwd_network", "tcp")
        return {"tcp", "udp"} if net == "both" else {net}
    return {"socks5": {"tcp", "udp"}, "shadowsocks": {"tcp", "udp"}, "hysteria2": {"udp"}, "wireguard": {"udp"}}.get(t, {"tcp"})


def wg_peers(ib):
    """The clients of a WireGuard server inbound: [{"name", "private_key", "public_key", "address"}]."""
    import json
    try:
        v = json.loads(ib.get("wg_peers") or "[]")
    except ValueError:
        return []
    return [x for x in v if isinstance(x, dict) and isinstance(x.get("public_key"), str)]


def wg_server_errors(ib):
    from .wgconf import valid_key
    errs = []
    if not valid_key(ib["wg_private_key"]):
        errs.append(_t("WireGuard server: generate the server key"))
    try:
        net = ipaddress.ip_interface(ib["wg_address"].strip())
        if net.network.prefixlen >= net.max_prefixlen - 1:
            raise ValueError
    except ValueError:
        errs.append(_t("WireGuard server: the tunnel address must be like 10.66.0.1/24"))
        net = None
    peers = wg_peers(ib)
    if not peers:
        errs.append(_t("WireGuard server: add at least one device"))
    seen = set()
    for x in peers:
        if not valid_key(x.get("public_key", "")):
            errs.append(_t("WireGuard server: '{name}' has no valid public key").format(name=x.get("name", "?")))
        try:
            a = ipaddress.ip_interface(x.get("address", "")).ip
            if net is not None and (a not in net.network or a == net.ip):
                raise ValueError
            if a in seen:
                raise ValueError
            seen.add(a)
        except ValueError:
            errs.append(_t("WireGuard server: '{name}' needs its own address inside {net}").format(
                name=x.get("name", "?"), net=ib["wg_address"]))
    if not 576 <= ib["wg_mtu"] <= 9000:
        errs.append(_t("WireGuard: MTU must be 576 to 9000"))
    return errs


def check_proxy(p, proxies):
    """Problems to fix before saving/starting (empty list = OK)."""
    from .ss import SS_METHODS, ss_make_key
    errs = []
    ib, ob = p["inbound"], p["outbound"]
    if not p["name"].strip():
        errs.append(_t("Enter a name"))
    if dns_server_spec((p.get("overrides") or {}).get("dns_server") or "") is None:
        errs.append(_t("DNS server: use an address like 1.1.1.1, tls://dns.google or https://cloudflare-dns.com/dns-query"))
    if p.get("unlinked"):
        errs.append(_t("Not connected to an outbound - connect it on the canvas or choose one in the editor"))
    tun = ib["type"] == "tun"
    if not tun and not _valid_listen_host(ib["host"]):
        errs.append(_t("The inbound address must be an IP (e.g. 127.0.0.1, 0.0.0.0, ::)"))
    if not tun and not 1 <= ib["port"] <= 65535:
        errs.append(_t("Invalid inbound port"))
    if ib["auth_enabled"] and ib["type"] in ("socks5", "http", "https"):
        if not (0 < len(ib["username"].encode()) <= 255 and 0 < len(ib["password"].encode()) <= 255):
            errs.append(_t("Enter a username and password to enable authentication (255 bytes max each)"))
    if ib["type"] == "shadowsocks":
        try:
            if not SS_METHODS[ib["ss_method"]][2] and len(ib["ss_password"]) < 8:
                raise ValueError(_t("The Shadowsocks password must be at least 8 characters"))
            ss_make_key(ib["ss_method"], ib["ss_password"])
        except ValueError as e:
            errs.append(_t("Inbound: {e}").format(e=e))
    if ib["type"] == "wireguard":
        errs += wg_server_errors(ib)
    if ib["type"] == "forward":
        if not ib["fwd_host"].strip() or not 1 <= ib["fwd_port"] <= 65535:
            errs.append(_t("Port forward: enter the destination address and port"))
        if ib["fwd_network"] not in ("tcp", "udp", "both"):
            errs.append(_t("Port forward: choose TCP, UDP or both"))
    if ib["type"] == "hysteria2" and len(ib["hy2_password"]) < 8:
        errs.append(_t("The Hysteria2 password must be at least 8 characters"))
    if ib["type"] in ("https", "hysteria2") and bool(ib["tls_cert"]) != bool(ib["tls_key"]):
        errs.append(_t("Set both the certificate and key files, or leave both empty (generated automatically)"))
    for f in ("tls_cert", "tls_key"):
        if ib["type"] in ("https", "hysteria2") and ib[f] and not os.path.isfile(ib[f]):
            errs.append(_t("File not found: {ib}").format(ib=ib[f]))
    errs += outbound_errors(ob)
    mine = _transports(ib["type"], ib)
    for other in proxies:
        if other["id"] == p["id"]:
            continue
        oi = other["inbound"]
        if tun or oi["type"] == "tun":     # a TUN card has no port
            continue
        shared = mine & _transports(oi["type"], oi)
        if oi["port"] != ib["port"] or not shared:
            continue
        if any(_binds_overlap(ib["host"], oi["host"], udp=t == "udp") for t in shared):
            errs.append(_t("Port {port} conflicts with '{name}'").format(name=other['name'], port=ib['port']))
    cycle = find_cycle(p, proxies)
    if cycle:
        errs.append(_t("The proxy chain loops: ") + " → ".join(cycle) + _t(" (check the outbound address/port)"))
    return errs


def hop_ports(text):
    """'443,5000-6000' -> ["443:443", "5000:6000"] (sing-box server_ports), or None if invalid."""
    out = []
    for part in text.replace(" ", "").split(","):
        if not part:
            continue
        a, _, b = part.partition("-")
        try:
            lo, hi = int(a), int(b or a)
        except ValueError:
            return None
        if not 1 <= lo <= hi <= 65535:
            return None
        out.append(f"{lo}:{hi}")
    return out or None


def outbound_errors(ob):
    """Problems in an outbound's settings, including the blocks it goes through (empty list = OK)."""
    from .ss import ss_make_key
    errs = []
    via = ob.get("via")
    if via:
        if ob["type"] == "hysteria2":
            errs.append(_t("Hysteria2 uses QUIC (UDP), so it cannot go through another outbound"))
        from .common import NEEDS_UDP_VIA, UDP_VIA_OK
        if ob["type"] in NEEDS_UDP_VIA and via["type"] not in UDP_VIA_OK:
            errs.append(_t("{type} needs UDP, so it cannot go through an HTTP or SOCKS4 proxy").format(type=OUTBOUND_TYPES[ob["type"]]))
        errs += outbound_errors(via)
    if ob["type"] == "block":
        pass
    elif ob["type"] == "group":
        members = ob.get("members") or []
        if not members:
            errs.append(_t("Connect at least one outbound block to the group's 'members' output"))
        if not ob["group_test_url"].strip().lower().startswith(("https://", "http://")):
            errs.append(_t("Group: the test address must start with http:// or https://"))
        if not 10 <= ob["group_interval"] <= 3600:
            errs.append(_t("Group: the interval must be 10 to 3600 seconds"))
        if not 0 <= ob["group_tolerance"] <= 5000:
            errs.append(_t("Group: the tolerance must be 0 to 5000 ms"))
        for m in members:
            errs += [f"{m['name']}: {e}" for e in outbound_errors(m["outbound"])]
    elif ob["type"] == "subscription":
        from .subscription import servers_of
        if not ob["sub_url"].strip().lower().startswith(("https://", "http://")):
            errs.append(_t("Enter the subscription address (https://...)"))
        if not servers_of(ob):
            errs.append(_t("The subscription has no servers yet - click 'Update now'"))
    elif ob["type"] == "direct":
        if ob.get("bind") and not _valid_listen_host(ob["bind"]):
            errs.append(_t("The source address must be an IP of this PC"))
    else:
        if not ob["host"].strip() or not 1 <= ob["port"] <= 65535:
            errs.append(_t("Enter the outbound server address and port"))
        if ob["type"] == "shadowsocks":
            try:
                ss_make_key(ob["ss_method"], ob["ss_password"])
                if not ob["ss_password"]:
                    raise ValueError(_t("Enter the Shadowsocks password"))
            except ValueError as e:
                errs.append(_t("Outbound: {e}").format(e=e))
        if ob["type"] == "hysteria2" and not ob["hy2_password"]:
            errs.append(_t("Enter the outbound Hysteria2 password"))
        if ob["type"] == "hysteria2" and ob["hy2_ports"].strip():
            if hop_ports(ob["hy2_ports"]) is None:
                errs.append(_t("Port hopping: use ports or ranges like 20000-30000 or 443,5000-6000"))
            if not 5 <= ob["hy2_hop_interval"] <= 3600:
                errs.append(_t("Port hopping: the interval must be 5 to 3600 seconds"))
        if ob["type"] == "hysteria2" and not (0 <= ob["hy2_up_mbps"] <= 100000 and 0 <= ob["hy2_down_mbps"] <= 100000):
            errs.append(_t("Bandwidth must be 0 to 100000 Mbps"))
        if ob["type"] == "vmess":
            if ob["security"] not in ("none", "tls"):
                errs.append(_t("VMess security: choose none or TLS"))
            if ob["vmess_cipher"] not in VMESS_CIPHERS:
                errs.append(_t("Choose the VMess encryption"))
            if not 0 <= ob["vmess_alter_id"] <= 65535:
                errs.append(_t("VMess alterId must be 0 to 65535"))
        if ob["type"] in ("vless", "vmess", "tuic"):
            import uuid as _uuid
            try:
                _uuid.UUID(ob["uuid"].strip())
            except ValueError:
                errs.append(_t("Enter the user UUID (e.g. 0b5b8d9e-...)"))
        if ob["type"] in ("trojan", "tuic") and not ob["password"]:
            errs.append(_t("Enter the password"))
        if ob["type"] == "vless":
            if ob["security"] not in ("none", "tls", "reality"):
                errs.append(_t("Choose the VLESS security (none, TLS or REALITY)"))
            if ob["security"] == "reality":
                import base64 as _b64
                try:
                    ok = len(_b64.urlsafe_b64decode(ob["reality_public_key"].strip() + "==")) == 32
                except ValueError:
                    ok = False
                if not ok:
                    errs.append(_t("REALITY: enter the server's public key"))
                sid = ob["reality_short_id"].strip()
                if len(sid) > 16 or len(sid) % 2 or any(c not in "0123456789abcdefABCDEF" for c in sid):
                    errs.append(_t("REALITY: the short ID is up to 16 hex digits"))
                if not ob["sni"].strip():
                    errs.append(_t("REALITY: enter the server name (SNI) of the site the server imitates"))
                if not ob["fingerprint"]:
                    errs.append(_t("REALITY needs a browser fingerprint"))
        if ob["type"] in ("vless", "vmess", "trojan") and ob["transport"] not in ("tcp", "ws", "grpc"):
            errs.append(_t("Choose the transport (TCP, WebSocket or gRPC)"))
        if ob["type"] == "wireguard":
            from .wgconf import valid_cidrs, valid_key
            if not valid_key(ob["wg_private_key"]):
                errs.append(_t("WireGuard: enter a valid private key (base64, or click Generate)"))
            if not valid_key(ob["wg_peer_public_key"]):
                errs.append(_t("WireGuard: enter the server's public key (base64)"))
            if ob["wg_preshared_key"] and not valid_key(ob["wg_preshared_key"]):
                errs.append(_t("WireGuard: the preshared key is not valid (base64, 32 bytes)"))
            if not valid_cidrs(ob["wg_address"]):
                errs.append(_t("WireGuard: enter this side's tunnel address, e.g. 10.0.0.2/32"))
            if not valid_cidrs(ob["wg_allowed_ips"]):
                errs.append(_t("WireGuard: allowed IPs must be IP ranges, e.g. 0.0.0.0/0, ::/0"))
            if not 576 <= ob["wg_mtu"] <= 9200:
                errs.append(_t("WireGuard: MTU must be between 576 and 9200"))
        if ob["type"] == "socks4" and ob["password"]:
            errs.append(_t("SOCKS4 does not support passwords (user ID only)"))
    return errs


# --- password storage: Windows DPAPI, elsewhere a user-only (0600) file ---

class _DATA_BLOB(ctypes.Structure):
    _fields_ = [("cbData", ctypes.c_uint32), ("pbData", ctypes.POINTER(ctypes.c_char))]


def _dpapi(data, protect):
    crypt32 = ctypes.WinDLL("crypt32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.LocalFree.argtypes = [ctypes.c_void_p]
    buf = ctypes.create_string_buffer(data, len(data))
    blob_in = _DATA_BLOB(len(data), ctypes.cast(buf, ctypes.POINTER(ctypes.c_char)))
    blob_out = _DATA_BLOB()
    fn = crypt32.CryptProtectData if protect else crypt32.CryptUnprotectData
    if not fn(ctypes.byref(blob_in), None, None, None, None, 0x1, ctypes.byref(blob_out)):
        raise OSError(ctypes.get_last_error(), _t("DPAPI failed"))
    try:
        return ctypes.string_at(blob_out.pbData, blob_out.cbData)
    finally:
        kernel32.LocalFree(ctypes.cast(blob_out.pbData, ctypes.c_void_p))


def _seal(d):
    """Prepare the password fields of a dict for saving (Windows: *_dpapi)."""
    out = dict(d)
    if not IS_WIN:
        return out
    for k in SECRET_FIELDS:
        if k in out:
            v = out.pop(k)
            out[k + "_dpapi"] = base64.b64encode(_dpapi(v.encode("utf-8"), True)).decode() if v else ""
    return out


def _unseal(d, where):
    if not isinstance(d, dict):
        return d, False
    out = dict(d)
    legacy = False
    for k in SECRET_FIELDS:
        enc = out.pop(k + "_dpapi", None)
        if isinstance(enc, str) and enc:
            if not IS_WIN:
                out[k] = ""
                continue
            try:
                out[k] = _dpapi(base64.b64decode(enc), False).decode("utf-8")
            except (OSError, ValueError, UnicodeDecodeError):
                log(LOG_WARN, _t("Could not decrypt the saved password ({where} {k}). Please enter it again").format(where=where, k=k))
                out[k] = ""
        elif IS_WIN and out.get(k):
            legacy = True
    return out, legacy


# --- read / write ---

def load():
    """(global, proxy list). Without a config, migrate the previous version's settings, or start with an empty list."""
    try:
        with open(CONFIG_PATH, encoding="utf-8") as f:
            raw = json.load(f)
    except FileNotFoundError:
        migrated = migrate_from_old()
        if migrated is not None:
            return migrated
        glob = validate_global({})
        graph = _graph.empty()
        save(glob, graph)
        return glob, graph
    except (OSError, ValueError) as e:
        # keep the broken file next to it so a later save does not overwrite it
        backup = CONFIG_PATH + ".broken"
        try:
            shutil.copy2(CONFIG_PATH, backup)
            log(LOG_WARN, _t("Could not read the settings file, starting with defaults ({e}). The original was kept as {backup}").format(e=e, backup=backup))
        except OSError:
            log(LOG_WARN, _t("Could not read the settings file, starting with defaults: {e}").format(e=e))
        return validate_global({}), _graph.empty()
    raw = raw if isinstance(raw, dict) else {}
    glob = validate_global(raw.get("global"))
    legacy = False
    if raw.get("version", 2) >= 3:
        g = raw.get("graph") if isinstance(raw.get("graph"), dict) else {}
        for key, field in (("inbounds", "inbound"), ("outbounds", "outbound")):
            items = []
            for item in g.get(key) or []:
                if isinstance(item, dict):
                    item = dict(item)
                    item[field], lg = _unseal(item.get(field), _t("inbound") if field == "inbound" else _t("outbound"))
                    legacy = legacy or lg
                    items.append(item)
            g = dict(g, **{key: items})
        graph = _graph.validate(g)
    else:       # settings v2: one inbound + outbound per proxy
        proxies = []
        for item in raw.get("proxies") or []:
            if not isinstance(item, dict):
                continue
            item = dict(item)
            item["inbound"], _ = _unseal(item.get("inbound"), _t("inbound"))
            item["outbound"], _ = _unseal(item.get("outbound"), _t("outbound"))
            proxies.append(validate_proxy(item))
        graph = _graph.from_proxies(proxies)
        legacy = True
        try:        # keep the v2 file so an older ProxyDock can still be used
            shutil.copy2(CONFIG_PATH, CONFIG_PATH.replace(".json", ".v2.json"))
        except OSError:
            pass
    if legacy:
        save(glob, graph)
    return glob, graph


def save(glob, graph):
    try:
        g = dict(graph, inbounds=[dict(n, inbound=_seal(n["inbound"])) for n in graph["inbounds"]],
                 outbounds=[dict(n, outbound=_seal(n["outbound"])) for n in graph["outbounds"]])
        data = {"version": 3, "global": glob, "graph": g}
    except OSError as e:
        log(LOG_WARN, _t("Could not encrypt passwords, settings not saved: {e}").format(e=e))
        return False
    try:
        os.makedirs(APP_DIR, exist_ok=True)
        tmp = CONFIG_PATH + ".tmp"
        if not IS_WIN:
            os.chmod(APP_DIR, 0o700)
            fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            os.close(fd)
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=2)
        os.replace(tmp, CONFIG_PATH)
        return True
    except OSError as e:
        log(LOG_WARN, _t("Failed to save settings: {e}").format(e=e))
        return False


def migrate_from_old():
    """Move the previous version's (SOCKS5 Proxy / ProxyLocal) settings into global settings + proxies. The original is kept."""
    old_dir = app_dir(OLD_APP_ID)
    old_path = os.path.join(old_dir, "config.json")
    try:
        with open(old_path, encoding="utf-8") as f:
            old = json.load(f)
    except (OSError, ValueError):
        return None
    if not isinstance(old, dict):
        return None
    old, _ = _unseal(old, _t("previous settings"))
    glob = validate_global(old)
    proxies = [new_proxy(_t("Default SOCKS5"), {
        "type": "socks5", "host": old.get("host", "127.0.0.1"), "port": old.get("port", 30307),
        "auth_enabled": old.get("auth_enabled", False), "username": old.get("username", ""),
        "password": old.get("password", ""),
    }, {"type": "direct"}, autostart=True)]
    if old.get("ss_enabled") and old.get("ss_password"):
        proxies.append(new_proxy("Shadowsocks", {
            "type": "shadowsocks", "host": old.get("host", "127.0.0.1"), "port": old.get("ss_port", 8388),
            "ss_method": old.get("ss_method", INBOUND_DEFAULTS["ss_method"]), "ss_password": old["ss_password"],
        }, {"type": "direct"}, autostart=True))
    os.makedirs(APP_DIR, exist_ok=True)
    for name in ("window.json",):
        src = os.path.join(old_dir, name)
        if os.path.exists(src) and not os.path.exists(os.path.join(APP_DIR, name)):
            shutil.copy2(src, os.path.join(APP_DIR, name))
    graph = _graph.from_proxies(proxies)
    save(glob, graph)
    log(LOG_INFO, _t("Imported settings from the previous version ({old_dir}) - {len} proxies").format(old_dir=old_dir, len=len(proxies)))
    return glob, graph
