"""설정: 경로, v2 형식(전역 + 프록시 목록), 검증, 비밀번호 보관, 이전 버전 설정 이전, 전역/로컬 병합.

config.json
{ "version": 2,
  "global":  { 앱 설정 + 프록시 기본값 },
  "proxies": [ { "id", "name", "autostart", "inbound": {...}, "outbound": {...}, "overrides": {...} } ] }

프록시의 실제 설정 = overrides 에 값이 있으면 그 값(로컬), null 이면 전역 기본값.
"""
import base64
import copy
import ctypes
import ipaddress
import json
import os
import shutil
import uuid

from .common import (
    APP_ID, INBOUND_TYPES, IS_MAC, IS_WIN, LOG_INFO, LOG_WARN, OLD_APP_ID, OUTBOUND_TYPES, log,
)


def app_dir(name=APP_ID):
    home = os.path.expanduser("~")
    if IS_WIN:
        base = os.environ.get("APPDATA") or home
    elif IS_MAC:
        base = os.path.join(home, "Library", "Application Support")
    else:
        base = os.environ.get("XDG_CONFIG_HOME") or os.path.join(home, ".config")
    return os.path.join(base, name)


APP_DIR = app_dir()
CONFIG_PATH = os.path.join(APP_DIR, "config.json")
LOG_DIR = os.path.join(APP_DIR, "logs")
CERT_DIR = os.path.join(APP_DIR, "certs")
WINDOW_STATE_PATH = os.path.join(APP_DIR, "window.json")

THEMES = [("auto", "기기 설정"), ("light", "라이트"), ("dark", "다크")]
UI_SCALES = [90, 100, 110, 125, 150]

GLOBAL_DEFAULTS = {
    # 앱 전용 (전역에만 있음)
    "theme": "auto",
    "ui_scale": 100,
    "ui_font": "",              # "" = 내장 Pretendard
    "mono_font": "",            # "" = 내장 JetBrains Mono
    "disable_ligatures": False,
    "log_font_size": 10,
    "close_exits": False,
    "log_to_file": False,
    # 프록시 기본값 (프록시 로컬 설정이 우선)
    "allowed_domains": ["*"],
    "block_private": False,
    "timeout": 120,
    "log_connections": True,
    "log_close": False,
}
OVERRIDABLE = ("allowed_domains", "block_private", "timeout", "log_connections", "log_close")

INBOUND_DEFAULTS = {
    "type": "socks5",
    "host": "127.0.0.1",
    "port": 30307,
    "auth_enabled": False,
    "username": "",
    "password": "",
    "tls_cert": "",             # "" = 자동 생성 자체 서명 인증서
    "tls_key": "",
    "ss_method": "2022-blake3-aes-256-gcm",
    "ss_password": "",
    "hy2_password": "",
}
OUTBOUND_DEFAULTS = {
    "type": "direct",
    "host": "",
    "port": 0,
    "username": "",
    "password": "",
    "tls_verify": True,
    "sni": "",
    "pin_sha256": "",
    "ss_method": "2022-blake3-aes-256-gcm",
    "ss_password": "",
    "hy2_password": "",
}
SECRET_FIELDS = ("password", "ss_password", "hy2_password")


# --- 검증 ---

def _typed(defaults, raw):
    out = dict(defaults)
    if not isinstance(raw, dict):
        return out
    for key, default in defaults.items():
        v = raw.get(key, default)
        if isinstance(default, bool):
            ok = isinstance(v, bool)
        elif isinstance(default, int):
            ok = isinstance(v, int) and not isinstance(v, bool)
        elif isinstance(default, list):
            ok = isinstance(v, list) and all(isinstance(x, str) for x in v)
            if ok:
                v = [x.strip() for x in v if x.strip()]
        else:
            ok = isinstance(v, str)
        if ok:
            out[key] = v
    return out


