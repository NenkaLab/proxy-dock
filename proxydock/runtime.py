"""프록시 실행: ProxyInstance(프록시 하나의 실행 상태) 와 ProxyManager(이벤트 루프 스레드에서 여러 프록시 관리)."""
import asyncio
import errno
import threading
from concurrent.futures import ThreadPoolExecutor

from . import config
from .common import (
    INBOUND_TYPES, LOG_ERROR, LOG_INFO, LOG_WARN, OUTBOUND_TYPES, Stats, describe_error, host_port,
    is_loopback_host, log,
)


class ProxyInstance:
    def __init__(self, proxy, glob, stats):
        self.stats = stats
        self.conns = set()
        self.tasks = set()
        self.hy2_conns = set()
        self.servers = []
        self._hy2 = None
        self.last_error = ""
        self.update(proxy, glob)

    # --- 설정 ---
    def update(self, proxy, glob):
        """설정 갱신. 받는/내보내는 쪽 값이 바뀐 경우는 매니저가 재시작한다."""
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
        ob = self.outbound
        if ob["type"] == "direct":
            return "Direct"
        return f"{OUTBOUND_TYPES[ob['type']]} {host_port(ob['host'], ob['port'])}"

    @property
    def summary(self):
        ib = self.inbound
        return f"{INBOUND_TYPES[ib['type']]} {host_port(ib['host'], ib['port'])} → {self.outbound_label}"

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

    def hy2_client(self):
        if self._hy2 is None:
            from .hysteria2 import Hy2Client
            self._hy2 = Hy2Client(self)
        return self._hy2

    # --- 시작 / 정지 ---
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
        if self._hy2 is not None:
            await self._hy2.close()
            self._hy2 = None


def _net_key(p):
    return (p["inbound"], p["outbound"])


class ProxyManager:
    """이벤트 루프 스레드 하나에서 모든 프록시를 돌린다. UI 스레드에서 부르는 메서드는 스레드 안전."""

    def __init__(self, status_hook=None):
        self.status_hook = status_hook      # (프록시 ID, 상태, 메시지) - 루프 스레드에서 호출됨
        self.instances = {}                 # 실행 중: id -> ProxyInstance
        self.stats = {}                     # id -> Stats (정지해도 누적 유지)
        self.status = {}                    # id -> (상태, 메시지)
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

    # --- UI 스레드에서 부르는 것 ---
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
        """프록시 설정이 바뀜: 실행 중이면 받는/내보내는 쪽 변경은 재시작, 나머지는 바로 반영."""
        return self.submit(self._apply(proxy, glob))

    def apply_global(self, glob, proxies):
        """전역 기본값이 바뀜: '전역 따름' 상태인 실행 중 프록시에 바로 반영."""
        by_id = {p["id"]: p for p in proxies}

        def run():
            for pid, inst in list(self.instances.items()):
                if pid in by_id:
                    inst.update(by_id[pid], glob)
        self.loop.call_soon_threadsafe(run)

    # --- 루프 스레드 ---
    async def _start(self, proxy, glob):
        pid = proxy["id"]
        if pid in self.instances:
            await self._stop(pid, quiet=True)
        self._set(pid, "starting")
        inst = None
        try:
            inst = ProxyInstance(proxy, glob, self.stats_for(pid))
            await inst.start()
        except Exception as e:
            msg = describe_error(e) if isinstance(e, OSError) else str(e) or type(e).__name__
            code = getattr(e, "winerror", None) or getattr(e, "errno", None)
            port = proxy["inbound"]["port"]
            if code in (10048, errno.EADDRINUSE, 98, 48):
                msg = f"포트 {port} 를 이미 다른 프로그램이 쓰고 있습니다"
            elif code in (10013, errno.EACCES):
                msg = f"포트 {port} 를 쓸 수 없습니다 (Windows 예약 범위 또는 권한)"
            elif code in (10049, errno.EADDRNOTAVAIL):
                msg = f"이 PC 에 {proxy['inbound']['host']} 주소가 없습니다"
            self._set(pid, "error", msg)
            log(LOG_ERROR, f"[{proxy['name']}] 시작 실패: {msg}")
            if inst is not None:
                try:
                    await inst.stop()
                except Exception:
                    pass
            return False
        self.instances[pid] = inst
        self._set(pid, "running")
        inst.log(LOG_INFO, f"시작: {inst.summary}")
        ib = inst.inbound
        if not is_loopback_host(ib["host"]):
            if ib["type"] == "socks4":
                inst.log(LOG_WARN, "SOCKS4 는 비밀번호 인증이 없어 외부에 공개되면 누구나 쓸 수 있습니다")
            elif ib["type"] in ("socks5", "http", "https") and not ib["auth_enabled"]:
                inst.log(LOG_WARN, f"{host_port(ib['host'], ib['port'])} 에서 인증 없이 수신 중입니다 (오픈 프록시)")
        return True

    async def _stop(self, pid, quiet=False):
        inst = self.instances.pop(pid, None)
        if inst is not None:
            await inst.stop()
            if not quiet:
                inst.log(LOG_INFO, "정지")
        if not quiet:
            self._set(pid, "stopped")

    async def _stop_all(self):
        for pid in list(self.instances):
            await self._stop(pid)

    async def _apply(self, proxy, glob):
        inst = self.instances.get(proxy["id"])
        if inst is None:
            return
        if _net_key(inst.proxy) != _net_key(proxy):
            await self._start(proxy, glob)
        else:
            inst.update(proxy, glob)


def _loop_exception(loop, context):
    # Proactor 가 이미 끊긴/half-close 된 소켓을 닫을 때 shutdown() 에서 나는 OSError 는 무시
    exc = context.get("exception")
    if isinstance(exc, OSError) and "_call_connection_lost" in str(context.get("handle", "")):
        return
    if isinstance(exc, (ConnectionResetError, ConnectionAbortedError)):
        return
    loop.default_exception_handler(context)
