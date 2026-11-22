"""내보내는 쪽: 목적지까지의 연결을 만든다.

connect(inst, host, port) -> 목적지와 이어진 Pipe (상위 프록시/터널 협상까지 끝난 상태, 바로 link 가능)
make_udp_egress(inst, session) -> UDP 내보내기 객체 (Direct / Shadowsocks / Hysteria2)
"""
import asyncio
import base64
import re
import socket
import struct

from .common import (
    ATYP_DOMAIN, ATYP_IPV4, ATYP_IPV6, BlockedDestination, UpstreamError, encode_address, host_port,
    is_internal_ip, is_ip_literal, parse_address,
)
from .relay import DatagramCallback, DirectEgress, Pipe, open_direct
from . import ss as ssc
from .tls import client_context


_LOOSE_IPV4 = re.compile(r"(?:0x[0-9a-f]+|[0-9]+)(?:\.(?:0x[0-9a-f]+|[0-9]+)){0,3}")


def _as_ip(h):
    """IP 로 해석될 수 있는 문자열이면 표준 표기 IP. 127.1, 0x7f.0.0.1, 2130706433 같은 옛 IPv4 표기도
    상위 프록시/OS 가 IP 로 받아들이므로 같이 처리한다."""
    if is_ip_literal(h):
        return h
    if _LOOSE_IPV4.fullmatch(h):
        try:
            return socket.inet_ntoa(socket.inet_aton(h))
        except OSError:
            return None
    return None


def check_destination(inst, host):
    """상위 프록시/터널을 쓸 때 내부망 차단: 도메인은 상위가 해석하므로 IP 직접 지정과 localhost 만 검사."""
    if not inst.cfg["block_private"]:
        return
    h = host.lower().rstrip(".")
    ip = _as_ip(h)
    if h == "localhost" or h.endswith(".localhost") or (ip is not None and is_internal_ip(ip)):
        raise BlockedDestination(f"내부망 주소 {host}")


def _is_self(inst, host, port):
    """목적지가 이 프록시 자신의 받는 쪽이면 True (자기 자신으로의 무한 연결 방지)."""
    ib = inst.inbound
    if port != ib["port"]:
        return False
    h = host.lower().rstrip(".")
    if h == ib["host"].lower() or h == "localhost":
        return True
    ip = _as_ip(h)
    if ip is None:
        return False
    import ipaddress
    a = ipaddress.ip_address(ip.split("%", 1)[0])
    return a.is_loopback or a.is_unspecified


async def _tcp(inst, ob, tls):
    loop = asyncio.get_running_loop()
    kw = {}
    if tls:
        kw["ssl"] = client_context(ob.get("tls_verify", True))
        kw["server_hostname"] = ob.get("sni") or ob["host"]
    try:
        _, pipe = await loop.create_connection(lambda: Pipe(inst), ob["host"], ob["port"],
                                               happy_eyeballs_delay=0.25, **kw)
    except OSError as e:
        raise UpstreamError(f"상위 프록시 {host_port(ob['host'], ob['port'])} 접속 실패: {e.strerror or e}",
                            "net") from e
    return pipe


async def _http_connect(inst, ob, host, port, tls):
    pipe = await _tcp(inst, ob, tls)
    try:
        target = host_port(host, port)
        req = [f"CONNECT {target} HTTP/1.1", f"Host: {target}", "User-Agent: ProxyDock"]
        if ob.get("username"):
            cred = base64.b64encode(f"{ob['username']}:{ob.get('password', '')}".encode()).decode()
            req.append(f"Proxy-Authorization: Basic {cred}")
        pipe.write(("\r\n".join(req) + "\r\n\r\n").encode())
        head = (await pipe.readuntil(b"\r\n\r\n")).decode("latin-1")
        status_line = head.split("\r\n", 1)[0]
        parts = status_line.split(" ", 2)
        code = int(parts[1]) if len(parts) > 1 and parts[1].isdigit() else 0
        if code // 100 != 2:
            reason = {407: "auth", 403: "denied", 502: "host", 504: "timeout"}.get(code, "general")
            raise UpstreamError(f"상위 HTTP 프록시 응답: {status_line.strip()}", reason)
        return pipe
    except BaseException:
        pipe.transport.abort()
        raise


async def _socks4_connect(inst, ob, host, port):
    if ":" in host:
        raise UpstreamError("SOCKS4 는 IPv6 주소를 지원하지 않습니다", "general")
    pipe = await _tcp(inst, ob, False)
    try:
        user = (ob.get("username") or "").encode()
        if is_ip_literal(host):
            req = struct.pack("!BBH", 4, 1, port) + socket.inet_aton(host) + user + b"\0"
        else:       # SOCKS4a: IP 자리에 0.0.0.1, 뒤에 도메인
            req = struct.pack("!BBH", 4, 1, port) + b"\0\0\0\1" + user + b"\0" + host.encode("idna") + b"\0"
        pipe.write(req)
        rep = await pipe.readexactly(8)
        if rep[1] != 0x5A:
            raise UpstreamError(f"상위 SOCKS4 프록시가 거부함 (코드 0x{rep[1]:02x})", "refused")
        return pipe
    except BaseException:
        pipe.transport.abort()
        raise


_SOCKS5_REASONS = {1: "general", 2: "denied", 3: "net", 4: "host", 5: "refused", 6: "timeout"}


