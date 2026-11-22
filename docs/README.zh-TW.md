# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · **繁體中文** · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

一個桌面代理「船塢」：可以建立多個代理，並分別開啟或關閉。為個人使用而做。

每個代理由 **接收端** (在這台電腦的連接埠上接收連線的方式) 和 **傳出端** (送達目的地的方式) 組合而成。
例如「以本機 SOCKS5 接收 → 透過加密通道送到遠端 Shadowsocks 伺服器」只需要一張卡片。

<p align="center">
  <img src="images/canvas.png" width="98%" alt="畫布">
</p>
<p align="center">
  <img src="images/proxies.png" width="49%" alt="代理清單">
  <img src="images/editor.png" width="49%" alt="編輯接收端">
</p>

## 功能

| | 支援 |
|---|---|
| 接收端 | SOCKS5 · SOCKS4/4a · HTTP · HTTPS (TLS) · Shadowsocks 伺服器 · Hysteria2 伺服器 |
| 傳出端 | Direct · 上游 HTTP / HTTPS / SOCKS4 / SOCKS5 代理 · Shadowsocks 通道 · Hysteria2 (QUIC) 通道 |
| UDP | 接收 SOCKS5 · Shadowsocks · Hysteria2，傳出 Direct · Shadowsocks · Hysteria2 |

- **畫布**：藍圖式編輯器 - 從接收端區塊拖線到傳出端區塊；多個接收端可共用一個傳出端，把接收端連到另一個接收端會經由它中繼，Direct 可以指定來源 IP
- **PC 區塊**：連接一個 HTTP 接收端，在其執行期間用作 Windows 系統代理；先前的設定會被保存，停止或結束應用程式時還原 (還原先前設定 / 關閉 按鈕)
- **串接**：連接傳出端區塊的「經由」輸出後，它到伺服器的連線會經由另一個傳出端 (例如穿過公司 HTTP 代理的 Shadowsocks、Hysteria2 通道中的 HTTPS 上游代理)，可以串接任意多層
- **Shadowsocks**：AEAD 2018 (aes-128/256-gcm、chacha20-ietf-poly1305) 與 2022 (SIP022、blake3)，TCP+UDP，複製 `ss://` 連結
- **Hysteria2**：已確認與官方實作雙向相容 (TCP、UDP)，憑證指紋固定，複製 `hysteria2://` 連結
- **設定**：全域預設值 + 每個代理的本機設定 (本機優先) - 允許的網域、封鎖內部網路、閒置逾時、記錄
- **編輯頁面的檢查連線**：儲存前測試傳出端 (上游代理/通道、密碼) 與接收端連接埠
- **連線測試**：透過執行中的代理以 HTTPS 連線多個國家的 21 個網站並測量回應時間
- **記錄 / 統計**：依代理檢視，即時速度圖，可選擇寫入檔案
- **安全性**：密碼以 Windows DPAPI 儲存 (其他作業系統為 0600 權限檔案)，依 IP 限制連線數，
  多次驗證失敗後暫時封鎖，防止代理鏈迴圈，Windows 防火牆外部封鎖按鈕
- **語言**：English (預設)、한국어、日本語、简体中文、繁體中文、Deutsch、Français、Español (設定 → 顯示 → 語言)
- 常駐系統匣、開機時啟動、淺色/深色佈景主題、內建字型 (Pretendard / JetBrains Mono)

## 執行

