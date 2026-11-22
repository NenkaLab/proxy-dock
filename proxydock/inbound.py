"""Inbound: SOCKS5 / SOCKS4(4a) / HTTP / HTTPS / Shadowsocks server.

Each protocol works out the destination and hands it to relay_to(). relay_to() checks the allow list,
opens the destination connection through the outbound (outbound.connect), sends the protocol's reply (Reply)
and relays. The Hysteria2 inbound lives in hysteria2.py.
"""
from .i18n import _t
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
    """Inbound client connection. Applies the total/per-IP connection limits and IP bans, then runs handle()."""

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
            self.inst.log(LOG_ERROR, _t("{client} error: {e!r}").format(client=self.client, e=e))
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


# --- shared relay ---

class Reply:
    """Per-protocol success/failure reply. The default sends nothing (Shadowsocks, successful plain HTTP request)."""

    def ok(self, p, bind):
        pass

    def fail(self, p, reason):
        pass


async def relay_to(p, client, host, port, reply, first=b"", tag=""):
    """Allow list check -> outbound connection -> reply -> relay. first: data to send to the destination right after connecting."""
    inst = p.inst
    target = host_port(host, port)
    who = f"{tag}{client}"
    if not inst.allowed(host):
        reply.fail(p, "denied")
        inst.stats.blocked += 1
        inst.log(LOG_BLOCKED, _t("{who} -> {target} not in the allow list").format(who=who, target=target))
        return
    try:
        remote = await asyncio.wait_for(outbound.connect(inst, host, port), CONNECT_TIMEOUT)
    except BlockedDestination as e:
        # record the reason before replying: the connection check reads it as soon as the reply arrives
        inst.last_reason = "denied"
        inst.stats.blocked += 1
        inst.log(LOG_BLOCKED, f"{who} -> {target} {e}")
        reply.fail(p, "denied")
        return
    except (asyncio.TimeoutError, OSError, ValueError, asyncio.IncompleteReadError) as e:
        inst.last_reason = fail_reason(e)
        inst.stats.failed += 1
        inst.log(LOG_FAIL, f"{who} -> {target} {describe_error(e)}")
        reply.fail(p, inst.last_reason)
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
    reason, st = _t("error"), None
    try:
        reason, st = await run_relay(p, remote)
    finally:
        inst.stats.tcp_active -= 1
        if inst.cfg["log_close"] and st is not None:
            inst.log(LOG_TCP, _t("{who} -> {target} closed · {reason} · ↑ {fmt_size} ↓ {fmt_size2} · {value:.1f}s").format(who=who, target=target, reason=reason, fmt_size=fmt_size(st.up + len(first)), fmt_size2=fmt_size(st.down), value=time.monotonic() - started))


