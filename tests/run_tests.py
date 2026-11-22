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
    w.editor.s_router.add_rule()
    w.editor.s_router.cards[1].edits["port"].setText("70000")
    check("ui: router editor reports a bad port", any("70000" in e for e in w.editor._errors(w.editor.collect())))
    w.editor.cancel()
    wait(300)
    ctl.edit_node(d["id"])
    wait(300)
    check("ui: Direct editor shows the network interface choice", w.editor.r_out_iface.isVisible()
          and w.editor.cb_out_iface.count() >= 1)
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
            check("subscription: Clash YAML / empty refused", False)
        except ValueError:
            check("subscription: Clash YAML / empty refused", True)
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
    mgr.stop_all()
    web.shutdown()
    origin.shutdown()


def main():
    t0 = time.time()
    tests = [test_translations, test_graph, test_engine, test_singbox_compile, test_router_graph, test_router_singbox, test_interface,
             test_wireguard, test_tun, test_run_as_admin, test_protocols,
             test_subscription]
    from proxydock.singbox import find_binary
    if find_binary():
        tests += [lambda: test_engine("sing-box"), test_singbox_guard, test_router_e2e, test_interface_e2e, test_wireguard_e2e, test_helper,
                  test_protocols_e2e, test_subscription_e2e]
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