def validate_global(raw):
    from .ss import SS_METHODS  # noqa: F401  (형식 확인용 import 순서 유지)
    g = _typed(GLOBAL_DEFAULTS, raw)
    if g["theme"] not in dict(THEMES):
        g["theme"] = "auto"
    if g["ui_scale"] not in UI_SCALES:
        g["ui_scale"] = 100
    g["log_font_size"] = min(max(g["log_font_size"], 8), 20)
    g["timeout"] = min(max(g["timeout"], 10), 86400)
    if not g["allowed_domains"]:
        g["allowed_domains"] = ["*"]
    return g


def validate_proxy(raw):
    from .ss import SS_METHODS
    raw = raw if isinstance(raw, dict) else {}
    p = {
        "id": raw.get("id") if isinstance(raw.get("id"), str) and raw.get("id") else uuid.uuid4().hex[:8],
        "name": (raw.get("name") or "프록시").strip()[:60] if isinstance(raw.get("name"), str) else "프록시",
        "autostart": raw.get("autostart") if isinstance(raw.get("autostart"), bool) else False,
        "inbound": _typed(INBOUND_DEFAULTS, raw.get("inbound")),
        "outbound": _typed(OUTBOUND_DEFAULTS, raw.get("outbound")),
        "overrides": {},
    }
    ib, ob = p["inbound"], p["outbound"]
    if ib["type"] not in INBOUND_TYPES:
        ib["type"] = "socks5"
    if ob["type"] not in OUTBOUND_TYPES:
        ob["type"] = "direct"
    for d in (ib, ob):
        if d["ss_method"] not in SS_METHODS:
            d["ss_method"] = INBOUND_DEFAULTS["ss_method"]
    if not 1 <= ib["port"] <= 65535:
        ib["port"] = INBOUND_DEFAULTS["port"]
    if not 0 <= ob["port"] <= 65535:
        ob["port"] = 0
    ov = raw.get("overrides") if isinstance(raw.get("overrides"), dict) else {}
    typed = _typed({k: GLOBAL_DEFAULTS[k] for k in OVERRIDABLE}, ov)
    for k in OVERRIDABLE:
        p["overrides"][k] = typed[k] if ov.get(k) is not None else None
    if p["overrides"]["timeout"] is not None:
        p["overrides"]["timeout"] = min(max(p["overrides"]["timeout"], 10), 86400)
    return p


def new_proxy(name, inbound=None, outbound=None, autostart=False):
    return validate_proxy({"id": uuid.uuid4().hex[:8], "name": name, "autostart": autostart,
                           "inbound": inbound or {}, "outbound": outbound or {}})


def resolve(proxy, glob):
    """프록시의 실제 설정 (로컬 우선, 없으면 전역)."""
    out = {}
    for k in OVERRIDABLE:
        v = proxy["overrides"].get(k)
        out[k] = copy.deepcopy(v if v is not None else glob[k])
    return out


def _valid_listen_host(h):
    if h.lower() == "localhost":
        return True
    try:
        ipaddress.ip_address(h)
        return True
    except ValueError:
        return False


def _bind_addrs(host):
    """받는 쪽 주소 -> 실제로 묶이는 (주소군, 주소) 집합. localhost 는 IPv4·IPv6 루프백 둘 다 연다."""
    if host.lower() == "localhost":
        return {(4, ipaddress.ip_address("127.0.0.1")), (6, ipaddress.ip_address("::1"))}
    try:
        a = ipaddress.ip_address(host)
    except ValueError:
        return set()
    return {(a.version, a)}


def _binds_overlap(h1, h2, udp=False):
    """같은 포트에서 두 주소가 동시에 열릴 수 없으면 True (같은 주소군에서 같은 주소이거나 한쪽이 전체 주소).
    TCP 는 IPv6 전용으로 열려 :: 와 0.0.0.0 이 같이 열리지만, UDP 의 :: 는 OS 에 따라 IPv4 도 받으므로 겹친다고 본다."""
    def addrs(h):
        out = _bind_addrs(h)
        if udp and (6, ipaddress.ip_address("::")) in out:
            out.add((4, ipaddress.ip_address("0.0.0.0")))
        return out
    for f1, a1 in addrs(h1):
        for f2, a2 in addrs(h2):
            if f1 == f2 and (a1 == a2 or a1.is_unspecified or a2.is_unspecified):
                return True
    return False


