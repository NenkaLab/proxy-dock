"""The Python app's validated graph of a settings file, in the shape the dump_graph example prints (secrets as
the start of their SHA-256). Reads only.

    python native/tools/py_dump_graph.py [config.json] [--lang ko] [--seq] [--compile]

--seq: new ids are id000001, id000002, ... (as the Rust dump) so they can be compared.
"""
import hashlib
import itertools
import json
import os
import sys
import types

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", ".."))
from proxydock import config, graph, i18n  # noqa: E402


def mask_all(v):
    """every secret field anywhere in v (nested via chains, group members, routes)"""
    if isinstance(v, dict):
        for x in v.values():
            mask_all(x)
        mask(v)
    elif isinstance(v, list):
        for x in v:
            mask_all(x)


def mask(d):
    for k in config.SECRET_FIELDS:
        if isinstance(d.get(k), str):
            d[k] = "sha256:" + hashlib.sha256(d[k].encode()).hexdigest()[:12] if d[k] else ""


def main():
    args = sys.argv[1:]
    path, lang, compile_ = config.CONFIG_PATH, "en", False
    while args:
        a = args.pop(0)
        if a == "--lang":
            lang = args.pop(0)
        elif a == "--compile":
            compile_ = True
        elif a == "--seq":
            counter = itertools.count(1)
            graph.new_id = lambda: f"id{next(counter):06}"
            # config.validate_proxy makes its ids with uuid.uuid4().hex[:8]
            config.uuid = types.SimpleNamespace(uuid4=lambda: types.SimpleNamespace(hex=graph.new_id()))
        else:
            path = a
    i18n.set_language(lang)
    with open(path, encoding="utf-8") as f:
        raw = json.load(f)
    g = raw.get("graph") if isinstance(raw.get("graph"), dict) else {}
    for key, field in (("inbounds", "inbound"), ("outbounds", "outbound")):
        items = []
        for item in g.get(key) or []:
            if isinstance(item, dict):
                item = dict(item)
                item[field], _ = config._unseal(item.get(field), field)
                items.append(item)      # (config.load keeps only dicts here)
        g = dict(g, **{key: items})
    out = graph.validate(g)
    if compile_:
        proxies = graph.compile(out)
        mask_all(proxies)
        print(json.dumps(proxies, ensure_ascii=False, indent=2))
        return
    for key, field in (("inbounds", "inbound"), ("outbounds", "outbound")):
        for n in out[key]:
            mask(n[field])
    print(json.dumps(out, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
