# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · **日本語** · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

複数のプロキシを作成し、それぞれオン/オフできるデスクトップ用プロキシ「ドック」です。個人用に作りました。

プロキシは **受信側** (この PC のポートで接続を受ける方式) と **送信側** (宛先まで送る方式) の組み合わせです。
たとえば「ローカル SOCKS5 で受けて → リモート Shadowsocks サーバーへ暗号化トンネル」という構成をカード 1 枚で作れます。

<p align="center">
  <img src="images/canvas.png" width="98%" alt="キャンバス">
</p>
<p align="center">
  <img src="images/proxies.png" width="49%" alt="プロキシ一覧">
  <img src="images/editor.png" width="49%" alt="受信側の編集">
</p>

## 機能

| | 対応 |
|---|---|
| 受信側 | SOCKS5 · SOCKS4/4a · HTTP · HTTPS (TLS) · Shadowsocks サーバー · Hysteria2 サーバー · TUN (すべてのアプリ) |
| 送信側 | Direct · 上位 HTTP / HTTPS / SOCKS4 / SOCKS5 プロキシ · Shadowsocks トンネル · Hysteria2 (QUIC) トンネル · WireGuard · VLESS (TLS / REALITY) · VMess · Trojan · TUIC · サブスクリプション · グループ |
| UDP | 受信 SOCKS5 · Shadowsocks · Hysteria2、送信 Direct · Shadowsocks · Hysteria2 · WireGuard · VLESS · VMess · Trojan · TUIC |

- **キャンバス**: ブループリント風エディター - 受信側ブロックから送信側ブロックへ線を引いてつなぎます。複数の受信側で 1 つの送信側を共有でき、受信側を別の受信側につなぐとそこを経由して中継し、Direct は送信元 IP を選べます
- **PC ブロック**: HTTP 受信側をつなぐと、実行中は Windows のシステムプロキシとして使います。以前の設定は保存しておき、停止時やアプリ終了時に元に戻します (以前に戻す / オフ ボタン)
- **チェーン**: 送信側ブロックの「経由」出力をつなぐと、そのサーバーへの接続が別の送信側を経由します (例: 会社の HTTP プロキシ越しの Shadowsocks、Hysteria2 トンネル内の HTTPS 上位プロキシ)。何段でもつなげます
- **Router**: 受信側と送信側の間に Router ブロックを置くと、ドメイン、IP 範囲、ポート、プログラム (chrome.exe など)、
  GeoIP の国、Geosite の分類のルールごとに出力ができます。合うルールがなければ既定の出口へ。ルールは任意の送信側、
  **ブロック** (接続を拒否)、別の Router につなげます
