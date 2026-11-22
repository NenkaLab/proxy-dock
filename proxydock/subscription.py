"""Subscriptions: a URL that returns a provider's server list (v2rayN style: share links, one per line, usually
base64-encoded as a whole). fetch() downloads it, parse() turns it into [{"name", "outbound"}].

The servers become members of one subscription block (sing-box selector = pick one, urltest = fastest
automatically, which also fails over when a server dies).
"""
from .i18n import _t
import base64
import binascii
import json
import time
import urllib.request

from .links import parse as parse_link

MAX_BYTES = 5 * 1024 * 1024
TIMEOUT = 20
USER_AGENT = "ProxyDock"


def fetch(url):
    """Download a subscription. Raises ValueError with a readable message."""
    if not url.lower().startswith(("https://", "http://")):
        raise ValueError(_t("The subscription address must start with https:// or http://"))
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    try:
        with urllib.request.urlopen(req, timeout=TIMEOUT) as r:
            data = r.read(MAX_BYTES + 1)
    except Exception as e:
        raise ValueError(_t("Could not download the subscription: {e}").format(e=e)) from None
    if len(data) > MAX_BYTES:
        raise ValueError(_t("The subscription is too large"))
    return data


def _text(data):
    text = data.decode("utf-8", errors="replace").strip().lstrip("﻿")
    if "://" in text:
        return text
    try:                                            # the whole list as base64 (standard or URL-safe)
        compact = "".join(text.split())
        raw = base64.b64decode(compact + "=" * (-len(compact) % 4), altchars=b"-_" if "-" in compact or "_" in compact else None)
        decoded = raw.decode("utf-8", errors="replace")
        if "://" in decoded:
            return decoded
    except (binascii.Error, ValueError):
        pass
    raise ValueError(_t("No server links found (only share-link lists are supported, not Clash YAML)"))


def parse(data):
    """-> (servers, skipped): servers = [{"name", "outbound"}], skipped = number of lines that were not usable."""
    from . import config
    servers, skipped, names = [], 0, set()
    for line in _text(data).splitlines():
        line = line.strip()
        if not line or "://" not in line:
            continue
        try:
            fields, name = parse_link(line)
        except ValueError:
            skipped += 1
            continue
        ob = dict(config.OUTBOUND_DEFAULTS, **fields)
        if config.outbound_errors(dict(ob, via=None)):
            skipped += 1
            continue
        base, i = name[:60] or ob["type"], 2
        while name in names:
            name = f"{base} ({i})"
            i += 1
        names.add(name)
        servers.append({"name": name, "outbound": ob})
    if not servers:
        raise ValueError(_t("No usable servers in the subscription"))
    return servers, skipped


def update(ob):
    """Fetch and parse the subscription of outbound ob. Returns a dict of fields to put into ob, and a note."""
    servers, skipped = parse(fetch(ob["sub_url"].strip()))
    note = _t("{count} servers").format(count=len(servers))
    if skipped:
        note += " · " + _t("{count} skipped (unsupported or invalid)").format(count=skipped)
    return {"sub_servers": json.dumps(servers, ensure_ascii=False), "sub_updated": int(time.time())}, note


def servers_of(ob):
    try:
        v = json.loads(ob.get("sub_servers") or "[]")
        return [s for s in v if isinstance(s, dict) and isinstance(s.get("outbound"), dict)]
    except ValueError:
        return []
