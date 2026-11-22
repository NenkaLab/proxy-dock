"""받는 쪽: SOCKS5 / SOCKS4(4a) / HTTP / HTTPS / Shadowsocks 서버.

각 프로토콜은 목적지를 알아낸 뒤 relay_to() 로 넘긴다. relay_to() 는 허용 목록을 확인하고
내보내는 쪽(outbound.connect)으로 목적지 연결을 만든 다음, 프로토콜별 응답(Reply)을 보내고 중계한다.
Hysteria2 받기는 hysteria2.py 에 있다.
"""
import asyncio
import base64
import hmac
import os
import socket
import struct
import time
import urllib.parse

from cryptography.exceptions import InvalidTag

from .common import (
    ATYP_DOMAIN, ATYP_IPV4, ATYP_IPV6, CMD_CONNECT, CMD_UDP_ASSOCIATE, CONNECT_TIMEOUT, HANDSHAKE_TIMEOUT,
    LOG_AUTH, LOG_BLOCKED, LOG_ERROR, LOG_FAIL, LOG_TCP, LOG_UDP, LOG_WARN, MAX_CONN, MAX_CONN_PER_IP,
    REP_ATYP_UNSUPPORTED, REP_DENIED, REP_GENERAL, REP_HOST_UNREACH, REP_NET_UNREACH, REP_REFUSED,
    REP_SUCCESS, REP_UNSUPPORTED, SOCKS5_VER, BlockedDestination, UpstreamError, describe_error,
    fail_reason, fmt_size, host_port, is_banned, is_loopback_ip, make_reply, make_udp_header,
    note_auth_failure, parse_address, per_ip, split_host_port, strip_scope, valid_domain,
)
from .relay import DatagramCallback, Pipe, UDPSession, disable_udp_connreset, enable_keepalive, \
    make_udp_socket, run_relay
from . import outbound
from . import ss as ssc

_clients = 0
_tasks = set()


class ClientPipe(Pipe):
    """받는 쪽 클라이언트 연결. 전체/IP 별 연결 상한과 차단 IP 를 적용하고 handle() 을 실행한다."""

    def __init__(self, inst):
        super().__init__(inst, upstream=True)
        self.counted = False
        self.ip = ""

    def connection_made(self, transport):
        global _clients
        super().connection_made(transport)
        self.ip = strip_scope((transport.get_extra_info("peername") or ("",))[0])
        local = is_loopback_ip(self.ip)
        if (_clients >= MAX_CONN or is_banned(self.ip)
                or (not local and per_ip.get(self.ip, 0) >= MAX_CONN_PER_IP)):
            transport.abort()
            return
        _clients += 1
        per_ip[self.ip] = per_ip.get(self.ip, 0) + 1
        self.counted = True
        self.inst.conns.add(self)
        task = asyncio.get_running_loop().create_task(self._run())
        _tasks.add(task)
        task.add_done_callback(_tasks.discard)

    @property
    def client(self):
        peer = self.transport.get_extra_info("peername") or ("?", 0)
        return host_port(strip_scope(peer[0]), peer[1])

    async def _run(self):
        try:
            await self.handle()
        except (asyncio.IncompleteReadError, asyncio.TimeoutError, ConnectionError, asyncio.CancelledError):
            pass
        except Exception as e:
            self.inst.log(LOG_ERROR, f"{self.client} 처리 중 오류: {e!r}")
        finally:
            self.transport.close()

    async def handle(self):
        raise NotImplementedError

    def connection_lost(self, exc):
        global _clients
        super().connection_lost(exc)
        self.inst.conns.discard(self)
        if self.counted:
            self.counted = False
            _clients -= 1
            n = per_ip.get(self.ip, 1) - 1
            if n > 0:
                per_ip[self.ip] = n
            else:
                per_ip.pop(self.ip, None)


# --- 공통 중계 ---

