# Building ProxyDock

[← README](../README.md)

ProxyDock 3 is written in Rust (`native/`): `proxydock.exe` is the tray and the proxy core, `proxydock-ui.exe` the
window (Slint). Windows is the tried system; Linux and macOS build (the tray uses the system's own menu there)
but are not tried on real machines yet. The Python app it replaced (2.x) is on the
[`old`](https://github.com/NenkaLab/proxy-dock/tree/old) branch.

## Run from source

Rust (the version in `native/rust-toolchain.toml` is installed by rustup on the first build) and Python 3 (for the
engine download script only):

```powershell
python tools\fetch_singbox.py     # sing-box and Hysteria for this PC, into bin\windows-amd64 (SHA-256 checked)
cd native
cargo build -p proxydock          # both programs: the tray starts the window next to itself
.\target\debug\proxydock.exe
```

`PROXYDOCK_HOME=<folder>` keeps the settings in another folder (handy for trying things without touching your own).

## Tests

```powershell
cd native
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Comparing the Rust core with the Python app (random settings, links and edits) and the checks done by hand before a
release: [TESTING.md](../TESTING.md) (Korean).

## Build the app

```powershell
python tools\build_rust.py
```

Builds with `cargo build --release` for the system it runs on and puts the result into `dist/release-rust/`, each
file with a `.sha256` (the same names as the 2.x packages):

| System | Program | Packages |
| --- | --- | --- |
| Windows | `dist/rust/ProxyDock` | `ProxyDock-<version>-windows-x64.zip`, `-setup.exe` (Inno Setup 6: `winget install JRSoftware.InnoSetup`) |
| Linux | `dist/rust/ProxyDock` | `ProxyDock-<version>-linux-<arch>.tar.gz`, `.AppImage` (appimagetool, downloaded if missing) |
| macOS | `dist/rust/ProxyDock.app` | `ProxyDock-<version>-macos-<arch>.zip`, `.dmg` (hdiutil) |

`--no-installer` skips the installer / AppImage / disk image. The version is `native/Cargo.toml`'s. Linux needs the
window's and the tray's libraries to build (Debian / Ubuntu: `pkg-config libfontconfig1-dev libfreetype-dev
libxkbcommon-dev libwayland-dev libgtk-3-dev libayatana-appindicator3-dev libxdo-dev`).

The Windows installer installs for the current user (no administrator needed), over a 2.x install, and keeps the
settings in `%APPDATA%\ProxyDock`.

## Direct line (VM images and checks)

```powershell
python toolsuild_bypass_image.py --arch x64 --test
python toolsuild_bypass_image.py --arch arm64
```

Needs Docker (arm64 runs emulated on an x64 PC). Builds the small Linux guest that lets a block go straight out
through a network card past a VPN: `pd-agent` (`native/bypass-guest`, static musl, `--test` runs its tests first), an
Alpine `linux-virt` kernel with a busybox initramfs and the musl build of sing-box (fetched into
`bin/linux-<arch>-musl`) on a read-only squashfs.

- **x64** (Windows Hyper-V, bridged to a physical card through an External switch): a disk with GRUB on an EFI
  partition -> `dist/bypass/proxydock-bypass-x64.vhdx` and `proxydock-bypass-x64.zip`.
- **arm64** (macOS Virtualization.framework on Apple Silicon, which boots Linux itself): the uncompressed kernel
  Image, the initramfs and the squashfs -> `dist/bypass/proxydock-bypass-arm64.zip`. The app bundle is signed
  (ad hoc) with the `com.apple.security.virtualization` entitlement (`tools/build_rust.py`). Not tried on a Mac
  yet; the guest itself boots in QEMU (`qemu-system-aarch64 -M virt`).
- **Linux** needs no image: the helper makes a network namespace with a macvlan (Wi-Fi: ipvlan) copy of the card
  and runs sing-box in it.

Each file gets a `.sha256` and `manifest-<arch>.json`. The installers do not include them: the release workflow
builds both on Linux and attaches the zips, and ProxyDock downloads its own version's the first time a VM is
prepared (checked against `SHA256SUMS.txt`). A development build finds `dist/bypass` above the program instead, or
the file `PROXYDOCK_BYPASS_IMAGE` names.

Inside the VM, `pd-agent` answers on vsock port 20548 (Hyper-V sockets / virtio-vsock; line JSON: `status`,
`net` = DHCP or a fixed address, `socks_auth`, `poweroff`) and relays every stream on port 20549 to the VM's sing-box
SOCKS5 server (127.0.0.1 only, with a random account ProxyDock sets at each start), which ProxyDock's sing-box uses
with UDP over TCP. On the host, `proxydock --bypass-helper` (elevated once per run: UAC, pkexec or the macOS
password prompt) creates the VMs (or namespaces) and relays a 127.0.0.1 port to each. A Hyper-V VM starts on a fresh
differencing disk each time; the image itself is never written. A VM needs at least 192 MB of memory (less and the
kernel cannot unpack itself); the app offers 200 MB and up.

End to end on a PC with Hyper-V (asks for UAC, creates and then removes a switch and a VM):

```powershell
$env:PROXYDOCK_BYPASS_ADAPTER = "Wi-Fi"; cargo run -p proxydock-core --example bypass_e2e
```

Linux, in a privileged container (a firewall rule blocks the container's own namespace, as a VPN's kill switch
would; the Direct line must still get out): see the top of `tools/docker_linux_bypass_check.sh`.

## Releases

Set the version in `native/Cargo.toml` (`[workspace.package] version`), commit, then tag and push:

```powershell
git tag -s v3.0.0 -m "ProxyDock 3.0.0"
git push origin v3.0.0
```

GitHub Actions (`.github/workflows/release.yml`) runs the tests and builds the packages on Windows, Linux and macOS,
then publishes the GitHub Release with `SHA256SUMS.txt` (Linux and macOS are allowed to fail: not tried on real
machines yet). The tag must match the version. "Run workflow" builds without releasing.
