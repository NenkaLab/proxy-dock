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


def connect_retry(port, timeout=15):
    """Connect to a freshly started local listener; a refusal is retried briefly. A lasting refusal (seen now and
    then on GitHub's Windows runners, never locally) fails with what the card and its engine looked like."""
    for i in range(6):
        try:
            return socket.create_connection(("127.0.0.1", port), timeout=timeout)
        except ConnectionRefusedError:
            if i == 5:
                raise ConnectionRefusedError(f"127.0.0.1:{port} refused\n" + port_diagnostics(port)) from None
            time.sleep(0.5)


def port_diagnostics(port):
    """Who should be listening on port: card state, engine process, its last output, the OS listener table, logs."""
    import gc
    import subprocess
    from proxydock import common
    from proxydock.runtime import ProxyManager
    out = []
    for mgr in [o for o in gc.get_objects() if isinstance(o, ProxyManager)]:
        for pid, inst in list(mgr.instances.items()):
            proxy = getattr(inst, "proxy", {}) or {}
            if (proxy.get("inbound") or {}).get("port") != port:
                continue
            eng = getattr(inst, "inner", None) or inst
            proc = getattr(eng, "proc", None)
            out.append(f"card {proxy.get('name')!r}: state={mgr.state(pid)} engine={type(eng).__name__} "
                       f"pid={getattr(proc, 'pid', None)} returncode={getattr(proc, 'returncode', None)}")
            out += ["  | " + x for x in list(getattr(eng, "raw_tail", []))[-15:]]
    if not out:
        out.append("no running card has this port")
    try:
        cmd = ["netstat", "-ano", "-p", "tcp"] if sys.platform == "win32" else ["sh", "-c", "ss -ltnp || netstat -an"]
        lines = subprocess.run(cmd, capture_output=True, text=True, timeout=10).stdout.splitlines()
        out += ["listener: " + x.strip() for x in lines if f":{port} " in x + " "] or [f"no TCP listener on :{port}"]
    except Exception as e:
        out.append(f"netstat failed: {e}")
    out += ["log: " + common.format_line(e) for e in list(common.pending_logs)[-12:]]
    return "\n".join(out)


def socks5_get(port, host, dport, auth=None):
    s = connect_retry(port)
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
    # router block: create, connect on the model, open its editor, cut a loop
    r = ctl.new_router(edit=False)
    blk = ctl.new_outbound({"type": "block"}, edit=False)
    r2 = ctl.new_router(edit=False)
    check("ui: router rule -> block", ctl.set_rule_target(r["id"], r["rules"][0]["id"], blk["id"]))
    check("ui: router default -> router", ctl.set_rule_target(r["id"], "default", r2["id"]))
    check("ui: router loop refused", not ctl.set_rule_target(r2["id"], "default", r["id"]))
    check("ui: inbound -> router", ctl.set_target(graph["inbounds"][0]["id"], r["id"]))
    w.switchTo(w.canvas_page)
    wait(300)
    check("ui: router drawn with rule outputs", r["id"] in w.canvas_page.node_of
          and len(w.canvas_page.node_of[r["id"]].outputs()) == 2)
    ctl.edit_node(r["id"])
    wait(300)
    check("ui: router editor opens", w.stackedWidget.currentWidget() is w.editor and w.editor.s_router.isVisible()
          and len(w.editor.s_router.cards) == 1)
    sr = w.editor.s_router
    check("ui: router editor starts in the simple view", not sr.cards[0].advanced and sr.cards[0].simple.isVisible())
    c0 = sr.add_rule(pick=False)
    c0.rows[0].ed.setText("example.com")
    pr = c0.add_row("process_name")
    pr.add_programs(["game.exe", "GAME.EXE", "chrome.exe"])
    sr.set_mode(True)
    check("ui: router simple -> advanced keeps the conditions", c0.edits["domain_suffix"].text() == "example.com"
          and c0.edits["process_name"].text() == "game.exe, chrome.exe", c0.edits["process_name"].text())
    c0.edits["port"].setText("443")
    sr.set_mode(False)
    check("ui: router advanced -> simple shows a row per condition", sorted(r.key() for r in c0.rows)
          == ["domain_suffix", "port", "process_name"], [r.key() for r in c0.rows])
    ads = sr.add_rule(*[t[1:] for t in __import__("proxydock.ui.router", fromlist=["templates"]).templates()
                        if t[3] == "block"][0], pick=False)
    check("ui: 'Ads' template goes to the Block outbound", ads.collect()["target"] == blk["id"]
          and ads.collect()["match"] == {"geosite": ["category-ads-all"]})
    from proxydock.platform_utils import running_programs
    progs = running_programs()
    check("ui: running programs listed for the picker", (not sys.platform.startswith("win")) or any(x.lower().endswith(".exe") for x in progs),
          progs[:5])
    sr.remove(ads)
    sr.remove(c0)
    sr.set_mode(True)
    w.editor.s_router.add_rule()
    w.editor.s_router.cards[1].edits["port"].setText("70000")
    check("ui: router editor reports a bad port", any("70000" in e for e in w.editor._errors(w.editor.collect())))
    w.editor.cancel()
    wait(300)
    ctl.edit_node(d["id"])
    wait(300)
    check("ui: Direct editor shows the network interface choice", w.editor.r_out_iface.isVisible()
          and w.editor.cb_out_iface.count() >= 1)
    import proxydock.ui.proxies as uip
    shown, real_info = [], uip.info
    uip.info = lambda *a, **k: shown.append(a)
    try:
        ed = w.editor
        ed.ifaces, ed.ifaces_unrouted = ["", "Routed", "VM switch"], {"VM switch"}
        ed._filling_ifaces = True
        ed.cb_out_iface.clear()
        ed.cb_out_iface.addItems(["Automatic", "Routed", "VM switch"])
        ed._filling_ifaces = False
        ed.cb_out_iface.setCurrentIndex(1)
        check("ui: a routed interface gives no warning", not shown, shown)
        ed.cb_out_iface.setCurrentIndex(2)
        check("ui: an interface without a gateway warns when picked", len(shown) == 1 and shown[0][1] == "warning", shown)
        ed._fill_ifaces("VM switch")
        check("ui: refilling the list does not warn again", len(shown) == 1, shown)
    finally:
        uip.info = real_info
    w.editor.cancel()
    wait(300)
    # pass-through block: canvas lanes, editor, inside wiring
    relay = ctl.new_relay(None, edit=False, lanes=2)
    check("ui: pass-through created", gm.find(ctl.graph, relay["id"])[0] == "relay" and len(relay["inputs"]) == 2)
    ui_in = graph["inbounds"][0]
    check("ui: inbound links into a lane", ctl.set_target(ui_in["id"], relay["inputs"][0]["id"]))
    check("ui: lane links on to an outbound", ctl.set_lane_target(relay["id"], relay["outputs"][0]["id"], d["id"]))
    wait(200)
    cv = w.canvas_page
    rnode = cv.node_of.get(relay["id"])
    check("ui: canvas draws the lanes", rnode is not None and len(rnode.inputs()) == 2 and len(rnode.outputs()) == 2
          and relay["inputs"][0]["id"] in cv.lane_port and rnode.view.links == [(0, 0), (1, 1)])
    port = cv.lane_port.get(relay["inputs"][0]["id"])
    check("ui: canvas link goes into the lane port", port is not None
          and any(cv.mid_of.get(x.node().id) == ui_in["id"] for x in port.connected_ports()))
    check("ui: card leaves through the lane", ctl.proxy_by_id(ui_in["id"])["outbound"]["type"] == "direct"
          and not ctl.proxy_by_id(ui_in["id"])["unlinked"])
    ctl.add_lane(relay["id"])
    wait(200)
    check("ui: footer adds a lane", len(gm.find(ctl.graph, relay["id"])[1]["inputs"]) == 3
          and len(cv.node_of[relay["id"]].inputs()) == 3)
    check("ui: a lane cannot loop into itself",
          not ctl.set_lane_target(relay["id"], relay["outputs"][0]["id"], relay["inputs"][0]["id"]))
    ctl.edit_node(relay["id"])
    wait(300)
    ed = w.editor
    check("ui: pass-through editor opens", w.stackedWidget.currentWidget() is ed and ed.s_relay.isVisible()
          and not ed.sections["in"].isVisible() and len(ed.s_relay.in_rows) == 3)
    ed.s_relay.cb_mode.setCurrentIndex(1)                   # routed
    ed.s_relay.sp_out.setValue(2)
    wait(100)
    check("ui: routed shows the inside wiring", ed.s_relay.in_rows[0].combo.isVisible() and len(ed.s_relay.out_rows) == 2)
    ed.s_relay.in_rows[2].combo.setCurrentIndex(1)          # input 3 -> output 1
    ed.s_relay.in_rows[1].gap.setValue(2)
    ed.save()
    wait(300)
    rn = gm.find(ctl.graph, relay["id"])[1]
    check("ui: pass-through editor saves", rn["routed"] and len(rn["outputs"]) == 2 and rn["inputs"][1]["gap"] == 2
          and rn["inputs"][2]["out"] == rn["outputs"][0]["id"], rn)
    check("ui: canvas follows the saved wiring", cv.node_of[relay["id"]].view.routed
          and (2, 0) in cv.node_of[relay["id"]].view.links)
    check("ui: inside wiring can be changed", ctl.set_lane_map(relay["id"], rn["inputs"][0]["id"], rn["outputs"][1]["id"])
          and gm.find(ctl.graph, relay["id"])[1]["inputs"][0]["out"] == rn["outputs"][1]["id"])
    check("ui: lane 1 now leads nowhere", ctl.proxy_by_id(ui_in["id"])["unlinked"])
    # dragging inside a routed block (real mouse events through the canvas view)
    from PySide6.QtCore import QEvent, QPoint, QPointF, Qt
    from PySide6.QtGui import QMouseEvent
    cv.fit()
    wait(300)
    viewer = cv.graph.viewer()

    def mouse(kind, pt, buttons):
        vp = viewer.viewport()
        ev = QMouseEvent({"press": QEvent.MouseButtonPress, "move": QEvent.MouseMove,
                          "release": QEvent.MouseButtonRelease}[kind], QPointF(pt), vp.mapToGlobal(QPointF(pt)),
                         Qt.NoButton if kind == "move" else Qt.LeftButton, buttons, Qt.NoModifier)
        QApplication.sendEvent(vp, ev)

    def drag(a, b):
        mouse("press", a, Qt.LeftButton)
        for k in range(1, 6):
            mouse("move", a + (b - a) * k / 5, Qt.LeftButton)
        mouse("release", b, Qt.NoButton)
        wait(300)

    def dots():
        item = cv.node_of[relay["id"]].view
        item.setZValue(item.zValue() + 100)     # (the test layout puts other blocks on top of it)
        ins, outs = item.lane_points()
        return ([viewer.mapFromScene(item.mapToScene(x)) for x in ins],
                [viewer.mapFromScene(item.mapToScene(x)) for x in outs])

    def wiring():
        n = gm.find(ctl.graph, relay["id"])[1]
        outs = [x["id"] for x in n["outputs"]]
        return [outs.index(x["out"]) if x["out"] in outs else None for x in n["inputs"]]
    ins, outs = dots()
    under = [type(x).__name__ for x in viewer.items(ins[0])]
    check("ui: an inner dot is not covered by the port", "RelayItem" in under and "PortItem" not in under, under)
    before = wiring()
    drag(ins[1], outs[0])
    check("ui: drag an input dot onto an output dot wires them", before[1] != 0 and wiring() == [before[0], 0, before[2]],
          (before, wiring()))
    ins, outs = dots()
    drag(outs[0], ins[0])
    check("ui: drag from an output dot works too", wiring()[0] == 0, wiring())
    ins, outs = dots()
    drag(ins[0], ins[0] + QPoint(0, 400))
    check("ui: dragging a wire off unwires it", wiring()[0] is None, wiring())
    ins, outs = dots()
    pos = list(gm.find(ctl.graph, relay["id"])[1]["pos"])
    drag(ins[1], ins[1] + QPoint(1, 0))
    check("ui: a click on a dot changes nothing", wiring()[1] == 0 and gm.find(ctl.graph, relay["id"])[1]["pos"] == pos)
    # block colours: swatch, links in the block colour, editor keeps it
    ctl.set_color(ui_in["id"], "#E5534B")
    wait(200)
    item = cv.node_of[ui_in["id"]].view
    pipes = [p for port in cv.node_of[ui_in["id"]].output_ports() for p in port.view.connected_pipes]
    check("ui: block colour shows on the swatch and its links", item.swatch == "#e5534b" and pipes
          and all(tuple(p.color)[:3] == (229, 83, 75) for p in pipes), [tuple(p.color) for p in pipes])
    ritem = cv.node_of[relay["id"]].view
    lane0 = [i for i, x in enumerate(gm.find(ctl.graph, relay["id"])[1]["inputs"]) if x["id"] == ui_in["target"]]
    check("ui: a pass-through lane hands the colour on", lane0 and ritem.link_colors[lane0[0]] == "#e5534b", ritem.link_colors)
    opened = []
    cv.color_menu = lambda mid, at: opened.append(mid)
    item.setZValue(item.zValue() + 100)          # (the test layout puts other blocks on top of it)
    sr = item.swatch_rect()
    pt = viewer.mapFromScene(item.mapToScene(sr.center()))
    mouse("press", pt, Qt.LeftButton)
    mouse("release", pt, Qt.NoButton)
    wait(100)
    del cv.color_menu
    check("ui: clicking the swatch opens the colour menu", opened == [ui_in["id"]], opened)
    ctl.edit_node(ui_in["id"])
    wait(300)
    w.editor.save()
    wait(300)
    ui_in = gm.find(ctl.graph, ui_in["id"])[1]          # (saving replaces the block)
    check("ui: saving in the editor keeps the colour", gm.find(ctl.graph, ui_in["id"])[1]["color"] == "#e5534b")
    ctl.set_color(ui_in["id"], "")
    wait(200)
    pipes = [p for port in cv.node_of[ui_in["id"]].output_ports() for p in port.view.connected_pipes]
    check("ui: default colour again", cv.node_of[ui_in["id"]].view.swatch == ""
          and all(tuple(p.color)[:3] == (175, 95, 30) for p in pipes))
    ctl.delete_node(relay["id"])
    wait(200)
    check("ui: deleting the pass-through hands its links on", ui_in["target"] is None and relay["id"] not in cv.node_of)
    ctl.set_target(ui_in["id"], d["id"])
    w.editor.cancel()
    wait(300)
    t = ctl.new_inbound("TUN", {"type": "tun"}, None, None, edit=False)
    ctl.edit_node(t["id"])
    wait(300)
    check("ui: TUN editor hides the port and shows TUN options", w.editor.r_tun_stack.isVisible()
          and not w.editor.r_in_listen.isVisible())
    w.editor.cancel()
    wait(300)
    ctl.delete_node(t["id"])
    import base64
    SubServer.body = base64.b64encode(b"ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ@1.2.3.4:8388#One\n"
                                      b"trojan://pw@b.example:443?sni=b.example#Two")
    web = ThreadingHTTPServer(("127.0.0.1", 0), SubServer)
    threading.Thread(target=web.serve_forever, daemon=True).start()
    sb = ctl.new_outbound({"type": "subscription"}, edit=False)
    ctl.edit_node(sb["id"])
    wait(300)
    w.editor.ed_sub_url.setText(f"http://127.0.0.1:{web.server_address[1]}/s")
    w.editor._sub_update()
    for _ in range(40):
        wait(100)
        if w.editor.btn_sub_update.isEnabled() and w.editor.sub_names:
            break
    check("ui: subscription 'Update now' lists the servers", w.editor.sub_names == ["One", "Two"], w.editor.sub_names)
    w.editor.save()
    wait(300)
    _, sbn = gm.find(ctl.graph, sb["id"])
    check("ui: subscription saved with its servers", sbn is not None and '"Two"' in sbn["outbound"]["sub_servers"])
    SubServer.body = base64.b64encode(b"ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ@1.2.3.4:8388#Three")
    ctl.update_due_subscriptions(force_id=sb["id"])
    for _ in range(40):
        wait(100)
        if '"Three"' in gm.find(ctl.graph, sb["id"])[1]["outbound"]["sub_servers"]:
            break
    check("ui: background subscription update", '"Three"' in gm.find(ctl.graph, sb["id"])[1]["outbound"]["sub_servers"])
    ctl.delete_node(sb["id"])
    web.shutdown()
    h2 = ctl.new_outbound({"type": "hysteria2", "host": "h.example", "port": 443}, edit=False)
    ctl.edit_node(h2["id"])
    wait(300)
    check("ui: Hysteria2 editor shows Salamander, bandwidth and port hopping",
          w.editor.r_out_obfs.isVisible() and w.editor.r_out_bw.isVisible() and w.editor.r_out_hop.isVisible())
    w.editor.ed_out_obfs.setText("x")
    w.editor.ed_out_hop.setText("20000-30000")
    got = w.editor.collect()["outbound"]
    check("ui: Hysteria2 advanced values collected", got["hy2_obfs_password"] == "x" and got["hy2_ports"] == "20000-30000")
    w.editor.cancel()
    wait(300)
    ctl.delete_node(h2["id"])
    ifb = ctl.new_router(edit=False, style="if")
    wait(300)
    node = w.canvas_page.node_of.get(ifb["id"])
    check("ui: IF block drawn with then / else", node is not None and list(node.outputs()) == ["then", "else"],
          list(node.outputs()) if node else None)
    ctl.edit_node(ifb["id"])
    wait(300)
    sr = w.editor.s_router
    check("ui: IF editor shows one condition, no 'Add rule'", sr.is_if and len(sr.cards) == 1 and not sr.b_add.isVisible()
          and w.editor.title.text() == "Edit IF")
    check("ui: IF needs a condition and a 'then' target", any("IF" in e for e in w.editor._errors(w.editor.collect())))
    sr.cb_if_mode.setCurrentIndex(1)
    check("ui: IF switch mode hides the conditions", not sr.cards[0].simple.isVisible() and sr.sw_if.isVisible())
    w.editor.cancel()
    wait(200)
    blk2 = ctl.new_outbound({"type": "block"}, edit=False)
    n_if = gm.find(ctl.graph, ifb["id"])[1]
    n_if.update(switch=True, default=d["id"])
    n_if["rules"][0]["target"] = blk2["id"]
    ctl._recompile()
    wait(300)
    tw = w.canvas_page.node_of[ifb["id"]].get_widget("toggle")
    check("ui: switch IF shows a checkbox on the block", tw is not None)
    tw.cb.setChecked(True)
    check("ui: clicking the block's switch turns it on", gm.find(ctl.graph, ifb["id"])[1]["switch_on"] is True)
    ctl.delete_node(blk2["id"])
    ctl.delete_node(ifb["id"])
    rt = ctl.new_router(edit=False)
    wait(300)
    check("ui: router listed on the proxies page", rt["id"] in w.proxies_page.router_cards
          and w.proxies_page.router_title.isVisibleTo(w.proxies_page))
    ctl.delete_node(rt["id"])
    wait(200)
    check("ui: deleted router leaves the list", rt["id"] not in w.proxies_page.router_cards)
    gd = ctl.new_outbound({"type": "direct"}, edit=False)
    gp = ctl.new_outbound({"type": "group"}, edit=False)
    check("ui: group member added", ctl.add_member(gp["id"], gd["id"]) and gm.find(ctl.graph, gp["id"])[1]["members"] == [gd["id"]])
    check("ui: a group cannot be a member", not ctl.add_member(gp["id"], gp["id"]))
    wait(300)
    gnode = w.canvas_page.node_of.get(gp["id"])
    check("ui: group drawn with its member link", gnode is not None and len(gnode.output(0).connected_ports()) == 1)
    ctl.edit_node(gp["id"])
    wait(300)
    check("ui: group editor lists the member", w.editor.r_grp_members.isVisible() and w.editor.lst_grp.count() == 1
          and not w.editor.r_out_server.isVisible())
    w.editor.cb_grp_mode.setCurrentIndex(1)
    got = w.editor.collect()
    check("ui: group values collected", got["outbound"]["group_mode"] == "failover" and got["members"] == [gd["id"]]
          and not w.editor.r_grp_tol.isVisible())
    w.editor.save()
    wait(300)
    check("ui: group saved", gm.find(ctl.graph, gp["id"])[1]["outbound"]["group_mode"] == "failover")
    gd2 = ctl.new_outbound({"type": "direct"}, edit=False)
    ctl.add_member(gp["id"], gd2["id"])
    gm.find(ctl.graph, gp["id"])[1]["outbound"]["group_mode"] = "select"
    ctl.select_member(gp["id"], gd2["id"])
    check("ui: manual group switches member", gm.find(ctl.graph, gp["id"])[1]["outbound"]["group_selected"] == gd2["id"])
    from proxydock.ui.proxies import member_menu
    check("ui: 'Use member' menu for a manual group", member_menu(ctl, gp["id"], w) is not None)
    ctl.delete_node(gd2["id"])
    ctl.delete_node(gd["id"])
    check("ui: deleting a member updates the group", gm.find(ctl.graph, gp["id"])[1]["members"] == [])
    ctl.delete_node(gp["id"])
    wgs = ctl.new_inbound("wg server ui", {"type": "wireguard", "host": "0.0.0.0", "port": 51829}, None, None, edit=False)
    ctl.edit_node(wgs["id"])
    wait(300)
    ed = w.editor
    check("ui: WireGuard server editor", ed.r_wgi_peers.isVisible() and ed.r_wgi_key.isVisible() and bool(ed.ed_wgi_key.text()))
    ed.ed_wgi_name.setText("Phone")
    ed._wgi_add()
    ed._wgi_add()
    got = ed.collect()["inbound"]
    peers = config.wg_peers(got)
    check("ui: WireGuard devices get keys and addresses", [x["address"] for x in peers] == ["10.66.0.2/32", "10.66.0.3/32"]
          and peers[0]["name"] == "Phone" and not config.wg_server_errors(got), (peers, config.wg_server_errors(got)))
    ed.lst_wgi.setCurrentRow(0)
    ed._wgi_copy()
    check("ui: WireGuard device config copied", "[Peer]" in QApplication.clipboard().text()
          and peers[0]["private_key"] in QApplication.clipboard().text())
    ed.cancel()
    wait(300)
    ctl.delete_node(wgs["id"])
    ar = ctl.new_inbound("auto restart ui", {"type": "socks5", "port": free_port()}, None, None, edit=False)
    wait(300)
    sw = w.canvas_page.node_of[ar["id"]].get_widget("switches")
    sw.restart.setChecked(True)
    sw.auto.setChecked(True)
    n_ar = gm.find(ctl.graph, ar["id"])[1]
    check("ui: block checkboxes set auto-restart and autostart", n_ar["auto_restart"] and n_ar["autostart"])
    sw = w.canvas_page.node_of[ar["id"]].get_widget("switches")
    sw.run.setChecked(True)
    for _ in range(60):
        wait(100)
        if ctl.manager.state(ar["id"])[0] == "running":
            break
    check("ui: 'Run' checkbox on the block starts the card", ctl.manager.state(ar["id"])[0] == "running", ctl.manager.state(ar["id"]))
    if getattr(ctl.manager.instances.get(ar["id"]), "proc", None) is not None:
        ctl.manager.instances[ar["id"]].proc.kill()
        seen_error = False
        for _ in range(120):
            wait(100)
            st = ctl.manager.state(ar["id"])[0]
            seen_error = seen_error or st == "error"
            if seen_error and st == "running":
                break
        check("ui: auto-restart brings a dead card back", seen_error and ctl.manager.state(ar["id"])[0] == "running",
              ctl.manager.state(ar["id"]))
    w.canvas_page.node_of[ar["id"]].get_widget("switches").run.setChecked(False)
    wait(500)
    check("ui: 'Run' unchecked stops it (no restart)", ctl.manager.state(ar["id"])[0] == "stopped" and ar["id"] not in ctl._restart_timers)
    ctl.delete_node(ar["id"])
    tn = ctl.new_inbound("tun ui", {"type": "tun", "port": 30301}, None, None, edit=False)
    ctl.edit_node(tn["id"])
    wait(300)
    check("ui: TUN editor shows Fake-IP", w.editor.r_tun_fakeip.isVisible())
    w.editor.sw_tun_fakeip.setChecked(True)
    check("ui: Fake-IP collected", w.editor.collect()["inbound"]["tun_fakeip"] is True)
    w.editor.cancel()
    wait(300)
    ctl.delete_node(tn["id"])
    tport, _, _ = tls_test_server()
    hb = ctl.new_outbound({"type": "https", "host": "127.0.0.1", "port": tport}, edit=False)
    ctl.edit_node(hb["id"])
    wait(300)
    w.editor._detect_sni()
    for _ in range(50):
        wait(100)
        if w.editor.btn_detect.isEnabled() and w.editor.ed_out_sni.text():
            break
    check("ui: 'Detect from server' fills the SNI", w.editor.ed_out_sni.text() == "localhost", w.editor.ed_out_sni.text())
    w.editor.cancel()
    wait(300)
    ctl.delete_node(hb["id"])
    vb = ctl.new_outbound({"type": "vless"}, edit=False)
    ctl.edit_node(vb["id"])
    wait(300)
    QApplication.clipboard().setText(VLESS_LINK)
    w.editor._paste_link()
    wait(200)
    got = w.editor.collect()["outbound"]
    check("ui: paste share link fills the editor", got["security"] == "reality" and got["host"] == "a.example"
          and w.editor.r_out_reality.isVisible() and w.editor.ed_name.text() == "JP Reality", got)
    w.editor.cancel()
    wait(300)
    ctl.delete_node(vb["id"])
    w.switchTo(w.settings_page)
    wait(200)
    w.settings_page.cb_dns.setText("tls://dns.google")
    w.settings_page.sw_admin.setChecked(True)
    w.settings_page.apply()
    check("ui: settings keep 'Run as administrator'", ctl.glob["run_as_admin"] is True)
    w.settings_page.sw_admin.setChecked(False)
    w.settings_page.apply()
    ctl.set_target(graph["inbounds"][0]["id"], d["id"])
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


