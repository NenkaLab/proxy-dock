"""UI 공통: 글꼴·테마·아이콘·색·텍스트 뷰·프록시 선택 드롭다운."""
import os
import sys

from PySide6.QtCore import Qt
from PySide6.QtGui import QColor, QFont, QFontDatabase, QIcon, QPainter, QPixmap
from qfluentwidgets import (
    ComboBox, InfoBar, InfoBarPosition, PlainTextEdit, Theme, isDarkTheme, qconfig, setFontFamilies, setTheme,
)

from ..common import (
    LOG_AUTH, LOG_BLOCKED, LOG_ERROR, LOG_FAIL, LOG_INFO, LOG_SETTING, LOG_TCP, LOG_UDP, LOG_WARN, log,
)

FONT_FILE = os.path.join("fonts", "PretendardVariable.ttf")
MONO_FONT_FILE = os.path.join("fonts", "JetBrainsMono.ttf")
LICENSE_DIR = "licenses"


class Fonts:
    builtin_ui = "Segoe UI"         # load_fonts() 성공 시 내장 Pretendard 패밀리명
    builtin_mono = "Cascadia Mono"  # load_fonts() 성공 시 내장 JetBrains Mono 패밀리명
    ui = "Segoe UI"                 # 실제 사용 중인 UI 글꼴
    startup_scale = 100             # 이번 실행에 적용된 앱 크기(%)
    theme = "auto"


def resource_path(rel):
    base = getattr(sys, "_MEIPASS", None) or os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    return os.path.join(base, rel)


def _register_font(rel):
    fid = QFontDatabase.addApplicationFont(resource_path(rel))
    fams = QFontDatabase.applicationFontFamilies(fid) if fid >= 0 else []
    if not fams:
        log(LOG_WARN, f"내장 글꼴을 불러오지 못해 시스템 글꼴 사용: {rel}")
    return fams[0] if fams else None


def load_fonts(app, glob):
    """내장 글꼴을 등록하고 UI 글꼴을 앱 전체(Fluent 위젯 포함)에 지정한다. QWidget 을 만들기 전에 호출."""
    Fonts.builtin_mono = _register_font(MONO_FONT_FILE) or Fonts.builtin_mono
    Fonts.builtin_ui = _register_font(FONT_FILE) or Fonts.builtin_ui
    chosen = glob["ui_font"]
    if chosen and chosen not in QFontDatabase.families():
        log(LOG_WARN, f"UI 글꼴 '{chosen}' 이(가) 없어 내장 글꼴 사용")
        chosen = ""
    Fonts.ui = chosen or Fonts.builtin_ui
    families = [Fonts.ui, Fonts.builtin_ui, "Segoe UI", "Malgun Gothic", "Apple SD Gothic Neo",
                "Noto Sans CJK KR", "Noto Sans KR"]
    setFontFamilies(families)
    font = QFont()
    font.setFamilies(families)
    font.setPointSize(10)
    font.setHintingPreference(QFont.PreferNoHinting)
    app.setFont(font)


def apply_theme(name):
    setTheme({"auto": Theme.AUTO, "light": Theme.LIGHT, "dark": Theme.DARK}[name])
    Fonts.theme = name


def make_mono_font(glob):
    """로그·라이선스 글꼴: 고정폭(기본 내장 JetBrains Mono) + 한글은 UI 글꼴로 대체."""
    font = QFont()
    font.setFamilies([glob["mono_font"] or Fonts.builtin_mono, Fonts.builtin_mono,
                      "Cascadia Mono", "Consolas", "Menlo", "DejaVu Sans Mono", Fonts.ui])
    font.setPointSize(glob["log_font_size"])
    font.setStyleHint(QFont.Monospace)
    if glob["disable_ligatures"]:
        for tag in ("calt", "liga", "dlig"):
            font.setFeature(QFont.Tag(tag), 0)
    return font


def make_icon():
    """앱 아이콘: 둥근 사각형 위에 도크(받침) 위의 상자 세 개."""
    pm = QPixmap(256, 256)
    pm.fill(Qt.transparent)
    p = QPainter(pm)
    p.setRenderHint(QPainter.Antialiasing)
    p.setPen(Qt.NoPen)
    p.setBrush(QColor(0, 120, 212))
    p.drawRoundedRect(16, 16, 224, 224, 52, 52)
    p.setBrush(QColor(255, 255, 255))
    p.drawRoundedRect(52, 170, 152, 20, 8, 8)              # 도크
    for i, h in enumerate((54, 84, 64)):                      # 상자 (프록시들)
        x = 62 + i * 46
        p.drawRoundedRect(x, 160 - h, 38, h, 8, 8)
    p.end()
    return QIcon(pm)