async def _socks5_connect(inst, ob, host, port):
    pipe = await _tcp(inst, ob, False)
    try:
        user, pw = ob.get("username") or "", ob.get("password") or ""
        pipe.write(b"\x05\x02\x00\x02" if user else b"\x05\x01\x00")
        ver, method = await pipe.readexactly(2)
        if method == 0x02:
            u, p = user.encode(), pw.encode()
            pipe.write(bytes([1, len(u)]) + u + bytes([len(p)]) + p)
            if (await pipe.readexactly(2))[1] != 0:
                raise UpstreamError("상위 SOCKS5 프록시 인증 실패", "auth")
        elif method != 0x00:
            raise UpstreamError("상위 SOCKS5 프록시가 인증 방식을 거부함", "auth")
        pipe.write(b"\x05\x01\x00" + encode_address(host, port))
        head = await pipe.readexactly(4)
        if head[1] != 0:
            raise UpstreamError(f"상위 SOCKS5 프록시 응답 코드 {head[1]}", _SOCKS5_REASONS.get(head[1], "general"))
        atyp = head[3]
        if atyp == ATYP_IPV4:
            await pipe.readexactly(6)
        elif atyp == ATYP_IPV6:
            await pipe.readexactly(18)
        elif atyp == ATYP_DOMAIN:
            n = (await pipe.readexactly(1))[0]
            await pipe.readexactly(n + 2)
        return pipe
    except BaseException:
        pipe.transport.abort()
        raise


class SSClientPipe(Pipe):
    """원격 Shadowsocks 서버와의 연결 (클라이언트 역할): 보낼 때 암호화, 받을 때 복호화."""

    def __init__(self, inst, enc, dec):
        super().__init__(inst)
        self.enc, self.dec = enc, dec

    def data_received(self, data):
        try:
            plain = self.dec.feed(data)
        except Exception as e:
            from .common import LOG_FAIL, log
            log(LOG_FAIL, f"Shadowsocks 터널 응답 복호화 실패 ({e}) - 비밀번호/방식 확인", self.inst)
            self.transport.abort()
            return
        if plain:
            super().data_received(plain)

    def send(self, data):
        self.transport.write(self.enc.encrypt(data))


async def _ss_connect(inst, ob, host, port):
    loop = asyncio.get_running_loop()
    method, key = inst.ss_out_key
    enc, dec = ssc.ss_client_codec(method, key, host, port)
    try:
        _, pipe = await loop.create_connection(lambda: SSClientPipe(inst, enc, dec), ob["host"], ob["port"],
                                               happy_eyeballs_delay=0.25)
    except OSError as e:
        raise UpstreamError(f"Shadowsocks 서버 {host_port(ob['host'], ob['port'])} 접속 실패: {e.strerror or e}",
                            "net") from e
    pipe.send(b"")      # 목적지 주소(헤더)를 바로 보낸다 - 서버가 먼저 말하는 프로토콜도 동작하도록
    return pipe


async def connect(inst, host, port):
    ob = inst.outbound
    t = ob["type"]
    if _is_self(inst, host, port):
        raise BlockedDestination("이 프록시 자신의 주소")
    if t == "direct":
        return await open_direct(inst, host, port)
    check_destination(inst, host)
    if t in ("http", "https"):
        return await _http_connect(inst, ob, host, port, tls=t == "https")
    if t == "socks4":
        return await _socks4_connect(inst, ob, host, port)
    if t == "socks5":
        return await _socks5_connect(inst, ob, host, port)
    if t == "shadowsocks":
        return await _ss_connect(inst, ob, host, port)
    if t == "hysteria2":
        from .hysteria2 import open_tcp
        return await open_tcp(inst, host, port)
    raise UpstreamError(f"알 수 없는 내보내기 방식: {t}")


# --- UDP 내보내기 ---

class SSEgress:
    """Shadowsocks 서버로 UDP 를 터널링."""

    def __init__(self, session):
        self.session = session
        self.inst = session.inst
        self.tr = None
        method, key = self.inst.ss_out_key
        self.codec = ssc.SSUDPClientCodec(method, key)

    async def start(self):
        ob = self.inst.outbound
        loop = asyncio.get_running_loop()
        self.tr, _ = await loop.create_datagram_endpoint(lambda: DatagramCallback(self._recv),
                                                         remote_addr=(ob["host"], ob["port"]))
        return self

    def close(self):
        if self.tr:
            self.tr.close()

    def send(self, atyp, host, port, payload):
        try:
            check_destination(self.inst, host)
            self.tr.sendto(self.codec.encode(encode_address(host, port) + payload))
            return True
        except (BlockedDestination, OSError, ValueError):
            self.session.dropped += 1
            return False

    def _recv(self, data, addr):
        try:
            plain = self.codec.decode(data)
            host, port, end = parse_address(plain, 0)
        except Exception:
            return
        self.session.deliver(host, port, plain[end:])


async def make_udp_egress(inst, session):
    t = inst.outbound["type"]
    if t == "direct":
        return await DirectEgress(session).start()
    if t == "shadowsocks":
        return await SSEgress(session).start()
    if t == "hysteria2":
        from .hysteria2 import Hy2Egress
        return await Hy2Egress(session).start()
    raise UpstreamError(f"{t} 상위 프록시로는 UDP 를 보낼 수 없습니다", "general")
