"""연결 테스트: 선택한 프록시를 실제로 거쳐 여러 사이트에 HTTPS 로 접속해 본다.

어떤 받는 쪽 프로토콜이든 같은 방법으로 시험하려고, 임시 SOCKS5 '다리' 프록시를 하나 띄운다.
다리의 내보내는 쪽 = 시험할 프록시의 받는 쪽 프로토콜(SOCKS/HTTP/HTTPS/Shadowsocks/Hysteria2) 클라이언트.
그래서 다리에 SOCKS5 로 접속하면 → 시험할 프록시 → 그 프록시의 내보내는 쪽 → 사이트 순서로 간다.
"""
import socket
import ssl
import struct
import time

from . import config
from .common import ATYP_IPV4, ATYP_IPV6, describe_error

TEST_SITES = [
    ("글로벌", "Google", "www.google.com"),
    ("글로벌", "YouTube", "www.youtube.com"),
    ("글로벌", "Microsoft", "www.microsoft.com"),
    ("글로벌", "Apple", "www.apple.com"),
    ("글로벌", "Amazon", "www.amazon.com"),
    ("글로벌", "Cloudflare", "www.cloudflare.com"),
    ("글로벌", "GitHub", "github.com"),
    ("글로벌", "Wikipedia", "www.wikipedia.org"),
    ("글로벌", "Netflix", "www.netflix.com"),
    ("한국", "NAVER", "www.naver.com"),
    ("일본", "Yahoo! JAPAN", "www.yahoo.co.jp"),
    ("중국", "Baidu", "www.baidu.com"),
    ("미국", "CNN", "www.cnn.com"),
    ("영국", "BBC", "www.bbc.co.uk"),
    ("독일", "DER SPIEGEL", "www.spiegel.de"),
    ("프랑스", "Le Monde", "www.lemonde.fr"),
    ("브라질", "Globo", "www.globo.com"),
    ("인도", "Flipkart", "www.flipkart.com"),
    ("호주", "ABC", "www.abc.net.au"),
    ("캐나다", "CBC", "www.cbc.ca"),
    ("러시아", "Yandex", "ya.ru"),
]
PROBE_TIMEOUT = 8
PROBE_WORKERS = 6           # 동시에 시도하는 사이트 수
PROBE_COOLDOWN = 15         # 테스트 후 다시 누를 수 있을 때까지(초)
BRIDGE_ID = "__probe__"
REP_NAMES = {1: "프록시 오류", 2: "차단됨 (허용 목록/내부망)", 3: "네트워크 연결 불가",
             4: "호스트 연결 불가 (DNS/경로/상위 프록시)", 5: "연결 거부", 6: "TTL 만료",
             7: "지원하지 않는 명령", 8: "지원하지 않는 주소 형식"}


def bridge_for(proxy):
    """시험할 프록시로 가는 임시 SOCKS5 다리 프록시 설정."""
    ib = proxy["inbound"]
    host = {"0.0.0.0": "127.0.0.1", "::": "::1"}.get(ib["host"], ib["host"])
    t = ib["type"]
    ob = {"type": t, "host": host, "port": ib["port"], "tls_verify": False}
    if t in ("socks5", "http", "https") and ib["auth_enabled"]:
        ob.update(username=ib["username"], password=ib["password"])
    if t == "shadowsocks":
        ob.update(ss_method=ib["ss_method"], ss_password=ib["ss_password"])
    if t == "hysteria2":
        ob.update(hy2_password=ib["hy2_password"])
    p = config.new_proxy("연결 테스트", {"type": "socks5", "host": "127.0.0.1", "port": 1}, ob)
    p["id"] = BRIDGE_ID
    p["inbound"]["port"] = 0            # 남는 포트를 쓴다 (validate 후 0 으로)
    p["overrides"].update(allowed_domains=["*"], block_private=False, log_connections=False, log_close=False)
    return p


def bridge_port(manager):
    inst = manager.instances.get(BRIDGE_ID)
    return inst.servers[0].sockets[0].getsockname()[1] if inst else None


def _recvn(s, n):
    data = b""
    while len(data) < n:
        chunk = s.recv(n - len(data))
        if not chunk:
            raise ConnectionError("프록시가 연결을 닫음")
        data += chunk
    return data


