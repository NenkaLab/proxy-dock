"""Download the native engines the app runs (development / build machines; the installer will ship them):
sing-box and the official Hysteria2 core (used for Hysteria2 servers open to other PCs).

    python tools/fetch_singbox.py              # this OS/arch -> bin/<os>-<arch>/sing-box[.exe], hysteria[.exe]
    python tools/fetch_singbox.py linux-amd64  # another target

The versions are proxydock.singbox.SINGBOX_VERSION / HYSTERIA_VERSION. Every file is checked against the SHA-256
digest GitHub lists for the release asset before it is used.
"""
import hashlib
import io
import json
import os
import sys
import tarfile
import urllib.request
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
from proxydock.singbox import HYSTERIA_VERSION, SINGBOX_VERSION, platform_dir  # noqa: E402

API = f"https://api.github.com/repos/SagerNet/sing-box/releases/tags/v{SINGBOX_VERSION}"
HY_API = f"https://api.github.com/repos/apernet/hysteria/releases/tags/app%2Fv{HYSTERIA_VERSION}"


def _get(url):
    accept = "application/vnd.github+json" if url.startswith("https://api.github.com/") else "application/octet-stream"
    req = urllib.request.Request(url, headers={"User-Agent": "proxydock-build", "Accept": accept})
    with urllib.request.urlopen(req, timeout=60) as r:
        return r.read()


def _verified(release, name):
    asset = next((a for a in release["assets"] if a["name"] == name), None)
    if asset is None:
        sys.exit(f"{name} is not in the release")
    digest = (asset.get("digest") or "").removeprefix("sha256:")
    if not digest:
        sys.exit(f"GitHub lists no SHA-256 digest for {name}; not downloading unverified")
    print(f"downloading {name} ({asset['size'] / 1e6:.1f} MB)")
    data = _get(asset["browser_download_url"])
    got = hashlib.sha256(data).hexdigest()
    if got != digest:
        sys.exit(f"SHA-256 mismatch for {name}: expected {digest}, got {got}")
    return data


def fetch_hysteria(target):
    os_name = target.split("-", 1)[0]
    exe = "hysteria.exe" if os_name == "windows" else "hysteria"
    data = _verified(json.loads(_get(HY_API)), f"hysteria-{target}" + (".exe" if os_name == "windows" else ""))
    out_dir = os.path.join(ROOT, "bin", target)
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, exe)
    with open(path, "wb") as f:
        f.write(data)
    if os_name != "windows":
        os.chmod(path, 0o755)
    print(f"ok: {path} (sha256 verified)")


def fetch(target):
    os_name = target.split("-", 1)[0]
    ext = ".zip" if os_name == "windows" else ".tar.gz"
    name = f"sing-box-{SINGBOX_VERSION}-{target}{ext}"
    release = json.loads(_get(API))
    asset = next((a for a in release["assets"] if a["name"] == name), None)
    if asset is None:
        sys.exit(f"{name} is not in the v{SINGBOX_VERSION} release")
    digest = (asset.get("digest") or "").removeprefix("sha256:")
    if not digest:
        sys.exit(f"GitHub lists no SHA-256 digest for {name}; not downloading unverified")
    print(f"downloading {name} ({asset['size'] / 1e6:.1f} MB)")
    data = _get(asset["browser_download_url"])
    got = hashlib.sha256(data).hexdigest()
    if got != digest:
        sys.exit(f"SHA-256 mismatch for {name}: expected {digest}, got {got}")
    exe = "sing-box.exe" if os_name == "windows" else "sing-box"
    out_dir = os.path.join(ROOT, "bin", target)
    os.makedirs(out_dir, exist_ok=True)
    wanted = {exe, "LICENSE"}
    if ext == ".zip":
        with zipfile.ZipFile(io.BytesIO(data)) as z:
            for info in z.infolist():
                base = os.path.basename(info.filename)
                if base in wanted:
                    with open(os.path.join(out_dir, base), "wb") as f:
                        f.write(z.read(info))
    else:
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as t:
            for m in t.getmembers():
                base = os.path.basename(m.name)
                if m.isfile() and base in wanted:
                    with open(os.path.join(out_dir, base), "wb") as f:
                        f.write(t.extractfile(m).read())
    path = os.path.join(out_dir, exe)
    if os_name != "windows":
        os.chmod(path, 0o755)
    print(f"ok: {path} (sha256 of archive verified)")


if __name__ == "__main__":
    t = sys.argv[1] if len(sys.argv) > 1 else platform_dir()
    fetch(t)
    fetch_hysteria(t)
