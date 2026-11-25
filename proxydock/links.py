"""Share links -> outbound settings: vless://, vmess://, trojan://, tuic://, ss://, hysteria2:// (hy2://).

These are the links VPN/proxy providers hand out (the formats v2rayN, Clash and sing-box clients use).
parse(link) -> (outbound fields for config.OUTBOUND_DEFAULTS, name). Raises ValueError for anything else.
"""
from .i18n import _t
import base64
import urllib.parse

from .ss import SS_METHODS

SCHEMES = ("vless", "vmess", "trojan", "tuic", "ss", "hysteria2", "hy2")


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
        raise ValueError(_t("Not a share link (vless://, vmess://, trojan://, tuic://, ss:// or hysteria2://)"))
    if scheme == "vmess":
        return _vmess(link)
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
        if method not in SS_METHODS:        # (the editor and the checks only know these)
            raise ValueError(_t("Shadowsocks cipher '{value}' is not supported").format(value=method))
        return {"type": "shadowsocks", "host": host, "port": port, "ss_method": method,
                "ss_password": password}, name or "Shadowsocks"

    if scheme in ("hysteria2", "hy2"):
        # the official format allows port hopping in the address: host:443,5000-6000
        hp = u.netloc.rsplit("@", 1)[-1]
        hosts, _, ports = (hp[1:].partition("]")[0], None, hp.partition("]")[2].lstrip(":")) if hp.startswith("[") \
            else (hp.rpartition(":")[0], None, hp.rpartition(":")[2])
        hop = q.get("mport", "")
        if ports and not ports.isdigit():
            hop, ports = ports, ports.replace("-", ",").split(",")[0]
        host, port = _host_port(f"[{hosts}]:{ports}" if ":" in hosts else f"{hosts}:{ports}")
        ob = {"type": "hysteria2", "host": host, "port": port, "hy2_password": user, **_tls_fields(q)}
        ob.pop("fingerprint", None)
        ob.pop("alpn", None)
        if q.get("pinSHA256"):
            ob["pin_sha256"] = q["pinSHA256"]
        if q.get("obfs"):
            if q["obfs"].lower() != "salamander":
                raise ValueError(_t("Hysteria2 obfuscation '{value}' is not supported").format(value=q["obfs"]))
            ob["hy2_obfs_password"] = q.get("obfs-password", "")
        if hop:
            ob["hy2_ports"] = hop
        return ob, name or "Hysteria2"

    host, port = _host_port(u.netloc)

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


def _vmess(link):
    """vmess://base64(JSON) as v2rayN writes it: add, port, id, aid, scy, net, type, host, path, tls, sni, alpn, fp, ps."""
    import json
    try:
        d = json.loads(_b64(link.split("://", 1)[1].split("#", 1)[0]))
        if not isinstance(d, dict):
            raise ValueError
    except (ValueError, UnicodeDecodeError):
        raise ValueError(_t("The vmess:// link is not readable (expected base64 JSON)")) from None

    def get(k):
        v = d.get(k)
        return "" if v is None else str(v).strip()

    host, port = get("add"), get("port")
    if not host or not port.isdigit() or not 0 < int(port) < 65536:
        raise ValueError(_t("The link has no valid server address and port"))
    net = (get("net") or "tcp").lower()
    if net == "tcp" and get("type").lower() not in ("", "none"):
        raise ValueError(_t("Transport '{value}' is not supported (TCP, WebSocket and gRPC are)").format(value="tcp/" + get("type")))
    q = {"type": net, "path": get("path") or ("/" if net == "ws" else ""), "host": get("host"), "serviceName": get("path")}
    ob = {"type": "vmess", "host": host.strip("[]"), "port": int(port), "uuid": get("id"),
          "vmess_cipher": get("scy").lower() or "auto", "vmess_alter_id": int(get("aid")) if get("aid").isdigit() else 0,
          "security": "tls" if get("tls").lower() == "tls" else "none", **_transport_fields(q)}
    if ob["security"] == "tls":
        ob.update(_tls_fields({"sni": get("sni") or get("host"), "alpn": get("alpn"), **({"fp": get("fp")} if get("fp") else {})}))
    return ob, get("ps") or "VMess"
