"""프록시 화면: 목록(카드) + 편집기.

프록시 하나 = 받는 쪽(이 PC 의 포트로 연결을 받는 방식) + 내보내는 쪽(목적지까지 보내는 방식) + 로컬 설정.
"""
import copy
import os
import threading

from PySide6.QtCore import Qt, Signal
from PySide6.QtGui import QColor, QPainter
from PySide6.QtWidgets import QApplication, QFileDialog, QHBoxLayout, QLabel, QVBoxLayout, QWidget
from qfluentwidgets import (
    Action, BodyLabel, CaptionLabel, CardWidget, CheckBox, ComboBox, FluentIcon as FIF, LineEdit, MessageBox,
    PasswordLineEdit, PrimaryDropDownPushButton, PrimaryPushButton, PushButton, RoundMenu, ScrollArea,
    SimpleCardWidget, SpinBox, StrongBodyLabel, SubtitleLabel, SwitchButton, TitleLabel, TransparentToolButton,
    isDarkTheme,
)

from .. import config
from ..common import INBOUND_TYPES, OUTBOUND_TYPES, host_port, is_loopback_host
from ..platform_utils import check_port, local_lan_ip
from .common import STATE_COLORS, STATE_TEXT, info


# --- 공통 ---

def client_url(p):
    """클라이언트 앱에 넣을 주소/링크."""
    from ..hysteria2 import hy2_link
    from ..ss import ss_link
    ib = p["inbound"]
    host = ib["host"]
    if host in ("0.0.0.0", "::"):
        host = local_lan_ip()
    t = ib["type"]
    if t == "shadowsocks":
        return ss_link(host, ib["port"], ib["ss_method"], ib["ss_password"], p["name"])
    if t == "hysteria2":
        return hy2_link(host, ib["port"], ib["hy2_password"], insecure=not ib["tls_cert"], name=p["name"])
    scheme = {"socks5": "socks5", "socks4": "socks4a", "http": "http", "https": "https"}[t]
    cred = ""
    if t in ("socks5", "http", "https") and ib["auth_enabled"]:
        cred = f"{ib['username']}:{ib['password']}@"
    return f"{scheme}://{cred}{host_port(host, ib['port'])}"


def summary(p):
    ib, ob = p["inbound"], p["outbound"]
    left = f"{INBOUND_TYPES[ib['type']]} {host_port(ib['host'], ib['port'])}"
    right = "Direct" if ob["type"] == "direct" else f"{OUTBOUND_TYPES[ob['type']]} {host_port(ob['host'], ob['port'])}"
    return f"{left}  →  {right}"


