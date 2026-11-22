"""TCP 중계(Pipe), Direct 연결, UDP 세션 + Direct UDP 내보내기.

Pipe 는 asyncio.Protocol 이다. 협상 단계에서는 받은 바이트를 버퍼에 모았다가(readexactly/readuntil),
link() 이후에는 받은 즉시 상대 Pipe 로 넘긴다. 상대 송신 버퍼가 차면(pause_writing) 읽기를 멈춰 메모리 폭주를 막는다.
모든 연결은 자기가 속한 프록시 인스턴스(inst)를 알고 있어 통계·로그·설정을 인스턴스별로 쓴다.
"""
import asyncio
import ctypes
import socket
import time

from .common import (
    ATYP_DOMAIN, DNS_TTL, HANDSHAKE_LIMIT, IS_WIN, UDP_BUF, BlockedDestination, is_internal_ip,
    make_udp_header, normalize_ip, strip_scope,
)


class RelayState:
    __slots__ = ("last", "up", "down")


class Pipe(asyncio.Protocol):
    def __init__(self, inst, upstream=False):
        self.inst = inst
        self.upstream = upstream    # True: 클라이언트 쪽 (여기서 받은 바이트 = 업로드)
        self.transport = None
        self.peer = None
        self.relay = None
        self.buf = bytearray()
        self.eof = False
        self.closed = False
        self.discard = False        # UDP 제어 연결: 받은 데이터 무시
        self.waiter = None
        self.lost = None

    def connection_made(self, transport):
        self.transport = transport
        self.tls = transport.get_extra_info("sslcontext") is not None
        self.lost = asyncio.get_running_loop().create_future()

    def data_received(self, data):
        peer = self.peer
        if peer is not None:
            if not peer.closed:
                peer.send(data)
            st = self.relay
            st.last = time.monotonic()
            stats = self.inst.stats
            if self.upstream:
                st.up += len(data)
                stats.up += len(data)
            else:
                st.down += len(data)
                stats.down += len(data)
            return
        if self.discard:
            return
        self.buf += data
        self._wake()
        if len(self.buf) > HANDSHAKE_LIMIT:
            self.transport.pause_reading()

    def eof_received(self):
        self.eof = True
        peer = self.peer
        if peer is not None:
            if peer.eof:
                self.transport.close()
                peer.transport.close()
            elif not peer.closed:
                half_close(peer.transport)
        elif self.discard:
            self.transport.close()
        else:
            self._wake()
        # half-close 유지. TLS 트랜스포트는 half-close 를 지원하지 않아 True 를 돌려주면 경고만 난다.
        return not getattr(self, "tls", False)

    def connection_lost(self, exc):
        self.closed = True
        self.eof = True
        self._wake()
        if self.lost is not None and not self.lost.done():
            self.lost.set_result(None)
        peer = self.peer
        if peer is not None and not peer.closed:
            peer.transport.close()

    # 내 송신 버퍼가 가득 차면 상대 읽기를 멈춘다
    def pause_writing(self):
        if self.peer is not None and not self.peer.closed:
            self.peer.transport.pause_reading()

    def resume_writing(self):
        if self.peer is not None and not self.peer.closed:
            self.peer.transport.resume_reading()

    def _wake(self):
        w = self.waiter
        if w is not None and not w.done():
            w.set_result(None)

    async def _wait_data(self):
        if self.eof:
            raise asyncio.IncompleteReadError(bytes(self.buf), None)
        if not self.transport.is_closing():
            self.transport.resume_reading()
        self.waiter = asyncio.get_running_loop().create_future()
        try:
            await self.waiter
        finally:
            self.waiter = None

    async def readexactly(self, n):
        while len(self.buf) < n:
            await self._wait_data()
        data = bytes(self.buf[:n])
        del self.buf[:n]
        return data

    async def readuntil(self, sep, limit=HANDSHAKE_LIMIT):
        """sep 까지(포함) 읽는다. limit 를 넘으면 ValueError."""
        while True:
            i = self.buf.find(sep)
            if i >= 0:
                data = bytes(self.buf[:i + len(sep)])
                del self.buf[:i + len(sep)]
                return data
            if len(self.buf) > limit:
                raise ValueError("header too long")
            await self._wait_data()

    def send(self, data):
        """상대에게 보낼 데이터를 이 연결로 쓴다 (터널은 여기서 암호화)."""
        self.transport.write(data)

    def write(self, data):
        if not self.closed:
            self.send(data)


