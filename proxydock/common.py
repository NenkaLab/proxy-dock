"""Shared: constants, logging, address handling, IP bans. No Qt dependency (used by the proxy thread and tests)."""
from .i18n import _t, on_language_change
import asyncio
import errno
import ipaddress
import socket
import struct
import os
import sys
import time
from collections import deque
from datetime import datetime

APP_NAME = "ProxyDock"
APP_ID = "ProxyDock"                    # settings folder, autostart name, etc.
OLD_APP_ID = "ProxyLocal"               # settings folder name of the previous version (SOCKS5 Proxy)

IS_WIN = sys.platform == "win32"
IS_MAC = sys.platform == "darwin"
IS_LINUX = sys.platform.startswith("linux")

BUFSIZE = 65536
MAX_CONN = 4096             # max concurrent client connections (total)
MAX_CONN_PER_IP = 256       # max concurrent connections from one IP (loopback excluded)
AUTH_FAIL_LIMIT = 10        # after this many authentication failures
AUTH_FAIL_WINDOW = 60       # (within this many seconds)
BAN_SECONDS = 300           # ban that IP for this long
UDP_BUF = 4 * 1024 * 1024
DNS_TTL = 60
HANDSHAKE_TIMEOUT = 15      # time limit for the whole inbound handshake (s)
CONNECT_TIMEOUT = 10        # time limit for the outbound connection, including upstream handshakes (s)
HANDSHAKE_LIMIT = 16 * 1024 # buffer limit during the handshake (reading pauses above it)
MAX_LOG = 2000

SOCKS5_VER = 0x05
CMD_CONNECT = 0x01
CMD_UDP_ASSOCIATE = 0x03
ATYP_IPV4 = 0x01
ATYP_DOMAIN = 0x03
ATYP_IPV6 = 0x04
REP_SUCCESS = 0x00
REP_GENERAL = 0x01
REP_DENIED = 0x02
REP_NET_UNREACH = 0x03
REP_HOST_UNREACH = 0x04
REP_REFUSED = 0x05
REP_UNSUPPORTED = 0x07
REP_ATYP_UNSUPPORTED = 0x08

# --- protocol names ---
INBOUND_TYPES = {           # inbound
    "socks5": "SOCKS5",
    "socks4": "SOCKS4",
    "http": "HTTP",
    "https": "HTTPS",
    "shadowsocks": "Shadowsocks",
    "hysteria2": "Hysteria2",
    "tun": "TUN",
}
OUTBOUND_TYPES = {}
TUN_STACKS = ("mixed", "system", "gvisor")


def inbound_where(ib):
    """Where an inbound accepts traffic: "127.0.0.1:1080", or "all apps (TUN)" for the virtual network card."""
    if ib["type"] == "tun":
        return _t("all apps") + f" · {ib.get('tun_stack', 'mixed')}"
    return host_port(ib["host"], ib["port"])


def is_admin():
    """True if this process may create a TUN device (Windows: elevated, elsewhere: root)."""
    if IS_WIN:
        import ctypes
        try:
            return bool(ctypes.windll.shell32.IsUserAnAdmin())
        except OSError:
            return False
    return os.geteuid() == 0


@on_language_change
def _tr_outbound_types():
    OUTBOUND_TYPES.clear()
    OUTBOUND_TYPES.update({          # outbound
        "direct": "Direct",
        "http": _t("HTTP proxy"),
        "https": _t("HTTPS proxy"),
        "socks4": _t("SOCKS4 proxy"),
        "socks5": _t("SOCKS5 proxy"),
        "shadowsocks": _t("Shadowsocks tunnel"),
        "hysteria2": _t("Hysteria2 tunnel"),
        "wireguard": _t("WireGuard tunnel"),
        "vless": _t("VLESS tunnel"),
        "trojan": _t("Trojan tunnel"),
        "tuic": _t("TUIC tunnel"),
        "block": _t("Block"),
    })


UDP_INBOUNDS = ("socks5", "shadowsocks", "hysteria2")     # inbound types that can receive UDP
UDP_OUTBOUNDS = ("direct", "shadowsocks", "hysteria2", "wireguard", "vless", "trojan", "tuic")   # can send UDP
NEEDS_UDP_VIA = ("wireguard", "tuic")       # UDP-based tunnels: may only go through blocks that carry UDP
UDP_VIA_OK = ("shadowsocks", "hysteria2", "wireguard", "socks5", "vless", "trojan", "tuic")
SINGBOX_ONLY_OUTBOUNDS = ("block", "wireguard", "vless", "trojan", "tuic")


# --- logging ---
# The proxy thread only appends to pending_logs; the UI thread drains it in batches on a timer.
# Entry: (time, level, message, proxy id, proxy name) - app-wide entries have an empty id/name.

