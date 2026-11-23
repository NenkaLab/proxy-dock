# ProxyDock

**English** · [한국어](docs/README.ko.md) · [日本語](docs/README.ja.md) · [简体中文](docs/README.zh-CN.md) · [繁體中文](docs/README.zh-TW.md) · [Deutsch](docs/README.de.md) · [Français](docs/README.fr.md) · [Español](docs/README.es.md)

A desktop app that runs several proxies and tunnels side by side. Link **inbound** blocks (how traffic enters,
e.g. a local SOCKS5 port) to **outbound** blocks (how it leaves, e.g. a Shadowsocks server) on a canvas, and
turn each card on or off.

<p align="center">
  <img src="docs/images/canvas.png" width="98%" alt="Canvas">
</p>

## Features

- **Inbound**: SOCKS5, SOCKS4, HTTP, HTTPS, Shadowsocks server, Hysteria2 server, WireGuard server (phones and PCs connect with the WireGuard app), TUN (all apps on this PC)
- **Outbound**: Direct, HTTP / HTTPS / SOCKS proxy, Shadowsocks, Hysteria2, WireGuard, VLESS (REALITY), VMess,
  Trojan, TUIC, subscriptions (share links or Clash YAML)
- **Blocks**: chains (one outbound through another), routers (rules by domain, IP, port, program - pick from running programs -, GeoIP/Geosite; simple and advanced views),
  groups (fastest or failover), Block
- **Network interface**: send an outbound through Tailscale, a VPN app or Wi-Fi without changing the PC's routes
- **DNS**: the PC's DNS, DoH / DoT / DoQ, per card, optionally through the tunnel; Fake-IP for TUN
- **PC block**: use an HTTP inbound as the Windows system proxy while it runs
- Share links (`ss://`, `vless://`, `vmess://`, `trojan://`, `tuic://`, `hysteria2://`), connection check and test,
  logs and live statistics, 8 languages, tray, start at login

Proxies run on [sing-box](https://github.com/SagerNet/sing-box) (one process per card, bundled in the release).
A Hysteria2 server open to other PCs runs on the official [Hysteria](https://github.com/apernet/hysteria) core.
A built-in Python engine is the fallback.

## Running

Download a build from [Releases](https://github.com/NenkaLab/proxy-dock/releases), or run from source
(Python 3.14):

```bash
python -m venv .venv
.venv\Scripts\activate            # macOS / Linux: source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py     # sing-box and Hysteria for this PC
python proxydock.py
```

Settings are saved in `%APPDATA%\ProxyDock` (macOS: `~/Library/Application Support/ProxyDock`,
Linux: `~/.config/ProxyDock`). The **How to use** page in the app explains the rest.

## Building

`tools\build_win.bat` → `dist/win/ProxyDock.exe` (Linux: `tools/build_linux.sh`, macOS: `tools/build_mac.sh`).
The build downloads sing-box and Hysteria (SHA-256 checked) and puts them inside the app.
Tests: `python tests/run_tests.py`. Releases: `tools\release.bat <version>` tags and pushes; GitHub Actions builds them.

## Limitations

- Routers, groups, WireGuard (both sides), VLESS / VMess / Trojan / TUIC, TUN, interfaces and custom DNS need the sing-box engine.
- No load balancing across servers; Hysteria2 servers cannot listen on a port range by themselves.
- Certificate pinning runs on the Python engine until sing-box 1.15.
- TUN and Fake-IP have not been tried on real setups yet. macOS has not been tried on a real Mac.
- SOCKS4 has no password, so only use it on this PC.

## License

[GPL-3.0](LICENSE). Bundled components (sing-box GPL-3.0, Hysteria MIT, Qt / PySide6 LGPL-3.0, fonts OFL-1.1, ...)
and their licenses are listed in [`licenses/`](licenses/).

Copyright (C) 2026 NenkaLab