def half_close(transport):
    try:
        if transport.can_write_eof():
            transport.write_eof()
        else:
            transport.close()
    except (OSError, RuntimeError):
        transport.close()


def link(a, b):
    st = RelayState()
    st.last = time.monotonic()
    st.up = st.down = 0
    a.relay = b.relay = st
    a.peer, b.peer = b, a
    stats = a.inst.stats
    # 협상 중 미리 도착한 데이터(클라이언트의 선행 전송, 서버 배너 등)를 먼저 넘긴다
    for src, dst in ((a, b), (b, a)):
        if src.buf:
            data = bytes(src.buf)
            src.buf.clear()
            dst.write(data)
            if src.upstream:
                st.up += len(data)
                stats.up += len(data)
            else:
                st.down += len(data)
                stats.down += len(data)
        if not src.closed:
            src.transport.resume_reading()
    for src, dst in ((a, b), (b, a)):
        if src.closed:
            if not dst.closed:
                dst.transport.close()
        elif src.eof and not dst.closed:
            half_close(dst.transport)
    if a.eof and b.eof:
        a.transport.close()
        b.transport.close()


async def run_relay(a, b):
    """두 Pipe 를 잇고 끝날 때까지 기다린다. (종료 사유, RelayState)."""
    link(a, b)
    st = a.relay
    reason = "정상 종료"
    try:
        while not (a.lost.done() and b.lost.done()):
            remaining = a.inst.cfg["timeout"] - (time.monotonic() - st.last)
            if remaining <= 0:
                reason = "유휴 타임아웃"
                break
            await asyncio.wait((a.lost, b.lost), timeout=remaining)
    except asyncio.CancelledError:
        reason = "취소됨"
        raise
    finally:
        for p in (a, b):
            if not p.closed:
                p.transport.abort()
    return reason, st


# --- Direct 연결 ---

def _close_if_connected(task):
    if not task.cancelled() and task.exception() is None:
        task.result()[0].close()


async def _race_connect(inst, addrs, delay=0.25):
    """RFC 8305 식 happy-eyeballs: delay 간격으로 다음 주소를 동시에 시도, 먼저 붙는 쪽 사용."""
    loop = asyncio.get_running_loop()
    pending = set()
    errors = []

    def pick(done):
        winner = None
        for t in done:
            exc = t.exception()
            if exc is not None:
                errors.append(exc)
            elif winner is None:
                winner = t.result()[1]
            else:
                t.result()[0].close()
        return winner

    try:
        for sa in addrs:
            pending.add(loop.create_task(loop.create_connection(lambda: Pipe(inst), sa[0], sa[1])))
            done, pending = await asyncio.wait(pending, timeout=delay, return_when=asyncio.FIRST_COMPLETED)
            winner = pick(done)
            if winner is not None:
                return winner
        while pending:
            done, pending = await asyncio.wait(pending, return_when=asyncio.FIRST_COMPLETED)
            winner = pick(done)
            if winner is not None:
                return winner
        raise errors[-1] if errors else OSError("connect failed")
    finally:
        for t in pending:
            t.cancel()
            t.add_done_callback(_close_if_connected)


async def open_direct(inst, host, port, block_private=None):
    """목적지에 직접 TCP 연결 -> Pipe. 내부망 차단이면 직접 해석·검사한 주소로만 연결(DNS rebinding 방지)."""
    loop = asyncio.get_running_loop()
    if block_private is None:
        block_private = inst.cfg["block_private"]
    if not block_private:
        _, pipe = await loop.create_connection(lambda: Pipe(inst), host, port, happy_eyeballs_delay=0.25)
        return pipe
    infos = await loop.getaddrinfo(host, port, type=socket.SOCK_STREAM)
    v4, v6, seen = [], [], set()
    first_fam = None
    for fam, _, _, _, sa in infos:
        if fam not in (socket.AF_INET, socket.AF_INET6) or sa[0] in seen:
            continue
        if is_internal_ip(sa[0]):
            raise BlockedDestination(f"내부망 주소 {sa[0]}")
        seen.add(sa[0])
        first_fam = first_fam or fam
        (v6 if fam == socket.AF_INET6 else v4).append(sa)
    if not seen:
        raise OSError(f"no address for {host}")
    a, b = (v6, v4) if first_fam == socket.AF_INET6 else (v4, v6)
    order = [x for pair in zip(a, b) for x in pair] + a[len(b):] + b[len(a):]
    return await _race_connect(inst, order)


