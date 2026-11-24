"""The Python app's problems for every card (check_proxy) and router rule (match_errors) of a settings file, in
the shape the dump_checks example prints. Reads a copy (loading may write the file back).

    python native/tools/py_dump_checks.py <config.json> [--lang ko]
"""
import itertools
import json
import os
import shutil
import sys
import tempfile
import types

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", ".."))
from proxydock import config, graph, i18n  # noqa: E402


def main():
    args = sys.argv[1:]
    src = args[0]
    if "--lang" in args:
        i18n.set_language(args[args.index("--lang") + 1])
    counter = itertools.count(1)
    graph.new_id = lambda: f"id{next(counter):06}"
    config.uuid = types.SimpleNamespace(uuid4=lambda: types.SimpleNamespace(hex=graph.new_id()))
    tmp = os.path.join(tempfile.mkdtemp(), "config.json")
    shutil.copyfile(src, tmp)
    config.CONFIG_PATH = tmp
    config.APP_DIR = os.path.dirname(tmp)
    _glob, g = config.load()
    import contextlib
    import io
    with contextlib.redirect_stdout(io.StringIO()):     # the UI toolkit prints a banner when imported
        from proxydock.ui.router import match_errors
    proxies = graph.compile(g)
    cards = {p["id"]: config.check_proxy(p, [o for o in proxies if o["id"] != p["id"]]) for p in proxies}
    rules = {}
    for r in g.get("routers", []):
        for i, x in enumerate(r["rules"]):
            rules[x["id"]] = match_errors(x["name"] or str(i + 1), x["match"])
    print(json.dumps({"cards": cards, "rules": rules}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    main()
