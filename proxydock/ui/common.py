"""Shared UI: fonts, theme, icon, colors, text view, proxy selector."""
from .. import i18n
from ..i18n import _t, on_language_change
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
# Built-in UI fonts for languages whose kanji/hanzi Pretendard draws in the Korean style; only the current one is loaded.
LANG_FONT_FILES = {
    "ja": os.path.join("fonts", "PretendardJPVariable.ttf"),
    "zh_CN": os.path.join("fonts", "NotoSansSC.ttf"),
    "zh_TW": os.path.join("fonts", "NotoSansTC.ttf"),
}
MONO_FONT_FILE = os.path.join("fonts", "JetBrainsMono.ttf")
LICENSE_DIR = "licenses"


class Fonts:
    builtin_ui = "Segoe UI"         # built-in Pretendard family name after load_fonts() succeeds
    builtin_mono = "Cascadia Mono"  # built-in JetBrains Mono family name after load_fonts() succeeds
    ui = "Segoe UI"                 # UI font actually in use
    ui_choice = ""                  # ui_font setting this run started with ("" = built-in)
    startup_scale = 100             # app scale (%) applied for this run
    theme = "auto"


def resource_path(rel):
    base = getattr(sys, "_MEIPASS", None) or os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    return os.path.join(base, rel)


def _register_font(rel):
    fid = QFontDatabase.addApplicationFont(resource_path(rel))
    fams = QFontDatabase.applicationFontFamilies(fid) if fid >= 0 else []
    if not fams:
        log(LOG_WARN, _t("Could not load the built-in font, using the system font: {rel}").format(rel=rel))
    return fams[0] if fams else None


_CJK_UI_FONTS = {
    "ja": ["Yu Gothic UI", "Meiryo UI", "Meiryo", "Hiragino Sans", "Noto Sans CJK JP", "Noto Sans JP"],
    "zh_CN": ["Microsoft YaHei UI", "Microsoft YaHei", "PingFang SC", "Noto Sans CJK SC", "Noto Sans SC"],
    "zh_TW": ["Microsoft JhengHei UI", "Microsoft JhengHei", "PingFang TC", "Noto Sans CJK TC", "Noto Sans TC"],
}


