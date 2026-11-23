# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · **繁體中文** · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

同時執行多個代理與通道的桌面應用程式。在畫布上把**傳入端**區塊 (流量進入的方式，例如本機 SOCKS5 連接埠) 連到
**傳出端**區塊 (流量離開的方式，例如 Shadowsocks 伺服器)，每張卡片可個別開關。

<sub>這是為個人使用而做的工具，按原樣分享。</sub>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/canvas-dark.png">
    <img src="images/canvas-light.png" width="98%" alt="畫布">
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

- **在畫布上連接區塊**：把入站（SOCKS5、HTTP、Shadowsocks / Hysteria2 / WireGuard 伺服器、TUN 等）
  連到出站（直連、上游代理、Shadowsocks、Hysteria2、WireGuard、VLESS、VMess、Trojan、TUIC、訂閱）
- **路由**：依網域、IP、程式或國家分流；IF 開關可當作斷網保護（kill switch）；群組選用最快的伺服器或自動切換
- **串接**：讓一個出站經過另一個出站，或經過 Tailscale、VPN 應用程式、Wi-Fi
- **系統代理**：在卡片執行期間把它用作 Windows 系統代理
- **常用工具**：分享連結、連線測試、記錄與即時統計、8 種語言、系統匣

代理由 [sing-box](https://github.com/SagerNet/sing-box) 執行，已隨發行版附帶。

## 執行

在 Windows 上，`ProxyDock-<版本>-windows-x64-setup.exe` 會為目前使用者安裝 (不需要系統管理員權限)；`.zip` 是同一個應用程式的資料夾版本。macOS 上開啟 `.dmg`，把 ProxyDock 拖到「應用程式」。Linux 上給 `.AppImage` 加上執行權限後執行。

從 [Releases](https://github.com/NenkaLab/proxy-dock/releases) 下載建置。

設定儲存在 `%APPDATA%\ProxyDock` (macOS: `~/Library/Application Support/ProxyDock`，Linux: `~/.config/ProxyDock`)。
其餘內容見程式內的**使用方法**頁面。

## 建置

從原始碼執行、測試、建置、安裝程式與發行：[Build.md](Build.md)（英文）。

## 限制

- 路由、群組、WireGuard、VLESS / VMess / Trojan / TUIC 和 TUN 需要 sing-box 引擎（已附帶）。
- TUN 和 macOS 還沒有在真實環境中試過。
- SOCKS4 沒有密碼，只在本機使用。

## 授權

[GPL-3.0](../LICENSE)。內建元件 (sing-box GPL-3.0、Hysteria MIT、Qt / PySide6 LGPL-3.0、字型 OFL-1.1 等) 與授權
見 [`licenses/`](../licenses/)。

Copyright (C) 2026 NenkaLab
