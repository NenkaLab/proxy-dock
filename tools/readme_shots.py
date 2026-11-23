"""README screenshots of a sample setup: canvas, proxy list and router editor, light and dark, at 2x
(2800x1720), into docs/images/<page>-<theme>.png (the files the READMEs show).

    python tools/readme_shots.py            # both themes
    python tools/readme_shots.py dark       # one theme
    python tools/readme_shots.py dark out/  # into another folder

Runs offscreen with a throwaway settings folder; nothing on this PC is changed. Each theme runs in its own
process (Qt's scale factor and theme are fixed per process). Takes about 15 seconds per theme (it waits for the
'system proxy set' notification to go away).
"""
import json
import os
import subprocess
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_OUT = os.path.join(ROOT, "docs", "images")

if __name__ == "__main__" and (len(sys.argv) < 2 or sys.argv[1] not in ("light", "dark")):
    out = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_OUT
    rc = 0
    for theme in ("light", "dark"):
        rc |= subprocess.call([sys.executable, os.path.abspath(__file__), theme, out])
    sys.exit(rc)

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
os.environ["QT_SCALE_FACTOR"] = "2"           # sharp images: 2 device pixels per pixel
sys.path.insert(0, ROOT)
THEME = sys.argv[1]
OUT = sys.argv[2] if len(sys.argv) > 2 else DEFAULT_OUT
os.makedirs(OUT, exist_ok=True)
from proxydock import config, graph as gm, wgconf
TMP = tempfile.mkdtemp(prefix="pd-readme-")
config.APP_DIR = TMP
config.CONFIG_PATH = os.path.join(TMP, "config.json")
config.LOG_DIR = os.path.join(TMP, "logs")
config.CERT_DIR = os.path.join(TMP, "certs")
config.WINDOW_STATE_PATH = os.path.join(TMP, "window.json")
config.app_dir = lambda name="x": os.path.join(TMP, "none")
from PySide6.QtCore import QEventLoop, QTimer
from PySide6.QtGui import QColor, QImage, QPainter
from PySide6.QtNetwork import QLocalServer
from PySide6.QtWidgets import QApplication
from proxydock.ui import common as uic
from proxydock.ui import window as winmod
winmod.system_proxy_supported = lambda: True
winmod.get_system_proxy = lambda: {"ProxyEnable": 0}
winmod.set_system_proxy = lambda v: None


def wait(ms):
    loop = QEventLoop()
    QTimer.singleShot(ms, loop.quit)
    loop.exec()


# --- sample setup: router, IFs (one a kill switch), a group, a pass-through and a wireless pair ---
g = gm.empty()
k1, k2 = wgconf.keypair(), wgconf.keypair()
wg = gm.add_outbound(g, {"type": "wireguard", "host": "jp.example.net", "port": 51820, "wg_private_key": k1[0],
                         "wg_address": "10.8.0.2/32", "wg_peer_public_key": k2[1]}, "Tokyo WireGuard", [1250, -240])
hy = gm.add_outbound(g, {"type": "hysteria2", "host": "kr.example.net", "port": 443, "hy2_password": "x"},
                     "Seoul Hysteria2", [1250, -60])
grp = gm.add_outbound(g, {"type": "group", "group_mode": "auto"}, "Fastest VPN", [840, -200])
grp["members"] = [wg["id"], hy["id"]]
direct = gm.add_outbound(g, {"type": "direct"}, "Direct", [1250, 130])
block = gm.add_outbound(g, {"type": "block"}, "Block", [1250, 320])
office = gm.add_outbound(g, {"type": "http", "host": "proxy.office.lan", "port": 3128}, "Office proxy", [1250, -440])

split = gm.add_router(g, "Split", [420, -130])
split["rules"] = [{"id": "r1", "name": "Games", "match": {"process_name": ["game.exe", "steam.exe"]}, "target": grp["id"]},
                  {"id": "r2", "name": "Ads", "match": {"geosite": ["category-ads-all"]}, "target": None}]
korea = gm.add_router(g, "Korean site?", [420, 330], style="if")
ks = gm.add_router(g, "Kill switch", [420, 100], style="if")
ks.update(switch=True, switch_on=True)
ks["rules"][0]["target"] = grp["id"]