def load_fonts(app, glob):
    """Register the built-in fonts and set the UI font app-wide (Fluent widgets included). Call before creating any QWidget."""
    Fonts.builtin_mono = _register_font(MONO_FONT_FILE) or Fonts.builtin_mono
    Fonts.builtin_ui = _register_font(FONT_FILE) or Fonts.builtin_ui
    chosen = glob["ui_font"]
    if chosen and chosen not in QFontDatabase.families():
        log(LOG_WARN, _t("UI font '{chosen}' not found, using the built-in font").format(chosen=chosen))
        chosen = ""
    # Pretendard only has Korean-style Hanja, so Japanese/Chinese use their own built-in font (Pretendard JP,
    # Noto Sans SC/TC), or the system font of that language when it is missing; otherwise kanji/hanzi would mix
    # two glyph styles.
    lang = i18n.current()
    lang_file = LANG_FONT_FILES.get(lang)
    lang_font = _register_font(lang_file) if lang_file and os.path.exists(resource_path(lang_file)) else None
    cjk = ([lang_font] if lang_font else []) + _CJK_UI_FONTS.get(lang, [])
    native = next((f for f in cjk if f in QFontDatabase.families()), "")
    Fonts.ui_choice = chosen
    Fonts.ui = chosen or native or Fonts.builtin_ui
    families = [Fonts.ui] + cjk + [Fonts.builtin_ui, "Segoe UI", "Malgun Gothic", "Apple SD Gothic Neo",
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
    """Log/license font: monospace (built-in JetBrains Mono by default), with the UI font as fallback for CJK text."""
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
    """App icon: three boxes on a dock over a rounded square."""
    pm = QPixmap(256, 256)
    pm.fill(Qt.transparent)
    p = QPainter(pm)
    p.setRenderHint(QPainter.Antialiasing)
    p.setPen(Qt.NoPen)
    p.setBrush(QColor(0, 120, 212))
    p.drawRoundedRect(16, 16, 224, 224, 52, 52)
    p.setBrush(QColor(255, 255, 255))
    p.drawRoundedRect(52, 170, 152, 20, 8, 8)              # dock
    for i, h in enumerate((54, 84, 64)):                      # boxes (the proxies)
        x = 62 + i * 46
        p.drawRoundedRect(x, 160 - h, 38, h, 8, 8)
    p.end()
    return QIcon(pm)


_PROBLEMS = frozenset((LOG_BLOCKED, LOG_AUTH, LOG_FAIL, LOG_ERROR, LOG_WARN))
LEVEL_FILTERS = []


@on_language_change
def _tr_level_filters():
    LEVEL_FILTERS[:] = [
        (_t("All"), None),
        (_t("Connections (TCP+UDP)"), frozenset((LOG_TCP, LOG_UDP))),
        ("TCP", frozenset((LOG_TCP,))),
        ("UDP", frozenset((LOG_UDP,))),
        (_t("All problems"), _PROBLEMS),
        (_t("Blocked"), frozenset((LOG_BLOCKED,))),
        (_t("Auth failures"), frozenset((LOG_AUTH,))),
        (_t("Connection failures"), frozenset((LOG_FAIL,))),
        (_t("Errors & warnings"), frozenset((LOG_ERROR, LOG_WARN))),
        (_t("Settings"), frozenset((LOG_SETTING,))),
        (_t("Info"), frozenset((LOG_INFO,))),
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
STATE_TEXT = {}


@on_language_change
def _tr_state_text():
    STATE_TEXT.clear()
    STATE_TEXT.update({"running": _t("Running"), "starting": _t("Starting…"), "stopped": _t("Stopped"), "error": _t("error")})


STATE_COLORS = {"running": "#16A34A", "starting": "#CA8A04", "stopped": "#8A8A8A", "error": "#DC2626"}


def level_color(level):
    return QColor((LOG_COLORS_DARK if isDarkTheme() else LOG_COLORS_LIGHT).get(level, "#888888"))


class TextView(PlainTextEdit):
    """Read-only text view for logs/licenses. Paints its viewport opaquely so scrolling does not repaint the whole window."""

    BG_DARK = QColor("#343434")
    BG_LIGHT = QColor("#FBFBFB")

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setReadOnly(True)
        self.setUndoRedoEnabled(False)
        vp = self.viewport()
        vp.setAttribute(Qt.WA_OpaquePaintEvent, True)
        vp.setAutoFillBackground(False)
        qconfig.themeChanged.connect(self._on_theme_changed)

    def _on_theme_changed(self, *_):
        self.viewport().update()

    def paintEvent(self, e):
        p = QPainter(self.viewport())
        p.fillRect(e.rect(), self.BG_DARK if isDarkTheme() else self.BG_LIGHT)
        p.end()
        super().paintEvent(e)


class ProxyCombo(ComboBox):
    """Proxy selector. With all_label, an 'all' item (value "") comes first."""

    def __init__(self, parent=None, all_label="", width=260):
        super().__init__(parent)
        self.all_label = _t("All proxies") if all_label == "" else all_label
        self.ids = []
        self.setFixedWidth(width)

    def set_proxies(self, proxies, only=None):
        """only: set of proxy ids to show (None = all). Keeps the selection when possible."""
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
    if sys.platform != "darwin":
        return getattr(InfoBar, kind)(title, content, duration=duration,
                                      position=InfoBarPosition.TOP_RIGHT, parent=parent)
    # macOS: an InfoBar paints through a QGraphicsOpacityEffect and deletes itself after fading out, which crashes
    # in Qt's paint path there (seen in CI, like the page transitions). Drop the effect and close without the fade.
    from PySide6.QtCore import QTimer
    bar = getattr(InfoBar, kind)(title, content, duration=-1, position=InfoBarPosition.TOP_RIGHT, parent=parent)
    bar.setGraphicsEffect(None)
    if duration >= 0:
        QTimer.singleShot(duration, bar, bar.close)
    return bar


DIALOG_NO_EFFECTS = sys.platform == "darwin"       # no fade/shadow on dialogs; tests switch it on to run the macOS path elsewhere


class MacSafeDialog:
    """Mix in before a qfluentwidgets dialog (MessageBoxBase, MessageBox, ColorDialog): on macOS paint it without
    graphics effects. Its fade in/out (QGraphicsOpacityEffect) and drop shadow (QGraphicsDropShadowEffect) both
    crashed in Qt's paint path there (seen in CI while a dialog was open or closing), like the InfoBar in info()
    and the page transitions."""

    def setShadowEffect(self, *args, **kwargs):
        if DIALOG_NO_EFFECTS:
            self.widget.setGraphicsEffect(None)
        else:
            super().setShadowEffect(*args, **kwargs)

    def showEvent(self, e):
        if DIALOG_NO_EFFECTS:
            from PySide6.QtWidgets import QDialog
            QDialog.showEvent(self, e)
        else:
            super().showEvent(e)

    def done(self, code):
        if DIALOG_NO_EFFECTS:
            from PySide6.QtWidgets import QDialog
            self.setGraphicsEffect(None)
            QDialog.done(self, code)
        else:
            super().done(code)


def ConfirmBox(title, content, parent):
    """qfluentwidgets MessageBox (yes/cancel) without graphics effects on macOS (see MacSafeDialog)."""
    from qfluentwidgets import MessageBox

    class _Box(MacSafeDialog, MessageBox):
        pass
    return _Box(title, content, parent)


def ColorPicker(color, title, parent):
    """qfluentwidgets ColorDialog without graphics effects on macOS (see MacSafeDialog)."""
    from qfluentwidgets import ColorDialog

    class _Picker(MacSafeDialog, ColorDialog):
        pass
    return _Picker(color, title, parent)