def enable_keepalive(transport):
    sock = transport.get_extra_info("socket")
    if sock is None:
        return
    try:
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_KEEPALIVE, 1)
        if IS_WIN:
            sock.ioctl(socket.SIO_KEEPALIVE_VALS, (1, 60000, 10000))
        else:
            idle = getattr(socket, "TCP_KEEPIDLE", getattr(socket, "TCP_KEEPALIVE", None))
            if idle is not None:
                sock.setsockopt(socket.IPPROTO_TCP, idle, 60)
            if hasattr(socket, "TCP_KEEPINTVL"):
                sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_KEEPINTVL, 10)
    except (OSError, ValueError, AttributeError):
        pass


# --- UDP ---
# 세션 = 클라이언트 쪽(받는 쪽 형식으로 주고받기) + 내보내기(egress: Direct / SS 터널 / Hysteria2 터널).
# 세션은 dispatch() 로 목적지 패킷을 egress 에 넘기고, egress 는 deliver() 로 응답을 돌려준다.

def disable_udp_connreset(sock):
    """Windows: 목적지의 ICMP port-unreachable 이 WSAECONNRESET 으로 올라오는 것을 막는다."""
    if not IS_WIN:
        return
    try:
        val = ctypes.c_uint32(0)
        ret = ctypes.c_uint32(0)
        ctypes.windll.ws2_32.WSAIoctl(
            ctypes.c_size_t(sock.fileno()), 0x9800000C,  # SIO_UDP_CONNRESET
            ctypes.byref(val), 4, None, 0, ctypes.byref(ret), None, None,
        )
    except Exception:
        pass


def make_udp_socket(family, bind_ip):
    sock = socket.socket(family, socket.SOCK_DGRAM)
    try:
        sock.setblocking(False)
        if family == socket.AF_INET6:
            sock.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
        for opt in (socket.SO_RCVBUF, socket.SO_SNDBUF):
            try:
                sock.setsockopt(socket.SOL_SOCKET, opt, UDP_BUF)
            except OSError:
                pass
        sock.bind((bind_ip, 0))
        disable_udp_connreset(sock)
    except Exception:
        sock.close()
        raise
    return sock


class DatagramCallback(asyncio.DatagramProtocol):
    def __init__(self, fn):
        self.fn = fn

    def datagram_received(self, data, addr):
        self.fn(data, addr)

    def error_received(self, exc):
        pass


class UDPSession:
    """UDP 세션 공통. 하위 클래스가 클라이언트에서 받은 패킷을 dispatch() 하고 _reply() 로 돌려준다."""

    MAX_HDR_CACHE = 4096

    def __init__(self, inst, client_ip):
        self.inst = inst
        self.client_ip = strip_scope(client_ip)
        self.client_addr = None
        self.egress = None
        self.hdr_cache = {}
        self.tasks = set()
        self.closed = False
        self.up = 0
        self.down = 0
        self.dropped = 0            # 허용 목록/내부망/DNS 실패로 버린 패킷
        self.started = time.monotonic()

    async def open_egress(self):
        from .outbound import make_udp_egress   # 순환 import 방지
        self.egress = await make_udp_egress(self.inst, self)

    def close(self):
        self.closed = True
        for t in list(self.tasks):
            t.cancel()
        if self.egress is not None:
            self.egress.close()

    def spawn(self, coro):
        task = asyncio.get_running_loop().create_task(coro)
        self.tasks.add(task)
        task.add_done_callback(self.tasks.discard)
        return task

    def dispatch(self, atyp, dst_addr, dst_port, payload):
        if self.closed or self.egress is None:
            return
        if not self.inst.allowed(dst_addr):
            self.dropped += 1
            return
        if self.egress.send(atyp, dst_addr, dst_port, payload):
            self.up += len(payload)
            self.inst.stats.up += len(payload)

    def deliver(self, src_host, src_port, data):
        """egress 가 받은 응답을 클라이언트로."""
        if self.closed or self.client_addr is None:
            return
        key = (src_host, src_port)
        hdr = self.hdr_cache.get(key)
        if hdr is None:
            if len(self.hdr_cache) >= self.MAX_HDR_CACHE:
                self.hdr_cache.clear()
            hdr = self.hdr_cache[key] = self._make_hdr(src_host, src_port)
        try:
            self._reply(hdr, data)
        except OSError:
            return
        self.down += len(data)
        self.inst.stats.down += len(data)

    def _make_hdr(self, host, port):
        return make_udp_header(host, port)

    def _reply(self, hdr, data):
        raise NotImplementedError


