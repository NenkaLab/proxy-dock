"""OS integration: start at login, Windows Firewall, port check, LAN IP, opening folders."""
from .i18n import _t
import ctypes
import errno
import os
import socket
import subprocess
import sys

from .common import APP_ID, APP_NAME, IS_MAC, IS_WIN, LOG_ERROR, LOG_INFO, log, self_exe

if IS_WIN:
    import winreg


def local_lan_ip():
    """LAN IP of this PC for connecting from other PCs (a UDP connect sends no packets)."""
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("10.255.255.255", 1))
        return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"
    finally:
        s.close()


def _is_default_route(dest, mask):
    """IPv4 dest/mask as the raw network-order DWORD both Windows and /proc/net/route use:
    0.0.0.0/0, or one half of the 0.0.0.0/1 + 128.0.0.0/1 pair VPN apps install instead."""
    return mask == 0 or (mask == 0x80 and dest in (0, 0x80))


def default_route_ifaces():
    """Interfaces with an IPv4 route to the internet (a default route), as a set of interface indexes
    (Windows) or names (macOS/Linux) matching QNetworkInterface.index()/name(). None = could not tell."""
    try:
        if IS_WIN:
            from ctypes import wintypes

            class Row(ctypes.Structure):        # MIB_IPFORWARDROW
                _fields_ = [(n, wintypes.DWORD) for n in (
                    "dest", "mask", "policy", "next_hop", "if_index", "type", "proto", "age",
                    "next_hop_as", "m1", "m2", "m3", "m4", "m5")]
            fn = ctypes.windll.iphlpapi.GetIpForwardTable
            size = wintypes.ULONG(0)
            fn(None, ctypes.byref(size), False)
            for _ in range(3):                  # the table can grow between the two calls
                buf = ctypes.create_string_buffer(size.value)
                rc = fn(buf, ctypes.byref(size), False)
                if rc == 0:
                    break
                if rc != 122:                   # ERROR_INSUFFICIENT_BUFFER
                    return None
            else:
                return None
            n = wintypes.DWORD.from_buffer(buf).value
            rows = (Row * n).from_buffer(buf, ctypes.sizeof(wintypes.DWORD))
            return {r.if_index for r in rows if _is_default_route(r.dest, r.mask)}
        if IS_MAC:
            out = subprocess.run(["netstat", "-rn", "-f", "inet"], capture_output=True, text=True, timeout=5).stdout
            found, col = set(), None
            for line in out.splitlines():
                f = line.split()
                if f and f[0] == "Destination" and "Netif" in f:    # the column moved between macOS versions
                    col = f.index("Netif")
                elif col is not None and len(f) > col and f[0] in ("default", "0/1", "128.0/1"):
                    found.add(f[col])
            return found if col is not None else None
        found = set()
        with open("/proc/net/route") as fh:
            for line in fh.readlines()[1:]:
                f = line.split()
                if len(f) >= 8 and _is_default_route(int(f[1], 16), int(f[7], 16)):
                    found.add(f[0])
        return found
    except (OSError, ValueError, AttributeError, subprocess.SubprocessError):
        return None


def running_programs():
    """Sorted names of the programs running now (e.g. chrome.exe), for router program rules."""
    names = set()
    try:
        if IS_WIN:
            out = subprocess.run(["tasklist", "/fo", "csv", "/nh"], capture_output=True, text=True, timeout=10,
                                 creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)).stdout
            for line in out.splitlines():
                name = line.split('","', 1)[0].strip('"')
                if name.lower().endswith(".exe"):
                    names.add(name)
        else:
            out = subprocess.run(["ps", "-A", "-o", "comm="], capture_output=True, text=True, timeout=10).stdout
            names = {os.path.basename(x.strip()) for x in out.splitlines() if x.strip()}
    except (OSError, subprocess.SubprocessError):
        pass
    skip = {"system idle process", "system", "registry", "smss.exe", "csrss.exe", "wininit.exe", "services.exe",
            "lsass.exe", "svchost.exe", "fontdrvhost.exe", "dwm.exe", "memory compression", "conhost.exe", "tasklist.exe"}
    return sorted((n for n in names if n.lower() not in skip), key=str.lower)