class Reply:
    """프로토콜별 성공/실패 응답. 기본은 응답 없음 (Shadowsocks, HTTP 일반 요청 성공)."""

    def ok(self, p, bind):
        pass

    def fail(self, p, reason):
        pass


async def relay_to(p, client, host, port, reply, first=b"", tag=""):
    """허용 목록 확인 -> 내보내는 쪽 연결 -> 응답 -> 중계. first: 연결 직후 목적지에 먼저 보낼 데이터."""
    inst = p.inst
    target = host_port(host, port)
    who = f"{tag}{client}"
    if not inst.allowed(host):
        reply.fail(p, "denied")
        inst.stats.blocked += 1
        inst.log(LOG_BLOCKED, f"{who} -> {target} 허용 목록에 없음")
        return
    try:
        remote = await asyncio.wait_for(outbound.connect(inst, host, port), CONNECT_TIMEOUT)
    except BlockedDestination as e:
        reply.fail(p, "denied")
        inst.stats.blocked += 1
        inst.log(LOG_BLOCKED, f"{who} -> {target} {e}")
        return
    except (asyncio.TimeoutError, OSError, ValueError, asyncio.IncompleteReadError) as e:
        reply.fail(p, fail_reason(e))
        inst.stats.failed += 1
        inst.log(LOG_FAIL, f"{who} -> {target} {describe_error(e)}")
        return
    if p.closed:
        remote.transport.close()
        return
    sock = remote.transport.get_extra_info("sockname") or ("0.0.0.0", 0)
    peer = remote.transport.get_extra_info("peername") or ("?",)
    reply.ok(p, sock[:2])
    if first:
        remote.write(first)
        inst.stats.up += len(first)
    inst.stats.total += 1
    inst.stats.tcp_active += 1
    ob = inst.outbound["type"]
    via = f" ({strip_scope(peer[0])})" if ob == "direct" and peer[0] != host else \
        (f" via {inst.outbound_label}" if ob != "direct" else "")
    inst.log_conn(LOG_TCP, f"{who} -> {target}{via}")
    started = time.monotonic()
    reason, st = "오류", None
    try:
        reason, st = await run_relay(p, remote)
    finally:
        inst.stats.tcp_active -= 1
        if inst.cfg["log_close"] and st is not None:
            inst.log(LOG_TCP, f"{who} -> {target} 종료 · {reason} · ↑ {fmt_size(st.up + len(first))} "
                              f"↓ {fmt_size(st.down)} · {time.monotonic() - started:.1f}s")


def log_udp_close(inst, who, session):
    if inst.cfg["log_close"] or inst.cfg["log_connections"]:
        extra = f" · 버린 패킷 {session.dropped}" if session.dropped else ""
        inst.log(LOG_UDP, f"{who} UDP 세션 종료 · ↑ {fmt_size(session.up)} ↓ {fmt_size(session.down)} · "
                          f"{time.monotonic() - session.started:.1f}s{extra}")


# --- SOCKS5 ---

_SOCKS5_CODES = {"denied": REP_DENIED, "refused": REP_REFUSED, "net": REP_NET_UNREACH,
                 "host": REP_HOST_UNREACH, "timeout": REP_HOST_UNREACH, "auth": REP_DENIED,
                 "general": REP_GENERAL}


class Socks5Reply(Reply):
    def ok(self, p, bind):
        p.write(make_reply(REP_SUCCESS, bind[0], bind[1]))

    def fail(self, p, reason):
        p.write(make_reply(_SOCKS5_CODES.get(reason, REP_GENERAL)))


