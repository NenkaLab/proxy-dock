# ProxyDock

[English](../README.md) · **한국어** · [日本語](README.ja.md) · [简体中文](README.zh-CN.md) · [繁體中文](README.zh-TW.md) · [Deutsch](README.de.md) · [Français](README.fr.md) · [Español](README.es.md)

여러 프록시와 터널을 나란히 돌리는 데스크톱 앱입니다. 캔버스에서 **받는 쪽** 블록(트래픽이 들어오는 곳, 예: 로컬 SOCKS5 포트)을
**내보내는 쪽** 블록(나가는 방법, 예: Shadowsocks 서버)에 잇고, 카드마다 켜고 끕니다.

<sub>개인적으로 쓰려고 만든 것을 그대로 공유합니다.</sub>

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

- **캔버스에서 블록 잇기**: 받는 쪽(SOCKS5, HTTP, Shadowsocks / Hysteria2 / WireGuard 서버, TUN 등)을
  내보내는 쪽(Direct, 상위 프록시, Shadowsocks, Hysteria2, WireGuard, VLESS, VMess, Trojan, TUIC, 구독)에 연결
- **라우팅**: 도메인, IP, 프로그램, 국가별로 나눠 보내기. IF 스위치는 킬 스위치로, 그룹은 가장 빠른 서버나 자동 전환으로
- **체인**: 출구를 다른 출구, 또는 Tailscale·VPN 앱·Wi-Fi를 거쳐 내보내기
- **시스템 프록시**: 카드를 켜져 있는 동안 Windows 시스템 프록시로 사용
- **편의 기능**: 공유 링크, 연결 테스트, 로그와 실시간 통계, 8개 언어, 트레이

프록시는 [sing-box](https://github.com/SagerNet/sing-box)로 돌아가며 릴리스에 포함되어 있습니다.

## 실행

Windows에서는 `ProxyDock-<버전>-windows-x64-setup.exe`가 현재 사용자에게 설치합니다(관리자 권한 필요 없음). `.zip`은 같은 앱을 폴더로 담은 것입니다. macOS는 `.dmg`를 열고 ProxyDock을 응용 프로그램 폴더로 끌어 넣습니다. Linux는 `.AppImage`에 실행 권한을 주고 실행합니다.

[Releases](https://github.com/NenkaLab/proxy-dock/releases)에서 빌드를 받습니다.

설정은 `%APPDATA%\ProxyDock`(macOS: `~/Library/Application Support/ProxyDock`, Linux: `~/.config/ProxyDock`)에
저장됩니다. 나머지는 앱의 **사용법** 페이지에 있습니다.

## 빌드

소스로 실행, 테스트, 빌드, 설치 파일, 릴리스: [Build.md](Build.md) (영어).

## 제한

- Router, 그룹, WireGuard, VLESS / VMess / Trojan / TUIC, TUN은 sing-box 엔진(포함됨)이 필요합니다.
- TUN과 macOS는 아직 실제 환경에서 시험하지 않았습니다.
- SOCKS4는 비밀번호가 없으니 이 PC에서만 쓰세요.

## 라이선스

[GPL-3.0](../LICENSE). 포함된 구성 요소(sing-box GPL-3.0, Hysteria MIT, Qt / PySide6 LGPL-3.0, 글꼴 OFL-1.1 등)와
라이선스는 [`licenses/`](../licenses/)에 있습니다.

Copyright (C) 2026 NenkaLab
