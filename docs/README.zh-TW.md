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

從 [Releases](https://github.com/NenkaLab/proxy-dock/releases) 下載。

- **Windows 10 / 11**：`ProxyDock-<版本>-windows-x64-setup.exe` 會為目前使用者安裝 (不需要系統管理員權限)；`.zip` 是同一個
  應用程式的資料夾版本。
- **macOS** (Apple Silicon)：開啟 `.dmg`，把 ProxyDock 拖到「應用程式」資料夾。沒有 Apple 簽署，第一次請按右鍵 → 打開。
- **Linux** (x64)：`.AppImage` 可直接執行 (先 `chmod +x`)；`.tar.gz` 是同一個應用程式的資料夾版本。

設定儲存在 `%APPDATA%\ProxyDock` (Windows)、`~/Library/Application Support/ProxyDock` (macOS) 或 `~/.config/ProxyDock`
(Linux)，版本 3 直接讀取 2.x 的設定。其餘內容見程式內的**使用方法**頁面。ProxyDock 3 是以 Rust 重寫的版本，2.x
(Python 版) 在 [`old`](https://github.com/NenkaLab/proxy-dock/tree/old) 分支。

## 建置

從原始碼執行、測試、建置、安裝程式與發行：[Build.md](Build.md)（英文）。發行前的檢查方法：[TESTING.md](../TESTING.md)（韓文）。

## 限制

- macOS 與 Linux 已自動建置與測試，但還沒有在實機上試過。
- macOS / Linux：沒有阻擋其他電腦的防火牆規則 (請用系統防火牆)，啟動時沒有「以系統管理員身分執行」(TUN 卡片啟動時會
  詢問密碼)，不讀取螢幕色彩設定檔 (以 sRGB 處理)，設定中的密碼不加密 (與 2.x 相同)，滑鼠的上一頁/下一頁鍵不會返回。
- Linux：系統代理 (PC 區塊) 只在 GNOME 與 KDE 上設定；通知需要 `notify-send`。
- TUN 還沒有在真實環境中試過。
- SOCKS4 沒有密碼，只在本機使用。

## 授權

[GPL-3.0](../LICENSE)。內建元件 (sing-box GPL-3.0、Hysteria MIT、Slint GPL-3.0、Rust crate、字型 OFL-1.1 等) 與授權
見 [`licenses/`](../licenses/)。

Copyright (C) 2026 NenkaLab