class Socks5UDPSession(UDPSession):
    async def start(self, local_ip, family):
        loop = asyncio.get_running_loop()
        sock = make_udp_socket(family, local_ip)
        self.client_tr, _ = await loop.create_datagram_endpoint(lambda: DatagramCallback(self.from_client),
                                                                sock=sock)
        await self.open_egress()
        return sock.getsockname()[1]

    def close(self):
        super().close()
        if getattr(self, "client_tr", None):
            self.client_tr.close()

    def from_client(self, data, addr):
        if self.closed or strip_scope(addr[0]) != self.client_ip:
            return
        if self.client_addr is None:
            self.client_addr = addr[:2]
        elif addr[:2] != self.client_addr:
            return
        if len(data) < 10 or data[2] != 0x00:  # FRAG != 0 은 버린다
            return
        try:
            dst_addr, dst_port, header_end = parse_address(data, 3)
        except ValueError:
            return
        self.dispatch(data[3], dst_addr, dst_port, data[header_end:])

    def _reply(self, hdr, data):
        self.client_tr.sendto(hdr + data, self.client_addr)


class Socks5Pipe(ClientPipe):
    async def handle(self):
        req = await asyncio.wait_for(self._negotiate(), HANDSHAKE_TIMEOUT)
        if req is None:
            return
        cmd, host, port = req
        if cmd == CMD_CONNECT:
            await relay_to(self, self.client, host, port, Socks5Reply())
        elif cmd == CMD_UDP_ASSOCIATE:
            await self._udp()
        else:
            self.write(make_reply(REP_UNSUPPORTED))
            self.inst.log(LOG_WARN, f"{self.client} 지원하지 않는 SOCKS 명령 0x{cmd:02x} (BIND 등)")

    async def _auth(self):
        ib = self.inst.inbound
        ver, ulen = await self.readexactly(2)
        uname = await self.readexactly(ulen)
        (plen,) = await self.readexactly(1)
        passwd = await self.readexactly(plen)
        ok_user = hmac.compare_digest(uname, ib.get("username", "").encode())
        ok_pass = hmac.compare_digest(passwd, ib.get("password", "").encode())
        if ver == 0x01 and ok_user and ok_pass:
            self.write(b"\x01\x00")
            return True
        self.inst.stats.blocked += 1
        note_auth_failure(self.ip, self.inst)
        self.inst.log(LOG_AUTH, f"{self.client} 인증 실패 (아이디 {uname.decode(errors='replace')!r})")
        await asyncio.sleep(0.5)    # 무차별 대입 지연
        self.write(b"\x01\x01")
        return False

    async def _negotiate(self):
        ver, nmethods = await self.readexactly(2)
        if ver != SOCKS5_VER:
            return None
        methods = await self.readexactly(nmethods)
        if self.inst.inbound.get("auth_enabled"):
            if 0x02 not in methods:
                self.write(b"\x05\xff")
                self.inst.stats.blocked += 1
                self.inst.log(LOG_AUTH, f"{self.client} 인증 없이 접속 시도 (클라이언트 인증 방식 {list(methods)})")
                return None
            self.write(b"\x05\x02")
            if not await self._auth():
                return None
        else:
            if 0x00 not in methods:
                self.write(b"\x05\xff")
                self.inst.log(LOG_AUTH, f"{self.client} 지원하지 않는 인증 방식 {list(methods)}")
                return None
            self.write(b"\x05\x00")
        head = await self.readexactly(4)
        ver, cmd, _, atyp = head
        if ver != SOCKS5_VER:
            return None
        if atyp == ATYP_IPV4:
            rest = await self.readexactly(6)
        elif atyp == ATYP_IPV6:
            rest = await self.readexactly(18)
        elif atyp == ATYP_DOMAIN:
            ln = await self.readexactly(1)
            rest = ln + await self.readexactly(ln[0] + 2)
        else:
            self.write(make_reply(REP_ATYP_UNSUPPORTED))
            return None
        try:
            host, port, _ = parse_address(head + rest, 3)
        except ValueError:
            self.write(make_reply(REP_GENERAL))
            return None
        return cmd, host, port

    async def _udp(self):
        inst = self.inst
        if inst.outbound["type"] not in ("direct", "shadowsocks", "hysteria2"):
            self.write(make_reply(REP_UNSUPPORTED))
            inst.log(LOG_WARN, f"{self.client} UDP 요청 거부 - {inst.outbound_label} 로는 UDP 를 보낼 수 없음")
            return
        local_ip = self.transport.get_extra_info("sockname")[0]
        family = socket.AF_INET6 if ":" in local_ip else socket.AF_INET
        session = Socks5UDPSession(inst, self.ip)
        try:
            relay_port = await session.start(local_ip, family)
        except (OSError, UpstreamError) as e:
            session.close()
            inst.log(LOG_ERROR, f"{self.client} UDP 릴레이 시작 실패: {describe_error(e)}")
            self.write(make_reply(REP_GENERAL))
            return
        # UDP 세션은 TCP 제어 연결이 닫힐 때까지 유지 (유휴 타임아웃 없음)
        self.discard = True
        self.buf.clear()
        self.transport.resume_reading()
        enable_keepalive(self.transport)
        self.write(make_reply(REP_SUCCESS, local_ip, relay_port))
        inst.stats.total += 1
        inst.stats.udp_active += 1
        inst.log_conn(LOG_UDP, f"{self.client} UDP 세션 시작 (릴레이 포트 {relay_port})")
        try:
            if not self.eof:
                await self.lost
        finally:
            session.close()
            inst.stats.udp_active -= 1
            log_udp_close(inst, self.client, session)


