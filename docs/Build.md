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

## Releases

Set the version in `native/Cargo.toml` (`[workspace.package] version`), commit, then tag and push:

```powershell
git tag -s v3.0.0 -m "ProxyDock 3.0.0"
git push origin v3.0.0
```

GitHub Actions (`.github/workflows/release.yml`) runs the tests and builds the packages on Windows, Linux and macOS,
then publishes the GitHub Release with `SHA256SUMS.txt` (Linux and macOS are allowed to fail: not tried on real
machines yet). The tag must match the version. "Run workflow" builds without releasing.