def _is_local_target(host):
    h = host.strip().lower().rstrip(".")
    if h == "localhost" or h.endswith(".localhost"):
        return True
    try:
        a = ipaddress.ip_address(h.strip("[]"))
    except ValueError:
        return False
    return a.is_loopback or a.is_unspecified


def _next_hop(p, proxies):
    """p 의 내보내는 쪽이 이 앱의 다른(또는 자기) 프록시 받는 쪽을 가리키면 그 프록시."""
    ob = p["outbound"]
    oh = ob["host"].strip().strip("[]")
    if ob["type"] == "direct" or not oh:
        return None
    for q in proxies:
        ih = q["inbound"]["host"]
        if q["inbound"]["port"] != ob["port"]:
            continue
        if oh.lower() == ih.lower() or (_is_local_target(oh) and _is_local_target(ih)):
            return q
    return None


def find_cycle(p, proxies):
    """p 부터 내보내는 쪽을 따라가다 p 로 돌아오면 경로(이름 목록), 아니면 None."""
    others = [q for q in proxies if q["id"] != p["id"]] + [p]
    path, cur, seen = [p["name"]], p, {p["id"]}
    while True:
        nxt = _next_hop(cur, others)
        if nxt is None:
            return None
        path.append(nxt["name"])
        if nxt["id"] == p["id"]:
            return path
        if nxt["id"] in seen:
            return None
        seen.add(nxt["id"])
        cur = nxt


def _transports(t):
    return {"socks5": {"tcp", "udp"}, "shadowsocks": {"tcp", "udp"}, "hysteria2": {"udp"}}.get(t, {"tcp"})


def check_proxy(p, proxies):
    """저장/시작 전에 확인할 문제 목록 (빈 목록이면 정상)."""
    from .ss import SS_METHODS, ss_make_key
    errs = []
    ib, ob = p["inbound"], p["outbound"]
    if not p["name"].strip():
        errs.append("이름을 입력하세요")
    if not _valid_listen_host(ib["host"]):
        errs.append("받는 쪽 주소는 IP 여야 합니다 (예: 127.0.0.1, 0.0.0.0, ::)")
    if not 1 <= ib["port"] <= 65535:
        errs.append("받는 쪽 포트가 잘못되었습니다")
    if ib["auth_enabled"] and ib["type"] in ("socks5", "http", "https"):
        if not (0 < len(ib["username"].encode()) <= 255 and 0 < len(ib["password"].encode()) <= 255):
            errs.append("인증을 켜려면 아이디와 비밀번호를 입력하세요 (각 255바이트 이하)")
    if ib["type"] == "shadowsocks":
        try:
            if not SS_METHODS[ib["ss_method"]][2] and len(ib["ss_password"]) < 8:
                raise ValueError("Shadowsocks 비밀번호는 8자 이상이어야 합니다")
            ss_make_key(ib["ss_method"], ib["ss_password"])
        except ValueError as e:
            errs.append(f"받는 쪽 {e}")
    if ib["type"] == "hysteria2" and len(ib["hy2_password"]) < 8:
        errs.append("Hysteria2 비밀번호는 8자 이상이어야 합니다")
    if ib["type"] in ("https", "hysteria2") and bool(ib["tls_cert"]) != bool(ib["tls_key"]):
        errs.append("인증서와 키 파일은 둘 다 지정하거나 둘 다 비워야 합니다 (비우면 자동 생성)")
    for f in ("tls_cert", "tls_key"):
        if ib["type"] in ("https", "hysteria2") and ib[f] and not os.path.isfile(ib[f]):
            errs.append(f"파일이 없습니다: {ib[f]}")
    if ob["type"] != "direct":
        if not ob["host"].strip() or not 1 <= ob["port"] <= 65535:
            errs.append("내보내는 쪽 서버 주소와 포트를 입력하세요")
        if ob["type"] == "shadowsocks":
            try:
                ss_make_key(ob["ss_method"], ob["ss_password"])
                if not ob["ss_password"]:
                    raise ValueError("Shadowsocks 비밀번호를 입력하세요")
            except ValueError as e:
                errs.append(f"내보내는 쪽 {e}")
        if ob["type"] == "hysteria2" and not ob["hy2_password"]:
            errs.append("내보내는 쪽 Hysteria2 비밀번호를 입력하세요")
        if ob["type"] == "socks4" and ob["password"]:
            errs.append("SOCKS4 는 비밀번호를 지원하지 않습니다 (아이디만 가능)")
    mine = _transports(ib["type"])
    for other in proxies:
        if other["id"] == p["id"]:
            continue
        oi = other["inbound"]
        shared = mine & _transports(oi["type"])
        if oi["port"] != ib["port"] or not shared:
            continue
        if any(_binds_overlap(ib["host"], oi["host"], udp=t == "udp") for t in shared):
            errs.append(f"'{other['name']}' 와 포트 {ib['port']} 가 겹칩니다")
    cycle = find_cycle(p, proxies)
    if cycle:
        errs.append("프록시 체인이 순환합니다: " + " → ".join(cycle) + " (내보내는 쪽 주소/포트를 확인하세요)")
    return errs


