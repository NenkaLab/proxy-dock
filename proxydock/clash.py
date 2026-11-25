"""Clash / Clash.Meta (mihomo) subscription YAML -> outbound settings.

Only the "proxies:" list is read (proxy groups and rules of the file are ignored; ProxyDock builds its own with
group and router blocks). Supported types: ss, vmess, vless, trojan, hysteria2, tuic, socks5, http, wireguard.
proxies(text) -> (list of (outbound fields, name), number of entries skipped). Raises ValueError if it is not
a Clash file.
"""
from .i18n import _t
import re

from .ss import SS_METHODS


def looks_like_clash(text):
    return re.search(r"^proxies\s*:", text, re.M) is not None


def _load(text):
    import yaml
    try:
        doc = yaml.safe_load(text)
    except yaml.YAMLError as e:
        raise ValueError(_t("The Clash subscription is not valid YAML: {e}").format(e=str(e).splitlines()[0])) from None
    if not isinstance(doc, dict) or not isinstance(doc.get("proxies"), list):
        raise ValueError(_t("The Clash subscription has no 'proxies' list"))
    return doc["proxies"]


def _s(v):
    return "" if v is None else str(v).strip()


def _b(v):
    return v is True or _s(v).lower() in ("true", "1", "yes")


def _mbps(v):
    """'100 Mbps', '1 Gbps', 100 -> Mbps (0 if unknown)."""
    m = re.match(r"^\s*([\d.]+)\s*([kmg]?)(?:bps|b)?\s*$", _s(v).lower())
    if not m:
        return 0
    n = float(m.group(1)) * {"k": 0.001, "m": 1, "g": 1000, "": 1}[m.group(2)]
    return max(0, min(int(n), 100000))


def _tls(p, sni_key="servername"):
    out = {}
    sni = _s(p.get(sni_key) or p.get("sni") or p.get("servername"))
    if sni:
        out["sni"] = sni
    if _b(p.get("skip-cert-verify")):
        out["tls_verify"] = False
    alpn = p.get("alpn")
    if isinstance(alpn, list) and alpn:
        out["alpn"] = ",".join(_s(x) for x in alpn if _s(x))
    if _s(p.get("client-fingerprint")):
        out["fingerprint"] = _s(p["client-fingerprint"])
    return out


def _transport(p):
    net = (_s(p.get("network")) or "tcp").lower()
    if net == "ws":
        o = p.get("ws-opts") if isinstance(p.get("ws-opts"), dict) else {}
        headers = o.get("headers") if isinstance(o.get("headers"), dict) else {}
        return {"transport": "ws", "transport_path": _s(o.get("path")) or "/",
                "transport_host": _s(headers.get("Host") or headers.get("host"))}
    if net == "grpc":
        o = p.get("grpc-opts") if isinstance(p.get("grpc-opts"), dict) else {}
        return {"transport": "grpc", "transport_path": _s(o.get("grpc-service-name"))}
    if net != "tcp":
        raise ValueError(f"transport {net}")
    return {"transport": "tcp"}


