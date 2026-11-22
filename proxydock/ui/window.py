"""Main window, controller (state and actions), tray, single instance, entry point."""
from ..i18n import _t
import argparse
import copy
import json
import logging
import logging.handlers
import os
import subprocess
import sys
import threading
import time

from PySide6.QtCore import (
    QByteArray, QEasingCurve, QObject, QPropertyAnimation, Qt, QTimer, Signal,
)
from PySide6.QtNetwork import QLocalServer, QLocalSocket
from PySide6.QtWidgets import QApplication, QGraphicsOpacityEffect, QLabel, QSystemTrayIcon
from qfluentwidgets import (
    Action, FluentIcon as FIF, FluentWindow, NavigationItemPosition, PushButton, SystemThemeListener, SystemTrayMenu,
)

from .. import common, config
from ..common import (
    APP_ID, APP_NAME, IS_WIN, LOG_ERROR, LOG_INFO, LOG_SETTING, LOG_WARN, Stats, fmt_rate, format_line, log,
)
from ..platform_utils import (
    firewall_apply, firewall_rule_exists, is_startup_hidden, is_startup_registered, migrate_autostart,
    update_startup_registry,
)
from ..runtime import ProxyManager
from .common import Fonts, STATE_TEXT, apply_theme, info, load_fonts, make_icon
from .conntest import ConnTestInterface
from .help import HelpInterface
from .license import LicenseInterface
from .logs import LogInterface
from .proxies import EditorInterface, ProxiesInterface
from .settings import SettingsInterface
from .stats import StatsInterface


