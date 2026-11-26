# Building ProxyDock

[← README](../README.md)

ProxyDock 3 is written in Rust (`native/`): `proxydock.exe` is the tray and the proxy core, `proxydock-ui.exe` the
window (Slint). It is Windows-only for now. The Python app it replaced (2.x) is on the
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

Builds with `cargo build --release`, then puts the program folder in `dist/rust/ProxyDock` (ProxyDock.exe,
proxydock-ui.exe, the engines and licenses) and into `dist/release-rust/` a `.zip` and the installer
`ProxyDock-<version>-windows-x64-setup.exe` (each with a `.sha256` file). The installer needs Inno Setup 6
(`winget install JRSoftware.InnoSetup`); `--no-installer` skips it. The version is `native/Cargo.toml`'s.

The installer installs for the current user (no administrator needed), over a 2.x install, and keeps the settings in
`%APPDATA%\ProxyDock`.

## Releases

Set the version in `native/Cargo.toml` (`[workspace.package] version`), commit, then tag and push:

```powershell
git tag -s v3.0.0 -m "ProxyDock 3.0.0"
git push origin v3.0.0
```

GitHub Actions (`.github/workflows/release.yml`) runs the tests, builds the installer and the zip and publishes the
GitHub Release with `SHA256SUMS.txt`. The tag must match the version. "Run workflow" builds without releasing.