def _one(p):
    t = _s(p.get("type")).lower()
    host, port = _s(p.get("server")), p.get("port")
    try:
        port = int(port)
    except (TypeError, ValueError):
        raise ValueError("port") from None
    base = {"host": host.strip("[]"), "port": port}
    if t == "ss":
        if p.get("plugin"):
            raise ValueError("plugin")      # obfs / v2ray-plugin are not supported
        method = _s(p.get("cipher")).lower()
        if method not in SS_METHODS:
            raise ValueError("cipher")      # (the checks only know these ciphers)
        return dict(base, type="shadowsocks", ss_method=method, ss_password=_s(p.get("password")))
    if t == "vmess":
        ob = dict(base, type="vmess", uuid=_s(p.get("uuid")), vmess_cipher=(_s(p.get("cipher")) or "auto").lower(),
                  vmess_alter_id=int(p.get("alterId") or 0), security="tls" if _b(p.get("tls")) else "none", **_transport(p))
        if ob["security"] == "tls":
            ob.update(_tls(p))
        return ob
    if t == "vless":
        reality = p.get("reality-opts") if isinstance(p.get("reality-opts"), dict) else None
        sec = "reality" if reality else "tls" if _b(p.get("tls")) else "none"
        ob = dict(base, type="vless", uuid=_s(p.get("uuid")), vless_flow=_s(p.get("flow")), security=sec, **_transport(p))
        if sec != "none":
            ob.update(_tls(p))
        if reality:
            ob.update(reality_public_key=_s(reality.get("public-key")), reality_short_id=_s(reality.get("short-id")))
            ob.setdefault("fingerprint", "chrome")
        return ob
    if t == "trojan":
        return dict(base, type="trojan", password=_s(p.get("password")), **_tls(p, "sni"), **_transport(p))
    if t == "hysteria2":
        ob = dict(base, type="hysteria2", hy2_password=_s(p.get("password") or p.get("auth")), **_tls(p, "sni"))
        ob.pop("fingerprint", None)
        ob.pop("alpn", None)
        obfs = _s(p.get("obfs")).lower()
        if obfs and obfs != "salamander":
            raise ValueError("obfs")
        if obfs:
            ob["hy2_obfs_password"] = _s(p.get("obfs-password"))
        ob["hy2_up_mbps"], ob["hy2_down_mbps"] = _mbps(p.get("up")), _mbps(p.get("down"))
        if _s(p.get("ports")):
            ob["hy2_ports"] = _s(p["ports"]).replace("/", ",")
            if p.get("hop-interval"):
                ob["hy2_hop_interval"] = int(p["hop-interval"])
        return ob
    if t == "tuic":
        ob = dict(base, type="tuic", uuid=_s(p.get("uuid")), password=_s(p.get("password")), **_tls(p, "sni"))
        ob.pop("fingerprint", None)
        if _s(p.get("congestion-controller")):
            ob["tuic_congestion"] = _s(p["congestion-controller"]).lower()
        if _s(p.get("udp-relay-mode")):
            ob["tuic_udp_relay"] = _s(p["udp-relay-mode"]).lower()
        return ob
    if t == "socks5":
        if _b(p.get("tls")):
            raise ValueError("socks5 tls")
        return dict(base, type="socks5", username=_s(p.get("username")), password=_s(p.get("password")))
    if t == "http":
        ob = dict(base, type="https" if _b(p.get("tls")) else "http", username=_s(p.get("username")),
                  password=_s(p.get("password")))
        if ob["type"] == "https":
            ob.update(_tls(p, "sni"))
            ob.pop("fingerprint", None)
            ob.pop("alpn", None)
        return ob
    if t == "wireguard":
        addrs = [x for x in (_s(p.get("ip")), _s(p.get("ipv6"))) if x]
        addrs = [a if "/" in a else a + ("/128" if ":" in a else "/32") for a in addrs]
        allowed = p.get("allowed-ips")
        ob = dict(base, type="wireguard", wg_private_key=_s(p.get("private-key")), wg_address=", ".join(addrs),
                  wg_peer_public_key=_s(p.get("public-key")), wg_preshared_key=_s(p.get("pre-shared-key")))
        if isinstance(allowed, list) and allowed:
            ob["wg_allowed_ips"] = ", ".join(_s(x) for x in allowed)
        if p.get("mtu"):
            ob["wg_mtu"] = int(p["mtu"])
        return ob
    raise ValueError(f"type {t}")


def proxies(text):
    entries = _load(text)
    out, skipped = [], 0
    for p in entries:
        if not isinstance(p, dict):
            skipped += 1
            continue
        try:
            fields = _one(p)
        except (ValueError, TypeError):
            skipped += 1
            continue
        out.append((fields, _s(p.get("name"))[:60] or fields["type"]))
    return out, skipped
