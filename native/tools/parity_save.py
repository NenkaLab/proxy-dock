"""Read and save every settings file in a folder with both apps and compare the written files exactly (the DPAPI
blobs differ every time, so each is replaced by what it decrypts to first).

    python native/tools/parity_save.py <dir> <resave.exe>
"""
import glob
import itertools
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import types

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pyapp  # noqa: E402,F401  (the Python app: see pyapp.py)
from proxydock import config, graph  # noqa: E402


def python_resave(src, dst):
    counter = itertools.count(1)
    graph.new_id = lambda: f"id{next(counter):06}"
    config.uuid = types.SimpleNamespace(uuid4=lambda: types.SimpleNamespace(hex=graph.new_id()))
    config.CONFIG_PATH = src
    glob_, g = config.load()
    config.CONFIG_PATH = dst
    config.APP_DIR = os.path.dirname(dst)
    assert config.save(glob_, g)


DPAPI = re.compile(r'"(\w+)_dpapi": "([^"]*)"')


def opened(text):
    def plain(m):
        if not m.group(2):
            return m.group(0)
        return f'"{m.group(1)}_dpapi": {json.dumps("PLAIN:" + config._dpapi(__import__("base64").b64decode(m.group(2)), False).decode("utf-8"), ensure_ascii=False)}'
    return DPAPI.sub(plain, text)


def main():
    folder, exe = sys.argv[1], sys.argv[2]
    same, diff = 0, 0
    out = tempfile.mkdtemp()
    for path in sorted(glob.glob(os.path.join(folder, "*.json"))):
        py_out, rs_out = os.path.join(out, "py.json"), os.path.join(out, "rs.json")
        # each app reads its own copy (loading can write the file back: plain passwords get sealed)
        py_in, rs_in = os.path.join(out, "py-in.json"), os.path.join(out, "rs-in.json")
        shutil.copyfile(path, py_in)
        shutil.copyfile(path, rs_in)
        # a fresh Python per file (the app's modules keep state between loads)
        subprocess.run([sys.executable, __file__, "--python", py_in, py_out], check=True)
        r = subprocess.run([exe, rs_in, rs_out, "--seq"], capture_output=True, encoding="utf-8")
        if r.returncode:
            raise RuntimeError(r.stderr[-800:])
        a = opened(open(py_out, encoding="utf-8").read())
        b = opened(open(rs_out, encoding="utf-8").read())
        # (the Rust app's own setting, which the Python app does not have: its default line left out)
        b = re.sub(r'\n\s*"theme_color": "",', "", b)
        if a == b:
            same += 1
            continue
        diff += 1
        if diff <= 3:
            la, lb = a.splitlines(), b.splitlines()
            k = next((i for i, (x, y) in enumerate(zip(la, lb)) if x != y), min(len(la), len(lb)))
            print(f"--- {os.path.basename(path)} line {k + 1}\n  py: {la[k] if k < len(la) else '<end>'}\n  rs: {lb[k] if k < len(lb) else '<end>'}")
    print(f"same: {same}, different: {diff}")
    sys.exit(1 if diff else 0)


if __name__ == "__main__":
    if sys.argv[1] == "--python":
        python_resave(sys.argv[2], sys.argv[3])
    else:
        main()
