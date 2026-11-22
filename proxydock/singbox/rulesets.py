"""GeoIP / Geosite rule-sets for router rules (official SagerNet sing-geoip / sing-geosite builds).

ProxyDock downloads them itself into one shared folder instead of letting sing-box fetch them: sing-box refuses to
start when a remote rule-set cannot be downloaded on first use, and its download option is deprecated in 1.14.
Here a file older than a day is refreshed; if that fails the last good copy keeps working (offline start).
"""
from ..i18n import _t
import os
import re
import time
import urllib.request

from .. import config

URLS = {
    "geoip": "https://raw.githubusercontent.com/SagerNet/sing-geoip/rule-set/geoip-{name}.srs",
    "geosite": "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-{name}.srs",
}
MAX_AGE = 24 * 3600
NAME_RE = re.compile(r"^[a-z0-9][a-z0-9!@._-]{0,63}$")


def ruleset_dir():
    return os.path.join(config.APP_DIR, "rulesets")


def tag(kind, name):
    return f"{kind}-{name}"


def valid_name(name):
    return bool(NAME_RE.match(name))


def path_of(kind, name):
    return os.path.join(ruleset_dir(), f"{tag(kind, name)}.srs")


def _download(kind, name, dest):
    url = URLS[kind].format(name=urllib.request.quote(name, safe="!@._-"))
    req = urllib.request.Request(url, headers={"User-Agent": "ProxyDock"})
    with urllib.request.urlopen(req, timeout=30) as r:
        data = r.read()
    if not data.startswith(b"SRS"):            # sing-box binary rule-set magic
        raise ValueError(_t("not a rule-set file"))
    tmp = dest + ".tmp"
    with open(tmp, "wb") as f:
        f.write(data)
    os.replace(tmp, dest)


def ensure(items, log=None):
    """items: iterable of (kind, name). Returns {tag: path}. Raises RuntimeError if one is missing and cannot be
    downloaded. log(message): called when a refresh fails but an old copy is used."""
    os.makedirs(ruleset_dir(), exist_ok=True)
    out = {}
    for kind, name in sorted(set(items)):
        if kind not in URLS or not valid_name(name):
            raise RuntimeError(_t("Invalid {kind} name: {name}").format(kind=kind, name=name))
        p = path_of(kind, name)
        fresh = os.path.exists(p) and time.time() - os.path.getmtime(p) < MAX_AGE
        if not fresh:
            try:
                _download(kind, name, p)
            except Exception as e:
                if not os.path.exists(p):
                    raise RuntimeError(_t("Could not download {kind} '{name}' for a router rule: {e}").format(
                        kind=kind, name=name, e=e)) from None
                if log:
                    log(_t("Could not update {kind} '{name}', using the saved copy: {e}").format(kind=kind, name=name, e=e))
        out[tag(kind, name)] = p
    return out


def needed(proxy):
    """(kind, name) pairs a proxy's router rules use."""
    items = []
    for r in proxy.get("routes") or []:
        for m in r["match"]:
            items += [("geoip", x) for x in m.get("geoip", [])] + [("geosite", x) for x in m.get("geosite", [])]
    return items
