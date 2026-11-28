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
- **Direct line**: chosen blocks go straight out through a network card, past a VPN, a TUN or any app that
  holds the whole PC's network, without its split tunnelling (Windows: a small Hyper-V VM; Linux: a network
  namespace; macOS: a small VM, not tried yet)
- **Home page**: one switch for the whole app (off refuses every connection, a kill switch; each card keeps its
  own on / off), the cards' state, live traffic and the Direct line VMs

Proxies run on [sing-box](https://github.com/SagerNet/sing-box), bundled in the release.

## Running

From [Releases](https://github.com/NenkaLab/proxy-dock/releases):

- **Windows 10 / 11**: `ProxyDock-<version>-windows-x64-setup.exe` installs it for your user (no administrator
  needed); the `.zip` is the same app as a folder.
- **macOS** (Apple Silicon): open the `.dmg` and drag ProxyDock into Applications. It is not notarized by Apple, so
  macOS blocks the first start: open System Settings → Privacy & Security and click **Open Anyway** (right-click →
  Open no longer works on macOS 15 and later), or clear the download mark in Terminal:
  ```
  xattr -dr com.apple.quarantine /Applications/ProxyDock.app
  ```
- **Linux** (x64): the `.AppImage` runs as it is (`chmod +x` it first); the `.tar.gz` is the same app as a folder.

Settings are saved in `%APPDATA%\ProxyDock` (Windows), `~/Library/Application Support/ProxyDock` (macOS) or
`~/.config/ProxyDock` (Linux); version 3 reads and keeps the settings of 2.x. The **How to use** page in the app
explains the rest. ProxyDock 3 is a rewrite in Rust; 2.x (the Python app) is on the [`old`](https://github.com/NenkaLab/proxy-dock/tree/old) branch.

## Building

Running from source, tests, builds, installers and releases: [docs/Build.md](docs/Build.md). What to check before a
release: [TESTING.md](TESTING.md).

## Limitations

- macOS and Linux are built and tested automatically but not tried on real machines yet.
- macOS / Linux: no rule that blocks other PCs (use the system's firewall), no "Run as administrator" at start (a
  TUN card asks for the password when it starts), the monitor's colour profile is not read (sRGB is assumed),
  passwords in the settings are not encrypted (as in 2.x), and the mouse's back / forward buttons do not go back.
- Linux: the system proxy (PC block) is set for GNOME and KDE only; notifications need `notify-send`.
- TUN has not been tried on real setups yet.
- SOCKS4 has no password: use it only on this PC.
- Direct line: on Windows it needs Hyper-V (Pro, Enterprise or Education) and administrator permission; each VM
  shows up on your router as a new device (its own MAC and IP address), and a provider that gives one address per
  line may not give it one. Its traffic shows your real IP address and no VPN protects it: you are responsible for
  using it (the app asks before making a VM).
- Direct line on Linux: tried in a container only (a network namespace with macvlan; Wi-Fi uses ipvlan, where
  DHCP may not work: use a fixed address). Needs `ip` (iproute2) and root through pkexec.
- Direct line on macOS (Apple Silicon): written and type-checked, but **not tried on any Mac yet**
  (Virtualization.framework with a bridged vmnet interface, which needs the helper to run as root).

## Contributing

If you fixed or improved something - a bug, a platform not tried yet (macOS, Linux), a translation - please send a
pull request. Write it yourself or with an AI tool, either is fine, but check it before you send it: build it,
run the tests (`cargo test --workspace` in `native/`) and try the change in the app, on a real machine where it
matters. Say in the pull request what you checked and how.

## License

[GPL-3.0](LICENSE). Bundled components (sing-box GPL-3.0, Hysteria MIT, Slint GPL-3.0, Rust crates, fonts OFL-1.1, ...)
and their licenses are listed in [`licenses/`](licenses/).

Copyright (C) 2026 NenkaLab