# --- SOCKS4 / 4a ---

class Socks4Reply(Reply):
    def ok(self, p, bind):
        p.write(b"\x00\x5a" + b"\0" * 6)

    def fail(self, p, reason):
        p.write(b"\x00\x5b" + b"\0" * 6)


class Socks4Pipe(ClientPipe):
    async def handle(self):
        async def negotiate():
            head = await self.readexactly(8)
            vn, cd, port = head[0], head[1], struct.unpack("!H", head[2:4])[0]
            if vn != 4:
                return None
            await self.readuntil(b"\0", 256)           # USERID (SOCKS4 는 비밀번호가 없다)
            ip = head[4:8]
            if ip[:3] == b"\0\0\0" and ip[3] != 0:      # SOCKS4a: 도메인이 뒤에 온다
                raw = (await self.readuntil(b"\0", 256))[:-1]
                try:
                    host = raw.decode("ascii")
                except UnicodeDecodeError:
                    return None
                if not valid_domain(host):
                    return None
            else:
                host = socket.inet_ntoa(ip)
            return cd, host, port

        req = await asyncio.wait_for(negotiate(), HANDSHAKE_TIMEOUT)
        if req is None:
            return
        cd, host, port = req
        if cd != CMD_CONNECT:
            Socks4Reply().fail(self, "general")
            self.inst.log(LOG_WARN, f"{self.client} 지원하지 않는 SOCKS4 명령 {cd} (BIND)")
            return
        await relay_to(self, self.client, host, port, Socks4Reply())


# --- HTTP / HTTPS ---

_HTTP_STATUS = {"denied": (403, "Forbidden"), "auth": (502, "Bad Gateway"), "refused": (502, "Bad Gateway"),
                "net": (502, "Bad Gateway"), "host": (502, "Bad Gateway"), "timeout": (504, "Gateway Timeout"),
                "general": (502, "Bad Gateway")}
_HOP_BY_HOP = {"proxy-authorization", "proxy-connection", "connection", "keep-alive", "te", "trailer",
               "transfer-encoding-proxy", "proxy-authenticate"}


def http_error(code, reason, extra_headers=""):
    body = f"{code} {reason}\n".encode()
    return (f"HTTP/1.1 {code} {reason}\r\nContent-Type: text/plain\r\nContent-Length: {len(body)}\r\n"
            f"Connection: close\r\n{extra_headers}\r\n").encode() + body


class HttpReply(Reply):
    def __init__(self, connect):
        self.connect = connect

    def ok(self, p, bind):
        if self.connect:
            p.write(b"HTTP/1.1 200 Connection established\r\n\r\n")

    def fail(self, p, reason):
        code, text = _HTTP_STATUS.get(reason, (502, "Bad Gateway"))
        p.write(http_error(code, text))


