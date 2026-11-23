"""SingBoxInstance: one proxy card run by its own sing-box process. Same interface as runtime.ProxyInstance
(start / stop / update / stats / log / summary / last_error) so ProxyManager can run either."""
from ..i18n import _t
import asyncio
import errno
import json
import os
import re
import secrets
import socket
import sys

from .. import config
from ..common import (
    IS_WIN, LOG_AUTH, LOG_BLOCKED, LOG_ERROR, LOG_FAIL, LOG_INFO, LOG_TCP, LOG_UDP, LOG_WARN, fmt_size, host_port, log,
    split_host_port,
)
import time
from . import compile as sbc, guard
from . import find_binary

START_TIMEOUT = 10
POLL_SECONDS = 1

# "LEVEL[0001] [1234567 5ms] message" (the bracket with the connection id is only on connection lines)
_LINE = re.compile(r"^([A-Z]+)\[\d+\]\s+(?:\[(\d+)\s[^\]]*\]\s+)?(.*)$")
_FROM = re.compile(r"^inbound/[\w-]+\[[^\]]*\]: (?:inbound )?(?:packet )?connection from (\S+)$")
_TO = re.compile(r"^inbound/[\w-]+\[[^\]]*\]: (?:\[[^\]]*\] )?inbound (packet )?connection to (\S+)$")
_PROC_ERR = re.compile(r"^inbound/[\w-]+\[[^\]]*\]: process connection from (\S+): (.*)$")
_AUTH_USER = re.compile(r"authentication failed, username=(.*?), password=")
_OPEN_FAIL = re.compile(r"^connection: open (?:packet )?connection to (\S+) using outbound/[\w-]+\[[^\]]*\]: (.*)$")
_REJECT = re.compile(r"^router: match\[(\d+)\].*=> reject$")
_ANSI = re.compile(r"\x1b\[[0-9;]*m")
_SECRET = re.compile(r"(password|secret)=\S+")
_BENIGN = ("use of closed network connection", "wsasend", "wsarecv", "EOF", "connection reset", "broken pipe",
           "forcibly closed", "aborted", "i/o timeout", "context canceled", "no Proxy-Authorization header")


def _ip_of(addr):
    try:
        return split_host_port(addr, 0)[0]
    except ValueError:
        return addr


# --- Windows: a job object so sing-box processes end when ProxyDock ends (even if it crashes) ---
_job = None


def _attach_to_job(pid):
    global _job
    if not IS_WIN:
        return
    import ctypes
    from ctypes import wintypes
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.CreateJobObjectW.restype = wintypes.HANDLE
    k32.OpenProcess.restype = wintypes.HANDLE
    if _job is None:
        class IO_COUNTERS(ctypes.Structure):
            _fields_ = [(n, ctypes.c_ulonglong) for n in ("r", "w", "o", "rt", "wt", "ot")]

        class BASIC(ctypes.Structure):
            _fields_ = [("PerProcessUserTimeLimit", ctypes.c_longlong), ("PerJobUserTimeLimit", ctypes.c_longlong),
                        ("LimitFlags", wintypes.DWORD), ("MinimumWorkingSetSize", ctypes.c_size_t),
                        ("MaximumWorkingSetSize", ctypes.c_size_t), ("ActiveProcessLimit", wintypes.DWORD),
                        ("Affinity", ctypes.c_size_t), ("PriorityClass", wintypes.DWORD),
                        ("SchedulingClass", wintypes.DWORD)]

        class EXTENDED(ctypes.Structure):
            _fields_ = [("BasicLimitInformation", BASIC), ("IoInfo", IO_COUNTERS),
                        ("ProcessMemoryLimit", ctypes.c_size_t), ("JobMemoryLimit", ctypes.c_size_t),
                        ("PeakProcessMemoryUsed", ctypes.c_size_t), ("PeakJobMemoryUsed", ctypes.c_size_t)]
        job = k32.CreateJobObjectW(None, None)
        info = EXTENDED()
        info.BasicLimitInformation.LimitFlags = 0x2000      # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        k32.SetInformationJobObject(wintypes.HANDLE(job), 9, ctypes.byref(info), ctypes.sizeof(info))
        _job = job
    h = k32.OpenProcess(0x0101, False, pid)                  # PROCESS_SET_QUOTA | PROCESS_TERMINATE
    if h:
        k32.AssignProcessToJobObject(wintypes.HANDLE(_job), wintypes.HANDLE(h))
        k32.CloseHandle(wintypes.HANDLE(h))


