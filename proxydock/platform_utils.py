"""OS 연동: 로그인 시 자동 실행, Windows 방화벽, 포트 확인, LAN IP, 폴더 열기."""
import ctypes
import errno
import os
import socket
import subprocess
import sys

from .common import APP_ID, APP_NAME, IS_MAC, IS_WIN, LOG_ERROR, LOG_INFO, log

if IS_WIN:
    import winreg


def local_lan_ip():
    """다른 PC 에서 접속할 때 쓸 이 PC 의 LAN IP (UDP connect 는 패킷을 보내지 않는다)."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("10.255.255.255", 1))
        return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"
    finally:
        s.close()


def check_port(host, port, udp=False):
    """(사용 가능 여부, 설명)."""
    bind_host = "127.0.0.1" if host == "localhost" else host
    try:
        fam = socket.AF_INET6 if ":" in bind_host else socket.AF_INET
        s = socket.socket(fam, socket.SOCK_DGRAM if udp else socket.SOCK_STREAM)
    except OSError as e:
        return False, str(e)
    try:
        if IS_WIN:      # 다른 프로그램과 포트를 공유하지 않도록 배타적으로 바인드해 본다
            s.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        s.bind((bind_host, port))
        return True, f"{bind_host}:{port} 를 사용할 수 있습니다."
    except OSError as e:
        code = getattr(e, "winerror", None)
        err = e.errno
        if code == 10048 or err == errno.EADDRINUSE:
            return False, "다른 프로그램(또는 실행 중인 다른 프록시)이 이미 이 포트를 쓰고 있습니다."
        if code == 10013:
            return False, "Windows 가 예약한 포트 범위이거나 접근이 거부되었습니다 (Hyper-V/WSL 예약 등)."
        if err == errno.EACCES:
            return False, "권한이 없습니다 (1024 미만 포트는 관리자 권한 필요)."
        if code == 10049 or err == errno.EADDRNOTAVAIL:
            return False, f"이 PC 에 {bind_host} 주소가 없습니다."
        return False, str(e)
    finally:
        s.close()


# --- Windows 방화벽 (관리자 권한으로 netsh 실행) ---

FW_RULE = f"{APP_NAME} - Block External"
FW_OLD_RULES = ("SOCKS5 Proxy - Block External", "Block SOCKS5 Proxy External", "Allow SOCKS5 Proxy Local",
                "Block SOCKS5 Proxy External UDP", "Allow SOCKS5 Proxy Local UDP")
CREATE_NO_WINDOW = 0x08000000


def firewall_rule_exists():
    try:
        r = subprocess.run(["netsh", "advfirewall", "firewall", "show", "rule", f"name={FW_RULE}"],
                           capture_output=True, creationflags=CREATE_NO_WINDOW, timeout=15)
        return r.returncode == 0
    except (OSError, subprocess.SubprocessError):
        return False


class _SHELLEXECUTEINFOW(ctypes.Structure):
    _fields_ = [
        ("cbSize", ctypes.c_uint32), ("fMask", ctypes.c_ulong), ("hwnd", ctypes.c_void_p),
        ("lpVerb", ctypes.c_wchar_p), ("lpFile", ctypes.c_wchar_p), ("lpParameters", ctypes.c_wchar_p),
        ("lpDirectory", ctypes.c_wchar_p), ("nShow", ctypes.c_int), ("hInstApp", ctypes.c_void_p),
        ("lpIDList", ctypes.c_void_p), ("lpClass", ctypes.c_wchar_p), ("hkeyClass", ctypes.c_void_p),
        ("dwHotKey", ctypes.c_uint32), ("hIconOrMonitor", ctypes.c_void_p), ("hProcess", ctypes.c_void_p),
    ]


def firewall_apply(block):
    """UAC 로 승격해 규칙을 추가/제거하고 끝날 때까지 기다린다. (성공 여부, 메시지)"""
    exe = sys.executable
    if any(c in exe for c in '"%^&|<>'):
        return False, "프로그램 경로에 특수 문자(\" % ^ & | < >)가 있어 규칙을 만들 수 없습니다. 다른 폴더로 옮겨 주세요"
    cmds = [f'netsh advfirewall firewall delete rule name="{n}" >nul 2>&1' for n in FW_OLD_RULES + (FW_RULE,)]
    if block:
        cmds.append(f'netsh advfirewall firewall add rule name="{FW_RULE}" dir=in action=block '
                    f'program="{exe}" enable=yes')
    params = '/s /c "' + " & ".join(cmds) + '"'
    info = _SHELLEXECUTEINFOW()
    info.cbSize = ctypes.sizeof(info)
    info.fMask = 0x00000040             # SEE_MASK_NOCLOSEPROCESS
    info.lpVerb = "runas"
    info.lpFile = "cmd.exe"
    info.lpParameters = params
    info.nShow = 0                      # SW_HIDE
    shell32 = ctypes.WinDLL("shell32", use_last_error=True)
    if not shell32.ShellExecuteExW(ctypes.byref(info)):
        err = ctypes.get_last_error()
        return False, "관리자 권한 요청이 취소되었습니다." if err == 1223 else f"실행 실패 (오류 {err})"
    if not info.hProcess:
        return True, "요청을 보냈습니다"
    k32 = ctypes.windll.kernel32
    k32.WaitForSingleObject(ctypes.c_void_p(info.hProcess), 30000)
    code = ctypes.c_uint32(1)
    k32.GetExitCodeProcess(ctypes.c_void_p(info.hProcess), ctypes.byref(code))
    k32.CloseHandle(ctypes.c_void_p(info.hProcess))
    if code.value != 0:
        return False, f"netsh 가 실패했습니다 (종료 코드 {code.value})"
    return True, ("외부 차단 규칙을 추가했습니다" if block else "외부 차단 규칙을 제거했습니다")


# --- 로그인 시 자동 실행 ---
# Windows: HKCU\...\Run 값, macOS: ~/Library/LaunchAgents/*.plist, Linux: ~/.config/autostart/*.desktop

REG_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
OLD_REG_NAME = "SOCKS5 Proxy"
MAC_AGENT = os.path.expanduser("~/Library/LaunchAgents/com.proxydock.app.plist")
OLD_MAC_AGENT = os.path.expanduser("~/Library/LaunchAgents/com.proxylocal.socks5.plist")
_XDG = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
LINUX_AUTOSTART = os.path.join(_XDG, "autostart", "proxydock.desktop")
OLD_LINUX_AUTOSTART = os.path.join(_XDG, "autostart", "proxylocal.desktop")


def _launch_argv(hidden):
    if getattr(sys, "frozen", False):
        argv = [sys.executable]
    else:
        exe = sys.executable
        if IS_WIN:      # 스크립트 실행이면 콘솔 창이 뜨지 않도록 pythonw.exe 사용
            w = os.path.join(os.path.dirname(exe), "pythonw.exe")
            exe = w if os.path.exists(w) else exe
        argv = [exe, os.path.abspath(sys.argv[0])]
    if hidden:
        argv.append("--hidden")
    return argv


def _read_startup(name=APP_ID, mac=MAC_AGENT, linux=LINUX_AUTOSTART):
    """등록된 자동 실행 명령 문자열, 없으면 None."""
    try:
        if IS_WIN:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, REG_KEY, 0, winreg.KEY_READ) as key:
                return winreg.QueryValueEx(key, name)[0]
        with open(mac if IS_MAC else linux, encoding="utf-8") as f:
            return f.read()
    except OSError:
        return None


def is_startup_registered():
    return _read_startup() is not None


def is_startup_hidden():
    return "--hidden" in (_read_startup() or "")


def update_startup_registry(register, hidden=False):
    try:
        if IS_WIN:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, REG_KEY, 0, winreg.KEY_SET_VALUE) as key:
                if register:
                    argv = _launch_argv(hidden)
                    cmd = f'"{argv[0]}" {subprocess.list2cmdline(argv[1:])}'.rstrip()
                    winreg.SetValueEx(key, APP_ID, 0, winreg.REG_SZ, cmd)
                else:
                    try:
                        winreg.DeleteValue(key, APP_ID)
                    except FileNotFoundError:
                        pass
            return True
        path = MAC_AGENT if IS_MAC else LINUX_AUTOSTART
        if not register:
            try:
                os.remove(path)
            except FileNotFoundError:
                pass
            return True
        argv = _launch_argv(hidden)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        if IS_MAC:
            from xml.sax.saxutils import escape as _x
            args = "".join(f"<string>{_x(a)}</string>" for a in argv)
            text = ('<?xml version="1.0" encoding="UTF-8"?>\n'
                    '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" '
                    '"http://www.apple.com/DTDs/PropertyList-1.0.dtd">\n'
                    '<plist version="1.0"><dict>'
                    '<key>Label</key><string>com.proxydock.app</string>'
                    f'<key>ProgramArguments</key><array>{args}</array>'
                    '<key>RunAtLoad</key><true/></dict></plist>\n')
        else:
            import shlex
            text = ("[Desktop Entry]\nType=Application\n"
                    f"Name={APP_NAME}\nExec={' '.join(shlex.quote(a) for a in argv)}\n"
                    "X-GNOME-Autostart-enabled=true\nTerminal=false\n")
        with open(path, "w", encoding="utf-8") as f:
            f.write(text)
        return True
    except OSError as e:
        log(LOG_ERROR, f"자동 실행 등록 오류: {e}")
        return False


def migrate_autostart():
    """이전 버전(SOCKS5 Proxy) 자동 실행 항목이 있으면 지우고 ProxyDock 으로 다시 등록 (숨김 실행 여부 유지)."""
    old = _read_startup(OLD_REG_NAME, OLD_MAC_AGENT, OLD_LINUX_AUTOSTART)
    if old is None:
        return
    hidden = "--hidden" in old
    try:
        if IS_WIN:
            with winreg.OpenKey(winreg.HKEY_CURRENT_USER, REG_KEY, 0, winreg.KEY_SET_VALUE) as key:
                winreg.DeleteValue(key, OLD_REG_NAME)
        else:
            os.remove(OLD_MAC_AGENT if IS_MAC else OLD_LINUX_AUTOSTART)
    except OSError:
        return
    if update_startup_registry(True, hidden=hidden):
        log(LOG_INFO, "이전 버전의 시작프로그램 등록을 ProxyDock 으로 옮겼습니다")


def open_path(path):
    """폴더/파일을 OS 기본 탐색기로 연다 (Windows 탐색기, macOS Finder, Linux 파일 관리자)."""
    from PySide6.QtCore import QUrl
    from PySide6.QtGui import QDesktopServices
    if not os.path.splitext(path)[1]:
        os.makedirs(path, exist_ok=True)
    if not QDesktopServices.openUrl(QUrl.fromLocalFile(path)):
        raise OSError(f"열 수 없습니다: {path}")
