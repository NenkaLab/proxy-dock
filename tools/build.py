"""ProxyDock build tool.

    python tools/build.py win|linux|mac|all
    python tools/build.py package win|linux|mac
    python tools/build.py installer | dmg | appimage

  win    Windows app folder   -> dist/win/ProxyDock/ProxyDock.exe   (on Windows only)
  linux  Linux app folder     -> dist/linux/ProxyDock/ProxyDock     (natively on Linux, with Docker on Windows/macOS)
  mac    macOS app bundle     -> dist/mac/ProxyDock.app             (on macOS only)
  all    build everything this computer can and print a summary
  package <target>  archive an existing build for a release -> dist/release/ProxyDock-<version>-<os>-<arch>.zip/.tar.gz
                    (+ .sha256), used by the release workflow (.github/workflows/release.yml)
  installer         Windows installer from an existing win build (Inno Setup 6, tools/ProxyDock.iss)
                    -> dist/release/ProxyDock-<version>-windows-<arch>-setup.exe (+ .sha256)
  dmg               macOS disk image from an existing mac build (hdiutil, on macOS)
                    -> dist/release/ProxyDock-<version>-macos-<arch>.dmg (+ .sha256)
  appimage          Linux AppImage from an existing linux build (appimagetool, downloaded if missing, on Linux)
                    -> dist/release/ProxyDock-<version>-linux-<arch>.AppImage (+ .sha256)

Creates a virtual environment per OS (.venv / .venv-linux / .venv-mac), installs only the packages that are
missing or differ from requirements.txt, then builds with tools/ProxyDock.spec.
The batch/shell scripts (tools/build*.bat, tools/build*.sh) only call this file (all logic lives here so output does not
depend on the console code page).
"""
import hashlib
import os
import platform
import re
import shutil
import subprocess
import sys
import stat
import tarfile
import urllib.request
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SPEC = "tools/ProxyDock.spec"
DOCKER_IMAGE = "proxydock-linux-build"
PLATFORM = "win" if sys.platform == "win32" else "mac" if sys.platform == "darwin" else "linux"
NAMES = {"win": "Windows", "linux": "Linux", "mac": "macOS"}
OUTPUTS = {"win": "dist/win/ProxyDock/ProxyDock.exe", "linux": "dist/linux/ProxyDock/ProxyDock", "mac": "dist/mac/ProxyDock.app"}


def app_folder(target):
    """What gets shipped: the app folder (win/linux) or dist/mac (holds ProxyDock.app + licenses)."""
    return os.path.join(ROOT, "dist", target) if target == "mac" else os.path.join(ROOT, "dist", target, "ProxyDock")


class Skip(Exception):
    """This computer cannot build this target (not a failure)."""


def say(target, step, msg):
    print(f"[{target} {step}] {msg}", flush=True)


def run(cmd, **kw):
    print("    $ " + " ".join(cmd), flush=True) if os.environ.get("BUILD_VERBOSE") else None
    return subprocess.run(cmd, cwd=ROOT, **kw)


def venv_python(target, quiet=False):
    default = {"win": ".venv", "linux": ".venv-linux", "mac": ".venv-mac"}[target]
    venv = os.environ.get("VENV_DIR") or os.path.join(ROOT, default)
    py = os.path.join(venv, "Scripts", "python.exe") if target == "win" else os.path.join(venv, "bin", "python")
    if quiet:
        if not os.path.exists(py):
            raise RuntimeError(f"virtual environment not found: {venv} - build first")
        return py
    if os.path.exists(py):
        say(target, "1/5", f"checking virtual environment: {os.path.relpath(venv, ROOT) if venv.startswith(ROOT) else venv}")
    else:
        if sys.version_info[:2] < (3, 14):
            print(f"    note: creating the virtual environment with Python {sys.version.split()[0]} (3.14 recommended)")
        say(target, "1/5", f"Python {sys.version.split()[0]} creating virtual environment: {venv}")
        if run([sys.executable, "-m", "venv", venv]).returncode != 0:
            raise RuntimeError("could not create the virtual environment")
    return py


