"""Connection test: connect to several sites over HTTPS through the selected proxy.

To test every inbound protocol the same way, a temporary SOCKS5 "bridge" proxy is started. Its outbound is a
client for the tested proxy's inbound protocol (SOCKS/HTTP/HTTPS/Shadowsocks/Hysteria2), so a SOCKS5 connection
to the bridge goes bridge -> tested proxy -> that proxy's outbound -> site.
"""
from .i18n import _t
import socket
import ssl
import struct
import time

from . import config
from .common import ATYP_IPV4, ATYP_IPV6, describe_error

TEST_SITES = [
    (_t("Global"), "Google", "www.google.com"),
    (_t("Global"), "YouTube", "www.youtube.com"),
    (_t("Global"), "Microsoft", "www.microsoft.com"),
    (_t("Global"), "Apple", "www.apple.com"),
    (_t("Global"), "Amazon", "www.amazon.com"),
    (_t("Global"), "Cloudflare", "www.cloudflare.com"),
    (_t("Global"), "GitHub", "github.com"),
    (_t("Global"), "Wikipedia", "www.wikipedia.org"),
    (_t("Global"), "Netflix", "www.netflix.com"),
    (_t("Korea"), "NAVER", "www.naver.com"),
    (_t("Japan"), "Yahoo! JAPAN", "www.yahoo.co.jp"),
    (_t("China"), "Baidu", "www.baidu.com"),
    (_t("USA"), "CNN", "www.cnn.com"),
    (_t("UK"), "BBC", "www.bbc.co.uk"),
    (_t("Germany"), "DER SPIEGEL", "www.spiegel.de"),
    (_t("France"), "Le Monde", "www.lemonde.fr"),
    (_t("Brazil"), "Globo", "www.globo.com"),
    (_t("India"), "Flipkart", "www.flipkart.com"),
    (_t("Australia"), "ABC", "www.abc.net.au"),
    (_t("Canada"), "CBC", "www.cbc.ca"),
    (_t("Russia"), "Yandex", "ya.ru"),
]
PROBE_TIMEOUT = 8
PROBE_WORKERS = 6           # sites tried at the same time
PROBE_COOLDOWN = 15         # seconds before the test can be run again
BRIDGE_ID = "__probe__"
REP_NAMES = {1: _t("Proxy error"), 2: _t("Blocked (allow list / private network)"), 3: _t("Network unreachable"),
             4: _t("Host unreachable (DNS / route / upstream proxy)"), 5: _t("Connection refused"), 6: _t("TTL expired"),
             7: _t("Command not supported"), 8: _t("Address type not supported")}


def bridge_for(proxy):
    """Settings of the temporary SOCKS5 bridge proxy that leads to the proxy under test."""
    ib = proxy["inbound"]
    host = {"0.0.0.0": "127.0.0.1", "::": "::1"}.get(ib["host"], ib["host"])
    t = ib["type"]
    ob = {"type": t, "host": host, "port": ib["port"], "tls_verify": False}
    if t in ("socks5", "http", "https") and ib["auth_enabled"]:
        ob.update(username=ib["username"], password=ib["password"])
    if t == "shadowsocks":
        ob.update(ss_method=ib["ss_method"], ss_password=ib["ss_password"])
    if t == "hysteria2":
        ob.update(hy2_password=ib["hy2_password"])
    p = config.new_proxy(_t("Connection test"), {"type": "socks5", "host": "127.0.0.1", "port": 1}, ob)
    p["id"] = BRIDGE_ID
    p["inbound"]["port"] = 0            # use a free port (set to 0 after validation)
    p["overrides"].update(allowed_domains=["*"], block_private=False, log_connections=False, log_close=False)
    return p


def bridge_port(manager):
    inst = manager.instances.get(BRIDGE_ID)
    return inst.servers[0].sockets[0].getsockname()[1] if inst else None


def _recvn(s, n):
    data = b""
    while len(data) < n:
        chunk = s.recv(n - len(data))
        if not chunk:
            raise ConnectionError(_t("The proxy closed the connection"))
        data += chunk
    return data