def probe_site(host, port):
    """다리(127.0.0.1:port)를 거쳐 host:443 에 접속. 결과 dict (ok, status, connect_ms, total_ms, error, note)."""
    res = {"ok": False, "status": "", "connect_ms": None, "total_ms": None, "error": ""}
    t0 = time.perf_counter()
    try:
        s = socket.create_connection(("127.0.0.1", port), timeout=PROBE_TIMEOUT)
    except OSError as e:
        res["error"] = f"테스트 연결 실패: {describe_error(e)}"
        return res
    try:
        s.sendall(b"\x05\x01\x00")
        _recvn(s, 2)
        hb = host.encode("idna")
        s.sendall(b"\x05\x01\x00\x03" + bytes([len(hb)]) + hb + struct.pack("!H", 443))
        rep = _recvn(s, 4)
        if rep[1] != 0:
            raise ValueError(REP_NAMES.get(rep[1], f"SOCKS 오류 {rep[1]}"))
        _recvn(s, {ATYP_IPV4: 6, ATYP_IPV6: 18}.get(rep[3], 6))
        res["connect_ms"] = (time.perf_counter() - t0) * 1000
        ctx = ssl.create_default_context()
        with ctx.wrap_socket(s, server_hostname=host) as ss:
            ss.sendall((f"HEAD / HTTP/1.1\r\nHost: {host}\r\n"
                        "User-Agent: Mozilla/5.0 (Windows NT 10.0; Win64; x64)\r\n"
                        "Connection: close\r\n\r\n").encode())
            line = ss.recv(512).split(b"\r\n", 1)[0].decode("latin-1")
        parts = line.split()
        res["status"] = parts[1] if len(parts) > 1 and line.startswith("HTTP/") else "응답 없음"
        res["ok"] = res["status"].isdigit()
        if not res["ok"]:
            res["error"] = "HTTP 응답이 아님"
    except ssl.SSLCertVerificationError:
        # 경로와 TLS 서버 도달은 성공. Python 이 OS 의 필요 시 내려받는 루트 인증서를 못 쓰는 경우가 많다.
        res["ok"] = True
        res["note"] = "인증서 확인 불가 (로컬 루트 인증서 없음)"
    except ssl.SSLError as e:
        res["error"] = f"TLS 오류 ({e.reason or e})"
    except socket.timeout:
        res["error"] = "시간 초과"
    except ValueError as e:
        res["error"] = str(e)
    except OSError as e:
        res["error"] = describe_error(e)
    finally:
        s.close()
        res["total_ms"] = (time.perf_counter() - t0) * 1000
    return res


# --- 편집 화면의 '연결 확인': 저장하지 않은 설정으로 내보내는 쪽을 시험 ---

CHECK_ID = "__check__"
CHECK_SITES = ("www.google.com", "www.cloudflare.com", "www.microsoft.com")


def checker_for(proxy, glob):
    """편집 중인 프록시와 같은 내보내는 쪽 + 로컬 설정을 쓰는 임시 SOCKS5 프록시 (127.0.0.1, 남는 포트)."""
    import copy
    p = config.new_proxy("연결 확인", {"type": "socks5", "host": "127.0.0.1", "port": 1},
                         copy.deepcopy(proxy["outbound"]))
    p["id"] = CHECK_ID
    p["inbound"]["port"] = 0
    p["overrides"] = config.resolve(proxy, glob)
    p["overrides"].update(allowed_domains=["*"], log_connections=False, log_close=False)
    return p


def check_outbound(manager, proxy, glob):
    """내보내는 쪽을 거쳐 CHECK_SITES 중 하나에 HTTPS 로 접속되는지 확인.
    결과 dict: ok, site, status, connect_ms, total_ms, error, detail(프록시가 남긴 실패 이유), note."""
    res = {"ok": False, "site": "", "error": "", "detail": ""}
    try:
        if not manager.start(checker_for(proxy, glob), glob).result(15):
            res["error"] = manager.state(CHECK_ID)[1] or "확인용 연결을 만들지 못했습니다"
            return res
        inst = manager.instances.get(CHECK_ID)
        port = bridge_port_for(manager, CHECK_ID)
        for site in CHECK_SITES:
            inst.last_error = ""
            r = probe_site(site, port)
            r["site"] = site
            r["detail"] = inst.last_error
            res = r
            if r["ok"] or r["error"] == "시간 초과":    # 시간 초과는 대개 경로 문제라 다른 사이트도 같다
                break
        return res
    except Exception as e:
        res["error"] = str(e) or type(e).__name__
        return res
    finally:
        try:
            manager.stop(CHECK_ID).result(5)
        except Exception:
            pass


def bridge_port_for(manager, pid):
    inst = manager.instances.get(pid)
    return inst.servers[0].sockets[0].getsockname()[1] if inst else None
