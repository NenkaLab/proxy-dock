"""The Python app's sing-box config of every card in a settings file, in the shape the dump_singbox example prints
(secrets masked). Reads only.

    python native/tools/py_dump_singbox.py [config.json] [--lang ko] [--seq]
"""
import hashlib
import itertools
import json
import os
import sys
import types

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", ".."))
from proxydock import config, graph, i18n  # noqa: E402
from proxydock.singbox import compile as sb, rulesets  # noqa: E402

MASK = ("password", "private_key", "pre_shared_key", "uuid", "secret")


def mask_all(v):
    if isinstance(v, dict):
        for k, x in v.items():
            if k in MASK and isinstance(x, str):
                if x:
                    v[k] = "sha256:" + hashlib.sha256(x.encode()).hexdigest()[:12]
            else:
                mask_all(x)
    elif isinstance(v, list):
        for x in v:
            mask_all(x)


def main():
    args = sys.argv[1:]
    path, lang = config.CONFIG_PATH, "en"
    while args:
        a = args.pop(0)
        if a == "--lang":
            lang = args.pop(0)
        elif a == "--seq":
            counter = itertools.count(1)
            graph.new_id = lambda: f"id{next(counter):06}"
            config.uuid = types.SimpleNamespace(uuid4=lambda: types.SimpleNamespace(hex=graph.new_id()))
        else:
            path = a
    i18n.set_language(lang)
    with open(path, encoding="utf-8") as f:
        raw = json.load(f)
    glob = config.validate_global(raw.get("global"))
    g = raw.get("graph") if isinstance(raw.get("graph"), dict) else {}
    for key, field in (("inbounds", "inbound"), ("outbounds", "outbound")):
        items = []
        for item in g.get(key) or []:
            if isinstance(item, dict):
                item = dict(item)
                item[field], _ = config._unseal(item.get(field), field)
                items.append(item)
        g = dict(g, **{key: items})
    out = []
    for p in graph.compile(graph.validate(g)):
        cfg = config.resolve(p, glob)
        sets = {}
        for r in p["routes"]:
            for m in r["match"]:
                for kind in ("geoip", "geosite"):
                    for name in m.get(kind, []):
                        t = rulesets.tag(kind, name)
                        sets.setdefault(t, f"RS/{t}")
        exclude = ["E/sing-box.exe", "E/hysteria.exe"] if p["inbound"]["type"] == "tun" else None
        try:
            conf, reasons = sb.build(p, cfg, bans_path="BANS", api_port=9090, api_secret="S", cert=("C", "K"),
                                     rulesets=sets, exclude_paths=exclude)
            mask_all(conf)
            out.append({"id": p["id"], "config": conf, "reasons": [list(x) if isinstance(x, tuple) else x for x in reasons]})
        except Exception as e:      # noqa: BLE001 - compared by type with the Rust port
            out.append({"id": p["id"], "error": type(e).__name__})
    print(json.dumps(out, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
