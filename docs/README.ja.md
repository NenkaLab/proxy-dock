# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · **日本語** · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

複数のプロキシを作成し、それぞれオン/オフできるデスクトップ用プロキシ「ドック」です。個人用に作りました。

プロキシは **受信側** (この PC のポートで接続を受ける方式) と **送信側** (宛先まで送る方式) の組み合わせです。
たとえば「ローカル SOCKS5 で受けて → リモート Shadowsocks サーバーへ暗号化トンネル」という構成をカード 1 枚で作れます。

<p align="center">
  <img src="images/proxies.png" width="49%" alt="プロキシ一覧">
  <img src="images/editor.png" width="49%" alt="プロキシ編集">
</p>

## 機能

| | 対応 |
|---|---|
| 受信側 | SOCKS5 · SOCKS4/4a · HTTP · HTTPS (TLS) · Shadowsocks サーバー · Hysteria2 サーバー |
| 送信側 | Direct · 上位 HTTP / HTTPS / SOCKS4 / SOCKS5 プロキシ · Shadowsocks トンネル · Hysteria2 (QUIC) トンネル |
| UDP | 受信 SOCKS5 · Shadowsocks · Hysteria2、送信 Direct · Shadowsocks · Hysteria2 |

- **Shadowsocks**: AEAD 2018 (aes-128/256-gcm, chacha20-ietf-poly1305) と 2022 (SIP022, blake3)、TCP+UDP、`ss://` リンクのコピー
- **Hysteria2**: 公式実装と双方向で互換性を確認 (TCP・UDP)、証明書フィンガープリントの固定、`hysteria2://` リンクのコピー
- **設定**: 全体の既定値 + プロキシごとのローカル設定 (ローカル優先) - 許可ドメイン、内部ネットワークのブロック、アイドルタイムアウト、ログ
- **編集画面の接続確認**: 保存前に送信側 (上位プロキシ・トンネル・パスワード) と受信側のポートを試せます
- **接続テスト**: 実行中のプロキシ経由で各国のサイト 21 か所に HTTPS で接続し、応答時間を測定
- **ログ / 統計**: プロキシごとに表示、リアルタイムの速度グラフ、ファイルへのログ (任意)
- **セキュリティ**: パスワードは Windows DPAPI (他の OS は 0600 権限のファイル) で保管、IP ごとの接続上限、
  認証失敗が続いたら一時ブロック、プロキシチェーンのループ防止、Windows ファイアウォールの外部ブロックボタン
- **言語**: English (既定)、한국어、日本語、简体中文、繁體中文、Deutsch、Français、Español (設定 → 表示 → 言語)
- トレイ常駐、ログイン時に起動、ライト/ダークテーマ、内蔵フォント (Pretendard / JetBrains Mono)

## 実行

