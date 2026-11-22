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
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


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


def socks5_get(port, host, dport):
    s = socket.create_connection(("127.0.0.1", port), timeout=15)
    try:
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


def test_engine():
    from proxydock.runtime import ProxyManager
    from proxydock.ss import ss_generate_password
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    glob = config.validate_global({})
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
        check(f"server {name} starts", mgr.state(p["id"])[0] == "running", mgr.state(p["id"]))
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
    for label, names in cases:
        p = config.new_proxy(label, {"type": "socks5", "port": free_port()})
        if names:
            p["outbound"] = chain(*names)
        mgr.start(p, glob).result(15)
        r = socks5_get(p["inbound"]["port"], "127.0.0.1", oport)
        check(f"engine: socks5 -> {label} -> origin", r == "200", r)

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
    check("engine: http inbound plain GET", b" 200 " in data[:20] and b"Connection: close" in data, data[:80])
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


def main():
    t0 = time.time()
    tests = [test_translations, test_graph, test_engine]
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