def check_requirements(target, py):
    say(target, "2/5", "checking requirements.txt...")
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


def engines(target, py):
    """sing-box and the official Hysteria2 core go into the app; download them (SHA-256 checked) if missing."""
    r = run([py, "-c", "from proxydock.singbox import EXE, HY_EXE, platform_dir, SINGBOX_VERSION, HYSTERIA_VERSION;"
                       "print(platform_dir(), EXE, HY_EXE, SINGBOX_VERSION, HYSTERIA_VERSION)"],
            capture_output=True, text=True, encoding="utf-8", errors="replace")
    if r.returncode != 0:
        raise RuntimeError("could not read the engine versions: " + r.stderr.strip()[-300:])
    pdir, exe, hy, sbv, hyv = r.stdout.split()
    folder = os.path.join(ROOT, "bin", pdir)
    if all(os.path.isfile(os.path.join(folder, n)) for n in (exe, hy)):
        say(target, "3/5", f"engines: sing-box {sbv}, Hysteria {hyv} (bin/{pdir})")
        return
    say(target, "3/5", f"downloading engines: sing-box {sbv}, Hysteria {hyv} -> bin/{pdir}")
    if run([py, "tools/fetch_singbox.py", pdir]).returncode != 0:
        raise RuntimeError("could not download sing-box / hysteria (tools/fetch_singbox.py)")


def pyinstaller(target, py):
    say(target, "4/5", "building...")
    for old in ("ProxyDock.exe", "licenses"):      # left over from the old single-file build
        path = os.path.join(ROOT, "dist", target, old)
        try:
            if target != "mac" and os.path.isfile(path):
                os.remove(path)
            elif target != "mac" and os.path.isdir(path):
                shutil.rmtree(path)
        except OSError as e:
            print(f"          note: could not remove the old dist/{target}/{old} ({e.strerror}); delete it later")
    r = run([py, "-m", "PyInstaller", "--noconfirm", "--clean", "--log-level", "WARN",
             "--distpath", f"dist/{target}", "--workpath", f"build/{target}", SPEC])
    if r.returncode != 0:
        raise RuntimeError("PyInstaller build failed")
    if target == "mac":     # intermediate folder left over after creating the .app bundle
        shutil.rmtree(os.path.join(ROOT, "dist", "mac", "ProxyDock"), ignore_errors=True)


def copy_licenses(target):
    say(target, "5/5", "copying licenses...")
    dst = os.path.join(app_folder(target), "licenses")
    os.makedirs(dst, exist_ok=True)
    src = os.path.join(ROOT, "licenses")
    for name in os.listdir(src):
        if name.endswith(".txt"):
            shutil.copy2(os.path.join(src, name), dst)


def build_native(target):
    py = venv_python(target)
    check_requirements(target, py)
    engines(target, py)
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
    say("linux", "0/5", "preparing the build image (slow the first time only)...")
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


def version():
    with open(os.path.join(ROOT, "proxydock", "__init__.py"), encoding="utf-8") as f:
        return re.search(r'__version__ = "([^"]+)"', f.read()).group(1)


def arch():
    m = platform.machine().lower()
    return {"amd64": "x64", "x86_64": "x64", "arm64": "arm64", "aarch64": "arm64"}.get(m, m)


def package(target):
    """Archive dist/<target> for a release. Returns the archive path."""
    src = app_folder(target)
    main = os.path.join(ROOT, OUTPUTS[target])
    if not os.path.exists(main):
        raise RuntimeError(f"{OUTPUTS[target]} not found - build it first")
    out_dir = os.path.join(ROOT, "dist", "release")
    os.makedirs(out_dir, exist_ok=True)
    os_name = {"win": "windows", "linux": "linux", "mac": "macos"}[target]
    name = f"ProxyDock-{version()}-{os_name}-{arch()}"
    if target == "linux":           # tar keeps the executable bit
        path = os.path.join(out_dir, name + ".tar.gz")
        with tarfile.open(path, "w:gz") as tar:
            for entry in sorted(os.listdir(src)):
                tar.add(os.path.join(src, entry), arcname=f"{name}/{entry}")
    elif target == "mac":           # ditto keeps the symlinks and permissions inside the .app bundle
        path = os.path.join(out_dir, name + ".zip")
        if os.path.exists(path):
            os.remove(path)
        staging = os.path.join(ROOT, "build", "package", name)
        shutil.rmtree(staging, ignore_errors=True)
        shutil.copytree(src, staging, symlinks=True)
        if run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", staging, path]).returncode != 0:
            raise RuntimeError("ditto failed")
    else:
        path = os.path.join(out_dir, name + ".zip")
        with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
            for base, dirs, files in os.walk(src):
                dirs.sort()
                for fn in sorted(files):
                    full = os.path.join(base, fn)
                    z.write(full, f"{name}/{os.path.relpath(full, src)}".replace(os.sep, "/"))
    return checksum(path, "packaged")


