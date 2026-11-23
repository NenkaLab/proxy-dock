"""Hysteria2 (QUIC-based tunnel) server and client on top of aioquic.

Protocol (https://v2.hysteria.network/docs/developers/Protocol/):
- QUIC + TLS (ALPN h3). Authentication is an HTTP/3 `POST /auth` with a `Hysteria-Auth` header; status 233 = OK.
- TCP: for every bidirectional stream the client opens after authenticating
      request   varint(0x401) varint(address length) "host:port" varint(padding length) padding
      response  u8 status (0 = OK) varint(message length) message varint(padding length) padding
  after which the stream carries the data as is.
- UDP: QUIC DATAGRAM frames
      u32 session id, u16 packet id, u8 fragment index, u8 fragment count, varint(address length) "host:port", data
Not supported: Brutal congestion control (bandwidth settings), Salamander obfuscation, port hopping.
"""
from .i18n import _t
import asyncio
import hashlib
import os
import secrets
import ssl
import struct
import time

from aioquic.asyncio import QuicConnectionProtocol, connect as quic_connect, serve as quic_serve
from aioquic.buffer import Buffer, BufferReadError, encode_uint_var
from aioquic.h3.connection import H3Connection
from aioquic.h3.events import HeadersReceived
from aioquic.quic.configuration import QuicConfiguration
from aioquic.quic.events import (
    ConnectionTerminated, DatagramFrameReceived, HandshakeCompleted, StreamDataReceived, StreamReset,
)
from cryptography.hazmat.primitives import serialization

from .common import (
    ATYP_DOMAIN, CONNECT_TIMEOUT, HANDSHAKE_TIMEOUT, LOG_AUTH, LOG_ERROR, LOG_TCP, LOG_UDP, UpstreamError,
    describe_error, host_port, is_ip_literal, note_auth_failure, split_host_port, strip_scope, valid_domain,
    is_banned, is_loopback_ip,
)
from .relay import Pipe, UDPSession

TCP_REQUEST_ID = 0x401
STATUS_AUTH_OK = b"233"
UDP_IDLE = 120
MAX_UDP_SESSIONS = 1024
MAX_QUIC_CONNS = 1024               # max concurrent QUIC connections per proxy
MAX_QUIC_PER_IP = 64                # max concurrent QUIC connections from one IP (loopback excluded)
DEFRAG_TTL = 5                      # drop a UDP message whose fragments did not all arrive within this time (s)
DEFRAG_MAX = 128                    # max messages waiting for reassembly per connection
SEND_HIGH = 2 * 1024 * 1024         # stop reading from the peer when the stream send buffer grows this large
SEND_LOW = 512 * 1024


def _padding():
    return os.urandom(secrets.randbelow(64) + 1)


def _hex_padding():
    return secrets.token_hex(secrets.randbelow(32) + 8).encode()


def _quic_config(is_client, idle=30.0):
    cfg = QuicConfiguration(is_client=is_client, alpn_protocols=["h3"], max_datagram_frame_size=65536,
                            idle_timeout=idle, max_data=64 * 1024 * 1024, max_stream_data=8 * 1024 * 1024)
    try:
        cfg.congestion_control_algorithm = "cubic"
    except Exception:
        pass
    return cfg


def _read_varint_from(data, pos):
    buf = Buffer(data=bytes(data[pos:pos + 8]))
    v = buf.pull_uint_var()
    return v, pos + buf.tell()


# --- fake transport on top of a QUIC stream, so the existing Pipe/run_relay can be reused ---

