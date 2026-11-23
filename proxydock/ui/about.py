"""About page: app and engine versions, settings folder, links, update check."""
from ..i18n import _t
import threading

from PySide6.QtCore import QObject, QUrl, Qt, Signal
from PySide6.QtGui import QDesktopServices
from PySide6.QtWidgets import QHBoxLayout, QVBoxLayout, QWidget
from qfluentwidgets import (
    BodyLabel, CaptionLabel, FluentIcon as FIF, ImageLabel, PushButton, ScrollArea, TitleLabel,
)

from .. import __version__, config
from ..common import APP_NAME, RELEASES_URL, REPO_URL
from ..platform_utils import open_path
from .common import info, make_icon
from .proxies import Row, Section

_CORES = {}             # "sing-box"/"hysteria" -> (path, version or None), filled once per process


class _CoreBridge(QObject):
    done = Signal()


_bridge = None
_busy = False


def _bridge_obj():
    global _bridge
    if _bridge is None:
        _bridge = _CoreBridge()
    return _bridge


def probe_cores(glob):
    """Look up the engine versions in a thread (each runs `<core> version`); _bridge.done fires when finished."""
    global _busy
    if _busy or _CORES:
        return
    _busy = True
    from ..singbox import core_version, find_binary, find_hysteria
    paths = {"sing-box": find_binary(glob), "hysteria": find_hysteria(glob)}
    bridge = _bridge_obj()

    def work():
        global _busy
        for name, exe in paths.items():
            _CORES[name] = (exe, core_version(exe))
        _busy = False
        bridge.done.emit()
    threading.Thread(target=work, daemon=True).start()


def open_url(url):
    QDesktopServices.openUrl(QUrl(url))


class AboutInterface(ScrollArea):
    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        self.setObjectName("aboutInterface")
        self.ctl = ctl
        self.view = QWidget(self)
        self.setWidget(self.view)
        self.setWidgetResizable(True)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        self.enableTransparentBackground()
        v = self.view

        # header: icon, name, version
        icon = ImageLabel(make_icon().pixmap(64, 64), v)
        icon.setFixedSize(64, 64)
        name = TitleLabel(APP_NAME, v)
        self.version_label = BodyLabel(_t("Version {version}").format(version=__version__), v)
        desc = CaptionLabel(_t("Creates and runs proxies from inbound and outbound blocks."), v)
        col = QVBoxLayout()
        col.setSpacing(2)
        col.addWidget(name)
        col.addWidget(self.version_label)
        col.addWidget(desc)
        header = QHBoxLayout()
        header.setSpacing(16)
        header.addWidget(icon, 0, Qt.AlignVCenter)
        header.addLayout(col, 1)

        # version + update check
        s_ver = Section(_t("Version"), v)
        self.btn_check = PushButton(_t("Check for updates"), v, FIF.SYNC)
        self.btn_check.clicked.connect(self._check_updates)
        self.r_app = s_ver.add(Row(APP_NAME, __version__, v).right(self.btn_check))

        # engines
        s_eng = Section(_t("Engines"), v)
        self.r_singbox = s_eng.add(Row("sing-box", _t("Checking…"), v))
        self.r_hysteria = s_eng.add(Row("Hysteria2", _t("Checking…"), v))

        # settings folder
        s_dir = Section(_t("Settings folder"), v)
        btn_dir = PushButton(_t("Open folder"), v, FIF.FOLDER)
        btn_dir.clicked.connect(self._open_dir)
        s_dir.add(Row(_t("Settings and data"), config.APP_DIR, v).right(btn_dir))

        # links
        s_links = Section(_t("Links"), v)
        btn_repo = PushButton(_t("Open"), v, FIF.GITHUB)
        btn_repo.clicked.connect(lambda: open_url(REPO_URL))
        s_links.add(Row(_t("Source code (GitHub)"), REPO_URL, v).right(btn_repo))
        btn_rel = PushButton(_t("Open"), v, FIF.LINK)
        btn_rel.clicked.connect(lambda: open_url(RELEASES_URL))
        s_links.add(Row(_t("Releases"), RELEASES_URL, v).right(btn_rel))
        btn_lic = PushButton(_t("Licenses"), v, FIF.CERTIFICATE)
        btn_lic.clicked.connect(self._show_licenses)
        s_links.add(Row(_t("Licenses"), _t("Licenses of the fonts and open source components included in this program"), v).right(btn_lic))

        root = QVBoxLayout(v)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(18)
        root.addLayout(header)
        for s in (s_ver, s_eng, s_dir, s_links):
            root.addWidget(s)
        root.addStretch(1)

        _bridge_obj().done.connect(self._show_cores)
        self._show_cores()
        probe_cores(ctl.glob)

    def _show_cores(self):
        for row, key in ((self.r_singbox, "sing-box"), (self.r_hysteria, "hysteria")):
            if key not in _CORES:
                continue
            exe, version = _CORES[key]
            if not exe:
                text = _t("Not found")
            elif version:
                text = _t("Version {version}").format(version=version) + "\n" + exe
            else:
                text = _t("Version unknown") + "\n" + exe
            row.desc.setText(text)
            row.desc.setVisible(True)

    def set_update_status(self, text, busy=False):
        """Text under the app version (latest / newer version / error); busy disables the button."""
        self.r_app.desc.setText(__version__ + (f"  ·  {text}" if text else ""))
        self.btn_check.setEnabled(not busy)

    def _check_updates(self):
        updater = getattr(self.ctl, "updater", None)
        if updater is not None:
            updater.check(manual=True)

    def _show_licenses(self):
        w = self.window()
        if hasattr(w, "license_page"):
            w.switchTo(w.license_page)

    def _open_dir(self):
        try:
            open_path(config.APP_DIR)
        except OSError as e:
            info(self.window(), "error", _t("Cannot open the folder"), str(e))