def test_bans_file_locked():
    """bans.json is shared by every sing-box card. On Windows a file another process has open cannot be replaced:
    starting a card used to rewrite it unconditionally and failed with WinError 5 while other cards ran."""
    from proxydock import common
    from proxydock.singbox import guard
    common._banned.clear()
    guard._written = None
    check("bans: first write", guard.write_bans(force=True) and os.path.exists(guard.BANS_PATH))
    before = os.path.getmtime(guard.BANS_PATH)
    held = open(guard.BANS_PATH, encoding="utf-8")      # like a running sing-box reading it
    try:
        check("bans: unchanged set is not rewritten (card start while the file is open)",
              guard.write_bans(force=True) and os.path.getmtime(guard.BANS_PATH) == before)
        if sys.platform == "win32":
            common._banned["203.0.113.9"] = time.monotonic() + 60
            check("bans: a locked file is reported, not raised", guard.write_bans() is False)
    finally:
        held.close()
    check("bans: the next try writes it", guard.write_bans() and "203.0.113.9" in open(guard.BANS_PATH, encoding="utf-8").read()
          if sys.platform == "win32" else True)
    common._banned.clear()
    guard.write_bans()


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


def test_router_graph():
    g = gm.empty()
    d = gm.add_outbound(g, {"type": "direct"}, "Direct")
    up = gm.add_outbound(g, {"type": "socks5", "host": "127.0.0.1", "port": 1081}, "Up")
    blk = gm.add_outbound(g, {"type": "block"})
    r1, r2 = gm.add_router(g, "R1"), gm.add_router(g, "R2")
    r1["rules"][0].update(match={"domain_suffix": ["ads.example"]}, target=blk["id"])
    r1["rules"].append({"id": "n", "name": "kr", "match": {"geoip": ["kr"]}, "target": r2["id"]})
    r1["rules"].append({"id": "e", "name": "empty", "match": {}, "target": up["id"]})     # no condition: skipped
    r1["default"] = up["id"]
    r2["rules"][0].update(match={"port": ["443"]}, target=d["id"])
    r2["default"] = blk["id"]
    gm.add_inbound(g, "in", {"type": "socks5", "port": 1080}, target=r1["id"])
    p = gm.compile(g)[0]
    got = [(x["name"], [list(m) for m in x["match"]], x["outbound"]["type"]) for x in p["routes"]]
    check("router: flatten rules, nested router, default", got == [
        ("Rule 1", [["domain_suffix"]], "block"),
        ("kr › Rule 1", [["geoip"], ["port"]], "direct"),
        ("kr", [["geoip"]], "block"),
    ] and p["outbound"]["type"] == "socks5" and not p["unlinked"], got)
    check("router: target label", p["target_name"] == "Router R1", p["target_name"])
    r1["default"] = None
    check("router: no default -> not connected", gm.compile(g)[0]["unlinked"])
    r1["default"] = up["id"]
    raw = json.loads(json.dumps(g))
    raw["routers"][1]["default"] = r1["id"]          # R2 -> R1 -> R2 loop
    v = gm.validate(raw)
    check("router: validate cuts loops", not gm.router_loop(v, r1["id"]) and not gm.router_loop(v, r2["id"]))
    check("router: validate round trip", gm.validate(json.loads(json.dumps(g)))["routers"] == g["routers"])
    gm.remove(g, blk["id"])
    check("router: removing a block clears rule links", r1["rules"][0]["target"] is None and r2["default"] is None)