class StreamTransport(asyncio.Transport):
    def __init__(self, proto, stream_id, pipe, peername):
        super().__init__()
        self.proto = proto
        self.sid = stream_id
        self.pipe = pipe
        self.peername = peername
        self.fin_sent = False
        self.fin_recv = False
        self.closing = False
        self.lost_called = False
        self.paused = False
        self.rbuf = []
        self.rfin = False
        self.write_paused = False

    # read side (QUIC -> Pipe)
    # Passing on every QUIC packet (~1.2 KB) right away causes tens of thousands of small writes on the peer TCP socket.
    # Data that arrives in the same event loop iteration is collected and passed on at once.
    def feed(self, data, end):
        if self.lost_called:
            return
        if data:
            self.rbuf.append(data)
        self.rfin = self.rfin or end
        if not self.paused and not getattr(self, "_flush_scheduled", False):
            self._flush_scheduled = True
            asyncio.get_running_loop().call_soon(self._flush)

    def _flush(self):
        self._flush_scheduled = False
        if self.lost_called or self.paused:
            return
        chunks, self.rbuf = self.rbuf, []
        if chunks:
            self.pipe.data_received(chunks[0] if len(chunks) == 1 else b"".join(chunks))
        if self.rfin and not self.fin_recv and not self.paused:
            self.rfin = False
            self.fin_recv = True
            self.pipe.eof_received()
            self._maybe_lost()

    def pause_reading(self):
        self.paused = True

    def resume_reading(self):
        if not self.paused:
            return
        self.paused = False
        if self.rbuf or self.rfin:
            self._flush()

    def is_reading(self):
        return not self.paused

    # write side (Pipe -> QUIC)
    def write(self, data):
        if self.closing or self.fin_sent or self.lost_called or not data:
            return
        try:
            self.proto._quic.send_stream_data(self.sid, bytes(data), False)
        except Exception:
            self.abort()
            return
        self.proto.transmit()
        if not self.write_paused and self.pending() > SEND_HIGH:
            self.write_paused = True
            self.pipe.pause_writing()
            self.proto.watch(self)

    def pending(self):
        stream = self.proto._quic._streams.get(self.sid)
        return len(stream.sender._buffer) if stream is not None else 0

    def check_resume(self):
        if self.write_paused and self.pending() < SEND_LOW:
            self.write_paused = False
            self.pipe.resume_writing()
        return self.write_paused

    def can_write_eof(self):
        return True

    def write_eof(self):
        if self.fin_sent or self.lost_called:
            return
        self.fin_sent = True
        try:
            self.proto._quic.send_stream_data(self.sid, b"", True)
            self.proto.transmit()
        except Exception:
            pass
        self._maybe_lost()

    def close(self):
        if self.closing:
            return
        self.closing = True
        self.write_eof()
        if not self.fin_recv:
            try:
                self.proto._quic.stop_stream(self.sid, 0)
                self.proto.transmit()
            except Exception:
                pass
        self._lost(None)

    def abort(self):
        if self.lost_called:
            return
        self.closing = True
        try:
            self.proto._quic.reset_stream(self.sid, 0)
            self.proto.transmit()
        except Exception:
            pass
        self._lost(None)

    def is_closing(self):
        return self.closing or self.lost_called

    def get_extra_info(self, name, default=None):
        if name == "peername":
            return self.peername
        if name == "sockname":
            return ("0.0.0.0", 0)
        return default

    def reset_by_peer(self):
        self._lost(ConnectionResetError("stream reset"))

    def _maybe_lost(self):
        if self.fin_sent and self.fin_recv:
            self._lost(None)

    def _lost(self, exc):
        if self.lost_called:
            return
        self.lost_called = True
        self.proto.streams.pop(self.sid, None)
        asyncio.get_running_loop().call_soon(self.pipe.connection_lost, exc)


class _Hy2Protocol(QuicConnectionProtocol):
    """Shared by server and client: stream routing, send buffer watching."""

    def __init__(self, *args, **kw):
        super().__init__(*args, **kw)
        self.h3 = None
        self.streams = {}
        self._watched = set()
        self._watch_task = None

    def watch(self, tr):
        self._watched.add(tr)
        if self._watch_task is None or self._watch_task.done():
            self._watch_task = asyncio.get_running_loop().create_task(self._watch_loop())

    async def _watch_loop(self):
        while self._watched:
            await asyncio.sleep(0.02)
            for tr in list(self._watched):
                if tr.lost_called or not tr.check_resume():
                    self._watched.discard(tr)

    def peer(self):
        try:
            addr = self._quic._network_paths[0].addr
            return strip_scope(addr[0]), addr[1]
        except (AttributeError, IndexError):
            return ("?", 0)

    def _route_stream(self, event):
        tr = self.streams.get(event.stream_id)
        if tr is not None:
            tr.feed(event.data, event.end_stream)
            return True
        return False

    def _terminate_all(self):
        for tr in list(self.streams.values()):
            tr.reset_by_peer()


