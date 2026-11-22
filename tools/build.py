"""ProxyDock build tool.

    python tools/build.py win|linux|mac|all

  win    Windows executable   -> dist/win/ProxyDock.exe   (on Windows only)
  linux  Linux executable     -> dist/linux/ProxyDock     (natively on Linux, with Docker on Windows/macOS)
  mac    macOS app bundle     -> dist/mac/ProxyDock.app   (on macOS only)
  all    build everything this computer can and print a summary

Creates a virtual environment per OS (.venv / .venv-linux / .venv-mac), installs only the packages that are
missing or differ from requirements.txt, then builds with tools/ProxyDock.spec.
The batch/shell scripts (tools/build*.bat, tools/build*.sh) only call this file (all logic lives here so output does not
depend on the console code page).
"""
import os
import shutil
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SPEC = "tools/ProxyDock.spec"
DOCKER_IMAGE = "proxydock-linux-build"
PLATFORM = "win" if sys.platform == "win32" else "mac" if sys.platform == "darwin" else "linux"
NAMES = {"win": "Windows", "linux": "Linux", "mac": "macOS"}
OUTPUTS = {"win": "dist/win/ProxyDock.exe", "linux": "dist/linux/ProxyDock", "mac": "dist/mac/ProxyDock.app"}


class Skip(Exception):
    """This computer cannot build this target (not a failure)."""


def say(target, step, msg):
    print(f"[{target} {step}] {msg}", flush=True)


def run(cmd, **kw):
    print("    $ " + " ".join(cmd), flush=True) if os.environ.get("BUILD_VERBOSE") else None
    return subprocess.run(cmd, cwd=ROOT, **kw)


def venv_python(target):
    default = {"win": ".venv", "linux": ".venv-linux", "mac": ".venv-mac"}[target]
    venv = os.environ.get("VENV_DIR") or os.path.join(ROOT, default)
    py = os.path.join(venv, "Scripts", "python.exe") if target == "win" else os.path.join(venv, "bin", "python")
    if os.path.exists(py):
        say(target, "1/4", f"checking virtual environment: {os.path.relpath(venv, ROOT) if venv.startswith(ROOT) else venv}")
    else:
        if sys.version_info[:2] < (3, 14):
            print(f"    note: creating the virtual environment with Python {sys.version.split()[0]} (3.14 recommended)")
        say(target, "1/4", f"Python {sys.version.split()[0]} creating virtual environment: {venv}")
        if run([sys.executable, "-m", "venv", venv]).returncode != 0:
            raise RuntimeError("could not create the virtual environment")
    return py


def check_requirements(target, py):
    say(target, "2/4", "checking requirements.txt...")
    if not os.path.exists(os.path.join(ROOT, "requirements.txt")):
        raise RuntimeError("requirements.txt not found")
    r = run([py, "-m", "pip", "install", "--dry-run", "--disable-pip-version-check", "-r", "requirements.txt"],
            capture_output=True, text=True, encoding="utf-8", errors="replace")
    if r.returncode != 0:
        print(r.stdout + r.stderr)
        raise RuntimeError("could not check requirements.txt")
    need = [l for l in r.stdout.splitlines() if l.startswith("Would install")]
    if not need:
        print("          all packages match requirements.txt.")
    else:
        for line in need:
            print(f"          to install: {line[len('Would install '):]}")
        print("          installing...", flush=True)
        if run([py, "-m", "pip", "install", "--disable-pip-version-check", "-q", "-r", "requirements.txt"]).returncode:
            raise RuntimeError("package installation failed")
    c = run([py, "-m", "pip", "check", "--disable-pip-version-check"], capture_output=True, text=True,
            encoding="utf-8", errors="replace")
    if c.returncode != 0:
        print("warning: installed packages have conflicting versions.\n" + c.stdout)


def pyinstaller(target, py):
    say(target, "3/4", "building...")
    r = run([py, "-m", "PyInstaller", "--noconfirm", "--clean", "--log-level", "WARN",
             "--distpath", f"dist/{target}", "--workpath", f"build/{target}", SPEC])
    if r.returncode != 0:
        raise RuntimeError("PyInstaller build failed")
    if target == "mac":     # intermediate folder left over after creating the .app bundle
        shutil.rmtree(os.path.join(ROOT, "dist", "mac", "ProxyDock"), ignore_errors=True)


def copy_licenses(target):
    say(target, "4/4", "copying licenses...")
    dst = os.path.join(ROOT, "dist", target, "licenses")
    os.makedirs(dst, exist_ok=True)
    src = os.path.join(ROOT, "licenses")
    for name in os.listdir(src):
        if name.endswith(".txt"):
            shutil.copy2(os.path.join(src, name), dst)


def build_native(target):
    py = venv_python(target)
    check_requirements(target, py)
    pyinstaller(target, py)
    copy_licenses(target)
    print(f"done: {OUTPUTS[target]}")
    if target == "mac":
        print("note: the app is not signed/notarized, so on other Macs the first launch may need right-click -> Open.")


def build_linux_docker():
    """Linux build inside a Docker container on Windows/macOS. The image and packages are cached."""
    if shutil.which("docker") is None:
        raise Skip("Docker is not installed (Docker Desktop required)")
    if run(["docker", "info"], capture_output=True).returncode != 0:
        raise Skip("Docker is not running (start Docker Desktop)")
    say("linux", "0/4", "preparing the build image (slow the first time only)...")
    if run(["docker", "build", "-q", "-t", DOCKER_IMAGE, "tools/docker/linux"], capture_output=True).returncode:
        raise RuntimeError("could not build the Docker image (docker build tools/docker/linux)")
    r = run(["docker", "run", "--rm",
             "-v", f"{ROOT}:/src",
             "-v", "proxydock-linux-venv:/opt/venv",        # virtualenv/package cache (faster from the next build on)
             "-v", "proxydock-pip-cache:/root/.cache/pip",
             "-e", "VENV_DIR=/opt/venv", "-e", "PYTHONIOENCODING=utf-8",
             "-w", "/src", DOCKER_IMAGE, "python", "tools/build.py", "linux"])
    if r.returncode != 0:
        raise RuntimeError("build failed inside the container")


def build(target):
    if target == PLATFORM:
        build_native(target)
    elif target == "linux":
        build_linux_docker()
    elif target == "mac":
        raise Skip("macOS apps can only be built on a Mac (tools/build_mac.sh on the Mac)")
    else:
        raise Skip("Windows executables can only be built on Windows (tools\\build_win.bat on Windows)")


def main(argv):
    if len(argv) != 2 or argv[1] not in ("win", "linux", "mac", "all"):
        print(__doc__)
        return 64
    target = argv[1]
    if target != "all":
        try:
            build(target)
            return 0
        except Skip as e:
            print(f"skipped: {e}")
            return 2
        except RuntimeError as e:
            print(f"failed: {e}")
            return 1

    results = {}
    for t in ("win", "linux", "mac"):
        try:
            build(t)
            results[t] = f"success - {OUTPUTS[t]}"
        except Skip as e:
            results[t] = f"skipped - {e}"
        except RuntimeError as e:
            results[t] = f"failed  - {e}"
        print()
    print("===== Build results =====")
    for t, r in results.items():
        print(f"  {NAMES[t]:8}: {r}")
    return 1 if any(r.startswith("failed") for r in results.values()) else 0


if __name__ == "__main__":
    # keep output readable in any Windows console/pipe
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8" if not stream.isatty() else stream.encoding, errors="replace")
        except (AttributeError, ValueError):
            pass
    sys.exit(main(sys.argv))