def probe_site(host, port):
    """Connect to host:443 through the bridge (127.0.0.1:port). Result dict (ok, status, connect_ms, total_ms, error, note)."""
    res = {"ok": False, "status": "", "connect_ms": None, "total_ms": None, "error": "", "kind": ""}
    t0 = time.perf_counter()
    try:
        s = socket.create_connection(("127.0.0.1", port), timeout=PROBE_TIMEOUT)
    except OSError as e:
        res["error"] = _t("Test connection failed: {describe_error}").format(describe_error=describe_error(e))
        return res
    try:
        s.sendall(b"\x05\x01\x00")
        _recvn(s, 2)
        hb = host.encode("idna")
        s.sendall(b"\x05\x01\x00\x03" + bytes([len(hb)]) + hb + struct.pack("!H", 443))
        rep = _recvn(s, 4)
        if rep[1] != 0:
            raise ValueError(REP_NAMES.get(rep[1], _t("SOCKS error {rep}").format(rep=rep[1])))
        _recvn(s, {ATYP_IPV4: 6, ATYP_IPV6: 18}.get(rep[3], 6))
        res["connect_ms"] = (time.perf_counter() - t0) * 1000
        ctx = ssl.create_default_context()
        with ctx.wrap_socket(s, server_hostname=host) as ss:
            ss.sendall((f"HEAD / HTTP/1.1\r\nHost: {host}\r\n"
                        "User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64)\r\n"
                        "Connection: close\r\n\r\n").encode())
            line = ss.recv(512).split(b"\r\n", 1)[0].decode("latin-1")
        parts = line.split()
        res["status"] = parts[1] if len(parts) > 1 and line.startswith("HTTP/") else _t("No response")
        res["ok"] = res["status"].isdigit()
        if not res["ok"]:
            res["error"] = _t("Not an HTTP response")
    except ssl.SSLCertVerificationError:
        # The route and the TLS server were reached. Python often cannot use root certificates the OS downloads on demand.
        res["ok"] = True
        res["note"] = _t("Certificate not verified (no local root certificate)")
    except ssl.SSLError as e:
        res["error"] = _t("TLS error ({value})").format(value=e.reason or e)
        res["kind"] = "tls"
    except socket.timeout:
        res["error"] = _t("Timed out")
        res["kind"] = "timeout"
    except ValueError as e:
        res["error"] = str(e)
        res["kind"] = "socks"
    except OSError as e:
        res["error"] = describe_error(e)
        res["kind"] = "closed" if isinstance(e, ConnectionError) else "net"
    finally:
        s.close()
        res["total_ms"] = (time.perf_counter() - t0) * 1000
    return res


# --- "Check connection" in the editor: test the outbound with unsaved settings ---

CHECK_ID = "__check__"
CHECK_SITES = ("www.google.com", "www.cloudflare.com", "www.microsoft.com")


def checker_for(proxy, glob):
    """Temporary SOCKS5 proxy (127.0.0.1, free port) with the same outbound + local settings as the proxy being edited."""
    import copy
    p = config.new_proxy(_t("Check connection"), {"type": "socks5", "host": "127.0.0.1", "port": 1},
                         copy.deepcopy(proxy["outbound"]))
    p["id"] = CHECK_ID
    p["inbound"]["port"] = 0
    p["overrides"] = config.resolve(proxy, glob)
    p["overrides"].update(allowed_domains=["*"], log_connections=False, log_close=False)
    return p


def check_outbound(manager, proxy, glob):
    """Check that one of CHECK_SITES can be reached over HTTPS through the outbound.
    Result dict: ok, site, status, connect_ms, total_ms, error, detail (failure reason logged by the proxy), note."""
    res = {"ok": False, "site": "", "error": "", "detail": ""}
    try:
        if not manager.start(checker_for(proxy, glob), glob).result(15):
            res["error"] = manager.state(CHECK_ID)[1] or _t("Could not create the check connection")
            return res
        inst = manager.instances.get(CHECK_ID)
        port = bridge_port_for(manager, CHECK_ID)
        for site in CHECK_SITES:
            inst.last_error = inst.last_reason = ""
            r = probe_site(site, port)
            r["site"] = site
            r["detail"] = inst.last_error
            r["reason"] = inst.last_reason
            res = r
            if r["ok"] or r["kind"] == "timeout":    # a timeout is usually a route problem, so other sites would time out too
                break
        return res
    except Exception as e:
        res["error"] = str(e) or type(e).__name__
        return res
    finally:
        try:
            manager.stop(CHECK_ID).result(5)
        except Exception:
            pass


def bridge_port_for(manager, pid):
    inst = manager.instances.get(pid)
    return inst.servers[0].sockets[0].getsockname()[1] if inst else None