[Releases](https://github.com/NenkaLab/proxy-dock/releases) にビルド済みファイルがあればそれを使うか、ソースから直接実行します。

```bash
python -m venv .venv
# Windows: .venv\Scripts\activate   /   macOS・Linux: source .venv/bin/activate
pip install -r requirements.txt
python proxydock.py            # --hidden : ウィンドウなしでトレイに起動
```

Python 3.14 で開発・テストしています。

初回起動時はプロキシがありません。**新しいプロキシ** でテンプレート (ローカル SOCKS5、ローカル HTTP、Shadowsocks サーバー、
ローカル → Shadowsocks トンネルなど) を選んで作成し、カードのスイッチで実行します。詳しくはアプリの **使い方** 画面を参照してください。
表示言語の既定は英語で、設定 → 表示 → 言語 で変更できます (再起動後に適用)。

設定の保存場所

| OS | パス |
|---|---|
| Windows | `%APPDATA%\ProxyDock` |
| macOS | `~/Library/Application Support/ProxyDock` |
| Linux | `$XDG_CONFIG_HOME/ProxyDock` (既定 `~/.config/ProxyDock`) |

## ビルド

ビルド関連のファイルはすべて [`tools/`](../tools/) にあります。PyInstaller で単一の実行ファイル (macOS は `.app`) を作ります。
スクリプトが OS ごとの仮想環境を作り、`requirements.txt` と異なるパッケージだけをインストールして `tools/ProxyDock.spec` でビルドします。
どのフォルダーから実行しても構いません。

| コマンド | 出力 |
|---|---|
| `tools\build_win.bat` | `dist/win/ProxyDock.exe` (Windows で) |
| `tools\build_linux.bat` / `tools/build_linux.sh` | `dist/linux/ProxyDock` (Linux では直接、Windows・macOS では Docker で) |
| `tools/build_mac.sh` | `dist/mac/ProxyDock.app` (macOS のみ) |
| `tools\build.bat` / `tools/build.sh` | このコンピューターでビルドできるものをすべてビルドし結果を表示 |

macOS ビルドはまだ実機の Mac で確認していません。

## 既知の制限

- Hysteria2 は Python の QUIC 実装 (aioquic) 上で動くため、約 100 Mbps です。帯域指定 (Brutal)、
  Salamander 難読化、ポートホッピングには対応していません。
- 公式 Hysteria2 サーバーは SNI が証明書と一致しないと接続を拒否します - 送信側の SNI にサーバー証明書のドメインを入力してください。
- HTTP 受信側の通常 (CONNECT 以外) のリクエストは 1 接続につき 1 件のみ処理します。
- SOCKS4 にはパスワード認証がないため、この PC だけで使ってください。
- 日本語・中国語はシステムの UI フォントを使います (内蔵の Pretendard はハングル・ラテン文字用)。

## 構成

```
proxydock.py            エントリーポイント
proxydock/
  common.py             共通の定数・ログ・アドレス処理・ブロックリスト
  config.py             設定形式 (v2)、検証、全体/ローカルのマージ、パスワード保管、旧バージョンからの移行
  runtime.py            ProxyInstance / ProxyManager (1 つの asyncio スレッドで複数のプロキシを実行)
  relay.py              TCP/UDP 中継のコア
  inbound.py            受信側プロトコル
  outbound.py           送信側プロトコル
  ss.py                 Shadowsocks 2018/2022 の暗号化
  hysteria2.py          Hysteria2 サーバー・クライアント (aioquic)
  tls.py                自己署名証明書、TLS コンテキスト
  probe.py              接続テスト / 接続確認
  platform_utils.py     ログイン時の起動、ファイアウォール、ポート確認
  i18n.py, locales/     翻訳 (コード内の英語原文 -> <言語>.json)
  ui/                   PySide6 + Fluent Widgets の画面
tools/                  ビルドスクリプト、PyInstaller spec、Linux ビルド用 Dockerfile
licenses/               同梱しているサードパーティのライセンス
docs/                   翻訳した README、スクリーンショット
```

### 翻訳

画面の文言はコード内に英語で書かれ (`_t("...")`)、`proxydock/locales/<言語>.json` で翻訳されます。
翻訳がない文言は英語で表示されます。

## ライセンス

[GPL-3.0](../LICENSE)。UI ライブラリ [PySide6-Fluent-Widgets](https://github.com/zhiyiYo/PyQt-Fluent-Widgets) が
GPL v3 のため、このプログラムも GPL v3 で配布します。ビルドした実行ファイルを配布する場合も GPL v3 の条件 (ソースの提供など) に従う必要があります。

同梱しているフォント・ライブラリのライセンスは [`licenses/`](../licenses/) フォルダーにあります。

| コンポーネント | ライセンス |
|---|---|
| Pretendard, JetBrains Mono (内蔵フォント) | SIL OFL 1.1 |
| PySide6-Fluent-Widgets | GPL v3 |
| Qt 6 / PySide6 / Shiboken6, PySideSix-Frameless-Window | LGPL v3 |
| aioquic, pylsqpack, darkdetect | BSD 3-Clause |
| cryptography | Apache 2.0 / BSD 3-Clause |
| pyOpenSSL | Apache 2.0 |
| service-identity, attrs | MIT |
| certifi | MPL 2.0 |
| Python ランタイム | PSF License |
| PyInstaller ブートローダー | GPL v2+ (ブートローダー例外) |

Copyright (C) 2026 NenkaLab