def test_relay_graph():
    """Pass-through blocks: only wiring, compiled away."""
    g = gm.empty()
    d = gm.add_outbound(g, {"type": "direct"}, "Direct")
    blk = gm.add_outbound(g, {"type": "block"}, "Block")
    up = gm.add_outbound(g, {"type": "socks5", "host": "127.0.0.1", "port": 1081}, "Up")
    bus = gm.add_relay(g, "Bus", lanes=2)
    check("relay: new block is straight with matching lanes", not bus["routed"] and len(bus["outputs"]) == 2
          and [x["out"] for x in bus["inputs"]] == [x["id"] for x in bus["outputs"]])
    bus["outputs"][0]["target"] = d["id"]
    bus["outputs"][1]["target"] = blk["id"]
    a = gm.add_inbound(g, "A", {"type": "socks5", "port": 1080}, target=bus["inputs"][0]["id"])
    b = gm.add_inbound(g, "B", {"type": "http", "port": 8080}, target=bus["inputs"][1]["id"])
    ps = {p["name"]: p for p in gm.compile(g)}
    check("relay: straight lanes lead to their outputs", ps["A"]["outbound"]["type"] == "direct"
          and ps["B"]["outbound"]["type"] == "block" and not ps["A"]["unlinked"], ps)
    check("relay: target label shows the real block", ps["A"]["target_name"] == "Direct", ps["A"]["target_name"])
    check("relay: used by looks through lanes", gm.used_by(g, d["id"]) == ["A"] and gm.users(g, blk["id"]) == [b])
    check("relay: lane sources", gm.lane_sources(g, bus["inputs"][0]["id"]) == ["A"])
    check("relay: compile does not change the saved graph", a["target"] == bus["inputs"][0]["id"] and g["relays"])
    # routed: cross and merge
    bus["routed"] = True
    bus["inputs"][1]["out"] = bus["outputs"][0]["id"]
    ps = {p["name"]: p for p in gm.compile(g)}
    check("relay: routed lanes can merge", ps["B"]["outbound"]["type"] == "direct" and sorted(gm.used_by(g, d["id"])) == ["A", "B"])
    bus["inputs"][1]["out"] = None
    check("relay: an unwired input is not connected", {p["name"]: p for p in gm.compile(g)}["B"]["unlinked"])
    # router -> lane -> outbound, lane -> router
    r = gm.add_router(g, "R")
    r["rules"][0].update(match={"domain_suffix": ["example.com"]}, target=bus["inputs"][0]["id"])
    r["default"] = up["id"]
    bus["inputs"][1]["out"] = bus["outputs"][1]["id"]
    bus["outputs"][1]["target"] = r["id"]
    p = {p["name"]: p for p in gm.compile(g)}["B"]
    check("relay: lanes around a router", [x["outbound"]["type"] for x in p["routes"]] == ["direct"]
          and p["outbound"]["type"] == "socks5" and not p["unlinked"], p["routes"])
    check("relay: no router loop yet", not gm.router_loop(g, r["id"]))
    r["default"] = bus["inputs"][1]["id"]               # R -> lane 2 -> R
    check("relay: router loop through a lane", gm.router_loop(g, r["id"]))
    r["default"] = up["id"]
    # validate: round trip, bad values, loops
    v = gm.validate(json.loads(json.dumps(g)))
    check("relay: validate round trip", v["relays"] == g["relays"] and v["inbounds"][0]["target"] == a["target"], v["relays"])
    raw = json.loads(json.dumps(g))
    raw["relays"][0]["outputs"][0]["target"] = raw["relays"][0]["inputs"][0]["id"]     # lane 1 -> itself
    raw["relays"][0]["inputs"][0]["gap"] = 99
    raw["relays"][0]["outputs"][1]["target"] = "missing"
    v = gm.validate(raw)
    check("relay: validate cuts a lane loop", v["relays"][0]["outputs"][0]["target"] is None)
    check("relay: validate drops bad gaps and links", v["relays"][0]["inputs"][0]["gap"] == 0
          and v["relays"][0]["outputs"][1]["target"] is None)
    raw = json.loads(json.dumps(g))
    raw["relays"][0]["routed"] = False
    raw["relays"][0]["inputs"].append({"id": "extra", "gap": 2})
    v = gm.validate(raw)["relays"][0]
    check("relay: straight keeps both sides in step", len(v["outputs"]) == 3 and v["outputs"][2]["gap"] == 2
          and [x["out"] for x in v["inputs"]] == [x["id"] for x in v["outputs"]])
    # lane count, copy, dead links
    c = gm.copy_relay(bus)
    old_ids = {x["id"] for x in bus["inputs"] + bus["outputs"]} | {bus["id"]}
    check("relay: copy has new ids and keeps the wiring", not old_ids & ({x["id"] for x in c["inputs"] + c["outputs"]} | {c["id"]})
          and c["inputs"][1]["out"] == c["outputs"][1]["id"])
    gm.set_lane_count(bus, "inputs", 1)
    gm.drop_dead_lane_links(g)
    check("relay: fewer lanes cut links into the removed ones", b["target"] is None and a["target"] == bus["inputs"][0]["id"])
    gm.set_lane_count(bus, "outputs", gm.MAX_LANES + 5)
    check("relay: lane count is capped", len(bus["outputs"]) == gm.MAX_LANES)
    gm.set_lane_count(bus, "outputs", 2)
    # deleting a pass-through splices its links
    b["target"] = None
    gm.remove(g, bus["id"])
    check("relay: deleting splices the links", a["target"] == d["id"] and r["rules"][0]["target"] == d["id"]
          and not g["relays"], (a["target"], r["rules"][0]["target"]))
    # chained pass-throughs
    x1, x2 = gm.add_relay(g, "X1", lanes=1), gm.add_relay(g, "X2", lanes=1)
    x1["outputs"][0]["target"] = x2["inputs"][0]["id"]
    x2["outputs"][0]["target"] = up["id"]
    a["target"] = x1["inputs"][0]["id"]
    check("relay: chained pass-throughs", {p["name"]: p for p in gm.compile(g)}["A"]["outbound"]["type"] == "socks5")
    x2["outputs"][0]["target"] = x1["inputs"][0]["id"]
    check("relay: chained loop is found", gm.lane_loop(g, x1["inputs"][0]["id"])
          and {p["name"]: p for p in gm.compile(g)}["A"]["unlinked"])


def test_block_colors():
    """Block colours and the link colours they give (pass-throughs hand them on)."""
    check("color: valid values", gm.valid_color("#ABCDEF") == "#abcdef" and gm.valid_color("red") == ""
          and gm.valid_color("#12345g") == "" and gm.valid_color(None) == "")
    g = gm.empty()
    d = gm.add_outbound(g, {"type": "direct"}, "Direct")
    r = gm.add_relay(g, "Bus", lanes=3)
    r["routed"] = True
    r["color"] = "#9aa0a6"
    a = gm.add_inbound(g, "A", {"type": "socks5", "port": 1080}, target=r["inputs"][0]["id"])
    b = gm.add_inbound(g, "B", {"type": "socks5", "port": 1081}, target=r["inputs"][1]["id"])
    a["color"], b["color"], d["color"] = "#e5534b", "#4f8ef7", "#57AB5A"
    o = [x["id"] for x in r["outputs"]]
    r["inputs"][0]["out"], r["inputs"][1]["out"], r["inputs"][2]["out"] = o[0], o[2], o[2]
    rt = gm.add_router(g, "R")
    rt["color"] = "#d9b92f"
    rt["default"] = r["inputs"][2]["id"]
    v = gm.validate(json.loads(json.dumps(g)))
    check("color: kept by validate on every block", v["inbounds"][0]["color"] == "#e5534b" and v["outbounds"][0]["color"] == "#57ab5a"
          and v["relays"][0]["color"] == "#9aa0a6" and v["routers"][0]["color"] == "#d9b92f")
    out_c, in_c = gm.wire_colors(v)
    rv = v["relays"][0]
    check("color: a lane hands on what comes in", out_c[rv["outputs"][0]["id"]] == "#e5534b"
          and in_c[rv["inputs"][1]["id"]] == "#4f8ef7" and in_c[rv["inputs"][2]["id"]] == "#d9b92f")
    check("color: merging colours use the block's own", out_c[rv["outputs"][2]["id"]] == "#9aa0a6")
    check("color: nothing coming in uses the block's own", out_c[rv["outputs"][1]["id"]] == "#9aa0a6")
    b["color"] = rt["color"] = ""
    out_c, _ = gm.wire_colors(gm.validate(json.loads(json.dumps(g))))
    check("color: default colours merge as one", out_c[o[2]] == "", out_c[o[2]])
    x1, x2 = gm.add_relay(g, "X1", lanes=1), gm.add_relay(g, "X2", lanes=1)
    x1["outputs"][0]["target"] = x2["inputs"][0]["id"]
    x2["outputs"][0]["target"] = d["id"]
    a["target"] = x1["inputs"][0]["id"]
    out_c, _ = gm.wire_colors(g)
    check("color: carried through chained pass-throughs", out_c[x2["outputs"][0]["id"]] == "#e5534b")
    x2["outputs"][0]["target"] = x1["inputs"][0]["id"]          # loop: must not hang
    out_c, _ = gm.wire_colors(g)
    check("color: a lane loop does not hang", x2["outputs"][0]["id"] in out_c)


def test_router_singbox():
    from proxydock.singbox import compile as sbc, engine_for
    g = gm.empty()
    d = gm.add_outbound(g, {"type": "direct"}, "Direct")
    up = gm.add_outbound(g, {"type": "http", "host": "127.0.0.1", "port": 3128}, "Up")
    blk = gm.add_outbound(g, {"type": "block"})
    r = gm.add_router(g, "R")
    r["rules"][0].update(name="ads", match={"domain_suffix": ["*.ads.example"], "port": ["80", "8000-9000"]}, target=blk["id"])
    r["rules"].append({"id": "x", "name": "kr", "match": {"geoip": ["kr"], "ip_cidr": ["10.1.2.3"]}, "target": up["id"]})
    r["rules"].append({"id": "y", "name": "app", "match": {"process_name": ["chrome.exe"]}, "target": up["id"]})
    r["default"] = d["id"]
    gm.add_inbound(g, "in", {"type": "socks5", "port": 1080}, target=r["id"])
    p = gm.compile(g)[0]
    cfg = config.resolve(p, config.validate_global({}))
    conf, reasons = sbc.build(p, cfg, bans_path="b.json", api_port=1, api_secret="s", rulesets={"geoip-kr": "kr.srs"})
    rules = conf["route"]["rules"]
    ads = rules[reasons.index(("rule", "ads"))]
    check("router sing-box: block rule -> reject with fields",
          ads == {"domain_suffix": ["ads.example"], "port": [80], "port_range": ["8000:9000"], "action": "reject"}, ads)
    kr = next(x for x in rules if x.get("rule_set") == ["geoip-kr"])
    check("router sing-box: geoip rule-set + cidr -> shared upstream chain",
          kr["ip_cidr"] == ["10.1.2.3/32"] and kr["outbound"] == "r0_0"
          and sum(1 for o in conf["outbounds"] if o["tag"].startswith("r")) == 1, kr)
    check("router sing-box: resolve before IP rules", any(x.get("action") == "resolve" for x in rules[:rules.index(kr)]))
    check("router sing-box: local rule-set listed",
          {"type": "local", "tag": "geoip-kr", "format": "binary", "path": "kr.srs"} in conf["route"]["rule_set"])
    check("router sing-box: router rules after protections", rules.index(ads) > reasons.index(sbc.R_BANNED))
    r["rules"] = r["rules"][2:]      # only the program rule: no IP conditions, so no resolve
    conf2, _ = sbc.build(gm.compile(g)[0], cfg, bans_path="b.json", api_port=1, api_secret="s")
    check("router sing-box: no resolve without IP rules", not any(x.get("action") == "resolve" for x in conf2["route"]["rules"]))
    r["default"] = blk["id"]
    p3 = gm.compile(g)[0]
    conf3, reasons3 = sbc.build(p3, cfg, bans_path="b.json", api_port=1, api_secret="s")
    check("router sing-box: Block default -> final reject", conf3["route"]["rules"][-1] == {"action": "reject"}
          and reasons3[-1] == ("rule", "default"))
    check("router: Python engine setting -> cannot start", engine_for(p3, config.validate_global({"engine": "python"}))[0] == "none")
    from proxydock.runtime import ProxyManager
    mgr = ProxyManager()
    ok = mgr.start(p3, config.validate_global({"engine": "python"})).result(10)
    check("router: start refused with a clear message", not ok and "sing-box" in mgr.state(p3["id"])[1], mgr.state(p3["id"]))
    mgr.stop_all()


