"""ProxyDock tests (no internet needed). Used by CI and tools/release.py.

    python tests/run_tests.py            # everything
    python tests/run_tests.py --no-ui    # skip the UI smoke test

Covers: translations complete for every UI string, block graph compile/validation (relay, via chains, loops),
settings checks (port conflicts), and the proxy engine end to end on 127.0.0.1 - every inbound type, upstream
proxies, Shadowsocks/Hysteria2 tunnels and chained outbounds (TLS and Shadowsocks stacked on other hops).
"""
import ast
import faulthandler
import glob
import json
import os
import socket
import string
import struct
import sys
import tempfile
import threading
import time
import traceback
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
faulthandler.enable()           # print the Python stack if Qt crashes (shows up in CI logs)
sys.path.insert(0, ROOT)
if sys.platform != "darwin":       # macOS: the frameless window needs a real Cocoa window
    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from proxydock import config, graph as gm  # noqa: E402

TMP = tempfile.mkdtemp(prefix="proxydock-test-")
config.APP_DIR = TMP
config.CONFIG_PATH = os.path.join(TMP, "config.json")
config.LOG_DIR = os.path.join(TMP, "logs")
config.CERT_DIR = os.path.join(TMP, "certs")
config.WINDOW_STATE_PATH = os.path.join(TMP, "window.json")
config.app_dir = lambda name="x": os.path.join(TMP, "none")

results = []


def check(name, cond, extra=""):
    results.append((name, bool(cond)))
    print(("PASS " if cond else "FAIL ") + name + (f"  {extra}" if extra and not cond else ""), flush=True)


def free_port():
    """A port free for both TCP and UDP (Shadowsocks/Hysteria2 inbounds also listen on UDP)."""
    while True:
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        p = s.getsockname()[1]
        u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            u.bind(("0.0.0.0", p))
            return p
        except OSError:
            continue
        finally:
            u.close()
            s.close()


# --- translations ---

def ui_keys():
    keys = set()
    for path in glob.glob(os.path.join(ROOT, "proxydock", "**", "*.py"), recursive=True):
        for n in ast.walk(ast.parse(open(path, encoding="utf-8").read())):
            if isinstance(n, ast.Call) and isinstance(n.func, ast.Name) and n.func.id == "_t" and n.args \
                    and isinstance(n.args[0], ast.Constant) and isinstance(n.args[0].value, str):
                keys.add(n.args[0].value)
    return keys


def fields(t):
    return sorted((f or "", s or "", c or "") for _, f, s, c in string.Formatter().parse(t) if f is not None)


def test_translations():
    keys = ui_keys()
    for path in sorted(glob.glob(os.path.join(ROOT, "proxydock", "locales", "*.json"))):
        table = json.load(open(path, encoding="utf-8"))
        missing = [k for k in keys if k not in table]
        bad = [k for k in keys if k in table and fields(k) != fields(table[k])]
        check(f"translations {os.path.basename(path)}: complete", not missing, missing[:3])
        check(f"translations {os.path.basename(path)}: placeholders match", not bad, bad[:3])


# --- graph / settings ---

def test_graph():
    p1 = config.new_proxy("A", {"type": "socks5", "port": 1080}, {"type": "direct"})
    p2 = config.new_proxy("B", {"type": "http", "port": 8080}, {"type": "direct"})
    g = gm.from_proxies([p1, p2])
    check("v2 -> graph shares one Direct block", len(g["outbounds"]) == 1)
    a, b = g["inbounds"]
    b["target"] = a["id"]
    c = gm.compile(g)
    check("relay compiles to the target inbound's protocol", c[1]["outbound"]["type"] == "socks5"
          and c[1]["outbound"]["port"] == 1080)
    a["target"] = b["id"]
    c = gm.compile(g)
    check("relay loop detected", bool(config.find_cycle(c[0], c)))
    corp = gm.add_outbound(g, {"type": "http", "host": "corp", "port": 3128}, "Corp")
    vps = gm.add_outbound(g, {"type": "shadowsocks", "host": "vps", "port": 8388, "ss_password": "x" * 10}, "VPS")
    vps["via"] = corp["id"]
    a["target"] = vps["id"]
    c = gm.compile(g)
    check("via chain resolved", c[0]["outbound"]["via"]["host"] == "corp" and c[0]["outbound"]["via"]["via"] is None)
    corp["via"] = vps["id"]
    check("via loop detected", gm.via_loop(g, vps["id"]))
    fixed = gm.validate(g)
    check("validate cuts via loops", not any(gm.via_loop(fixed, n["id"]) for n in fixed["outbounds"]))
    hy = gm.validate_outbound_node({"outbound": {"type": "hysteria2"}, "via": corp["id"]})
    check("hysteria2 cannot have a via", hy["via"] is None)
    x = config.new_proxy("X", {"type": "socks5", "host": "0.0.0.0", "port": 2000})
    y = config.new_proxy("Y", {"type": "http", "host": "127.0.0.1", "port": 2000})
    check("port conflict with 0.0.0.0", bool(config.check_proxy(y, [x, y])))


