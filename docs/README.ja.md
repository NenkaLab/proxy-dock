# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · **日本語** · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

複数のプロキシやトンネルを並べて動かすデスクトップアプリです。キャンバスで**受信側**ブロック (通信の入口、例: ローカル SOCKS5 ポート) を
**送信側**ブロック (出口、例: Shadowsocks サーバー) につなぎ、カードごとにオン・オフします。

<sub>個人的に使うために作ったものを、そのまま公開しています。</sub>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/canvas-dark.png">
    <img src="images/canvas-light.png" width="98%" alt="キャンバス">
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

## 機能

- **キャンバスでブロックをつなぐ**: 受信側 (SOCKS5、HTTP、Shadowsocks / Hysteria2 / WireGuard サーバー、TUN など) を
  送信側 (Direct、上位プロキシ、Shadowsocks、Hysteria2、WireGuard、VLESS、VMess、Trojan、TUIC、サブスクリプション) へ
- **ルーティング**: ドメイン、IP、プログラム、国ごとに振り分け。IF スイッチはキルスイッチに、グループは最速サーバーや自動切り替えに
- **チェーン**: 送信側を別の送信側、または Tailscale・VPN アプリ・Wi-Fi 経由で出す
- **システムプロキシ**: カードを動作中だけ Windows のシステムプロキシとして使う
- **便利な機能**: 共有リンク、接続テスト、ログとリアルタイム統計、8 言語、トレイ
- **回線直結**: 選んだブロックだけをネットワークカードから直接送り出す。PC 全体のネットワークを握る VPN、TUN、
  その他のアプリを、そのアプリのスプリットトンネリングなしで迂回 (Windows: 小さな Hyper-V VM、Linux: ネットワーク
  名前空間、macOS: 小さな VM、未検証)
- **ホーム画面**: アプリ全体のスイッチ (オフですべての接続を拒否するキルスイッチ。各カードのオン/オフはそのまま)、
  カードの状態、リアルタイムの通信量、回線直結 VM

プロキシは [sing-box](https://github.com/SagerNet/sing-box) で動き、リリースに同梱されています。

## 実行

[Releases](https://github.com/NenkaLab/proxy-dock/releases) から入手します。

- **Windows 10 / 11**: `ProxyDock-<バージョン>-windows-x64-setup.exe` が現在のユーザーにインストールします (管理者権限
  は不要)。`.zip` は同じアプリをフォルダーにしたものです。
- **macOS** (Apple Silicon): `.dmg` を開いて ProxyDock をアプリケーションフォルダーへドラッグします。Apple の公証が
  ないため初回の起動がブロックされます。システム設定 → プライバシーとセキュリティ → **このまま開く** を押すか
  (macOS 15 以降は右クリック → 開く では開けません)、ターミナルでダウンロードの印を消してください:
  ```
  xattr -dr com.apple.quarantine /Applications/ProxyDock.app
  ```
- **Linux** (x64): `.AppImage` をそのまま実行します (先に `chmod +x`)。`.tar.gz` は同じアプリをフォルダーにしたものです。

設定は `%APPDATA%\ProxyDock` (Windows)、`~/Library/Application Support/ProxyDock` (macOS)、`~/.config/ProxyDock`
(Linux) に保存されます。バージョン 3 は 2.x の設定をそのまま読み込みます。詳しくはアプリの**使い方**ページを見て
ください。ProxyDock 3 は Rust で作り直した版で、2.x (Python 版) は [`old`](https://github.com/NenkaLab/proxy-dock/tree/old) ブランチにあります。

## ビルド

ソースからの実行、テスト、ビルド、インストーラー、リリース: [Build.md](Build.md) (英語)。リリース前の確認方法:
[TESTING.md](../TESTING.md) (韓国語)。

## 制限

- macOS と Linux は自動でビルド・テストしていますが、実機ではまだ確認していません。
- macOS / Linux: 他の PC をブロックするファイアウォール規則はなく (システムのファイアウォールを使ってください)、
  起動時の「管理者として実行」もありません (TUN カードは起動時にパスワードを尋ねます)。モニターのカラープロファイル
  は読まず (sRGB とみなします)、設定のパスワードは暗号化されず (2.x と同じ)、マウスの戻る/進むボタンでは移動しません。
- Linux: システムプロキシ (PC ブロック) は GNOME と KDE でのみ設定されます。通知には `notify-send` が必要です。
- TUN はまだ実環境で試していません。
- SOCKS4 にはパスワードがないので、この PC の中だけで使ってください。
- 回線直結: Windows では Hyper-V (Pro、Enterprise、Education) と管理者権限が必要です。VM ごとにルーターから新しい
  機器 (独自の MAC と IP アドレス) に見え、回線ごとに 1 つのアドレスしか割り当てないプロバイダーではアドレスを
  取得できないことがあります。この通信では実際の IP アドレスが見え、VPN は保護しません。使用の責任はユーザーに
  あります (VM を作る前にアプリが同意を求めます)。
- Linux の回線直結: コンテナーでのみ試験しました (macvlan を使うネットワーク名前空間。Wi-Fi は ipvlan で、DHCP が
  使えないことがあるので固定アドレスを使ってください)。`ip` (iproute2) と pkexec による root 権限が必要です。
- macOS (Apple Silicon) の回線直結: コードと型チェックまでで、**実際の Mac ではまだ一度も試していません**
  (Virtualization.framework とブリッジの vmnet インターフェイス。ヘルパーを root で実行する必要があります)。

## コントリビュート

バグ、まだ試していないプラットフォーム (macOS、Linux)、翻訳など、自分で直したり改善したりしたものがあれば
プルリクエストを送ってください。自分で書いても AI を使っても構いませんが、送る前に必ず検証してください。ビルドし、
テストを実行し (`native/` で `cargo test --workspace`)、アプリで変更を実際に使ってみてください (必要なら実機で)。
プルリクエストには何をどう確認したかを書いてください。

## ライセンス

[GPL-3.0](../LICENSE)。同梱のコンポーネント (sing-box GPL-3.0、Hysteria MIT、Slint GPL-3.0、Rust クレート、フォント OFL-1.1 など) と
ライセンスは [`licenses/`](../licenses/) にあります。

Copyright (C) 2026 NenkaLab
