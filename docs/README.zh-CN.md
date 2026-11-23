# ProxyDock

[English](../README.md) · [한국어](README.ko.md) · [日本語](README.ja.md) · **简体中文** · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

一个桌面代理"坞站"：可以创建多个代理，并分别开启或关闭。为个人使用而做。

每个代理由 **入站** (在本机端口上接收连接的方式) 和 **出站** (到达目标的方式) 组合而成。
例如"用本地 SOCKS5 接收 → 通过加密隧道发往远程 Shadowsocks 服务器"只需一张卡片。

<p align="center">
  <img src="images/canvas.png" width="98%" alt="画布">
</p>
<p align="center">
  <img src="images/proxies.png" width="49%" alt="代理列表">
  <img src="images/editor.png" width="49%" alt="编辑入站">
</p>

## 功能

| | 支持 |
|---|---|
| 入站 | SOCKS5 · SOCKS4/4a · HTTP · HTTPS (TLS) · Shadowsocks 服务器 · Hysteria2 服务器 · TUN (所有应用) |
| 出站 | Direct · 上游 HTTP / HTTPS / SOCKS4 / SOCKS5 代理 · Shadowsocks 隧道 · Hysteria2 (QUIC) 隧道 · WireGuard · VLESS (TLS / REALITY) · VMess · Trojan · TUIC · 订阅 · 分组 |
| UDP | 入站 SOCKS5 · Shadowsocks · Hysteria2，出站 Direct · Shadowsocks · Hysteria2 · WireGuard · VLESS · VMess · Trojan · TUIC |

- **画布**：蓝图式编辑器 - 从入站块拖线到出站块；多个入站可共用一个出站，把入站连到另一个入站会经由它中继，Direct 可以指定源 IP
- **PC 块**：连接一个 HTTP 入站，在其运行期间用作 Windows 系统代理；之前的设置会被保存，停止或退出应用时恢复 (恢复之前的设置 / 关闭 按钮)
- **链式连接**：连接出站块的'经由'输出后，它到服务器的连接会经由另一个出站 (例如穿过公司 HTTP 代理的 Shadowsocks、Hysteria2 隧道中的 HTTPS 上游代理)，可以串联任意多级
- **路由**: 在入站和出站之间放一个路由块，域名、IP 范围、端口、程序 (chrome.exe 等)、GeoIP 国家、Geosite 分类的
  每条规则都有自己的输出；没有规则匹配时走默认出口。规则可以连接到任何出站、**阻止**块 (拒绝连接) 或另一个路由