# --- server ---

class Hy2ServerPipe(Pipe):
    def __init__(self, inst):
        super().__init__(inst, upstream=True)


class Hy2Reply:
    def ok(self, p, bind):
        p.write(b"\x00" + encode_uint_var(0) + _len_pad())

    def fail(self, p, reason):
        msg = reason.encode()
        p.write(b"\x01" + encode_uint_var(len(msg)) + msg + _len_pad())


def _len_pad():
    pad = _padding()
    return encode_uint_var(len(pad)) + pad


class Hy2ServerUDPSession(UDPSession):
    def __init__(self, proto, session_id):
        host, port = proto.peer()
        super().__init__(proto.inst, host)
        self.proto = proto
        self.session_id = session_id
        self.client_addr = (host, port)
        self.pid = 0
        self.last = time.monotonic()
        self.ready = False
        self.backlog = []

    async def open(self):
        try:
            await self.open_egress()
        except (OSError, UpstreamError) as e:
            self.inst.log(LOG_ERROR, _t("Could not start Hysteria2 UDP outbound: {describe_error}").format(describe_error=describe_error(e)))
            return
        self.ready = True
        for item in self.backlog:
            self.dispatch(*item)
        self.backlog = []

    def feed(self, host, port, payload):
        self.last = time.monotonic()
        item = (0x01 if is_ip_literal(host) else ATYP_DOMAIN, host, port, payload)
        if self.ready:
            self.dispatch(*item)
        elif len(self.backlog) < 64:
            self.backlog.append(item)

    def _make_hdr(self, host, port):
        return host_port(host, port).encode()

    def _reply(self, addr_bytes, data):
        self.last = time.monotonic()
        self.pid = (self.pid + 1) & 0xFFFF
        send_udp(self.proto, self.session_id, self.pid, addr_bytes, data)


def send_udp(proto, session_id, packet_id, addr_bytes, data):
    """Send a UDP message as DATAGRAM frames, split into fragments if it does not fit in one."""
    quic = proto._quic
    cap = quic.get_datagram_frame_capacity() or 1200
    head_len = 4 + 2 + 2 + len(encode_uint_var(len(addr_bytes))) + len(addr_bytes)
    room = max(cap - head_len, 64)
    chunks = [data[i:i + room] for i in range(0, len(data), room)] or [b""]
    if len(chunks) > 255:
        return
    for i, chunk in enumerate(chunks):
        msg = struct.pack("!IHBB", session_id, packet_id, i, len(chunks)) + \
            encode_uint_var(len(addr_bytes)) + addr_bytes + chunk
        try:
            quic.send_datagram_frame(msg)
        except Exception:
            return
    proto.transmit()


class _Defrag:
    """Reassemble fragmented UDP messages (per session/packet id, old ones are dropped)."""

    def __init__(self):
        self.parts = {}             # insertion order = oldest first

    def add(self, key, idx, count, addr, data):
        if count <= 1:
            return addr, data
        if idx >= count:
            return None
        now = time.monotonic()
        # drop stale incomplete messages (the front is the oldest) + size limit
        while self.parts:
            k0, v0 = next(iter(self.parts.items()))
            if now - v0[0] < DEFRAG_TTL and len(self.parts) < DEFRAG_MAX:
                break
            del self.parts[k0]
        entry = self.parts.get(key)
        if entry is None or entry[1] != count:
            entry = self.parts[key] = [now, count, {}, addr]
        entry[2][idx] = data
        if len(entry[2]) == count:
            del self.parts[key]
            return entry[3], b"".join(entry[2][i] for i in range(count))
        return None


