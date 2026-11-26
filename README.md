# ProxyDock

**English** · [한국어](docs/README.ko.md) · [日本語](docs/README.ja.md) · [简体中文](docs/README.zh-CN.md) · [繁體中文](docs/README.zh-TW.md) · [Deutsch](docs/README.de.md) · [Français](docs/README.fr.md) · [Español](docs/README.es.md)

A desktop app that runs several proxies and tunnels side by side. Link **inbound** blocks (how traffic enters,
e.g. a local SOCKS5 port) to **outbound** blocks (how it leaves, e.g. a Shadowsocks server) on a canvas, and
turn each card on or off.

<sub>Made for my own use and shared as is.</sub>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/canvas-dark.png">
    <img src="docs/images/canvas-light.png" width="98%" alt="Canvas">
  </picture>
</p>
<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/proxies-dark.png">
    <img src="docs/images/proxies-light.png" width="49%" alt="Proxies">
  </picture>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/images/router-dark.png">
    <img src="docs/images/router-light.png" width="49%" alt="Router">
  </picture>
</p>

## Features

- **Link blocks on a canvas**: inbounds (SOCKS5, HTTP, Shadowsocks / Hysteria2 / WireGuard server, TUN, ...) to
  outbounds (Direct, upstream proxy, Shadowsocks, Hysteria2, WireGuard, VLESS, VMess, Trojan, TUIC, subscriptions)
- **Routing**: send traffic by domain, IP, program or country; an IF switch works as a kill switch; groups use the
  fastest server or fail over
- **Chains**: send one outbound through another, or through Tailscale, a VPN app or Wi-Fi
- **System proxy**: use a card as the Windows system proxy while it runs
- **Everyday tools**: share links, connection tests, logs and live statistics, 8 languages, tray

Proxies run on [sing-box](https://github.com/SagerNet/sing-box), bundled in the release.

## Running

Windows 10 / 11: `ProxyDock-<version>-windows-x64-setup.exe` installs it for your user (no administrator needed);
the `.zip` is the same app as a folder. Download it from [Releases](https://github.com/NenkaLab/proxy-dock/releases).

Settings are saved in `%APPDATA%\ProxyDock`; version 3 reads and keeps the settings of 2.x. The **How to use** page in
the app explains the rest.

ProxyDock 3 is a rewrite in Rust and runs on Windows only for now. For macOS and Linux, use 2.x (the Python app, on
the [`old`](https://github.com/NenkaLab/proxy-dock/tree/old) branch and in the 2.x releases).

## Building

Running from source, tests, builds, installers and releases: [docs/Build.md](docs/Build.md). What to check before a
release: [TESTING.md](TESTING.md).

## Limitations

- Windows only (macOS / Linux: 2.x).
- TUN has not been tried on real setups yet.
- SOCKS4 has no password: use it only on this PC.

## License

[GPL-3.0](LICENSE). Bundled components (sing-box GPL-3.0, Hysteria MIT, Slint GPL-3.0, Rust crates, fonts OFL-1.1, ...)
and their licenses are listed in [`licenses/`](licenses/).

Copyright (C) 2026 NenkaLab
