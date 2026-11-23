# Building ProxyDock

[← README](../README.md)

## Run from source

Python 3.14:

```bash
python -m venv .venv
.venv\Scripts\activate            # macOS / Linux: source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py     # sing-box and Hysteria for this PC
python proxydock.py
```

## Tests

```bash
python tests/run_tests.py
```

## README images

```bash
python tools/readme_shots.py          # docs/images/{canvas,proxies,router}-{light,dark}.png
```

Draws a sample setup offscreen at 2x (nothing on this PC is changed).

## Build the app

| Target  | Command                   | Output                              | Builds on                          |
|---------|---------------------------|-------------------------------------|------------------------------------|
| Windows | `tools\build_win.bat`     | `dist/win/ProxyDock/ProxyDock.exe`  | Windows                            |
| Linux   | `tools/build_linux.sh`    | `dist/linux/ProxyDock/ProxyDock`    | Linux (Docker on Windows / macOS)  |
| macOS   | `tools/build_mac.sh`      | `dist/mac/ProxyDock.app`            | macOS                              |

The scripts call `tools/build.py` (`python tools/build.py win|linux|mac|all`). It makes a virtual environment per OS,
installs what `requirements.txt` needs, downloads sing-box and Hysteria (SHA-256 checked) and puts them inside the app.

## Installers

Made from an existing build, into `dist/release/` (each with a `.sha256` file):

- Windows installer: `python tools/build.py installer` (needs Inno Setup 6)
- macOS disk image: `python tools/build.py dmg` (on macOS)
- Linux AppImage: `python tools/build.py appimage` (on Linux)

## Releases

```bash
tools\release.bat <version>       # macOS / Linux: tools/release.sh <version>
```

This checks the tree and runs the tests, bumps the version, commits, tags `v<version>` and pushes.
GitHub Actions (`.github/workflows/release.yml`) then builds every platform and publishes the GitHub Release.
Add `--dry-run` to see what would happen.
