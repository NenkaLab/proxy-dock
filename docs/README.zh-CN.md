# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · **简体中文** · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

同时运行多个代理和隧道的桌面应用。在画布上把**入站**块 (流量进入的方式，例如本地 SOCKS5 端口) 连到
**出站**块 (流量离开的方式，例如 Shadowsocks 服务器)，每张卡片可单独开关。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/canvas-dark.png">
    <img src="images/canvas-light.png" width="98%" alt="画布">
  </picture>
</p>
<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/proxies-dark.png">
    <img src="images/proxies-light.png" width="49%" alt="Proxies">
  </picture>
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/router-dark.png">
    <img src="images/router-light.png" width="49%" alt="Router">
  </picture>
</p>

## 功能

- **入站**: SOCKS5、SOCKS4、HTTP、HTTPS、Shadowsocks 服务器、Hysteria2 服务器、WireGuard 服务器 (手机和电脑用 WireGuard 应用连接)、端口转发、TUN (本机所有应用)
- **出站**: Direct、HTTP / HTTPS / SOCKS 代理、Shadowsocks、Hysteria2、WireGuard、VLESS (REALITY)、VMess、
  Trojan、TUIC、订阅 (分享链接或 Clash YAML)
- **块**: 链式 (出站经由另一个出站)、路由器 (按域名、IP、端口、程序 (可从运行中的程序选择)、GeoIP/Geosite 分流，简单和高级视图)、IF (一个条件，或手动开关，例如断网开关)、分组 (最快、故障转移或手动)、阻止
- **网络接口**: 不改动本机路由，让出站经 Tailscale、VPN 应用或 Wi-Fi 出去
- **DNS**: 本机 DNS、DoH / DoT / DoQ，可按卡片设置，可经由隧道发送; TUN 可用 Fake-IP
- **卡片**: 在块上直接运行、随应用启动、自动重启 (停止或启动失败时)
- **PC 块**: HTTP 入站运行期间用作 Windows 系统代理
- 分享链接 (`ss://`、`vless://`、`vmess://`、`trojan://`、`tuic://`、`hysteria2://`)、连接检查与测试、日志和实时统计、
  8 种语言、托盘、登录时启动

代理由 [sing-box](https://github.com/SagerNet/sing-box) 运行 (每张卡片一个进程，发布版已内置)。
对其他电脑开放的 Hysteria2 服务器使用官方 [Hysteria](https://github.com/apernet/hysteria) 核心，内置 Python 引擎作为备用。

## 运行

在 Windows 上，`ProxyDock-<版本>-windows-x64-setup.exe` 为当前用户安装 (不需要管理员权限)；`.zip` 是同一个应用的文件夹版本。macOS 上打开 `.dmg`，把 ProxyDock 拖到“应用程序”。Linux 上给 `.AppImage` 加上执行权限后运行。

从 [Releases](https://github.com/NenkaLab/proxy-dock/releases) 下载构建，或从源码运行 (Python 3.14):

```bash
python -m venv .venv
.venv\Scripts\activate            # macOS / Linux: source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py     # 本机用的 sing-box 和 Hysteria
python proxydock.py
```

设置保存在 `%APPDATA%\ProxyDock` (macOS: `~/Library/Application Support/ProxyDock`，Linux: `~/.config/ProxyDock`)。
其余内容见应用内的**使用方法**页面。

## 构建

`tools\build_win.bat` → `dist/win/ProxyDock/ProxyDock.exe` (Linux: `tools/build_linux.sh`，macOS: `tools/build_mac.sh`)。
构建时会下载 sing-box 和 Hysteria (校验 SHA-256) 并放进应用。
测试: `python tests/run_tests.py`。发布: `tools\release.bat <版本>` 打标签并推送，由 GitHub Actions 构建。
安装程序: `python tools/build.py installer` (Inno Setup 6)。macOS: `python tools/build.py dmg`，Linux: `python tools/build.py appimage`。

## 限制

- 路由器、分组、WireGuard、VLESS / VMess / Trojan / TUIC、TUN、网络接口和自定义 DNS 需要 sing-box 引擎。
- 不支持多服务器负载均衡; Hysteria2 服务器无法单独监听端口范围。
- 在 sing-box 1.15 之前，证书固定由 Python 引擎运行。
- TUN 和 Fake-IP 尚未在真实环境中测试，macOS 也未在真机上测试。
- SOCKS4 没有密码，请只在本机使用。

## 许可证

[GPL-3.0](../LICENSE)。内置组件 (sing-box GPL-3.0、Hysteria MIT、Qt / PySide6 LGPL-3.0、字体 OFL-1.1 等) 及其许可证
见 [`licenses/`](../licenses/)。

Copyright (C) 2026 NenkaLab