_dns_cache = {}


class DirectEgress:
    """목적지로 직접 보내는 UDP. 보낸 적 있는 IP 에서 온 응답만 받는다."""

    MAX_PENDING_DNS = 64     # 동시에 해석 중인 호스트 수
    MAX_QUEUED = 64          # 해석 대기 중 호스트당 보관 패킷 수
    MAX_KNOWN_IPS = 4096

    def __init__(self, session):
        self.session = session
        self.inst = session.inst
        self.remote_tr = {}
        self.known_ips = set()
        self.resolving = {}         # host -> [(port, payload), ...] 해석 대기열 (순서 유지)

    async def start(self):
        loop = asyncio.get_running_loop()
        for fam, bind_ip in ((socket.AF_INET, "0.0.0.0"), (socket.AF_INET6, "::")):
            try:
                rsock = make_udp_socket(fam, bind_ip)
            except OSError:
                continue
            tr, _ = await loop.create_datagram_endpoint(lambda: DatagramCallback(self.from_remote), sock=rsock)
            self.remote_tr[fam] = tr
        return self

    def close(self):
        for tr in self.remote_tr.values():
            tr.close()

    def send(self, atyp, host, port, payload):
        if atyp != ATYP_DOMAIN:
            return self._send(payload, host, port)
        queue = self.resolving.get(host)
        if queue is not None:
            if len(queue) < self.MAX_QUEUED:
                queue.append((port, payload))
            return False
        hit = _dns_cache.get(host)
        if hit and hit[0] > time.monotonic():
            return self._send(payload, hit[1], port)
        if len(self.resolving) >= self.MAX_PENDING_DNS:
            return False
        self.resolving[host] = [(port, payload)]
        self.session.spawn(self._resolve(host))
        return False

    async def _resolve(self, host):
        ip = None
        try:
            infos = await asyncio.get_running_loop().getaddrinfo(host, None, type=socket.SOCK_DGRAM)
            # UDP 는 happy-eyeballs 가 없으므로 IPv4 를 우선 (IPv6 경로가 깨진 환경 대비)
            infos.sort(key=lambda i: i[0] != socket.AF_INET)
            for fam, _, _, _, sockaddr in infos:
                if fam in self.remote_tr:
                    ip = normalize_ip(sockaddr[0])
                    break
        except (OSError, ValueError):
            pass
        finally:
            queued = self.resolving.pop(host, ())
        if ip is None:
            self.session.dropped += len(queued)
            return
        if len(_dns_cache) > 1024:
            _dns_cache.clear()
        _dns_cache[host] = (time.monotonic() + DNS_TTL, ip)
        for port, payload in queued:
            if self._send(payload, ip, port):
                self.session.up += len(payload)
                self.inst.stats.up += len(payload)

    def _send(self, payload, ip, port):
        if self.session.closed:
            return False
        tr = self.remote_tr.get(socket.AF_INET6 if ":" in ip else socket.AF_INET)
        if tr is None:
            return False
        if ip not in self.known_ips:
            if self.inst.cfg["block_private"] and is_internal_ip(ip):
                self.session.dropped += 1
                return False
            if len(self.known_ips) >= self.MAX_KNOWN_IPS:
                self.known_ips.clear()
            self.known_ips.add(ip)
        try:
            tr.sendto(payload, (ip, port))
        except OSError:
            return False
        return True

    def from_remote(self, data, addr):
        ip = addr[0]
        if ip not in self.known_ips:
            try:
                ip = normalize_ip(ip)
            except (OSError, ValueError):
                return
            if ip not in self.known_ips:
                return
        self.session.deliver(ip, addr[1], data)