class HttpPipe(ClientPipe):
    _resp = None        # 일반 요청: 아직 다 받지 못한 응답 헤더 (None = 그대로 전달)

    def send(self, data):
        """목적지 응답을 클라이언트로. 일반 요청이면 응답 헤더의 Connection/Keep-Alive 를 close 로 바꾼다
        (연결당 요청 하나만 처리하므로, 서버가 keep-alive 라고 해도 클라이언트가 재사용하지 않도록)."""
        if self._resp is None:
            return super().send(data)
        self._resp += data
        end = self._resp.find(b"\r\n\r\n")
        if end < 0:
            if len(self._resp) > 64 * 1024:     # 헤더가 비정상적으로 김: 손대지 않고 넘긴다
                data, self._resp = bytes(self._resp), None
                super().send(data)
            return
        lines = bytes(self._resp[:end]).split(b"\r\n")
        rest = bytes(self._resp[end + 4:])
        self._resp = None
        parts = lines[0].split(b" ", 2)
        code = int(parts[1]) if len(parts) > 1 and parts[1].isdigit() else 0
        if 100 <= code < 200 and code != 101:   # 100 Continue 등 중간 응답: 다음 헤더를 다시 고친다
            super().send(b"\r\n".join(lines) + b"\r\n\r\n")
            self._resp = bytearray()
            if rest:
                self.send(rest)
            return
        if code != 101:
            lines = [lines[0]] + [ln for ln in lines[1:] if ln.split(b":", 1)[0].strip().lower()
                                  not in (b"connection", b"keep-alive", b"proxy-connection")]
            lines.append(b"Connection: close")
        super().send(b"\r\n".join(lines) + b"\r\n\r\n" + rest)

    async def handle(self):
        head = await asyncio.wait_for(self.readuntil(b"\r\n\r\n", 64 * 1024), HANDSHAKE_TIMEOUT)
        lines = head.decode("latin-1").split("\r\n")
        try:
            method, target, version = lines[0].split(" ", 2)
        except ValueError:
            self.write(http_error(400, "Bad Request"))
            return
        headers = [h for h in lines[1:] if h]
        if not self._check_auth(headers):
            return
        if method.upper() == "CONNECT":
            try:
                host, port = split_host_port(target, 443)
            except ValueError:
                self.write(http_error(400, "Bad Request"))
                return
            await relay_to(self, self.client, host, port, HttpReply(True))
            return
        # 일반 요청: GET http://host[:port]/path -> origin-form 으로 바꿔 전달 (연결당 요청 하나)
        url = urllib.parse.urlsplit(target)
        if url.scheme.lower() != "http" or not url.hostname:
            self.write(http_error(400, "Bad Request"))
            self.inst.log(LOG_WARN, f"{self.client} 프록시 요청이 아님 ({method} {target[:80]})")
            return
        host, port = url.hostname, url.port or 80
        path = urllib.parse.urlunsplit(("", "", url.path or "/", url.query, ""))
        upgrade = any(h.lower().startswith("upgrade:") for h in headers)
        # Connection 헤더에 이름이 나온 헤더도 이 구간에서만 쓰는 헤더(hop-by-hop)라 지운다
        listed = set()
        for h in headers:
            name, _, value = h.partition(":")
            if name.strip().lower() == "connection":
                listed |= {v.strip().lower() for v in value.split(",") if v.strip()}
        listed -= {"upgrade"}
        # RFC 9112 3.2.2: 절대 주소로 받은 요청은 Host 헤더를 그 주소로 바꾼다 (허용 목록과 실제 대상 일치)
        authority = host_port(host, port) if url.port else (f"[{host}]" if ":" in host else host)
        out = [f"{method} {path} {version}", f"Host: {authority}"]
        for h in headers:
            name = h.split(":", 1)[0].strip().lower()
            if (name in _HOP_BY_HOP or name in listed or name == "host") and not (upgrade and name == "connection"):
                continue
            out.append(h)
        if not upgrade:
            out.append("Connection: close")
            self._resp = bytearray()    # 응답 헤더도 Connection: close 로 고쳐서 보낸다
        await relay_to(self, self.client, host, port, HttpReply(False),
                       first=("\r\n".join(out) + "\r\n\r\n").encode("latin-1"))

    def _check_auth(self, headers):
        ib = self.inst.inbound
        if not ib.get("auth_enabled"):
            return True
        cred = None
        for h in headers:
            name, _, value = h.partition(":")
            if name.strip().lower() == "proxy-authorization":
                scheme, _, token = value.strip().partition(" ")
                if scheme.lower() == "basic":
                    try:
                        cred = base64.b64decode(token.strip()).decode("utf-8", errors="replace")
                    except ValueError:
                        cred = ""
        if cred is not None:
            user, _, pw = cred.partition(":")
            if hmac.compare_digest(user.encode(), ib.get("username", "").encode()) & \
                    hmac.compare_digest(pw.encode(), ib.get("password", "").encode()):
                return True
            self.inst.stats.blocked += 1
            note_auth_failure(self.ip, self.inst)
            self.inst.log(LOG_AUTH, f"{self.client} 인증 실패 (아이디 {user!r})")
        self.write(http_error(407, "Proxy Authentication Required",
                              'Proxy-Authenticate: Basic realm="ProxyDock"\r\n'))
        return False