def check_port(host, port, udp=False):
    """(available, description)."""
    bind_host = "127.0.0.1" if host == "localhost" else host
    try:
        fam = socket.AF_INET6 if ":" in bind_host else socket.AF_INET
        s = socket.socket(fam, socket.SOCK_DGRAM if udp else socket.SOCK_STREAM)
    except OSError as e:
        return False, str(e)
    try:
        if IS_WIN:      # bind exclusively so the port is not shared with another program
            s.setsockopt(socket.SOL_SOCKET, socket.SO_EXCLUSIVEADDRUSE, 1)
        s.bind((bind_host, port))
        return True, _t("{bind_host}:{port} is available.").format(bind_host=bind_host, port=port)
    except OSError as e:
        code = getattr(e, "winerror", None)
        err = e.errno
        if code == 10048 or err == errno.EADDRINUSE:
            return False, _t("Another program (or another running proxy) is already using this port.")
        if code == 10013:
            return False, _t("The port is in a range reserved by Windows, or access was denied (Hyper-V/WSL reservations etc.).")
        if err == errno.EACCES:
            return False, _t("Permission denied (ports below 1024 need administrator rights).")
        if code == 10049 or err == errno.EADDRNOTAVAIL:
            return False, _t("This PC does not have the address {bind_host}.").format(bind_host=bind_host)
        return False, str(e)
    finally:
        s.close()


# --- Windows Firewall (netsh with administrator rights) ---

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
    """Elevate through UAC, add/remove the rule and wait for it to finish. (success, message)"""
    exe = sys.executable
    if any(c in exe for c in '"%^&|<>'):
        return False, _t("The program path contains special characters (\" % ^ & | < >), so the rule cannot be created. Move it to another folder")
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
        return False, _t("The administrator permission request was cancelled.") if err == 1223 else _t("Failed to run (error {err})").format(err=err)
    if not info.hProcess:
        return True, _t("Request sent")
    k32 = ctypes.windll.kernel32
    k32.WaitForSingleObject(ctypes.c_void_p(info.hProcess), 30000)
    code = ctypes.c_uint32(1)
    k32.GetExitCodeProcess(ctypes.c_void_p(info.hProcess), ctypes.byref(code))
    k32.CloseHandle(ctypes.c_void_p(info.hProcess))
    if code.value != 0:
        return False, _t("netsh failed (exit code {value})").format(value=code.value)
    return True, (_t("Added the external block rule") if block else _t("Removed the external block rule"))


# --- start at login ---
# Windows: HKCU\...\Run value, macOS: ~/Library/LaunchAgents/*.plist, Linux: ~/.config/autostart/*.desktop

REG_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
OLD_REG_NAME = "SOCKS5 Proxy"
MAC_AGENT = os.path.expanduser("~/Library/LaunchAgents/com.proxydock.app.plist")
OLD_MAC_AGENT = os.path.expanduser("~/Library/LaunchAgents/com.proxylocal.socks5.plist")
_XDG = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
LINUX_AUTOSTART = os.path.join(_XDG, "autostart", "proxydock.desktop")
OLD_LINUX_AUTOSTART = os.path.join(_XDG, "autostart", "proxylocal.desktop")


def _launch_argv(hidden):
    if getattr(sys, "frozen", False):
        argv = [self_exe()]
    else:
        exe = sys.executable
        if IS_WIN:      # use pythonw.exe when running as a script so no console window appears
            w = os.path.join(os.path.dirname(exe), "pythonw.exe")
            exe = w if os.path.exists(w) else exe
        argv = [exe, os.path.abspath(sys.argv[0])]
    if hidden:
        argv.append("--hidden")
    return argv