class Dot(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.color = QColor("#8A8A8A")
        self.setFixedSize(12, 12)

    def set_color(self, c):
        self.color = QColor(c)
        self.update()

    def paintEvent(self, _):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        p.setPen(Qt.NoPen)
        p.setBrush(self.color)
        p.drawEllipse(1, 1, 10, 10)
        p.end()


# --- 목록 ---

TEMPLATES = [
    ("로컬 SOCKS5", "이 PC 에서 쓰는 SOCKS5 프록시", {"type": "socks5", "port": 1080}, {"type": "direct"}),
    ("로컬 HTTP", "브라우저·앱의 HTTP 프록시 설정용", {"type": "http", "port": 8080}, {"type": "direct"}),
    ("HTTPS 프록시", "TLS 로 감싼 HTTP 프록시 (자체 서명 인증서 자동 생성)", {"type": "https", "port": 8443},
     {"type": "direct"}),
    ("SOCKS4", "오래된 프로그램용 (인증 없음)", {"type": "socks4", "port": 1084}, {"type": "direct"}),
    ("Shadowsocks 서버", "다른 기기의 Shadowsocks 앱이 접속", {"type": "shadowsocks", "host": "0.0.0.0", "port": 8388},
     {"type": "direct"}),
    ("Hysteria2 서버", "다른 기기의 Hysteria2 앱이 접속 (QUIC)", {"type": "hysteria2", "host": "0.0.0.0", "port": 8443},
     {"type": "direct"}),
    ("로컬 → 상위 프록시", "로컬 SOCKS5 로 받아 회사/다른 HTTP·SOCKS 프록시로 보냄", {"type": "socks5", "port": 1080},
     {"type": "http"}),
    ("로컬 → Shadowsocks 터널", "로컬 SOCKS5 로 받아 원격 Shadowsocks 서버로 암호화", {"type": "socks5", "port": 1080},
     {"type": "shadowsocks"}),
    ("로컬 → Hysteria2 터널", "로컬 SOCKS5 로 받아 원격 Hysteria2 서버로 (QUIC)", {"type": "socks5", "port": 1080},
     {"type": "hysteria2"}),
]


class ProxyCard(CardWidget):
    def __init__(self, page, proxy, parent=None):
        super().__init__(parent)
        self.page = page
        self.pid = proxy["id"]
        self.dot = Dot(self)
        self.name = StrongBodyLabel("", self)
        self.desc = CaptionLabel("", self)
        self.state = CaptionLabel("", self)
        self.switch = SwitchButton(self)
        self.switch.setOnText("실행")
        self.switch.setOffText("중지")
        self.switch.checkedChanged.connect(lambda on: page.ctl.toggle_proxy(self.pid, on))
        self.btn_copy = TransparentToolButton(FIF.COPY, self)
        self.btn_copy.setToolTip("클라이언트용 주소/링크 복사")
        self.btn_copy.clicked.connect(lambda: page.copy_url(self.pid))
        self.btn_edit = TransparentToolButton(FIF.EDIT, self)
        self.btn_edit.setToolTip("편집")
        self.btn_edit.clicked.connect(lambda: page.ctl.edit_proxy(self.pid))
        self.btn_more = TransparentToolButton(FIF.MORE, self)
        self.btn_more.clicked.connect(self._menu)
        self.clicked.connect(lambda: page.ctl.edit_proxy(self.pid))

        text = QVBoxLayout()
        text.setSpacing(2)
        top = QHBoxLayout()
        top.setSpacing(8)
        top.addWidget(self.dot, 0, Qt.AlignVCenter)
        top.addWidget(self.name)
        top.addWidget(self.state)
        top.addStretch(1)
        text.addLayout(top)
        text.addWidget(self.desc)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(18, 12, 12, 12)
        lay.addLayout(text, 1)
        for w in (self.switch, self.btn_copy, self.btn_edit, self.btn_more):
            lay.addWidget(w, 0, Qt.AlignVCenter)
        self.update_from(proxy, ("stopped", ""))

    def update_from(self, proxy, status):
        state, msg = status
        self.name.setText(proxy["name"] + ("   · 자동 시작" if proxy["autostart"] else ""))
        self.desc.setText(summary(proxy))
        full = STATE_TEXT.get(state, state) + (f" - {msg}" if msg else "")
        fm = self.state.fontMetrics()
        self.state.setText(fm.elidedText(full, Qt.ElideRight, 520))
        self.state.setToolTip(full if len(full) > 30 else "")
        self.dot.set_color(STATE_COLORS.get(state, "#8A8A8A"))
        self.switch.blockSignals(True)
        self.switch.setChecked(state in ("running", "starting"))
        self.switch.blockSignals(False)

    def _menu(self):
        m = RoundMenu(parent=self)
        a1 = Action(FIF.COPY, "복제", m)
        a1.triggered.connect(lambda: self.page.ctl.duplicate_proxy(self.pid))
        a2 = Action(FIF.DELETE, "삭제", m)
        a2.triggered.connect(lambda: self.page.confirm_delete(self.pid))
        m.addActions([a1, a2])
        m.exec(self.btn_more.mapToGlobal(self.btn_more.rect().bottomLeft()))


class ProxiesInterface(ScrollArea):
    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        self.setObjectName("proxiesInterface")
        self.ctl = ctl
        self.cards = {}
        self.view = QWidget(self)
        self.setWidget(self.view)
        self.setWidgetResizable(True)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        self.enableTransparentBackground()

        title = TitleLabel("프록시", self.view)
        self.btn_new = PrimaryDropDownPushButton(FIF.ADD, "새 프록시", self.view)
        menu = RoundMenu(parent=self.btn_new)
        for name, desc, ib, ob in TEMPLATES:
            act = Action(f"{name}  —  {desc}", menu)
            act.triggered.connect(lambda _=False, n=name, i=ib, o=ob: ctl.new_proxy(n, i, o))
            menu.addAction(act)
        self.btn_new.setMenu(menu)
        self.btn_stop = PushButton("모두 중지", self.view, FIF.PAUSE)
        self.btn_stop.clicked.connect(ctl.stop_all)
        self.empty = BodyLabel("프록시가 없습니다. '새 프록시' 로 만들어 보세요.", self.view)

        header = QHBoxLayout()
        header.addWidget(title)
        header.addStretch(1)
        header.addWidget(self.btn_stop)
        header.addWidget(self.btn_new)
        self.hint = CaptionLabel("프록시 = 받는 쪽(이 PC 의 포트로 연결을 받는 방식) → 내보내는 쪽(목적지까지 보내는 방식). "
                                 "카드를 누르면 편집, 스위치로 실행/중지.", self.view)
        self.hint.setWordWrap(True)
        self.list = QVBoxLayout()
        self.list.setSpacing(8)
        root = QVBoxLayout(self.view)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(12)
        root.addLayout(header)
        root.addWidget(self.hint)
        root.addWidget(self.empty)
        root.addLayout(self.list)
        root.addStretch(1)

    def refresh(self, proxies, statuses):
        ids = [p["id"] for p in proxies]
        for pid in list(self.cards):
            if pid not in ids:
                self.cards.pop(pid).deleteLater()
        for i, p in enumerate(proxies):
            card = self.cards.get(p["id"])
            if card is None:
                card = self.cards[p["id"]] = ProxyCard(self, p, self.view)
            self.list.removeWidget(card)
            self.list.insertWidget(i, card)
            card.update_from(p, statuses.get(p["id"], ("stopped", "")))
        self.empty.setVisible(not proxies)

    def set_status(self, pid, status, proxy):
        card = self.cards.get(pid)
        if card is not None and proxy is not None:
            card.update_from(proxy, status)

    def copy_url(self, pid):
        p = self.ctl.proxy_by_id(pid)
        if p is None:
            return
        url = client_url(p)
        QApplication.clipboard().setText(url)
        shown = url if len(url) < 70 else url[:67] + "…"
        info(self.window(), "success", "복사됨", shown, 2500)

    def confirm_delete(self, pid):
        p = self.ctl.proxy_by_id(pid)
        if p is None:
            return
        box = MessageBox("프록시 삭제", f"'{p['name']}' 을(를) 삭제할까요? 실행 중이면 중지됩니다.", self.window())
        box.yesButton.setText("삭제")
        box.cancelButton.setText("취소")
        if box.exec():
            self.ctl.delete_proxy(pid)


# --- 편집기 ---

class Section(QWidget):
    """제목 + 카드 목록 (숨긴 카드는 자리를 차지하지 않는다)."""

    def __init__(self, title, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(6)
        lay.addWidget(SubtitleLabel(title, self))
        self.lay = lay

    def add(self, w):
        self.lay.addWidget(w)
        return w


class Row(SimpleCardWidget):
    """설정 한 줄: 제목/설명 + 오른쪽 위젯들."""

    def __init__(self, title, desc="", parent=None):
        super().__init__(parent)
        self.title = BodyLabel(title, self)
        self.desc = CaptionLabel(desc, self)
        self.desc.setWordWrap(True)
        self.desc.setVisible(bool(desc))
        col = QVBoxLayout()
        col.setSpacing(1)
        col.addWidget(self.title)
        col.addWidget(self.desc)
        self.hl = QHBoxLayout(self)
        self.hl.setContentsMargins(16, 10, 16, 10)
        self.hl.setSpacing(10)
        self.hl.addLayout(col, 1)

    def right(self, *widgets):
        for w in widgets:
            self.hl.addWidget(w, 0, Qt.AlignVCenter)
        return self


def _switch(parent, on="켜짐", off="꺼짐"):
    sw = SwitchButton(parent)
    sw.setOnText(on)
    sw.setOffText(off)
    return sw


class EditorInterface(ScrollArea):
    """프록시 편집 화면 (탐색 메뉴에는 없음, 목록에서 열린다)."""

    closed = Signal()
    check_done = Signal(object, object)     # (내보내는 쪽 결과, 받는 쪽 결과)

    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        from ..ss import SS_METHODS
        self.setObjectName("editorInterface")
        self.ctl = ctl
        self.proxy = None
        self.is_new = False
        self.ss_methods = list(SS_METHODS)
        self.view = QWidget(self)
        self.setWidget(self.view)
        self.setWidgetResizable(True)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        self.enableTransparentBackground()
        v = self.view

        self.title = TitleLabel("프록시 편집", v)
        self.btn_cancel = PushButton("취소", v)
        self.btn_cancel.clicked.connect(self.cancel)
        self.btn_save = PrimaryPushButton("저장", v, FIF.SAVE)
        self.btn_save.clicked.connect(self.save)

        # 기본
        s_basic = Section("기본", v)
        self.ed_name = LineEdit(v)
        self.ed_name.setFixedWidth(260)
        s_basic.add(Row("이름", parent=v).right(self.ed_name))
        self.sw_auto = _switch(v)
        s_basic.add(Row("앱 시작 시 자동 실행", "ProxyDock 을 켜면 이 프록시도 바로 시작합니다", v).right(self.sw_auto))

        # 받는 쪽
        s_in = Section("받는 쪽 (이 PC 에서 연결을 받는 방식)", v)
        self.cb_in = ComboBox(v)
        self.in_types = list(INBOUND_TYPES)
        self.cb_in.addItems([INBOUND_TYPES[t] for t in self.in_types])
        self.cb_in.setFixedWidth(200)
        self.r_in_type = s_in.add(Row("프로토콜", "", v).right(self.cb_in))
        self.ed_in_host = LineEdit(v)
        self.ed_in_host.setFixedWidth(170)
        self.sp_in_port = SpinBox(v)
        self.sp_in_port.setRange(1, 65535)
        self.sp_in_port.setFixedWidth(150)
        btn_check = PushButton("사용 가능 확인", v, FIF.SEARCH)
        btn_check.clicked.connect(self._check_port)
        s_in.add(Row("수신 주소 · 포트", "127.0.0.1 = 이 PC 만, 0.0.0.0 = 다른 PC 허용 (인증·방화벽 확인)", v)
                 .right(self.ed_in_host, self.sp_in_port, btn_check))
        self.sw_in_auth = _switch(v)
        self.r_in_auth = s_in.add(Row("사용자 인증", "", v).right(self.sw_in_auth))
        self.ed_in_user = LineEdit(v)
        self.ed_in_user.setPlaceholderText("아이디")
        self.ed_in_user.setFixedWidth(150)
        self.ed_in_pass = PasswordLineEdit(v)
        self.ed_in_pass.setPlaceholderText("비밀번호")
        self.ed_in_pass.setFixedWidth(180)
        self.r_in_cred = s_in.add(Row("계정", "", v).right(self.ed_in_user, self.ed_in_pass))
        self.ed_cert = LineEdit(v)
        self.ed_cert.setPlaceholderText("비우면 자동 생성")
        self.ed_cert.setFixedWidth(220)
        b1 = PushButton("찾기", v, FIF.FOLDER)
        b1.clicked.connect(lambda: self._pick(self.ed_cert, "인증서 (*.crt *.pem *.cer);;모든 파일 (*)"))
        self.r_cert = s_in.add(Row("TLS 인증서", "PEM 형식. 비우면 자체 서명 인증서를 만들어 씁니다 (클라이언트에서 '인증서 확인 안 함' 필요)", v)
                               .right(self.ed_cert, b1))
        self.ed_key = LineEdit(v)
        self.ed_key.setPlaceholderText("비우면 자동 생성")
        self.ed_key.setFixedWidth(220)
        b2 = PushButton("찾기", v, FIF.FOLDER)
        b2.clicked.connect(lambda: self._pick(self.ed_key, "개인 키 (*.key *.pem);;모든 파일 (*)"))
        self.r_key = s_in.add(Row("TLS 개인 키", "", v).right(self.ed_key, b2))
        self.cb_in_ss = ComboBox(v)
        self.cb_in_ss.addItems(self._ss_labels())
        self.cb_in_ss.setFixedWidth(330)
        self.r_in_ss_method = s_in.add(Row("암호화 방식", "클라이언트와 같은 방식을 골라야 합니다", v).right(self.cb_in_ss))
        self.ed_in_ss_pw = PasswordLineEdit(v)
        self.ed_in_ss_pw.setFixedWidth(260)
        g1 = PushButton("생성", v, FIF.SYNC)
        g1.clicked.connect(lambda: self._gen_ss(self.cb_in_ss, self.ed_in_ss_pw))
        self.r_in_ss_pw = s_in.add(Row("Shadowsocks 비밀번호", "2022 방식은 base64 키 - 방식을 바꿨다면 '생성'", v)
                                   .right(self.ed_in_ss_pw, g1))
        self.ed_in_hy = PasswordLineEdit(v)
        self.ed_in_hy.setFixedWidth(260)
        g2 = PushButton("생성", v, FIF.SYNC)
        g2.clicked.connect(lambda: self.ed_in_hy.setText(_token()))
        self.r_in_hy = s_in.add(Row("Hysteria2 비밀번호", "8자 이상", v).right(self.ed_in_hy, g2))
        self.in_note = CaptionLabel("", v)
        self.in_note.setWordWrap(True)
        s_in.add(self.in_note)

        # 내보내는 쪽
        s_out = Section("내보내는 쪽 (목적지까지 보내는 방식)", v)
        self.cb_out = ComboBox(v)
        self.out_types = list(OUTBOUND_TYPES)
        self.cb_out.addItems([OUTBOUND_TYPES[t] for t in self.out_types])
        self.cb_out.setFixedWidth(200)
        s_out.add(Row("방식", "Direct = 이 PC 에서 바로 연결. 나머지는 다른 프록시/서버를 거칩니다", v).right(self.cb_out))
        self.ed_out_host = LineEdit(v)
        self.ed_out_host.setPlaceholderText("서버 주소")
        self.ed_out_host.setFixedWidth(220)
        self.sp_out_port = SpinBox(v)
        self.sp_out_port.setRange(0, 65535)
        self.sp_out_port.setFixedWidth(150)
        self.r_out_server = s_out.add(Row("서버 주소 · 포트", "", v).right(self.ed_out_host, self.sp_out_port))
        self.ed_out_user = LineEdit(v)
        self.ed_out_user.setPlaceholderText("아이디 (없으면 비움)")
        self.ed_out_user.setFixedWidth(170)
        self.ed_out_pass = PasswordLineEdit(v)
        self.ed_out_pass.setPlaceholderText("비밀번호")
        self.ed_out_pass.setFixedWidth(180)
        self.r_out_cred = s_out.add(Row("상위 프록시 계정", "", v).right(self.ed_out_user, self.ed_out_pass))
        self.sw_out_verify = _switch(v, "확인", "안 함")
        self.r_out_verify = s_out.add(Row("서버 인증서 확인", "자체 서명 인증서 서버라면 끄세요 (보안상 지문 고정 권장)", v)
                                      .right(self.sw_out_verify))
        self.ed_out_sni = LineEdit(v)
        self.ed_out_sni.setPlaceholderText("비우면 서버 주소")
        self.ed_out_sni.setFixedWidth(220)
        self.r_out_sni = s_out.add(Row("SNI (서버 이름)", "", v).right(self.ed_out_sni))
        self.ed_out_pin = LineEdit(v)
        self.ed_out_pin.setPlaceholderText("SHA-256 (선택)")
        self.ed_out_pin.setFixedWidth(300)
        self.r_out_pin = s_out.add(Row("인증서 지문 고정", "입력하면 이 지문의 인증서만 허용합니다", v).right(self.ed_out_pin))
        self.cb_out_ss = ComboBox(v)
        self.cb_out_ss.addItems(self._ss_labels())
        self.cb_out_ss.setFixedWidth(330)
        self.r_out_ss_method = s_out.add(Row("암호화 방식", "서버와 같은 방식", v).right(self.cb_out_ss))
        self.ed_out_ss_pw = PasswordLineEdit(v)
        self.ed_out_ss_pw.setFixedWidth(260)
        self.r_out_ss_pw = s_out.add(Row("Shadowsocks 비밀번호", "", v).right(self.ed_out_ss_pw))
        self.ed_out_hy = PasswordLineEdit(v)
        self.ed_out_hy.setFixedWidth(260)
        self.r_out_hy = s_out.add(Row("Hysteria2 비밀번호", "", v).right(self.ed_out_hy))
        self.out_note = CaptionLabel("", v)
        self.out_note.setWordWrap(True)
        s_out.add(self.out_note)
        self.btn_probe = PushButton("연결 확인", v, FIF.SPEED_HIGH)
        self.btn_probe.clicked.connect(self._probe)
        self.r_probe = s_out.add(Row("연결 확인", CHECK_DESC, v).right(self.btn_probe))
        self.check_done.connect(self._on_probe_done)
        self.probing = False
        self._probe_token = 0

        # 로컬 설정 (전역보다 우선)
        s_local = Section("로컬 설정 (체크를 풀면 이 프록시만의 값을 씁니다)", v)
        self.ov = {}
        self.ed_allow = LineEdit(v)
        self.ed_allow.setFixedWidth(280)
        self.sw_private = _switch(v)
        self.sp_timeout = SpinBox(v)
        self.sp_timeout.setRange(10, 86400)
        self.sp_timeout.setFixedWidth(150)
        self.sw_logconn = _switch(v)
        self.sw_logclose = _switch(v)
        for key, title, desc, w in (
            ("allowed_domains", "허용 도메인", "쉼표로 구분 · * = 전체 · *.example.com = 하위 도메인 포함", self.ed_allow),
            ("block_private", "내부망 목적지 차단", "루프백·사설 IP 로의 연결 거부", self.sw_private),
            ("timeout", "유휴 타임아웃(초)", "", self.sp_timeout),
            ("log_connections", "연결 로그 기록", "", self.sw_logconn),
            ("log_close", "연결 종료 상세 기록", "", self.sw_logclose),
        ):
            follow = CheckBox("전역 따름", v)
            follow.stateChanged.connect(lambda *_, k=key: self._sync_override(k))
            self.ov[key] = (follow, w)
            s_local.add(Row(title, desc, v).right(follow, w))

        header = QHBoxLayout()
        header.addWidget(self.title)
        header.addStretch(1)
        header.addWidget(self.btn_cancel)
        header.addWidget(self.btn_save)
        root = QVBoxLayout(v)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(18)
        root.addLayout(header)
        for s in (s_basic, s_in, s_out, s_local):
            root.addWidget(s)
        root.addStretch(1)

        self.cb_in.currentIndexChanged.connect(lambda *_: self._sync())
        self.cb_out.currentIndexChanged.connect(lambda *_: self._sync())
        self.sw_in_auth.checkedChanged.connect(lambda *_: self._sync())

    def _ss_labels(self):
        from ..ss import SS_METHODS
        return [m + ("  (권장)" if SS_METHODS[m][2] else "  (구버전 호환)") for m in self.ss_methods]

    def _gen_ss(self, combo, edit):
        from ..ss import ss_generate_password
        edit.setText(ss_generate_password(self.ss_methods[max(combo.currentIndex(), 0)]))

    def _pick(self, edit, filt):
        path, _ = QFileDialog.getOpenFileName(self, "파일 선택", os.path.dirname(edit.text()) or "", filt)
        if path:
            edit.setText(path)

    # --- 표시 ---
    def load(self, proxy, is_new):
        self.proxy = copy.deepcopy(proxy)
        self.is_new = is_new
        p = self.proxy
        ib, ob, ov = p["inbound"], p["outbound"], p["overrides"]
        self.title.setText("새 프록시" if is_new else "프록시 편집")
        self.btn_save.setText("만들기" if is_new else "저장")
        self.ed_name.setText(p["name"])
        self.sw_auto.setChecked(p["autostart"])
        self.cb_in.setCurrentIndex(self.in_types.index(ib["type"]))
        self.ed_in_host.setText(ib["host"])
        self.sp_in_port.setValue(ib["port"])
        self.sw_in_auth.setChecked(ib["auth_enabled"])
        self.ed_in_user.setText(ib["username"])
        self.ed_in_pass.setText(ib["password"])
        self.ed_cert.setText(ib["tls_cert"])
        self.ed_key.setText(ib["tls_key"])
        self.cb_in_ss.setCurrentIndex(self.ss_methods.index(ib["ss_method"]))
        self.ed_in_ss_pw.setText(ib["ss_password"])
        self.ed_in_hy.setText(ib["hy2_password"])
        self.cb_out.setCurrentIndex(self.out_types.index(ob["type"]))
        self.ed_out_host.setText(ob["host"])
        self.sp_out_port.setValue(ob["port"])
        self.ed_out_user.setText(ob["username"])
        self.ed_out_pass.setText(ob["password"])
        self.sw_out_verify.setChecked(ob["tls_verify"])
        self.ed_out_sni.setText(ob["sni"])
        self.ed_out_pin.setText(ob["pin_sha256"])
        self.cb_out_ss.setCurrentIndex(self.ss_methods.index(ob["ss_method"]))
        self.ed_out_ss_pw.setText(ob["ss_password"])
        self.ed_out_hy.setText(ob["hy2_password"])
        glob = self.ctl.glob
        for key, (follow, w) in self.ov.items():
            follow.blockSignals(True)
            follow.setChecked(ov.get(key) is None)
            follow.blockSignals(False)
            self._set_value(key, ov[key] if ov.get(key) is not None else glob[key])
            w.setEnabled(ov.get(key) is not None)
        if is_new and ib["type"] == "shadowsocks" and not ib["ss_password"]:
            self._gen_ss(self.cb_in_ss, self.ed_in_ss_pw)
        if is_new and ib["type"] == "hysteria2" and not ib["hy2_password"]:
            self.ed_in_hy.setText(_token())
        self._probe_token += 1      # 다른 프록시를 열면 진행 중이던 확인 결과는 버린다
        self._set_probe_text(CHECK_DESC, None)
        self._sync()
        self.verticalScrollBar().setValue(0)

    def _set_value(self, key, val):
        w = self.ov[key][1]
        if key == "allowed_domains":
            w.setText(", ".join(val))
        elif key == "timeout":
            w.setValue(val)
        else:
            w.setChecked(val)

    def _get_value(self, key):
        w = self.ov[key][1]
        if key == "allowed_domains":
            return [d.strip() for d in w.text().replace(";", ",").split(",") if d.strip()] or ["*"]
        if key == "timeout":
            return w.value()
        return w.isChecked()

    def _sync_override(self, key):
        follow, w = self.ov[key]
        if follow.isChecked():
            self._set_value(key, self.ctl.glob[key])
        w.setEnabled(not follow.isChecked())

    def _sync(self):
        it = self.in_types[max(self.cb_in.currentIndex(), 0)]
        ot = self.out_types[max(self.cb_out.currentIndex(), 0)]
        can_auth = it in ("socks5", "http", "https")
        self.r_in_auth.setVisible(can_auth)
        self.r_in_cred.setVisible(can_auth and self.sw_in_auth.isChecked())
        self.r_cert.setVisible(it in ("https", "hysteria2"))
        self.r_key.setVisible(it in ("https", "hysteria2"))
        self.r_in_ss_method.setVisible(it == "shadowsocks")
        self.r_in_ss_pw.setVisible(it == "shadowsocks")
        self.r_in_hy.setVisible(it == "hysteria2")
        notes = {
            "socks5": "가장 널리 쓰이는 프록시. TCP 와 UDP(QUIC·DNS 등)를 모두 중계합니다.",
            "socks4": "SOCKS4/4a. TCP 만 되고 비밀번호 인증이 없습니다 (아이디는 무시). 이 PC 에서만 쓰세요.",
            "http": "브라우저·OS 의 'HTTP 프록시' 설정용. HTTPS 사이트는 CONNECT 로 터널링합니다. TCP 만.",
            "https": "HTTP 프록시를 TLS 로 감쌉니다. 클라이언트에서 'HTTPS 프록시'(예: curl -x https://…)로 지정.",
            "shadowsocks": "다른 기기의 Shadowsocks 앱이 접속하는 암호화 터널 서버. TCP+UDP. 카드의 복사 버튼으로 ss:// 링크.",
            "hysteria2": "QUIC(UDP) 기반 암호화 터널 서버. TCP+UDP. 카드의 복사 버튼으로 hysteria2:// 링크. "
                         "Python 구현이라 속도는 약 100 Mbps 수준입니다.",
        }
        self.in_note.setText(notes[it])
        remote = ot != "direct"
        self.r_out_server.setVisible(remote)
        self.r_out_cred.setVisible(ot in ("http", "https", "socks4", "socks5"))
        self.ed_out_pass.setVisible(ot != "socks4")
        self.r_out_verify.setVisible(ot in ("https", "hysteria2"))
        self.r_out_sni.setVisible(ot in ("https", "hysteria2"))
        self.r_out_pin.setVisible(ot == "hysteria2")
        self.r_out_ss_method.setVisible(ot == "shadowsocks")
        self.r_out_ss_pw.setVisible(ot == "shadowsocks")
        self.r_out_hy.setVisible(ot == "hysteria2")
        out_notes = {
            "direct": "이 PC 에서 목적지로 바로 연결합니다.",
            "http": "상위 HTTP 프록시에 CONNECT 로 연결합니다. UDP 는 보낼 수 없습니다.",
            "https": "상위 HTTPS 프록시(TLS)에 CONNECT 로 연결합니다. UDP 는 보낼 수 없습니다.",
            "socks4": "상위 SOCKS4a 프록시로. UDP·IPv6 목적지는 안 됩니다.",
            "socks5": "상위 SOCKS5 프록시로 (TCP). UDP 는 보낼 수 없습니다.",
            "shadowsocks": "원격 Shadowsocks 서버로 암호화 터널 (TCP+UDP).",
            "hysteria2": "원격 Hysteria2 서버로 QUIC 터널 (TCP+UDP). 대역폭 지정(Brutal)·난독화·포트 호핑은 미지원.",
        }
        note = out_notes[ot]
        if remote:
            note += " 상위를 거칠 때 '내부망 차단'은 IP 직접 지정과 localhost 에만 적용됩니다."
        self.out_note.setText(note)

    # --- 연결 확인 ---
    def _probe(self):
        if self.probing:
            return
        p = self.collect()
        errs = config.check_proxy(p, [x for x in self.ctl.proxies if x["id"] != p["id"]])
        if errs:
            info(self.window(), "error", "먼저 확인해 주세요", "\n".join(errs[:4]), 6000)
            return
        self.probing = True
        self.btn_probe.setEnabled(False)
        self.btn_probe.setText("확인 중…")
        self._set_probe_text("내보내는 쪽을 거쳐 접속해 보는 중…", None)
        running = (not self.is_new and self.ctl.manager.state(p["id"])[0] == "running"
                   and self.proxy["inbound"]["host"] == p["inbound"]["host"]
                   and self.proxy["inbound"]["port"] == p["inbound"]["port"])
        mgr, glob = self.ctl.manager, self.ctl.glob
        token = self._probe_token

        def work():
            from ..probe import check_outbound
            out = check_outbound(mgr, p, glob)
            inb = _check_inbound(p, running)
            out["token"] = token
            self.check_done.emit(out, inb)
        threading.Thread(target=work, daemon=True).start()

    def _set_probe_text(self, text, ok):
        d = self.r_probe.desc
        d.setText(text)
        d.setVisible(True)
        if ok is None:
            d.setTextColor()
        else:
            d.setTextColor(QColor("#1E7F3C" if ok else "#C42B1C"), QColor("#6CCB5F" if ok else "#FF99A4"))

    def _on_probe_done(self, out, inb):
        self.probing = False
        self.btn_probe.setEnabled(True)
        self.btn_probe.setText("연결 확인")
        if out.get("token") != self._probe_token or not self.isVisible():
            return
        ot =self.out_types[max(self.cb_out.currentIndex(), 0)]
        ms = lambda v: f"{v:.0f} ms" if v is not None else "-"
        if out.get("ok"):
            line = (f"내보내는 쪽 정상 · {out['site']} 응답 {out.get('status') or '-'} · "
                    f"목적지 연결 {ms(out.get('connect_ms'))} · 전체 {ms(out.get('total_ms'))}")
            if out.get("note"):
                line += f" ({out['note']})"
        else:
            line = f"내보내는 쪽 실패 · {out.get('error') or '알 수 없는 오류'}"
            if out.get("detail"):
                line += f"\n  ↳ {out['detail']}"
            hint = _probe_hint(ot, out)
            if hint:
                line += f"\n  ↳ {hint}"
        in_ok, in_msg = inb
        line += f"\n받는 쪽 {'정상' if in_ok else '문제'} · {in_msg}"
        ok = bool(out.get("ok")) and in_ok
        self._set_probe_text(line, ok)
        self.ensureWidgetVisible(self.r_probe)
        info(self.window(), "success" if ok else "error", "연결 확인 " + ("성공" if ok else "실패"),
             line.split("\n", 1)[0], 4000)

    def _check_port(self):
        host = self.ed_in_host.text().strip() or "127.0.0.1"
        port = self.sp_in_port.value()
        it = self.in_types[max(self.cb_in.currentIndex(), 0)]
        if self.proxy and not self.is_new and self.ctl.manager.state(self.proxy["id"])[0] == "running" \
                and self.proxy["inbound"]["port"] == port and self.proxy["inbound"]["host"] == host:
            info(self.window(), "success", f"포트 {port}", "이 프록시가 이 포트로 실행 중입니다.", 3000)
            return
        ok, msg = check_port(host, port, udp=it == "hysteria2")
        info(self.window(), "success" if ok else "error", f"포트 {port} " + ("사용 가능" if ok else "사용 불가"), msg, 4000)

    # --- 저장 ---
    def collect(self):
        p = copy.deepcopy(self.proxy)
        p["name"] = self.ed_name.text().strip()
        p["autostart"] = self.sw_auto.isChecked()
        ib, ob = p["inbound"], p["outbound"]
        ib.update(type=self.in_types[self.cb_in.currentIndex()], host=self.ed_in_host.text().strip(),
                  port=self.sp_in_port.value(), auth_enabled=self.sw_in_auth.isChecked(),
                  username=self.ed_in_user.text(), password=self.ed_in_pass.text(),
                  tls_cert=self.ed_cert.text().strip(), tls_key=self.ed_key.text().strip(),
                  ss_method=self.ss_methods[self.cb_in_ss.currentIndex()], ss_password=self.ed_in_ss_pw.text().strip(),
                  hy2_password=self.ed_in_hy.text())
        if ib["type"] not in ("socks5", "http", "https"):
            ib["auth_enabled"] = False
        ob.update(type=self.out_types[self.cb_out.currentIndex()], host=self.ed_out_host.text().strip(),
                  port=self.sp_out_port.value(), username=self.ed_out_user.text(),
                  password="" if self.out_types[self.cb_out.currentIndex()] == "socks4" else self.ed_out_pass.text(),
                  tls_verify=self.sw_out_verify.isChecked(), sni=self.ed_out_sni.text().strip(),
                  pin_sha256=self.ed_out_pin.text().strip(),
                  ss_method=self.ss_methods[self.cb_out_ss.currentIndex()], ss_password=self.ed_out_ss_pw.text().strip(),
                  hy2_password=self.ed_out_hy.text())
        for key, (follow, _w) in self.ov.items():
            p["overrides"][key] = None if follow.isChecked() else self._get_value(key)
        return config.validate_proxy(p)

    def save(self):
        p = self.collect()
        others = [x for x in self.ctl.proxies if x["id"] != p["id"]]
        errs = config.check_proxy(p, others)
        if errs:
            info(self.window(), "error", "확인해 주세요", "\n".join(errs[:4]), 6000)
            return
        ib = p["inbound"]
        if not is_loopback_host(ib["host"]) and ib["type"] in ("socks5", "http", "https") and not ib["auth_enabled"]:
            info(self.window(), "warning", "인증 없이 외부에 공개됩니다",
                 "같은 네트워크의 누구나 이 프록시를 쓸 수 있습니다. 사용자 인증을 켜는 것을 권장합니다.", 6000)
        self.ctl.save_proxy(p, self.is_new)
        self.closed.emit()

    def cancel(self):
        self.closed.emit()


CHECK_DESC = ("저장하지 않은 지금 설정으로 확인합니다: 내보내는 쪽을 거쳐 HTTPS 사이트에 접속하고, "
              "받는 쪽 포트를 열 수 있는지(인증서 포함) 봅니다.")


def _check_inbound(p, running):
    """받는 쪽 확인 (작업 스레드에서). (정상 여부, 설명)."""
    ib = p["inbound"]
    if running:
        return True, f"{host_port(ib['host'], ib['port'])} 에서 실행 중"
    ok, msg = check_port(ib["host"], ib["port"], udp=ib["type"] == "hysteria2")
    if not ok:
        return False, f"포트 {ib['port']}: {msg}"
    if ib["type"] in ("https", "hysteria2") and ib["tls_cert"]:
        try:
            from ..tls import server_context
            server_context(ib["tls_cert"], ib["tls_key"])
        except Exception as e:
            return False, f"인증서/키를 읽지 못했습니다: {e}"
    return True, f"포트 {ib['port']} 사용 가능" + (" · 인증서 정상" if ib["tls_cert"] else "")


def _probe_hint(ot, out):
    err = (out.get("error") or "") + " " + (out.get("detail") or "")
    if ot == "shadowsocks" and ("시간 초과" in err or "TLS" in err or "닫음" in err or "복호화" in err):
        return "서버와 암호화 방식·비밀번호가 같은지 확인하세요 (틀리면 서버가 응답하지 않습니다)"
    if ot == "hysteria2" and "인증" in err:
        return "Hysteria2 비밀번호를 확인하세요"
    if ot in ("http", "https", "socks5") and ("인증" in err or "407" in err):
        return "상위 프록시 아이디/비밀번호를 확인하세요"
    if ot in ("https", "hysteria2") and ("certificate" in err.lower() or "인증서" in err):
        return "자체 서명 인증서 서버라면 '서버 인증서 확인'을 끄거나 지문을 고정하세요"
    return ""


def _token():
    import secrets
    return secrets.token_urlsafe(18)
