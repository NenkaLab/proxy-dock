"""Updates in the UI: startup check (once a day), the About page button, notifications, the release notes dialog,
and the in-app update (download with progress, SHA-256 check, install). Network work runs in threads; results
come back through Qt signals. The user is asked before downloading and again before installing."""
from ..i18n import _t
import os
import subprocess
import sys
import tempfile
import threading
import time

from PySide6.QtCore import QObject, QTimer, QUrl, Signal
from PySide6.QtGui import QDesktopServices
from PySide6.QtWidgets import QSystemTrayIcon
from qfluentwidgets import (
    BodyLabel, CaptionLabel, MessageBox, MessageBoxBase, ProgressBar, PushButton, SubtitleLabel, TextBrowser,
)

from .. import __version__, update
from ..common import APP_NAME, LOG_ERROR, LOG_INFO, LOG_WARN, RELEASES_URL, log
from .common import info


def fmt_size(n):
    return f"{n / (1024 * 1024):.1f} MB"


def open_url(url):
    QDesktopServices.openUrl(QUrl(url))


class ReleaseDialog(MessageBoxBase):
    """New version: release notes (Markdown) + download / later."""

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


class DownloadDialog(MessageBoxBase):
    """Download progress; Cancel stops the download."""

    def __init__(self, name, parent):
        super().__init__(parent)
        self.viewLayout.addWidget(SubtitleLabel(_t("Downloading the update"), self))
        self.viewLayout.addWidget(CaptionLabel(name, self))
        self.bar = ProgressBar(self)
        self.bar.setRange(0, 1000)
        self.bar.setMinimumWidth(420)
        self.viewLayout.addWidget(self.bar)
        self.label = BodyLabel(_t("Starting…"), self)
        self.viewLayout.addWidget(self.label)
        self.yesButton.hide()
        self.cancelButton.setText(_t("Cancel"))

    def set_progress(self, done, total):
        if total:
            self.bar.setValue(int(done * 1000 / total))
            self.label.setText(f"{fmt_size(done)} / {fmt_size(total)}")
        else:
            self.label.setText(fmt_size(done))


class Updater(QObject):
    checked = Signal(object, str, bool)         # release or None, error ("" = ok), manual
    progress = Signal(object, object)           # bytes done, total (0 = unknown)
    downloaded = Signal(str, str, str)          # file path ("" = failed or cancelled), error, kind

    def __init__(self, ctl):
        super().__init__()
        self.ctl = ctl
        self.release = None             # newer release found by the last check
        self.status = ""                # shown on the About page
        self.install_info = update.current_install      # -> (os name, arch, kind); tests replace it
        self._checking = False
        self._downloading = False
        self._cancel = threading.Event()
        self._dialog = None
        self._tray_release = False      # the last tray message was about an update (a click opens the dialog)
        self.checked.connect(self._on_checked)
        self.progress.connect(self._on_progress)
        self.downloaded.connect(self._on_downloaded)

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
            except Exception as e:      # never let the thread die silently
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
                info(self.ctl.window, "success", _t("No updates"),
                     _t("{APP_NAME} {version} is the latest version").format(APP_NAME=APP_NAME, version=__version__))
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

    # --- release notes, then download ---
    def asset_for(self, rel):
        """-> (file name, kind) to install on this computer, or (None, None) when only the release page helps."""
        os_name, arch, kind = self.install_info()
        name = update.pick_asset(rel, os_name, arch, kind) if kind else None
        return (name, kind) if name else (None, None)

    def show_dialog(self):
        rel = self.release
        if rel is None or self._downloading:
            return
        name, kind = self.asset_for(rel)
        if name:
            size = rel["assets"][name].get("size", 0)
            action = _t("Download and install") + (f" ({fmt_size(size)})" if size else "")
        else:
            action = _t("Open the release page")
        if not ReleaseDialog(rel, action, self.ctl.window).exec():
            return
        if name:
            self.start_download(rel, name, kind)
        else:
            open_url(rel["url"] or RELEASES_URL)

    def start_download(self, rel, name, kind):
        if self._downloading:
            return
        self._downloading = True
        self._cancel.clear()
        folder = tempfile.mkdtemp(prefix="proxydock-update-")
        dlg = self._dialog = DownloadDialog(name, self.ctl.window)
        dlg.rejected.connect(self._cancel.set)
        dlg.open()
        log(LOG_INFO, _t("Downloading {name}").format(name=name))

        def work():
            try:
                path = update.fetch_verified(rel, name, folder, lambda d, t: self.progress.emit(d, t), self._cancel.is_set)
                self.downloaded.emit(path, "", kind)
            except update.Cancelled:
                self.downloaded.emit("", "", kind)
            except ValueError as e:
                self.downloaded.emit("", str(e), kind)
            except Exception as e:
                self.downloaded.emit("", repr(e), kind)
        threading.Thread(target=work, daemon=True).start()

    def _on_progress(self, done, total):
        if self._dialog is not None:
            try:
                self._dialog.set_progress(done, total)
            except RuntimeError:        # dialog already deleted (window rebuilt)
                self._dialog = None

    def _on_downloaded(self, path, err, kind):
        self._downloading = False
        dlg, self._dialog = self._dialog, None
        if dlg is not None:
            try:
                if dlg.isVisible():
                    dlg.accept()
            except RuntimeError:
                pass
        if not path:
            if err:
                log(LOG_ERROR, _t("Update download failed: {e}").format(e=err))
                info(self.ctl.window, "error", _t("Update download failed"), err, -1)
            else:
                log(LOG_INFO, _t("Update download cancelled"))
            return
        log(LOG_INFO, _t("Downloaded and verified (SHA-256): {path}").format(path=path))
        if self.ask_install(kind):
            self.install(path, kind)

    # --- install ---
    def ask_install(self, kind):
        text = {
            "installer": _t("ProxyDock closes, the installer updates it and then starts it again. Running proxies stop until then."),
            "dmg": _t("The disk image opens. Quit ProxyDock, then drag ProxyDock into Applications to replace the old version."),
            "appimage": _t("The AppImage file is replaced with the new version and ProxyDock restarts. Running proxies stop until then."),
        }.get(kind, "")
        box = MessageBox(_t("Install the update?"), text, self.ctl.window)
        box.yesButton.setText(_t("Install"))
        box.cancelButton.setText(_t("Later"))
        return bool(box.exec())

    def install(self, path, kind):
        try:
            if kind == "installer":
                from .. import i18n
                cmd = update.installer_command(path, os.path.dirname(sys.executable), i18n.current())
                log(LOG_INFO, _t("Starting the installer: {path}").format(path=path))
                subprocess.Popen(cmd, close_fds=True, creationflags=0x00000008 | 0x00000200)   # detached, own group
                self.ctl.exit()
            elif kind == "dmg":
                if not QDesktopServices.openUrl(QUrl.fromLocalFile(path)):
                    raise OSError(_t("Cannot open: {path}").format(path=path))
            elif kind == "appimage":
                update.replace_appimage(path, os.environ["APPIMAGE"])
                log(LOG_INFO, _t("Updated {path}").format(path=os.environ["APPIMAGE"]))
                self.ctl.restart()
            else:
                open_url(RELEASES_URL)
        except (OSError, KeyError) as e:
            log(LOG_ERROR, _t("Update failed: {e}").format(e=e))
            info(self.ctl.window, "error", _t("Update failed"), str(e), -1)