LOG_TCP = 0        # TCP connect / close
LOG_ERROR = 1      # internal error
LOG_SETTING = 2    # settings change
LOG_INFO = 3       # startup / status info
LOG_UDP = 4        # UDP session
LOG_BLOCKED = 5    # allow list, private network, IP ban
LOG_AUTH = 6       # authentication failure
LOG_FAIL = 7       # remote connection failure (DNS, refused, timeout, upstream proxy error)
LOG_WARN = 8       # warning (open proxy, etc.)
LEVEL_TAGS = {
    LOG_TCP: "TCP", LOG_UDP: "UDP", LOG_BLOCKED: "BLOCK", LOG_AUTH: "AUTH",
    LOG_FAIL: "FAIL", LOG_ERROR: "ERROR", LOG_WARN: "WARN", LOG_SETTING: "SET",
    LOG_INFO: "INFO",
}

pending_logs = deque(maxlen=5000)
notify_hook = None          # set by the UI: function(message) that shows a settings-change notification

_CTRL = {i: " " for i in range(32)}
_CTRL[0x7F] = " "


def log(level, message, inst=None):
    """inst: the ProxyInstance this entry belongs to (None for app-wide entries)."""
    message = message.translate(_CTRL)
    pid, pname = (inst.id, inst.name) if inst is not None else ("", "")
    if inst is not None and level in (LOG_FAIL, LOG_AUTH, LOG_BLOCKED, LOG_ERROR):
        inst.last_error = message       # shown as the concrete failure reason in the connection check
    pending_logs.append((datetime.now().strftime("%Y-%m-%d %H:%M:%S"), level, message, pid, pname))
    if level == LOG_SETTING and notify_hook is not None:
        notify_hook(message)


def format_line(entry, with_name=True):
    ts, level, message, _pid, pname = entry
    name = f"[{pname}] " if with_name and pname else ""
    return f"[{ts}] {LEVEL_TAGS.get(level, '?'):<5} {name}{message}"


def fmt_size(n):
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024:
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024
    return f"{n:.2f} TB"


def fmt_rate(n):
    return fmt_size(n) + "/s"


def describe_error(e):
    if isinstance(e, asyncio.TimeoutError):
        return _t("Connection timed out")
    if isinstance(e, socket.gaierror):
        return _t("DNS lookup failed")
    if isinstance(e, ConnectionRefusedError):
        return _t("Connection refused")
    if isinstance(e, UpstreamError):
        return str(e)
    code = getattr(e, "winerror", None)
    err = getattr(e, "errno", None)
    if code in (10051, 10050) or err in (errno.ENETUNREACH, errno.ENETDOWN):
        return _t("Network unreachable") + (f" (WinError {code})" if code else "")
    if code in (10060, 10065) or err in (errno.EHOSTUNREACH, errno.ETIMEDOUT):
        return _t("Host unreachable") + (f" (WinError {code})" if code else "")
    return f"{type(e).__name__}" + (f" (WinError {code})" if code else "") + (f": {e}" if str(e) else "")


class BlockedDestination(Exception):
    """Destination outside the allow list / on the private network."""


class UpstreamError(OSError):
    """An upstream proxy or tunnel refused/failed the connection. reason: refused/host/net/denied/auth/general"""

    def __init__(self, message, reason="general"):
        super().__init__(message)
        self.reason = reason


def fail_reason(exc):
    """Exception -> inbound reply kind: denied / refused / host / net / timeout / general"""
    if isinstance(exc, BlockedDestination):
        return "denied"
    if isinstance(exc, UpstreamError):
        return exc.reason
    if isinstance(exc, ConnectionRefusedError):
        return "refused"
    if isinstance(exc, asyncio.TimeoutError):
        return "timeout"
    if isinstance(exc, socket.gaierror):
        return "host"
    code = getattr(exc, "winerror", None)
    err = getattr(exc, "errno", None)
    if code in (10051, 10050) or err in (errno.ENETUNREACH, errno.ENETDOWN):
        return "net"
    if code in (10060, 10065) or err in (errno.EHOSTUNREACH, errno.ETIMEDOUT):
        return "host"
    if isinstance(exc, OSError) and not isinstance(exc, ConnectionError):
        return "host"
    return "general"


# --- statistics (only incremented on the proxy thread; the UI reads them once a second) ---

class Stats:
    __slots__ = ("tcp_active", "udp_active", "total", "failed", "blocked", "up", "down")

    def __init__(self):
        for name in self.__slots__:
            setattr(self, name, 0)

    @classmethod
    def sum(cls, items):
        out = cls()
        for s in items:
            for name in cls.__slots__:
                setattr(out, name, getattr(out, name) + getattr(s, name))
        return out


# --- addresses ---

def strip_scope(addr):
    return addr.split("%", 1)[0]


def is_internal_ip(ip):
    """True for addresses that are not public internet addresses (loopback, private, link-local, CGNAT, multicast, ...)."""
    try:
        a = ipaddress.ip_address(strip_scope(ip))
    except ValueError:
        return True
    if a.version == 6 and a.ipv4_mapped:
        a = a.ipv4_mapped
    return not a.is_global or a.is_multicast


def is_ip_literal(host):
    try:
        ipaddress.ip_address(strip_scope(host))
        return True
    except ValueError:
        return False


def is_loopback_ip(ip):
    try:
        return ipaddress.ip_address(strip_scope(ip)).is_loopback
    except ValueError:
        return False


