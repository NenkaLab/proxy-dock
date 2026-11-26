# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · **简体中文** · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

同时运行多个代理和隧道的桌面应用。在画布上把**入站**块 (流量进入的方式，例如本地 SOCKS5 端口) 连到
**出站**块 (流量离开的方式，例如 Shadowsocks 服务器)，每张卡片可单独开关。

<sub>这是为个人使用而做的工具，按原样分享。</sub>

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

- **在画布上连接块**：把入站（SOCKS5、HTTP、Shadowsocks / Hysteria2 / WireGuard 服务器、TUN 等）
  连到出站（直连、上游代理、Shadowsocks、Hysteria2、WireGuard、VLESS、VMess、Trojan、TUIC、订阅）
- **路由**：按域名、IP、程序或国家分流；IF 开关可当作断网保护（kill switch）；分组选用最快的服务器或自动切换
- **链式**：让一个出站经过另一个出站，或经过 Tailscale、VPN 应用、Wi-Fi
- **系统代理**：在卡片运行期间把它用作 Windows 系统代理
- **常用工具**：分享链接、连接测试、日志和实时统计、8 种语言、托盘

代理由 [sing-box](https://github.com/SagerNet/sing-box) 运行，已随发布版附带。

## 运行

Windows 10 / 11：`ProxyDock-<版本>-windows-x64-setup.exe` 为当前用户安装 (不需要管理员权限)；`.zip` 是同一个应用的文件夹版本。
从 [Releases](https://github.com/NenkaLab/proxy-dock/releases) 下载。

设置保存在 `%APPDATA%\ProxyDock`，版本 3 直接读取 2.x 的设置。其余内容见应用内的**使用方法**页面。

ProxyDock 3 是用 Rust 重写的版本，目前只支持 Windows。macOS 和 Linux 请使用 2.x (Python 版，
[`old`](https://github.com/NenkaLab/proxy-dock/tree/old) 分支和 2.x 发布)。

## 构建

从源码运行、测试、构建、安装包和发布：[Build.md](Build.md)（英文）。发布前的检查方法：[TESTING.md](../TESTING.md)（韩文）。

## 限制

- 仅支持 Windows (macOS / Linux：2.x)。
- TUN 还没有在真实环境中试过。
- SOCKS4 没有密码，只在本机使用。

## 许可证

[GPL-3.0](../LICENSE)。内置组件 (sing-box GPL-3.0、Hysteria MIT、Slint GPL-3.0、Rust crate、字体 OFL-1.1 等) 及其许可证
见 [`licenses/`](../licenses/)。

Copyright (C) 2026 NenkaLab
