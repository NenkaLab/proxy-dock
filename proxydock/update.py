"""Update check against the GitHub releases of ProxyDock. No Qt here (the UI side is ui/updater.py).

check() asks the GitHub API for the latest release (drafts and prereleases are not returned by /latest, and are
ignored anyway) and returns it when its tag is newer than this version. Only a generic User-Agent
("ProxyDock/<version>") is sent: no settings, IDs or other data.
"""
from .i18n import _t
import json
import re
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
