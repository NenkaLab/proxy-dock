"""공통: 상수, 로그, 주소 처리, IP 차단. Qt 에 의존하지 않는다 (프록시 스레드·테스트에서도 사용)."""
import asyncio
import errno
import ipaddress
import socket
import struct
import sys
import time
from collections import deque
from datetime import datetime

APP_NAME = "ProxyDock"
APP_ID = "ProxyDock"                    # 설정 폴더, 자동 실행 이름 등
OLD_APP_ID = "ProxyLocal"               # 이전 버전(SOCKS5 Proxy) 설정 폴더 이름

IS_WIN = sys.platform == "win32"
IS_MAC = sys.platform == "darwin"
IS_LINUX = sys.platform.startswith("linux")

BUFSIZE = 65536
MAX_CONN = 4096             # 동시 클라이언트 연결 상한 (전체)
MAX_CONN_PER_IP = 256       # 같은 IP(루프백 제외)의 동시 연결 상한
AUTH_FAIL_LIMIT = 10        # 이 횟수만큼 인증 실패하면
AUTH_FAIL_WINDOW = 60       # (이 시간(초) 안에)
BAN_SECONDS = 300           # 이 시간 동안 해당 IP 차단
UDP_BUF = 4 * 1024 * 1024
DNS_TTL = 60
HANDSHAKE_TIMEOUT = 15      # 받는 쪽 협상 전체 제한 시간(초)
CONNECT_TIMEOUT = 10        # 내보내는 쪽 연결(상위 프록시 협상 포함) 제한 시간(초)
HANDSHAKE_LIMIT = 16 * 1024 # 협상 중 버퍼 상한 (넘으면 읽기 일시정지)
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

# --- 프로토콜 이름 ---
INBOUND_TYPES = {           # 받는 쪽
    "socks5": "SOCKS5",
    "socks4": "SOCKS4",
    "http": "HTTP",
    "https": "HTTPS",
    "shadowsocks": "Shadowsocks",
    "hysteria2": "Hysteria2",
}
OUTBOUND_TYPES = {          # 내보내는 쪽
    "direct": "Direct",
    "http": "HTTP 프록시",
    "https": "HTTPS 프록시",
    "socks4": "SOCKS4 프록시",
    "socks5": "SOCKS5 프록시",
    "shadowsocks": "Shadowsocks 터널",
    "hysteria2": "Hysteria2 터널",
}
UDP_INBOUNDS = ("socks5", "shadowsocks", "hysteria2")     # UDP 를 받을 수 있는 받는 쪽
UDP_OUTBOUNDS = ("direct", "shadowsocks", "hysteria2")    # UDP 를 내보낼 수 있는 내보내는 쪽


# --- 로그 ---
# 프록시 스레드는 _pending 에 넣기만 하고, UI 스레드가 타이머로 묶어서 가져간다.
# 항목: (시각, 종류, 메시지, 프록시 ID, 프록시 이름) - 앱 전체 로그는 ID/이름이 "".

LOG_TCP = 0        # TCP 연결 / 종료
LOG_ERROR = 1      # 내부 오류
LOG_SETTING = 2    # 설정 변경
LOG_INFO = 3       # 시작/상태 정보
LOG_UDP = 4        # UDP 세션
LOG_BLOCKED = 5    # 허용 목록·내부망·IP 차단
LOG_AUTH = 6       # 인증 실패
LOG_FAIL = 7       # 원격 연결 실패 (DNS, 거부, 타임아웃, 상위 프록시 오류)
LOG_WARN = 8       # 경고 (오픈 프록시 등)
LEVEL_TAGS = {
    LOG_TCP: "TCP", LOG_UDP: "UDP", LOG_BLOCKED: "BLOCK", LOG_AUTH: "AUTH",
    LOG_FAIL: "FAIL", LOG_ERROR: "ERROR", LOG_WARN: "WARN", LOG_SETTING: "SET",
    LOG_INFO: "INFO",
}

pending_logs = deque(maxlen=5000)
notify_hook = None          # UI 가 설정: 설정 변경 알림을 띄우는 함수(message)

_CTRL = {i: " " for i in range(32)}
_CTRL[0x7F] = " "


def log(level, message, inst=None):
    """inst: 이 로그가 속한 ProxyInstance (없으면 앱 전체 로그)."""
    message = message.translate(_CTRL)
    pid, pname = (inst.id, inst.name) if inst is not None else ("", "")
    if inst is not None and level in (LOG_FAIL, LOG_AUTH, LOG_BLOCKED, LOG_ERROR):
        inst.last_error = message       # 연결 확인 결과에 구체적인 실패 이유로 보여 준다
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
        return "연결 시간 초과"
    if isinstance(e, socket.gaierror):
        return "DNS 해석 실패"
    if isinstance(e, ConnectionRefusedError):
        return "연결 거부됨"
    if isinstance(e, UpstreamError):
        return str(e)
    code = getattr(e, "winerror", None)
    err = getattr(e, "errno", None)
    if code in (10051, 10050) or err in (errno.ENETUNREACH, errno.ENETDOWN):
        return "네트워크 연결 불가" + (f" (WinError {code})" if code else "")
    if code in (10060, 10065) or err in (errno.EHOSTUNREACH, errno.ETIMEDOUT):
        return "호스트 연결 불가" + (f" (WinError {code})" if code else "")
    return f"{type(e).__name__}" + (f" (WinError {code})" if code else "") + (f": {e}" if str(e) else "")


class BlockedDestination(Exception):
    """허용 목록 밖 / 내부망 목적지."""


class UpstreamError(OSError):
    """상위 프록시·터널이 연결을 거부/실패함. reason: refused/host/net/denied/auth/general"""

    def __init__(self, message, reason="general"):
        super().__init__(message)
        self.reason = reason


def fail_reason(exc):
    """예외 -> 받는 쪽 응답 종류: denied / refused / host / net / timeout / general"""
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


# --- 통계 (프록시 스레드에서만 증가, UI 는 1초마다 읽기만 한다) ---

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


# --- 주소 ---

def strip_scope(addr):
    return addr.split("%", 1)[0]


def is_internal_ip(ip):
    """루프백/사설/링크로컬/CGNAT/멀티캐스트 등 공인 인터넷 주소가 아닌 경우 True."""
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
    """SOCKS5 형식 주소(ATYP+주소+포트) -> (주소, 포트, 끝 위치)."""
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
    """임의 호스트(IP 또는 도메인) -> SOCKS5 형식 주소 바이트."""
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


# --- IP 별 연결 상한 / 인증 실패 반복 시 일시 차단 (모든 프록시 공용) ---

per_ip = {}
_auth_fails = {}
_banned = {}


def note_auth_failure(ip, inst=None):
    """인증/복호화 실패 기록. 짧은 시간에 너무 많으면 그 IP 를 잠시 차단 (루프백 제외)."""
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
        log(LOG_BLOCKED, f"{ip} 인증 실패가 반복되어 {BAN_SECONDS // 60}분간 차단합니다", inst)
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