def checksum(path, what):
    digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
    with open(path + ".sha256", "w", encoding="ascii") as f:
        f.write(f"{digest}  {os.path.basename(path)}\n")
    print(f"{what}: {os.path.relpath(path, ROOT)}  sha256={digest[:16]}...")
    return path


def find_iscc():
    """Inno Setup compiler: PATH, then the usual install folders (machine or per-user install)."""
    found = shutil.which("ISCC") or shutil.which("iscc")
    if found:
        return found
    import glob
    for base in (os.environ.get("ProgramFiles(x86)"), os.environ.get("ProgramFiles"),
                 os.path.join(os.environ.get("LOCALAPPDATA", ""), "Programs")):
        found = sorted(glob.glob(os.path.join(base, "Inno Setup *", "ISCC.exe"))) if base else []
        if found:
            return found[0]         # "Inno Setup 6" before 7 (tools/innosetup/*.isl are 6.x messages)
    return None


def installer():
    """Windows installer (per-user, no administrator needed) from dist/win/ProxyDock. Returns its path."""
    if PLATFORM != "win":
        raise Skip("the Windows installer can only be built on Windows")
    if not os.path.exists(os.path.join(ROOT, OUTPUTS["win"])):
        raise RuntimeError(f"{OUTPUTS['win']} not found - build it first (python tools/build.py win)")
    iscc = find_iscc()
    if not iscc:
        raise Skip("Inno Setup 6 is not installed (winget install JRSoftware.InnoSetup)")
    out_dir = os.path.join(ROOT, "dist", "release")
    os.makedirs(out_dir, exist_ok=True)
    name = release_name("windows") + "-setup"
    r = run([iscc, "/Q", f"/DAppVersion={version()}", f"/DSourceDir={app_folder('win')}",
             f"/DOutputDir={out_dir}", f"/DOutputName={name}", "tools/ProxyDock.iss"])
    if r.returncode != 0:
        raise RuntimeError("Inno Setup failed (tools/ProxyDock.iss)")
    return checksum(os.path.join(out_dir, name + ".exe"), "installer")


def release_name(os_name):
    return f"ProxyDock-{version()}-{os_name}-{arch()}"


def dmg():
    """macOS disk image: ProxyDock.app + a link to /Applications to drag it onto (+ licenses)."""
    if PLATFORM != "mac":
        raise Skip("a .dmg can only be made on macOS")
    app = os.path.join(ROOT, OUTPUTS["mac"])
    if not os.path.isdir(app):
        raise RuntimeError(f"{OUTPUTS['mac']} not found - build it first (python tools/build.py mac)")
    staging = os.path.join(ROOT, "build", "dmg")
    shutil.rmtree(staging, ignore_errors=True)
    os.makedirs(staging)
    shutil.copytree(app, os.path.join(staging, "ProxyDock.app"), symlinks=True)
    lic = os.path.join(ROOT, "dist", "mac", "licenses")
    if os.path.isdir(lic):
        shutil.copytree(lic, os.path.join(staging, "licenses"))
    os.symlink("/Applications", os.path.join(staging, "Applications"))
    out_dir = os.path.join(ROOT, "dist", "release")
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, release_name("macos") + ".dmg")
    if run(["hdiutil", "create", "-volname", "ProxyDock", "-srcfolder", staging, "-fs", "HFS+",
            "-format", "UDZO", "-imagekey", "zlib-level=9", "-ov", path]).returncode != 0:
        raise RuntimeError("hdiutil failed")
    return checksum(path, "disk image")