def test_router_e2e():
    """SOCKS5 -> router: blocked port, domain via an upstream proxy card, default Direct."""
    from proxydock import common
    from proxydock.runtime import ProxyManager
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    origin2 = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin2.serve_forever, daemon=True).start()
    oport, oport2 = origin.server_address[1], origin2.server_address[1]
    glob = config.validate_global({"engine": "sing-box"})
    mgr = ProxyManager()
    upstream = config.new_proxy("upstream", {"type": "http", "port": free_port()})
    mgr.start(upstream, glob).result(15)
    g = gm.empty()
    d = gm.add_outbound(g, {"type": "direct"}, "Direct")
    up = gm.add_outbound(g, {"type": "http", "host": "127.0.0.1", "port": upstream["inbound"]["port"]}, "Up")
    blk = gm.add_outbound(g, {"type": "block"})
    r = gm.add_router(g, "R")
    r["rules"][0].update(name="no-origin2", match={"port": [str(oport2)]}, target=blk["id"])
    r["rules"].append({"id": "u", "name": "via-up", "match": {"domain": ["localhost"]}, "target": up["id"]})
    r["default"] = d["id"]
    gm.add_inbound(g, "router card", {"type": "socks5", "port": free_port()}, target=r["id"])
    p = gm.compile(g)[0]
    mgr.start(p, glob).result(15)
    check("[sing-box] router: card starts", mgr.state(p["id"])[0] == "running", mgr.state(p["id"]))
    port = p["inbound"]["port"]
    before = mgr.stats_for(upstream["id"]).total
    check("[sing-box] router: default -> Direct", socks5_get(port, "127.0.0.1", oport) == "200")
    time.sleep(0.3)
    check("[sing-box] router: default did not use the upstream", mgr.stats_for(upstream["id"]).total == before)
    check("[sing-box] router: domain rule -> upstream proxy", socks5_get(port, "localhost", oport) == "200")
    time.sleep(0.3)
    check("[sing-box] router: upstream used", mgr.stats_for(upstream["id"]).total > before)
    check("[sing-box] router: port rule -> Block", socks5_get(port, "127.0.0.1", oport2) != "200")
    time.sleep(0.5)
    check("[sing-box] router: block logged with the rule name",
          any("no-origin2" in e[2] and e[3] == p["id"] for e in common.pending_logs))
    mgr.stop_all()
    origin.shutdown()
    origin2.shutdown()


def test_interface():
    """Leaving through a chosen network interface (sing-box bind_interface)."""
    from proxydock.singbox import compile as sbc, engine_for
    g = gm.empty()
    tail = gm.add_outbound(g, {"type": "socks5", "host": "10.0.0.1", "port": 1080, "interface": "Tailscale"}, "Tail")
    head = gm.add_outbound(g, {"type": "shadowsocks", "host": "9.9.9.9", "port": 8388, "interface": "ignored"}, "SS")
    head["via"] = tail["id"]
    gm.add_inbound(g, "in", {"type": "socks5", "port": 1080}, target=head["id"])
    p = gm.compile(g)[0]
    conf, _ = sbc.build(p, config.resolve(p, config.validate_global({})), bans_path="b.json", api_port=1, api_secret="s")
    obs = {o["tag"]: o for o in conf["outbounds"]}
    check("interface: applied to the hop that dials out", obs["out1"].get("bind_interface") == "Tailscale"
          and "bind_interface" not in obs["out0"], obs)
    check("interface: needs sing-box (not run on Python)",
          engine_for(p, config.validate_global({"engine": "python"}))[0] == "none")
    d = gm.add_outbound(g, {"type": "direct", "interface": "Wi-Fi"})
    check("interface: block name shows it", d["name"] == "Direct (Wi-Fi)", d["name"])
    from proxydock.platform_utils import _is_default_route, default_route_ifaces
    check("interface: default route rule", _is_default_route(0, 0) and _is_default_route(0, 0x80)
          and _is_default_route(0x80, 0x80) and not _is_default_route(0x1CE0A8C0, 0x00FFFFFF)
          and not _is_default_route(0x40, 0x80))
    routes = default_route_ifaces()
    check("interface: routing table readable", routes is None or isinstance(routes, set), routes)


def test_interface_e2e():
    from proxydock import common
    from proxydock.runtime import ProxyManager
    glob = config.validate_global({"engine": "sing-box"})
    mgr = ProxyManager()
    p = config.new_proxy("iface", {"type": "socks5", "port": free_port()},
                         {"type": "direct", "interface": "NoSuchAdapter-ProxyDock"})
    mgr.start(p, glob).result(15)
    check("[sing-box] interface: card starts", mgr.state(p["id"])[0] == "running", mgr.state(p["id"]))
    # sing-box does not bind loopback destinations; 192.0.2.1 (TEST-NET) fails at the interface lookup, nothing is sent
    r = socks5_get(p["inbound"]["port"], "192.0.2.1", 9)
    time.sleep(0.5)
    check("[sing-box] interface: missing interface fails with a hint", r != "200"
          and any("network interface does not exist" in e[2] and e[3] == p["id"] for e in common.pending_logs), r)
    mgr.stop_all()


WG_CONF = """[Interface]
PrivateKey = {priv}
Address = 10.66.0.2/32, fd00:66::2/128
DNS = 1.1.1.1
MTU = 1380

[Peer]
PublicKey = {pub}
PresharedKey = {psk}
AllowedIPs = 0.0.0.0/0, ::/0
Endpoint = vpn.example.net:51820
PersistentKeepalive = 25
"""


def test_wireguard():
    from proxydock import wgconf
    from proxydock.singbox import compile as sbc, engine_for
    priv, pub = wgconf.keypair()
    check("wireguard: key pair round trip", wgconf.public_of(priv) == pub and wgconf.valid_key(pub))
    psk = wgconf.keypair()[0]
    ob, notes = wgconf.parse_conf(WG_CONF.format(priv=priv, pub=pub, psk=psk))
    check("wireguard: .conf import", ob["host"] == "vpn.example.net" and ob["port"] == 51820 and ob["wg_mtu"] == 1380
          and ob["wg_keepalive"] == 25 and ob["wg_address"].startswith("10.66.0.2/32") and ob["wg_preshared_key"] == psk
          and any("DNS" in n for n in notes), (ob, notes))
    try:
        wgconf.parse_conf("[Interface]\nAddress = 10.0.0.2/32\n")
        check("wireguard: bad .conf refused", False)
    except ValueError:
        check("wireguard: bad .conf refused", True)
    full = dict(config.OUTBOUND_DEFAULTS, **ob, via=None)
    check("wireguard: valid outbound has no errors", config.outbound_errors(full) == [], config.outbound_errors(full))
    bad = dict(full, wg_private_key="nope", wg_address="10.0.0.2")
    check("wireguard: bad key and address reported", len(config.outbound_errors(bad)) >= 2)
    over_http = dict(full, via=dict(config.OUTBOUND_DEFAULTS, type="http", host="h", port=8080, via=None))
    check("wireguard: cannot go through an HTTP proxy (no UDP)",
          any("UDP" in e for e in config.outbound_errors(over_http)))
    g = gm.empty()
    outer = gm.add_outbound(g, dict(ob), "JP")
    inner = gm.add_outbound(g, dict(ob, host="kr.example.net"), "KR")
    inner["via"] = outer["id"]
    gm.add_inbound(g, "in", {"type": "socks5", "port": 1080}, target=inner["id"])
    p = gm.compile(g)[0]
    conf, _ = sbc.build(p, config.resolve(p, config.validate_global({})), bans_path="b.json", api_port=1, api_secret="s")
    eps = {e["tag"]: e for e in conf.get("endpoints", [])}
    check("wireguard: double VPN -> two endpoints, inner detours through outer",
          set(eps) == {"out0", "out1"} and eps["out0"].get("detour") == "out1"
          and eps["out0"]["peers"][0]["address"] == "kr.example.net"
          and eps["out0"]["peers"][0]["persistent_keepalive_interval"] == 25
          and conf["route"]["final"] == "out0", conf.get("endpoints"))
    check("wireguard: needs sing-box", engine_for(p, config.validate_global({"engine": "python"}))[0] == "none")
    # keys are sealed like passwords when the settings are saved (Windows DPAPI)
    config.save(config.validate_global({}), g)
    raw = open(config.CONFIG_PATH, encoding="utf-8").read()
    check("wireguard: private key not saved in plain text" if os.name == "nt" else "wireguard: settings saved",
          (priv not in raw and psk not in raw) if os.name == "nt" else True)
    _, g2 = config.load()
    check("wireguard: keys survive save/load", g2["outbounds"][0]["outbound"]["wg_private_key"] == priv)