def is_loopback_host(host):
    return host.lower() == "localhost" or is_loopback_ip(host)


def valid_domain(addr):
    return bool(addr) and not any(ord(c) < 33 or ord(c) == 0x7F for c in addr) and len(addr) <= 255


def parse_address(data, offset):
    """SOCKS5-style address (ATYP + address + port) -> (address, port, end offset)."""
    try:
        atyp = data[offset]
        if atyp == ATYP_IPV4:
            addr = socket.inet_ntoa(data[offset + 1 : offset + 5])
            port = struct.unpack("!H", data[offset + 5 : offset + 7])[0]
            return addr, port, offset + 7
        elif atyp == ATYP_DOMAIN:
            length = data[offset + 1]
            addr = data[offset + 2 : offset + 2 + length].decode()
            if not valid_domain(addr):
                raise ValueError("invalid domain")
            port = struct.unpack("!H", data[offset + 2 + length : offset + 4 + length])[0]
            return addr, port, offset + 4 + length
        elif atyp == ATYP_IPV6:
            addr = socket.inet_ntop(socket.AF_INET6, data[offset + 1 : offset + 17])
            port = struct.unpack("!H", data[offset + 17 : offset + 19])[0]
            return addr, port, offset + 19
    except (struct.error, IndexError, OSError, UnicodeDecodeError) as e:
        raise ValueError(f"Malformed address: {e}")
    raise ValueError(f"Unknown ATYP: {atyp}")


def pack_addr(addr):
    addr = strip_scope(addr)
    if ":" in addr:
        return ATYP_IPV6, socket.inet_pton(socket.AF_INET6, addr)
    return ATYP_IPV4, socket.inet_aton(addr)


def encode_address(host, port):
    """Any host (IP or domain) -> SOCKS5-style address bytes."""
    try:
        atyp, raw = pack_addr(host)
        return bytes([atyp]) + raw + struct.pack("!H", port)
    except OSError:
        h = host.encode("idna")
        if len(h) > 255:
            raise ValueError("domain too long")
        return bytes([ATYP_DOMAIN, len(h)]) + h + struct.pack("!H", port)


def normalize_ip(ip):
    ip = strip_scope(ip)
    fam = socket.AF_INET6 if ":" in ip else socket.AF_INET
    return socket.inet_ntop(fam, socket.inet_pton(fam, ip))


def make_reply(rep, bind_addr="0.0.0.0", bind_port=0):
    atyp, addr_bytes = pack_addr(bind_addr)
    return struct.pack("!BBBB", SOCKS5_VER, rep, 0x00, atyp) + addr_bytes + struct.pack("!H", bind_port)


def make_udp_header(addr, port):
    atyp, addr_bytes = pack_addr(addr)
    return struct.pack("!HBB", 0x0000, 0x00, atyp) + addr_bytes + struct.pack("!H", port)


def host_port(host, port):
    return f"[{host}]:{port}" if ":" in host else f"{host}:{port}"


def split_host_port(s, default_port=None):
    """'host:port' / '[v6]:port' -> (host, port)."""
    s = s.strip()
    if s.startswith("["):
        host, _, rest = s[1:].partition("]")
        port = rest[1:] if rest.startswith(":") else ""
    else:
        host, _, port = s.rpartition(":") if s.count(":") == 1 else (s, "", "")
    if not port:
        if default_port is None:
            raise ValueError("port missing")
        return host, default_port
    p = int(port)
    if not 0 < p < 65536:
        raise ValueError("bad port")
    return host, p


# --- per-IP connection limit / temporary ban after repeated auth failures (shared by all proxies) ---

per_ip = {}
_auth_fails = {}
_banned = {}


def note_auth_failure(ip, inst=None):
    """Record an authentication/decryption failure. Too many in a short time bans the IP for a while (loopback excluded)."""
    if not ip or is_loopback_ip(ip):
        return
    now = time.monotonic()
    q = _auth_fails.setdefault(ip, deque())
    q.append(now)
    while q and now - q[0] > AUTH_FAIL_WINDOW:
        q.popleft()
    if len(q) >= AUTH_FAIL_LIMIT:
        q.clear()
        _banned[ip] = now + BAN_SECONDS
        log(LOG_BLOCKED, _t("Blocking {ip} for {value} min after repeated authentication failures").format(ip=ip, value=BAN_SECONDS // 60), inst)
    if len(_auth_fails) > 10000:
        _auth_fails.clear()


def is_banned(ip):
    until = _banned.get(ip)
    if until is None:
        return False
    if time.monotonic() >= until:
        del _banned[ip]
        return False
    return True


def hard_exit(code=0):
    """End the process now. On Windows skip DLL unload routines: Qt plugin DLLs can crash there
    (access violation at exit) while background threads are still running."""
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.flush()
        except Exception:
            pass
    if IS_WIN:
        import ctypes
        kernel32 = ctypes.windll.kernel32
        kernel32.GetCurrentProcess.restype = ctypes.c_void_p
        kernel32.TerminateProcess(ctypes.c_void_p(kernel32.GetCurrentProcess()), code)
    os._exit(code)
