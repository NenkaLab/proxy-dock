"""전역 설정 페이지: 프록시 기본값(로컬 설정이 우선) + 앱 설정(로그·화면·글꼴·시작·방화벽)."""
from PySide6.QtCore import Qt
from PySide6.QtGui import QFontDatabase
from PySide6.QtWidgets import QHBoxLayout, QVBoxLayout, QWidget
from qfluentwidgets import (
    BodyLabel, CaptionLabel, ComboBox, FluentIcon as FIF, LineEdit, PrimaryPushButton, PushButton, ScrollArea,
    SpinBox, TitleLabel,
)

from .. import config
from ..common import IS_WIN
from ..platform_utils import open_path
from .common import Fonts, info
from .proxies import Row, Section, _switch


class SettingsInterface(ScrollArea):
    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        self.setObjectName("settingsInterface")
        self.ctl = ctl
        self.view = QWidget(self)
        self.setWidget(self.view)
        self.setWidgetResizable(True)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        self.enableTransparentBackground()
        v = self.view

        title = TitleLabel("설정", v)
        self.btn_apply = PrimaryPushButton("적용", v, FIF.SAVE)
        self.btn_apply.clicked.connect(self.apply)

        # 프록시 기본값
        s_def = Section("프록시 기본값 (각 프록시에서 '전역 따름' 인 항목에 적용)", v)
        self.ed_allow = LineEdit(v)
        self.ed_allow.setFixedWidth(300)
        s_def.add(Row("허용 도메인", "쉼표로 구분 · * = 전체 · *.example.com = 하위 도메인 포함", v).right(self.ed_allow))
        self.sw_private = _switch(v)
        s_def.add(Row("내부망 목적지 차단", "루프백·사설 IP·링크로컬 주소로의 연결을 거부합니다", v).right(self.sw_private))
        self.sp_timeout = SpinBox(v)
        self.sp_timeout.setRange(10, 86400)
        self.sp_timeout.setFixedWidth(150)
        s_def.add(Row("유휴 타임아웃(초)", "데이터가 오가지 않는 TCP 연결을 끊는 시간", v).right(self.sp_timeout))
        self.sw_logconn = _switch(v)
        s_def.add(Row("연결 로그 기록", "끄면 성공한 연결은 기록하지 않습니다 (오류는 항상 기록)", v).right(self.sw_logconn))
        self.sw_logclose = _switch(v)
        s_def.add(Row("연결 종료 상세 기록", "연결이 끝날 때 업/다운로드 양, 지속 시간, 종료 사유", v).right(self.sw_logclose))

        # 로그
        s_log = Section("로그", v)
        btn_folder = PushButton("폴더 열기", v, FIF.FOLDER)
        btn_folder.clicked.connect(self._open_log_dir)
        self.sw_logfile = _switch(v)
        s_log.add(Row("파일로 저장", config.LOG_DIR, v).right(btn_folder, self.sw_logfile))

        # 화면
        s_view = Section("화면", v)
        self.cb_theme = ComboBox(v)
        self.cb_theme.addItems([label for _, label in config.THEMES])
        self.cb_theme.setFixedWidth(160)
        s_view.add(Row("테마", "기기 설정을 따르거나 라이트/다크로 고정합니다", v).right(self.cb_theme))
        self.cb_scale = ComboBox(v)
        self.cb_scale.addItems([f"{s}%" + (" (기본)" if s == 100 else "") for s in config.UI_SCALES])
        self.cb_scale.setFixedWidth(160)
        s_view.add(Row("앱 크기", "글자와 화면 요소를 함께 키우거나 줄입니다 (재시작 후 적용)", v).right(self.cb_scale))
        self.sp_logsize = SpinBox(v)
        self.sp_logsize.setRange(8, 20)
        self.sp_logsize.setFixedWidth(150)
        s_view.add(Row("로그 글자 크기", "로그·라이선스 창의 글자 크기(pt)", v).right(self.sp_logsize))
        self.sw_close = _switch(v)
        s_view.add(Row("X 버튼으로 종료", "끄면 X 를 눌러도 트레이에서 계속 동작합니다", v).right(self.sw_close))

        # 글꼴
        s_font = Section("글꼴", v)
        fams = sorted({f for f in QFontDatabase.families() if not f.startswith("@")}, key=str.lower)
        self.ui_fonts = [""] + [f for f in fams if f != Fonts.builtin_ui]
        self.mono_fonts = [""] + [f for f in fams if f != Fonts.builtin_mono and QFontDatabase.isFixedPitch(f)]
        self.cb_uifont = ComboBox(v)
        self.cb_uifont.addItems([f"내장 · {Fonts.builtin_ui}"] + self.ui_fonts[1:])
        self.cb_uifont.setFixedWidth(240)
        self.cb_uifont.setMaxVisibleItems(12)
        s_font.add(Row("UI 글꼴", "메뉴·설정 등 화면 전체 (재시작 후 적용)", v).right(self.cb_uifont))
        self.cb_monofont = ComboBox(v)
        self.cb_monofont.addItems([f"내장 · {Fonts.builtin_mono}"] + self.mono_fonts[1:])
        self.cb_monofont.setFixedWidth(240)
        self.cb_monofont.setMaxVisibleItems(12)
        s_font.add(Row("로그 글꼴", "로그 창의 고정폭 글꼴 (한글은 UI 글꼴로 표시)", v).right(self.cb_monofont))
        self.sw_liga = _switch(v)
        s_font.add(Row("합자(리거처) 끄기", "로그 창에서 기호를 합쳐 그리지 않습니다 (예: -> 를 화살표로)", v).right(self.sw_liga))

        # 시작 (즉시 적용)
        s_start = Section("시작 (바로 적용)", v)
        self.sw_startup = _switch(v)
        s_start.add(Row("시작프로그램 등록", "로그인할 때 ProxyDock 을 자동으로 실행합니다", v).right(self.sw_startup))
        self.sw_hidden = _switch(v)
        s_start.add(Row("시작 알림 숨기기", "자동 실행 시 '실행 중' 알림을 표시하지 않습니다", v).right(self.sw_hidden))
        self.sw_startup.checkedChanged.connect(ctl.set_startup)
        self.sw_hidden.checkedChanged.connect(ctl.set_hidden)

        sections = [s_def, s_log, s_view, s_font, s_start]
        if IS_WIN:
            s_fw = Section("Windows 방화벽", v)
            self.fw_label = BodyLabel("확인 중…", v)
            self.btn_fw_on = PushButton("차단 켜기", v)
            self.btn_fw_off = PushButton("차단 끄기", v)
            self.btn_fw_on.clicked.connect(lambda: ctl.firewall(True))
            self.btn_fw_off.clicked.connect(lambda: ctl.firewall(False))
            s_fw.add(Row("외부 접속 차단", "다른 PC 에서 ProxyDock 으로 들어오는 연결을 막습니다 (모든 프록시, 관리자 권한 필요). "
                         "다른 기기용 서버(Shadowsocks·Hysteria2 등)를 쓰려면 끄세요", v)
                     .right(self.fw_label, self.btn_fw_on, self.btn_fw_off))
            sections.append(s_fw)

        header = QHBoxLayout()
        header.addWidget(title)
        header.addStretch(1)
        header.addWidget(self.btn_apply)
        root = QVBoxLayout(v)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(18)
        root.addLayout(header)
        for s in sections:
            root.addWidget(s)
        root.addStretch(1)
        self.load(ctl.glob)

    def load(self, g):
        self.ed_allow.setText(", ".join(g["allowed_domains"]))
        self.sw_private.setChecked(g["block_private"])
        self.sp_timeout.setValue(g["timeout"])
        self.sw_logconn.setChecked(g["log_connections"])
        self.sw_logclose.setChecked(g["log_close"])
        self.sw_logfile.setChecked(g["log_to_file"])
        self.cb_theme.setCurrentIndex([k for k, _ in config.THEMES].index(g["theme"]))
        self.cb_scale.setCurrentIndex(config.UI_SCALES.index(g["ui_scale"]))
        self.sp_logsize.setValue(g["log_font_size"])
        self.sw_close.setChecked(g["close_exits"])
        self.cb_uifont.setCurrentIndex(self.ui_fonts.index(g["ui_font"]) if g["ui_font"] in self.ui_fonts else 0)
        self.cb_monofont.setCurrentIndex(self.mono_fonts.index(g["mono_font"]) if g["mono_font"] in self.mono_fonts else 0)
        self.sw_liga.setChecked(g["disable_ligatures"])

    def apply(self):
        domains = [d.strip() for d in self.ed_allow.text().replace(";", ",").split(",") if d.strip()]
        if not domains:
            info(self.window(), "error", "허용 도메인", "하나 이상 입력하세요 (전체 허용은 *)")
            return
        g = dict(self.ctl.glob)
        g.update(
            allowed_domains=domains, block_private=self.sw_private.isChecked(), timeout=self.sp_timeout.value(),
            log_connections=self.sw_logconn.isChecked(), log_close=self.sw_logclose.isChecked(),
            log_to_file=self.sw_logfile.isChecked(),
            theme=config.THEMES[max(self.cb_theme.currentIndex(), 0)][0],
            ui_scale=config.UI_SCALES[max(self.cb_scale.currentIndex(), 0)],
            log_font_size=self.sp_logsize.value(), close_exits=self.sw_close.isChecked(),
            ui_font=self.ui_fonts[max(self.cb_uifont.currentIndex(), 0)],
            mono_font=self.mono_fonts[max(self.cb_monofont.currentIndex(), 0)],
            disable_ligatures=self.sw_liga.isChecked(),
        )
        self.ctl.apply_global(config.validate_global(g))

    def refresh_startup(self, registered, hidden):
        for sw, val in ((self.sw_startup, registered), (self.sw_hidden, hidden)):
            sw.blockSignals(True)
            sw.setChecked(val)
            sw.blockSignals(False)
        self.sw_hidden.setEnabled(registered)

    def set_firewall_status(self, state):
        if not IS_WIN:
            return
        self.fw_label.setText({True: "켜짐", False: "꺼짐", None: "확인 중…"}[state])
        busy = state is None
        self.btn_fw_on.setEnabled(not busy and state is not True)
        self.btn_fw_off.setEnabled(not busy and state is not False)

    def _open_log_dir(self):
        try:
            open_path(config.LOG_DIR)
        except OSError as e:
            info(self.window(), "error", "폴더를 열 수 없습니다", str(e))
