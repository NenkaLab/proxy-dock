"""Running proxies: ProxyInstance (run state of one proxy) and ProxyManager (runs many proxies on an event loop thread)."""
from .i18n import _t
import asyncio
import errno
import json
import threading
from concurrent.futures import ThreadPoolExecutor

from . import config
from .common import (
    INBOUND_TYPES, LOG_ERROR, LOG_INFO, LOG_WARN, OUTBOUND_TYPES, Stats, describe_error, host_port,
    is_loopback_host, log,
)


class ProxyInstance:
    engine = "python"

    def __init__(self, proxy, glob, stats):
        self.stats = stats
        self.conns = set()
        self.tasks = set()
        self.hy2_conns = set()
        self.servers = []
        self._hy2 = {}
        self._ss_keys = {}
        self.last_error = ""
        self.last_reason = ""     # code of the last destination connection failure (auth/denied/net/host/timeout/...)
        self.update(proxy, glob)

    # --- settings ---
    def update(self, proxy, glob):
        """Update the settings. The manager restarts the proxy when the inbound/outbound values change."""
        from .ss import ss_make_key
        self.proxy = proxy
        self.id = proxy["id"]
        self.name = proxy["name"]
        self.inbound = proxy["inbound"]
        self.outbound = proxy["outbound"]
        cfg = config.resolve(proxy, glob)
        doms = [d.lower().rstrip(".") for d in cfg["allowed_domains"]]
        self.allow = ("*" in doms,
                      frozenset(d for d in doms if not d.startswith("*.")),
                      tuple(d[2:] for d in doms if d.startswith("*.")))
        self.cfg = cfg
        ib, ob = self.inbound, self.outbound
        self.ss_in_key = (ib["ss_method"], ss_make_key(ib["ss_method"], ib["ss_password"])) \
            if ib["type"] == "shadowsocks" else None
        self.ss_out_key = (ob["ss_method"], ss_make_key(ob["ss_method"], ob["ss_password"])) \
            if ob["type"] == "shadowsocks" else None

    def allowed(self, host):
        allow_all, exact, suffixes = self.allow
        if allow_all:
            return True
        h = host.lower().rstrip(".")
        if h in exact:
            return True
        return any(h == s or h.endswith("." + s) for s in suffixes)

    @property
    def outbound_label(self):
        parts, ob = [], self.outbound
        while ob:
            if ob["type"] == "group":
                parts.append(f"{OUTBOUND_TYPES['group']} ({', '.join(m['name'] for m in ob.get('members') or [])})")
                break
            parts.append("Direct" if ob["type"] == "direct" else OUTBOUND_TYPES["block"] if ob["type"] == "block"
                         else f"{OUTBOUND_TYPES[ob['type']]} {host_port(ob['host'], ob['port'])}")
            ob = ob.get("via")
        return " ⇢ ".join(parts)

    @property
    def summary(self):
        ib = self.inbound
        from .common import inbound_where
        return f"{INBOUND_TYPES[ib['type']]} {inbound_where(ib)} → {self.outbound_label}"

    def log(self, level, message):
        log(level, message, self)

    def log_conn(self, level, message):
        if self.cfg["log_connections"]:
            log(level, message, self)

    def server_cert_paths(self):
        ib = self.inbound
        if ib["tls_cert"] and ib["tls_key"]:
            return ib["tls_cert"], ib["tls_key"]
        from .tls import ensure_self_signed
        return ensure_self_signed(config.CERT_DIR, f"proxy-{self.id}")

    def server_ssl_context(self):
        from .tls import server_context
        cert, key = self.server_cert_paths()
        return server_context(cert, key)

    def hy2_client(self, ob=None):
        """Hysteria2 client for this outbound (one QUIC connection per server, shared by all streams)."""
        from .hysteria2 import Hy2Client
        ob = ob or self.outbound
        key = json.dumps({k: v for k, v in ob.items() if k != "via"}, sort_keys=True)
        client = self._hy2.get(key)
        if client is None:
            client = self._hy2[key] = Hy2Client(self, ob)
        return client

    def ss_key(self, ob):
        """(method, key) for a Shadowsocks outbound anywhere in the chain (cached)."""
        from .ss import ss_make_key
        k = (ob["ss_method"], ob["ss_password"])
        if k not in self._ss_keys:
            self._ss_keys[k] = (ob["ss_method"], ss_make_key(ob["ss_method"], ob["ss_password"]))
        return self._ss_keys[k]

    # --- start / stop ---
    async def start(self):
        from .inbound import start_servers
        self.servers = await start_servers(self)

    async def stop(self):
        for srv in self.servers:
            try:
                srv.close()
            except Exception:
                pass
        for srv in self.servers:
            if isinstance(srv, asyncio.AbstractServer):
                try:
                    await asyncio.wait_for(srv.wait_closed(), 2)
                except (asyncio.TimeoutError, Exception):
                    pass
        self.servers = []
        for c in list(self.conns):
            try:
                c.transport.abort()
            except Exception:
                pass
        for t in list(self.tasks):
            t.cancel()
        for client in list(self._hy2.values()):
            await client.close()
        self._hy2 = {}


def _net_key(p):
    return (p["inbound"], p["outbound"], p.get("routes"))


def _make_instance(proxy, glob, stats):
    """A native engine for the card when one can run it, otherwise the Python engine (with the reason logged)."""
    from .singbox import engine_for
    engine, reason = engine_for(proxy, glob)
    if engine == "sing-box":
        from .singbox.process import SingBoxInstance
        return SingBoxInstance(proxy, glob, stats)
    if engine == "hysteria":
        from .singbox.hysteria import HysteriaInstance
        return HysteriaInstance(proxy, glob, stats)
    if engine == "none":
        raise RuntimeError(_t("This card uses settings that need the sing-box engine: {reason}").format(reason=reason))
    inst = ProxyInstance(proxy, glob, stats)
    if glob.get("engine") == "sing-box" and reason != "internal":
        inst.log(LOG_INFO, _t("Running on the Python engine: {reason}").format(reason=reason))
    return inst


