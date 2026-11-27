# 테스트 방법

릴리스 전에 확인하는 순서입니다. **자동 테스트**(CI가 push마다 돌림)와, 자동으로 할 수 없어 **사람이 직접 확인하는 것**으로 나뉩니다.

## 자동 테스트

모두 `native/` 폴더에서 실행합니다. GitHub Actions의 CI(`.github/workflows/ci.yml`)가 main push·PR마다 같은 것을 Windows·Linux·macOS에서 돌리고, 각 OS의 릴리스 빌드(`proxydock`, `proxydock-ui`)를 아티팩트로 올립니다. 릴리스(`release.yml`)는 세 OS의 설치본(Windows 설치기·zip, Linux AppImage·tar.gz, macOS dmg·zip)을 만들며, Linux·macOS는 실제 기기에서 아직 확인하지 않아 실패해도 Windows만으로 릴리스됩니다.

```powershell
python ..\tools\fetch_singbox.py      # sing-box·Hysteria 엔진 (처음 한 번, bin\windows-amd64)
cargo fmt --check                    # 코드 형식
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace               # 코어·IPC·엔진·매니저·앱 테스트 (엔진 테스트는 실제 sing-box를 띄움)
```

- 테스트는 실제 설정·레지스트리를 건드리지 않습니다(임시 폴더, `HKCU\Software\ProxyDockTest`).

### Python 앱과의 동등성 (CI의 "parity" 작업)

Rust 코어가 이전 Python 앱과 같은 결과를 내는지 무작위 설정·링크·편집으로 비교합니다. 기준은 Python 앱이 main에서 빠지기 직전 커밋 `1e882fc`입니다. 저장소 루트에서 실행합니다.

```powershell
git worktree add --detach ..\proxy-py 1e882fc
python -m pip install -r ..\proxy-py\requirements.txt
$env:PROXYDOCK_PY = "..\proxy-py"; $env:QT_QPA_PLATFORM = "offscreen"
python native\tools\parity_all.py 150
```

- 마지막 줄에 `all parity checks passed`가 나오면 통과입니다.

### 설치본 만들기

```powershell
python tools\build_rust.py           # dist\release-rust 에 zip·설치기 (각각 .sha256 포함)
```

- 버전은 `native/Cargo.toml`의 `version`입니다. 설치기를 만들려면 Inno Setup 6이 필요합니다(`winget install JRSoftware.InnoSetup`).

## 사람이 직접 확인하는 것

자동 테스트와 창 확인으로는 다룰 수 없는 기능들입니다. 모두 Windows 시스템(레지스트리·방화벽·UAC·드라이버)을 실제로 바꾸므로, 사람이 직접 눌러 확인해야 합니다. 항목마다 **되돌리는 방법**을 함께 적었습니다.

### 준비

- 설치본(`dist/release-rust/ProxyDock-<버전>-windows-x64-setup.exe`)이나 `native/target/release/proxydock.exe`로 실행합니다.
- 평소 설정을 건드리지 않으려면 PowerShell에서 임시 설정 폴더로 띄웁니다.
  ```powershell
  $env:PROXYDOCK_HOME = "$env:TEMP\proxydock-check"; & "C:\proxy\native\target\release\proxydock.exe"
  ```
  - 단, 아래의 시작 시 실행·방화벽·시스템 프록시는 설정 폴더와 상관없이 **실제 시스템**에 적용됩니다.
- Python ProxyDock이 켜져 있으면 먼저 종료합니다(같은 포트·같은 시작 등록 이름을 씀).

### 1. 관리자 권한으로 실행 (UAC)

1. 설정 → 시작 → "관리자 권한으로 실행"을 켜고 적용합니다.
2. 앱을 종료한 뒤 다시 실행합니다.
   - 기대 결과: UAC 창이 뜨고, 승인하면 트레이 아이콘이 하나만 생깁니다(관리자 복사본).