def _read_startup(name=APP_ID, mac=MAC_AGENT, linux=LINUX_AUTOSTART):
    """Registered autostart command, or None."""
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
        log(LOG_ERROR, _t("Startup registration error: {e}").format(e=e))
        return False


def migrate_autostart():
    """If the previous version (SOCKS5 Proxy) has an autostart entry, remove it and register ProxyDock (keeping hidden start)."""
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
        log(LOG_INFO, _t("Moved the previous version's startup entry to ProxyDock"))


def open_path(path):
    """Open a folder/file in the OS file manager (Explorer, Finder, Linux file manager)."""
    from PySide6.QtCore import QUrl
    from PySide6.QtGui import QDesktopServices
    if not os.path.splitext(path)[1]:
        os.makedirs(path, exist_ok=True)
    if not QDesktopServices.openUrl(QUrl.fromLocalFile(path)):
        raise OSError(_t("Cannot open: {path}").format(path=path))


# --- system proxy (the "PC" block): Windows WinINet settings, used by browsers and most apps ---

INET_KEY = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings"
INET_VALUES = ("ProxyEnable", "ProxyServer", "ProxyOverride", "AutoConfigURL")
PC_INBOUND_TYPES = ("http", "socks4")       # what WinINet can talk to (its "socks=" is SOCKS4)


def system_proxy_supported():
    return IS_WIN


def system_proxy_server(inbound):
    """(ProxyServer value, error) for using this inbound as the system proxy."""
    if inbound["type"] not in PC_INBOUND_TYPES:
        return None, _t("Windows can only use an HTTP (or SOCKS4) inbound as the system proxy - add an HTTP inbound for the PC")
    host = {"0.0.0.0": "127.0.0.1", "::": "::1"}.get(inbound["host"], inbound["host"])
    hp = f"[{host}]:{inbound['port']}" if ":" in host else f"{host}:{inbound['port']}"
    return (hp if inbound["type"] == "http" else f"socks={hp}"), None


def get_system_proxy():
    """Current WinINet proxy values (missing value = None)."""
    out = {name: None for name in INET_VALUES}
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, INET_KEY, 0, winreg.KEY_READ) as key:
        for name in INET_VALUES:
            try:
                out[name] = winreg.QueryValueEx(key, name)[0]
            except FileNotFoundError:
                pass
    return out


def set_system_proxy(values):
    """Write WinINet proxy values (None = delete the value) and tell running apps about it."""
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, INET_KEY, 0, winreg.KEY_SET_VALUE) as key:
        for name in INET_VALUES:
            if name not in values:
                continue
            v = values[name]
            if v is None:
                try:
                    winreg.DeleteValue(key, name)
                except FileNotFoundError:
                    pass
            elif name == "ProxyEnable":
                winreg.SetValueEx(key, name, 0, winreg.REG_DWORD, int(v))
            else:
                winreg.SetValueEx(key, name, 0, winreg.REG_SZ, str(v))
    _notify_inet_change()


def _notify_inet_change():
    try:
        inet = ctypes.windll.wininet
        inet.InternetSetOptionW(None, 39, None, 0)      # INTERNET_OPTION_SETTINGS_CHANGED
        inet.InternetSetOptionW(None, 37, None, 0)      # INTERNET_OPTION_REFRESH
    except (AttributeError, OSError):
        pass


def proxy_values_for(server, backup):
    """Values to apply: our server, bypass local addresses (keeping the user's own exceptions), no PAC script."""
    bypass = ["localhost", "127.*", "<local>"]
    old = (backup or {}).get("ProxyOverride") or ""
    for item in old.split(";"):
        if item.strip() and item.strip() not in bypass:
            bypass.insert(len(bypass) - 1, item.strip())
    return {"ProxyEnable": 1, "ProxyServer": server, "ProxyOverride": ";".join(bypass), "AutoConfigURL": None}