APPIMAGETOOL_URL = "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-{arch}.AppImage"
DESKTOP = """[Desktop Entry]
Type=Application
Name=ProxyDock
Comment=Local proxy manager
Exec=ProxyDock
Icon=proxydock
Categories=Network;
Terminal=false
"""
APPRUN = """#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/lib/proxydock/ProxyDock" "$@"
"""


def appimagetool():
    found = os.environ.get("APPIMAGETOOL") or shutil.which("appimagetool")
    if found:
        return found
    machine = {"x64": "x86_64", "arm64": "aarch64"}.get(arch(), arch())
    path = os.path.join(ROOT, "build", "tools", f"appimagetool-{machine}.AppImage")
    if not os.path.isfile(path):
        os.makedirs(os.path.dirname(path), exist_ok=True)
        url = APPIMAGETOOL_URL.format(arch=machine)
        print(f"          downloading {url}", flush=True)
        urllib.request.urlretrieve(url, path + ".part")
        os.replace(path + ".part", path)
    os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)
    return path


def appimage():
    """Linux AppImage: one file that runs on most distributions (mounted, not unpacked to /tmp)."""
    if PLATFORM != "linux":
        raise Skip("an AppImage can only be made on Linux (python tools/build.py linux builds in Docker)")
    folder = app_folder("linux")
    if not os.path.exists(os.path.join(ROOT, OUTPUTS["linux"])):
        raise RuntimeError(f"{OUTPUTS['linux']} not found - build it first (python tools/build.py linux)")
    appdir = os.path.join(ROOT, "build", "appimage", "ProxyDock.AppDir")
    shutil.rmtree(appdir, ignore_errors=True)
    shutil.copytree(folder, os.path.join(appdir, "usr", "lib", "proxydock"), symlinks=True)
    with open(os.path.join(appdir, "AppRun"), "w", encoding="utf-8", newline="\n") as f:
        f.write(APPRUN)
    os.chmod(os.path.join(appdir, "AppRun"), 0o755)
    with open(os.path.join(appdir, "proxydock.desktop"), "w", encoding="utf-8", newline="\n") as f:
        f.write(DESKTOP)
    icon = os.path.join(appdir, "proxydock.png")
    r = run([venv_python("linux", quiet=True), "-c",
             "import sys; from PySide6.QtWidgets import QApplication; app = QApplication(['x', '-platform', 'offscreen']);"
             "from proxydock.ui.common import make_icon; sys.exit(0 if make_icon().pixmap(256, 256).save(sys.argv[1]) else 1)",
             icon], capture_output=True, text=True, encoding="utf-8", errors="replace")
    if r.returncode != 0:
        raise RuntimeError("could not draw the icon: " + r.stderr.strip()[-300:])
    os.symlink("proxydock.png", os.path.join(appdir, ".DirIcon"))
    out_dir = os.path.join(ROOT, "dist", "release")
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, release_name("linux") + ".AppImage")
    env = dict(os.environ, APPIMAGE_EXTRACT_AND_RUN="1",      # no FUSE needed (containers, CI)
               ARCH={"x64": "x86_64", "arm64": "aarch64"}.get(arch(), arch()))
    if run([appimagetool(), "--no-appstream", appdir, path], env=env).returncode != 0:
        raise RuntimeError("appimagetool failed")
    return checksum(path, "AppImage")


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
    if len(argv) == 3 and argv[1] == "package" and argv[2] in ("win", "linux", "mac"):
        try:
            package(argv[2])
            return 0
        except RuntimeError as e:
            print(f"failed: {e}")
            return 1
    if len(argv) == 2 and argv[1] in ("installer", "dmg", "appimage"):
        try:
            {"installer": installer, "dmg": dmg, "appimage": appimage}[argv[1]]()
            return 0
        except Skip as e:
            print(f"skipped: {e}")
            return 2
        except RuntimeError as e:
            print(f"failed: {e}")
            return 1
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
