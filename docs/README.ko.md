# ProxyDock

[English](../README.md) · **한국어** · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

여러 프록시와 터널을 나란히 돌리는 데스크톱 앱입니다. 캔버스에서 **받는 쪽** 블록(트래픽이 들어오는 곳, 예: 로컬 SOCKS5 포트)을
**내보내는 쪽** 블록(나가는 방법, 예: Shadowsocks 서버)에 잇고, 카드마다 켜고 끕니다.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="images/canvas-dark.png">
    <img src="images/canvas-light.png" width="98%" alt="캔버스">
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

## 기능

- **받는 쪽**: SOCKS5, SOCKS4, HTTP, HTTPS, Shadowsocks 서버, Hysteria2 서버, WireGuard 서버(휴대폰·PC가 WireGuard 앱으로 접속), 포트 포워딩, TUN(이 PC의 모든 앱)
- **내보내는 쪽**: Direct, HTTP / HTTPS / SOCKS 프록시, Shadowsocks, Hysteria2, WireGuard, VLESS(REALITY), VMess,
  Trojan, TUIC, 구독(공유 링크 또는 Clash YAML)
- **블록**: 체인(출구를 다른 출구로 거치기), Router(도메인·IP·포트·프로그램(실행 중인 목록에서 선택)·GeoIP/Geosite 규칙, 간편·고급 화면), IF(조건 하나, 또는 킬 스위치 같은 수동 켜기/끄기 스위치), 그룹(가장 빠른 것, 자동 전환, 직접 선택), 차단
- **네트워크 인터페이스**: PC의 라우팅을 바꾸지 않고 출구를 Tailscale, VPN 앱, Wi-Fi로 내보냄
- **DNS**: PC의 DNS, DoH / DoT / DoQ, 카드별 설정, 터널을 거쳐 보내기, TUN용 Fake-IP
- **카드**: 블록에서 바로 실행, 앱 시작 시 실행, 자동 재시작(멈추거나 시작에 실패하면)
- **PC 블록**: HTTP 받는 쪽을 켜져 있는 동안 Windows 시스템 프록시로 사용
- 공유 링크(`ss://`, `vless://`, `vmess://`, `trojan://`, `tuic://`, `hysteria2://`), 연결 확인·테스트, 로그와 실시간 통계,
  8개 언어, 트레이, 로그인 시 시작

프록시는 [sing-box](https://github.com/SagerNet/sing-box)로 돌아갑니다(카드마다 프로세스 하나, 릴리스에 포함).
다른 PC에 연 Hysteria2 서버는 공식 [Hysteria](https://github.com/apernet/hysteria) 코어로 돌고, 내장 Python 엔진은 대체용입니다.

## 실행

[Releases](https://github.com/NenkaLab/proxy-dock/releases)에서 빌드를 받거나, 소스로 실행합니다(Python 3.14):

```bash
python -m venv .venv
.venv\Scripts\activate            # macOS / Linux: source .venv/bin/activate
pip install -r requirements.txt
python tools/fetch_singbox.py     # 이 PC용 sing-box와 Hysteria
python proxydock.py
```

설정은 `%APPDATA%\ProxyDock`(macOS: `~/Library/Application Support/ProxyDock`, Linux: `~/.config/ProxyDock`)에
저장됩니다. 나머지는 앱의 **사용법** 페이지에 있습니다.

## 빌드

`tools\build_win.bat` → `dist/win/ProxyDock.exe` (Linux: `tools/build_linux.sh`, macOS: `tools/build_mac.sh`).
빌드는 sing-box와 Hysteria를 내려받아(SHA-256 확인) 앱 안에 넣습니다.
테스트: `python tests/run_tests.py`. 릴리스: `tools\release.bat <버전>`이 태그를 달고 push하면 GitHub Actions가 빌드합니다.

## 제한

- Router, 그룹, WireGuard, VLESS / VMess / Trojan / TUIC, TUN, 인터페이스 선택, DNS 서버 지정은 sing-box 엔진이 필요합니다.
- 여러 서버로 나눠 보내는 부하 분산은 없고, Hysteria2 서버는 혼자서 포트 범위로 받을 수 없습니다.
- 인증서 고정은 sing-box 1.15 전까지 Python 엔진으로 돕니다.
- TUN과 Fake-IP는 아직 실제 환경에서 시험하지 않았고, macOS도 실제 Mac에서 시험하지 않았습니다.
- SOCKS4는 비밀번호가 없으니 이 PC에서만 쓰세요.

## 라이선스

[GPL-3.0](../LICENSE). 포함된 구성 요소(sing-box GPL-3.0, Hysteria MIT, Qt / PySide6 LGPL-3.0, 글꼴 OFL-1.1 등)와
라이선스는 [`licenses/`](../licenses/)에 있습니다.

Copyright (C) 2026 NenkaLab