# --- Shadowsocks 서버 ---

class SSServerPipe(ClientPipe):
    """Shadowsocks TCP 클라이언트 연결. 받은 암호문은 풀어서 평문 파이프로, 보낼 때는 암호화."""

    def __init__(self, inst):
        super().__init__(inst)
        method, key = inst.ss_in_key
        self.v2022 = ssc.SS_METHODS[method][2]
        self.dec = (ssc.SS22Decryptor if self.v2022 else ssc.SSDecryptor)(method, key)
        self.enc = None             # 2022 응답은 요청 salt 가 필요하므로 첫 전송 때 만든다
        self.bad = False

    def data_received(self, data):
        if self.bad:
            return
        try:
            plain = self.dec.feed(data)
        except (InvalidTag, ValueError, struct.error) as e:
            # 탐지 방지: 바로 끊지 않고 응답 없이 두었다가 협상 시간 초과로 닫는다
            self.bad = True
            self.discard = True
            self.buf.clear()
            self.inst.stats.blocked += 1
            note_auth_failure(self.ip, self.inst)
            reason = str(e) if isinstance(e, ValueError) else "비밀번호/암호화 방식 불일치"
            self.inst.log(LOG_AUTH, f"SS {self.client} 복호화 실패 - {reason}")
            return
        if plain:
            Pipe.data_received(self, plain)

    def send(self, data):
        if self.enc is None:
            method, key = self.inst.ss_in_key
            self.enc = ssc.SS22Encryptor(method, key, self.dec.salt) if self.v2022 else ssc.SSEncryptor(method, key)
        self.transport.write(self.enc.encrypt(data))

    async def handle(self):
        async def read_target():
            atyp = (await self.readexactly(1))[0]
            if atyp == ATYP_IPV4:
                rest = await self.readexactly(6)
            elif atyp == ATYP_IPV6:
                rest = await self.readexactly(18)
            elif atyp == ATYP_DOMAIN:
                ln = await self.readexactly(1)
                rest = ln + await self.readexactly(ln[0] + 2)
            else:
                raise ValueError(f"unknown ATYP {atyp}")
            addr, port, _ = parse_address(bytes([atyp]) + rest, 0)
            return addr, port

        try:
            host, port = await asyncio.wait_for(read_target(), HANDSHAKE_TIMEOUT)
        except ValueError:
            return
        await relay_to(self, self.client, host, port, Reply(), tag="SS ")