class MainWindow(FluentWindow):
    visibilityChanged = Signal(bool)
    FADE_IN_MS = 260
    FADE_OUT_MS = 150

    def __init__(self, ctl):
        super().__init__()
        self.ctl = ctl
        self.setWindowTitle(APP_NAME)
        self.setWindowIcon(make_icon())
        screen = QApplication.primaryScreen().availableGeometry()
        w = min(1280, int(screen.width() * 0.72))
        h = w * 9 // 16
        if h > screen.height() * 0.88:
            h = int(screen.height() * 0.88)
            w = h * 16 // 9
        self.resize(w, h)
        self.move(screen.center().x() - self.width() // 2, screen.center().y() - self.height() // 2)

        g = ctl.glob
        self.proxies_page = ProxiesInterface(ctl, self)
        self.editor = EditorInterface(ctl, self)
        self.log_page = LogInterface(g, self)
        self.stats_page = StatsInterface(self)
        self.test_page = ConnTestInterface(ctl, self)
        self.help_page = HelpInterface(ctl, self)
        self.license_page = LicenseInterface(g, self)
        self.settings_page = SettingsInterface(ctl, self)
        self.addSubInterface(self.proxies_page, FIF.TILES, _t("Proxy"))
        self.addSubInterface(self.log_page, FIF.COMMAND_PROMPT, _t("Logs"))
        self.addSubInterface(self.stats_page, FIF.MARKET, _t("Statistics"))
        self.addSubInterface(self.test_page, FIF.SPEED_HIGH, _t("Connection test"))
        self.addSubInterface(self.help_page, FIF.HELP, _t("How to use"), NavigationItemPosition.BOTTOM)
        self.addSubInterface(self.license_page, FIF.CERTIFICATE, _t("Licenses"), NavigationItemPosition.BOTTOM)
        self.addSubInterface(self.settings_page, FIF.SETTING, _t("Settings"), NavigationItemPosition.BOTTOM)
        self.stackedWidget.addWidget(self.editor)       # editor page that is not in the navigation menu
        self.editor.closed.connect(lambda: self.switchTo(self.proxies_page))
        self.navigationInterface.setExpandWidth(180)
        self._install_fade()
        self.restore_state()

    # --- remember window size/position ---
    def restore_state(self):
        try:
            with open(config.WINDOW_STATE_PATH, encoding="utf-8") as f:
                data = json.load(f)
            geo = QByteArray.fromBase64(data["geometry"].encode("ascii"))
        except (OSError, ValueError, KeyError, TypeError, AttributeError):
            return
        if not self.restoreGeometry(geo):
            return
        if QApplication.screenAt(self.frameGeometry().center()) is None:
            avail = QApplication.primaryScreen().availableGeometry()
            self.move(avail.center().x() - self.width() // 2, avail.center().y() - self.height() // 2)

    def save_state(self):
        try:
            os.makedirs(config.APP_DIR, exist_ok=True)
            tmp = config.WINDOW_STATE_PATH + ".tmp"
            with open(tmp, "w", encoding="utf-8") as f:
                json.dump({"geometry": bytes(self.saveGeometry().toBase64()).decode("ascii")}, f)
            os.replace(tmp, config.WINDOW_STATE_PATH)
        except OSError:
            pass

    def hideEvent(self, event):
        super().hideEvent(event)
        if not event.spontaneous():
            self.save_state()
            self.visibilityChanged.emit(False)

    def showEvent(self, event):
        super().showEvent(event)
        self.visibilityChanged.emit(True)

    # --- page transitions: slide + fade ---
    def _install_fade(self):
        sw = self.stackedWidget
        view = sw.view
        self._snap = QLabel(view)
        self._snap.setAttribute(Qt.WA_TransparentForMouseEvents)
        self._snap.setAttribute(Qt.WA_TranslucentBackground)
        self._snap.hide()
        self._snap_eff = QGraphicsOpacityEffect(self._snap)
        self._snap.setGraphicsEffect(self._snap_eff)
        self._snap_ani = QPropertyAnimation(self._snap_eff, b"opacity", self)
        self._snap_ani.setEasingCurve(QEasingCurve.OutQuad)
        self._snap_ani.finished.connect(self._snap.hide)
        self._fading = {}
        self._fade_in_after_pop = None
        view.aniFinished.connect(self._on_pop_finished)
        orig = sw.setCurrentWidget

        def set_current(widget, popOut=True):
            cur = sw.currentWidget()
            if cur is None or cur is widget or not sw.isAnimationEnabled() or not self.isVisible():
                return orig(widget, popOut)
            for wd in list(self._fading):
                self._finish_fade(wd)
            if popOut:
                self._fade(cur, 1.0, 0.0, 300, QEasingCurve.InQuad)
                self._fade_in_after_pop = widget
                orig(widget, popOut)
            else:
                self._snap.setPixmap(cur.grab())
                self._snap.setGeometry(cur.geometry())
                self._snap_eff.setOpacity(1.0)
                self._snap.show()
                self._snap.raise_()
                self._fade(widget, 0.0, 1.0, self.FADE_IN_MS, QEasingCurve.OutCubic)
                orig(widget, popOut)
                self._snap.raise_()
                self._snap_ani.stop()
                self._snap_ani.setDuration(self.FADE_OUT_MS)
                self._snap_ani.setStartValue(1.0)
                self._snap_ani.setEndValue(0.0)
                self._snap_ani.start()
        sw.setCurrentWidget = set_current

    def _fade(self, widget, start, end, ms, curve):
        eff = QGraphicsOpacityEffect(widget)
        eff.setOpacity(start)
        widget.setGraphicsEffect(eff)
        ani = QPropertyAnimation(eff, b"opacity", self)
        ani.setDuration(ms)
        ani.setStartValue(start)
        ani.setEndValue(end)
        ani.setEasingCurve(curve)
        ani.finished.connect(lambda: self._finish_fade(widget))
        self._fading[widget] = ani
        ani.start()

    def _finish_fade(self, widget):
        ani = self._fading.pop(widget, None)
        if ani is not None:
            ani.stop()
            ani.deleteLater()
        widget.setGraphicsEffect(None)

    def _on_pop_finished(self):
        w, self._fade_in_after_pop = self._fade_in_after_pop, None
        for wd in list(self._fading):
            self._finish_fade(wd)
        if w is not None:
            self._fade(w, 0.0, 1.0, self.FADE_IN_MS, QEasingCurve.OutCubic)

    def show_window(self):
        if self.isMinimized():
            self.showNormal()
        else:
            self.show()
        self.raise_()
        self.activateWindow()

    def closeEvent(self, event):
        event.ignore()
        if self.ctl.glob["close_exits"] or not self.ctl.has_tray:
            self.ctl.exit()
        else:
            self.hide()     # default: X only hides the window; proxies and the tray keep running


class _Bridge(QObject):
    status = Signal(str, str, str)      # proxy id, state, message (loop thread -> UI)
    notify = Signal(str)
    firewall = Signal(object, str)


class Controller:
    def __init__(self, server, show_start_notice, glob, proxies):
        self.server = server
        self.glob = glob
        self.proxies = proxies
        self.sig = _Bridge()
        self.sig.status.connect(self._on_status)
        self.sig.notify.connect(self.on_notify)
        self.sig.firewall.connect(self._on_firewall)
        common.notify_hook = self.sig.notify.emit
        self.manager = ProxyManager(status_hook=lambda pid, st, msg: self.sig.status.emit(pid, st, msg))
        server.newConnection.connect(self._on_second_instance)

        self.file_logger = logging.getLogger("proxydock")
        self.file_logger.setLevel(logging.INFO)
        self.file_logger.propagate = False
        self.file_handler = None

        self.has_tray = QSystemTrayIcon.isSystemTrayAvailable()
        self.window = MainWindow(self)
        self.tray = QSystemTrayIcon(make_icon(), self.window)
        self.tray.setToolTip(APP_NAME)
        self.menu = SystemTrayMenu(parent=self.window)
        self.act_show = Action(FIF.VIEW, _t("Open window"), self.menu)
        self.act_stop_all = Action(FIF.PAUSE, _t("Stop all proxies"), self.menu)
        self.act_startup = Action(FIF.POWER_BUTTON, _t("Start at login"), self.menu)
        self.act_hidden = Action(FIF.INFO, _t("Hide startup notification"), self.menu)
        self.act_restart = Action(FIF.SYNC, _t("Restart"), self.menu)
        self.act_exit = Action(FIF.CLOSE, _t("Quit"), self.menu)
        for act in (self.act_startup, self.act_hidden):
            act.setCheckable(True)
        self.act_show.triggered.connect(self.toggle_window)
        self.window.visibilityChanged.connect(self._sync_show_action)
        self.act_stop_all.triggered.connect(self.stop_all)
        self.act_startup.triggered.connect(self.set_startup)
        self.act_hidden.triggered.connect(self.set_hidden)
        self.act_restart.triggered.connect(self.restart)
        self.act_exit.triggered.connect(self.exit)
        self.proxy_actions = []
        self.tray.setContextMenu(self.menu)
        self.tray.activated.connect(self._on_tray_activated)

        self.theme_listener = SystemThemeListener(self.window)
        self.theme_listener.systemThemeChanged.connect(
            lambda: self.glob["theme"] == "auto" and apply_theme("auto"))
        self.theme_listener.start()
        self._fw_busy = False
        self.refresh_firewall()
        self._apply_file_logging(glob["log_to_file"])

        self.log_timer = QTimer()
        self.log_timer.setInterval(150)
        self.log_timer.timeout.connect(self.drain_logs)
        self.log_timer.start()
        self._last = {}
        self._last_t = time.monotonic()
        self.stats_timer = QTimer()
        self.stats_timer.setInterval(1000)
        self.stats_timer.timeout.connect(self._tick)
        self.stats_timer.start()

        self.refresh_lists()
        self.refresh_states()
        if self.has_tray:
            self.tray.show()
        else:
            log(LOG_WARN, _t("The system tray is not available, so closing the window quits the program"))
            QTimer.singleShot(0, self.window.show_window)
        auto = [p for p in self.proxies if p["autostart"]]
        for p in auto:
            self.manager.start(p, self.glob)
        if show_start_notice:
            QTimer.singleShot(800, lambda: self.tray.showMessage(
                APP_NAME, _t("Running · {len} autostart proxies").format(len=len(auto)), QSystemTrayIcon.Information, 3000))

    # --- proxy list ---
    def proxy_by_id(self, pid):
        return next((p for p in self.proxies if p["id"] == pid), None)

    def statuses(self):
        return {p["id"]: self.manager.state(p["id"]) for p in self.proxies}

    def running_ids(self):
        return {pid for pid, (st, _) in self.statuses().items() if st == "running"}

    def refresh_lists(self):
        w = self.window
        w.proxies_page.refresh(self.proxies, self.statuses())
        w.log_page.set_proxies(self.proxies)
        w.stats_page.set_proxies(self.proxies)
        w.test_page.set_proxies(self.proxies, self.running_ids())
        self._rebuild_tray_proxies()

    def _persist(self):
        if not config.save(self.glob, self.proxies):
            info(self.window, "error", _t("Save failed"), _t("Could not save the settings file. Check the log."))

    def _unique_name(self, base):
        names = {p["name"] for p in self.proxies}
        if base not in names:
            return base
        i = 2
        while f"{base} {i}" in names:
            i += 1
        return f"{base} {i}"

    def _free_port(self, start):
        used = {p["inbound"]["port"] for p in self.proxies}
        port = start
        while port in used:
            port += 1
        return port

    def new_proxy(self, name, inbound, outbound):
        ib = dict(inbound)
        ib["port"] = self._free_port(ib.get("port", 1080))
        p = config.new_proxy(self._unique_name(name), ib, outbound)
        self.window.editor.load(p, is_new=True)
        self.window.switchTo(self.window.editor)

    def edit_proxy(self, pid):
        p = self.proxy_by_id(pid)
        if p is not None:
            self.window.editor.load(p, is_new=False)
            self.window.switchTo(self.window.editor)

    def save_proxy(self, p, is_new):
        if is_new:
            self.proxies.append(p)
            log(LOG_SETTING, _t("Created proxy '{name}'").format(name=p['name']))
        else:
            self.proxies = [p if x["id"] == p["id"] else x for x in self.proxies]
            log(LOG_SETTING, _t("Saved proxy '{name}'").format(name=p['name']))
            self.manager.apply(p, self.glob)
        self._persist()
        self.refresh_lists()

    def duplicate_proxy(self, pid):
        src = self.proxy_by_id(pid)
        if src is None:
            return
        p = copy.deepcopy(src)
        p["inbound"]["port"] = self._free_port(p["inbound"]["port"] + 1)
        p = config.new_proxy(self._unique_name(src["name"] + _t(" copy")), p["inbound"], p["outbound"])
        p["overrides"] = copy.deepcopy(src["overrides"])
        self.window.editor.load(p, is_new=True)
        self.window.switchTo(self.window.editor)

    def delete_proxy(self, pid):
        p = self.proxy_by_id(pid)
        if p is None:
            return
        self.manager.stop(pid)
        self.proxies = [x for x in self.proxies if x["id"] != pid]
        self._persist()
        log(LOG_SETTING, _t("Deleted proxy '{name}'").format(name=p['name']))
        self.refresh_lists()

    def toggle_proxy(self, pid, on):
        p = self.proxy_by_id(pid)
        if p is None:
            return
        if on:
            errs = config.check_proxy(p, [x for x in self.proxies if x["id"] != pid])
            if errs:
                info(self.window, "error", _t("Cannot start '{name}'").format(name=p['name']), "\n".join(errs[:3]), 5000)
                self.window.proxies_page.set_status(pid, self.manager.state(pid), p)
                return
            self.manager.start(p, self.glob)
        else:
            self.manager.stop(pid)

    def stop_all(self):
        for p in self.proxies:
            if self.manager.state(p["id"])[0] in ("running", "starting"):
                self.manager.stop(p["id"])

    def _on_status(self, pid, state, msg):
        if pid.startswith("__"):
            return      # temporary proxy used by the connection test
        p = self.proxy_by_id(pid)
        self.window.proxies_page.set_status(pid, (state, msg), p)
        self.window.test_page.set_proxies(self.proxies, self.running_ids())
        self._rebuild_tray_proxies()
        if state == "error" and p is not None:
            self.on_notify(_t("'{name}' failed to start: {msg}").format(name=p['name'], msg=msg))

    # --- global settings ---
    def apply_global(self, g):
        old = self.glob
        self.glob = g
        self._persist()
        self.manager.apply_global(g, self.proxies)
        self._apply_file_logging(g["log_to_file"])
        self.window.log_page.apply_font(g)
        from .common import make_mono_font
        self.window.license_page.view.setFont(make_mono_font(g))
        if g["theme"] != Fonts.theme:
            apply_theme(g["theme"])
        pending = []
        if g["ui_font"] != Fonts.ui_choice:
            pending.append(_t("UI font"))
        from ..i18n import current as current_language
        if g["language"] != current_language():
            pending.append(_t("Language"))
        if g["ui_scale"] != Fonts.startup_scale:
            pending.append(_t("App scale"))
        log(LOG_SETTING, _t("Settings saved") + (_t(" (applies after restart: {join})").format(join=', '.join(pending)) if pending else ""))
        if pending:
            bar = info(self.window, "warning", _t("Restart required"), _t("Applies after a restart: {join}").format(join=', '.join(pending)), -1)
            btn = PushButton(_t("Restart now"), bar, FIF.SYNC)
            btn.clicked.connect(self.restart)
            bar.addWidget(btn)
        if self.window.editor.isVisible():
            pass
        _ = old

    # --- logs / statistics ---
    def drain_logs(self):
        q = common.pending_logs
        if not q:
            return
        batch = []
        try:
            while True:
                batch.append(q.popleft())
        except IndexError:
            pass
        if self.file_handler is not None:
            for e in batch:
                self.file_logger.info(format_line(e))
        self.window.log_page.add_entries(batch)

    def _apply_file_logging(self, enabled):
        if enabled and self.file_handler is None:
            try:
                os.makedirs(config.LOG_DIR, exist_ok=True)
                h = logging.handlers.RotatingFileHandler(
                    os.path.join(config.LOG_DIR, "proxydock.log"), maxBytes=2 * 1024 * 1024,
                    backupCount=3, encoding="utf-8", delay=True)
            except OSError as e:
                log(LOG_ERROR, _t("Cannot open the log file: {e}").format(e=e))
                return
            h.setFormatter(logging.Formatter("%(message)s"))
            self.file_logger.addHandler(h)
            self.file_handler = h
        elif not enabled and self.file_handler is not None:
            self.file_logger.removeHandler(self.file_handler)
            self.file_handler.close()
            self.file_handler = None

    def _tick(self):
        now = time.monotonic()
        dt = max(now - self._last_t, 1e-3)
        self._last_t = now
        snaps = {}
        items = [(pid, s) for pid, s in self.manager.stats.items() if not pid.startswith("__")]
        for pid, s in items:
            up0, down0 = self._last.get(pid, (s.up, s.down))
            snaps[pid] = (s, (s.up - up0) / dt, (s.down - down0) / dt)
            self._last[pid] = (s.up, s.down)
        total = Stats.sum(s for _, s in items)
        up = sum(v[1] for v in snaps.values())
        down = sum(v[2] for v in snaps.values())
        snaps[""] = (total, up, down)
        self.window.stats_page.push(snaps)
        running = len(self.running_ids())
        self.tray.setToolTip(_t("{APP_NAME}\n{running} proxies running\n↓ {fmt_rate}  ↑ {fmt_rate2}  · connections {value}").format(APP_NAME=APP_NAME, running=running, fmt_rate=fmt_rate(down), fmt_rate2=fmt_rate(up), value=total.tcp_active + total.udp_active))

    # --- tray ---
    def _rebuild_tray_proxies(self):
        m = self.menu
        m.clear()
        m.addAction(self.act_show)
        m.addSeparator()
        self.proxy_actions = []
        for p in self.proxies[:15]:
            st = self.manager.state(p["id"])[0]
            act = Action(FIF.PLAY if st != "running" else FIF.PAUSE,
                         f"{p['name']}  ·  {STATE_TEXT.get(st, st)}", m)
            act.triggered.connect(lambda _=False, pid=p["id"], s=st: self.toggle_proxy(pid, s != "running"))
            self.proxy_actions.append(act)
            m.addAction(act)
        if self.proxies:
            m.addAction(self.act_stop_all)
            m.addSeparator()
        m.addActions([self.act_startup, self.act_hidden])
        m.addSeparator()
        m.addActions([self.act_restart, self.act_exit])
        self._sync_show_action(self.window.isVisible())

    def toggle_window(self):
        if self.window.isVisible() and not self.window.isMinimized():
            self.window.hide()
        else:
            self.window.show_window()

    def _sync_show_action(self, visible):
        self.act_show.setText(_t("Close window") if visible else _t("Open window"))
        self.act_show.setIcon(FIF.HIDE.icon() if visible else FIF.VIEW.icon())

    def _on_tray_activated(self, reason):
        if reason in (QSystemTrayIcon.Trigger, QSystemTrayIcon.DoubleClick):
            if self.window.isVisible() and not self.window.isMinimized() and self.window.isActiveWindow():
                self.window.hide()
            else:
                self.window.show_window()

    def on_notify(self, message):
        if self.window.isVisible():
            info(self.window, "info", APP_NAME, message)
        elif self.has_tray:
            self.tray.showMessage(APP_NAME, message, QSystemTrayIcon.Information, 3000)

    # --- start at login ---
    def refresh_states(self):
        reg = is_startup_registered()
        hid = is_startup_hidden()
        self.act_startup.setChecked(reg)
        self.act_startup.setText(_t("Start at login  ·  {value}").format(value=_t("On") if reg else _t("Off")))
        self.act_hidden.setChecked(hid)
        self.act_hidden.setText(_t("Hide startup notification  ·  {value}").format(value=_t("On") if hid else _t("Off")))
        self.act_hidden.setEnabled(reg)
        self.window.settings_page.refresh_startup(reg, hid)

    def set_startup(self, checked):
        if update_startup_registry(bool(checked), hidden=is_startup_hidden()):
            log(LOG_SETTING, _t("Start at login") if checked else _t("Removed from startup"))
        self.refresh_states()

    def set_hidden(self, checked):
        if not is_startup_registered():
            self.refresh_states()
            return
        if update_startup_registry(True, hidden=bool(checked)):
            log(LOG_SETTING, _t("Hide startup notification {value}").format(value='ON' if checked else 'OFF'))
        self.refresh_states()

    # --- firewall (Windows) ---
    def _fw_set_ui(self, state):
        self.window.settings_page.set_firewall_status(state)
        self.window.help_page.set_firewall_status(state)

    def refresh_firewall(self):
        if not IS_WIN:
            return
        self._fw_set_ui(None)
        threading.Thread(target=lambda: self.sig.firewall.emit(firewall_rule_exists(), ""), daemon=True).start()

    def firewall(self, block):
        if self._fw_busy:
            return
        if not getattr(sys, "frozen", False):
            info(self.window, "warning", _t("Running as a script"),
                 _t("The rule would apply to all of {basename}. Use it when running the exe.").format(basename=os.path.basename(sys.executable)), 5000)
        self._fw_busy = True
        self._fw_set_ui(None)

        def work():
            ok, msg = firewall_apply(block)
            self.sig.firewall.emit(firewall_rule_exists(), msg if ok else "!" + msg)
        threading.Thread(target=work, daemon=True).start()

    def _on_firewall(self, exists, msg):
        self._fw_busy = False
        self._fw_set_ui(bool(exists))
        if msg.startswith("!"):
            info(self.window, "error", _t("Firewall change failed"), msg[1:], 4000)
        elif msg:
            log(LOG_SETTING, _t("Firewall: {msg}").format(msg=msg))

    # --- misc ---
    def _on_second_instance(self):
        conn = self.server.nextPendingConnection()
        if conn is not None:
            conn.disconnectFromServer()
        self.window.show_window()

    def restart(self):
        cmd = [sys.executable] if getattr(sys, "frozen", False) else [sys.executable, sys.argv[0]]
        cmd += [a for a in sys.argv[1:] if a != "--restart"] + ["--restart"]
        try:
            if IS_WIN:
                subprocess.Popen(cmd, close_fds=True, creationflags=0x00000200)
            else:
                subprocess.Popen(cmd, close_fds=True, start_new_session=True)
        except OSError as e:
            log(LOG_ERROR, _t("Restart failed: {e}").format(e=e))
            return
        self.exit()

    def exit(self):
        if self.window.isVisible():
            self.window.save_state()
        self.manager.stop_all(2)
        self.drain_logs()
        self.tray.hide()
        os._exit(0)


# --- single instance ---

SERVER_NAME = f"{APP_ID}-single-instance"


def become_primary(wait=False):
    """Returns a QLocalServer for the first instance. If already running, shows the existing window and returns None."""
    deadline = time.monotonic() + (10 if wait else 0)
    while True:
        sock = QLocalSocket()
        sock.connectToServer(SERVER_NAME)
        if sock.waitForConnected(300):
            if not wait:
                sock.write(b"show")
                sock.waitForBytesWritten(500)
                sock.disconnectFromServer()
                return None
            sock.abort()
        else:
            QLocalServer.removeServer(SERVER_NAME)
            server = QLocalServer()
            server.setSocketOptions(QLocalServer.UserAccessOption)
            if server.listen(SERVER_NAME):
                return server
        if time.monotonic() >= deadline:
            return None
        time.sleep(0.25)


def main():
    parser = argparse.ArgumentParser(prog="ProxyDock")
    parser.add_argument("--hidden", action="store_true", help=_t("Start in the tray without a notification"))
    parser.add_argument("--restart", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()

    glob, proxies = config.load()
    migrate_autostart()

    if IS_WIN:
        try:
            import ctypes
            ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID("ProxyDock.App")
        except Exception:
            pass
    Fonts.startup_scale = glob["ui_scale"]
    if Fonts.startup_scale != 100:
        os.environ["QT_SCALE_FACTOR"] = f"{Fonts.startup_scale / 100:g}"

    app = QApplication([sys.argv[0]])
    app.setApplicationName(APP_NAME)
    app.setQuitOnLastWindowClosed(False)
    # Built-in texts of Qt / Fluent widgets follow the app language
    from PySide6.QtCore import QLocale
    from qfluentwidgets import FluentTranslator
    from .. import i18n
    locale = QLocale(i18n.current())
    QLocale.setDefault(locale)
    app._fluent_translator = FluentTranslator(locale)
    app.installTranslator(app._fluent_translator)
    apply_theme(glob["theme"])
    load_fonts(app, glob)

    single = become_primary(wait=args.restart)
    if single is None:
        sys.exit(0)
    log(LOG_INFO, _t("{APP_NAME} started - {len} proxies").format(APP_NAME=APP_NAME, len=len(proxies)))
    controller = Controller(single, show_start_notice=not args.hidden, glob=glob, proxies=proxies)
    _ = controller
    sys.exit(app.exec())
