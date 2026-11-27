"""Build the Rust app (native/) for this computer's system: the program, an archive and the installer.

    python tools/build_rust.py [--version X.Y.Z] [--no-installer]
    python tools/build_rust.py --current           # print the version (native/Cargo.toml)

Steps: cargo build --release (the version is built in through PROXYDOCK_VERSION), then the program with the
tray + core (proxydock), the window (proxydock-ui), bin/<os>-<arch> (sing-box, Hysteria; from bin/ - fetch them
with tools/fetch_singbox.py first) and licenses/, then into dist/release-rust, each with a .sha256:

  Windows  dist/rust/ProxyDock          -> ProxyDock-<version>-windows-x64.zip and -setup.exe (Inno Setup 6, the
           same tools/ProxyDock.iss as the Python app: same AppId, so it installs over a Python install)
  Linux    dist/rust/ProxyDock          -> ProxyDock-<version>-linux-<arch>.tar.gz and .AppImage (appimagetool,
           downloaded if missing)
  macOS    dist/rust/ProxyDock.app      -> ProxyDock-<version>-macos-<arch>.zip and .dmg (hdiutil)

The same names as the Python app's packages. Without --version the version in native/Cargo.toml
([workspace.package]) is used.
"""
import argparse
import hashlib
import os
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import urllib.request
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NATIVE = os.path.join(ROOT, "native")
DIST = os.path.join(ROOT, "dist", "rust")
# (apart from the Python builds in dist/release)
RELEASE = os.path.join(ROOT, "dist", "release-rust")
SYSTEM = "windows" if sys.platform == "win32" else "macos" if sys.platform == "darwin" else "linux"
EXE = ".exe" if SYSTEM == "windows" else ""
# the licences that apply to what the program ships
LICENSES = ["README.txt", "rust-crates.txt", "sing-box-GPL-3.0.txt", "Hysteria-MIT.txt",
            "JetBrainsMono-OFL-1.1.txt", "Pretendard-OFL-1.1.txt", "MaterialSymbols-Apache-2.0.txt"]
APPIMAGETOOL_URL = "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-{arch}.AppImage"
DESKTOP = """[Desktop Entry]
Type=Application
Name=ProxyDock
Comment=Local proxy manager
Exec=proxydock
Icon=proxydock
Categories=Network;
Terminal=false
"""
APPRUN = """#!/bin/sh
HERE="$(dirname "$(readlink -f "$0")")"
exec "$HERE/usr/lib/proxydock/proxydock" "$@"
"""
INFO_PLIST = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>ProxyDock</string>
    <key>CFBundleDisplayName</key><string>ProxyDock</string>
    <key>CFBundleIdentifier</key><string>com.proxydock.app</string>
    <key>CFBundleExecutable</key><string>proxydock</string>
    <key>CFBundleIconFile</key><string>ProxyDock</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>{version}</string>
    <key>CFBundleVersion</key><string>{version}</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>LSMinimumSystemVersion</key><string>12.0</string>
    <!-- a menu bar (tray) app: no Dock icon for the tray process -->
    <key>LSUIElement</key><true/>
