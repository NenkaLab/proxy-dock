"""Administrator helper for TUN cards (Windows UAC).

A TUN device needs administrator rights, but the ProxyDock UI should not run elevated. When a TUN card starts and
ProxyDock is not elevated, it starts `ProxyDock --tun-helper <port> <token> <config> <dir>` through UAC ("runas").
The helper connects back to 127.0.0.1:<port>, proves itself with <token>, runs the card's sing-box (only the
bundled binary, never a path from the settings) and streams its output back. "STOP" or a closed connection (the
UI quit or crashed) stops sing-box, and the helper exits with it.

Parent side: HelperProc, used by SingBoxInstance in place of an asyncio subprocess (same stdout/wait/terminate).
"""
from ..i18n import _t
import asyncio
import os
import secrets
import socket
import subprocess
import sys
import threading

from ..common import IS_WIN

EXIT_MARK = b"\x00PROXYDOCK-EXIT "
CONNECT_TIMEOUT = 60            # the user may take a while to answer the UAC prompt


def helper_command(port, token, config_path, workdir):
    return app_command("--tun-helper", str(port), token, config_path, workdir)


def app_command(*extra):
    """Command line that starts ProxyDock itself (frozen exe, or python proxydock.py while developing)."""
    if getattr(sys, "frozen", False):
        return [sys.executable, *extra]
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    return [sys.executable, os.path.join(root, "proxydock.py"), *extra]


def runas(args, show=False):
    """Start args[0] elevated (UAC). Raises RuntimeError if the user says no."""
    return _runas(args, show)


def _runas(args, show=False):
    """Start args[0] elevated (UAC). Raises RuntimeError if the user says no."""
    if not IS_WIN:
        raise RuntimeError(_t("A TUN card needs ProxyDock to run as root"))
    import ctypes
    from ctypes import wintypes

    class SHELLEXECUTEINFOW(ctypes.Structure):
        _fields_ = [("cbSize", wintypes.DWORD), ("fMask", ctypes.c_ulong), ("hwnd", wintypes.HWND),
                    ("lpVerb", wintypes.LPCWSTR), ("lpFile", wintypes.LPCWSTR), ("lpParameters", wintypes.LPCWSTR),
                    ("lpDirectory", wintypes.LPCWSTR), ("nShow", ctypes.c_int), ("hInstApp", wintypes.HINSTANCE),
                    ("lpIDList", ctypes.c_void_p), ("lpClass", wintypes.LPCWSTR), ("hkeyClass", wintypes.HKEY),
                    ("dwHotKey", wintypes.DWORD), ("hIconOrMonitor", wintypes.HANDLE), ("hProcess", wintypes.HANDLE)]
    info = SHELLEXECUTEINFOW()
    info.cbSize = ctypes.sizeof(info)
    info.fMask = 0x00000040 | 0x00000100            # SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC
    info.lpVerb = "runas"
    info.lpFile = args[0]
    info.lpParameters = subprocess.list2cmdline(args[1:])
    info.nShow = 1 if show else 0                   # SW_SHOWNORMAL / SW_HIDE
    shell32 = ctypes.WinDLL("shell32", use_last_error=True)
    if not shell32.ShellExecuteExW(ctypes.byref(info)):
        err = ctypes.get_last_error()
        if err == 1223:                             # ERROR_CANCELLED
            raise RuntimeError(_t("Administrator permission was not given (the UAC prompt was cancelled)"))
        raise RuntimeError(_t("Could not start the administrator helper (error {code})").format(code=err))
    if info.hProcess:
        ctypes.windll.kernel32.CloseHandle(info.hProcess)


class _Stdout:
    """Reads the helper's stream like a process stdout; the exit mark ends it and sets the return code."""

    def __init__(self, proc, reader):
        self.proc, self.reader = proc, reader

    async def readline(self):
        line = await self.reader.readline()
        if line.startswith(EXIT_MARK):
            try:
                self.proc.returncode = int(line[len(EXIT_MARK):].strip() or 1)
            except ValueError:
                self.proc.returncode = 1
            return b""
        if not line and self.proc.returncode is None:
            self.proc.returncode = -1               # connection lost
        return line


class HelperProc:
    pid = None

    def __init__(self, reader, writer):
        self.returncode = None
        self.writer = writer
        self.stdout = _Stdout(self, reader)

    @classmethod
    async def launch(cls, config_path, workdir, elevate=True):
        token = secrets.token_hex(16)
        got = asyncio.get_running_loop().create_future()

        async def on_conn(reader, writer):
            try:
                line = await asyncio.wait_for(reader.readline(), 10)
            except (asyncio.TimeoutError, OSError):
                line = b""
            if not got.done() and secrets.compare_digest(line.strip(), token.encode()):
                got.set_result((reader, writer))
            else:
                writer.close()

        server = await asyncio.start_server(on_conn, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        args = helper_command(port, token, config_path, workdir)
        try:
            if elevate:
                await asyncio.get_running_loop().run_in_executor(None, _runas, args)
            else:                                   # tests: same protocol without UAC
                subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                 creationflags=0x08000000 if IS_WIN else 0)
            try:
                reader, writer = await asyncio.wait_for(got, CONNECT_TIMEOUT)
            except asyncio.TimeoutError:
                raise RuntimeError(_t("The administrator helper did not connect")) from None
        finally:
            server.close()
        return cls(reader, writer)

    def terminate(self):
        try:
            self.writer.write(b"STOP\n")
        except Exception:
            pass

    def kill(self):
        try:
            self.writer.close()
        except Exception:
            pass

    async def wait(self):
        while self.returncode is None:
            line = await self.stdout.readline()
            if not line:
                break
        return self.returncode


# --- helper side (runs elevated, no Qt) ---

def main(argv):
    port, token, config_path, workdir = argv[:4]
    from . import find_binary
    from .process import _attach_to_job
    sock = socket.create_connection(("127.0.0.1", int(port)), 10)
    sock.sendall(token.encode() + b"\n")
    exe = find_binary(None)             # the bundled/dev binary only: never a path from the (unelevated) settings
    if exe is None:
        sock.sendall(b"FATAL[0000] sing-box was not found\n" + EXIT_MARK + b"1\n")
        sock.close()
        return 1
    kw = {"creationflags": 0x08000000} if IS_WIN else {}
    p = subprocess.Popen([exe, "run", "--disable-color", "-c", config_path, "-D", workdir],
                         stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, **kw)
    _attach_to_job(p.pid)               # sing-box also ends if this helper is killed
    lock = threading.Lock()

    def forward():
        for line in iter(p.stdout.readline, b""):
            with lock:
                try:
                    sock.sendall(line)
                except OSError:
                    break

    t = threading.Thread(target=forward, daemon=True)
    t.start()

    def control():
        buf = b""
        while True:
            try:
                data = sock.recv(4096)
            except OSError:
                data = b""
            buf += data
            if not data or b"STOP" in buf:
                break
        if p.poll() is None:
            p.terminate()
            try:
                p.wait(3)
            except subprocess.TimeoutExpired:
                p.kill()

    threading.Thread(target=control, daemon=True).start()
    code = p.wait()
    t.join(2)
    with lock:
        try:
            sock.sendall(EXIT_MARK + str(code).encode() + b"\n")
        except OSError:
            pass
    sock.close()
    return code