- **网络接口**: 任何出站都可以从选定的网卡 (Tailscale、WireGuard、VPN 应用、Wi-Fi) 发出，不改变电脑的路由，只有连到那里的流量才使用它 (sing-box 引擎)
- **WireGuard**: 在 ProxyDock 内运行的 WireGuard 隧道块 - 不需要 VPN 应用，可同时开启多个国家的隧道并按应用分配；可导入服务商的 `.conf`；把一个 WireGuard 块经由另一个即为双重 VPN (sing-box 引擎)
- **TUN**: 捕获本机所有应用 (包括没有代理设置的应用) 的虚拟网卡——连到路由后，程序规则 (game.exe、chrome.exe) 对所有应用生效。若 ProxyDock 不是以管理员身份运行，启动时会请求管理员权限 (UAC) (sing-box 引擎)
- **VLESS / VMess / Trojan / TUIC**: 支持 TLS 或 REALITY (Vision flow) 的 VLESS、Trojan、TUIC v5，可走 TCP、WebSocket 或 gRPC。复制服务商的 `vless://`、`vmess://`、`trojan://`、`tuic://`、`ss://` 或 `hysteria2://` 链接，然后点 **添加出站 → 从分享链接** (或编辑器里的 **粘贴分享链接**)。Cloudflare WARP：把 WARP 的 WireGuard 配置 (例如用 wgcf 生成) 导入 WireGuard 块 (sing-box 引擎)
- **订阅**: 把服务商的订阅 URL 作为包含所有服务器的一个块。可手动选择或自动使用最快的服务器 (每隔几分钟测量并避开失效服务器)。按设定间隔更新，列表加密保存，离线也能启动 (sing-box 引擎)
- **分组**: 分组块把多个出站块当作一个使用: 最快的，或按你设定的顺序第一个可用的 (故障转移)。从它的“成员”输出连接成员; 自动跳过失效服务器 (sing-box 引擎)
- **DNS**: 选择 ProxyDock 自己解析名称时 (Direct、IP 规则、TUN) 使用的 DNS 服务器: PC 的 DNS，或 UDP/TCP、DoT (tls://)、DoH (https://)、DoQ (quic://)、HTTP/3 (h3://)。可全局或按卡片设置，也可以经由卡片的隧道发送。TUN 卡片可使用 Fake-IP，让名称在出口而不是本地网络解析 (sing-box 引擎)
- **引擎**: 默认的 sing-box 为每个代理单独运行一个进程 - 多核、经过链路的 UDP、经由其他出站的 Hysteria2。内置 Python 引擎作为备用 (设置 → 代理引擎)
- **Shadowsocks**：AEAD 2018 (aes-128/256-gcm、chacha20-ietf-poly1305) 和 2022 (SIP022、blake3)，TCP+UDP，复制 `ss://` 链接
- **Hysteria2**：已确认与官方实现双向互通 (TCP、UDP)，证书指纹固定，复制 `hysteria2://` 链接
- **设置**：全局默认值 + 每个代理的本地设置 (本地优先) - 允许的域名、阻止内网、空闲超时、日志
- **编辑页面的检查连接**：保存前测试出站 (上游代理/隧道、密码) 和入站端口
- **连接测试**：通过运行中的代理以 HTTPS 连接多个国家的 21 个网站并测量响应时间
- **日志 / 统计**：按代理查看，实时速度图，可选写入文件
- **安全**：密码用 Windows DPAPI 保存 (其他系统为 0600 权限文件)，按 IP 限制连接数，
  多次认证失败后临时封禁，防止代理链循环，Windows 防火墙外部阻止按钮
- **语言**：English (默认)、한국어、日本語、简体中文、繁體中文、Deutsch、Français、Español (设置 → 显示 → 语言)
- 托盘常驻、开机启动、浅色/深色主题、内置字体 (Pretendard / Pretendard JP / Noto Sans SC·TC / JetBrains Mono)

## 运行

如果 [Releases](https://github.com/NenkaLab/proxy-dock/releases) 中有构建好的文件可直接使用，也可以从源码运行：

```bash
python -m venv .venv
# Windows: .venv\Scripts\activate   /   macOS、Linux: source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py   # sing-box 引擎 (没有时代理使用 Python 引擎)
python proxydock.py            # --hidden : 不显示窗口，直接启动到托盘
```

基于 Python 3.14 开发和测试。

首次启动时没有代理。在 **新建代理** 中选择模板 (本地 SOCKS5、本地 HTTP、Shadowsocks 服务器、
本地 → Shadowsocks 隧道等) 创建，然后用卡片上的开关运行。详细用法见应用中的 **使用方法** 页面。
界面语言默认为英语，可在 设置 → 显示 → 语言 中更改 (重启后生效)。

设置保存位置

| 系统 | 路径 |
|---|---|
| Windows | `%APPDATA%\ProxyDock` |
| macOS | `~/Library/Application Support/ProxyDock` |
| Linux | `$XDG_CONFIG_HOME/ProxyDock` (默认 `~/.config/ProxyDock`) |

## 构建

构建相关的文件都在 [`tools/`](../tools/) 中。用 PyInstaller 生成单个可执行文件 (macOS 为 `.app`)。
脚本会为每个系统创建虚拟环境，只安装与 `requirements.txt` 不一致的包，然后用 `tools/ProxyDock.spec` 构建。
可以在任意文件夹中运行。

| 命令 | 输出 |
|---|---|
| `tools\build_win.bat` | `dist/win/ProxyDock.exe` (在 Windows 上) |
| `tools\build_linux.bat` / `tools/build_linux.sh` | `dist/linux/ProxyDock` (Linux 上直接构建，Windows/macOS 上通过 Docker) |
| `tools/build_mac.sh` | `dist/mac/ProxyDock.app` (仅 macOS) |
| `tools\build.bat` / `tools/build.sh` | 构建本机能构建的全部目标并汇总结果 |

macOS 构建尚未在真实的 Mac 上验证。

## 发布与 CI

- 每次推送和 PR 都会在 Windows、Linux、macOS 上运行测试 ([CI](../.github/workflows/ci.yml)，
  本地运行 `python tests/run_tests.py` - 无需联网)。
- `tools\release.bat 2.1.0` (或 `python tools/release.py 2.1.0`) 会检查工作区、运行测试、提升 `__version__`、
  提交、创建 `v2.1.0` 标签并推送。标签会触发[发布工作流](../.github/workflows/release.yml)，构建 Windows / Linux / macOS
  并把压缩包和 `SHA256SUMS.txt` 发布到 GitHub Release。加上 `--dry-run` 只做预演。
- `python tools/build.py package win|linux|mac` 可在本地 `dist/release/` 生成相同的压缩包。

## 已知限制

- 订阅只读取分享链接列表 (纯文本或 base64)，不支持 Clash YAML。“自动选择最快”每隔几分钟获取一个小测试页 (gstatic.com generate_204) 来测量服务器。
- 分组不会把流量分摊到多个服务器 (负载均衡; sing-box 1.14 没有这种出站)。故障转移后，分组会一直使用新成员，直到它也失效。
- 隧道和上游代理仍像以前一样在服务器端解析域名; DNS 设置作用于在本机进行的解析。Fake-IP 尚未在真实环境 (TUN) 中测试。没有按域名的 DNS 规则 (部分名称使用其他服务器)。
- TUN 尚未在真实环境中测试。同一时间只能有一个 TUN 卡片；被捕获应用的 DNS 由本机的 ProxyDock 应答；
  严格路由在 Windows 上可能破坏 IPv6 (默认关闭)。在 macOS/Linux 上需要以 root 身份运行 ProxyDock。
- 路由和阻止块需要 sing-box 引擎。IP 和 GeoIP 规则会先在本机解析域名，因此其后的隧道直接连接 IP，而不是在远端解析域名。
  程序规则只对来自本机的连接有效。
- sing-box 引擎: 对其他电脑开放的 Hysteria2 服务器使用官方 Hysteria2 核心运行，核心每次连接都向 ProxyDock 询问认证
  (sing-box 不报告密码错误，无法封禁 IP)；收到的流量再经 sing-box 发出。证书固定的卡片仍使用 Python 引擎 (需要 sing-box 1.15)。
  单 IP 连接上限是事后断开多出的连接；TCP 空闲超时和“记录连接关闭详情”每秒检查一次，所以很短的连接不会留下关闭记录。
- Python 引擎: Hysteria2 不能*经由*其他出站，串联的出站不转发 UDP，Hysteria2 (aioquic) 约 100 Mbps。
  Hysteria2 带宽设置 (Brutal)、Salamander 和端口跳跃需要 sing-box 引擎；监听端口范围的服务器需要在该服务器上做端口转发 (ProxyDock 不会设置)。
- 官方 Hysteria2 服务器 (以及其他检查名称的服务器) 会拒绝与证书不符的 SNI。出站使用 IP 时，点 SNI 栏旁的 **从服务器检测** 读取证书中的名称。
- HTTP 入站的普通 (非 CONNECT) 请求每个连接只处理一个。
- SOCKS4 协议没有密码认证，请仅在本机使用。

## 结构

```
proxydock.py            入口
proxydock/
  common.py             公共常量、日志、地址处理、封禁列表
  config.py             设置格式 (v2)、校验、全局/本地合并、密码保存、旧版本迁移
  graph.py              块图 (入站 / 出站 / PC 块) -> 运行时代理
  runtime.py            ProxyInstance / ProxyManager (在一个 asyncio 线程中运行多个代理)
  relay.py              TCP/UDP 中继核心
  inbound.py            入站协议
  outbound.py           出站协议
  ss.py                 Shadowsocks 2018/2022 加密
  hysteria2.py          Hysteria2 服务器和客户端 (aioquic)
  singbox/              sing-box 引擎: 配置转换、进程运行、封禁与单 IP 连接上限
  tls.py                自签名证书、TLS 上下文
  probe.py              连接测试 / 检查连接
  platform_utils.py     开机启动、防火墙、端口检查
  i18n.py, locales/     翻译 (代码中的英文原文 -> <语言>.json)
  ui/                   PySide6 + Fluent Widgets 界面
    canvas.py           画布页面 (NodeGraphQt)
tools/                  构建脚本、PyInstaller spec、Linux 构建用 Dockerfile
licenses/               附带的第三方组件许可证
docs/                   翻译的 README、截图
```

### 翻译

界面文字在代码中以英文书写 (`_t("...")`)，通过 `proxydock/locales/<语言>.json` 翻译。
没有翻译的文字会以英文显示。

## 许可证

[GPL-3.0](../LICENSE)。界面库 [PySide6-Fluent-Widgets](https://github.com/zhiyiYo/PyQt-Fluent-Widgets) 使用
GPL v3，因此本程序也以 GPL v3 发布；分发构建好的可执行文件时同样需要遵守 GPL v3 条款 (提供源码等)。

附带的字体和库的许可证位于 [`licenses/`](../licenses/) 文件夹。

| 组件 | 许可证 |
|---|---|
| Pretendard, Pretendard JP, Noto Sans SC/TC, JetBrains Mono (内置字体) | SIL OFL 1.1 |
| PySide6-Fluent-Widgets | GPL v3 |
| sing-box | GPL v3 |
| Hysteria2 | MIT |
| Qt 6 / PySide6 / Shiboken6, PySideSix-Frameless-Window | LGPL v3 |
| aioquic, pylsqpack, darkdetect | BSD 3-Clause |
| cryptography | Apache 2.0 / BSD 3-Clause |
| pyOpenSSL | Apache 2.0 |
| service-identity, attrs, NodeGraphQt, Qt.py | MIT |
| certifi | MPL 2.0 |
| Python 运行时 | PSF License |
| PyInstaller 引导程序 | GPL v2+ (引导程序例外) |

Copyright (C) 2026 NenkaLab