def parse_udp(data):
    sid, pid, idx, count = struct.unpack("!IHBB", data[:8])
    alen, pos = _read_varint_from(data, 8)
    addr = data[pos:pos + alen].decode()
    return sid, pid, idx, count, addr, data[pos + alen:]


class Hy2ServerProtocol(_Hy2Protocol):
    inst = None     # set as a class attribute when created by serve()

    def __init__(self, *args, **kw):
        super().__init__(*args, **kw)
        self.h3 = H3Connection(self._quic)
        self.authed = False
        self.auth_stream = None
        self.pending = {}           # streams whose request has not fully arrived -> bytearray
        self.udp = {}
        self.defrag = _Defrag()
        self.reaper = None
        self.started = time.monotonic()
        self.inst.hy2_conns.add(self)
        if len(self.inst.hy2_conns) > MAX_QUIC_CONNS:
            asyncio.get_running_loop().call_soon(self._reject)
        asyncio.get_running_loop().call_later(HANDSHAKE_TIMEOUT, self._auth_deadline)

    def _reject(self):
        try:
            self._quic.close(error_code=0x100)
            self.transmit()
        except Exception:
            pass

    @property
    def client(self):
        h, p = self.peer()
        return host_port(h, p)

    def _auth_deadline(self):
        if not self.authed:
            self._quic.close(error_code=0x100)
            self.transmit()

    def quic_event_received(self, event):
        if isinstance(event, StreamDataReceived):
            if self._route_stream(event):
                return
            sid = event.stream_id
            if self.authed and sid % 4 == 0 and sid != self.auth_stream:
                self._tcp_request_data(sid, event.data, event.end_stream)
                return
        elif isinstance(event, DatagramFrameReceived):
            if self.authed:
                self._udp_datagram(event.data)
            return
        elif isinstance(event, StreamReset):
            tr = self.streams.get(event.stream_id)
            if tr:
                tr.reset_by_peer()
            self.pending.pop(event.stream_id, None)
            return
        elif isinstance(event, ConnectionTerminated):
            self._shutdown()
            return
        for h3_event in self.h3.handle_event(event):
            if isinstance(h3_event, HeadersReceived):
                self._on_headers(h3_event)

    def _on_headers(self, ev):
        h = {k.lower(): v for k, v in ev.headers}
        inst = self.inst
        ip = self.peer()[0]
        if is_banned(ip) or (not self.authed and not is_loopback_ip(ip) and sum(
                1 for c in inst.hy2_conns if c is not self and c.authed and c.peer()[0] == ip) >= MAX_QUIC_PER_IP):
            self._reject()
            return
        if h.get(b":method") == b"POST" and h.get(b":path") == b"/auth":
            ok = secrets.compare_digest(h.get(b"hysteria-auth", b""), inst.inbound["hy2_password"].encode())
            if ok:
                self.authed = True
                self.auth_stream = ev.stream_id
                self.h3.send_headers(ev.stream_id, [
                    (b":status", STATUS_AUTH_OK), (b"hysteria-udp", b"true"),
                    (b"hysteria-cc-rx", b"auto"), (b"hysteria-padding", _hex_padding()),
                ], end_stream=True)
                self.transmit()
                inst.log_conn(LOG_TCP, _t("H2 {client} connected (Hysteria2 authenticated)").format(client=self.client))
                self.reaper = asyncio.get_running_loop().create_task(self._reap())
                return
            inst.stats.blocked += 1
            note_auth_failure(ip, inst)
            inst.log(LOG_AUTH, _t("H2 {client} authentication failed").format(client=self.client))
        # failed auth / ordinary request: answer 404 like a normal HTTP/3 server
        self.h3.send_headers(ev.stream_id, [(b":status", b"404"), (b"content-length", b"0")], end_stream=True)
        self.transmit()

    def _tcp_request_data(self, sid, data, end):
        buf = self.pending.setdefault(sid, bytearray())
        buf += data
        try:
            rid, pos = _read_varint_from(buf, 0)
            alen, pos = _read_varint_from(buf, pos)
            if len(buf) < pos + alen:
                raise BufferReadError
            addr = bytes(buf[pos:pos + alen]).decode()
            pos += alen
            plen, pos = _read_varint_from(buf, pos)
            if len(buf) < pos + plen:
                raise BufferReadError
            pos += plen
        except (BufferReadError, UnicodeDecodeError, struct.error):
            if len(buf) > 4096:
                self.pending.pop(sid, None)
                self._quic.reset_stream(sid, 0)
                self.transmit()
            return
        del self.pending[sid]
        rest = bytes(buf[pos:])
        pipe = Hy2ServerPipe(self.inst)
        tr = StreamTransport(self, sid, pipe, self.peer())
        self.streams[sid] = tr
        pipe.connection_made(tr)
        if rid != TCP_REQUEST_ID:
            tr.abort()
            return
        if rest or end:
            tr.feed(rest, end)
        task = asyncio.get_running_loop().create_task(self._serve_tcp(pipe, tr, addr))
        self.inst.tasks.add(task)
        task.add_done_callback(self.inst.tasks.discard)

    async def _serve_tcp(self, pipe, tr, addr):
        from .inbound import relay_to
        try:
            host, port = split_host_port(addr)
            if not (is_ip_literal(host) or valid_domain(host)):
                raise ValueError
        except ValueError:
            Hy2Reply().fail(pipe, "bad address")
            tr.close()
            return
        try:
            await relay_to(pipe, self.client, host, port, Hy2Reply(), tag="H2 ")
        except Exception as e:
            self.inst.log(LOG_ERROR, _t("H2 {client} error: {e!r}").format(client=self.client, e=e))
        finally:
            tr.close()

    def _udp_datagram(self, data):
        try:
            sid, pid, idx, count, addr, payload = parse_udp(data)
            host, port = split_host_port(addr)
        except (BufferReadError, ValueError, struct.error, UnicodeDecodeError):
            return
        got = self.defrag.add((sid, pid), idx, count, addr, payload)
        if got is None:
            return
        sess = self.udp.get(sid)
        if sess is None:
            if len(self.udp) >= MAX_UDP_SESSIONS:
                return
            sess = self.udp[sid] = Hy2ServerUDPSession(self, sid)
            self.inst.stats.udp_active += 1
            self.inst.stats.total += 1
            self.inst.log_conn(LOG_UDP, _t("H2 {client} UDP session started").format(client=self.client))
            sess.spawn(sess.open())
        sess.feed(host, port, got[1])

    def _close_udp(self, sid):
        from .inbound import log_udp_close
        sess = self.udp.pop(sid, None)
        if sess is None:
            return
        sess.close()
        self.inst.stats.udp_active -= 1
        log_udp_close(self.inst, f"H2 {self.client}", sess)

    async def _reap(self):
        while True:
            await asyncio.sleep(15)
            now = time.monotonic()
            for sid, sess in list(self.udp.items()):
                if now - sess.last > UDP_IDLE:
                    self._close_udp(sid)

    def _shutdown(self):
        self._terminate_all()
        for sid in list(self.udp):
            self._close_udp(sid)
        if self.reaper:
            self.reaper.cancel()
        self.inst.hy2_conns.discard(self)

    def connection_lost(self, exc):
        self._shutdown()
        super().connection_lost(exc)


