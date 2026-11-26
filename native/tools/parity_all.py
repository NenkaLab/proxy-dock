"""Every parity check against the Python app at once (CI runs this): random settings, links, edits and v2 files
read or changed by both apps and compared. Exits 1 when any check found a difference.

    python native/tools/parity_all.py [count]          (default 150 cases per check, English and Korean)

Needs the Python app (commit 1e882fc, see pyapp.py: PROXYDOCK_PY) with its requirements installed, and builds
the Rust examples (cargo build --examples -p proxydock-core) first.
"""
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
NATIVE = os.path.dirname(HERE)
EXAMPLES = os.path.join(NATIVE, "target", "debug", "examples")
EXE = ".exe" if os.name == "nt" else ""


def run(args, name):
    print(f"--- {name}", flush=True)
    r = subprocess.run([sys.executable, *args], cwd=NATIVE, env=dict(os.environ, PYTHONIOENCODING="utf-8"))
    if r.returncode != 0:
        print(f"*** {name}: differences or errors (exit {r.returncode})", flush=True)
    return r.returncode == 0


def main():
    count = sys.argv[1] if len(sys.argv) > 1 else "150"
    import pyapp  # noqa: F401  (fails early with a message when the Python app is missing)

    print("--- cargo build --examples", flush=True)
    if subprocess.run(["cargo", "build", "--examples", "-p", "proxydock-core"], cwd=NATIVE).returncode != 0:
        sys.exit("could not build the Rust examples")
    ex = lambda name: os.path.join(EXAMPLES, name + EXE)  # noqa: E731
    tool = lambda name: os.path.join(HERE, name)  # noqa: E731
    ok = True
    with tempfile.TemporaryDirectory(prefix="proxydock-parity-") as tmp:
        dirs = {}
        for kind in ("graph", "config", "singbox", "checks", "compile"):
            dirs[kind] = os.path.join(tmp, kind)
            ok &= run([tool(f"fuzz_{kind}.py"), dirs[kind], count], f"fuzz {kind}")
        for lang in ("en", "ko"):
            flag = [] if lang == "en" else ["--lang", lang]
            ok &= run([tool("parity_graph.py"), dirs["graph"], ex("dump_graph"), *flag], f"graph ({lang})")
            ok &= run([tool("parity_graph.py"), dirs["compile"], ex("dump_graph"), *flag], f"compile ({lang})")
            ok &= run([tool("parity_singbox.py"), dirs["singbox"], ex("dump_singbox"), *flag], f"sing-box ({lang})")
            ok &= run([tool("parity_checks.py"), dirs["checks"], ex("dump_checks"), *flag], f"checks ({lang})")
            ok &= run([tool("parity_links.py"), count, "--seed", "7", *flag], f"links ({lang})")
            ok &= run([tool("parity_edit.py"), count, "--seed", "7", *flag], f"edit ({lang})")
            ok &= run([tool("parity_v2.py"), ex("dump_v2"), count, "--seed", "7", *flag], f"settings v2 ({lang})")
        ok &= run([tool("parity_config.py"), dirs["config"], ex("dump_config")], "config")
        if os.name == "nt":  # (DPAPI: Windows only)
            ok &= run([tool("parity_save.py"), dirs["config"], ex("resave")], "save")
    print("all parity checks passed" if ok else "*** some parity checks found differences")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
