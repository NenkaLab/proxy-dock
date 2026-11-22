"""Share links -> outbound settings: vless://, trojan://, tuic://, ss://, hysteria2:// (hy2://).

These are the links VPN/proxy providers hand out (the formats v2rayN, Clash and sing-box clients use).
parse(link) -> (outbound fields for config.OUTBOUND_DEFAULTS, name). Raises ValueError for anything else.
"""
from .i18n import _t
import base64
import urllib.parse

SCHEMES = ("vless", "trojan", "tuic", "ss", "hysteria2", "hy2")


def _b64(text):
    text = text.strip()
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4)).decode("utf-8")


def _truthy(v):
    return (v or "").lower() in ("1", "true", "yes")


def _host_port(netloc):
    hp = netloc.rsplit("@", 1)[-1]
    if hp.startswith("["):
        host, _, rest = hp[1:].partition("]")
        port = rest.lstrip(":")
    else:
        host, _, port = hp.rpartition(":")
    if not host or not port.isdigit() or not 0 < int(port) < 65536:
        raise ValueError(_t("The link has no valid server address and port"))
    return host, int(port)


def _tls_fields(q):
    out = {}
    if q.get("sni") or q.get("peer"):
        out["sni"] = q.get("sni") or q.get("peer")
    if _truthy(q.get("allowInsecure")) or _truthy(q.get("insecure")) or _truthy(q.get("allow_insecure")):
        out["tls_verify"] = False
    if q.get("alpn"):
        out["alpn"] = q["alpn"].replace(" ", "")
    if "fp" in q:
        out["fingerprint"] = q["fp"]
    return out


def _transport_fields(q):
    t = (q.get("type") or "tcp").lower()
    if t == "ws":
        return {"transport": "ws", "transport_path": q.get("path", "/"), "transport_host": q.get("host", "")}
    if t == "grpc":
        return {"transport": "grpc", "transport_path": q.get("serviceName") or q.get("path", "")}
    if t not in ("tcp", "raw", ""):
        raise ValueError(_t("Transport '{value}' is not supported (TCP, WebSocket and gRPC are)").format(value=t))
    return {"transport": "tcp"}


def parse(link):
    link = link.strip()
    scheme = link.split("://", 1)[0].lower() if "://" in link else ""
    if scheme not in SCHEMES:
        raise ValueError(_t("Not a share link (vless://, trojan://, tuic://, ss:// or hysteria2://)"))
    u = urllib.parse.urlsplit(link)
    q = {k: v[0] for k, v in urllib.parse.parse_qs(u.query, keep_blank_values=True).items()}
    name = urllib.parse.unquote(u.fragment).strip()
    user = urllib.parse.unquote(u.netloc.rsplit("@", 1)[0]) if "@" in u.netloc else ""

    if scheme == "ss":
        if "@" not in u.netloc:             # legacy ss://base64(method:password@host:port)
            decoded = _b64(u.netloc)
            userinfo, _, hp = decoded.rpartition("@")
            host, port = _host_port(hp)
        else:
            host, port = _host_port(u.netloc)
            raw = u.netloc.rsplit("@", 1)[0]
            userinfo = urllib.parse.unquote(raw) if ":" in urllib.parse.unquote(raw) else _b64(raw)
        method, _, password = userinfo.partition(":")
        return {"type": "shadowsocks", "host": host, "port": port, "ss_method": method,
                "ss_password": password}, name or "Shadowsocks"

    host, port = _host_port(u.netloc)
    if scheme in ("hysteria2", "hy2"):
        ob = {"type": "hysteria2", "host": host, "port": port, "hy2_password": user, **_tls_fields(q)}
        ob.pop("fingerprint", None)
        ob.pop("alpn", None)
        if q.get("pinSHA256"):
            ob["pin_sha256"] = q["pinSHA256"]
        if q.get("obfs"):
            raise ValueError(_t("Hysteria2 obfuscation (Salamander) is not supported yet"))
        return ob, name or "Hysteria2"

    if scheme == "vless":
        sec = (q.get("security") or "none").lower()
        if sec not in ("none", "tls", "reality"):
            raise ValueError(_t("VLESS security '{value}' is not supported").format(value=sec))
        ob = {"type": "vless", "host": host, "port": port, "uuid": user, "security": sec,
              "vless_flow": q.get("flow", ""), **_tls_fields(q), **_transport_fields(q)}
        if sec == "reality":
            ob.update(reality_public_key=q.get("pbk", ""), reality_short_id=q.get("sid", ""))
            ob.setdefault("fingerprint", "chrome")
        return ob, name or "VLESS"

    if scheme == "trojan":
        ob = {"type": "trojan", "host": host, "port": port, "password": user, **_tls_fields(q), **_transport_fields(q)}
        return ob, name or "Trojan"

    # tuic://uuid:password@host:port
    uid, _, pw = user.partition(":")
    ob = {"type": "tuic", "host": host, "port": port, "uuid": uid, "password": pw, **_tls_fields(q)}
    ob.pop("fingerprint", None)
    if q.get("congestion_control"):
        ob["tuic_congestion"] = q["congestion_control"]
    if q.get("udp_relay_mode"):
        ob["tuic_udp_relay"] = q["udp_relay_mode"]
    return ob, name or "TUIC"
