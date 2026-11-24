"""Run both graph validations on every settings file in a folder and compare (see fuzz_graph.py).

    python native/tools/parity_graph.py <dir> <dump_graph.exe> [--lang ko]
"""
import glob
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from compare_config import walk  # noqa: E402


def run(cmd):
    r = subprocess.run(cmd, capture_output=True, encoding="utf-8", env=dict(os.environ, PYTHONIOENCODING="utf-8"))
    if r.returncode:
        raise RuntimeError(f"{cmd}: {r.stderr.strip()[-800:]}")
    return json.loads(r.stdout)


def main():
    folder, exe, extra = sys.argv[1], sys.argv[2], sys.argv[3:]
    same, diff = 0, 0
    for path in sorted(glob.glob(os.path.join(folder, "*.json"))):
        py = run([sys.executable, os.path.join(HERE, "py_dump_graph.py"), path, "--seq", *extra])
        rust = run([exe, path, "--seq", *extra])
        problems = []
        walk(py, rust, "$", problems)
        if problems:
            diff += 1
            if diff <= 4:
                print(f"--- {os.path.basename(path)}")
                print("\n".join(problems[:8]))
        else:
            same += 1
    print(f"same: {same}, different: {diff}")
    sys.exit(1 if diff else 0)


if __name__ == "__main__":
    main()
