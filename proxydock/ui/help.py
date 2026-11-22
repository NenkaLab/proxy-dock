"""사용 방법 페이지."""
from PySide6.QtCore import Qt
from PySide6.QtWidgets import QHBoxLayout, QVBoxLayout, QWidget
from qfluentwidgets import (
    BodyLabel, CaptionLabel, FluentIcon as FIF, IconWidget, PushButton, ScrollArea, SimpleCardWidget, StrongBodyLabel,
    TitleLabel,
)

from ..common import IS_WIN

STEPS = [
    (FIF.TILES, "1. ProxyDock 은 프록시를 담는 '도크'입니다",
     "프록시를 여러 개 만들어 각각 켜고 끌 수 있습니다. 프록시 하나는 두 부분으로 되어 있습니다.\n"
     "• 받는 쪽 - 이 PC 의 포트에서 연결을 받는 방식: SOCKS5 · SOCKS4 · HTTP · HTTPS · Shadowsocks · Hysteria2\n"
     "• 내보내는 쪽 - 목적지까지 보내는 방식: Direct(바로 연결) · 상위 HTTP/HTTPS/SOCKS4/SOCKS5 프록시 · "
     "Shadowsocks 터널 · Hysteria2 터널"),
    (FIF.ADD, "2. 만들고 실행하기",
     "프록시 화면 → '새 프록시' 에서 용도에 맞는 템플릿을 고르고, 값을 확인한 뒤 '만들기'.\n"
     "편집 화면 아래쪽 '연결 확인' 으로 저장하기 전에 내보내는 쪽(상위 프록시·터널 접속, 비밀번호)과 받는 쪽 포트를 시험해 볼 수 있습니다.\n"
     "카드의 스위치로 실행/중지합니다. '앱 시작 시 자동 실행' 을 켜 두면 ProxyDock 을 켤 때 함께 시작합니다."),
    (FIF.GLOBE, "3. 프로그램에서 쓰기",
     "카드의 복사 버튼으로 주소(또는 링크)를 복사해 프로그램의 프록시 설정에 넣습니다.\n"
     "• Firefox: 설정 → 네트워크 설정 → 수동 프록시 (SOCKS5 는 'SOCKS v5 사용 시 DNS 프록시' 체크)\n"
     "• Chrome/Edge: 바로가기 대상 끝에  --proxy-server=\"socks5://127.0.0.1:1080\"  (http:// 도 가능)\n"
     "• curl:  curl -x socks5h://127.0.0.1:1080 https://example.com   (HTTPS 프록시는 -x https://… --proxy-insecure)\n"
     "• Shadowsocks / Hysteria2 앱: 복사한 ss:// · hysteria2:// 링크를 붙여넣기"),
    (FIF.VPN, "4. 터널과 상위 프록시",
     "내보내는 쪽을 바꾸면 연결을 다른 서버로 넘깁니다.\n"
     "• 회사 프록시 경유: 받는 쪽 SOCKS5(로컬) → 내보내는 쪽 HTTP 프록시(회사 주소)\n"
     "• 암호화 터널: 받는 쪽 SOCKS5(로컬) → 내보내는 쪽 Shadowsocks/Hysteria2 터널(원격 서버). "
     "원격 서버는 다른 PC 의 ProxyDock 'Shadowsocks 서버'/'Hysteria2 서버' 프록시여도 됩니다.\n"
     "UDP(QUIC·게임·DNS)는 SOCKS5·Shadowsocks·Hysteria2 로 받을 때, Direct·Shadowsocks·Hysteria2 로 보낼 때 됩니다. "
     "Hysteria2 는 Python 구현이라 약 100 Mbps 수준입니다."),
    (FIF.SETTING, "5. 전역 설정과 프록시 설정",
     "설정 화면의 '프록시 기본값'(허용 도메인, 내부망 차단, 타임아웃, 로그)은 모든 프록시에 적용됩니다.\n"
     "프록시 편집 화면에서 항목의 '전역 따름' 체크를 풀면 그 프록시만의 값이 우선합니다.\n"
     "실행 중에 바꾸면 허용 도메인·내부망 차단·로그는 새 연결부터, 타임아웃은 지금 연결에도 바로 적용됩니다. "
     "받는 쪽/내보내는 쪽을 바꾸면 그 프록시가 다시 시작되어 기존 연결이 끊깁니다.\n"
     "프록시끼리 체인으로 이을 때(A 의 내보내는 쪽 = B 의 받는 쪽) 순환이 생기면 저장되지 않습니다."),
    (FIF.WIFI, "6. 다른 PC 에서 쓰기 / 막기",
     ("받는 쪽 주소를 0.0.0.0 으로 하고 (SOCKS5·HTTP·HTTPS 는) 사용자 인증을 켜세요. "
      "아래 방화벽 차단이 켜져 있으면 끄세요. 이 PC 에서만 쓸 거라면 차단을 켜 두면 안전합니다 (관리자 확인 창이 뜹니다).")
     if IS_WIN else
     ("받는 쪽 주소를 0.0.0.0 으로 하고 (SOCKS5·HTTP·HTTPS 는) 사용자 인증을 켜세요. "
      "OS 방화벽(macOS 방화벽, ufw/firewalld 등)이 켜져 있으면 해당 포트를 허용해야 합니다.")),
    (FIF.MINIMIZE, "7. 창 닫기와 종료",
     "기본 설정에서는 X 를 눌러도 창만 숨겨지고 프록시는 트레이에서 계속 동작합니다.\n"
     "완전히 끄려면 트레이 아이콘 우클릭 → 종료. (설정 → 화면 → 'X 버튼으로 종료' 로 바꿀 수 있음)"),
]