如果 [Releases](https://github.com/NenkaLab/proxy-dock/releases) 中有建置好的檔案可以直接使用，也可以從原始碼執行：

```bash
python -m venv .venv
# Windows: .venv\Scripts\activate   /   macOS、Linux: source .venv/bin/activate
pip install -r requirements.txt
python proxydock.py            # --hidden : 不顯示視窗，直接啟動到系統匣
```

以 Python 3.14 開發與測試。

第一次啟動時沒有代理。在 **新增代理** 中選擇範本 (本機 SOCKS5、本機 HTTP、Shadowsocks 伺服器、
本機 → Shadowsocks 通道等) 建立，再用卡片上的開關執行。詳細用法請見應用程式中的 **使用方式** 頁面。
介面語言預設為英文，可在 設定 → 顯示 → 語言 中變更 (重新啟動後套用)。

設定儲存位置

| 作業系統 | 路徑 |
|---|---|
| Windows | `%APPDATA%\ProxyDock` |
| macOS | `~/Library/Application Support/ProxyDock` |
| Linux | `$XDG_CONFIG_HOME/ProxyDock` (預設 `~/.config/ProxyDock`) |

## 建置

建置相關的檔案都在 [`tools/`](../tools/) 中。以 PyInstaller 產生單一執行檔 (macOS 為 `.app`)。
指令碼會為每個作業系統建立虛擬環境，只安裝與 `requirements.txt` 不一致的套件，再以 `tools/ProxyDock.spec` 建置。
可以在任何資料夾中執行。

| 指令 | 輸出 |
|---|---|
| `tools\build_win.bat` | `dist/win/ProxyDock.exe` (在 Windows 上) |
| `tools\build_linux.bat` / `tools/build_linux.sh` | `dist/linux/ProxyDock` (Linux 上直接建置，Windows/macOS 上透過 Docker) |
| `tools/build_mac.sh` | `dist/mac/ProxyDock.app` (僅 macOS) |
| `tools\build.bat` / `tools/build.sh` | 建置這台電腦能建置的全部目標並彙整結果 |

macOS 建置尚未在實體 Mac 上驗證。

## 發佈與 CI

- 每次推送和 PR 都會在 Windows、Linux、macOS 上執行測試 ([CI](../.github/workflows/ci.yml)，
  本機執行 `python tests/run_tests.py` - 不需要網路)。
- `tools\release.bat 2.1.0` (或 `python tools/release.py 2.1.0`) 會檢查工作區、執行測試、提升 `__version__`、
  提交、建立 `v2.1.0` 標籤並推送。標籤會觸發[發佈工作流程](../.github/workflows/release.yml)，建置 Windows / Linux / macOS
  並把壓縮檔和 `SHA256SUMS.txt` 發佈到 GitHub Release。加上 `--dry-run` 只做預演。
- `python tools/build.py package win|linux|mac` 可在本機 `dist/release/` 產生相同的壓縮檔。

## 已知限制

- Hysteria2 (QUIC/UDP) 無法*經由*其他傳出端 (其他區塊可以經由 Hysteria2)；串接的傳出端不轉送 UDP。
- Hysteria2 執行在 Python 的 QUIC 實作 (aioquic) 上，速度約 100 Mbps。不支援頻寬設定 (Brutal)、
  Salamander 混淆與連接埠跳躍。
- 官方 Hysteria2 伺服器會拒絕 SNI 與憑證不符的連線 - 請在傳出端的 SNI 中填入伺服器憑證的網域。
- HTTP 接收端的一般 (非 CONNECT) 要求每個連線只處理一個。
- SOCKS4 通訊協定沒有密碼驗證，請只在這台電腦使用。
- 日文與中文使用系統介面字型 (內建的 Pretendard 用於韓文與拉丁字母)。

## 結構

```
proxydock.py            進入點
proxydock/
  common.py             共用常數、記錄、位址處理、封鎖清單
  config.py             設定格式 (v2)、驗證、全域/本機合併、密碼儲存、舊版移轉
  graph.py              區塊圖 (接收端 / 傳出端 / PC 區塊) -> 執行用代理
  runtime.py            ProxyInstance / ProxyManager (在一個 asyncio 執行緒中執行多個代理)
  relay.py              TCP/UDP 中繼核心
  inbound.py            接收端通訊協定
  outbound.py           傳出端通訊協定
  ss.py                 Shadowsocks 2018/2022 加密
  hysteria2.py          Hysteria2 伺服器與用戶端 (aioquic)
  tls.py                自我簽署憑證、TLS 內容
  probe.py              連線測試 / 檢查連線
  platform_utils.py     開機啟動、防火牆、連接埠檢查
  i18n.py, locales/     翻譯 (程式碼中的英文原文 -> <語言>.json)
  ui/                   PySide6 + Fluent Widgets 介面
    canvas.py           畫布頁面 (NodeGraphQt)
tools/                  建置指令碼、PyInstaller spec、Linux 建置用 Dockerfile
licenses/               隨附的第三方元件授權
docs/                   翻譯的 README、螢幕截圖
```

### 翻譯

介面文字在程式碼中以英文撰寫 (`_t("...")`)，透過 `proxydock/locales/<語言>.json` 翻譯。
沒有翻譯的文字會以英文顯示。

## 授權

[GPL-3.0](../LICENSE)。介面函式庫 [PySide6-Fluent-Widgets](https://github.com/zhiyiYo/PyQt-Fluent-Widgets) 採用
GPL v3，因此本程式也以 GPL v3 發佈；散布建置好的執行檔時同樣需要遵守 GPL v3 條款 (提供原始碼等)。

隨附字型與函式庫的授權位於 [`licenses/`](../licenses/) 資料夾。

| 元件 | 授權 |
|---|---|
| Pretendard, JetBrains Mono (內建字型) | SIL OFL 1.1 |
| PySide6-Fluent-Widgets | GPL v3 |
| Qt 6 / PySide6 / Shiboken6, PySideSix-Frameless-Window | LGPL v3 |
| aioquic, pylsqpack, darkdetect | BSD 3-Clause |
| cryptography | Apache 2.0 / BSD 3-Clause |
| pyOpenSSL | Apache 2.0 |
| service-identity, attrs, NodeGraphQt, Qt.py | MIT |
| certifi | MPL 2.0 |
| Python 執行環境 | PSF License |
| PyInstaller 開機載入程式 | GPL v2+ (開機載入程式例外) |

Copyright (C) 2026 NenkaLab