3. UAC 창에서 "아니요"를 누르면: 앱이 일반 권한으로 계속 실행되고 로그에 "Not running as administrator" 경고가 남습니다.
- 되돌리기: 같은 스위치를 끄고 적용 → 재시작.

### 2. TUN (모든 앱의 트래픽)

1. 새 받는 쪽 블록을 TUN으로 만들고 Direct(또는 테스트할 내보내는 쪽)에 연결해 실행합니다.
   - 기대 결과: 관리자 권한이 아니면 UAC 창이 뜨고(TUN 도우미), 승인하면 카드가 "실행 중"이 됩니다.
2. 브라우저로 아무 사이트나 열어 봅니다.
   - 기대 결과: 열리고, 로그에 TUN 카드의 연결 줄이 생깁니다.
3. 카드를 중지합니다.
   - 기대 결과: 인터넷이 원래대로 되고, 네트워크 어댑터 목록에서 TUN 어댑터가 사라집니다.
- 되돌리기: 카드 중지 또는 앱 종료. 남아 있으면 PC를 다시 시작합니다.

### 3. Windows 방화벽

1. 설정 → Windows 방화벽 → "외부 접속 차단"을 켭니다.
   - 기대 결과: UAC 창 → 승인하면 규칙 "ProxyDock - Block External"이 생깁니다.
2. 확인 명령:
   ```powershell
   netsh advfirewall firewall show rule name="ProxyDock - Block External"
   ```
   - 기대 결과: ProxyDock.exe, sing-box.exe, hysteria.exe 각각에 대해 들어오는 연결 차단 규칙이 있습니다.
3. 다른 PC에서 0.0.0.0으로 연 카드에 접속해 봅니다 → 막혀야 합니다. 같은 PC(127.0.0.1)는 됩니다.
- 되돌리기: 같은 스위치를 끄면 규칙이 지워집니다.

### 4. 시작 시 실행

1. 설정 → 시작 → "시작프로그램 등록"을 켭니다(필요하면 "시작 알림 숨기기"도).
2. 확인 명령:
   ```powershell
   Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' | Select-Object ProxyDock
   ```
   - 기대 결과: `"…\ProxyDock.exe"`, 숨기기를 켰다면 뒤에 ` --hidden`.
   - **주의:** Python ProxyDock과 같은 이름("ProxyDock")이라, 켜면 기존 Python 앱의 등록을 덮어씁니다.
3. 로그아웃 후 다시 로그인해 트레이에 뜨는지 봅니다.
- 되돌리기: 같은 스위치를 끄면 값이 지워집니다. Python 앱을 다시 쓰려면 Python 앱에서 다시 켭니다.

### 5. 트레이 풍선 알림

1. 창을 닫은 상태(트레이만)에서 트레이 메뉴의 IF 스위치(켜짐/꺼짐)를 바꿉니다. 설정이 바뀌는 동작이면 무엇이든 됩니다.
   - 기대 결과: 화면 오른쪽 아래에 "ProxyDock" 알림이 뜹니다(창이 열려 있으면 알림 대신 창 아래쪽에 메시지).
   - 자동 재시작이 꺼진 카드가 시작에 실패할 때도 알림이 뜹니다.
2. 앱을 `--hidden` 없이 시작하면 "실행 중 · 자동 시작 프록시 N개" 알림이 뜹니다.
- Windows 알림 설정에서 앱 알림이 꺼져 있으면 뜨지 않습니다(정상).

### 6. 한글 입력(IME)

1. 블록 이름 바꾸기(F2), 편집기의 이름 칸, 로그 검색 칸, 프로그램 고르기 검색 칸에서 한글을 입력합니다.
   - 기대 결과: 조합 중인 글자가 칸 안에 보이고, 완성된 글자가 빠지거나 두 번 들어가지 않습니다.
2. 한글 입력 중 Enter와 Esc가 기대대로 동작하는지 봅니다(이름 바꾸기 상자).

### 7. 함께 보면 좋은 것