class _ServerHandle:
    """What to close on stop: the QUIC server + its connections."""

    def __init__(self, server, inst):
        self.server, self.inst = server, inst

    def close(self):
        for conn in list(self.inst.hy2_conns):
            try:
                conn._quic.close(error_code=0)
                conn.transmit()
            except Exception:
                pass
        self.server.close()


async def start_server(inst):
    ib = inst.inbound
    cert, key = inst.server_cert_paths()
    cfg = _quic_config(is_client=False)
    cfg.load_cert_chain(cert, key)
    proto_cls = type("Hy2ServerProtocolBound", (Hy2ServerProtocol,), {"inst": inst})
    server = await quic_serve(ib["host"], ib["port"], configuration=cfg, create_protocol=proto_cls)
    return _ServerHandle(server, inst)


# --- client (outbound Hysteria2 tunnel) ---

class Hy2ClientProtocol(_Hy2Protocol):
    def __init__(self, *args, **kw):
        super().__init__(*args, **kw)
        self.h3 = H3Connection(self._quic)
        self.auth_future = asyncio.get_running_loop().create_future()
        # avoid "Future exception was never retrieved" when it fails after the waiter gave up
        self.auth_future.add_done_callback(lambda f: f.cancelled() or f.exception())
        self.udp_handlers = {}      # session id -> function(host, port, data)
        self.defrag = _Defrag()
        self.closed = False

    def quic_event_received(self, event):
        if isinstance(event, StreamDataReceived):
            if self._route_stream(event):
                return
        elif isinstance(event, DatagramFrameReceived):
            try:
                sid, pid, idx, count, addr, payload = parse_udp(event.data)
                host, port = split_host_port(addr)
            except (BufferReadError, ValueError, struct.error, UnicodeDecodeError):
                return
            got = self.defrag.add((sid, pid), idx, count, addr, payload)
            fn = self.udp_handlers.get(sid)
            if got is not None and fn is not None:
                fn(host, port, got[1])
            return
        elif isinstance(event, StreamReset):
            tr = self.streams.get(event.stream_id)
            if tr:
                tr.reset_by_peer()
            return
        elif isinstance(event, ConnectionTerminated):
            self.closed = True
            self._terminate_all()
            if not self.auth_future.done():
                if event.error_code == 0x150:   # TLS internal_error: the official server's sniGuard rejects a mismatched SNI
                    msg = (_t("The Hysteria2 server rejected TLS - try entering the server certificate's domain in 'SNI' (connecting by IP can make the SNI not match the certificate)"))
                else:
                    msg = _t("The Hysteria2 server closed the connection ({value})").format(value=event.reason_phrase or hex(event.error_code))
                self.auth_future.set_exception(UpstreamError(msg, "net"))
            return
        for h3_event in self.h3.handle_event(event):
            if isinstance(h3_event, HeadersReceived) and not self.auth_future.done():
                h = dict(h3_event.headers)
                if h.get(b":status") == STATUS_AUTH_OK:
                    self.auth_future.set_result(h)
                else:
                    self.auth_future.set_exception(UpstreamError(
                        _t("Hysteria2 authentication failed (status {decode})").format(decode=h.get(b':status', b'?').decode()), "auth"))

    def connection_lost(self, exc):
        self.closed = True
        self._terminate_all()
        super().connection_lost(exc)