def log_udp_close(inst, who, session):
    if inst.cfg["log_close"] or inst.cfg["log_connections"]:
        extra = _t(" · dropped packets {dropped}").format(dropped=session.dropped) if session.dropped else ""
        inst.log(LOG_UDP, _t("{who} UDP session ended · ↑ {fmt_size} ↓ {fmt_size2} · {value:.1f}s{extra}").format(who=who, fmt_size=fmt_size(session.up), fmt_size2=fmt_size(session.down), value=time.monotonic() - session.started, extra=extra))


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
        if len(data) < 10 or data[2] != 0x00:  # drop FRAG != 0
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
            self.inst.log(LOG_WARN, _t("{client} unsupported SOCKS command 0x{cmd:02x} (BIND etc.)").format(client=self.client, cmd=cmd))

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
        self.inst.log(LOG_AUTH, _t("{client} authentication failed (user {decode!r})").format(client=self.client, decode=uname.decode(errors='replace')))
        await asyncio.sleep(0.5)    # slow down brute force
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
                self.inst.log(LOG_AUTH, _t("{client} tried to connect without authentication (client methods {list})").format(client=self.client, list=list(methods)))
                return None
            self.write(b"\x05\x02")
            if not await self._auth():
                return None
        else:
            if 0x00 not in methods:
                self.write(b"\x05\xff")
                self.inst.log(LOG_AUTH, _t("{client} unsupported authentication methods {list}").format(client=self.client, list=list(methods)))
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
        if inst.outbound["type"] not in ("direct", "shadowsocks", "hysteria2") or inst.outbound.get("via"):
            self.write(make_reply(REP_UNSUPPORTED))
            inst.log(LOG_WARN, _t("{client} UDP request refused - UDP cannot be sent via {outbound_label}").format(client=self.client, outbound_label=inst.outbound_label))
            return
        local_ip = self.transport.get_extra_info("sockname")[0]
        family = socket.AF_INET6 if ":" in local_ip else socket.AF_INET
        session = Socks5UDPSession(inst, self.ip)
        try:
            relay_port = await session.start(local_ip, family)
        except (OSError, UpstreamError) as e:
            session.close()
            inst.log(LOG_ERROR, _t("{client} could not start UDP relay: {describe_error}").format(client=self.client, describe_error=describe_error(e)))
            self.write(make_reply(REP_GENERAL))
            return
        # the UDP session lives until the TCP control connection closes (no idle timeout)
        self.discard = True
        self.buf.clear()
        self.transport.resume_reading()
        enable_keepalive(self.transport)
        self.write(make_reply(REP_SUCCESS, local_ip, relay_port))
        inst.stats.total += 1
        inst.stats.udp_active += 1
        inst.log_conn(LOG_UDP, _t("{client} UDP session started (relay port {relay_port})").format(client=self.client, relay_port=relay_port))
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
            await self.readuntil(b"\0", 256)           # USERID (SOCKS4 has no password)
            ip = head[4:8]
            if ip[:3] == b"\0\0\0" and ip[3] != 0:      # SOCKS4a: the domain follows
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
            self.inst.log(LOG_WARN, _t("{client} unsupported SOCKS4 command {cd} (BIND)").format(client=self.client, cd=cd))
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
    _resp = None        # plain request: response header not fully received yet (None = pass through)

    def send(self, data):
        """Destination response to the client. For plain requests, rewrite Connection/Keep-Alive in the response header
        to close (only one request per connection is handled, so the client must not reuse it even if the server says keep-alive)."""
        if self._resp is None:
            return super().send(data)
        self._resp += data
        end = self._resp.find(b"\r\n\r\n")
        if end < 0:
            if len(self._resp) > 64 * 1024:     # abnormally long header: pass it on untouched
                data, self._resp = bytes(self._resp), None
                super().send(data)
            return
        lines = bytes(self._resp[:end]).split(b"\r\n")
        rest = bytes(self._resp[end + 4:])
        self._resp = None
        parts = lines[0].split(b" ", 2)
        code = int(parts[1]) if len(parts) > 1 and parts[1].isdigit() else 0
        if 100 <= code < 200 and code != 101:   # interim response such as 100 Continue: rewrite the next header again
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
        # plain request: GET http://host[:port]/path -> forwarded in origin-form (one request per connection)
        url = urllib.parse.urlsplit(target)
        if url.scheme.lower() != "http" or not url.hostname:
            self.write(http_error(400, "Bad Request"))
            self.inst.log(LOG_WARN, _t("{client} not a proxy request ({method} {target})").format(client=self.client, method=method, target=target[:80]))
            return
        host, port = url.hostname, url.port or 80
        path = urllib.parse.urlunsplit(("", "", url.path or "/", url.query, ""))
        upgrade = any(h.lower().startswith("upgrade:") for h in headers)
        # headers named in the Connection header are hop-by-hop too, so remove them
        listed = set()
        for h in headers:
            name, _, value = h.partition(":")
            if name.strip().lower() == "connection":
                listed |= {v.strip().lower() for v in value.split(",") if v.strip()}
        listed -= {"upgrade"}
        # RFC 9112 3.2.2: for an absolute-form request, replace Host with that authority (allow list matches the real target)
        authority = host_port(host, port) if url.port else (f"[{host}]" if ":" in host else host)
        out = [f"{method} {path} {version}", f"Host: {authority}"]
        for h in headers:
            name = h.split(":", 1)[0].strip().lower()
            if (name in _HOP_BY_HOP or name in listed or name == "host") and not (upgrade and name == "connection"):
                continue
            out.append(h)
        if not upgrade:
            out.append("Connection: close")
            self._resp = bytearray()    # also rewrite the response header to Connection: close
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
            self.inst.log(LOG_AUTH, _t("{client} authentication failed (user {user!r})").format(client=self.client, user=user))
        self.write(http_error(407, "Proxy Authentication Required",
                              'Proxy-Authenticate: Basic realm="ProxyDock"\r\n'))
        return False


