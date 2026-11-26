"""Build the Rust app (native/) for Windows: the program folder, a zip and the installer.

    python tools/build_rust.py [--version X.Y.Z] [--no-installer]
    python tools/build_rust.py --current           # print the version (native/Cargo.toml)

Steps: cargo build --release (the version is built in through PROXYDOCK_VERSION), then dist/rust/ProxyDock with
ProxyDock.exe (the tray + core), proxydock-ui.exe (the window), bin/windows-amd64 (sing-box, Hysteria; from
bin/ - fetch them with tools/fetch_singbox.py first) and licenses/, then ProxyDock-<version>-windows-x64.zip
and -setup.exe in dist/release-rust (Inno Setup 6, the same tools/ProxyDock.iss as the Python app: same AppId, so it installs over a
Python install, and the same settings folder %APPDATA%\\ProxyDock), each with a .sha256.

Without --version the version in native/Cargo.toml ([workspace.package]) is used.
"""
import argparse
import hashlib
import os
import re
import shutil
import subprocess
import sys
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NATIVE = os.path.join(ROOT, "native")
APP = os.path.join(ROOT, "dist", "rust", "ProxyDock")
# (apart from the Python builds in dist/release)
RELEASE = os.path.join(ROOT, "dist", "release-rust")
ENGINES = os.path.join(ROOT, "bin", "windows-amd64")
# the licences that apply to what this folder ships (the Python packages' are left out)
LICENSES = ["README.txt", "rust-crates.txt", "sing-box-GPL-3.0.txt", "Hysteria-MIT.txt", "LGPL-3.0.txt",
            "JetBrainsMono-OFL-1.1.txt", "Pretendard-OFL-1.1.txt", "NotoSans-OFL-1.1.txt", "MaterialSymbols-Apache-2.0.txt"]


def say(step, msg):
    print(f"[rust {step}] {msg}", flush=True)


def cargo_version():
    with open(os.path.join(NATIVE, "Cargo.toml"), encoding="utf-8") as f:
        return re.search(r'^version = "([^"]+)"', f.read(), re.M).group(1)


def cargo(version):
    say("1/4", f"cargo build --release (version {version})")
    env = dict(os.environ, PROXYDOCK_VERSION=version)
    if subprocess.run(["cargo", "build", "--release", "-p", "proxydock"], cwd=NATIVE, env=env).returncode != 0:
        raise SystemExit("cargo build failed")


def folder():
    say("2/4", f"program folder -> {os.path.relpath(APP, ROOT)}")
    for exe in ("sing-box.exe", "hysteria.exe"):
        if not os.path.isfile(os.path.join(ENGINES, exe)):
            raise SystemExit(f"{exe} not in bin/windows-amd64 - run: python tools/fetch_singbox.py")
    shutil.rmtree(APP, ignore_errors=True)
    os.makedirs(os.path.join(APP, "bin", "windows-amd64"))
    os.makedirs(os.path.join(APP, "licenses"))
    target = os.path.join(NATIVE, "target", "release")
    # (Windows file names ignore case: ProxyDock.exe is the same program the app starts its window next to)
    shutil.copy2(os.path.join(target, "proxydock.exe"), os.path.join(APP, "ProxyDock.exe"))
    shutil.copy2(os.path.join(target, "proxydock-ui.exe"), os.path.join(APP, "proxydock-ui.exe"))
    for exe in ("sing-box.exe", "hysteria.exe"):
        shutil.copy2(os.path.join(ENGINES, exe), os.path.join(APP, "bin", "windows-amd64", exe))
    for name in LICENSES:
        src = os.path.join(ROOT, "licenses", name)
        if os.path.isfile(src):
            shutil.copy2(src, os.path.join(APP, "licenses", name))
    shutil.copy2(os.path.join(ROOT, "LICENSE"), os.path.join(APP, "LICENSE.txt"))


def checksum(path):
    digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
    with open(path + ".sha256", "w", encoding="ascii", newline="\n") as f:
        f.write(f"{digest}  {os.path.basename(path)}\n")
    print(f"          {os.path.relpath(path, ROOT)}  sha256={digest[:16]}...")
    return path


def package(version):
    say("3/4", "zip")
    os.makedirs(RELEASE, exist_ok=True)
    name = f"ProxyDock-{version}-windows-x64"
    path = os.path.join(RELEASE, name + ".zip")
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for base, dirs, files in os.walk(APP):
            dirs.sort()
            for fn in sorted(files):
                full = os.path.join(base, fn)
                z.write(full, f"{name}/{os.path.relpath(full, APP)}".replace(os.sep, "/"))
    return checksum(path)


def find_iscc():
    found = shutil.which("ISCC") or shutil.which("iscc")
    if found:
        return found
    import glob
    for base in (os.environ.get("ProgramFiles(x86)"), os.environ.get("ProgramFiles"),
                 os.path.join(os.environ.get("LOCALAPPDATA", ""), "Programs")):
        hits = sorted(glob.glob(os.path.join(base, "Inno Setup *", "ISCC.exe"))) if base else []
        if hits:
            return hits[0]
    return None


def installer(version):
    say("4/4", "installer (Inno Setup)")
    iscc = find_iscc()
    if not iscc:
        raise SystemExit("Inno Setup 6 is not installed (winget install JRSoftware.InnoSetup)")
    name = f"ProxyDock-{version}-windows-x64-setup"
    r = subprocess.run([iscc, "/Q", f"/DAppVersion={version}", f"/DSourceDir={APP}", f"/DOutputDir={RELEASE}",
                        f"/DOutputName={name}", os.path.join("tools", "ProxyDock.iss")], cwd=ROOT)
    if r.returncode != 0:
        raise SystemExit("Inno Setup failed (tools/ProxyDock.iss)")
    return checksum(os.path.join(RELEASE, name + ".exe"))


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--version", default=None, help="the version to build in (default: native/Cargo.toml's)")
    ap.add_argument("--no-installer", action="store_true")
    ap.add_argument("--current", action="store_true", help="print the version in native/Cargo.toml and stop")
    args = ap.parse_args()
    if args.current:
        print(cargo_version())
        return
    if sys.platform != "win32":
        raise SystemExit("the Rust app is Windows-only for now")
    version = args.version or cargo_version()
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise SystemExit(f"not a version: {version}")
    cargo(version)
    folder()
    package(version)
    if not args.no_installer:
        installer(version)
    print("done")


if __name__ == "__main__":
    main()
