"""The Python app's reading of config.json in the shape native/core/examples/dump_config.rs prints (secrets as the
start of their SHA-256). Reads only: config.load() is not used because it may save the file again.

    python native/tools/py_dump_config.py [config.json]
"""
import hashlib
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", ".."))
from proxydock import config, graph  # noqa: E402

SECRET = config.SECRET_FIELDS


def mask(d):
    out = dict(d)
    for k in SECRET:
        if isinstance(out.get(k), str):
            out[k] = "sha256:" + hashlib.sha256(out[k].encode()).hexdigest()[:12] if out[k] else ""
    return out


def main():
    path = sys.argv[1] if len(sys.argv) > 1 else config.CONFIG_PATH
    with open(path, encoding="utf-8") as f:
        raw = json.load(f)
    g = raw.get("graph") or {}
    legacy = False
    inbounds, outbounds = [], []
    for item in g.get("inbounds") or []:
        if isinstance(item, dict):
            ib, lg = config._unseal(item.get("inbound"), "inbound")
            legacy = legacy or lg
            p = config.validate_proxy(dict(item, inbound=ib))
            inbounds.append({"id": item.get("id"), "inbound": mask(p["inbound"])})
    for item in g.get("outbounds") or []:
        if isinstance(item, dict):
            ob, lg = config._unseal(item.get("outbound"), "outbound")
            legacy = legacy or lg
            n = graph.validate_outbound_node(dict(item, outbound=ob))
            outbounds.append({"id": item.get("id"), "outbound": mask(n["outbound"])})
    out = {"global": config.validate_global(raw.get("global")), "inbounds": inbounds, "outbounds": outbounds,
           "needs_save": legacy}
    print(json.dumps(out, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