# --- engine ---

class Origin(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b"origin-ok"
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


def socks5_get(port, host, dport, auth=None):
    s = socket.create_connection(("127.0.0.1", port), timeout=15)
    try:
        if auth:
            s.sendall(b"\x05\x01\x02")
            s.recv(2)
            u, pw = (x.encode() for x in auth)
            s.sendall(b"\x01" + bytes([len(u)]) + u + bytes([len(pw)]) + pw)
            if s.recv(2)[1:2] != b"\x00":
                return "auth failed"
        else:
            s.sendall(b"\x05\x01\x00")
            s.recv(2)
        h = host.encode()
        s.sendall(b"\x05\x01\x00\x03" + bytes([len(h)]) + h + struct.pack("!H", dport))
        rep = s.recv(10)
        if len(rep) < 2 or rep[1] != 0:
            return f"socks reply {rep[1] if len(rep) > 1 else '?'}"
        s.sendall(f"GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n".encode())
        data = b""
        while True:
            d = s.recv(65536)
            if not d:
                break
            data += d
        return "200" if data.startswith(b"HTTP/1.") and b" 200 " in data[:20] and data.endswith(b"origin-ok") else data[:60]
    finally:
        s.close()


def test_engine(engine="python"):
    from proxydock.runtime import ProxyManager
    from proxydock.ss import ss_generate_password
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    glob = config.validate_global({"engine": engine})
    tag = "" if engine == "python" else f"[{engine}] "
    k22 = ss_generate_password("2022-blake3-aes-256-gcm")
    servers = {
        "http": {"type": "http"}, "https": {"type": "https"}, "socks5": {"type": "socks5"},
        "socks4": {"type": "socks4"},
        "ss22": {"type": "shadowsocks", "ss_method": "2022-blake3-aes-256-gcm", "ss_password": k22},
        "ss18": {"type": "shadowsocks", "ss_method": "chacha20-ietf-poly1305", "ss_password": "old-style-pass"},
        "hy2": {"type": "hysteria2", "hy2_password": "hy2-test-password"},
    }
    mgr = ProxyManager()
    out = {}
    for name, ib in servers.items():
        port = free_port()
        p = config.new_proxy(name, dict(ib, port=port))
        mgr.start(p, glob).result(15)
        check(f"{tag}server {name} starts", mgr.state(p["id"])[0] == "running", mgr.state(p["id"]))
        ob = {"type": ib["type"] if ib["type"] != "shadowsocks" else "shadowsocks", "host": "127.0.0.1", "port": port,
              "tls_verify": False, "ss_method": ib.get("ss_method", "2022-blake3-aes-256-gcm"),
              "ss_password": ib.get("ss_password", ""), "hy2_password": ib.get("hy2_password", "")}
        out[name] = ob

    def chain(*names):
        ob = None
        for n in reversed(names):
            d = dict(config.OUTBOUND_DEFAULTS)
            d.update(out[n])
            d["via"] = ob
            ob = d
        return ob

    cases = [("direct", None)] + [(n, (n,)) for n in servers] + [
        ("ss22 via http", ("ss22", "http")), ("https via ss22", ("https", "ss22")),
        ("http via hy2", ("http", "hy2")), ("ss18 via socks4", ("ss18", "socks4")),
        ("ss22 via https via socks5", ("ss22", "https", "socks5")),
    ]
    if engine == "sing-box":     # only sing-box can send Hysteria2 (QUIC) through another outbound
        cases.append(("hy2 via ss22", ("hy2", "ss22")))
    for label, names in cases:
        p = config.new_proxy(label, {"type": "socks5", "port": free_port()})
        if names:
            p["outbound"] = chain(*names)
        mgr.start(p, glob).result(15)
        r = socks5_get(p["inbound"]["port"], "127.0.0.1", oport)
        check(f"{tag}engine: socks5 -> {label} -> origin", r == "200", r)

    # HTTP inbound, plain request (absolute URI)
    hp = config.new_proxy("plain", {"type": "http", "port": free_port()})
    mgr.start(hp, glob).result(15)
    s = socket.create_connection(("127.0.0.1", hp["inbound"]["port"]), timeout=15)
    s.sendall(f"GET http://127.0.0.1:{oport}/ HTTP/1.1\r\nHost: x\r\n\r\n".encode())
    data = b""
    while True:
        d = s.recv(65536)
        if not d:
            break
        data += d
    s.close()
    check(f"{tag}engine: http inbound plain GET", b" 200 " in data[:20] and b"Connection: close" in data, data[:80])
    mgr.stop_all()
    origin.shutdown()


# --- UI smoke (offscreen) ---

def test_ui():
    from PySide6.QtCore import QEventLoop, QTimer
    from PySide6.QtNetwork import QLocalServer
    from PySide6.QtWidgets import QApplication
    from proxydock.ui import common as uic
    from proxydock.ui import window as winmod
    winmod.system_proxy_supported = lambda: True
    winmod.get_system_proxy = lambda: {"ProxyEnable": 0}
    winmod.set_system_proxy = lambda v: None
    errors = []
    sys.excepthook = lambda *a: errors.append(a[1])

    def wait(ms):
        loop = QEventLoop()
        QTimer.singleShot(ms, loop.quit)
        loop.exec()

    g = gm.empty()
    d = gm.add_outbound(g, {"type": "direct"}, "Direct")
    gm.add_inbound(g, "UI test", {"type": "http", "port": free_port()}, [0, 0], d["id"], autostart=True)
    glob = config.validate_global({})
    config.save(glob, g)
    glob, graph = config.load()
    app = QApplication.instance() or QApplication([])
    uic.apply_theme("light")
    uic.load_fonts(app, glob)
    srv = QLocalServer()
    srv.listen(f"proxydock-test-{os.getpid()}")
    ctl = winmod.Controller(srv, False, glob, graph)
    w = ctl.window
    w.resize(1200, 760)
    w.show()
    wait(800)
    for page in (w.canvas_page, w.proxies_page, w.log_page, w.stats_page, w.test_page, w.help_page,
                 w.license_page, w.settings_page):
        w.switchTo(page)
        wait(150)
    ctl.edit_node(graph["inbounds"][0]["id"])
    wait(300)
    check("ui: editor opens", w.stackedWidget.currentWidget() is w.editor)
    w.go_back()
    wait(300)
    check("ui: back leaves the editor", w.stackedWidget.currentWidget() is not w.editor)
    ctl.change_language("ko")
    wait(600)
    check("ui: language switch rebuilds the window", ctl.window.canvas_page.btn_in.text() == "받는 쪽 추가")
    ctl.change_language("en")
    wait(400)
    check("ui: autostart proxy running", any(s[0] == "running" for s in ctl.statuses().values()))
    check("ui: no exceptions", not errors, [repr(e) for e in errors][:3])
    ctl.manager.stop_all()


def test_singbox_compile():
    from proxydock.singbox import compile as sbc, python_reason
    via = dict(config.OUTBOUND_DEFAULTS, type="socks4", host="10.0.0.2", port=1080, via=None)
    ob = dict(config.OUTBOUND_DEFAULTS, type="https", host="up.example", port=443, tls_verify=False, sni="sni.example",
              via=dict(config.OUTBOUND_DEFAULTS, type="shadowsocks", host="9.9.9.9", port=8388, via=via))
    p = config.new_proxy("c", {"type": "socks5", "port": 1080, "auth_enabled": True, "username": "u", "password": "p"})
    p["outbound"] = ob
    cfg = dict(config.resolve(p, config.validate_global({})), allowed_domains=["a.example", "*.b.example", "1.2.3.4"],
               block_private=True)
    conf, reasons = sbc.build(p, cfg, bans_path="bans.json", api_port=1, api_secret="s")
    outs = conf["outbounds"]
    check("sing-box: via chain -> detour tags", [o.get("detour") for o in outs] == ["out1", "out2", None]
          and conf["route"]["final"] == "out0", outs)
    check("sing-box: https upstream tls", outs[0]["tls"] == {"enabled": True, "server_name": "sni.example", "insecure": True})
    check("sing-box: socks4 upstream is 4a", outs[2]["version"] == "4a")
    check("sing-box: socks5 inbound users", conf["inbounds"][0]["users"] == [{"username": "u", "password": "p"}])
    rules = conf["route"]["rules"]
    allow = rules[reasons.index(sbc.R_ALLOW)]
    check("sing-box: allow list rule", allow["invert"] and allow["domain"] == ["a.example"]
          and allow["domain_suffix"] == ["b.example"] and allow["ip_cidr"] == ["1.2.3.4/32"], allow)
    check("sing-box: no resolve through an upstream", not any(r.get("action") == "resolve" for r in rules))
    check("sing-box: reasons line up with rules", len(rules) == len(reasons) and reasons[1] == sbc.R_BANNED)
    p2 = config.new_proxy("d", {"type": "http", "port": 8080})
    p2["outbound"] = dict(config.OUTBOUND_DEFAULTS, type="direct", bind="192.168.0.5", via=None)
    conf2, _ = sbc.build(p2, cfg, bans_path="bans.json", api_port=1, api_secret="s")
    check("sing-box: direct bind + resolve before private check",
          conf2["outbounds"][0].get("inet4_bind_address") == "192.168.0.5"
          and any(r.get("action") == "resolve" for r in conf2["route"]["rules"]))
    check("sing-box: ban rule-set", sbc.bans_ruleset(["8.8.8.8", "2001:db8::1"])["rules"][0]["source_ip_cidr"]
          == ["2001:db8::1/128", "8.8.8.8/32"])
    g = config.validate_global({})
    hy = config.new_proxy("h", {"type": "hysteria2", "host": "0.0.0.0", "port": 4443, "hy2_password": "x"})
    from proxydock.singbox import engine_for, find_hysteria
    check("engines: exposed Hysteria2 server -> official core (or Python without it)",
          engine_for(hy, g)[0] == ("hysteria" if find_hysteria() else "python"), engine_for(hy, g))
    hy_local = config.new_proxy("hl", {"type": "hysteria2", "host": "127.0.0.1", "port": 4444, "hy2_password": "x"})
    check("engines: loopback Hysteria2 server -> sing-box", engine_for(hy_local, g)[0] == "sing-box")
    pin = config.new_proxy("pin", {"type": "socks5", "port": 1081})
    pin["outbound"] = dict(config.OUTBOUND_DEFAULTS, type="hysteria2", host="h", port=443, pin_sha256="ab" * 32, via=None)
    check("sing-box: pinned certificate stays on Python", python_reason(pin, g) != "")
    check("sing-box: python engine setting", python_reason(p2, config.validate_global({"engine": "python"})) != "")


def test_singbox_guard():
    """Auth failures from sing-box logs feed the shared ban list; sing-box rejects banned client IPs."""
    import time as _time
    from proxydock import common
    from proxydock.runtime import ProxyManager
    from proxydock.singbox import guard
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    glob = config.validate_global({"engine": "sing-box"})
    mgr = ProxyManager()
    p = config.new_proxy("guard", {"type": "socks5", "port": free_port(), "auth_enabled": True,
                                    "username": "u", "password": "p"})
    mgr.start(p, glob).result(15)
    inst = mgr.instances.get(p["id"])
    check("[sing-box] guard: card runs on sing-box", getattr(inst, "engine", "") == "sing-box", type(inst).__name__)
    # log lines from a non-loopback client: ten failures ban the IP (same rule as the Python engine)
    for i in range(common.AUTH_FAIL_LIMIT):
        inst._handle("ERROR", str(1000 + i), "inbound/socks[in]: process connection from 203.0.113.7:4000: "
                     "socks5: authentication failed, username=u, password=bad")
    data = json.load(open(guard.BANS_PATH, encoding="utf-8"))
    check("[sing-box] guard: ban written to the rule-set", "203.0.113.7/32" in data["rules"][0]["source_ip_cidr"], data)
    check("[sing-box] guard: password not logged", not any("password=bad" in e[2] for e in common.pending_logs))
    port = p["inbound"]["port"]
    check("[sing-box] guard: allowed before ban", socks5_get(port, "127.0.0.1", oport, auth=("u", "p")) == "200")
    common._banned["127.0.0.1"] = _time.monotonic() + 60     # loopback is never banned for real; force it to test sing-box
    guard.write_bans()
    _time.sleep(2)
    r = socks5_get(port, "127.0.0.1", oport, auth=("u", "p"))
    check("[sing-box] guard: banned IP rejected by sing-box", r != "200", r)
    common._banned.clear()
    guard.write_bans()
    _time.sleep(2)
    check("[sing-box] guard: allowed again after the ban ends", socks5_get(port, "127.0.0.1", oport, auth=("u", "p")) == "200")
    _time.sleep(1.5)
    st = mgr.stats_for(p["id"])
    check("[sing-box] guard: stats counted", st.total >= 2 and st.down > 0 and st.blocked >= 1,
          (st.total, st.down, st.blocked))
    mgr.stop_all()
    check("[sing-box] guard: process ended on stop", inst.proc is None)
    origin.shutdown()


def test_hysteria():
    """Hysteria2 server open to other PCs: official core + ProxyDock auth server (bans) + sing-box outbound."""
    from proxydock import common, singbox
    from proxydock.runtime import ProxyManager
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    glob = config.validate_global({"engine": "sing-box"})
    singbox.FORCE_HYSTERIA = True       # a real exposed (0.0.0.0) server would trigger the firewall prompt
    try:
        mgr = ProxyManager()
        srv = config.new_proxy("hy-srv", {"type": "hysteria2", "port": free_port(), "hy2_password": "right-pass"})
        mgr.start(srv, glob).result(20)
        inst = mgr.instances.get(srv["id"])
        check("[hysteria] server card runs on the official core", getattr(inst, "engine", "") == "hysteria",
              (type(inst).__name__, mgr.state(srv["id"])))

        def client(pw):
            p = config.new_proxy("hy-cli", {"type": "socks5", "port": free_port()})
            p["outbound"] = dict(config.OUTBOUND_DEFAULTS, type="hysteria2", host="127.0.0.1",
                                 port=srv["inbound"]["port"], hy2_password=pw, tls_verify=False, via=None)
            mgr.start(p, glob).result(20)
            return p

        good = client("right-pass")
        check("[hysteria] right password -> origin", socks5_get(good["inbound"]["port"], "127.0.0.1", oport) == "200")
        bad = client("wrong-pass")
        r = socks5_get(bad["inbound"]["port"], "127.0.0.1", oport)
        _t = time.time()
        while time.time() - _t < 3 and not any("authentication failed" in e[2] and e[3] == srv["id"]
                                               for e in common.pending_logs):
            time.sleep(0.1)
        check("[hysteria] wrong password refused and logged", r != "200"
              and any("authentication failed" in e[2] and e[3] == srv["id"] for e in common.pending_logs), r)
        ip = "203.0.113.9"
        for _ in range(common.AUTH_FAIL_LIMIT):
            inst.check_auth(f"{ip}:5000", "nope")
        check("[hysteria] repeated failures ban the IP", common.is_banned(ip))
        check("[hysteria] banned IP refused even with the right password", not inst.check_auth(f"{ip}:5001", "right-pass"))
        check("[hysteria] other IPs still allowed", inst.check_auth("198.51.100.4:5000", "right-pass"))
        time.sleep(1.5)                 # statistics come from the Clash API, read once a second
        st = mgr.stats_for(srv["id"])
        check("[hysteria] stats from the inner sing-box", st.total >= 1 and st.down > 0, (st.total, st.down))
        mgr.stop_all()
        check("[hysteria] processes ended on stop", inst.proc is None and inst.inner is None)
    finally:
        singbox.FORCE_HYSTERIA = False
        common._banned.clear()
        origin.shutdown()


def main():
    t0 = time.time()
    tests = [test_translations, test_graph, test_engine, test_singbox_compile]
    from proxydock.singbox import find_binary
    if find_binary():
        tests += [lambda: test_engine("sing-box"), test_singbox_guard]
        from proxydock.singbox import find_hysteria
        if find_hysteria():
            tests.append(test_hysteria)
        else:
            print("SKIP official Hysteria2 core tests (no binary: python tools/fetch_singbox.py)")
    else:
        print("SKIP sing-box engine tests (no binary: python tools/fetch_singbox.py)")
    if "--no-ui" not in sys.argv:
        tests.append(test_ui)
    for t in tests:
        try:
            t()
        except Exception:
            traceback.print_exc()
            check(f"{t.__name__} ran without an exception", False)
    failed = [n for n, ok in results if not ok]
    print(f"\n{len(results) - len(failed)}/{len(results)} passed in {time.time() - t0:.1f}s")
    if failed:
        print("FAILED: " + ", ".join(failed))
    sys.stdout.flush()
    from proxydock.common import hard_exit
    hard_exit(1 if failed else 0)


if __name__ == "__main__":
    main()
