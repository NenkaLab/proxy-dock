# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · **繁體中文** · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

同時執行多個代理與通道的桌面應用程式。在畫布上把**傳入端**區塊 (流量進入的方式，例如本機 SOCKS5 連接埠) 連到
**傳出端**區塊 (流量離開的方式，例如 Shadowsocks 伺服器)，每張卡片可個別開關。

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

- **傳入端**: SOCKS5、SOCKS4、HTTP、HTTPS、Shadowsocks 伺服器、Hysteria2 伺服器、WireGuard 伺服器 (手機和電腦用 WireGuard 程式連線)、連接埠轉送、TUN (本機所有程式)
- **傳出端**: Direct、HTTP / HTTPS / SOCKS 代理、Shadowsocks、Hysteria2、WireGuard、VLESS (REALITY)、VMess、
  Trojan、TUIC、訂閱 (分享連結或 Clash YAML)
- **區塊**: 串接 (傳出端經由另一個傳出端)、路由器 (依網域、IP、連接埠、程式 (可從執行中的程式選擇)、GeoIP/Geosite 分流，簡易與進階檢視)、IF (一個條件，或手動開關，例如斷網開關)、群組 (最快、容錯移轉或手動)、封鎖
- **網路介面**: 不改動本機路由，讓傳出端經 Tailscale、VPN 程式或 Wi-Fi 出去
- **DNS**: 本機 DNS、DoH / DoT / DoQ，可依卡片設定，可經由通道送出; TUN 可用 Fake-IP
- **卡片**: 在區塊上直接執行、隨程式啟動、自動重新啟動 (停止或啟動失敗時)
- **PC 區塊**: HTTP 傳入端執行期間當作 Windows 系統代理
- 分享連結 (`ss://`、`vless://`、`vmess://`、`trojan://`、`tuic://`、`hysteria2://`)、連線檢查與測試、記錄與即時統計、
  8 種語言、系統匣、登入時啟動

代理由 [sing-box](https://github.com/SagerNet/sing-box) 執行 (每張卡片一個程序，發行版已內建)。
對其他電腦開放的 Hysteria2 伺服器使用官方 [Hysteria](https://github.com/apernet/hysteria) 核心，內建 Python 引擎作為備用。

## 執行

在 Windows 上，`ProxyDock-<版本>-windows-x64-setup.exe` 會為目前使用者安裝 (不需要系統管理員權限)；`.zip` 是同一個應用程式的資料夾版本。macOS 上開啟 `.dmg`，把 ProxyDock 拖到「應用程式」。Linux 上給 `.AppImage` 加上執行權限後執行。

從 [Releases](https://github.com/NenkaLab/proxy-dock/releases) 下載建置，或從原始碼執行 (Python 3.14):

```bash
python -m venv .venv
.venv\Scripts\activate            # macOS / Linux: source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py     # 本機用的 sing-box 與 Hysteria
python proxydock.py
```

設定儲存在 `%APPDATA%\ProxyDock` (macOS: `~/Library/Application Support/ProxyDock`，Linux: `~/.config/ProxyDock`)。
其餘內容見程式內的**使用方法**頁面。

## 建置

`tools\build_win.bat` → `dist/win/ProxyDock/ProxyDock.exe` (Linux: `tools/build_linux.sh`，macOS: `tools/build_mac.sh`)。
建置時會下載 sing-box 與 Hysteria (檢查 SHA-256) 並放進程式。
測試: `python tests/run_tests.py`。發行: `tools\release.bat <版本>` 加上標籤並推送，由 GitHub Actions 建置。
安裝程式: `python tools/build.py installer` (Inno Setup 6)。macOS: `python tools/build.py dmg`，Linux: `python tools/build.py appimage`。

## 限制

- 路由器、群組、WireGuard、VLESS / VMess / Trojan / TUIC、TUN、網路介面與自訂 DNS 需要 sing-box 引擎。
- 不支援多伺服器負載平衡; Hysteria2 伺服器無法單獨接聽連接埠範圍。
- 在 sing-box 1.15 之前，憑證固定由 Python 引擎執行。
- TUN 與 Fake-IP 尚未在實際環境中測試，macOS 也未在實機上測試。
- SOCKS4 沒有密碼，請只在本機使用。

## 授權

[GPL-3.0](../LICENSE)。內建元件 (sing-box GPL-3.0、Hysteria MIT、Qt / PySide6 LGPL-3.0、字型 OFL-1.1 等) 與授權
見 [`licenses/`](../licenses/)。

Copyright (C) 2026 NenkaLab