class Hy2Client:
    """One per proxy instance. Keeps the QUIC connection to the remote Hysteria2 server and reconnects when it drops."""

    def __init__(self, inst, ob=None):
        self.inst = inst
        self.ob = ob or inst.outbound
        self.cm = None
        self.proto = None
        self.lock = asyncio.Lock()

    async def get(self):
        async with self.lock:
            if self.proto is not None and not self.proto.closed:
                return self.proto
            await self._close()
            ob = self.ob
            cfg = _quic_config(is_client=True)
            cfg.server_name = ob.get("sni") or ob["host"]
            pin = (ob.get("pin_sha256") or "").replace(":", "").lower()
            if not ob.get("tls_verify", True) or pin:
                cfg.verify_mode = ssl.CERT_NONE
            self.cm = quic_connect(ob["host"], ob["port"], configuration=cfg, create_protocol=Hy2ClientProtocol)
            try:
                proto = await asyncio.wait_for(self.cm.__aenter__(), CONNECT_TIMEOUT)
            except Exception as e:
                self.cm = None
                raise UpstreamError(_t("Could not connect to Hysteria2 server {host_port}: {describe_error}").format(host_port=host_port(ob['host'], ob['port']), describe_error=describe_error(e)), "net") from e
            if pin:
                certs = proto._quic.tls.peer_certificates
                got = hashlib.sha256(certs[0].public_bytes(serialization.Encoding.DER)).hexdigest() if certs else ""
                if got != pin:
                    self.proto = proto
                    await self._close()
                    raise UpstreamError(_t("The Hysteria2 server certificate does not match the pinned fingerprint"), "denied")
            sid = proto._quic.get_next_available_stream_id()
            proto.h3.send_headers(sid, [
                (b":method", b"POST"), (b":scheme", b"https"), (b":authority", b"hysteria"),
                (b":path", b"/auth"), (b"hysteria-auth", ob["hy2_password"].encode()),
                (b"hysteria-cc-rx", b"0"), (b"hysteria-padding", _hex_padding()),
            ], end_stream=True)
            proto.transmit()
            self.proto = proto
            try:
                await asyncio.wait_for(asyncio.shield(proto.auth_future), CONNECT_TIMEOUT)
            except Exception:
                await self._close()
                raise
            return proto

    async def _close(self):
        cm, self.cm, self.proto = self.cm, None, None
        if cm is not None:
            try:
                await cm.__aexit__(None, None, None)
            except Exception:
                pass

    async def close(self):
        async with self.lock:
            await self._close()


