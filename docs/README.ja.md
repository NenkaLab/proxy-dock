# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · **日本語** · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

複数のプロキシやトンネルを並べて動かすデスクトップアプリです。キャンバスで**受信側**ブロック (通信の入口、例: ローカル SOCKS5 ポート) を
**送信側**ブロック (出口、例: Shadowsocks サーバー) につなぎ、カードごとにオン・オフします。

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

プロキシは [sing-box](https://github.com/SagerNet/sing-box) で動き、リリースに同梱されています。

## 実行

Windows では `ProxyDock-<バージョン>-windows-x64-setup.exe` が現在のユーザーにインストールします (管理者権限は不要)。`.zip` は同じアプリをフォルダーにしたものです。macOS では `.dmg` を開いて ProxyDock をアプリケーションにドラッグします。Linux では `.AppImage` に実行権限を付けて実行します。

[Releases](https://github.com/NenkaLab/proxy-dock/releases) からビルドを入手します。

設定は `%APPDATA%\ProxyDock` (macOS: `~/Library/Application Support/ProxyDock`、Linux: `~/.config/ProxyDock`) に
保存されます。詳しくはアプリの**使い方**ページを見てください。

## ビルド

ソースからの実行、テスト、ビルド、インストーラー、リリース: [Build.md](Build.md) (英語)。

## 制限

- ルーター、グループ、WireGuard、VLESS / VMess / Trojan / TUIC、TUN には sing-box エンジン (同梱) が必要です。
- TUN と macOS はまだ実環境で試していません。
- SOCKS4 にはパスワードがないので、この PC の中だけで使ってください。

## ライセンス

[GPL-3.0](../LICENSE)。同梱のコンポーネント (sing-box GPL-3.0、Hysteria MIT、Qt / PySide6 LGPL-3.0、フォント OFL-1.1 など) と
ライセンスは [`licenses/`](../licenses/) にあります。

Copyright (C) 2026 NenkaLab