def _linux_parent_death():
    import ctypes
    import signal
    try:
        ctypes.CDLL("libc.so.6").prctl(1, signal.SIGTERM)    # PR_SET_PDEATHSIG
    except OSError:
        pass


def _free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


class StartError(OSError):
    pass


def _start_error(text, proxy):
    t = text.lower()
    if "only one usage of each socket address" in t or "address already in use" in t:
        return StartError(errno.EADDRINUSE, text)
    if "forbidden by its access permissions" in t or "permission denied" in t:
        return StartError(errno.EACCES, text)
    if "requested address is not valid" in t or "cannot assign requested address" in t:
        return StartError(errno.EADDRNOTAVAIL, text)
    return RuntimeError("sing-box: " + _SECRET.sub(r"\1=***", text))


FORCE_HELPER = False        # tests: run through the helper protocol without UAC


class SingBoxInstance:
    engine = "sing-box"
    tun_owner = None            # id of the running TUN card (one TUN at a time)

    def __init__(self, proxy, glob, stats):
        self.stats = stats
        self.last_error = ""
        self.last_reason = ""
        self.servers = []           # (no Python sockets; kept for ProxyInstance compatibility)
        self.proc = None
        self.tasks = set()
        self.reasons = []
        self.rulesets = {}
        self.glob = glob
        self._clients = {}          # sing-box connection id -> client "ip:port"
        self._targets = {}          # sing-box connection id -> destination "host:port"
        self._last_totals = (0, 0)
        self._live = {}             # Clash API connection id -> what we know (close details, idle timeout)
        self.update(proxy, glob)

    # --- settings ---
    def update(self, proxy, glob):
        self.proxy = proxy
        self.id = proxy["id"]
        self.name = proxy["name"]
        self.inbound = proxy["inbound"]
        self.outbound = proxy["outbound"]
        self.glob = glob
        self.cfg = config.resolve(proxy, glob)

    def needs_restart(self, proxy, glob):
        """Route settings live in the sing-box config, so changing them restarts the process (only this card)."""
        new = config.resolve(proxy, glob)
        keys = ("allowed_domains", "block_private", "timeout", "dns_server", "dns_via_outbound")
        return any(new[k] != self.cfg[k] for k in keys) or proxy.get("routes") != self.proxy.get("routes")

    @property
    def outbound_label(self):
        from ..runtime import ProxyInstance
        return ProxyInstance.outbound_label.fget(self)

    @property
    def summary(self):
        from ..runtime import ProxyInstance
        return ProxyInstance.summary.fget(self) + " · sing-box"

    def log(self, level, message):
        log(level, message, self)

    def log_conn(self, level, message):
        if self.cfg["log_connections"]:
            log(level, message, self)

    def server_cert_paths(self):
        from ..runtime import ProxyInstance
        return ProxyInstance.server_cert_paths(self)

    # --- start / stop ---
    @property
    def config_path(self):
        return os.path.join(guard.RUN_DIR, f"{self.id}.json")

    def _write_config(self):
        self.api_port = _free_port()
        self.api_secret = secrets.token_hex(16)
        cert = self.server_cert_paths() if self.inbound["type"] in ("https", "hysteria2") else None
        exclude = None
        if self.inbound["type"] == "tun":
            from . import find_hysteria
            exclude = sorted({os.path.abspath(x) for x in (find_binary(self.glob), find_hysteria(self.glob), sys.executable) if x})
        conf, self.reasons = sbc.build(self.proxy, self.cfg, bans_path=guard.BANS_PATH, api_port=self.api_port,
                                       api_secret=self.api_secret, cert=cert, rulesets=self.rulesets,
                                       exclude_paths=exclude)
        os.makedirs(guard.RUN_DIR, exist_ok=True)
        path = self.config_path
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)     # contains passwords
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(conf, f, indent=1)

    async def start(self):
        exe = find_binary(self.glob)
        if exe is None:
            raise RuntimeError(_t("sing-box was not found"))
        guard.write_bans(force=True)
        from . import rulesets
        need = rulesets.needed(self.proxy)
        self.rulesets = {}
        if need:            # GeoIP / Geosite files (downloaded or refreshed off the event loop)
            self.rulesets = await asyncio.get_running_loop().run_in_executor(
                None, lambda: rulesets.ensure(need, lambda m: self.log(LOG_WARN, m)))
        tun = self.inbound["type"] == "tun"
        if tun:
            if SingBoxInstance.tun_owner not in (None, self.id):
                raise RuntimeError(_t("Another TUN card is already running; only one can run at a time"))
            SingBoxInstance.tun_owner = self.id
        self._write_config()
        from ..common import is_admin
        if (tun and not is_admin()) or FORCE_HELPER:
            # TUN needs administrator rights: run this card's sing-box through the UAC helper
            from .helper import HelperProc
            self.log(LOG_INFO, _t("Asking for administrator permission to start the TUN…"))
            self.proc = await HelperProc.launch(self.config_path, guard.RUN_DIR, elevate=not FORCE_HELPER)
        else:
            kw = {}
            if IS_WIN:
                kw["creationflags"] = 0x08000000        # CREATE_NO_WINDOW
            elif sys.platform.startswith("linux"):
                kw["preexec_fn"] = _linux_parent_death
            self.proc = await asyncio.create_subprocess_exec(
                exe, "run", "--disable-color", "-c", self.config_path, "-D", guard.RUN_DIR,
                stdin=asyncio.subprocess.DEVNULL, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT, **kw)
            _attach_to_job(self.proc.pid)
        started = asyncio.get_running_loop().create_future()
        self._spawn(self._read_output(started))
        try:
            await asyncio.wait_for(started, START_TIMEOUT)
        except asyncio.TimeoutError:
            raise RuntimeError(_t("sing-box did not start in time")) from None
        self._spawn(self._poll())

    def _spawn(self, coro):
        t = asyncio.get_running_loop().create_task(coro)
        self.tasks.add(t)
        t.add_done_callback(self.tasks.discard)

    async def stop(self):
        for t in list(self.tasks):
            t.cancel()
        guard.forget(self)
        if SingBoxInstance.tun_owner == self.id:
            SingBoxInstance.tun_owner = None
        p, self.proc = self.proc, None
        if p is not None and p.returncode is None:
            try:
                p.terminate()
                await asyncio.wait_for(p.wait(), 3)
            except (asyncio.TimeoutError, ProcessLookupError):
                try:
                    p.kill()
                except ProcessLookupError:
                    pass
            except Exception:
                pass
        try:
            os.remove(self.config_path)
        except OSError:
            pass

    # --- sing-box output ---
    async def _read_output(self, started):
        errors = []
        while True:
            raw = await self.proc.stdout.readline()
            if not raw:
                break
            line = _ANSI.sub("", raw.decode("utf-8", errors="replace")).rstrip()
            m = _LINE.match(line)
            if not m:
                continue
            level, cid, msg = m.groups()
            if not started.done():
                if msg.startswith("sing-box started"):
                    started.set_result(True)
                    continue
                if level in ("FATAL", "PANIC", "ERROR"):
                    errors.append(msg)
            try:
                self._handle(level, cid, msg)
            except Exception as e:      # never let a log format surprise stop the reader
                self.log(LOG_ERROR, f"sing-box log: {e}")
        await self.proc.wait() if self.proc else None
        if not started.done():
            text = errors[-1] if errors else _t("sing-box exited (code {code})").format(code=self.proc.returncode if self.proc else "?")
            text = re.sub(r"^start service: ", "", text)
            started.set_exception(_start_error(text, self.proxy))
        elif self.proc is not None:     # exited while running (not via stop())
            self.log(LOG_ERROR, _t("sing-box exited (code {code})").format(code=self.proc.returncode))

    def _handle(self, level, cid, msg):
        if cid is None:
            if level in ("WARN", "ERROR", "FATAL", "PANIC"):
                self.log(LOG_WARN if level == "WARN" else LOG_ERROR, "sing-box: " + _SECRET.sub(r"\1=***", msg))
            return
        m = _FROM.match(msg)
        if m:
            self._clients[cid] = m.group(1)
            if len(self._clients) > 20000:
                self._clients.clear()
                self._targets.clear()
            return
        who = self._clients.get(cid, "?")
        m = _TO.match(msg)
        if m:
            udp, target = m.groups()
            self._targets[cid] = target
            self.stats.total += 1
            if udp:
                self.log_conn(LOG_UDP, f"{who} -> {target} UDP")
            else:
                self.log_conn(LOG_TCP, f"{who} -> {target}")
            return
        target = self._targets.get(cid, "?")
        m = _REJECT.match(msg)
        if m:
            idx = int(m.group(1))
            reason = self.reasons[idx] if idx < len(self.reasons) else None
            self.stats.blocked += 1
            self.last_reason = "denied"
            if reason == sbc.R_ALLOW:
                self.log(LOG_BLOCKED, _t("{who} -> {target} not in the allow list").format(who=who, target=target))
            elif reason == sbc.R_PRIVATE:
                self.log(LOG_BLOCKED, f"{who} -> {target} " + _t("private address {host}").format(host=target))
            elif reason == sbc.R_BANNED:
                self.log(LOG_BLOCKED, f"{who} -> {target} " + _t("blocked (temporarily banned IP)"))
            elif reason == sbc.R_SELF:
                self.log(LOG_BLOCKED, f"{who} -> {target} " + _t("blocked (this proxy's own port)"))
            elif isinstance(reason, tuple) and reason[0] == sbc.R_RULE:
                name = _t("default") if reason[1] == "default" else reason[1]
                self.log(LOG_BLOCKED, f"{who} -> {target} " + _t("blocked by router rule '{name}'").format(name=name))
            return
        m = _PROC_ERR.match(msg)
        if m:
            client, err = m.groups()
            if "authentication failed" in err and "no Proxy-Authorization header" not in err:
                um = _AUTH_USER.search(err)
                self.stats.blocked += 1
                if um:
                    self.log(LOG_AUTH, _t("{client} authentication failed (user {user!r})").format(client=client, user=um.group(1)))
                else:
                    self.log(LOG_AUTH, f"{client} " + _t("authentication failed (wrong key or not Shadowsocks)"))
                guard.auth_failure(_ip_of(client), self)
            elif not any(b in err for b in _BENIGN) and self.cfg["log_connections"]:
                self.log(LOG_FAIL, f"{client} " + _SECRET.sub(r"\1=***", err))
            return
        m = _OPEN_FAIL.match(msg)
        if m:
            dest, err = m.groups()
            self.stats.failed += 1
            self.last_reason = "auth" if "authentication failed" in err else "net"
            hint = ""
            if "no such network interface" in err:
                hint = " - " + _t("the chosen network interface does not exist (renamed or removed?)")
            elif "unreachable network" in err or "network is unreachable" in err:
                hint = " - " + _t("the chosen network interface may be disconnected")
            self.log(LOG_FAIL, f"{who} -> {dest} " + _SECRET.sub(r"\1=***", err) + hint)
            return

    # --- close details and TCP idle timeout (sing-box only has a UDP timeout) ---
    def _track(self, conns):
        """Follow the open connections between polls. Returns the TCP connections idle for longer than the timeout
        (to close). Logs a close line for connections that are gone (amounts as of the last poll)."""
        now = time.monotonic()
        timeout = self.cfg["timeout"]
        seen, idle = set(), []
        for c in conns:
            cid = c.get("id")
            if not cid:
                continue
            seen.add(cid)
            up, down = c.get("upload", 0), c.get("download", 0)
            e = self._live.get(cid)
            if e is None:
                md = c.get("metadata") or {}
                try:
                    port = int(md.get("destinationPort") or 0)
                except ValueError:
                    port = 0
                e = self._live[cid] = {
                    "who": host_port(md.get("sourceIP", "?"), md.get("sourcePort", "?")),
                    "target": host_port(md.get("host") or md.get("destinationIP") or "?", port),
                    "udp": md.get("network") == "udp", "up": up, "down": down, "start": _started(c.get("start")),
                    "change": now, "reason": None}
            elif (up, down) != (e["up"], e["down"]):
                e.update(up=up, down=down, change=now)
            if not e["udp"] and e["reason"] is None and now - e["change"] >= timeout:
                e["reason"] = _t("idle timeout")
                idle.append(cid)
        for cid in [k for k in self._live if k not in seen]:
            self._closed(self._live.pop(cid))
        return idle

    def _closed(self, e):
        secs = max(time.time() - e["start"], 0.0)
        if e["udp"]:
            if self.cfg["log_close"] or self.cfg["log_connections"]:
                self.log(LOG_UDP, _t("{who} UDP session ended · ↑ {fmt_size} ↓ {fmt_size2} · {value:.1f}s{extra}").format(
                    who=e["who"], fmt_size=fmt_size(e["up"]), fmt_size2=fmt_size(e["down"]), value=secs, extra=""))
        elif self.cfg["log_close"]:
            self.log(LOG_TCP, _t("{who} -> {target} closed · {reason} · ↑ {fmt_size} ↓ {fmt_size2} · {value:.1f}s").format(
                who=e["who"], target=e["target"], reason=e["reason"] or _t("normal close"), fmt_size=fmt_size(e["up"]),
                fmt_size2=fmt_size(e["down"]), value=secs))

    # --- statistics / per-IP limit through the Clash API ---
    async def _api(self, method, path):
        r, w = await asyncio.wait_for(asyncio.open_connection("127.0.0.1", self.api_port), 3)
        try:
            w.write(f"{method} {path} HTTP/1.0\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {self.api_secret}\r\n\r\n".encode())
            data = await asyncio.wait_for(r.read(), 5)
        finally:
            w.close()
        head, _, body = data.partition(b"\r\n\r\n")
        return json.loads(body) if body.strip() else None

    async def _poll(self):
        while True:
            await asyncio.sleep(POLL_SECONDS)
            guard.tick()
            try:
                d = await self._api("GET", "/connections")
            except (OSError, asyncio.TimeoutError, ValueError):
                continue
            conns = d.get("connections") or []
            st = self.stats
            st.tcp_active = sum(1 for c in conns if c.get("metadata", {}).get("network") == "tcp")
            st.udp_active = len(conns) - st.tcp_active
            up, down = d.get("uploadTotal", 0), d.get("downloadTotal", 0)
            lu, ld = self._last_totals
            st.up += max(up - lu, 0)
            st.down += max(down - ld, 0)
            self._last_totals = (up, down)
            over = guard.report_connections(self, conns)
            if over:
                st.blocked += len(over)
                self.log(LOG_BLOCKED, _t("Closed {count} connections over the per-IP limit ({limit})").format(
                    count=len(over), limit=guard.MAX_CONN_PER_IP))
            idle = self._track(conns)
            for cid in list(over) + idle:
                try:
                    await self._api("DELETE", f"/connections/{cid}")
                except (OSError, asyncio.TimeoutError, ValueError):
                    pass


_ISO = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)(?:\.(\d+))?(Z|[+-]\d\d:\d\d)?$")


def _started(text):
    """Clash API 'start' (RFC 3339 with nanoseconds) -> epoch seconds (now if it cannot be read)."""
    from datetime import datetime
    m = _ISO.match(text or "")
    if not m:
        return time.time()
    base, frac, tz = m.groups()
    tz = "+00:00" if tz in (None, "Z") else tz
    try:
        return datetime.fromisoformat(f"{base}.{(frac or '0')[:6].ljust(6, '0')}{tz}").timestamp()
    except ValueError:
        return time.time()
