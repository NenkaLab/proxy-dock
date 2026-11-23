"""Update check in the UI: startup check (once a day), the About page button, notifications and the release notes
dialog. The network work runs in threads; results come back through Qt signals."""
from ..i18n import _t
import threading
import time

from PySide6.QtCore import QObject, QTimer, Signal
from PySide6.QtWidgets import QSystemTrayIcon
from qfluentwidgets import CaptionLabel, MessageBoxBase, PushButton, SubtitleLabel, TextBrowser

from .. import __version__, update
from ..common import APP_NAME, LOG_INFO, LOG_WARN, RELEASES_URL, log
from .common import info


class ReleaseDialog(MessageBoxBase):
    """New version: release notes (Markdown) + install / later."""

    def __init__(self, release, action_text, parent):
        super().__init__(parent)
        self.viewLayout.addWidget(SubtitleLabel(_t("{APP_NAME} {version} is available").format(APP_NAME=APP_NAME, version=release["version"]), self))
        self.viewLayout.addWidget(CaptionLabel(_t("Installed version: {version}").format(version=__version__), self))
        self.notes = TextBrowser(self)
        self.notes.setOpenExternalLinks(True)
        self.notes.setMarkdown(release["notes"] or _t("No release notes."))
        self.notes.setMinimumSize(520, 260)
        self.viewLayout.addWidget(self.notes, 1)
        self.yesButton.setText(action_text)
        self.cancelButton.setText(_t("Later"))


class Updater(QObject):
    checked = Signal(object, str, bool)         # release or None, error ("" = ok), manual

    def __init__(self, ctl):
        super().__init__()
        self.ctl = ctl
        self.release = None             # newer release found by the last check
        self.status = ""                # shown on the About page
        self._checking = False
        self._tray_release = False      # the last tray message was about an update (click opens the dialog)
        self.checked.connect(self._on_checked)

    # --- checking ---
    def startup_check(self):
        if update.due(self.ctl.glob):
            self.check(manual=False)

    def check(self, manual=True):
        if self._checking:
            return
        self._checking = True
        self._set_status(_t("Checking for updates…"), busy=True)

        def work():
            try:
                rel, err = update.check(), ""
            except ValueError as e:
                rel, err = None, str(e)
            except Exception as e:      # never let a thread die silently
                rel, err = None, repr(e)
            self.checked.emit(rel, err, manual)
        threading.Thread(target=work, daemon=True).start()

    def _on_checked(self, rel, err, manual):
        self._checking = False
        self.ctl.glob["update_checked"] = int(time.time())
        self.ctl._persist()
        if err:
            self._set_status(_t("Update check failed"))
            log(LOG_WARN, _t("Update check failed: {e}").format(e=err))
            if manual:
                info(self.ctl.window, "error", _t("Update check failed"), err, 5000)
            return
        self.release = rel
        if rel is None:
            self._set_status(_t("This is the latest version"))
            if manual:
                info(self.ctl.window, "success", _t("No updates"), _t("{APP_NAME} {version} is the latest version").format(APP_NAME=APP_NAME, version=__version__))
            return
        text = _t("Version {version} is available").format(version=rel["version"])
        self._set_status(text)
        log(LOG_INFO, text)
        if manual:
            self.show_dialog()
            return
        bar = info(self.ctl.window, "info", _t("Update available"), text, -1)
        btn = PushButton(_t("Details"), bar)
        btn.clicked.connect(lambda: (bar.close(), self.show_dialog()))
        bar.addWidget(btn)
        if self.ctl.has_tray:
            self._tray_release = True
            self.ctl.tray.showMessage(APP_NAME, text, QSystemTrayIcon.Information, 8000)
            QTimer.singleShot(15000, lambda: setattr(self, "_tray_release", False))

    def on_tray_message_clicked(self):
        if self._tray_release and self.release is not None:
            self._tray_release = False
            self.ctl.window.show_window()
            QTimer.singleShot(300, self.show_dialog)

    def _set_status(self, text, busy=False):
        self.status = text
        page = getattr(self.ctl.window, "about_page", None) if hasattr(self.ctl, "window") else None
        if page is not None:
            page.set_update_status(text, busy)

    # --- the dialog ---
    def show_dialog(self):
        rel = self.release
        if rel is None:
            return
        dlg = ReleaseDialog(rel, _t("Open the release page"), self.ctl.window)
        if dlg.exec():
            from .about import open_url
            open_url(rel["url"] or RELEASES_URL)
