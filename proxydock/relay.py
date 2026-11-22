"""TCP relay (Pipe), Direct connections, UDP sessions + Direct UDP egress.

Pipe is an asyncio.Protocol. During the handshake it buffers received bytes (readexactly/readuntil);
after link() it passes them straight to the peer Pipe. When the peer's send buffer is full (pause_writing)
it stops reading so memory cannot run away.
Every connection knows its proxy instance (inst), so statistics, logs and settings are per instance.
"""
from .i18n import _t
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
        self.upstream = upstream    # True: client side (bytes received here = upload)
        self.transport = None
        self.peer = None
        self.relay = None
        self.buf = bytearray()
        self.eof = False
        self.closed = False
        self.discard = False        # UDP control connection: received data is ignored
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
        # keep half-close. TLS transports do not support half-close; returning True there only causes a warning.
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

    # stop reading from the peer when my send buffer is full
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
        """Read up to and including sep. ValueError if limit is exceeded."""
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
        """Write data meant for the peer to this connection (tunnels encrypt here)."""
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
    # pass on data that arrived during the handshake first (client data sent early, server banners, ...)
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
    """Link two Pipes and wait until they finish. (close reason, RelayState)."""
    link(a, b)
    st = a.relay
    reason = _t("normal close")
    try:
        while not (a.lost.done() and b.lost.done()):
            remaining = a.inst.cfg["timeout"] - (time.monotonic() - st.last)
            if remaining <= 0:
                reason = _t("idle timeout")
                break
            await asyncio.wait((a.lost, b.lost), timeout=remaining)
    except asyncio.CancelledError:
        reason = _t("cancelled")
        raise
    finally:
        for p in (a, b):
            if not p.closed:
                p.transport.abort()
    return reason, st


# --- Direct connections ---

def _close_if_connected(task):
    if not task.cancelled() and task.exception() is None:
        task.result()[0].close()


def _bind_addr(inst):
    """Direct outbound source address ("" = automatic)."""
    ob = getattr(inst, "outbound", None) or {}
    return ob.get("bind", "") if ob.get("type") == "direct" else ""


async def _race_connect(inst, addrs, delay=0.25, local=None):
    """RFC 8305-style happy eyeballs: start the next address every delay seconds, use whichever connects first."""
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
            pending.add(loop.create_task(loop.create_connection(lambda: Pipe(inst), sa[0], sa[1], local_addr=local)))
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
    """Direct TCP connection to the destination -> Pipe. With private network blocking, only connect to addresses resolved and checked here (prevents DNS rebinding)."""
    loop = asyncio.get_running_loop()
    if block_private is None:
        block_private = inst.cfg["block_private"]
    bind = _bind_addr(inst)
    fam = (socket.AF_INET6 if ":" in bind else socket.AF_INET) if bind else 0
    local = (bind, 0) if bind else None
    if not block_private:
        _, pipe = await loop.create_connection(lambda: Pipe(inst), host, port, happy_eyeballs_delay=0.25,
                                               family=fam, local_addr=local)
        return pipe
    infos = await loop.getaddrinfo(host, port, type=socket.SOCK_STREAM, family=fam)
    v4, v6, seen = [], [], set()
    first_fam = None
    for fam, _, _, _, sa in infos:
        if fam not in (socket.AF_INET, socket.AF_INET6) or sa[0] in seen:
            continue
        if is_internal_ip(sa[0]):
            raise BlockedDestination(_t("private address {sa}").format(sa=sa[0]))
        seen.add(sa[0])
        first_fam = first_fam or fam
        (v6 if fam == socket.AF_INET6 else v4).append(sa)
    if not seen:
        raise OSError(f"no address for {host}")
    a, b = (v6, v4) if first_fam == socket.AF_INET6 else (v4, v6)
    order = [x for pair in zip(a, b) for x in pair] + a[len(b):] + b[len(a):]
    return await _race_connect(inst, order, local=local)


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
# Session = client side (speaks the inbound format) + egress (Direct / SS tunnel / Hysteria2 tunnel).
# The session hands destination packets to the egress with dispatch(); the egress returns replies with deliver().

def disable_udp_connreset(sock):
    """Windows: stop ICMP port-unreachable from the destination surfacing as WSAECONNRESET."""
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
    """Base UDP session. Subclasses dispatch() packets from the client and answer with _reply()."""

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
        self.dropped = 0            # packets dropped by the allow list / private network / DNS failure
        self.started = time.monotonic()

    async def open_egress(self):
        from .outbound import make_udp_egress   # avoid a circular import
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
        """Reply received by the egress -> client."""
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
    """UDP sent straight to the destination. Only accepts replies from IPs it has sent to."""

    MAX_PENDING_DNS = 64     # hosts being resolved at the same time
    MAX_QUEUED = 64          # packets kept per host while it is being resolved
    MAX_KNOWN_IPS = 4096

    def __init__(self, session):
        self.session = session
        self.inst = session.inst
        self.remote_tr = {}
        self.known_ips = set()
        self.resolving = {}         # host -> [(port, payload), ...] waiting for resolution (order kept)

    async def start(self):
        loop = asyncio.get_running_loop()
        bind = _bind_addr(self.inst)
        pairs = [(socket.AF_INET6 if ":" in bind else socket.AF_INET, bind)] if bind else             [(socket.AF_INET, "0.0.0.0"), (socket.AF_INET6, "::")]
        for fam, bind_ip in pairs:
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
            # UDP has no happy eyeballs, so prefer IPv4 (in case the IPv6 route is broken)
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
