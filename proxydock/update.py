"""Update check against the GitHub releases of ProxyDock. No Qt here (the UI side is ui/updater.py).

check() asks the GitHub API for the latest release (drafts and prereleases are not returned by /latest, and are
ignored anyway) and returns it when its tag is newer than this version. Only a generic User-Agent
("ProxyDock/<version>") is sent: no settings, IDs or other data.
"""
from .i18n import _t
import hashlib
import json
import ntpath
import os
import platform
import re
import shutil
import sys
import time
import urllib.request

from . import __version__

API_URL = "https://api.github.com/repos/NenkaLab/proxy-dock/releases/latest"
USER_AGENT = f"ProxyDock/{__version__}"
TIMEOUT = 20
MAX_JSON = 2 * 1024 * 1024
CHECK_INTERVAL = 24 * 3600          # automatic check at startup: at most once a day


def http_get(url, max_bytes=MAX_JSON, accept=None):
    """GET url -> bytes. Raises ValueError with a readable message. Tests replace this (no network)."""
    headers = {"User-Agent": USER_AGENT}
    if accept:
        headers["Accept"] = accept
    req = urllib.request.Request(url, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            data = r.read(max_bytes + 1)
    except Exception as e:
        raise ValueError(_t("Could not reach GitHub: {e}").format(e=e)) from None
    if len(data) > max_bytes:
        raise ValueError(_t("The response from GitHub is too large"))
    return data


def parse_version(tag):
    """"v2.2.1" / "2.2.1" -> (2, 2, 1); None for anything else (prerelease suffixes like -rc1 included)."""
    m = re.fullmatch(r"v?(\d+)\.(\d+)\.(\d+)", (tag or "").strip())
    return tuple(int(x) for x in m.groups()) if m else None


def is_newer(tag, current=__version__):
    new, cur = parse_version(tag), parse_version(current)
    return new is not None and cur is not None and new > cur


def parse_release(data):
    """GitHub release JSON (bytes, str or dict) -> release dict, or None for a draft, a prerelease or a tag that is
    not a plain version. Release: {"version", "tag", "name", "notes", "url", "assets": {name: {"url", "size"}}}."""
    if isinstance(data, (bytes, str)):
        try:
            data = json.loads(data)
        except ValueError:
            raise ValueError(_t("GitHub returned an unexpected answer")) from None
    if not isinstance(data, dict):
        raise ValueError(_t("GitHub returned an unexpected answer"))
    if data.get("draft") or data.get("prerelease"):
        return None
    tag = data.get("tag_name") if isinstance(data.get("tag_name"), str) else ""
    version = parse_version(tag)
    if version is None:
        return None
    assets = {}
    for a in data.get("assets") or []:
        if isinstance(a, dict) and isinstance(a.get("name"), str) and isinstance(a.get("browser_download_url"), str) \
                and a["browser_download_url"].startswith("https://"):
            assets[a["name"]] = {"url": a["browser_download_url"], "size": a.get("size") if isinstance(a.get("size"), int) else 0}
    url = data.get("html_url") if isinstance(data.get("html_url"), str) and data["html_url"].startswith("https://") else ""
    return {
        "version": ".".join(map(str, version)),
        "tag": tag,
        "name": data.get("name") if isinstance(data.get("name"), str) else tag,
        "notes": (data.get("body") if isinstance(data.get("body"), str) else "").strip(),
        "url": url,
        "assets": assets,
    }


def latest(fetch=None):
    """The latest published release (dict, see parse_release) or None. Raises ValueError."""
    return parse_release((fetch or http_get)(API_URL, accept="application/vnd.github+json"))


def check(current=__version__, fetch=None):
    """-> the latest release if it is newer than `current`, else None. Raises ValueError."""
    rel = latest(fetch)
    return rel if rel is not None and is_newer(rel["tag"], current) else None


def due(glob, now=None):
    """True when the automatic startup check should run (switched on and the last check is a day old)."""
    now = time.time() if now is None else now
    last = glob.get("update_checked", 0)
    return bool(glob.get("check_updates", True)) and (now - last >= CHECK_INTERVAL or last > now)


# --- in-app update: pick the file for this computer, download, verify, install ---

SUMS_NAME = "SHA256SUMS.txt"
MAX_SUMS = 64 * 1024
CHUNK = 256 * 1024
SUFFIXES = {"installer": "-setup.exe", "dmg": ".dmg", "appimage": ".AppImage"}


def platform_key(sys_platform=None, machine=None):
    """-> (os name, arch) as used in the release file names: ("windows"|"macos"|"linux", "x64"|"arm64")."""
    p = sys.platform if sys_platform is None else sys_platform
    m = (platform.machine() if machine is None else machine).lower()
    os_name = "windows" if p == "win32" else "macos" if p == "darwin" else "linux"
    return os_name, {"amd64": "x64", "x86_64": "x64", "aarch64": "arm64", "arm64": "arm64"}.get(m, m)


def install_kind(os_name, appimage=False, installed=False):
    """How this copy of ProxyDock can update itself: "installer" (Windows, installed with the setup program),
    "dmg" (macOS app bundle), "appimage" (Linux AppImage), or None (zip/tar.gz copy or a source checkout: open the
    release page instead)."""
    if os_name == "windows":
        return "installer" if installed else None
    if os_name == "macos":
        return "dmg" if installed else None
    return "appimage" if appimage else None


def pick_asset(release, os_name, arch, kind):
    """Name of the release file to install for (os_name, arch, kind), or None."""
    if kind not in SUFFIXES:
        return None
    want = f"ProxyDock-{release['version']}-{os_name}-{arch}{SUFFIXES[kind]}"
    if want in release["assets"]:
        return want
    tail = f"-{os_name}-{arch}{SUFFIXES[kind]}"         # tolerate a differently written version in the file name
    return next((n for n in sorted(release["assets"]) if n.startswith("ProxyDock-") and n.endswith(tail)), None)


def current_install():
    """-> (os name, arch, kind) for the running program (see install_kind)."""
    os_name, arch = platform_key()
    frozen = bool(getattr(sys, "frozen", False))
    appimage = frozen and bool(os.environ.get("APPIMAGE"))
    if os_name == "windows":       # the Inno Setup uninstaller sits next to an installed ProxyDock.exe
        installed = frozen and os.path.isfile(os.path.join(os.path.dirname(sys.executable), "unins000.exe"))
    elif os_name == "macos":
        installed = frozen and ".app/Contents/" in sys.executable
    else:
        installed = False
    return os_name, arch, install_kind(os_name, appimage, installed)


def parse_sums(text):
    """SHA256SUMS.txt ("<hex>  <name>" or "<hex> *<name>" per line) -> {name: hex}."""
    if isinstance(text, bytes):
        text = text.decode("utf-8", errors="replace")
    sums = {}
    for line in text.splitlines():
        m = re.fullmatch(r"([0-9a-fA-F]{64}) [ *](.+)", line.strip())
        if m:
            sums[m.group(2).strip()] = m.group(1).lower()
    return sums


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(CHUNK), b""):
            h.update(block)
    return h.hexdigest()


