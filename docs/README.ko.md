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
- **회선 직결**: 고른 블록만 네트워크 카드로 바로 내보내기. PC 전체 네트워크를 잡는 VPN, TUN, 다른 앱을 그 앱의
  스플릿 터널링 없이 건너뜀 (Windows: 작은 Hyper-V VM, Linux: 네트워크 네임스페이스, macOS: 작은 VM, 아직 미시험)
- **홈 화면**: 앱 전체 스위치(끄면 모든 연결 거부, 킬 스위치. 각 카드의 켜짐/꺼짐은 그대로), 카드 상태, 실시간
  트래픽, 회선 직결 VM

프록시는 [sing-box](https://github.com/SagerNet/sing-box)로 돌아가며 릴리스에 포함되어 있습니다.

## 실행

[Releases](https://github.com/NenkaLab/proxy-dock/releases)에서 받습니다.

- **Windows 10 / 11**: `ProxyDock-<버전>-windows-x64-setup.exe`가 현재 사용자에게 설치합니다(관리자 권한 필요 없음).
  `.zip`은 같은 앱을 폴더로 담은 것입니다.
- **macOS**(Apple Silicon): `.dmg`를 열어 ProxyDock을 응용 프로그램 폴더로 끌어 넣습니다. Apple 서명이 없어서 처음
  한 번은 오른쪽 클릭 → 열기로 엽니다.
- **Linux**(x64): `.AppImage`를 그대로 실행합니다(먼저 `chmod +x`). `.tar.gz`는 같은 앱을 폴더로 담은 것입니다.

설정은 `%APPDATA%\ProxyDock`(Windows), `~/Library/Application Support/ProxyDock`(macOS),
`~/.config/ProxyDock`(Linux)에 저장되며, 버전 3은 2.x의 설정을 그대로 읽어 씁니다. 나머지는 앱의 **사용법**
페이지에 있습니다. ProxyDock 3은 Rust로 새로 만든 버전이며, 2.x(Python 앱)는 [`old`](https://github.com/NenkaLab/proxy-dock/tree/old) 브랜치에 있습니다.

## 빌드

소스로 실행, 테스트, 빌드, 설치 파일, 릴리스: [Build.md](Build.md) (영어). 릴리스 전 확인 방법:
[TESTING.md](../TESTING.md).

## 제한

- macOS와 Linux는 자동 빌드·테스트는 하지만 실제 기기에서는 아직 확인하지 않았습니다.
- macOS / Linux: 다른 PC를 막는 방화벽 규칙이 없고(시스템 방화벽을 쓰세요), 시작할 때 "관리자로 실행"이 없으며(TUN
  카드는 시작할 때 암호를 묻습니다), 모니터 색 프로파일을 읽지 않고(sRGB로 봄), 설정의 비밀번호를 암호화하지
  않으며(2.x와 같음), 마우스 뒤로/앞으로 버튼으로 이동하지 않습니다.
- Linux: 시스템 프록시(PC 블록)는 GNOME과 KDE에서만 설정됩니다. 알림에는 `notify-send`가 필요합니다.
- TUN은 아직 실제 환경에서 시험하지 않았습니다.
- SOCKS4는 비밀번호가 없으니 이 PC에서만 쓰세요.
- 회선 직결: Windows에서는 Hyper-V(Pro, Enterprise, Education)와 관리자 권한이 필요합니다. VM마다 공유기에 새
  기기(새 MAC·IP 주소)로 보이고, 회선당 주소를 하나만 주는 인터넷 회사라면 주소를 받지 못할 수 있습니다. 이
  트래픽은 실제 IP 주소가 드러나며 VPN이 보호하지 않습니다. 사용에 대한 책임은 사용자에게 있습니다(앱이 VM을
  만들기 전에 동의를 받습니다).
- Linux의 회선 직결: 컨테이너에서만 시험했습니다(macvlan을 쓰는 네트워크 네임스페이스. Wi-Fi는 ipvlan을 쓰는데 DHCP가
  안 될 수 있으니 고정 주소를 쓰세요). `ip`(iproute2)와 pkexec를 통한 root 권한이 필요합니다.
- macOS(Apple Silicon)의 회선 직결: 코드와 타입 검사까지만 했고 **실제 Mac에서는 아직 한 번도 시험하지 않았습니다**
  (Virtualization.framework와 브리지 vmnet 인터페이스. 도우미가 root로 실행되어야 합니다).

## 기여

버그, 아직 시험하지 않은 플랫폼(macOS, Linux), 번역 등 직접 고치거나 개선한 것이 있으면 PR을 보내 주세요. 직접
고쳤든 AI를 썼든 상관없지만, 보내기 전에 꼭 검증해 주세요. 빌드하고, 테스트를 돌리고(`native/`에서
`cargo test --workspace`), 앱에서 바뀐 부분을 직접 써 보세요(필요하면 실제 기기에서). PR에는 무엇을 어떻게
확인했는지 적어 주세요.

## 라이선스

[GPL-3.0](../LICENSE). 포함된 구성 요소(sing-box GPL-3.0, Hysteria MIT, Slint GPL-3.0, Rust 크레이트, 글꼴 OFL-1.1 등)와
라이선스는 [`licenses/`](../licenses/)에 있습니다.

Copyright (C) 2026 NenkaLab
