"""Settings v2 files and the previous version's (SOCKS5 Proxy) settings: random cases read by the Python app
(config.load's v2 branch, config.migrate_from_old) and by the Rust core (examples/dump_v2.rs), compared. Block ids
made on the way are random on both sides, so they are numbered in order of appearance.

    python native/tools/parity_v2.py <dump_v2.exe> [count] [--seed N] [--lang ko]
"""
import json
import os
import random
import re
import subprocess
import sys
import tempfile
import time

ROOT = os.path.join(os.path.dirname(__file__), "..", "..")
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pyapp  # noqa: E402,F401  (the Python app: see pyapp.py)
from proxydock import config, graph, i18n  # noqa: E402

R = random.Random()
IB = ["socks5", "http", "https", "shadowsocks", "hysteria2", "socks4", "bogus"]
OB = ["direct", "socks5", "http", "shadowsocks", "trojan", "vless", "block"]
NAMES = ["", "a", "Proxy", "  pad ", "한글", "x" * 70]


def pick(xs):
    return R.choice(xs)


def v2_case():
    obs = [{"type": pick(OB), "host": pick(["h.example", "1.2.3.4", ""]), "port": pick([0, 443, 8388, 70000]),
            "password": pick(["", "pw"]), "ss_password": pick(["", "k"]), "ss_method": pick(["aes-256-gcm", "nope"])}
           for _ in range(R.randint(1, 3))]
    proxies = []
    for i in range(R.randint(0, 5)):
        p = {"id": pick(["%08x" % R.getrandbits(32), "", None]), "name": pick(NAMES), "autostart": pick([True, False, 1]),
             "inbound": {"type": pick(IB), "host": pick(["127.0.0.1", "0.0.0.0", "bad host"]), "port": pick([1080, 0, 8080, "x"]),
                         "auth_enabled": pick([True, False]), "username": "u", "password": pick(["", "p"])},
             "outbound": dict(pick(obs)),
             "overrides": pick([{}, {"timeout": pick([5, 300, "x"])}, {"dns_server": pick(["1.1.1.1", "ftp://x"])}, None])}
        if R.random() < 0.1:
            p = pick(["x", 3, None])
        proxies.append(p)
    raw = {"version": pick([2, 2, 1]), "global": pick([{}, {"language": "ko", "timeout": 5}, None]), "proxies": proxies}
    if R.random() < 0.1:
        raw.pop("version")
    return raw


def old_case():
    return {k: v for k, v in {
        "host": pick(["127.0.0.1", "0.0.0.0", None]), "port": pick([30307, 1080, "x", None]),
        "auth_enabled": pick([True, False, None]), "username": pick(["u", None]), "password": pick(["p", "", None]),
        "ss_enabled": pick([True, False, None]), "ss_password": pick(["k", "", None]), "ss_port": pick([8388, None]),
        "ss_method": pick(["aes-256-gcm", "bad", None]), "language": pick(["ko", "en", None]), "timeout": pick([60, None]),
    }.items() if v is not None}


def python_v2(raw):
    glob = config.validate_global(raw.get("global"))
    proxies = []
    for item in raw.get("proxies") or []:
        if not isinstance(item, dict):
            continue
        item = dict(item)
        item["inbound"], _ = config._unseal(item.get("inbound"), "inbound")
        item["outbound"], _ = config._unseal(item.get("outbound"), "outbound")
        proxies.append(config.validate_proxy(item))
    return {"global": glob, "graph": graph.validate(graph.from_proxies(proxies))}


def python_old(raw, tmp):
    with open(os.path.join(tmp, "config.json"), "w", encoding="utf-8") as f:
        json.dump(raw, f)
    config.app_dir, config.APP_DIR = (lambda name: tmp), os.path.join(tmp, "new")
    config.save, config.log = (lambda g, gr: True), (lambda *a, **k: None)
    got = config.migrate_from_old()
    return None if got is None else {"global": got[0], "graph": graph.validate(got[1])}


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
    exe = args[0]
    lang = args[args.index("--lang") + 1] if "--lang" in args else "en"
    seed = int(args[args.index("--seed") + 1]) if "--seed" in args else int(time.time())
    count = int(args[1]) if len(args) > 1 and args[1].isdigit() else 300
    R.seed(seed)
    i18n.set_language(lang)
    cases = []
    for _ in range(count):
        if R.random() < 0.7:
            cases.append({"kind": "v2", "raw": v2_case()})
        else:
            cases.append({"kind": "old", "raw": old_case()})
    data = [{"kind": c["kind"], "text": json.dumps(c["raw"], ensure_ascii=False)} for c in cases]
    with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False, encoding="utf-8") as f:
        json.dump(data, f, ensure_ascii=False)
    env = dict(os.environ, PROXYDOCK_LANG=lang)
    rust = json.loads(subprocess.run([exe, f.name, lang], capture_output=True, check=True, env=env).stdout.decode("utf-8"))
    os.unlink(f.name)
    tmp = tempfile.mkdtemp()
    bad = 0
    for c, r in zip(cases, rust):
        # (the Rust app's own setting, which the Python app does not have)
        if isinstance(r, dict) and isinstance(r.get("global"), dict):
            r["global"].pop("theme_color", None)
        known = set(re.findall(r'"([0-9a-f]{8})"', json.dumps(c["raw"])))
        py = python_v2(c["raw"]) if c["kind"] == "v2" else python_old(c["raw"], tmp)
        a, b = normalise(py, known), normalise(r, known)
        if a != b:
            bad += 1
            if bad <= 3:
                print("DIFF", c["kind"], json.dumps(c["raw"], ensure_ascii=False)[:300])
                for i, (x, y) in enumerate(zip(a, b)):
                    if x != y:
                        print("  py:", a[max(0, i - 150):i + 150])
                        print("  rs:", b[max(0, i - 150):i + 150])
                        break
    print(f"seed {seed} lang {lang}: {count} cases, {bad} differences")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