# --- 비밀번호 보관: Windows DPAPI, 그 외에는 사용자 전용 권한(0600) 파일 ---

class _DATA_BLOB(ctypes.Structure):
    _fields_ = [("cbData", ctypes.c_uint32), ("pbData", ctypes.POINTER(ctypes.c_char))]


def _dpapi(data, protect):
    crypt32 = ctypes.WinDLL("crypt32", use_last_error=True)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.LocalFree.argtypes = [ctypes.c_void_p]
    buf = ctypes.create_string_buffer(data, len(data))
    blob_in = _DATA_BLOB(len(data), ctypes.cast(buf, ctypes.POINTER(ctypes.c_char)))
    blob_out = _DATA_BLOB()
    fn = crypt32.CryptProtectData if protect else crypt32.CryptUnprotectData
    if not fn(ctypes.byref(blob_in), None, None, None, None, 0x1, ctypes.byref(blob_out)):
        raise OSError(ctypes.get_last_error(), "DPAPI 실패")
    try:
        return ctypes.string_at(blob_out.pbData, blob_out.cbData)
    finally:
        kernel32.LocalFree(ctypes.cast(blob_out.pbData, ctypes.c_void_p))


def _seal(d):
    """dict 안의 비밀번호 필드를 저장용으로 (Windows: *_dpapi)."""
    out = dict(d)
    if not IS_WIN:
        return out
    for k in SECRET_FIELDS:
        if k in out:
            v = out.pop(k)
            out[k + "_dpapi"] = base64.b64encode(_dpapi(v.encode("utf-8"), True)).decode() if v else ""
    return out


def _unseal(d, where):
    if not isinstance(d, dict):
        return d, False
    out = dict(d)
    legacy = False
    for k in SECRET_FIELDS:
        enc = out.pop(k + "_dpapi", None)
        if isinstance(enc, str) and enc:
            if not IS_WIN:
                out[k] = ""
                continue
            try:
                out[k] = _dpapi(base64.b64decode(enc), False).decode("utf-8")
            except (OSError, ValueError, UnicodeDecodeError):
                log(LOG_WARN, f"저장된 비밀번호({where} {k})를 복호화하지 못했습니다. 다시 입력하세요")
                out[k] = ""
        elif IS_WIN and out.get(k):
            legacy = True
    return out, legacy


# --- 읽기 / 쓰기 ---