def test_wireguard_e2e():
    """SOCKS5 -> WireGuard (and WireGuard through WireGuard) to local sing-box WireGuard servers."""
    import subprocess
    from proxydock import wgconf
    from proxydock.runtime import ProxyManager
    from proxydock.singbox import find_binary
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    procs = []

    def udp_port():
        u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        u.bind(("127.0.0.1", 0))
        port = u.getsockname()[1]
        u.close()
        return port

    def server(net, client_pub):
        priv, pub = wgconf.keypair()
        port = udp_port()
        conf = {"log": {"level": "warn"}, "endpoints": [{
            "type": "wireguard", "tag": "wg", "listen_port": port, "address": [f"{net}.1/24"], "private_key": priv,
            "peers": [{"public_key": client_pub, "allowed_ips": [f"{net}.2/32"]}]}],
            "outbounds": [{"type": "direct", "tag": "d"}],
            # stands in for the internet behind the VPN server: <net>.99 is this PC's 127.0.0.1
            "route": {"rules": [{"ip_cidr": [f"{net}.99/32"], "action": "route-options", "override_address": "127.0.0.1"}],
                      "final": "d"}}
        path = os.path.join(TMP, f"wg-server-{net}.json")
        json.dump(conf, open(path, "w"))
        procs.append(subprocess.Popen([find_binary(), "run", "-c", path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        return pub, port

    try:
        c1, c2 = wgconf.keypair(), wgconf.keypair()
        s1_pub, s1_port = server("10.91.0", c1[1])
        s2_pub, s2_port = server("10.92.0", c2[1])
        time.sleep(1.5)
        glob = config.validate_global({"engine": "sing-box"})
        mgr = ProxyManager()

        def wg(priv, net, pub, host, port):
            return dict(config.OUTBOUND_DEFAULTS, type="wireguard", host=host, port=port, wg_private_key=priv,
                        wg_address=f"{net}.2/32", wg_peer_public_key=pub, via=None)

        single = config.new_proxy("wg", {"type": "socks5", "port": free_port()})
        single["outbound"] = wg(c1[0], "10.91.0", s1_pub, "127.0.0.1", s1_port)
        mgr.start(single, glob).result(20)
        check("[sing-box] wireguard: card starts", mgr.state(single["id"])[0] == "running", mgr.state(single["id"]))
        check("[sing-box] wireguard: TCP through the tunnel",
              socks5_get(single["inbound"]["port"], "10.91.0.99", oport) == "200")
        double = config.new_proxy("wg2", {"type": "socks5", "port": free_port()})
        # inner tunnel's server is reached through the outer tunnel (10.91.0.99 = this PC behind server 1)
        double["outbound"] = dict(wg(c2[0], "10.92.0", s2_pub, "10.91.0.99", s2_port),
                                  via=wg(c1[0], "10.91.0", s1_pub, "127.0.0.1", s1_port))
        mgr.start(double, glob).result(20)
        check("[sing-box] wireguard: double VPN (WireGuard through WireGuard)",
              socks5_get(double["inbound"]["port"], "10.92.0.99", oport) == "200")
        mgr.stop_all()
    finally:
        for pr in procs:
            pr.terminate()
        origin.shutdown()


def test_tun():
    """TUN cards: config, engine gate, relays, checks (no administrator rights needed)."""
    from proxydock.singbox import compile as sbc, engine_for, guard
    g = gm.empty()
    d = gm.add_outbound(g, {"type": "direct"}, "Direct")
    blk = gm.add_outbound(g, {"type": "block"})
    r = gm.add_router(g, "R")
    r["rules"][0].update(name="games", match={"process_name": ["game.exe"]}, target=blk["id"])
    r["default"] = d["id"]
    tun = gm.add_inbound(g, "TUN", {"type": "tun"}, target=r["id"])
    relay = gm.add_inbound(g, "relay", {"type": "socks5", "port": 1080}, target=tun["id"])
    ps = {x["id"]: x for x in gm.compile(g)}
    p = ps[tun["id"]]
    conf, reasons = sbc.build(p, config.resolve(p, config.validate_global({})), bans_path="b.json", api_port=1,
                              api_secret="s", exclude_paths=["C:/x/sing-box.exe"])
    rules = conf["route"]["rules"]
    check("tun: inbound is a sing-box tun with auto route", conf["inbounds"][0]["type"] == "tun"
          and conf["inbounds"][0]["auto_route"] and conf["route"].get("auto_detect_interface"))
    check("tun: sniff + DNS hijack first, ProxyDock's own engines go Direct",
          rules[0] == {"action": "sniff"} and rules[1] == {"protocol": "dns", "action": "hijack-dns"}
          and rules[2] == {"process_path": ["C:/x/sing-box.exe"], "action": "route", "outbound": sbc.TUN_DIRECT}, rules[:3])
    check("tun: no self-port rule, DNS server set", sbc.R_SELF not in reasons and conf.get("dns", {}).get("servers"))
    check("tun: program rule kept", any(x.get("process_name") == ["game.exe"] for x in rules))
    check("tun: needs sing-box", engine_for(p, config.validate_global({"engine": "python"}))[0] == "none")
    check("tun: no port checks", config.check_proxy(p, list(ps.values())) == [], config.check_proxy(p, list(ps.values())))
    check("tun: another inbound cannot relay into a TUN card", ps[relay["id"]]["unlinked"])

    class Fake:
        inbound = {"type": "tun"}
    check("tun: no per-IP limit (all apps share the TUN address)",
          guard.report_connections(Fake(), [{"id": str(i), "metadata": {"sourceIP": "172.19.0.1"}} for i in range(300)]) == [])
    from proxydock.runtime import ProxyManager
    from proxydock.singbox.process import SingBoxInstance
    mgr = ProxyManager()
    SingBoxInstance.tun_owner = "someone-else"
    try:
        ok = mgr.start(p, config.validate_global({"engine": "sing-box"})).result(15)
    finally:
        SingBoxInstance.tun_owner = None
    check("tun: only one TUN card at a time", not ok and "only one" in mgr.state(p["id"])[1], mgr.state(p["id"]))
    mgr.stop_all()


def test_helper():
    """The administrator helper protocol (same as for TUN, without the UAC prompt)."""
    import proxydock.singbox.process as sp
    from proxydock.runtime import ProxyManager
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    sp.FORCE_HELPER = True
    try:
        mgr = ProxyManager()
        p = config.new_proxy("helper", {"type": "socks5", "port": free_port()})
        mgr.start(p, config.validate_global({"engine": "sing-box"})).result(30)
        inst = mgr.instances.get(p["id"])
        check("[helper] card runs through the helper", inst is not None and type(inst.proc).__name__ == "HelperProc",
              mgr.state(p["id"]))
        check("[helper] traffic through the helper's sing-box",
              socks5_get(p["inbound"]["port"], "127.0.0.1", origin.server_address[1]) == "200")
        time.sleep(1.2)
        check("[helper] stats via the Clash API", mgr.stats_for(p["id"]).down > 0)
        mgr.stop_all()
        check("[helper] stopped", inst.proc is None)
    finally:
        sp.FORCE_HELPER = False
        origin.shutdown()


def test_run_as_admin():
    """Settings 'Run as administrator': relaunch through UAC (the prompt itself is replaced here)."""
    import types
    from proxydock.ui import window as winmod
    from proxydock.singbox import helper
    from proxydock import common
    args = types.SimpleNamespace(elevated=False, hidden=True, restart=False)
    g = config.validate_global({"run_as_admin": True})
    check("admin: setting saved", g["run_as_admin"] is True and config.validate_global({})["run_as_admin"] is False)
    real_runas, real_admin, real_win = helper.runas, common.is_admin, winmod.IS_WIN
    calls = []
    try:
        winmod.IS_WIN = True
        common.is_admin = lambda: False
        check("admin: off -> no relaunch", winmod.relaunch_elevated(config.validate_global({}), args) is None)
        check("admin: already elevated copy -> no relaunch",
              winmod.relaunch_elevated(g, types.SimpleNamespace(elevated=True, hidden=False, restart=False)) is None)

        def cancelled(cmd, show=False):
            raise RuntimeError("cancelled")
        helper.runas = cancelled
        note = winmod.relaunch_elevated(g, args)
        check("admin: UAC cancelled -> keeps running with a note", note is not None and "cancelled" in note, note)
        helper.runas = lambda cmd, show=False: calls.append(cmd)
        try:
            winmod.relaunch_elevated(g, args)
            exited = False
        except SystemExit:
            exited = True
        check("admin: relaunches elevated with --elevated --hidden and exits",
              exited and calls and calls[0][-2:] == ["--elevated", "--hidden"], calls)
    finally:
        helper.runas, common.is_admin, winmod.IS_WIN = real_runas, real_admin, real_win


VLESS_LINK = ("vless://0b5b8d9e-1111-2222-3333-444455556666@a.example:443?security=reality&sni=www.microsoft.com"
              "&fp=chrome&pbk=hDNje6eG6lfpI7mTJVGTTFZmGw7VS0RJRHFwWQzu2iM&sid=6ba85179&flow=xtls-rprx-vision&type=tcp#JP%20Reality")


def test_protocols():
    from proxydock import links
    from proxydock.singbox import compile as sbc, engine_for
    ob, name = links.parse(VLESS_LINK)
    check("links: vless reality", name == "JP Reality" and ob["security"] == "reality" and ob["vless_flow"] == "xtls-rprx-vision"
          and ob["reality_short_id"] == "6ba85179" and ob["sni"] == "www.microsoft.com", ob)
    ob2, _ = links.parse("trojan://p%40ss@b.example:443?sni=b.example&type=ws&path=%2Fws&host=cdn.example&allowInsecure=1#T")
    check("links: trojan over websocket", ob2["password"] == "p@ss" and ob2["transport"] == "ws"
          and ob2["transport_host"] == "cdn.example" and ob2["tls_verify"] is False, ob2)
    ob3, _ = links.parse("tuic://0b5b8d9e-1111-2222-3333-444455556666:pw@[2001:db8::1]:443?congestion_control=cubic&alpn=h3#T")
    check("links: tuic with IPv6 server", ob3["host"] == "2001:db8::1" and ob3["password"] == "pw"
          and ob3["tuic_congestion"] == "cubic", ob3)
    ss, _ = links.parse("ss://YWVzLTI1Ni1nY206cGFzcw@1.2.3.4:8388#SS")
    hy, _ = links.parse("hy2://pw@h.example:443?sni=x&insecure=1#H")
    check("links: ss and hy2", ss["ss_method"] == "aes-256-gcm" and ss["ss_password"] == "pass"
          and hy["type"] == "hysteria2" and hy["tls_verify"] is False)
    for bad in ("http://example.com", "vless://nouser"):
        try:
            links.parse(bad)
            check(f"links: refuses {bad}", False)
        except ValueError:
            check(f"links: refuses {bad}", True)
    full = dict(config.OUTBOUND_DEFAULTS, **ob, via=None)
    check("protocols: valid VLESS/REALITY has no errors", config.outbound_errors(full) == [], config.outbound_errors(full))
    check("protocols: bad UUID and REALITY key reported",
          len(config.outbound_errors(dict(full, uuid="x", reality_public_key="nope"))) >= 2)
    tuic_over_http = dict(config.OUTBOUND_DEFAULTS, **ob3, via=dict(config.OUTBOUND_DEFAULTS, type="http", host="h", port=80, via=None))
    check("protocols: TUIC cannot go through an HTTP proxy", any("UDP" in e for e in config.outbound_errors(tuic_over_http)))
    d = sbc.outbound(full, "out0")
    check("protocols: sing-box vless reality", d["type"] == "vless" and d["flow"] == "xtls-rprx-vision"
          and d["tls"]["reality"]["public_key"] == ob["reality_public_key"] and d["tls"]["utls"]["fingerprint"] == "chrome", d)
    t = sbc.outbound(dict(config.OUTBOUND_DEFAULTS, **ob2, via=None), "out0")
    check("protocols: sing-box trojan ws", t["transport"] == {"type": "ws", "path": "/ws", "headers": {"Host": "cdn.example"}}
          and t["tls"]["insecure"] is True, t)
    u = sbc.outbound(dict(config.OUTBOUND_DEFAULTS, **ob3, via=None), "out0")
    check("protocols: sing-box tuic", u["congestion_control"] == "cubic" and u["tls"]["alpn"] == ["h3"] and "utls" not in u["tls"], u)
    p = config.new_proxy("v", {"type": "socks5", "port": 1080})
    p["outbound"] = full
    check("protocols: need sing-box", engine_for(p, config.validate_global({"engine": "python"}))[0] == "none")


def test_protocols_e2e():
    """SOCKS5 card -> VLESS (TLS, REALITY, WebSocket) / Trojan (TLS, gRPC) / TUIC -> local sing-box servers -> origin."""
    import ssl
    import subprocess
    import uuid as _uuid
    from proxydock.runtime import ProxyManager
    from proxydock.singbox import find_binary
    from proxydock.tls import ensure_self_signed
    exe = find_binary()
    cert, key = ensure_self_signed(TMP, "proto-test")
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    # a TLS 1.3 site for REALITY to imitate
    tls_port = free_port()
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(cert, key)
    lsock = socket.socket()
    lsock.bind(("127.0.0.1", tls_port))
    lsock.listen()

    def tls_site():
        while True:
            try:
                c, _ = lsock.accept()
            except OSError:
                return
            try:
                with ctx.wrap_socket(c, server_side=True) as sc:
                    sc.recv(100)
            except Exception:
                pass
    threading.Thread(target=tls_site, daemon=True).start()
    kp = subprocess.run([exe, "generate", "reality-keypair"], capture_output=True, text=True).stdout
    priv = kp.split("PrivateKey:")[1].split()[0]
    pub = kp.split("PublicKey:")[1].split()[0]
    uid, pw = str(_uuid.uuid4()), "test-pass-123"
    tls_in = {"enabled": True, "certificate_path": cert, "key_path": key}
    servers = {
        "vless-tls": ({"type": "vless", "users": [{"uuid": uid}], "tls": tls_in},
                      {"type": "vless", "uuid": uid, "security": "tls", "tls_verify": False, "sni": "localhost"}),
        "vless-reality": ({"type": "vless", "users": [{"uuid": uid, "flow": "xtls-rprx-vision"}], "tls": {
                              "enabled": True, "server_name": "localhost",
                              "reality": {"enabled": True, "handshake": {"server": "127.0.0.1", "server_port": tls_port},
                                          "private_key": priv, "short_id": ["6ba85179"]}}},
                          {"type": "vless", "uuid": uid, "security": "reality", "vless_flow": "xtls-rprx-vision",
                           "sni": "localhost", "reality_public_key": pub, "reality_short_id": "6ba85179"}),
        "vless-ws": ({"type": "vless", "users": [{"uuid": uid}], "transport": {"type": "ws", "path": "/ws"}},
                     {"type": "vless", "uuid": uid, "security": "none", "transport": "ws", "transport_path": "/ws"}),
        "trojan-tls": ({"type": "trojan", "users": [{"password": pw}], "tls": tls_in},
                       {"type": "trojan", "password": pw, "tls_verify": False, "sni": "localhost"}),
        "trojan-grpc": ({"type": "trojan", "users": [{"password": pw}], "tls": tls_in,
                         "transport": {"type": "grpc", "service_name": "svc"}},
                        {"type": "trojan", "password": pw, "tls_verify": False, "sni": "localhost", "transport": "grpc",
                         "transport_path": "svc"}),
        "vmess-tls": ({"type": "vmess", "users": [{"uuid": uid, "alterId": 0}], "tls": tls_in},
                      {"type": "vmess", "uuid": uid, "security": "tls", "tls_verify": False, "sni": "localhost"}),
        "vmess-ws": ({"type": "vmess", "users": [{"uuid": uid}], "transport": {"type": "ws", "path": "/vm"}},
                     {"type": "vmess", "uuid": uid, "security": "none", "transport": "ws", "transport_path": "/vm",
                      "vmess_cipher": "chacha20-poly1305"}),
        "tuic": ({"type": "tuic", "users": [{"uuid": uid, "password": pw}], "congestion_control": "bbr",
                  "tls": dict(tls_in, alpn=["h3"])},
                 {"type": "tuic", "uuid": uid, "password": pw, "tls_verify": False, "sni": "localhost"}),
    }
    inbounds, clients = [], {}
    for i, (name, (srv, cli)) in enumerate(servers.items()):
        port = free_port()
        inbounds.append(dict(srv, tag=f"s{i}", listen="127.0.0.1", listen_port=port))
        clients[name] = dict(config.OUTBOUND_DEFAULTS, host="127.0.0.1", port=port, fingerprint="chrome", via=None, **cli)
    path = os.path.join(TMP, "proto-servers.json")
    json.dump({"log": {"level": "warn"}, "inbounds": inbounds, "outbounds": [{"type": "direct", "tag": "d"}]}, open(path, "w"))
    srv_proc = subprocess.Popen([exe, "run", "-c", path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(1.5)
    try:
        glob = config.validate_global({"engine": "sing-box"})
        mgr = ProxyManager()
        for name, ob in clients.items():
            errs = config.outbound_errors(ob)
            p = config.new_proxy(name, {"type": "socks5", "port": free_port()})
            p["outbound"] = ob
            mgr.start(p, glob).result(20)
            r = socks5_get(p["inbound"]["port"], "127.0.0.1", oport)
            check(f"[sing-box] protocol {name} -> origin", r == "200" and not errs, (r, errs, mgr.state(p["id"])))
        mgr.stop_all()
    finally:
        srv_proc.terminate()
        lsock.close()
        origin.shutdown()


class SubServer(BaseHTTPRequestHandler):
    body = b""

    def do_GET(self):
        self.send_response(200)
        self.send_header("Content-Length", str(len(self.body)))
        self.end_headers()
        self.wfile.write(self.body)

    def log_message(self, *a):
        pass


def test_subscription():
    import base64
    from proxydock import subscription as sub
    from proxydock.singbox import compile as sbc, engine_for
    text = "\n".join(["ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ@1.2.3.4:8388#Tokyo", "trojan://pw@b.example:443?sni=b.example#Seoul",
                      "vmess://abc", "ss://YWVzLTI1Ni1nY206cGFzc3dvcmQ@1.2.3.5:8388#Tokyo"])
    servers, skipped = sub.parse(base64.b64encode(text.encode()))
    check("subscription: base64 list parsed, unknown skipped, names unique",
          [x["name"] for x in servers] == ["Tokyo", "Seoul", "Tokyo (2)"] and skipped == 1, (servers, skipped))
    plain, _ = sub.parse(text.encode())
    check("subscription: plain list parsed", len(plain) == 3)
    for bad in (b"proxies:\n  - name: x\n", b""):
        try:
            sub.parse(bad)
            check("subscription: Clash entry without a type / empty refused", False)
        except ValueError:
            check("subscription: Clash entry without a type / empty refused", True)
    ob = dict(config.OUTBOUND_DEFAULTS, type="subscription", sub_url="https://x/sub", sub_servers=json.dumps(servers),
              sub_mode="select", sub_selected="Seoul", interface="Wi-Fi", via=None)
    obs = sbc.outbounds(ob)
    check("subscription: selector over all servers, chosen default, interface on each",
          obs[-1]["type"] == "selector" and obs[-1]["default"] == "out0_m1" and len(obs) == 4
          and all(o.get("bind_interface") == "Wi-Fi" for o in obs[:-1]), obs)
    check("subscription: auto = urltest", sbc.outbounds(dict(ob, sub_mode="auto"))[-1]["type"] == "urltest")
    check("subscription: needs a URL and servers", len(config.outbound_errors(dict(ob, sub_url="", sub_servers=""))) == 2)
    p = config.new_proxy("s", {"type": "socks5", "port": 1080})
    p["outbound"] = ob
    check("subscription: needs sing-box", engine_for(p, config.validate_global({"engine": "python"}))[0] == "none")
    g = gm.empty()
    n = gm.add_outbound(g, ob)
    config.save(config.validate_global({}), g)
    raw = open(config.CONFIG_PATH, encoding="utf-8").read()
    check("subscription: URL and server list not saved in plain text" if os.name == "nt" else "subscription: saved",
          ("https://x/sub" not in raw and "1.2.3.4" not in raw) if os.name == "nt" else True)
    _, g2 = config.load()
    check("subscription: survives save/load", sub.servers_of(g2["outbounds"][0]["outbound"])[1]["name"] == "Seoul")


def test_subscription_e2e():
    """A local subscription URL listing two Shadowsocks servers (ProxyDock cards); pick mode and fastest mode."""
    import base64
    from proxydock import subscription as sub
    from proxydock.runtime import ProxyManager
    from proxydock.ss import ss_generate_password, ss_link
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    glob = config.validate_global({"engine": "sing-box"})
    mgr = ProxyManager()
    servers = []
    for name in ("Alpha", "Beta"):
        key = ss_generate_password("2022-blake3-aes-128-gcm")
        sp = config.new_proxy(name, {"type": "shadowsocks", "port": free_port(), "ss_method": "2022-blake3-aes-128-gcm",
                                     "ss_password": key})
        mgr.start(sp, glob).result(15)
        servers.append(sp)
    links = "\n".join(ss_link("127.0.0.1", x["inbound"]["port"], "2022-blake3-aes-128-gcm", x["inbound"]["ss_password"], x["name"])
                      for x in servers)
    SubServer.body = base64.b64encode(links.encode())
    web = ThreadingHTTPServer(("127.0.0.1", 0), SubServer)
    threading.Thread(target=web.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{web.server_address[1]}/sub?token=secret"
    fields, note = sub.update({"sub_url": url})
    check("[sing-box] subscription: downloaded from the URL", len(sub.servers_of(fields)) == 2, note)
    ob = dict(config.OUTBOUND_DEFAULTS, type="subscription", sub_url=url, sub_mode="select", sub_selected="Beta",
              sub_test_url=f"http://127.0.0.1:{oport}/", via=None, **fields)
    card = config.new_proxy("sub card", {"type": "socks5", "port": free_port()})
    card["outbound"] = ob
    mgr.start(card, glob).result(20)
    before = [mgr.stats_for(x["id"]).total for x in servers]
    check("[sing-box] subscription: picked server -> origin", socks5_get(card["inbound"]["port"], "127.0.0.1", oport) == "200",
          mgr.state(card["id"]))
    time.sleep(0.5)
    after = [mgr.stats_for(x["id"]).total for x in servers]
    check("[sing-box] subscription: the picked server (Beta) was used", after[1] > before[1] and after[0] == before[0],
          (before, after))
    card2 = config.new_proxy("sub auto", {"type": "socks5", "port": free_port()})
    card2["outbound"] = dict(ob, sub_mode="auto")
    mgr.start(card2, glob).result(20)
    check("[sing-box] subscription: fastest mode -> origin", socks5_get(card2["inbound"]["port"], "127.0.0.1", oport) == "200")
    SubServer.body = ("proxies:\n" + "".join(
        f'  - {{name: "{x["name"]} Y", type: ss, server: 127.0.0.1, port: {x["inbound"]["port"]}, '
        f'cipher: 2022-blake3-aes-128-gcm, password: "{x["inbound"]["ss_password"]}"}}\n' for x in servers)).encode()
    fields, note = sub.update({"sub_url": url})
    card3 = config.new_proxy("sub clash", {"type": "socks5", "port": free_port()})
    card3["outbound"] = dict(ob, sub_mode="select", sub_selected="Alpha Y", **fields)
    mgr.start(card3, glob).result(20)
    check("[sing-box] subscription: Clash YAML -> origin", len(sub.servers_of(fields)) == 2
          and socks5_get(card3["inbound"]["port"], "127.0.0.1", oport) == "200", note)
    mgr.stop_all()
    web.shutdown()
    origin.shutdown()


def tls_test_server():
    """A TLS server on 127.0.0.1 with a certificate for 'localhost' (returns port, cert, key)."""
    import ssl
    from proxydock.tls import ensure_self_signed
    cert, key = ensure_self_signed(TMP, "detect-test")
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(cert, key)
    ls = socket.socket()
    ls.bind(("127.0.0.1", 0))
    ls.listen()

    def run():
        while True:
            try:
                c, _ = ls.accept()
            except OSError:
                return
            try:
                with ctx.wrap_socket(c, server_side=True) as sc:
                    sc.recv(10)
            except Exception:
                pass
    threading.Thread(target=run, daemon=True).start()
    return ls.getsockname()[1], cert, key


def test_detect_sni():
    """'Detect from server': the certificate's name for the SNI, over TLS/TCP and QUIC."""
    import hashlib
    import ssl
    import subprocess
    from proxydock import certinfo
    from proxydock.singbox import find_binary
    port, cert, key = tls_test_server()
    r = certinfo.detect(dict(config.OUTBOUND_DEFAULTS, type="trojan", host="127.0.0.1", port=port))
    der = ssl.PEM_cert_to_DER_cert(open(cert, encoding="ascii").read())
    check("detect: TLS server name and fingerprint", r["sni"] == "localhost" and r["sha256"] == hashlib.sha256(der).hexdigest(), r)
    try:
        certinfo.detect(dict(config.OUTBOUND_DEFAULTS, type="https", host="127.0.0.1", port=free_port()))
        check("detect: closed port reported", False)
    except ValueError:
        check("detect: closed port reported", True)
    if find_binary():
        u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        u.bind(("127.0.0.1", 0))
        qport = u.getsockname()[1]
        u.close()
        conf = {"log": {"level": "warn"}, "inbounds": [{"type": "hysteria2", "listen": "127.0.0.1", "listen_port": qport,
                "users": [{"password": "x"}], "tls": {"enabled": True, "certificate_path": cert, "key_path": key}}],
                "outbounds": [{"type": "direct"}]}
        path = os.path.join(TMP, "detect-hy2.json")
        json.dump(conf, open(path, "w"))
        pr = subprocess.Popen([find_binary(), "run", "-c", path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1.5)
        try:
            q = certinfo.detect(dict(config.OUTBOUND_DEFAULTS, type="hysteria2", host="127.0.0.1", port=qport))
            check("detect: QUIC (Hysteria2) server name", q["sni"] == "localhost", q)
        finally:
            pr.terminate()


def test_hy2_advanced():
    from proxydock import links
    from proxydock.singbox import compile as sbc, engine_for
    ob, _ = links.parse("hysteria2://pw@h.example:443,5000-6000/?obfs=salamander&obfs-password=s3cret&sni=h.example#H")
    check("hy2 adv: link with Salamander and port hopping", ob["hy2_obfs_password"] == "s3cret" and ob["port"] == 443
          and ob["hy2_ports"] == "443,5000-6000", ob)
    ob6, _ = links.parse("hy2://pw@[2001:db8::1]:443?mport=20000-30000#V6")
    check("hy2 adv: IPv6 + mport", ob6["host"] == "2001:db8::1" and ob6["hy2_ports"] == "20000-30000", ob6)
    try:
        links.parse("hy2://pw@h:443?obfs=other")
        check("hy2 adv: unknown obfuscation refused", False)
    except ValueError:
        check("hy2 adv: unknown obfuscation refused", True)
    check("hy2 adv: port list parsing", config.hop_ports("443, 5000-6000") == ["443:443", "5000:6000"]
          and config.hop_ports("70000") is None and config.hop_ports("9-1") is None)
    full = dict(config.OUTBOUND_DEFAULTS, **ob, hy2_up_mbps=20, hy2_down_mbps=100, via=None)
    d = sbc.outbound(full, "o")
    check("hy2 adv: sing-box outbound fields", d["obfs"] == {"type": "salamander", "password": "s3cret"}
          and d["up_mbps"] == 20 and d["down_mbps"] == 100 and d["server_ports"] == ["443:443", "5000:6000"]
          and d["hop_interval"] == "30s", d)
    check("hy2 adv: bad hop range reported", any("hopping" in e for e in config.outbound_errors(dict(full, hy2_ports="x"))))
    ib = dict(config.INBOUND_DEFAULTS, type="hysteria2", hy2_password="pw123456", hy2_obfs_password="o", hy2_up_mbps=10)
    si = sbc.inbound(ib, ("c", "k"))
    check("hy2 adv: sing-box server fields", si["obfs"]["password"] == "o" and si["up_mbps"] == 10 and "down_mbps" not in si)
    p = config.new_proxy("h", {"type": "socks5", "port": 1080})
    p["outbound"] = full
    check("hy2 adv: needs sing-box", engine_for(p, config.validate_global({"engine": "python"}))[0] == "none")
    srv = config.new_proxy("hs", dict(ib, port=4443))
    check("hy2 adv: server options need sing-box too", engine_for(srv, config.validate_global({"engine": "python"}))[0] == "none")


def test_hy2_advanced_e2e():
    """Salamander + bandwidth on both ends, port hopping on the client, on sing-box and on the official core."""
    from proxydock import singbox
    from proxydock.runtime import ProxyManager
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    glob = config.validate_global({"engine": "sing-box"})
    mgr = ProxyManager()

    def client(port, obfs, hop=""):
        c = config.new_proxy("hy2 client", {"type": "socks5", "port": free_port()})
        c["outbound"] = dict(config.OUTBOUND_DEFAULTS, type="hysteria2", host="127.0.0.1", port=port, hy2_password="hy2-adv-pass",
                             tls_verify=False, hy2_obfs_password=obfs, hy2_up_mbps=50, hy2_down_mbps=200,
                             hy2_ports=hop, hy2_hop_interval=10, via=None)
        mgr.start(c, glob).result(20)
        try:
            return socks5_get(c["inbound"]["port"], "127.0.0.1", oport)
        except OSError as e:          # (a wrong Salamander password may just time out)
            return repr(e)

    def server(name):
        sp = config.new_proxy(name, {"type": "hysteria2", "port": free_port(), "hy2_password": "hy2-adv-pass",
                                     "hy2_obfs_password": "obfs-secret", "hy2_up_mbps": 500, "hy2_down_mbps": 500})
        mgr.start(sp, glob).result(20)
        return sp

    try:
        s1 = server("hy2 sing-box server")
        check("[sing-box] hy2 adv: server card runs on sing-box", getattr(mgr.instances.get(s1["id"]), "engine", "") == "sing-box")
        port = s1["inbound"]["port"]
        check("[sing-box] hy2 adv: Salamander + Brutal -> origin", client(port, "obfs-secret") == "200")
        check("[sing-box] hy2 adv: port hopping range -> origin", client(port, "obfs-secret", f"{port}-{port}") == "200")
        check("[sing-box] hy2 adv: wrong Salamander password fails", client(port, "wrong") != "200")
        from proxydock.singbox import find_hysteria
        if find_hysteria():
            singbox.FORCE_HYSTERIA = True
            try:
                s2 = server("hy2 official server")
                check("[hysteria] hy2 adv: official core server with Salamander",
                      getattr(mgr.instances.get(s2["id"]), "engine", "") == "hysteria"
                      and client(s2["inbound"]["port"], "obfs-secret") == "200")
            finally:
                singbox.FORCE_HYSTERIA = False
    finally:
        mgr.stop_all()
        origin.shutdown()


def test_group():
    from proxydock import graph as gm
    from proxydock.singbox import compile as sbc, engine_for
    g = gm.empty()
    a = gm.add_outbound(g, {"type": "socks5", "host": "a.example", "port": 1080}, "A")
    hop = gm.add_outbound(g, {"type": "http", "host": "corp", "port": 8080}, "Corp")
    a["via"] = hop["id"]
    b = gm.add_outbound(g, {"type": "direct"}, "B")
    blk = gm.add_outbound(g, {"type": "block"}, "Blk")
    grp = gm.add_outbound(g, {"type": "group", "group_mode": "failover"}, "G")
    other = gm.add_outbound(g, {"type": "group"}, "G2")
    grp["members"] = [a["id"], b["id"], blk["id"], other["id"], grp["id"], "missing"]
    other["members"] = [b["id"]]
    s5 = gm.add_outbound(g, {"type": "socks5", "host": "x", "port": 1}, "S5")
    s5["via"] = grp["id"]
    gm.add_inbound(g, "in", {"type": "socks5", "port": 1080}, target=grp["id"])
    g = gm.validate(json.loads(json.dumps(g)))
    G = gm.find(g, grp["id"])[1]
    check("group: only plain outbound members are kept", G["members"] == [a["id"], b["id"]], G["members"])
    check("group: nothing goes via a group", gm.find(g, s5["id"])[1]["via"] is None)
    p = gm.compile(g)[0]
    check("group: compiled members with their chains", [m["name"] for m in p["outbound"]["members"]] == ["A", "B"]
          and p["outbound"]["members"][0]["outbound"]["via"]["host"] == "corp")
    conf, _ = sbc.build(p, config.resolve(p, config.validate_global({})), bans_path="b.json", api_port=1, api_secret="s")
    tags = {o["tag"]: o for o in conf["outbounds"]}
    ut = tags.get("out0", {})
    check("group: urltest over the member chains", ut.get("type") == "urltest" and ut["outbounds"] == ["out0_m0_0", "out0_m1_0"]
          and tags["out0_m0_0"]["detour"] == "out0_m0_1" and ut["tolerance"] == sbc.FAILOVER_TOLERANCE, ut)
    G["outbound"]["group_mode"] = "auto"
    G["outbound"]["group_tolerance"] = 80
    p = gm.compile(g)[0]
    conf, _ = sbc.build(p, config.resolve(p, config.validate_global({})), bans_path="b.json", api_port=1, api_secret="s")
    check("group: fastest mode uses the tolerance", next(o for o in conf["outbounds"] if o["tag"] == "out0")["tolerance"] == 80)
    check("group: needs sing-box", engine_for(p, config.validate_global({"engine": "python"}))[0] == "none")
    gm.find(g, a["id"])[1]["outbound"]["type"] = "https"
    gm.find(g, a["id"])[1]["outbound"]["pin_sha256"] = "ab" * 32
    p = gm.compile(g)[0]
    check("group: a pinned member is noticed", "1.15" in engine_for(p, config.validate_global({}))[1])
    r = gm.add_router(g, "R")
    r["rules"][0].update(match={"domain": ["x.example"]}, target=grp["id"])
    r["default"] = b["id"]
    g["inbounds"][0]["target"] = r["id"]
    gm.find(g, a["id"])[1]["outbound"].update(type="socks5", pin_sha256="")
    p = gm.compile(g)[0]
    conf, _ = sbc.build(p, config.resolve(p, config.validate_global({})), bans_path="b.json", api_port=1, api_secret="s")
    check("group: router rule -> group", any(o["tag"] == "r0_0" and o["type"] == "urltest" for o in conf["outbounds"])
          and any(x.get("outbound") == "r0_0" for x in conf["route"]["rules"]))
    gm.remove(g, b["id"])
    check("group: deleting a member removes it from the group", gm.find(g, grp["id"])[1]["members"] == [a["id"]])
    empty_grp = dict(config.OUTBOUND_DEFAULTS, type="group", members=[], via=None)
    check("group: no members is an error", any("members" in e for e in config.outbound_errors(empty_grp)))


def test_group_e2e():
    """Group of two Shadowsocks servers (ProxyDock cards): failover skips a dead first member and otherwise keeps
    the first one; fastest mode works."""
    from proxydock.runtime import ProxyManager
    from proxydock.ss import ss_generate_password
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    glob = config.validate_global({"engine": "sing-box"})
    mgr = ProxyManager()
    try:
        servers, members = [], []
        for name in ("Alpha", "Beta"):
            key = ss_generate_password("2022-blake3-aes-128-gcm")
            sp = config.new_proxy(name, {"type": "shadowsocks", "port": free_port(), "ss_method": "2022-blake3-aes-128-gcm",
                                         "ss_password": key})
            mgr.start(sp, glob).result(15)
            servers.append(sp)
            members.append({"id": name, "name": name, "outbound": dict(
                config.OUTBOUND_DEFAULTS, type="shadowsocks", host="127.0.0.1", port=sp["inbound"]["port"],
                ss_method="2022-blake3-aes-128-gcm", ss_password=key, via=None)})
        dead = {"id": "Dead", "name": "Dead", "outbound": dict(members[0]["outbound"], port=free_port())}

        def card(mode, mems):
            c = config.new_proxy(f"group {mode}", {"type": "socks5", "port": free_port()})
            c["outbound"] = dict(config.OUTBOUND_DEFAULTS, type="group", group_mode=mode, members=mems,
                                 group_test_url=f"http://127.0.0.1:{oport}/", via=None)
            mgr.start(c, glob).result(20)
            time.sleep(1.5)         # first measurement
            return c

        c1 = card("failover", [members[0], members[1]])
        before = [mgr.stats_for(x["id"]).total for x in servers]
        ok = socks5_get(c1["inbound"]["port"], "127.0.0.1", oport) == "200"
        time.sleep(0.5)
        after = [mgr.stats_for(x["id"]).total for x in servers]
        check("[sing-box] group: failover -> origin through the first member", ok and after[0] > before[0] and after[1] == before[1],
              (ok, before, after))
        c2 = card("failover", [dead, members[1]])
        check("[sing-box] group: failover skips a dead first member", socks5_get(c2["inbound"]["port"], "127.0.0.1", oport) == "200")
        c3 = card("auto", [dead, members[0], members[1]])
        check("[sing-box] group: fastest mode -> origin", socks5_get(c3["inbound"]["port"], "127.0.0.1", oport) == "200")
    finally:
        mgr.stop_all()
        origin.shutdown()


def test_dns():
    from proxydock.singbox import compile as sbc, engine_for
    spec = config.dns_server_spec
    check("dns: server formats", spec("") == {} and spec("1.1.1.1") == {"type": "udp", "server": "1.1.1.1"}
          and spec("tls://dns.google") == {"type": "tls", "server": "dns.google"}
          and spec("https://cloudflare-dns.com/dns-query") == {"type": "https", "server": "cloudflare-dns.com", "path": "/dns-query"}
          and spec("tcp://[2606:4700::1111]:5353") == {"type": "tcp", "server": "2606:4700::1111", "server_port": 5353}
          and spec("h3://dns.google") == {"type": "h3", "server": "dns.google", "path": "/dns-query"})
    check("dns: bad server refused", spec("ftp://x") is None and spec("tls://dns.google/x") is None and spec("1.1.1.1:99999") is None)
    glob = config.validate_global({"dns_server": "https://dns.google/dns-query", "dns_via_outbound": True})
    check("dns: bad global value falls back", config.validate_global({"dns_server": "nope://"})["dns_server"] == "")

    def build(p, g=glob):
        return sbc.build(p, config.resolve(p, g), bans_path="b.json", api_port=1, api_secret="s")[0]

    p = config.new_proxy("d", {"type": "socks5", "port": 1080})
    p["outbound"] = dict(config.OUTBOUND_DEFAULTS, type="socks5", host="up.example", port=1080, via=None)
    c = build(p)
    srv = next(x for x in c["dns"]["servers"] if x["tag"] == "dns")
    check("dns: DoH server through the outbound", srv["type"] == "https" and srv["domain_resolver"] == "local"
          and srv.get("detour") == "out0" and c["route"]["default_domain_resolver"] == "dns", c["dns"])
    p2 = config.new_proxy("d2", {"type": "socks5", "port": 1081})
    c2 = build(p2)
    check("dns: Direct does not detour", "detour" not in next(x for x in c2["dns"]["servers"] if x["tag"] == "dns"))
    check("dns: no section without a server", "dns" not in build(p2, config.validate_global({})))
    t = config.new_proxy("t", {"type": "tun", "port": 30300, "tun_fakeip": True})
    t["overrides"]["block_private"] = True
    t["routes"] = [{"match": [{"ip_cidr": ["10.0.0.0/8"]}], "outbound": {"type": "block"}, "name": "lan", "block": "b"}]
    ct = build(t, config.validate_global({}))
    resolves = [r for r in ct["route"]["rules"] if r.get("action") == "resolve"]
    check("dns: TUN Fake-IP", any(x["type"] == "fakeip" for x in ct["dns"]["servers"])
          and ct["dns"]["rules"][0]["server"] == "fakeip" and ct["dns"]["final"] == "local"
          and resolves and all(r.get("server") == "local" for r in resolves)
          and not any("198.18.0.0/15" in r.get("ip_cidr", []) for r in ct["route"]["rules"]), ct["dns"])
    py = config.validate_global({"engine": "python", "dns_server": "1.1.1.1"})
    check("dns: own DNS server needs sing-box", engine_for(p2, py)[0] == "none")
    bad = config.new_proxy("bad", {"type": "socks5", "port": 1082})
    bad["overrides"]["dns_server"] = "x://"
    check("dns: bad local server reported", any("DNS server" in e for e in config.check_proxy(bad, [])))


class _DnsServer:
    """UDP + TCP DNS on 127.0.0.1 answering proxydock.test -> 127.0.0.1 (everything else: no answer)."""

    def __init__(self):
        self.queries = []
        import random
        for _ in range(300):        # one port number free for both TCP and UDP (Windows reserves port blocks per
            self.port = random.randint(20000, 60000)    # protocol, so ask for a number, not an ephemeral port)
            self.t, self.u = socket.socket(), socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            try:
                self.t.bind(("127.0.0.1", self.port))
                self.u.bind(("127.0.0.1", self.port))
                break
            except OSError:
                self.u.close()
                self.t.close()
        self.t.listen()
        threading.Thread(target=self._udp, daemon=True).start()
        threading.Thread(target=self._tcp, daemon=True).start()

    def answer(self, q):
        tid = struct.unpack(">H", q[:2])[0]
        i, labels = 12, []
        while q[i]:
            labels.append(q[i + 1:i + 1 + q[i]].decode())
            i += q[i] + 1
        qtype = struct.unpack(">H", q[i + 1:i + 3])[0]
        name = ".".join(labels)
        self.queries.append(name)
        ans = b""
        if name == "proxydock.test" and qtype == 1:
            ans = b"\xc0\x0c" + struct.pack(">HHIH", 1, 1, 60, 4) + socket.inet_aton("127.0.0.1")
        return struct.pack(">HHHHHH", tid, 0x8180, 1, 1 if ans else 0, 0, 0) + q[12:i + 5] + ans

    def _udp(self):
        while True:
            try:
                q, a = self.u.recvfrom(2048)
                self.u.sendto(self.answer(q), a)
            except OSError:
                return

    def _tcp(self):
        while True:
            try:
                c, _ = self.t.accept()
            except OSError:
                return
            try:
                n = struct.unpack(">H", c.recv(2))[0]
                r = self.answer(c.recv(n))
                c.sendall(struct.pack(">H", len(r)) + r)
            except (OSError, struct.error):
                pass
            finally:
                c.close()

    def close(self):
        self.u.close()
        self.t.close()


def test_dns_e2e():
    """A card's own DNS server (a local test server knowing proxydock.test) and DNS through the card's outbound."""
    from proxydock.runtime import ProxyManager
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    dns = _DnsServer()
    glob = config.validate_global({"engine": "sing-box"})
    mgr = ProxyManager()
    try:
        c = config.new_proxy("dns direct", {"type": "socks5", "port": free_port()})
        c["overrides"]["dns_server"] = f"127.0.0.1:{dns.port}"
        mgr.start(c, glob).result(20)
        check("[sing-box] dns: Direct uses the card's DNS server", socks5_get(c["inbound"]["port"], "proxydock.test", oport) == "200"
              and "proxydock.test" in dns.queries, dns.queries)
        up = config.new_proxy("dns upstream", {"type": "socks5", "port": free_port()})
        mgr.start(up, glob).result(20)
        g = gm.empty()
        d = gm.add_outbound(g, {"type": "direct"}, "D")
        u = gm.add_outbound(g, {"type": "socks5", "host": "127.0.0.1", "port": up["inbound"]["port"]}, "U")
        r = gm.add_router(g, "R")
        r["rules"][0].update(match={"ip_cidr": ["127.0.0.0/8"]}, target=d["id"])
        r["default"] = u["id"]
        gm.add_inbound(g, "dns via", {"type": "socks5", "port": free_port()}, target=r["id"])
        p = gm.compile(g)[0]
        p["overrides"].update(dns_server=f"tcp://127.0.0.1:{dns.port}", dns_via_outbound=True)
        mgr.start(p, glob).result(20)
        dns.queries.clear()
        before = mgr.stats_for(up["id"]).total
        ok = socks5_get(p["inbound"]["port"], "proxydock.test", oport) == "200"
        time.sleep(1.5)
        check("[sing-box] dns: lookup went through the outbound, traffic by the IP rule",
              ok and "proxydock.test" in dns.queries and mgr.stats_for(up["id"]).total > before,
              (ok, dns.queries, before, mgr.stats_for(up["id"]).total))
    finally:
        mgr.stop_all()
        dns.close()
        origin.shutdown()


def test_review_fixes():
    import base64
    from proxydock import links
    from proxydock.singbox import compile as sbc
    from proxydock.singbox.process import SingBoxInstance
    vm = {"v": "2", "ps": "JP vm", "add": "vm.example", "port": "443", "id": "0b5b8d9e-1111-2222-3333-444455556666",
          "aid": "0", "scy": "auto", "net": "ws", "type": "none", "host": "cdn.example", "path": "/ray", "tls": "tls",
          "sni": "vm.example", "fp": "chrome"}
    ob, name = links.parse("vmess://" + base64.b64encode(json.dumps(vm).encode()).decode())
    check("review: vmess:// link", name == "JP vm" and ob["type"] == "vmess" and ob["security"] == "tls"
          and ob["transport"] == "ws" and ob["transport_host"] == "cdn.example" and ob["sni"] == "vm.example", ob)
    full = dict(config.OUTBOUND_DEFAULTS, **ob, via=None)
    d = sbc.outbound(full, "o")
    check("review: vmess sing-box outbound", d["type"] == "vmess" and d["tls"]["server_name"] == "vm.example"
          and d["transport"]["type"] == "ws" and d["alter_id"] == 0 and not config.outbound_errors(full), d)
    check("review: vmess REALITY refused", any("VMess" in e for e in config.outbound_errors(dict(full, security="reality"))))
    try:
        links.parse("vmess://" + base64.b64encode(json.dumps(dict(vm, net="tcp", type="http")).encode()).decode())
        check("review: vmess HTTP header obfuscation refused", False)
    except ValueError:
        check("review: vmess HTTP header obfuscation refused", True)
    p = config.new_proxy("r", {"type": "socks5", "port": 1080})
    glob = config.validate_global({})
    inst = SingBoxInstance.__new__(SingBoxInstance)
    inst.proxy, inst.cfg = p, config.resolve(p, glob)
    check("review: changing the DNS server restarts a running card",
          inst.needs_restart(p, config.validate_global({"dns_server": "1.1.1.1"})) and not inst.needs_restart(p, glob))
    g = gm.empty()
    a = gm.add_outbound(g, {"type": "direct"}, "A")
    grp = gm.add_outbound(g, {"type": "group"}, "G")
    grp["members"] = [a["id"]]
    r = gm.add_router(g, "R")
    r["default"] = a["id"]
    gm.add_inbound(g, "In", {"type": "socks5", "port": 1080}, target=a["id"])
    check("review: used-by lists inbounds, groups and routers", sorted(gm.used_by(g, a["id"])) == ["G", "In", "R"])


def test_close_log_e2e():
    """sing-box engine: close details in the log, and the TCP idle timeout closes a silent connection."""
    from proxydock import common
    from proxydock.runtime import ProxyManager
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    silent = socket.socket()
    silent.bind(("127.0.0.1", 0))
    silent.listen()
    held = []
    threading.Thread(target=lambda: held.append(silent.accept()), daemon=True).start()
    glob = config.validate_global({"engine": "sing-box"})
    mgr = ProxyManager()
    try:
        c = config.new_proxy("close log", {"type": "socks5", "port": free_port()})
        c["overrides"]["log_close"] = True
        mgr.start(c, glob).result(20)
        s1 = socket.create_connection(("127.0.0.1", c["inbound"]["port"]), timeout=10)
        s1.sendall(b"\x05\x01\x00")
        s1.recv(2)
        s1.sendall(b"\x05\x01\x00\x01\x7f\x00\x00\x01" + struct.pack("!H", oport))
        s1.recv(10)
        time.sleep(1.5)             # seen by a poll
        s1.sendall(b"GET / HTTP/1.0\r\nHost: x\r\n\r\n")
        while s1.recv(65536):
            pass
        s1.close()
        time.sleep(2)
        lines = [e[2] for e in common.pending_logs if e[3] == c["id"]]
        check("[sing-box] close details logged", any(" closed · " in x and "↓" in x for x in lines), lines[-5:])
        inst = mgr.instances[c["id"]]
        inst.cfg["timeout"] = 2     # (the setting's minimum is 10 s; shortened for the test)
        s2 = socket.create_connection(("127.0.0.1", c["inbound"]["port"]), timeout=10)
        s2.sendall(b"\x05\x01\x00")
        s2.recv(2)
        s2.sendall(b"\x05\x01\x00\x01\x7f\x00\x00\x01" + struct.pack("!H", silent.getsockname()[1]))
        s2.recv(10)
        s2.settimeout(8)
        t0 = time.monotonic()
        try:
            closed = s2.recv(10) == b""
        except (socket.timeout, OSError):
            closed = False
        took = time.monotonic() - t0
        s2.close()
        time.sleep(1.2)
        lines = [e[2] for e in common.pending_logs if e[3] == c["id"]]
        check("[sing-box] idle TCP connection closed after the timeout", closed and took < 7
              and any("idle timeout" in x for x in lines), (closed, took, lines[-3:]))
    finally:
        mgr.stop_all()
        silent.close()
        origin.shutdown()


CLASH_YAML = """
port: 7890
mode: rule
proxies:
  - {name: "SS Tokyo", type: ss, server: 1.2.3.4, port: 8388, cipher: aes-256-gcm, password: "pw"}
  - name: VMess WS
    type: vmess
    server: vm.example
    port: 443
    uuid: 0b5b8d9e-1111-2222-3333-444455556666
    alterId: 0
    cipher: auto
    tls: true
    servername: vm.example
    network: ws
    ws-opts: {path: /ray, headers: {Host: cdn.example}}
  - name: Reality
    type: vless
    server: r.example
    port: 443
    uuid: 0b5b8d9e-1111-2222-3333-444455556666
    flow: xtls-rprx-vision
    tls: true
    servername: www.example.com
    client-fingerprint: chrome
    reality-opts: {public-key: "7RqHNlM-E9C5dZIq6lSVKiJxbK3n5mYXpHFuNWtLzE8", short-id: "6ba85179"}
  - {name: Trojan gRPC, type: trojan, server: t.example, port: 443, password: pw, sni: t.example, network: grpc,
     grpc-opts: {grpc-service-name: svc}}
  - {name: Hy2, type: hysteria2, server: h.example, port: 443, password: pw, obfs: salamander, obfs-password: ob,
     up: "30 Mbps", down: 200, ports: "20000-30000"}
  - {name: TUIC, type: tuic, server: q.example, port: 443, uuid: 0b5b8d9e-1111-2222-3333-444455556666, password: pw,
     congestion-controller: cubic, alpn: [h3]}
  - {name: WG, type: wireguard, server: w.example, port: 51820, ip: 10.0.0.2, private-key: "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=",
     public-key: "xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=", mtu: 1280}
  - {name: SSR, type: ssr, server: x.example, port: 1, cipher: none, password: x, protocol: origin, obfs: plain}
  - {name: SS plugin, type: ss, server: 1.2.3.5, port: 8388, cipher: aes-256-gcm, password: pw, plugin: obfs}
proxy-groups:
  - {name: Auto, type: url-test, proxies: [SS Tokyo, Hy2]}
rules:
  - MATCH,Auto
"""


def test_clash():
    from proxydock import clash, subscription as sub
    check("clash: detected", clash.looks_like_clash(CLASH_YAML) and not clash.looks_like_clash("ss://x\nvless://y"))
    servers, skipped = sub.parse(CLASH_YAML.encode())
    by = {x["name"]: x["outbound"] for x in servers}
    check("clash: supported proxies read, others skipped", len(servers) == 7 and skipped == 2, (list(by), skipped))
    vm = by.get("VMess WS", {})
    check("clash: vmess ws+tls", vm.get("type") == "vmess" and vm["security"] == "tls" and vm["transport_host"] == "cdn.example"
          and vm["transport_path"] == "/ray" and vm["sni"] == "vm.example", vm)
    r = by.get("Reality", {})
    check("clash: vless reality", r.get("security") == "reality" and r["reality_short_id"] == "6ba85179"
          and r["vless_flow"] == "xtls-rprx-vision" and r["sni"] == "www.example.com", r)
    h = by.get("Hy2", {})
    check("clash: hysteria2 obfs / bandwidth / ports", h.get("hy2_obfs_password") == "ob" and h["hy2_up_mbps"] == 30
          and h["hy2_down_mbps"] == 200 and h["hy2_ports"] == "20000-30000", h)
    check("clash: trojan grpc, tuic, wireguard", by["Trojan gRPC"]["transport_path"] == "svc"
          and by["TUIC"]["tuic_congestion"] == "cubic" and by["WG"]["wg_address"] == "10.0.0.2/32" and by["WG"]["wg_mtu"] == 1280)
    try:
        sub.parse(b"proxies: [unclosed")
        check("clash: broken YAML reported", False)
    except ValueError as e:
        check("clash: broken YAML reported", "YAML" in str(e), str(e))
    srv, _ = sub.parse(b"proxies:\n  - &a {name: x, type: socks5, server: s.example, port: 1080}\n  - *a\n")
    check("clash: YAML aliases work, duplicate names made unique", [x["name"] for x in srv] == ["x", "x (2)"], srv)


def test_wg_server():
    from proxydock import wgconf
    from proxydock.singbox import compile as sbc, engine_for
    priv, _ = wgconf.keypair()
    c1 = wgconf.keypair()
    ib = dict(config.INBOUND_DEFAULTS, type="wireguard", host="0.0.0.0", port=51820, wg_private_key=priv,
              wg_peers=json.dumps([{"name": "Phone", "private_key": c1[0], "public_key": c1[1], "address": "10.66.0.2/32"}]))
    check("wg server: valid settings", not config.wg_server_errors(ib), config.wg_server_errors(ib))
    check("wg server: next device address", wgconf.next_peer_address("10.66.0.1/24", ["10.66.0.2/32"]) == "10.66.0.3/32")
    bad = dict(ib, wg_peers=json.dumps([{"name": "X", "public_key": c1[1], "address": "10.9.0.2/32"}]))
    check("wg server: device outside the network reported", any("X" in e for e in config.wg_server_errors(bad)))
    conf_text = wgconf.client_conf(ib, config.wg_peers(ib)[0], "192.0.2.10")
    ob, _ = wgconf.parse_conf(conf_text)
    check("wg server: device config reads back", ob["host"] == "192.0.2.10" and ob["port"] == 51820
          and ob["wg_peer_public_key"] == wgconf.public_of(priv) and ob["wg_address"] == "10.66.0.2/32", ob)
    p = config.new_proxy("wgs", ib)
    c = sbc.build(p, config.resolve(p, config.validate_global({})), bans_path="b.json", api_port=1, api_secret="s")[0]
    ep = (c.get("endpoints") or [{}])[0]
    check("wg server: sing-box endpoint, no plain inbound", c["inbounds"] == [] and ep.get("listen_port") == 51820
          and ep["peers"][0]["allowed_ips"] == ["10.66.0.2/32"], c.get("endpoints"))
    check("wg server: needs sing-box", engine_for(p, config.validate_global({"engine": "python"}))[0] == "none")


def test_wg_server_e2e():
    """A WireGuard server card; a WireGuard client card using the device config it hands out reaches the origin."""
    from proxydock import wgconf
    from proxydock.platform_utils import local_lan_ip
    from proxydock.runtime import ProxyManager
    lan = local_lan_ip()
    if not lan or lan.startswith("127."):
        print("SKIP wg server e2e: no LAN address")
        return
    origin = ThreadingHTTPServer(("0.0.0.0", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    oport = origin.server_address[1]
    glob = config.validate_global({"engine": "sing-box"})
    mgr = ProxyManager()
    try:
        u = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        u.bind(("127.0.0.1", 0))
        wport = u.getsockname()[1]
        u.close()
        priv, _ = wgconf.keypair()
        dev = wgconf.keypair()
        srv = config.new_proxy("wg server", {"type": "wireguard", "host": "0.0.0.0", "port": wport, "wg_private_key": priv,
                                             "wg_peers": json.dumps([{"name": "PC", "private_key": dev[0], "public_key": dev[1],
                                                                      "address": "10.66.0.2/32"}])})
        mgr.start(srv, glob).result(20)
        ob, _ = wgconf.parse_conf(wgconf.client_conf(srv["inbound"], config.wg_peers(srv["inbound"])[0], "127.0.0.1"))
        cli = config.new_proxy("wg device", {"type": "socks5", "port": free_port()})
        cli["outbound"] = dict(config.OUTBOUND_DEFAULTS, **ob, via=None)
        mgr.start(cli, glob).result(20)
        time.sleep(1)
        r = socks5_get(cli["inbound"]["port"], lan, oport)
        check("[sing-box] wg server: device -> server card -> origin", r == "200" and mgr.stats_for(srv["id"]).total > 0,
              (r, mgr.state(srv["id"])))
    finally:
        mgr.stop_all()
        origin.shutdown()


def test_if_forward_select():
    from proxydock.singbox import compile as sbc, engine_for
    g = gm.empty()
    d = gm.add_outbound(g, {"type": "direct"}, "D")
    vpn = gm.add_outbound(g, {"type": "socks5", "host": "vpn.example", "port": 1080}, "VPN")
    f = gm.add_router(g, "Games?", style="if")
    check("if: one rule, style kept", f["style"] == "if" and len(f["rules"]) == 1)
    raw = json.loads(json.dumps(g))
    raw["routers"][0]["rules"].append({"name": "extra", "match": {"port": ["1"]}})
    g = gm.validate(raw)
    f = g["routers"][0]
    check("if: extra rules dropped on load", len(f["rules"]) == 1 and f["style"] == "if")
    f["rules"][0].update(match={"process_name": ["game.exe"]}, target=vpn["id"])
    f["default"] = d["id"]
    gm.add_inbound(g, "In", {"type": "socks5", "port": 1080}, target=f["id"])
    p = gm.compile(g)[0]
    check("if: then -> rule, else -> default", p["routes"][0]["match"] == [{"process_name": ["game.exe"]}]
          and p["outbound"]["type"] == "direct" and p["target_name"] == "IF Games?", (p["routes"], p["target_name"]))
    blk = gm.add_outbound(g, {"type": "block"}, "Blk")
    ks = gm.add_router(g, "Kill switch", style="if")
    ks.update(switch=True, switch_on=False, default=vpn["id"])
    ks["rules"][0]["target"] = blk["id"]
    g["inbounds"][0]["target"] = ks["id"]
    g = gm.validate(json.loads(json.dumps(g)))
    p = gm.compile(g)[0]
    check("if switch: off -> everything to 'else'", p["routes"] == [] and p["outbound"]["host"] == "vpn.example", p["outbound"])
    gm.find(g, ks["id"])[1]["switch_on"] = True
    p = gm.compile(g)[0]
    check("if switch: on -> everything to 'then' (Block)", p["routes"] == [] and p["outbound"]["type"] == "block")
    fw = config.new_proxy("fw", {"type": "forward", "port": 2222, "fwd_host": "10.0.0.5", "fwd_port": 22, "fwd_network": "tcp"})
    c = sbc.build(fw, config.resolve(fw, config.validate_global({})), bans_path="b.json", api_port=1, api_secret="s")[0]
    ib = c["inbounds"][0]
    check("forward: sing-box direct inbound", ib["type"] == "direct" and ib["override_address"] == "10.0.0.5"
          and ib["override_port"] == 22 and ib["network"] == "tcp", ib)
    check("forward: missing destination reported",
          any("Port forward" in e for e in config.check_proxy(config.new_proxy("x", {"type": "forward", "port": 2223}), [])))
    check("forward: needs sing-box", engine_for(fw, config.validate_global({"engine": "python"}))[0] == "none")
    a = config.new_proxy("a", {"type": "forward", "port": 2300, "fwd_host": "h", "fwd_port": 1, "fwd_network": "udp"})
    b = config.new_proxy("b", {"type": "socks4", "port": 2300})
    check("forward: UDP forward and a TCP card share a port", not any("conflicts" in e for e in config.check_proxy(a, [b])))
    grp = dict(config.OUTBOUND_DEFAULTS, type="group", group_mode="select", group_selected="m2", via=None, members=[
        {"id": "m1", "name": "A", "outbound": dict(config.OUTBOUND_DEFAULTS, type="direct", via=None)},
        {"id": "m2", "name": "B", "outbound": dict(config.OUTBOUND_DEFAULTS, type="socks5", host="b", port=1, via=None)}])
    out = sbc.outbounds(grp)
    sel = next(o for o in out if o["tag"] == "out0")
    check("group manual: selector with the chosen member", sel["type"] == "selector" and sel["default"] == "out0_m1_0"
          and sel["outbounds"] == ["out0_m0_0", "out0_m1_0"], sel)


def test_forward_e2e():
    """Port forward card: a plain TCP client on the local port reaches the fixed destination (the origin)."""
    from proxydock.runtime import ProxyManager
    origin = ThreadingHTTPServer(("127.0.0.1", 0), Origin)
    threading.Thread(target=origin.serve_forever, daemon=True).start()
    glob = config.validate_global({"engine": "sing-box"})
    mgr = ProxyManager()
    try:
        fw = config.new_proxy("fwd", {"type": "forward", "port": free_port(), "fwd_host": "127.0.0.1",
                                      "fwd_port": origin.server_address[1], "fwd_network": "tcp"})
        mgr.start(fw, glob).result(20)
        c = socket.create_connection(("127.0.0.1", fw["inbound"]["port"]), timeout=10)
        c.sendall(b"GET / HTTP/1.0\r\nHost: x\r\n\r\n")
        data = b""
        while True:
            chunk = c.recv(65536)
            if not chunk:
                break
            data += chunk
        c.close()
        check("[sing-box] port forward -> fixed destination", data.endswith(b"origin-ok"), data[:60])
    finally:
        mgr.stop_all()
        origin.shutdown()


def test_engine_died_e2e():
    """A card whose sing-box process exits on its own goes to the error state (it used to stay 'running')."""
    from proxydock.runtime import ProxyManager
    glob = config.validate_global({"engine": "sing-box"})
    mgr = ProxyManager()
    try:
        c = config.new_proxy("dies", {"type": "socks5", "port": free_port()})
        mgr.start(c, glob).result(20)
        mgr.instances[c["id"]].proc.kill()
        for _ in range(50):
            if mgr.state(c["id"])[0] == "error":
                break
            time.sleep(0.1)
        st = mgr.state(c["id"])
        check("[sing-box] engine exit -> card in error", st[0] == "error" and "exited" in st[1] and c["id"] not in mgr.instances, st)
    finally:
        mgr.stop_all()


def main():
    t0 = time.time()
    tests = [test_translations, test_graph, test_engine, test_singbox_compile, test_router_graph, test_relay_graph, test_block_colors, test_router_singbox, test_interface,
             test_wireguard, test_tun, test_run_as_admin, test_protocols,
             test_subscription, test_detect_sni, test_hy2_advanced, test_group, test_dns, test_review_fixes, test_clash, test_wg_server, test_if_forward_select]
    from proxydock.singbox import find_binary
    if find_binary():
        tests += [lambda: test_engine("sing-box"), test_bans_file_locked, test_singbox_guard, test_router_e2e, test_interface_e2e, test_wireguard_e2e, test_helper,
                  test_protocols_e2e, test_subscription_e2e, test_hy2_advanced_e2e, test_group_e2e, test_dns_e2e, test_close_log_e2e, test_wg_server_e2e, test_forward_e2e, test_engine_died_e2e]
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