- **시스템 프록시(PC 블록):** HTTP 받는 쪽을 PC 블록에 연결 → 인터넷 옵션(`inetcpl.cpl`) → 연결 → LAN 설정에 주소가 들어갑니다. 목록 페이지의 PC 바에서 "이전으로 되돌리기"를 누르면 원래 값으로 돌아갑니다.
- **클립보드:** 카드의 "클라이언트 주소/링크 복사", WireGuard 기기 "설정 복사", 로그 "복사"가 실제 클립보드에 들어가는지 확인합니다.
- **Windows 10:** 직접 그린 제목 표시줄은 Windows 11에서만 켜집니다. Windows 10에서는 기본 Windows 제목 표시줄이 나오는 것이 정상입니다.
- **앱 업데이트:** 설치본에서만 동작합니다(설치 폴더에 unins000.exe가 있을 때). 새 릴리스가 있으면 설정 → 업데이트에서 받아 설치합니다.

### 8. 회선 직결과 홈 스위치

1. **회선 직결 VM 만들기 (Windows, Hyper-V):** 설정 → 회선 직결에서 VM 이름을 넣어 추가하고 네트워크 카드를 골라 [만들기]를 누릅니다. 동의 창의 체크박스를 켜야 [만들기]가 눌리고, UAC 승인 뒤 20초 안팎에 "준비됨"이 됩니다.
2. **카드로 확인:** SOCKS5 카드의 출구(Direct)에서 네트워크 인터페이스를 "회선 직결 (이름)"으로 고르면 편집기에 ⚠ 경고가 붙습니다. 카드를 켜고 PC 전체 VPN이 켜진 상태에서 `https://am.i.mullvad.net/json`(또는 IP 확인 사이트)을 그 카드로 열면 VPN이 아닌 실제 IP가 나와야 합니다.
3. **VM 켜기 스위치:** 끄면 그 VM을 쓰는 카드가 멈추고, 그 카드를 다시 켜면 "꺼져 있습니다"로 시작하지 않아야 합니다.
4. **홈 스위치:** 카드를 켠 채 홈의 큰 스위치를 끄면 카드는 켜져 있지만 접속이 거부되고(로그 "차단됨 (ProxyDock 꺼짐)"), 켜면 바로 다시 됩니다. 트레이 메뉴의 "ProxyDock 켜짐/꺼짐" 항목도 같습니다. 앱을 다시 시작해도 꺼짐 상태가 유지됩니다.
5. **캔버스 연결선:** 한 입력 핀에 선이 여러 개일 때 원하는 선을 잡고 끌면 그 선만 다른 핀으로 옮겨지고, 빈 곳에 놓으면 끊깁니다.
6. **정리:** 회선 직결에서 [삭제]를 누르면 Hyper-V VM과 ProxyDock이 만든 스위치("ProxyDock (카드 이름)")가 지워집니다.
7. **Linux:** `tools/docker_linux_bypass_check.sh`(컨테이너) 외에 실제 Linux에서 유선·Wi-Fi로 확인합니다. Wi-Fi는 ipvlan이라 DHCP가 안 될 수 있습니다(고정 주소로 확인).
8. **macOS (Apple 실리콘):** 아직 실기 시험 전입니다. 위 1~6을 Mac에서 해 보고, 안 되면 도우미 로그와 함께 이슈나 PR로 알려 주세요.

## 릴리스

위 확인이 끝나면 `native/Cargo.toml`의 버전과 같은 태그를 올립니다. 릴리스 워크플로(`.github/workflows/release.yml`)가 테스트 → 빌드 → 설치기·zip·SHA256SUMS를 GitHub 릴리스에 올립니다.

```powershell
git tag -s v3.0.0 -m "ProxyDock 3.0.0"
git push origin v3.0.0
```

- 태그 이름이 Cargo.toml 버전과 다르면 워크플로가 멈춥니다.
- Actions의 "Run workflow"로 돌리면 릴리스 없이 빌드만 확인합니다.