# --- Shadowsocks server ---

class SSServerPipe(ClientPipe):
    """Shadowsocks TCP client connection. Received ciphertext is decrypted into the plaintext pipe; sends are encrypted."""

    def __init__(self, inst):
        super().__init__(inst)
        method, key = inst.ss_in_key
        self.v2022 = ssc.SS_METHODS[method][2]
        self.dec = (ssc.SS22Decryptor if self.v2022 else ssc.SSDecryptor)(method, key)
        self.enc = None             # the 2022 response needs the request salt, so it is created on the first send
        self.bad = False

    def data_received(self, data):
        if self.bad:
            return
        try:
            plain = self.dec.feed(data)
        except (InvalidTag, ValueError, struct.error) as e:
            # anti-probing: do not close right away; stay silent until the handshake timeout closes it
            self.bad = True
            self.discard = True
            self.buf.clear()
            self.inst.stats.blocked += 1
            note_auth_failure(self.ip, self.inst)
            reason = str(e) if isinstance(e, ValueError) else _t("password/cipher mismatch")
            self.inst.log(LOG_AUTH, _t("SS {client} decryption failed - {reason}").format(client=self.client, reason=reason))
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
    """Shadowsocks UDP session (per client address, or per 2022 session id)."""

    def __init__(self, server, client_addr, key=None):
        super().__init__(server.inst, client_addr[0])
        self.server = server
        self.client_addr = client_addr
        self.key = key              # 2022: client session id
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
            self.inst.log(LOG_ERROR, _t("Could not start SS UDP outbound: {describe_error}").format(describe_error=describe_error(e)))
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
        return make_udp_header(host, port)[3:]   # SS has only the address, without RSV/FRAG

    def _reply(self, hdr, data):
        self.server.transport.sendto(self.server.encode_reply(self, hdr + data), self.client_addr)


class SSUDPServer(asyncio.DatagramProtocol):
    """Shadowsocks UDP server. AEAD-2018 sessions are per client address, 2022 sessions per client session id."""

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
        sess.client_addr = addr     # reply to the latest address after NAT rebinding
        return sess, body[11 + pad:]

    def _new_session(self, key, addr):
        if len(self.sessions) >= ssc.SS_MAX_UDP_SESSIONS:
            return None
        sess = self.sessions[key] = SSUDPSession(self, addr, key)
        self.inst.stats.udp_active += 1
        self.inst.stats.total += 1
        self.inst.log_conn(LOG_UDP, _t("SS {host_port} UDP session started").format(host_port=host_port(addr[0], addr[1])))
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


# --- server startup ---

PIPES = {"socks5": Socks5Pipe, "socks4": Socks4Pipe, "http": HttpPipe, "https": HttpPipe,
         "shadowsocks": SSServerPipe}


async def start_servers(inst):
    """Open the inbound servers. Returns what to close later (asyncio.Server / transports)."""
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
            inst.log(LOG_WARN, _t("Could not open Shadowsocks UDP (TCP only): {e}").format(e=e))
    return out
