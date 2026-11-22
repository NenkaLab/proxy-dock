"""HysteriaInstance: a Hysteria2 server card open to other PCs, run by the official Hysteria2 core.

sing-box's Hysteria2 server does not report wrong passwords, so IP bans would not work there. The official core
asks an HTTP auth server for every connection with the client address ("auth: type: http"), so ProxyDock answers
those requests itself and keeps the Python engine's rules: 10 failures in 60 s ban the IP for 5 min
(common.note_auth_failure / is_banned, shared with every other card) and at most MAX_QUIC_PER_IP connections per IP.

Data path: client -QUIC-> hysteria server -SOCKS5 (127.0.0.1, random credentials)-> this card's own sing-box
process -> the card's outbound chain. sing-box applies the allow list, private network blocking and chains (UDP
included), and its Clash API gives the statistics, so only authentication lives in hysteria + Python.
"""
from ..i18n import _t
import asyncio
import copy
import hmac
import json
import os
import secrets
import sys

from .. import config
from ..common import (
    IS_WIN, LOG_AUTH, LOG_ERROR, LOG_TCP, LOG_UDP, LOG_WARN, host_port, is_loopback_ip, log,
    split_host_port,
)
from ..hysteria2 import MAX_QUIC_PER_IP
from . import guard
from .process import START_TIMEOUT, SingBoxInstance, _attach_to_job, _linux_parent_death, _start_error

RESTART_KEYS = ("allowed_domains", "block_private", "timeout", "log_connections")


def _ip(addr):
    try:
        return split_host_port(addr, 0)[0]
    except ValueError:
        return addr