</dict>
</plist>
"""

# the Direct line on macOS runs a small Linux VM with Virtualization.framework, which needs this entitlement on the
# program that starts it (proxydock, whose helper mode runs as root); an ad-hoc signature carries it
ENTITLEMENTS = """<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.virtualization</key><true/>
</dict>
</plist>
"""


def say(step, msg):
    print(f"[rust {step}] {msg}", flush=True)


def cargo_version():
    with open(os.path.join(NATIVE, "Cargo.toml"), encoding="utf-8") as f:
        return re.search(r'^version = "([^"]+)"', f.read(), re.M).group(1)


def arch():
    m = platform.machine().lower()
    return {"amd64": "x64", "x86_64": "x64", "arm64": "arm64", "aarch64": "arm64"}.get(m, m)


def engines_dir():
    """bin/<os>-<arch>, as tools/fetch_singbox.py names it and the app looks for it"""
    os_name = {"windows": "windows", "macos": "darwin", "linux": "linux"}[SYSTEM]
    return f"{os_name}-{'amd64' if arch() == 'x64' else arch()}"


def release_name(version):
    return f"ProxyDock-{version}-{SYSTEM}-{arch()}"


def run(cmd, **kw):
    r = subprocess.run(cmd, **kw)
    if r.returncode != 0:
        raise SystemExit(f"failed: {' '.join(map(str, cmd))}")
    return r


def cargo(version):
    say("1/4", f"cargo build --release (version {version})")
    env = dict(os.environ, PROXYDOCK_VERSION=version)
    run(["cargo", "build", "--release", "-p", "proxydock"], cwd=NATIVE, env=env)


def icon_pngs(folder, sizes):
    """the app icon as <size>.png files (drawn by the app's own code)"""
    run(["cargo", "run", "-q", "--release", "-p", "proxydock", "--example", "make_png", "--", folder]
        + [str(s) for s in sizes], cwd=NATIVE)


def clear(path):
    """an old build out of the way (a running copy of it keeps its files)"""
    try:
        shutil.rmtree(path)
    except FileNotFoundError:
        pass
    except OSError as e:
        raise SystemExit(f"cannot replace {os.path.relpath(path, ROOT)} ({e.strerror}): is it running? Quit it first")


def fill(app_dir, licenses_dir):
    """the programs, the engines and the licences into a program folder"""
    target = os.path.join(NATIVE, "target", "release")
    engines = os.path.join(ROOT, "bin", engines_dir())
    for exe in ("sing-box", "hysteria"):
        if not os.path.isfile(os.path.join(engines, exe + EXE)):
            raise SystemExit(f"{exe}{EXE} not in bin/{engines_dir()} - run: python tools/fetch_singbox.py")
    os.makedirs(os.path.join(app_dir, "bin", engines_dir()))
    os.makedirs(licenses_dir, exist_ok=True)
    # (Windows file names ignore case: ProxyDock.exe is the program the app starts its window next to)
    shutil.copy2(os.path.join(target, "proxydock" + EXE),
                 os.path.join(app_dir, "ProxyDock.exe" if SYSTEM == "windows" else "proxydock"))
    shutil.copy2(os.path.join(target, "proxydock-ui" + EXE), os.path.join(app_dir, "proxydock-ui" + EXE))
    for exe in ("sing-box", "hysteria"):
        shutil.copy2(os.path.join(engines, exe + EXE), os.path.join(app_dir, "bin", engines_dir(), exe + EXE))
    for name in LICENSES:
        src = os.path.join(ROOT, "licenses", name)
        if os.path.isfile(src):
            shutil.copy2(src, os.path.join(licenses_dir, name))
    shutil.copy2(os.path.join(ROOT, "LICENSE"), os.path.join(os.path.dirname(licenses_dir), "LICENSE.txt"))


def folder(version):
    """the program folder (Windows, Linux) or ProxyDock.app (macOS); returns its path"""
    if SYSTEM == "macos":
        app = os.path.join(DIST, "ProxyDock.app")
        say("2/4", f"app bundle -> {os.path.relpath(app, ROOT)}")
        clear(app)
        contents = os.path.join(app, "Contents")
        # the programs and the engines in Contents/MacOS (the app looks for bin/ next to itself)
        fill(os.path.join(contents, "MacOS"), os.path.join(contents, "Resources", "licenses"))
        with open(os.path.join(contents, "Info.plist"), "w", encoding="utf-8") as f:
            f.write(INFO_PLIST.format(version=version))
        iconset = os.path.join(ROOT, "build", "rust", "ProxyDock.iconset")
        shutil.rmtree(iconset, ignore_errors=True)
        icon_pngs(iconset, [16, 32, 64, 128, 256, 512, 1024])
        # (iconutil's names: icon_<n>x<n>.png and icon_<n>x<n>@2x.png)
        for n in (16, 32, 128, 256, 512):
            shutil.copy2(os.path.join(iconset, f"{n}.png"), os.path.join(iconset, f"icon_{n}x{n}.png"))
            shutil.copy2(os.path.join(iconset, f"{n * 2}.png"), os.path.join(iconset, f"icon_{n}x{n}@2x.png"))
        for n in (16, 32, 64, 128, 256, 512, 1024):
            os.remove(os.path.join(iconset, f"{n}.png"))
        run(["iconutil", "-c", "icns", iconset, "-o", os.path.join(contents, "Resources", "ProxyDock.icns")])
        # ad-hoc signature: the bundle (and its main program, proxydock) with the Virtualization entitlement
        ent = os.path.join(ROOT, "build", "rust", "entitlements.plist")
        with open(ent, "w", encoding="utf-8") as f:
            f.write(ENTITLEMENTS)
        run(["codesign", "--force", "--sign", "-", "--entitlements", ent, os.path.join(contents, "MacOS", "proxydock")])
        run(["codesign", "--force", "--sign", "-", "--entitlements", ent, app])
        return app
    app = os.path.join(DIST, "ProxyDock")
    say("2/4", f"program folder -> {os.path.relpath(app, ROOT)}")
    clear(app)
    fill(app, os.path.join(app, "licenses"))
    return app


def checksum(path):
    digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
    with open(path + ".sha256", "w", encoding="ascii", newline="\n") as f:     # LF: sha256sum -c works anywhere
        f.write(f"{digest}  {os.path.basename(path)}\n")
    print(f"          {os.path.relpath(path, ROOT)}  sha256={digest[:16]}...")
    return path


def package(version, app):
    """zip (Windows), tar.gz (Linux: keeps the executable bits) or a ditto zip (macOS: keeps the bundle)"""
    os.makedirs(RELEASE, exist_ok=True)
    name = release_name(version)
    if SYSTEM == "linux":
        say("3/4", "tar.gz")
        path = os.path.join(RELEASE, name + ".tar.gz")
        with tarfile.open(path, "w:gz") as tar:
            for entry in sorted(os.listdir(app)):
                tar.add(os.path.join(app, entry), arcname=f"{name}/{entry}")
        return checksum(path)
    if SYSTEM == "macos":
        say("3/4", "zip (ditto)")
        path = os.path.join(RELEASE, name + ".zip")
        if os.path.exists(path):
            os.remove(path)
        run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", app, path])
        return checksum(path)
    say("3/4", "zip")
    path = os.path.join(RELEASE, name + ".zip")
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as z:
        for base, dirs, files in os.walk(app):
            dirs.sort()
            for fn in sorted(files):
                full = os.path.join(base, fn)
                z.write(full, f"{name}/{os.path.relpath(full, app)}".replace(os.sep, "/"))
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


def windows_installer(version, app):
    say("4/4", "installer (Inno Setup)")
    iscc = find_iscc()
    if not iscc:
        raise SystemExit("Inno Setup 6 is not installed (winget install JRSoftware.InnoSetup)")
    name = release_name(version) + "-setup"
    run([iscc, "/Q", f"/DAppVersion={version}", f"/DSourceDir={app}", f"/DOutputDir={RELEASE}",
         f"/DOutputName={name}", os.path.join("tools", "ProxyDock.iss")], cwd=ROOT)
    return checksum(os.path.join(RELEASE, name + ".exe"))


def dmg(version, app):
    """macOS disk image: ProxyDock.app and a link to /Applications to drag it onto"""
    say("4/4", "disk image (hdiutil)")
    staging = os.path.join(ROOT, "build", "rust", "dmg")
    shutil.rmtree(staging, ignore_errors=True)
    os.makedirs(staging)
    shutil.copytree(app, os.path.join(staging, "ProxyDock.app"), symlinks=True)
    os.symlink("/Applications", os.path.join(staging, "Applications"))
    path = os.path.join(RELEASE, release_name(version) + ".dmg")
    run(["hdiutil", "create", "-volname", "ProxyDock", "-srcfolder", staging, "-fs", "HFS+",
         "-format", "UDZO", "-imagekey", "zlib-level=9", "-ov", path])
    return checksum(path)


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


def appimage(version, app):
    """Linux AppImage: one file that runs on most distributions"""
    say("4/4", "AppImage (appimagetool)")
    appdir = os.path.join(ROOT, "build", "rust", "ProxyDock.AppDir")
    shutil.rmtree(appdir, ignore_errors=True)
    shutil.copytree(app, os.path.join(appdir, "usr", "lib", "proxydock"), symlinks=True)
    with open(os.path.join(appdir, "AppRun"), "w", encoding="utf-8", newline="\n") as f:
        f.write(APPRUN)
    os.chmod(os.path.join(appdir, "AppRun"), 0o755)
    with open(os.path.join(appdir, "proxydock.desktop"), "w", encoding="utf-8", newline="\n") as f:
        f.write(DESKTOP)
    icons = os.path.join(ROOT, "build", "rust", "icons")
    icon_pngs(icons, [256])
    shutil.copy2(os.path.join(icons, "256.png"), os.path.join(appdir, "proxydock.png"))
    os.symlink("proxydock.png", os.path.join(appdir, ".DirIcon"))
    path = os.path.join(RELEASE, release_name(version) + ".AppImage")
    env = dict(os.environ, APPIMAGE_EXTRACT_AND_RUN="1",      # no FUSE needed (containers, CI)
               ARCH={"x64": "x86_64", "arm64": "aarch64"}.get(arch(), arch()))
    run([appimagetool(), "--no-appstream", appdir, path], env=env)
    return checksum(path)


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--version", default=None, help="the version to build in (default: native/Cargo.toml's)")
    ap.add_argument("--no-installer", action="store_true", help="no installer / .dmg / .AppImage")
    ap.add_argument("--current", action="store_true", help="print the version in native/Cargo.toml and stop")
    args = ap.parse_args()
    if args.current:
        print(cargo_version())
        return
    version = args.version or cargo_version()
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise SystemExit(f"not a version: {version}")
    cargo(version)
    app = folder(version)
    package(version, app)
    if not args.no_installer:
        {"windows": windows_installer, "macos": dmg, "linux": appimage}[SYSTEM](version, app)
    print("done")


if __name__ == "__main__":
    main()