_PROBLEMS = frozenset((LOG_BLOCKED, LOG_AUTH, LOG_FAIL, LOG_ERROR, LOG_WARN))
LEVEL_FILTERS = [
    ("전체", None),
    ("연결 (TCP+UDP)", frozenset((LOG_TCP, LOG_UDP))),
    ("TCP", frozenset((LOG_TCP,))),
    ("UDP", frozenset((LOG_UDP,))),
    ("문제 전체", _PROBLEMS),
    ("차단", frozenset((LOG_BLOCKED,))),
    ("인증 실패", frozenset((LOG_AUTH,))),
    ("연결 실패", frozenset((LOG_FAIL,))),
    ("오류·경고", frozenset((LOG_ERROR, LOG_WARN))),
    ("설정", frozenset((LOG_SETTING,))),
    ("정보", frozenset((LOG_INFO,))),
]
LOG_COLORS_DARK = {
    LOG_TCP: "#6CCB5F", LOG_UDP: "#60CDFF", LOG_BLOCKED: "#FFB86C",
    LOG_AUTH: "#D7A6FF", LOG_FAIL: "#F7A8B8", LOG_ERROR: "#FF6B6B",
    LOG_WARN: "#FCE100", LOG_SETTING: "#E8D44D", LOG_INFO: "#A8A8A8",
}
LOG_COLORS_LIGHT = {
    LOG_TCP: "#107C10", LOG_UDP: "#005FB8", LOG_BLOCKED: "#C25E00",
    LOG_AUTH: "#8764B8", LOG_FAIL: "#A4262C", LOG_ERROR: "#C42B1C",
    LOG_WARN: "#8A6D00", LOG_SETTING: "#9D5D00", LOG_INFO: "#605E5C",
}
STATE_TEXT = {"running": "실행 중", "starting": "시작 중…", "stopped": "중지됨", "error": "오류"}
STATE_COLORS = {"running": "#16A34A", "starting": "#CA8A04", "stopped": "#8A8A8A", "error": "#DC2626"}


def level_color(level):
    return QColor((LOG_COLORS_DARK if isDarkTheme() else LOG_COLORS_LIGHT).get(level, "#888888"))


class TextView(PlainTextEdit):
    """로그·라이선스용 읽기 전용 텍스트 뷰. 뷰포트를 불투명하게 직접 칠해 스크롤 때 창 전체가 다시 그려지지 않게."""

    BG_DARK = QColor("#343434")
    BG_LIGHT = QColor("#FBFBFB")

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setReadOnly(True)
        self.setUndoRedoEnabled(False)
        vp = self.viewport()
        vp.setAttribute(Qt.WA_OpaquePaintEvent, True)
        vp.setAutoFillBackground(False)
        qconfig.themeChanged.connect(lambda *_: vp.update())

    def paintEvent(self, e):
        p = QPainter(self.viewport())
        p.fillRect(e.rect(), self.BG_DARK if isDarkTheme() else self.BG_LIGHT)
        p.end()
        super().paintEvent(e)


class ProxyCombo(ComboBox):
    """프록시 선택 드롭다운. all_label 이 있으면 맨 위에 '전체' 항목 (값 "")."""

    def __init__(self, parent=None, all_label="전체 프록시", width=260):
        super().__init__(parent)
        self.all_label = all_label
        self.ids = []
        self.setFixedWidth(width)

    def set_proxies(self, proxies, only=None):
        """only: 표시할 프록시 ID 집합 (None 이면 모두). 선택은 가능하면 유지."""
        cur = self.current_id()
        self.blockSignals(True)
        self.clear()
        self.ids = []
        if self.all_label:
            self.addItem(self.all_label)
            self.ids.append("")
        for p in proxies:
            if only is not None and p["id"] not in only:
                continue
            self.addItem(p["name"])
            self.ids.append(p["id"])
        idx = self.ids.index(cur) if cur in self.ids else 0
        if self.ids:
            self.setCurrentIndex(idx)
        self.blockSignals(False)
        if cur != self.current_id():
            self.currentIndexChanged.emit(self.currentIndex())

    def current_id(self):
        i = self.currentIndex()
        return self.ids[i] if 0 <= i < len(self.ids) else None

    def select_id(self, pid):
        if pid in self.ids:
            self.setCurrentIndex(self.ids.index(pid))


def info(parent, kind, title, content, duration=2500):
    return getattr(InfoBar, kind)(title, content, duration=duration,
                                  position=InfoBarPosition.TOP_RIGHT, parent=parent)