def verify(path, name, sums):
    """Raise ValueError unless the file's SHA-256 matches the entry for `name` in `sums` (dict or the file's text)."""
    if not isinstance(sums, dict):
        sums = parse_sums(sums)
    want = sums.get(name)
    if not want:
        raise ValueError(_t("{name} is not listed in {sums}, so it cannot be verified").format(name=name, sums=SUMS_NAME))
    if sha256_file(path) != want:
        raise ValueError(_t("The downloaded file is damaged or was changed (its SHA-256 does not match)"))


class Cancelled(Exception):
    pass


def open_stream(url):
    """Open url for reading (a urllib response). Tests replace this (no network)."""
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": "application/octet-stream"})
    return urllib.request.urlopen(req, timeout=TIMEOUT)


def download(url, path, size=0, progress=None, cancelled=None):
    """Save url to path. progress(done, total) is called now and then (total 0 = unknown); cancelled() -> True
    stops with Cancelled. Raises ValueError for network and disk errors."""
    done, last = 0, 0.0
    try:
        with open_stream(url) as r, open(path, "wb") as f:
            headers = getattr(r, "headers", None)
            total = size or (int(headers.get("Content-Length") or 0) if headers is not None else 0)
            while True:
                if cancelled is not None and cancelled():
                    raise Cancelled()
                block = r.read(CHUNK)
                if not block:
                    break
                f.write(block)
                done += len(block)
                now = time.monotonic()
                if progress is not None and now - last >= 0.1:
                    last = now
                    progress(done, total)
    except Cancelled:
        raise
    except Exception as e:
        raise ValueError(_t("Download failed: {e}").format(e=e)) from None
    if total and done != total:
        raise ValueError(_t("Download failed: {e}").format(e=_t("the file is incomplete")))
    if progress is not None:
        progress(done, done)
    return path


def fetch_verified(release, name, folder, progress=None, cancelled=None):
    """Download release file `name` into folder and check it against SHA256SUMS.txt. Returns its path.
    Raises ValueError (no checksum, mismatch, network) or Cancelled; a bad or partial file is deleted."""
    if SUMS_NAME not in release["assets"]:
        raise ValueError(_t("This release has no {sums}, so its files cannot be verified").format(sums=SUMS_NAME))
    sums = parse_sums(http_get(release["assets"][SUMS_NAME]["url"], MAX_SUMS, accept="application/octet-stream"))
    if name not in sums:        # fail before a long download
        raise ValueError(_t("{name} is not listed in {sums}, so it cannot be verified").format(name=name, sums=SUMS_NAME))
    path = os.path.join(folder, os.path.basename(name))
    try:
        download(release["assets"][name]["url"], path, release["assets"][name].get("size", 0), progress, cancelled)
        verify(path, name, sums)
    except BaseException:
        try:
            os.remove(path)
        except OSError:
            pass
        raise
    return path


def under_program_files(path, env=None):
    """True when path is inside Program Files (an all-users install, which needs administrator rights)."""
    env = os.environ if env is None else env
    p = ntpath.normcase(ntpath.normpath(path)) + "\\"
    for key in ("ProgramW6432", "ProgramFiles", "ProgramFiles(x86)"):
        base = env.get(key)
        if base and p.startswith(ntpath.normcase(ntpath.normpath(base)) + "\\"):
            return True
    return False


def installer_command(setup, app_dir, language="en", env=None):
    """Command line for the Inno Setup installer (tools/ProxyDock.iss): silent with a progress window, into the
    current folder and for the same users (/ALLUSERS makes setup ask for administrator rights itself), closing
    the running app, and starting ProxyDock again when done (/RELAUNCH=1, see the [Run] section of the .iss)."""
    return [setup, "/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/FORCECLOSEAPPLICATIONS",
            "/ALLUSERS" if under_program_files(app_dir, env) else "/CURRENTUSER", f"/DIR={app_dir}",
            f"/LANG={language}", "/RELAUNCH=1"]


def replace_appimage(new_file, target):
    """Put the downloaded AppImage in place of the running one (the running copy keeps working: it is mounted
    from the old file, which stays until this process exits). Raises OSError."""
    tmp = os.path.join(os.path.dirname(target), "." + os.path.basename(target) + ".new")
    shutil.copyfile(new_file, tmp)
    os.chmod(tmp, 0o755)
    os.replace(tmp, target)
