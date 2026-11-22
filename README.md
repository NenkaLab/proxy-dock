# ProxyDock

**English** · [한국어](docs/README.ko.md) · [日本語](docs/README.ja.md) · [简体中文](docs/README.zh-CN.md) · [繁體中文](docs/README.zh-TW.md) · [Deutsch](docs/README.de.md) · [Français](docs/README.fr.md) · [Español](docs/README.es.md)

A desktop "dock" for proxies: create several proxies and turn each one on or off. Made for personal use.

Each proxy combines an **inbound** (how it accepts connections on a port of this PC) and an **outbound**
(how it reaches the destination). For example, "accept on a local SOCKS5 port → encrypted tunnel to a remote
Shadowsocks server" is a single card.

<p align="center">
  <img src="docs/images/proxies.png" width="49%" alt="Proxy list">
  <img src="docs/images/editor.png" width="49%" alt="Proxy editor">
</p>

## Features

| | Supported |
|---|---|
| Inbound | SOCKS5 · SOCKS4/4a · HTTP · HTTPS (TLS) · Shadowsocks server · Hysteria2 server |
| Outbound | Direct · upstream HTTP / HTTPS / SOCKS4 / SOCKS5 proxy · Shadowsocks tunnel · Hysteria2 (QUIC) tunnel |
| UDP | inbound SOCKS5 · Shadowsocks · Hysteria2, outbound Direct · Shadowsocks · Hysteria2 |

- **Shadowsocks**: AEAD 2018 (aes-128/256-gcm, chacha20-ietf-poly1305) and 2022 (SIP022, blake3), TCP+UDP, copy `ss://` links
- **Hysteria2**: interoperates both ways with the official implementation (TCP and UDP), certificate pinning, copy `hysteria2://` links
- **Settings**: global defaults + per-proxy local settings (local wins) - allowed domains, private network blocking, idle timeout, logging
- **Check connection** in the editor: tests the outbound (upstream/tunnel, passwords) and the inbound port before saving
- **Connection test**: connects to 21 sites in many countries over HTTPS through a running proxy and measures response times
- **Logs / statistics** per proxy, live throughput graph, optional log file
- **Security**: passwords stored with Windows DPAPI (0600 files on other OSes), per-IP connection limits,
  temporary bans after repeated authentication failures, proxy chain loop detection, Windows Firewall block button
- **Languages**: English (default), 한국어, 日本語, 简体中文, 繁體中文, Deutsch, Français, Español (Settings → Display → Language)
- Tray, start at login, light/dark theme, built-in fonts (Pretendard / JetBrains Mono)

## Running

Use a build from [Releases](https://github.com/NenkaLab/proxy-dock/releases) if there is one, or run from source:

```bash
python -m venv .venv
# Windows: .venv\Scripts\activate   /   macOS, Linux: source .venv/bin/activate
pip install -r requirements.txt
python proxydock.py            # --hidden : start in the tray without a window
```

Developed and tested with Python 3.14.

There are no proxies on first launch. Pick a template under **New proxy** (Local SOCKS5, Local HTTP,
Shadowsocks server, Local → Shadowsocks tunnel, ...) and start it with the switch on its card.
The **How to use** page in the app has the details.

Settings location

| OS | Path |
|---|---|
| Windows | `%APPDATA%\ProxyDock` |
| macOS | `~/Library/Application Support/ProxyDock` |
| Linux | `$XDG_CONFIG_HOME/ProxyDock` (default `~/.config/ProxyDock`) |

## Building

Everything for building lives in [`tools/`](tools/). PyInstaller produces a single executable (an `.app` on macOS).
The scripts create a virtual environment per OS, install only the packages that differ from `requirements.txt`,
and build with `tools/ProxyDock.spec`. They can be run from any folder.

| Command | Output |
|---|---|
| `tools\build_win.bat` | `dist/win/ProxyDock.exe` (on Windows) |
| `tools\build_linux.bat` / `tools/build_linux.sh` | `dist/linux/ProxyDock` (natively on Linux, with Docker on Windows/macOS) |
| `tools/build_mac.sh` | `dist/mac/ProxyDock.app` (on macOS only) |
| `tools\build.bat` / `tools/build.sh` | everything this computer can build, with a summary |

The macOS build has not been tried on a real Mac yet.

## Known limitations

- Hysteria2 runs on a Python QUIC implementation (aioquic), so expect about 100 Mbps. Bandwidth settings (Brutal),
  Salamander obfuscation and port hopping are not supported.
- The official Hysteria2 server rejects connections whose SNI does not match its certificate - put the server
  certificate's domain into the outbound SNI field.
- The HTTP inbound handles one plain (non-CONNECT) request per connection.
- SOCKS4 has no password authentication, so only use it on this PC.
- Japanese and Chinese use the system UI font (the built-in Pretendard covers Korean and Latin).

## Layout

```
proxydock.py            entry point
proxydock/
  common.py             shared constants, logging, address handling, bans
  config.py             settings format (v2), validation, global/local merge, password storage, migration
  runtime.py            ProxyInstance / ProxyManager (many proxies on one asyncio thread)
  relay.py              TCP/UDP relay core
  inbound.py            inbound protocols
  outbound.py           outbound protocols
  ss.py                 Shadowsocks 2018/2022 encryption
  hysteria2.py          Hysteria2 server and client (aioquic)
  tls.py                self-signed certificates, TLS contexts
  probe.py              connection test / connection check
  platform_utils.py     start at login, firewall, port check
  i18n.py, locales/     translations (English source strings -> <language>.json)
  ui/                   PySide6 + Fluent Widgets UI
tools/                  build scripts, PyInstaller spec, Linux build Dockerfile
licenses/               licenses of the bundled third-party components
docs/                   translated READMEs, screenshots
```

### Translations

UI strings are written in English in the code (`_t("...")`) and translated through
`proxydock/locales/<language>.json`. A missing entry falls back to English.

## License

[GPL-3.0](LICENSE). The UI library [PySide6-Fluent-Widgets](https://github.com/zhiyiYo/PyQt-Fluent-Widgets) is
GPL v3, so this program is distributed under GPL v3 as well; distributing built executables must follow the GPL v3
terms (providing the source, etc.).

Licenses of the bundled fonts and libraries are in [`licenses/`](licenses/).

| Component | License |
|---|---|
| Pretendard, JetBrains Mono (bundled fonts) | SIL OFL 1.1 |
| PySide6-Fluent-Widgets | GPL v3 |
| Qt 6 / PySide6 / Shiboken6, PySideSix-Frameless-Window | LGPL v3 |
| aioquic, pylsqpack, darkdetect | BSD 3-Clause |
| cryptography | Apache 2.0 / BSD 3-Clause |
| pyOpenSSL | Apache 2.0 |
| service-identity, attrs | MIT |
| certifi | MPL 2.0 |
| Python runtime | PSF License |
| PyInstaller bootloader | GPL v2+ (bootloader exception) |

Copyright (C) 2026 NenkaLab