class SSUDPSession(UDPSession):
    """Shadowsocks UDP 세션 (클라이언트 주소 또는 2022 세션 ID 별)."""

    def __init__(self, server, client_addr, key=None):
        super().__init__(server.inst, client_addr[0])
        self.server = server
        self.client_addr = client_addr
        self.key = key              # 2022: 클라이언트 세션 ID
        self.ready = False
        self.backlog = []
        self.last = time.monotonic()
        self.server_sid = os.urandom(8)
        self.server_pid = 0
        self.client_aead = None
        self.server_aead = None
        self.window = ssc._ReplayWindow()

    async def open(self):
        try:
            await self.open_egress()
        except (OSError, UpstreamError) as e:
            self.inst.log(LOG_ERROR, f"SS UDP 내보내기 시작 실패: {describe_error(e)}")
            return
        self.ready = True
        for item in self.backlog:
            self.dispatch(*item)
        self.backlog = []

    def feed(self, addr_and_payload):
        self.last = time.monotonic()
        try:
            host, port, end = parse_address(addr_and_payload, 0)
        except ValueError:
            return
        item = (addr_and_payload[0], host, port, addr_and_payload[end:])
        if self.ready:
            self.dispatch(*item)
        elif len(self.backlog) < 64:
            self.backlog.append(item)

    def deliver(self, src_host, src_port, data):
        self.last = time.monotonic()
        super().deliver(src_host, src_port, data)

    def _make_hdr(self, host, port):
        return make_udp_header(host, port)[3:]   # SS 는 RSV/FRAG 없이 주소만

    def _reply(self, hdr, data):
        self.server.transport.sendto(self.server.encode_reply(self, hdr + data), self.client_addr)


