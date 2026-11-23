"""Native engines: each running proxy card is one sing-box process (compile.py, process.py, guard.py).

- A Hysteria2 server reachable from other PCs runs on the official Hysteria2 core instead (hysteria.py): sing-box
  does not report wrong passwords, so IP bans would not work; the official core asks ProxyDock about every login.
- The Python engine (runtime.ProxyInstance) still runs: certificate pinning (pin_sha256 is a whole-certificate hash;
  sing-box supports that only from 1.15), cards whose binary is missing, and the temporary bridge/check proxies of
  the connection test (ids starting with "__").
"""
from ..i18n import _t
import os
import shutil
import sys

from ..common import IS_WIN, SINGBOX_ONLY_OUTBOUNDS, is_loopback_host

SINGBOX_VERSION = "1.14.4"
HYSTERIA_VERSION = "2.14.0"         # official Hysteria2 core (apernet/hysteria, app/v...)
EXE = "sing-box.exe" if IS_WIN else "sing-box"
HY_EXE = "hysteria.exe" if IS_WIN else "hysteria"
FORCE_HYSTERIA = False              # tests: run loopback Hysteria2 servers on the official core too


def platform_dir():
    import platform
    m = platform.machine().lower()
    arch = {"x86_64": "amd64", "amd64": "amd64", "aarch64": "arm64", "arm64": "arm64"}.get(m, m)
    osname = "windows" if IS_WIN else "darwin" if sys.platform == "darwin" else "linux"
    return f"{osname}-{arch}"


def resource_path(rel):
    """Same as ui.common.resource_path (no Qt import here): PyInstaller folder or the repository root."""
    base = getattr(sys, "_MEIPASS", None) or os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    return os.path.join(base, rel)


def _find(exe, custom=""):
    if custom:
        return custom if os.path.isfile(custom) else None
    for rel in (os.path.join("bin", platform_dir(), exe), os.path.join("bin", exe)):
        p = resource_path(rel)
        if os.path.isfile(p):
            return p
    return shutil.which(os.path.splitext(exe)[0])


def find_binary(glob=None):
    """Path of the sing-box executable, or None. Order: setting -> bundled/dev bin/ folder -> PATH."""
    return _find(EXE, (glob or {}).get("singbox_path", ""))


def find_hysteria(glob=None):
    """Path of the official Hysteria2 core, or None (same search order as sing-box)."""
    return _find(HY_EXE, (glob or {}).get("hysteria_path", ""))


def parse_core_version(text):
    """Version number from `sing-box version` ("sing-box version 1.14.4") or `hysteria version`
    ("Version:\tv2.14.0"), or None."""
    import re
    m = re.search(r"version[:\s]+v?(\d+\.\d+\.\d+[\w.+-]*)", text or "", re.IGNORECASE)
    return m.group(1) if m else None


def core_version(exe, timeout=10):
    """Run `<exe> version` and return its version number, or None. Blocks: call it off the UI thread."""
    import subprocess
    if not exe:
        return None
    try:
        r = subprocess.run([exe, "version"], capture_output=True, text=True, errors="replace", timeout=timeout,
                           stdin=subprocess.DEVNULL, creationflags=0x08000000 if IS_WIN else 0)
    except (OSError, subprocess.SubprocessError):
        return None
    return parse_core_version(r.stdout + "\n" + r.stderr)


def _chain(ob):
    """Every outbound of a chain, including the chains of a group's members."""
    while ob:
        yield ob
        for m in ob.get("members") or ():
            yield from _chain(m["outbound"])
        ob = ob.get("via")


def needs_singbox(proxy):
    """Router rules and the Block outbound only exist as sing-box route rules; binding to a network interface is
    a sing-box dial option."""
    chains = [proxy["outbound"]] + [r["outbound"] for r in proxy.get("routes") or []]
    ib = proxy["inbound"]
    if ib["type"] == "hysteria2" and (ib.get("hy2_obfs_password") or ib.get("hy2_up_mbps") or ib.get("hy2_down_mbps")):
        return True             # Salamander / bandwidth: native engines only
    if any(ob["type"] == "hysteria2" and (ob.get("hy2_obfs_password") or ob.get("hy2_up_mbps") or ob.get("hy2_down_mbps")
                                          or (ob.get("hy2_ports") or "").strip())
           for c in chains for ob in _chain(c)):
        return True
    return (bool(proxy.get("routes")) or proxy["outbound"]["type"] == "block" or proxy["inbound"]["type"] in ("tun", "wireguard", "forward")
            or any(ob.get("interface") or ob["type"] in SINGBOX_ONLY_OUTBOUNDS for c in chains for ob in _chain(c)))


def needs_singbox_dns(proxy, glob):
    """A DNS server of its own (DoH, DoT, ...) and Fake-IP only exist in sing-box; the Python engine uses the PC's DNS."""
    from ..config import resolve
    return bool(resolve(proxy, glob).get("dns_server")) or bool(proxy["inbound"].get("tun_fakeip"))


def engine_for(proxy, glob):
    """(engine, reason): engine is "sing-box", "hysteria", "python" or "none" (cannot run: needs sing-box);
    reason says why it is not sing-box."""
    engine, reason = _engine_for(proxy, glob)
    if engine == "python" and reason != "internal" and (needs_singbox(proxy) or needs_singbox_dns(proxy, glob)):
        return "none", reason
    return engine, reason


def _engine_for(proxy, glob):
    if glob.get("engine", "sing-box") != "sing-box":
        return "python", _t("Python engine selected in Settings")
    if proxy["id"].startswith("__") and not needs_singbox(proxy):
        return "python", "internal"       # connection-test helpers (sing-box only when the outbound needs it)
    if any(ob.get("pin_sha256") for ob in _chain(proxy["outbound"]) if ob["type"] in ("https", "hysteria2")):
        return "python", _t("certificate pinning needs sing-box 1.15 or later")
    if find_binary(glob) is None:
        return "python", _t("sing-box was not found")
    ib = proxy["inbound"]
    if ib["type"] == "hysteria2" and (FORCE_HYSTERIA or not is_loopback_host(ib["host"])):
        if find_hysteria(glob) is None:
            return "python", _t("a Hysteria2 server open to other PCs needs the official Hysteria2 core, which was not found")
        return "hysteria", ""
    return "sing-box", ""


def python_reason(proxy, glob):
    """Why this proxy runs on the Python engine ("" = a native engine runs it)."""
    engine, reason = engine_for(proxy, glob)
    return reason if engine == "python" else ""