class HysteriaInstance:
    engine = "hysteria"

    def __init__(self, proxy, glob, stats):
        self.stats = stats
        self.last_error = ""
        self.last_reason = ""
        self.servers = []
        self.proc = None
        self.inner = None
        self.auth_server = None
        self.tasks = set()
        self.active = {}            # client ip -> connected QUIC connections (from hysteria's log)
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
        if self.inner is not None:
            self.inner.update(self._inner_proxy(), glob)

    def needs_restart(self, proxy, glob):
        new = config.resolve(proxy, glob)
        return any(new[k] != self.cfg[k] for k in RESTART_KEYS)

    @property
    def outbound_label(self):
        from ..runtime import ProxyInstance
        return ProxyInstance.outbound_label.fget(self)

    @property
    def summary(self):
        from ..runtime import ProxyInstance
        return ProxyInstance.summary.fget(self) + " · Hysteria2 + sing-box"

    def log(self, level, message):
        log(level, message, self)

    def log_conn(self, level, message):
        if self.cfg["log_connections"]:
            log(level, message, self)

    def server_cert_paths(self):
        from ..runtime import ProxyInstance
        return ProxyInstance.server_cert_paths(self)

    # --- inner sing-box (the card's outbound chain) ---
    def _inner_proxy(self):
        p = copy.deepcopy(self.proxy)
        p["inbound"] = dict(config.INBOUND_DEFAULTS, type="socks5", host="127.0.0.1", port=self.inner_port,
                            auth_enabled=True, username=self.inner_user, password=self.inner_pass)
        # connections are logged from hysteria's own log (it knows the real client address)
        p["overrides"] = dict(p["overrides"], log_connections=False, log_close=False)
        return p

    # --- auth server (hysteria asks it about every new connection) ---
    async def _auth_client(self, reader, writer):
        try:
            head = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 5)
            length = 0
            for line in head.split(b"\r\n"):
                k, _, v = line.partition(b":")
                if k.strip().lower() == b"content-length":
                    length = int(v.strip() or 0)
            body = json.loads(await asyncio.wait_for(reader.readexactly(length), 5) or b"{}") if length else {}
            ok = self.check_auth(str(body.get("addr", "")), str(body.get("auth", "")))
            out = json.dumps({"ok": ok, "id": "user" if ok else ""}).encode()
            writer.write(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n"
                         + f"Content-Length: {len(out)}\r\n\r\n".encode() + out)
            await writer.drain()
        except (asyncio.IncompleteReadError, asyncio.TimeoutError, asyncio.LimitOverrunError, ValueError, OSError):
            pass
        finally:
            writer.close()

    def check_auth(self, addr, password):
        """Decide one connection attempt (also used by the tests). addr = "ip:port" of the client."""
        ip = _ip(addr)
        if guard.common.is_banned(ip):
            self.stats.blocked += 1
            return False
        if not is_loopback_ip(ip) and self.active.get(ip, 0) >= MAX_QUIC_PER_IP:
            self.stats.blocked += 1      # same as the Python engine: refused without a log line
            return False
        if hmac.compare_digest(password.encode(), self.inbound["hy2_password"].encode()):
            return True
        self.stats.blocked += 1
        self.log(LOG_AUTH, _t("H2 {client} authentication failed").format(client=addr))
        guard.auth_failure(ip, self)
        return False

    # --- start / stop ---
    @property
    def config_path(self):
        return os.path.join(guard.RUN_DIR, f"{self.id}-hysteria.yaml")

    def _write_config(self, auth_port):
        ib = self.inbound
        host = ib["host"] if ib["host"].lower() != "localhost" else "127.0.0.1"
        cert, key = self.server_cert_paths()
        conf = {
            "listen": host_port(host, ib["port"]),
            "tls": {"cert": cert, "key": key},
            "auth": {"type": "http", "http": {"url": f"http://127.0.0.1:{auth_port}/auth"}},
            "outbounds": [{"name": "singbox", "type": "socks5", "socks5": {
                "addr": f"127.0.0.1:{self.inner_port}", "username": self.inner_user, "password": self.inner_pass}}],
            "udpIdleTimeout": f"{self.cfg['timeout']}s",
        }
        os.makedirs(guard.RUN_DIR, exist_ok=True)
        fd = os.open(self.config_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as f:
            json.dump(conf, f, indent=1)        # JSON is valid YAML

    async def start(self):
        from . import find_hysteria
        from .process import _free_port
        exe = find_hysteria(self.glob)
        if exe is None:
            raise RuntimeError(_t("The Hysteria2 core was not found"))
        self.inner_port = _free_port()
        self.inner_user, self.inner_pass = secrets.token_hex(8), secrets.token_hex(16)
        self.inner = SingBoxInstance(self._inner_proxy(), self.glob, self.stats)
        await self.inner.start()
        self.auth_server = await asyncio.start_server(self._auth_client, "127.0.0.1", 0)
        self._write_config(self.auth_server.sockets[0].getsockname()[1])
        env = dict(os.environ, HYSTERIA_LOG_LEVEL="debug" if self.cfg["log_connections"] else "info",
                   HYSTERIA_LOG_FORMAT="json")
        kw = {"creationflags": 0x08000000} if IS_WIN else {}
        if sys.platform.startswith("linux"):
            kw["preexec_fn"] = _linux_parent_death
        self.proc = await asyncio.create_subprocess_exec(
            exe, "server", "-c", self.config_path, "--disable-update-check",
            stdin=asyncio.subprocess.DEVNULL, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.STDOUT,
            env=env, cwd=guard.RUN_DIR, **kw)
        _attach_to_job(self.proc.pid)
        started = asyncio.get_running_loop().create_future()
        self._spawn(self._read_output(started))
        try:
            await asyncio.wait_for(started, START_TIMEOUT)
        except asyncio.TimeoutError:
            raise RuntimeError(_t("The Hysteria2 core did not start in time")) from None

    def _spawn(self, coro):
        t = asyncio.get_running_loop().create_task(coro)
        self.tasks.add(t)
        t.add_done_callback(self.tasks.discard)

    async def stop(self):
        for t in list(self.tasks):
            t.cancel()
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
        if self.auth_server is not None:
            self.auth_server.close()
            self.auth_server = None
        if self.inner is not None:
            await self.inner.stop()
            self.inner = None
        try:
            os.remove(self.config_path)
        except OSError:
            pass

    # --- hysteria output (zap JSON lines) ---
    async def _read_output(self, started):
        errors = []
        while True:
            raw = await self.proc.stdout.readline()
            if not raw:
                break
            line = raw.decode("utf-8", errors="replace").strip()
            try:
                e = json.loads(line)
            except ValueError:
                if line:
                    errors.append(line)
                continue
            if not isinstance(e, dict):
                continue
            msg, level = e.get("msg", ""), e.get("level", "")
            if not started.done():
                if msg == "server up and running":
                    started.set_result(True)
                    continue
                if level in ("fatal", "error", "panic"):
                    errors.append(f"{msg}: {e.get('error', '')}".rstrip(": "))
            try:
                self._handle(e)
            except Exception as ex:
                self.log(LOG_ERROR, f"hysteria log: {ex}")
        if self.proc is not None:
            await self.proc.wait()
        if not started.done():
            code = self.proc.returncode if self.proc else "?"
            text = errors[-1] if errors else _t("The Hysteria2 core exited (code {code})").format(code=code)
            started.set_exception(_start_error(text, self.proxy))
        elif self.proc is not None:
            self.log(LOG_ERROR, _t("The Hysteria2 core exited (code {code})").format(code=self.proc.returncode))

    def _handle(self, e):
        msg, addr = e.get("msg", ""), e.get("addr", "")
        if msg == "client connected":
            ip = _ip(addr)
            self.active[ip] = self.active.get(ip, 0) + 1
            self.log_conn(LOG_TCP, _t("H2 {client} connected (Hysteria2 authenticated)").format(client=addr))
        elif msg == "client disconnected":
            ip = _ip(addr)
            n = self.active.get(ip, 1) - 1
            if n > 0:
                self.active[ip] = n
            else:
                self.active.pop(ip, None)
        elif msg == "TCP request":
            self.log_conn(LOG_TCP, f"H2 {addr} -> {e.get('reqAddr', '')}")
        elif msg == "UDP request":
            self.log_conn(LOG_UDP, f"H2 {addr} -> {e.get('reqAddr', '')} UDP")
        elif msg in ("TCP error", "UDP error"):
            if self.cfg["log_connections"]:
                self.log(LOG_WARN, f"H2 {addr} -> {e.get('reqAddr', '')} {e.get('error', '')}".rstrip())
        elif e.get("level") in ("warn", "error", "fatal"):
            self.log(LOG_WARN if e.get("level") == "warn" else LOG_ERROR,
                     f"Hysteria2: {msg} {e.get('error', '')}".rstrip())