class SSUDPServer(asyncio.DatagramProtocol):
    """Shadowsocks UDP 서버. AEAD-2018 은 클라이언트 주소별, 2022 는 클라이언트 세션 ID 별 세션."""

    def __init__(self, inst):
        self.inst = inst
        self.transport = None
        self.sessions = {}
        self.reaper = None
        self.method, self.key = inst.ss_in_key
        self.key_len, self.cls, self.v2022 = ssc.SS_METHODS[self.method]
        self.ecb = None
        if self.v2022 and self.cls is ssc.AESGCM:
            self.ecb = ssc.Cipher(ssc.algorithms.AES(self.key), ssc.modes.ECB())

    def connection_made(self, transport):
        self.transport = transport
        try:
            disable_udp_connreset(transport.get_extra_info("socket"))
        except Exception:
            pass
        self.reaper = asyncio.get_running_loop().create_task(self._reap())

    def connection_lost(self, exc):
        if self.reaper:
            self.reaper.cancel()
        for key, sess in list(self.sessions.items()):
            self._close(key, sess)

    def _subkey_aead(self, sid):
        return self.cls(ssc.blake3_derive_key("shadowsocks 2022 session subkey", self.key + sid, self.key_len))

    def _decode22(self, data, addr):
        if self.ecb is not None:
            if len(data) < 16 + ssc.SS_TAG + 11:
                raise ValueError("too short")
            d = self.ecb.decryptor()
            hdr = d.update(data[:16]) + d.finalize()
            sid, pid = hdr[:8], struct.unpack("!Q", hdr[8:])[0]
            sess = self.sessions.get(sid)
            aead = sess.client_aead if sess else self._subkey_aead(sid)
            body = aead.decrypt(hdr[4:16], data[16:], None)
        else:
            if len(data) < 24 + 16 + 11 + ssc.SS_TAG:
                raise ValueError("too short")
            plain = ssc.xchacha_decrypt(self.key, data[:24], data[24:])
            sid, pid, body = plain[:8], struct.unpack("!Q", plain[8:16])[0], plain[16:]
            sess = self.sessions.get(sid)
            aead = None
        typ, ts, pad = struct.unpack("!BQH", body[:11])
        if typ != 0:
            raise ValueError("bad type")
        ssc._check_ss22_time(ts)
        if pad > ssc.SS_MAX_PADDING or len(body) < 11 + pad:
            raise ValueError("bad padding")
        if sess is None:
            sess = self._new_session(sid, addr)
            if sess is None:
                raise ValueError("too many sessions")
            sess.client_aead = aead
            if self.ecb is not None:
                sess.server_aead = self._subkey_aead(sess.server_sid)
        if not sess.window.check(pid):
            raise ValueError("replayed packet")
        sess.client_addr = addr     # NAT 재바인딩 시 최신 주소로 응답
        return sess, body[11 + pad:]

    def _new_session(self, key, addr):
        if len(self.sessions) >= ssc.SS_MAX_UDP_SESSIONS:
            return None
        sess = self.sessions[key] = SSUDPSession(self, addr, key)
        self.inst.stats.udp_active += 1
        self.inst.stats.total += 1
        self.inst.log_conn(LOG_UDP, f"SS {host_port(addr[0], addr[1])} UDP 세션 시작")
        sess.spawn(sess.open())
        return sess

    def encode_reply(self, sess, addr_and_payload):
        if not self.v2022:
            return ssc.ss_udp_encrypt(self.method, self.key, addr_and_payload)
        body = struct.pack("!BQ", 1, int(time.time())) + sess.key + b"\0\0" + addr_and_payload
        hdr = sess.server_sid + struct.pack("!Q", sess.server_pid)
        sess.server_pid += 1
        if self.ecb is not None:
            e = self.ecb.encryptor()
            return e.update(hdr) + e.finalize() + sess.server_aead.encrypt(hdr[4:16], body, None)
        nonce = os.urandom(24)
        return nonce + ssc.xchacha_encrypt(self.key, nonce, hdr + body)

    def datagram_received(self, data, addr):
        try:
            if self.v2022:
                sess, plain = self._decode22(data, addr)
            else:
                plain = ssc.ss_udp_decrypt(self.method, self.key, data)
                sess = self.sessions.get(addr) or self._new_session(addr, addr)
                if sess is None:
                    return
        except (InvalidTag, ValueError, struct.error):
            return
        sess.feed(plain)

    def error_received(self, exc):
        pass

    def _close(self, key, sess):
        if self.sessions.pop(key, None) is None:
            return
        sess.close()
        self.inst.stats.udp_active -= 1
        a = sess.client_addr
        log_udp_close(self.inst, f"SS {host_port(a[0], a[1])}", sess)

    async def _reap(self):
        while True:
            await asyncio.sleep(15)
            now = time.monotonic()
            for key, sess in list(self.sessions.items()):
                if now - sess.last > ssc.SS_UDP_IDLE:
                    self._close(key, sess)


# --- 서버 시작 ---

PIPES = {"socks5": Socks5Pipe, "socks4": Socks4Pipe, "http": HttpPipe, "https": HttpPipe,
         "shadowsocks": SSServerPipe}


async def start_servers(inst):
    """받는 쪽 서버들을 연다. 닫을 대상 목록(asyncio.Server / 트랜스포트)을 돌려준다."""
    loop = asyncio.get_running_loop()
    ib = inst.inbound
    t = ib["type"]
    host, port = ib["host"], ib["port"]
    if t == "hysteria2":
        from .hysteria2 import start_server
        return [await start_server(inst)]
    kw = {}
    if t == "https":
        kw["ssl"] = inst.server_ssl_context()
    cls = PIPES[t]
    srv = await loop.create_server(lambda: cls(inst), host, port, backlog=1024, **kw)
    out = [srv]
    if t == "shadowsocks":
        try:
            tr, _ = await loop.create_datagram_endpoint(lambda: SSUDPServer(inst), local_addr=(host, port))
            out.append(tr)
        except OSError as e:
            inst.log(LOG_WARN, f"Shadowsocks UDP 를 열지 못했습니다 (TCP 만 동작): {e}")
    return out
