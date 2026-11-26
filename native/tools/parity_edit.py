"""Duplicate and rename on random graphs, done by the Python app (Controller.duplicate_node / rename_node /
set_wireless_title, as the canvas calls them) and by the Rust core (examples/dump_edit.rs), compared. New block and
lane ids are random on both sides, so they are numbered in order of appearance before comparing.

    python native/tools/parity_edit.py [count] [--lang ko] [--seed N]
"""
import json
import os
import random
import re
import subprocess
import sys
import tempfile
import time
import types

ROOT = os.path.join(os.path.dirname(__file__), "..", "..")
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pyapp  # noqa: E402,F401  (the Python app: see pyapp.py)
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
from proxydock import graph as graph_mod, i18n  # noqa: E402
from proxydock.ui import window  # noqa: E402

R = random.Random()
NAMES = ["SOCKS5", "SOCKS5 copy", "SOCKS5 copy 2", "Direct", "Direct copy", "Router", "x", "  pad  ", "", " ",
         "a" * 70, "한글 이름", "Wireless in", "Rule", "Pass-through", "Pass-through copy", "　wide　"]
OB_TYPES = ["direct", "socks5", "http", "shadowsocks", "vless", "group", "block", "trojan"]


def rid():
    return "%08x" % R.getrandbits(32)


def random_graph():
    g = {"inbounds": [], "outbounds": [], "routers": [], "relays": []}
    outs = []
    for _ in range(R.randint(0, 4)):
        i = rid()
        outs.append(i)
        ob = {"type": R.choice(OB_TYPES), "host": "h.example", "port": R.choice([443, 1080, 8388])}
        if ob["type"] == "shadowsocks":
            ob.update(password="pw", cipher="aes-256-gcm")
        g["outbounds"].append({"id": i, "name": R.choice(NAMES), "outbound": ob, "pos": [R.randint(-500, 500), R.randint(-500, 500)],
                               "via": R.choice([None] + outs), "members": R.sample(outs, k=min(len(outs), R.randint(0, 2)))})
    routers = []
    for _ in range(R.randint(0, 2)):
        i = rid()
        routers.append(i)
        g["routers"].append({"id": i, "name": R.choice(NAMES), "pos": [R.randint(-500, 500), 0],
                             "style": R.choice(["router", "if"]), "default": R.choice([None] + outs),
                             "rules": [{"id": rid(), "name": R.choice(NAMES), "match": {"domain": ["a.com"]},
                                        "target": R.choice([None] + outs)} for _ in range(R.randint(1, 3))]})
    for _ in range(R.randint(0, 3)):
        if R.random() < 0.5:
            a, b = rid(), rid()
            g["relays"].append({"id": rid(), "name": R.choice(NAMES), "wireless": True, "number": R.choice([1, 2, 3, None]),
                                "pos": [0, 0], "pos_out": [R.randint(0, 300), 50], "name_out": R.choice(NAMES),
                                "inputs": [{"id": a, "out": b}], "outputs": [{"id": b, "target": R.choice([None] + outs)}],
                                "broken": R.choice([None, None, "in", "out"])})
        else:
            outs_l = [{"id": rid(), "target": R.choice([None] + outs)} for _ in range(R.randint(1, 3))]
            g["relays"].append({"id": rid(), "name": R.choice(NAMES), "pos": [10, 10], "routed": R.random() < 0.3,
                                "inputs": [{"id": rid(), "out": R.choice([None] + [x["id"] for x in outs_l])} for _ in outs_l],
                                "outputs": outs_l})
    for _ in range(R.randint(0, 4)):
        g["inbounds"].append({"id": rid(), "name": R.choice(NAMES), "pos": [R.randint(-500, 500), 100],
                              "inbound": {"type": R.choice(["socks5", "http", "shadowsocks"]), "port": R.choice([1080, 1081, 1082, 65535, 2000])},
                              "autostart": R.random() < 0.5, "target": R.choice([None] + outs + routers)})
    return graph_mod.validate(g)


def keys_of(g):
    ks = ["pc", "nope"]
    for kind in ("inbounds", "outbounds", "routers", "relays"):
        for n in g[kind]:
            ks.append(n["id"])
            if n.get("wireless"):
                ks.append(n["id"] + "/out")
    return ks