- **ネットワークインターフェース**: どの送信側も選んだネットワークカード (Tailscale、WireGuard、VPN アプリ、Wi-Fi) から出せます。PC のルートは変えないので、そこにつないだ通信だけがそのカードを使います (sing-box エンジン)
- **WireGuard**: ProxyDock の中で動く WireGuard トンネルブロック - VPN アプリ不要なので、複数の国のトンネルを同時に使いアプリごとに分けられます。事業者の `.conf` を読み込めて、WireGuard ブロックを別の WireGuard ブロック経由にすると二重 VPN になります (sing-box エンジン)
- **TUN**: この PC のすべてのアプリ (プロキシ設定のないアプリも) を捕まえる仮想ネットワークカード。Router につなぐとプログラムのルール (game.exe、chrome.exe) がすべてのアプリに効きます。ProxyDock が管理者でなければ起動時に管理者権限 (UAC) を求めます (sing-box エンジン)
- **VLESS / VMess / Trojan / TUIC**: TLS または REALITY (Vision flow) の VLESS、Trojan、TUIC v5 を TCP・WebSocket・gRPC で使えます。事業者の `vless://`、`vmess://`、`trojan://`、`tuic://`、`ss://`、`hysteria2://` リンクをコピーして **送信側を追加 → 共有リンクから** (またはエディターの **共有リンクを貼り付け**) を押します。Cloudflare WARP は WARP の WireGuard プロファイル (wgcf などで作成) を WireGuard ブロックに読み込みます (sing-box エンジン)
- **サブスクリプション**: 事業者のサブスクリプション URL を、全サーバーを含む 1 つのブロックとして使います。サーバーを選ぶか最速を自動で使えます (数分ごとに測定し、止まったサーバーは避けます)。定期的に更新し、一覧を暗号化して保存するのでオフラインでも起動します (sing-box エンジン)
- **グループ**: グループブロックは複数の送信側ブロックを 1 つとして使います。最速のもの、または設定した順で最初に動作するもの (自動切り替え) を使います。メンバーは「メンバー」出力からつなぎます。止まったサーバーは自動で飛ばします (sing-box エンジン)
- **DNS**: ProxyDock 自身が名前を調べる場所 (Direct、IP ルール、TUN) で使う DNS サーバーを選べます。PC の DNS、または UDP/TCP、DoT (tls://)、DoH (https://)、DoQ (quic://)、HTTP/3 (h3://)。全体またはカードごとに設定でき、カードのトンネル経由にもできます。TUN カードは Fake-IP を使うと、名前をローカルネットワークではなく出口で調べます (sing-box エンジン)
- **エンジン**: 既定の sing-box はプロキシごとに別プロセスで動きます - マルチコア、チェーン経由の UDP、他の送信側を経由する Hysteria2。内蔵の Python エンジンは代替用です (設定 → プロキシエンジン)
- **Shadowsocks**: AEAD 2018 (aes-128/256-gcm, chacha20-ietf-poly1305) と 2022 (SIP022, blake3)、TCP+UDP、`ss://` リンクのコピー
- **Hysteria2**: 公式実装と双方向で互換性を確認 (TCP・UDP)、証明書フィンガープリントの固定、`hysteria2://` リンクのコピー
- **設定**: 全体の既定値 + プロキシごとのローカル設定 (ローカル優先) - 許可ドメイン、内部ネットワークのブロック、アイドルタイムアウト、ログ
- **編集画面の接続確認**: 保存前に送信側 (上位プロキシ・トンネル・パスワード) と受信側のポートを試せます
- **接続テスト**: 実行中のプロキシ経由で各国のサイト 21 か所に HTTPS で接続し、応答時間を測定
- **ログ / 統計**: プロキシごとに表示、リアルタイムの速度グラフ、ファイルへのログ (任意)
- **セキュリティ**: パスワードは Windows DPAPI (他の OS は 0600 権限のファイル) で保管、IP ごとの接続上限、
  認証失敗が続いたら一時ブロック、プロキシチェーンのループ防止、Windows ファイアウォールの外部ブロックボタン
- **言語**: English (既定)、한국어、日本語、简体中文、繁體中文、Deutsch、Français、Español (設定 → 表示 → 言語)
- トレイ常駐、ログイン時に起動、ライト/ダークテーマ、内蔵フォント (Pretendard / Pretendard JP / Noto Sans SC·TC / JetBrains Mono)

## 実行

[Releases](https://github.com/NenkaLab/proxy-dock/releases) にビルド済みファイルがあればそれを使うか、ソースから直接実行します。

```bash
python -m venv .venv
# Windows: .venv\Scripts\activate   /   macOS・Linux: source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py   # sing-box エンジン (ない場合は Python エンジンで動作)
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

## リリースと CI

- プッシュと PR のたびに Windows・Linux・macOS でテストが実行されます ([CI](../.github/workflows/ci.yml)、
  ローカルでは `python tests/run_tests.py` - インターネット不要)。
- `tools\release.bat 2.1.0` (または `python tools/release.py 2.1.0`) は作業ツリーを確認してテストを実行し、
  `__version__` を上げてコミット、`v2.1.0` タグを作成してプッシュします。タグが[リリースワークフロー](../.github/workflows/release.yml)を
  起動し、Windows / Linux / macOS をビルドしてアーカイブと `SHA256SUMS.txt` を GitHub Release に公開します。
  `--dry-run` で事前確認のみ行えます。
- `python tools/build.py package win|linux|mac` で同じアーカイブをローカルの `dist/release/` に作れます。

## 既知の制限

- サブスクリプションは共有リンクの一覧 (そのまま、または base64) だけを読み、Clash YAML は読みません。「最速を自動で選ぶ」は数分ごとに小さなテストページ (gstatic.com generate_204) を取得してサーバーを測ります。
- グループは通信を複数のサーバーに分散しません (負荷分散。sing-box 1.14 にはその送信側がありません)。自動切り替えのあとは、そのメンバーも止まるまで新しいメンバーを使い続けます。
- トンネルや上位プロキシはこれまでどおりサーバー側でドメインを解決します。DNS 設定はこの PC で行う名前解決に効きます。Fake-IP はまだ実際の環境で試していません (TUN と一緒に)。ドメインごとの DNS ルール (一部の名前だけ別のサーバー) はありません。
- TUN はまだ実際の環境で試していません。TUN カードは同時に 1 つで、捕まえたアプリの DNS はこの PC で ProxyDock が
  答えます。厳格なルートは Windows で IPv6 を壊すことがあるため既定でオフです。macOS/Linux では ProxyDock 自体を root で実行する必要があります。
- Router とブロックには sing-box エンジンが必要です。IP・GeoIP のルールはこの PC で先にドメインを解決するため、その後ろの
  トンネルはドメインをリモートで解決せず IP に接続します。プログラムのルールはこの PC からの接続にだけ効きます。
- sing-box エンジン: 他の PC に公開した Hysteria2 サーバーは公式 Hysteria2 コアで動き、コアが接続ごとに ProxyDock に認証を問い合わせます
  (sing-box はパスワード誤りを通知しないため IP ブロックが効かない)。受けた通信は sing-box を通って出ます。証明書ピン留めのカードは
  引き続き Python エンジンで動きます (sing-box 1.15 が必要)。IP ごとの接続上限は超えた接続を後から切断し、
  TCP のアイドルタイムアウトと「接続終了の詳細を記録」は 1 秒ごとに確認するので、ごく短い接続には終了の行が出ません。
- Python エンジン: Hysteria2 は別の送信側を*経由*できず、チェーンでつないだ送信側では UDP を送らず、
  Hysteria2 (aioquic) は約 100 Mbps です。Hysteria2 の帯域指定 (Brutal)、Salamander、ポートホッピングには sing-box エンジンが必要です。ポート範囲で待ち受けるサーバーにはそのサーバーでのポート転送が必要です (ProxyDock は設定しません)。
- 公式 Hysteria2 サーバー (名前を確認する他のサーバーも) は証明書と異なる SNI を拒否します。送信側が IP を使うなら、SNI 欄の横の **サーバーから検出** を押して証明書の名前を入れてください。
- HTTP 受信側の通常 (CONNECT 以外) のリクエストは 1 接続につき 1 件のみ処理します。
- SOCKS4 にはパスワード認証がないため、この PC だけで使ってください。

## 構成

```
proxydock.py            エントリーポイント
proxydock/
  common.py             共通の定数・ログ・アドレス処理・ブロックリスト
  config.py             設定形式 (v2)、検証、全体/ローカルのマージ、パスワード保管、旧バージョンからの移行
  graph.py              ブロックグラフ (受信側 / 送信側 / PC ブロック) -> 実行用プロキシ
  runtime.py            ProxyInstance / ProxyManager (1 つの asyncio スレッドで複数のプロキシを実行)
  relay.py              TCP/UDP 中継のコア
  inbound.py            受信側プロトコル
  outbound.py           送信側プロトコル
  ss.py                 Shadowsocks 2018/2022 の暗号化
  hysteria2.py          Hysteria2 サーバー・クライアント (aioquic)
  singbox/              sing-box エンジン: 設定変換、プロセス実行、ブロック・IP ごとの接続上限
  tls.py                自己署名証明書、TLS コンテキスト
  probe.py              接続テスト / 接続確認
  platform_utils.py     ログイン時の起動、ファイアウォール、ポート確認
  i18n.py, locales/     翻訳 (コード内の英語原文 -> <言語>.json)
  ui/                   PySide6 + Fluent Widgets の画面
    canvas.py           キャンバス画面 (NodeGraphQt)
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
| Pretendard, Pretendard JP, Noto Sans SC/TC, JetBrains Mono (内蔵フォント) | SIL OFL 1.1 |
| PySide6-Fluent-Widgets | GPL v3 |
| sing-box | GPL v3 |
| Hysteria2 | MIT |
| Qt 6 / PySide6 / Shiboken6, PySideSix-Frameless-Window | LGPL v3 |
| aioquic, pylsqpack, darkdetect | BSD 3-Clause |
| cryptography | Apache 2.0 / BSD 3-Clause |
| pyOpenSSL | Apache 2.0 |
| service-identity, attrs, NodeGraphQt, Qt.py | MIT |
| certifi | MPL 2.0 |
| Python ランタイム | PSF License |
| PyInstaller ブートローダー | GPL v2+ (ブートローダー例外) |

Copyright (C) 2026 NenkaLab
