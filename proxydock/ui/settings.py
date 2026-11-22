"""Global settings page: proxy defaults (local settings take precedence) + app settings (logs, display, fonts, startup, firewall)."""
from ..i18n import LANGUAGE_CODES, LANGUAGES, _t
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

        title = TitleLabel(_t("Settings"), v)
        self.btn_apply = PrimaryPushButton(_t("Apply"), v, FIF.SAVE)
        self.btn_apply.clicked.connect(self.apply)

        # proxy defaults
        s_def = Section(_t("Proxy defaults (apply to items set to 'Use global' in each proxy)"), v)
        self.ed_allow = LineEdit(v)
        self.ed_allow.setFixedWidth(300)
        s_def.add(Row(_t("Allowed domains"), _t("Comma separated · * = all · *.example.com = including subdomains"), v).right(self.ed_allow))
        self.sw_private = _switch(v)
        s_def.add(Row(_t("Block private network destinations"), _t("Refuses connections to loopback, private and link-local addresses"), v).right(self.sw_private))
        self.sp_timeout = SpinBox(v)
        self.sp_timeout.setRange(10, 86400)
        self.sp_timeout.setFixedWidth(150)
        s_def.add(Row(_t("Idle timeout (s)"), _t("Time after which an idle TCP connection is closed"), v).right(self.sp_timeout))
        self.sw_logconn = _switch(v)
        s_def.add(Row(_t("Log connections"), _t("When off, successful connections are not logged (errors always are)"), v).right(self.sw_logconn))
        self.sw_logclose = _switch(v)
        s_def.add(Row(_t("Log connection close details"), _t("Upload/download amount, duration and reason when a connection ends"), v).right(self.sw_logclose))

        # engine
        s_eng = Section(_t("Proxy engine"), v)
        self.cb_engine = ComboBox(v)
        self.cb_engine.addItems([_t("sing-box (recommended)"), _t("Python (built-in)")])
        self.cb_engine.setFixedWidth(200)
        from ..singbox import find_binary
        exe = find_binary(ctl.glob)
        s_eng.add(Row(_t("Engine"), _t("sing-box runs each proxy in its own process: faster, uses every CPU core, UDP works through chains. Running proxies restart when this changes.")
                      + "\n" + (exe or _t("sing-box was not found - proxies use the Python engine")), v).right(self.cb_engine))

        # logs
        s_log = Section(_t("Logs"), v)
        btn_folder = PushButton(_t("Open folder"), v, FIF.FOLDER)
        btn_folder.clicked.connect(self._open_log_dir)
        self.sw_logfile = _switch(v)
        s_log.add(Row(_t("Save to file"), config.LOG_DIR, v).right(btn_folder, self.sw_logfile))

        # display
        s_view = Section(_t("Display"), v)
        self.cb_lang = ComboBox(v)
        self.cb_lang.addItems([name for _, name in LANGUAGES])
        self.cb_lang.setFixedWidth(160)
        s_view.add(Row(_t("Language"), _t("Applies right away"), v).right(self.cb_lang))
        self.cb_theme = ComboBox(v)
        self.cb_theme.addItems([label for _, label in config.THEMES])
        self.cb_theme.setFixedWidth(160)
        s_view.add(Row(_t("Theme"), _t("Follow the system or force light/dark"), v).right(self.cb_theme))
        self.cb_scale = ComboBox(v)
        self.cb_scale.addItems([f"{s}%" + (_t(" (default)") if s == 100 else "") for s in config.UI_SCALES])
        self.cb_scale.setFixedWidth(160)
        s_view.add(Row(_t("App scale"), _t("Scales text and UI elements together (applies after restart)"), v).right(self.cb_scale))
        self.sp_logsize = SpinBox(v)
        self.sp_logsize.setRange(8, 20)
        self.sp_logsize.setFixedWidth(150)
        s_view.add(Row(_t("Log font size"), _t("Font size of the log and license views (pt)"), v).right(self.sp_logsize))
        self.sw_close = _switch(v)
        s_view.add(Row(_t("Quit with the X button"), _t("When off, the app keeps running in the tray after X"), v).right(self.sw_close))

        # fonts
        s_font = Section(_t("Fonts"), v)
        fams = sorted({f for f in QFontDatabase.families() if not f.startswith("@")}, key=str.lower)
        self.ui_fonts = [""] + [f for f in fams if f != Fonts.builtin_ui]
        self.mono_fonts = [""] + [f for f in fams if f != Fonts.builtin_mono and QFontDatabase.isFixedPitch(f)]
        self.cb_uifont = ComboBox(v)
        self.cb_uifont.addItems([_t("Built-in · {builtin_ui}").format(builtin_ui=Fonts.builtin_ui)] + self.ui_fonts[1:])
        self.cb_uifont.setFixedWidth(240)
        self.cb_uifont.setMaxVisibleItems(12)
        s_font.add(Row(_t("UI font"), _t("Menus, settings and the whole UI (applies after restart)"), v).right(self.cb_uifont))
        self.cb_monofont = ComboBox(v)
        self.cb_monofont.addItems([_t("Built-in · {builtin_mono}").format(builtin_mono=Fonts.builtin_mono)] + self.mono_fonts[1:])
        self.cb_monofont.setFixedWidth(240)
        self.cb_monofont.setMaxVisibleItems(12)
        s_font.add(Row(_t("Log font"), _t("Monospace font of the log view (CJK text uses the UI font)"), v).right(self.cb_monofont))
        self.sw_liga = _switch(v)
        s_font.add(Row(_t("Disable ligatures"), _t("Do not merge symbols in the log view (e.g. -> into an arrow)"), v).right(self.sw_liga))

        # startup (applied immediately)
        s_start = Section(_t("Startup (applied immediately)"), v)
        self.sw_startup = _switch(v)
        s_start.add(Row(_t("Start at login"), _t("Runs ProxyDock automatically when you sign in"), v).right(self.sw_startup))
        self.sw_hidden = _switch(v)
        s_start.add(Row(_t("Hide startup notification"), _t("Do not show the 'running' notification when started automatically"), v).right(self.sw_hidden))
        self.sw_startup.checkedChanged.connect(ctl.set_startup)
        self.sw_hidden.checkedChanged.connect(ctl.set_hidden)

        sections = [s_def, s_eng, s_log, s_view, s_font, s_start]
        if IS_WIN:
            s_fw = Section(_t("Windows Firewall"), v)
            self.fw_label = BodyLabel(_t("Checking…"), v)
            self.btn_fw_on = PushButton(_t("Turn block on"), v)
            self.btn_fw_off = PushButton(_t("Turn block off"), v)
            self.btn_fw_on.clicked.connect(lambda: ctl.firewall(True))
            self.btn_fw_off.clicked.connect(lambda: ctl.firewall(False))
            s_fw.add(Row(_t("Block external access"), _t("Blocks connections from other PCs to ProxyDock (all proxies, needs administrator rights). Turn off to use servers for other devices (Shadowsocks, Hysteria2, etc.)"), v)
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
        self.cb_lang.setCurrentIndex(LANGUAGE_CODES.index(g["language"]))
        self.cb_theme.setCurrentIndex([k for k, _ in config.THEMES].index(g["theme"]))
        self.cb_scale.setCurrentIndex(config.UI_SCALES.index(g["ui_scale"]))
        self.sp_logsize.setValue(g["log_font_size"])
        self.sw_close.setChecked(g["close_exits"])
        self.cb_uifont.setCurrentIndex(self.ui_fonts.index(g["ui_font"]) if g["ui_font"] in self.ui_fonts else 0)
        self.cb_monofont.setCurrentIndex(self.mono_fonts.index(g["mono_font"]) if g["mono_font"] in self.mono_fonts else 0)
        self.sw_liga.setChecked(g["disable_ligatures"])
        self.cb_engine.setCurrentIndex(config.ENGINES.index(g["engine"]))

    def apply(self):
        domains = [d.strip() for d in self.ed_allow.text().replace(";", ",").split(",") if d.strip()]
        if not domains:
            info(self.window(), "error", _t("Allowed domains"), _t("Enter at least one (* allows all)"))
            return
        g = dict(self.ctl.glob)
        g.update(
            allowed_domains=domains, block_private=self.sw_private.isChecked(), timeout=self.sp_timeout.value(),
            log_connections=self.sw_logconn.isChecked(), log_close=self.sw_logclose.isChecked(),
            log_to_file=self.sw_logfile.isChecked(),
            language=LANGUAGE_CODES[max(self.cb_lang.currentIndex(), 0)],
            theme=config.THEMES[max(self.cb_theme.currentIndex(), 0)][0],
            ui_scale=config.UI_SCALES[max(self.cb_scale.currentIndex(), 0)],
            log_font_size=self.sp_logsize.value(), close_exits=self.sw_close.isChecked(),
            ui_font=self.ui_fonts[max(self.cb_uifont.currentIndex(), 0)],
            mono_font=self.mono_fonts[max(self.cb_monofont.currentIndex(), 0)],
            disable_ligatures=self.sw_liga.isChecked(),
            engine=config.ENGINES[max(self.cb_engine.currentIndex(), 0)],
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
        self.fw_label.setText({True: _t("On"), False: _t("Off"), None: _t("Checking…")}[state])
        busy = state is None
        self.btn_fw_on.setEnabled(not busy and state is not True)
        self.btn_fw_off.setEnabled(not busy and state is not False)

    def _open_log_dir(self):
        try:
            open_path(config.LOG_DIR)
        except OSError as e:
            info(self.window(), "error", _t("Cannot open the folder"), str(e))