class HelpInterface(ScrollArea):
    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        self.setObjectName("helpInterface")
        self.view = QWidget(self)
        self.setWidget(self.view)
        self.setWidgetResizable(True)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        self.enableTransparentBackground()

        root = QVBoxLayout(self.view)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(12)
        root.addWidget(TitleLabel("사용 방법", self.view))
        for i, (icon, title, body) in enumerate(STEPS):
            card = SimpleCardWidget(self.view)
            lay = QHBoxLayout(card)
            lay.setContentsMargins(20, 16, 20, 16)
            lay.setSpacing(16)
            ic = IconWidget(icon, card)
            ic.setFixedSize(20, 20)
            lay.addWidget(ic, 0, Qt.AlignTop)
            col = QVBoxLayout()
            col.setSpacing(4)
            col.addWidget(StrongBodyLabel(title, card))
            text = BodyLabel(body, card)
            text.setWordWrap(True)
            text.setTextInteractionFlags(Qt.TextSelectableByMouse)
            col.addWidget(text)
            lay.addLayout(col, 1)
            if i == 5 and IS_WIN:
                box = QVBoxLayout()
                box.setSpacing(6)
                self.fw_label = CaptionLabel("방화벽 차단: 확인 중…", card)
                self.btn_fw_on = PushButton("차단 켜기", card, FIF.CERTIFICATE)
                self.btn_fw_off = PushButton("차단 끄기", card, FIF.CLOSE)
                self.btn_fw_on.clicked.connect(lambda: ctl.firewall(True))
                self.btn_fw_off.clicked.connect(lambda: ctl.firewall(False))
                box.addWidget(self.fw_label, 0, Qt.AlignRight)
                row = QHBoxLayout()
                row.addWidget(self.btn_fw_on)
                row.addWidget(self.btn_fw_off)
                box.addLayout(row)
                lay.addLayout(box)
            root.addWidget(card)
        root.addStretch(1)

    def set_firewall_status(self, state):
        if not hasattr(self, "fw_label"):
            return
        self.fw_label.setText("방화벽 차단: " + {True: "켜짐", False: "꺼짐", None: "확인 중…"}[state])
        busy = state is None
        self.btn_fw_on.setEnabled(not busy and state is not True)
        self.btn_fw_off.setEnabled(not busy and state is not False)
