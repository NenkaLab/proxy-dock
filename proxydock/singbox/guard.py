"""Protections the Python engine has built in, done from outside sing-box for all sing-box cards together:
- temporary IP bans after repeated authentication failures (common.note_auth_failure rules), written to a shared
  local rule-set file that every sing-box process rejects (sing-box reloads it on change)
- the per-IP concurrent connection limit (common.MAX_CONN_PER_IP), enforced by closing the newest connections
  over the limit through each process's Clash API (after the fact, unlike the Python engine which refuses them)
"""
import json
import os
import time

from .. import common, config
from ..common import MAX_CONN_PER_IP, is_loopback_ip
from .compile import bans_ruleset

RUN_DIR = os.path.join(config.APP_DIR, "run")
BANS_PATH = os.path.join(RUN_DIR, "bans.json")

_written = None             # ban set last written to BANS_PATH
_conns = {}                 # SingBoxInstance -> {source ip: [(start, conn id), ...]}


def current_bans():
    return frozenset(ip for ip in list(common._banned) if common.is_banned(ip))


def write_bans(force=False):
    """Write the ban list if it changed (also run periodically so expired bans drop out)."""
    global _written
    bans = current_bans()
    if bans == _written and not force:
        return
    os.makedirs(RUN_DIR, exist_ok=True)
    tmp = BANS_PATH + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(bans_ruleset(bans), f)
    os.replace(tmp, BANS_PATH)      # replace in one step: sing-box watches the file
    _written = bans


def auth_failure(ip, inst):
    common.note_auth_failure(ip, inst)
    if common.is_banned(ip):
        write_bans()


def report_connections(inst, conns):
    """conns: Clash API connection list of one process. Returns connection ids to close (over the per-IP limit)."""
    by_ip = {}
    for c in conns:
        ip = c.get("metadata", {}).get("sourceIP", "")
        if ip and not is_loopback_ip(ip):
            by_ip.setdefault(ip, []).append((c.get("start", ""), c["id"]))
    _conns[inst] = by_ip
    close = []
    for ip, mine in by_ip.items():
        total = sum(len(other.get(ip, ())) for other in _conns.values())
        if total > MAX_CONN_PER_IP:
            mine.sort()
            close += [cid for _, cid in mine[-(total - MAX_CONN_PER_IP):]]
    return close


def forget(inst):
    _conns.pop(inst, None)


_last_tick = 0.0


def tick():
    """Called about once a second by the running instances."""
    global _last_tick
    now = time.monotonic()
    if now - _last_tick >= 1:
        _last_tick = now
        write_bans()