def _engine_changed(inst, proxy, glob):
    from .singbox import engine_for
    return getattr(inst, "engine", "python") != engine_for(proxy, glob)[0]


class ProxyManager:
    """Runs every proxy on one event loop thread. Methods called from the UI thread are thread-safe."""

    def __init__(self, status_hook=None):
        self.status_hook = status_hook      # (proxy id, state, message) - called on the loop thread
        self.instances = {}                 # running: id -> ProxyInstance
        self.stats = {}                     # id -> Stats (kept across stops)
        self.status = {}                    # id -> (state, message)
        self.loop = asyncio.new_event_loop()
        self.loop.set_default_executor(ThreadPoolExecutor(max_workers=64, thread_name_prefix="dns"))
        self.loop.set_exception_handler(_loop_exception)
        self.thread = threading.Thread(target=self.loop.run_forever, name="proxy-loop", daemon=True)
        self.thread.start()

    def submit(self, coro):
        return asyncio.run_coroutine_threadsafe(coro, self.loop)

    def _set(self, pid, state, msg=""):
        self.status[pid] = (state, msg)
        if self.status_hook:
            self.status_hook(pid, state, msg)

    def state(self, pid):
        return self.status.get(pid, ("stopped", ""))

    def stats_for(self, pid):
        return self.stats.setdefault(pid, Stats())

    # --- called from the UI thread ---
    def start(self, proxy, glob):
        return self.submit(self._start(proxy, glob))

    def stop(self, pid):
        return self.submit(self._stop(pid))

    def stop_all(self, timeout=3):
        try:
            self.submit(self._stop_all()).result(timeout)
        except Exception:
            pass

    def apply(self, proxy, glob):
        """Proxy settings changed: a running proxy restarts for inbound/outbound changes, other changes apply immediately."""
        return self.submit(self._apply(proxy, glob))

    def apply_global(self, glob, proxies):
        """Global defaults changed: apply right away to running proxies that use the global value."""
        by_id = {p["id"]: p for p in proxies}

        def run():
            for pid, inst in list(self.instances.items()):
                if pid not in by_id:
                    continue
                p = by_id[pid]
                if _engine_changed(inst, p, glob) or getattr(inst, "needs_restart", lambda *a: False)(p, glob):
                    self.loop.create_task(self._start(p, glob))
                else:
                    inst.update(p, glob)
        self.loop.call_soon_threadsafe(run)

    # --- loop thread ---
    async def _start(self, proxy, glob):
        pid = proxy["id"]
        if pid in self.instances:
            await self._stop(pid, quiet=True)
        self._set(pid, "starting")
        inst = None
        try:
            inst = _make_instance(proxy, glob, self.stats_for(pid))
            await inst.start()
        except Exception as e:
            msg = describe_error(e) if isinstance(e, OSError) else str(e) or type(e).__name__
            code = getattr(e, "winerror", None) or getattr(e, "errno", None)
            port = proxy["inbound"]["port"]
            if code in (10048, errno.EADDRINUSE, 98, 48):
                msg = _t("Port {port} is already in use by another program").format(port=port)
            elif code in (10013, errno.EACCES):
                msg = _t("Port {port} cannot be used (reserved by Windows, or no permission)").format(port=port)
            elif code in (10049, errno.EADDRNOTAVAIL):
                msg = _t("This PC does not have the address {host}").format(host=proxy['inbound']['host'])
            self._set(pid, "error", msg)
            log(LOG_ERROR, _t("[{name}] failed to start: {msg}").format(name=proxy['name'], msg=msg))
            if inst is not None:
                try:
                    await inst.stop()
                except Exception:
                    pass
            return False
        self.instances[pid] = inst
        self._set(pid, "running")
        inst.log(LOG_INFO, _t("Started: {summary}").format(summary=inst.summary))
        ib = inst.inbound
        if ib["type"] != "tun" and not is_loopback_host(ib["host"]):
            if ib["type"] == "socks4":
                inst.log(LOG_WARN, _t("SOCKS4 has no password authentication; anyone can use it if it is exposed"))
            elif ib["type"] in ("socks5", "http", "https") and not ib["auth_enabled"]:
                inst.log(LOG_WARN, _t("Listening on {host_port} without authentication (open proxy)").format(host_port=host_port(ib['host'], ib['port'])))
        return True

    async def _stop(self, pid, quiet=False):
        inst = self.instances.pop(pid, None)
        if inst is not None:
            await inst.stop()
            if not quiet:
                inst.log(LOG_INFO, _t("Stopped"))
        if not quiet:
            self._set(pid, "stopped")

    async def _stop_all(self):
        for pid in list(self.instances):
            await self._stop(pid)

    async def _apply(self, proxy, glob):
        inst = self.instances.get(proxy["id"])
        if inst is None:
            return
        if _net_key(inst.proxy) != _net_key(proxy) or _engine_changed(inst, proxy, glob)                 or getattr(inst, "needs_restart", lambda *a: False)(proxy, glob):
            await self._start(proxy, glob)
        else:
            inst.update(proxy, glob)


def _loop_exception(loop, context):
    # ignore the OSError from shutdown() when Proactor closes a socket that is already gone/half-closed
    exc = context.get("exception")
    if isinstance(exc, OSError) and "_call_connection_lost" in str(context.get("handle", "")):
        return
    if isinstance(exc, (ConnectionResetError, ConnectionAbortedError)):
        return
    loop.default_exception_handler(context)