bus = gm.add_relay(g, "Pass-through", [880, 130], lanes=4)          # tidies the links into Direct and Block
split["default"] = bus["inputs"][0]["id"]
korea["rules"][0].update(match={"geoip": ["kr"]}, target=bus["inputs"][1]["id"])
split["rules"][1]["target"] = bus["inputs"][2]["id"]
bus["outputs"][0]["target"] = bus["outputs"][1]["target"] = direct["id"]
bus["outputs"][2]["target"] = bus["outputs"][3]["target"] = block["id"]
ks["default"] = bus["inputs"][3]["id"]

w = gm.add_wireless(g, [880, 420])                                   # else -> far away, without a long wire
w["name"], w["name_out"] = "To office", "From Phone"
w["pos_out"] = [880, -440]
korea["default"] = w["inputs"][0]["id"]
w["outputs"][0]["target"] = office["id"]

browser = gm.add_inbound(g, "Browser (HTTP)", {"type": "http", "port": 8080}, [0, -130], split["id"], autostart=True)
apps = gm.add_inbound(g, "Apps (SOCKS5)", {"type": "socks5", "port": 1080}, [0, 100], ks["id"], autostart=True)
apps["auto_restart"] = True
d3 = wgconf.keypair()
phone = gm.add_inbound(g, "Phone (WireGuard)", {"type": "wireguard", "host": "0.0.0.0", "port": 51821,
                       "wg_private_key": k2[0], "wg_peers": json.dumps([{"name": "Phone", "private_key": d3[0],
                       "public_key": d3[1], "address": "10.66.0.2/32"}])}, [0, 330], korea["id"])
browser["color"], apps["color"], phone["color"] = "#4f8ef7", "#e8912d", "#b76de0"
g["pc"]["inbound"] = browser["id"]
g["pc"]["pos"] = [420, -420]

glob = config.validate_global({"language": "en", "theme": THEME})
config.save(glob, g)
glob, graph = config.load()
app = QApplication.instance() or QApplication([])
uic.apply_theme(THEME)
uic.load_fonts(app, glob)
srv = QLocalServer()
srv.listen(f"pd-readme-{os.getpid()}")
ctl = winmod.Controller(srv, False, glob, graph)
win = ctl.window
win.resize(1400, 860)
win.show()
wait(900)
BG = QColor(32, 32, 32) if THEME == "dark" else QColor(243, 243, 243)
running = {p["id"]: ("running", "") for p in ctl.proxies[:2]}
pc = ("applied", "In use: Browser (HTTP) (127.0.0.1:8080)")

from proxydock import probe                                          # (sample latency results)
for oid, ms in ((wg["id"], 182), (hy["id"], 64), (direct["id"], 21), (office["id"], 744), (grp["id"], 70)):
    ctl.latency[oid] = {"ok": True, "ms": ms, "error": "", "key": probe.latency_key(ctl.graph, oid)}


def shot(name):
    img = win.grab().toImage()
    out = QImage(img.size(), QImage.Format_RGB32)
    out.setDevicePixelRatio(img.devicePixelRatio())     # (draw the 2x capture at full size)
    out.fill(BG)
    p = QPainter(out)
    p.drawImage(0, 0, img)
    p.end()
    out.save(os.path.join(OUT, f"{name}-{THEME}.png"))


cv = win.canvas_page
win.switchTo(cv)
wait(7000)          # let the "system proxy set" notification go away
cv.update_states(ctl.proxies, running, pc)
cv.fit()
wait(300)
cv.node_of[ks["id"]].set_selected(True)                              # (the toolbar lights up for a selection)
cv._on_selection()
wait(300)
shot("canvas")
cv.graph.clear_selection()
win.switchTo(win.proxies_page)
wait(400)
for pid, st in running.items():
    win.proxies_page.set_status(pid, st, ctl.proxy_by_id(pid))
win.proxies_page.pc_bar.set_status(pc)
wait(200)
shot("proxies")
ctl.edit_node(split["id"])
wait(600)
shot("router")
print(f"{THEME}: canvas, proxies, router -> {OUT}", flush=True)
os._exit(0)