async def _read_varint(pipe):
    first = await pipe.readexactly(1)
    n = 1 << (first[0] >> 6)
    rest = await pipe.readexactly(n - 1) if n > 1 else b""
    v, _ = _read_varint_from(first + rest, 0)
    return v


async def open_tcp(inst, host, port, ob=None):
    """Open a TCP stream to host:port through the remote Hysteria2 server -> Pipe."""
    proto = await inst.hy2_client(ob).get()
    sid = proto._quic.get_next_available_stream_id()
    pipe = Pipe(inst)
    tr = StreamTransport(proto, sid, pipe, proto.peer())
    proto.streams[sid] = tr
    pipe.connection_made(tr)
    addr = host_port(host, port).encode()
    pad = _padding()
    tr.write(encode_uint_var(TCP_REQUEST_ID) + encode_uint_var(len(addr)) + addr +
             encode_uint_var(len(pad)) + pad)
    try:
        status = (await pipe.readexactly(1))[0]
        mlen = await _read_varint(pipe)
        msg = (await pipe.readexactly(mlen)).decode(errors="replace") if mlen else ""
        plen = await _read_varint(pipe)
        if plen:
            await pipe.readexactly(plen)
    except Exception:
        tr.abort()
        raise UpstreamError(_t("The Hysteria2 server did not respond"), "general")
    if status != 0:
        tr.abort()
        raise UpstreamError(_t("Rejected by the Hysteria2 server: {value}").format(value=msg or status), "refused")
    return pipe


class Hy2Egress:
    """Tunnel UDP through the remote Hysteria2 server."""

    def __init__(self, session):
        self.session = session
        self.inst = session.inst
        self.proto = None
        self.sid = secrets.randbits(32)
        self.pid = 0

    async def start(self):
        self.proto = await self.inst.hy2_client().get()
        self.proto.udp_handlers[self.sid] = self._recv
        return self

    def close(self):
        if self.proto is not None:
            self.proto.udp_handlers.pop(self.sid, None)

    def send(self, atyp, host, port, payload):
        from .outbound import check_destination
        if self.proto is None or self.proto.closed:
            return False
        try:
            check_destination(self.inst, host)
        except Exception:
            self.session.dropped += 1
            return False
        self.pid = (self.pid + 1) & 0xFFFF
        send_udp(self.proto, self.sid, self.pid, host_port(host, port).encode(), payload)
        return True

    def _recv(self, host, port, data):
        self.session.deliver(host, port, data)


def hy2_link(host, port, password, sni="", insecure=False, name="ProxyDock", obfs=""):
    import urllib.parse
    h = f"[{host}]" if ":" in host else host
    q = []
    if obfs:
        q.append("obfs=salamander")
        q.append(f"obfs-password={urllib.parse.quote(obfs, safe='')}")
    if sni:
        q.append(f"sni={urllib.parse.quote(sni)}")
    if insecure:
        q.append("insecure=1")
    query = ("?" + "&".join(q)) if q else ""
    return f"hysteria2://{urllib.parse.quote(password, safe='')}@{h}:{port}/{query}#{urllib.parse.quote(name, safe='')}"