def load():
    """(전역, 프록시 목록). 설정이 없으면 이전 버전 설정을 이전하고, 그것도 없으면 빈 목록으로 시작한다."""
    try:
        with open(CONFIG_PATH, encoding="utf-8") as f:
            raw = json.load(f)
    except FileNotFoundError:
        migrated = migrate_from_old()
        if migrated is not None:
            return migrated
        glob = validate_global({})
        save(glob, [])
        return glob, []
    except (OSError, ValueError) as e:
        # 깨진 설정은 나중 저장에 덮이지 않도록 옆에 보관해 둔다
        backup = CONFIG_PATH + ".broken"
        try:
            shutil.copy2(CONFIG_PATH, backup)
            log(LOG_WARN, f"설정 파일을 읽지 못해 기본값으로 시작합니다 ({e}). 원본은 {backup} 에 보관했습니다")
        except OSError:
            log(LOG_WARN, f"설정 파일을 읽지 못해 기본값으로 시작합니다: {e}")
        return validate_global({}), []
    raw = raw if isinstance(raw, dict) else {}
    glob = validate_global(raw.get("global"))
    proxies, legacy = [], False
    for item in raw.get("proxies") or []:
        if not isinstance(item, dict):
            continue
        item = dict(item)
        item["inbound"], l1 = _unseal(item.get("inbound"), "받는 쪽")
        item["outbound"], l2 = _unseal(item.get("outbound"), "내보내는 쪽")
        legacy = legacy or l1 or l2
        proxies.append(validate_proxy(item))
    if legacy:
        save(glob, proxies)
    return glob, proxies


def save(glob, proxies):
    try:
        data = {"version": 2, "global": glob,
                "proxies": [dict(p, inbound=_seal(p["inbound"]), outbound=_seal(p["outbound"])) for p in proxies]}
    except OSError as e:
        log(LOG_WARN, f"비밀번호 암호화 실패, 저장하지 않았습니다: {e}")
        return False
    try:
        os.makedirs(APP_DIR, exist_ok=True)
        tmp = CONFIG_PATH + ".tmp"
        if not IS_WIN:
            os.chmod(APP_DIR, 0o700)
            fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            os.close(fd)
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, indent=2)
        os.replace(tmp, CONFIG_PATH)
        return True
    except OSError as e:
        log(LOG_WARN, f"설정 저장 실패: {e}")
        return False


def migrate_from_old():
    """이전 버전(SOCKS5 Proxy / ProxyLocal) 설정을 전역 설정 + 프록시로 옮긴다. 원본은 남겨 둔다."""
    old_dir = app_dir(OLD_APP_ID)
    old_path = os.path.join(old_dir, "config.json")
    try:
        with open(old_path, encoding="utf-8") as f:
            old = json.load(f)
    except (OSError, ValueError):
        return None
    if not isinstance(old, dict):
        return None
    old, _ = _unseal(old, "이전 설정")
    glob = validate_global(old)
    proxies = [new_proxy("기본 SOCKS5", {
        "type": "socks5", "host": old.get("host", "127.0.0.1"), "port": old.get("port", 30307),
        "auth_enabled": old.get("auth_enabled", False), "username": old.get("username", ""),
        "password": old.get("password", ""),
    }, {"type": "direct"}, autostart=True)]
    if old.get("ss_enabled") and old.get("ss_password"):
        proxies.append(new_proxy("Shadowsocks", {
            "type": "shadowsocks", "host": old.get("host", "127.0.0.1"), "port": old.get("ss_port", 8388),
            "ss_method": old.get("ss_method", INBOUND_DEFAULTS["ss_method"]), "ss_password": old["ss_password"],
        }, {"type": "direct"}, autostart=True))
    os.makedirs(APP_DIR, exist_ok=True)
    for name in ("window.json",):
        src = os.path.join(old_dir, name)
        if os.path.exists(src) and not os.path.exists(os.path.join(APP_DIR, name)):
            shutil.copy2(src, os.path.join(APP_DIR, name))
    save(glob, proxies)
    log(LOG_INFO, f"이전 버전 설정을 가져왔습니다 ({old_dir}) - 프록시 {len(proxies)}개")
    return glob, proxies