def random_ops(g):
    ops = []
    for _ in range(R.randint(1, 4)):
        ks = keys_of(g)
        if R.random() < 0.5:
            ops.append(["duplicate", R.sample(ks, k=R.randint(1, min(3, len(ks))))])
        else:
            ops.append(["rename", R.choice(ks), R.choice(NAMES)])
    return ops


class Fake:
    """The parts of the Python Controller that duplicate / rename use."""

    def __init__(self, g):
        self.graph = g

    _unique_name = window.Controller._unique_name
    _free_port = window.Controller._free_port
    duplicate_node = window.Controller.duplicate_node
    rename_node = window.Controller.rename_node
    set_wireless_title = window.Controller.set_wireless_title

    def _recompile(self):
        pass

    def sig_refresh(self):
        pass


def python_case(case):
    g = json.loads(json.dumps(case["graph"]))
    ctl = Fake(g)
    results = []
    logged = []
    window.log = lambda level, msg, inst=None: logged.append(msg)
    for op in case["ops"]:
        if op[0] == "duplicate":
            # the canvas: each block once (a wireless pair's halves are one block), not the PC block
            mids = []
            for k in op[1]:
                mid = k[:-4] if k.endswith("/out") else k
                if k != "pc" and mid not in mids:
                    mids.append(mid)
            count = 0
            for mid in mids:
                kind, n = graph_mod.find(ctl.graph, mid)
                if n is not None:
                    count += 1
                ctl.duplicate_node(mid)
            results.append(count)
            continue
        key, name = op[1], op[2]
        logged.clear()
        mid, out_half = (key[:-4], True) if key.endswith("/out") else (key, False)
        kind, n = graph_mod.find(ctl.graph, mid)
        name = str(name).strip()
        if n is not None and kind == "relay" and n.get("wireless"):
            ctl.set_wireless_title(mid, "out" if out_half else "in", name)
        elif n is not None and name and name[:60] != n["name"]:     # canvas _apply_rename
            ctl.rename_node(mid, name)
        results.append(logged[0] if logged else None)
    return {"graph": ctl.graph, "results": results}


def normalise(out, known):
    text = json.dumps(out, sort_keys=True, ensure_ascii=False)
    seen = {}

    def sub(m):
        v = m.group(1)
        if v in known:
            return m.group(0)
        seen.setdefault(v, "new%d" % len(seen))
        return '"' + seen[v] + '"'
    return re.sub(r'"([0-9a-f]{8})"', sub, text)


def main():
    args = sys.argv[1:]
    lang = args[args.index("--lang") + 1] if "--lang" in args else "en"
    seed = int(args[args.index("--seed") + 1]) if "--seed" in args else int(time.time())
    count = int(args[0]) if args and args[0].isdigit() else 500
    R.seed(seed)
    i18n.set_language(lang)
    cases = []
    for _ in range(count):
        g = random_graph()
        cases.append({"graph": g, "ops": random_ops(g)})
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False, encoding="utf-8") as f:
        json.dump(cases, f, ensure_ascii=False)
    exe = os.path.join(ROOT, "native", "target", "debug", "examples", "dump_edit.exe")
    rust = json.loads(subprocess.run([exe, f.name, lang], capture_output=True, check=True).stdout.decode("utf-8"))
    os.unlink(f.name)
    saved = i18n._t("Saved proxy '{name}'")
    bad = 0
    for c, r in zip(cases, rust):
        known = set(re.findall(r'"([0-9a-f]{8})"', json.dumps(c["graph"])))
        py = python_case(c)
        r["results"] = [saved.format(name=x) if isinstance(x, str) else x for x in r["results"]]
        a, b = normalise(py, known), normalise(r, known)
        if a != b:
            bad += 1
            if bad <= 3:
                print("DIFF ops", json.dumps(c["ops"], ensure_ascii=False))
                for i, (x, y) in enumerate(zip(a, b)):
                    if x != y:
                        print("  py:", a[max(0, i - 200):i + 200])
                        print("  rs:", b[max(0, i - 200):i + 200])
                        break
    print(f"seed {seed} lang {lang}: {count} cases, {bad} differences")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
