"""Run both config readers on every file in a folder and compare (see fuzz_config.py).

    python native/tools/parity_config.py <dir> <dump_config.exe>
"""
import glob
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from compare_config import walk  # noqa: E402
import json  # noqa: E402


def run(cmd):
    r = subprocess.run(cmd, capture_output=True, encoding="utf-8", env=dict(os.environ, PYTHONIOENCODING="utf-8"))
    if r.returncode:
        raise RuntimeError(r.stderr.strip()[-500:])
    return json.loads(r.stdout)


def main():
    folder, exe = sys.argv[1], sys.argv[2]
    same, diff = 0, 0
    for path in sorted(glob.glob(os.path.join(folder, "*.json"))):
        py = run([sys.executable, os.path.join(HERE, "py_dump_config.py"), path])
        rust = run([exe, path])
        rust.pop("undecryptable", None)
        for k in ("theme_color", "flow_fins"):  # (Rust app only)
            rust.get("global", {}).pop(k, None)
        problems = []
        walk(py, rust, "$", problems)
        if problems:
            diff += 1
            if diff <= 5:
                print(f"--- {os.path.basename(path)}")
                print("\n".join(problems[:10]))
        else:
            same += 1
    print(f"same: {same}, different: {diff}")
    sys.exit(1 if diff else 0)


if __name__ == "__main__":
    main()
