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
- **線路直連**：讓選定的區塊直接從網路卡送出，繞過接管整台電腦網路的 VPN、TUN 或其他應用程式，不需它們的分流
  功能（Windows：小型 Hyper-V 虛擬機；Linux：網路命名空間；macOS：小型虛擬機，尚未測試）
- **首頁**：整個應用程式的開關（關閉即拒絕所有連線的斷線開關；每張卡片的開關狀態保持不變）、卡片狀態、即時流量、
  線路直連虛擬機

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
- 線路直連：在 Windows 上需要 Hyper-V（專業版、企業版或教育版）和系統管理員權限；每台虛擬機在路由器上顯示為新
  裝置（有自己的 MAC 和 IP 位址），如果服務商每條線路只分配一個位址，它可能取得不到位址。這些流量會暴露你的
  真實 IP 位址，不受任何 VPN 保護：使用責任由你承擔（應用程式在建立虛擬機前會徵求同意）。
- Linux 上的線路直連：只在容器中測試過（使用 macvlan 的網路命名空間；Wi-Fi 使用 ipvlan，可能無法 DHCP，請使用
  固定位址）。需要 `ip`（iproute2）以及透過 pkexec 取得的 root 權限。
- macOS（Apple 晶片）上的線路直連：只寫好了程式碼並做了型別檢查，**尚未在任何 Mac 上測試過**
  （Virtualization.framework 加橋接的 vmnet 介面，協助程式需要以 root 執行）。

## 貢獻

如果你修正或改進了什麼——bug、尚未測試的平台（macOS、Linux）、翻譯——歡迎提交 pull request。自己寫或用 AI
都可以，但提交前請務必驗證：建置、執行測試（在 `native/` 中執行 `cargo test --workspace`），並在應用程式裡實際
試用改動（必要時在實機上）。請在 pull request 裡說明你檢查了什麼、怎麼檢查的。

## 授權

[GPL-3.0](../LICENSE)。內建元件 (sing-box GPL-3.0、Hysteria MIT、Slint GPL-3.0、Rust crate、字型 OFL-1.1 等) 與授權
見 [`licenses/`](../licenses/)。

Copyright (C) 2026 NenkaLab
