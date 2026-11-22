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
    QByteArray, QEasingCurve, QEvent, QObject, Qt, QTimer, QVariantAnimation, Signal,
)
from PySide6.QtGui import QKeySequence, QPainter, QShortcut
from PySide6.QtNetwork import QLocalServer, QLocalSocket
from PySide6.QtWidgets import QApplication, QGraphicsEffect, QGraphicsOpacityEffect, QSystemTrayIcon, QWidget
from qfluentwidgets import (
    Action, FluentIcon as FIF, FluentWindow, NavigationItemPosition, PushButton, SystemThemeListener, SystemTrayMenu,
)

from .. import common, config
from .. import graph as graph_mod
from ..common import (
    APP_ID, APP_NAME, IS_MAC, IS_WIN, LOG_ERROR, LOG_INFO, LOG_SETTING, LOG_WARN, Stats, fmt_rate, format_line, log,
)
from ..platform_utils import (
    firewall_apply, firewall_rule_exists, get_system_proxy, is_startup_hidden, is_startup_registered,
    migrate_autostart, proxy_values_for, set_system_proxy, system_proxy_server, system_proxy_supported,
    update_startup_registry,
)
from ..runtime import ProxyManager
from .common import Fonts, STATE_TEXT, apply_theme, info, load_fonts, make_icon
from .conntest import ConnTestInterface
from .help import HelpInterface
from .license import LicenseInterface
from .logs import LogInterface
from .canvas import CanvasInterface
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
        self.canvas_page = CanvasInterface(ctl, self)
        self.proxies_page = ProxiesInterface(ctl, self)
        self.editor = EditorInterface(ctl, self)
        self.log_page = LogInterface(g, self)
        self.stats_page = StatsInterface(self)
        self.test_page = ConnTestInterface(ctl, self)
        self.help_page = HelpInterface(ctl, self)
        self.license_page = LicenseInterface(g, self)
        self.settings_page = SettingsInterface(ctl, self)
        self.addSubInterface(self.canvas_page, FIF.IOT, _t("Canvas"))
        self.addSubInterface(self.proxies_page, FIF.TILES, _t("Proxy"))
        self.addSubInterface(self.log_page, FIF.COMMAND_PROMPT, _t("Logs"))
        self.addSubInterface(self.stats_page, FIF.MARKET, _t("Statistics"))
        self.addSubInterface(self.test_page, FIF.SPEED_HIGH, _t("Connection test"))
        self.addSubInterface(self.help_page, FIF.HELP, _t("How to use"), NavigationItemPosition.BOTTOM)
        self.addSubInterface(self.license_page, FIF.CERTIFICATE, _t("Licenses"), NavigationItemPosition.BOTTOM)
        self.addSubInterface(self.settings_page, FIF.SETTING, _t("Settings"), NavigationItemPosition.BOTTOM)
        self.stackedWidget.addWidget(self.editor)       # editor page that is not in the navigation menu
        self.editor.closed.connect(lambda: self.switchTo(self.editor_return or self.canvas_page))
        self.editor_return = None
        self.navigationInterface.setExpandWidth(180)
        self._install_fade()
        self._install_history()
        self._warmed = False
        self.restore_state()

    # --- back / forward between pages (mouse buttons, Alt+Left/Right, the window's back button) ---
    def _install_history(self):
        self.nav_back, self.nav_forward = [], []
        self._nav_current = self.stackedWidget.currentWidget()
        self._nav_jumping = False
        self.stackedWidget.currentChanged.connect(self._on_page_changed)
        panel = self.navigationInterface.panel
        try:
            panel.returnButton.clicked.disconnect()
            panel.history.emptyChanged.disconnect(panel.returnButton.setDisabled)
        except (RuntimeError, TypeError, AttributeError):
            pass
        panel.returnButton.clicked.connect(self.go_back)
        self._back_button = panel.returnButton
        QShortcut(QKeySequence(QKeySequence.Back), self, activated=self.go_back)
        QShortcut(QKeySequence(QKeySequence.Forward), self, activated=self.go_forward)
        self._mouse_nav = _MouseNavFilter(self)
        QApplication.instance().installEventFilter(self._mouse_nav)
        self._sync_back_button()

    def _on_page_changed(self, _index):
        new = self.stackedWidget.currentWidget()
        old, self._nav_current = self._nav_current, new
        if self._nav_jumping or old is None or old is new:
            return
        if old is self.editor:              # leaving the editor returns to where it was opened from
            if self.nav_back and self.nav_back[-1] is new:
                self.nav_back.pop()
        else:
            self.nav_back.append(old)
            del self.nav_back[:-30]
        self.nav_forward.clear()
        self._sync_back_button()

    def _jump(self, widget):
        self._nav_jumping = True
        try:
            self.switchTo(widget)
            self._nav_current = widget
        finally:
            self._nav_jumping = False
        self._sync_back_button()

    def go_back(self):
        cur = self.stackedWidget.currentWidget()
        if cur is self.editor:              # back in the editor = cancel
            self.editor.cancel()
            return
        if not self.nav_back:
            return
        self.nav_forward.append(cur)
        self._jump(self.nav_back.pop())

    def go_forward(self):
        if not self.nav_forward:
            return
        cur = self.stackedWidget.currentWidget()
        if cur is not self.editor:
            self.nav_back.append(cur)
        self._jump(self.nav_forward.pop())

    def _sync_back_button(self):
        self._back_button.setDisabled(not self.nav_back and self.stackedWidget.currentWidget() is not self.editor)

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
        if not self._warmed:
            self._warmed = True
            self._warm_pages()

    # --- page transitions: slide + fade ---
    # The new page is drawn by a graphics effect that offsets (sub-pixel) and fades it in. The widget itself
    # never moves, so layout updates during the animation cannot make it jump, and no snapshot of the old
    # page is needed (snapshots of the translucent Mica pages came out with a white background).
    TRANSITION_MS = 260
    SLIDE_PX = 28

    def _install_fade(self):
        sw = self.stackedWidget
        sw.setAnimationEnabled(False)       # the built-in slide moves the live widget
        self._transition = None
        orig = sw.setCurrentWidget

        def set_current(widget, popOut=False):
            cur = sw.currentWidget()
            orig(widget, False)
            if cur is not None and cur is not widget and self.isVisible():
                self._animate_in(widget)
        sw.setCurrentWidget = set_current

    def _animate_in(self, widget):
        self._finish_transition()
        # macOS: a Python draw() override crashes there (invalid QPainter wrapper), so fade with Qt's own effect
        effect = _FadeEffect(widget) if IS_MAC else _SlideFadeEffect(widget)
        widget.setGraphicsEffect(effect)
        ani = QVariantAnimation(self)
        ani.setDuration(self.TRANSITION_MS)
        ani.setStartValue(0.0)
        ani.setEndValue(1.0)
        ani.setEasingCurve(QEasingCurve.OutCubic)
        ani.valueChanged.connect(lambda t: effect.set_progress(t, self.SLIDE_PX))
        ani.finished.connect(self._finish_transition)
        self._transition = (widget, ani)
        effect.set_progress(0.0, self.SLIDE_PX)
        ani.start()

    def _finish_transition(self):
        tr, self._transition = self._transition, None
        if tr is None:
            return
        widget, ani = tr
        ani.stop()
        ani.deleteLater()
        try:
            effect = widget.graphicsEffect()
        except RuntimeError:        # widget already deleted
            return
        if isinstance(effect, (_SlideFadeEffect, _FadeEffect)):
            effect.set_progress(1.0, 0)
            # remove it on the next loop turn: deleting an effect inside a paint pass crashes on macOS

            def drop(w=widget, e=effect):
                try:
                    if w.graphicsEffect() is e:
                        w.setGraphicsEffect(None)
                except RuntimeError:
                    pass
            QTimer.singleShot(0, widget, drop)

    def _warm_pages(self):
        """Lay out and render each page once while idle, so its first opening does not drop frames."""
        pages = [w for w in (self.canvas_page, self.proxies_page, self.log_page, self.stats_page, self.test_page,
                             self.help_page, self.license_page, self.settings_page, self.editor)
                 if w is not self.stackedWidget.currentWidget()]

        def step():
            if not pages or not self.isVisible():
                return
            if self._transition is not None:        # never render while a page is animating
                QTimer.singleShot(120, self, step)
                return
            page = pages.pop(0)
            if page.isVisible():
                QTimer.singleShot(30, self, step)
                return
            try:
                page.resize(self.stackedWidget.view.size())
                page.grab()
            except RuntimeError:
                return
            QTimer.singleShot(30, self, step)       # tied to the window: cancelled if it is deleted
        QTimer.singleShot(400, self, step)

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


class _SlideFadeEffect(QGraphicsEffect):
    """Draws the widget moved down by dy (fractional) with the given opacity."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.opacity = 0.0
        self.dy = 0.0

    def set_progress(self, t, distance):
        self.opacity = t
        self.dy = (1.0 - t) * distance
        self.update()

    def draw(self, painter):
        # PySide6 returns only the pixmap; with NoPad it covers sourceBoundingRect exactly
        pixmap = self.sourcePixmap(Qt.LogicalCoordinates, mode=QGraphicsEffect.NoPad)
        origin = self.sourceBoundingRect(Qt.LogicalCoordinates).topLeft()
        painter.save()
        painter.setOpacity(self.opacity)
        painter.setRenderHint(QPainter.SmoothPixmapTransform)
        painter.translate(0, self.dy)
        painter.drawPixmap(origin, pixmap)
        painter.restore()


class _ThemeListener(SystemThemeListener):
    """System theme watcher; quietly stops where the OS offers no theme notifications (e.g. Windows Server)."""

    def run(self):
        try:
            super().run()
        except Exception:
            pass


class _FadeEffect(QGraphicsOpacityEffect):
    """Fade only, drawn by Qt in C++ (used on macOS)."""

    def set_progress(self, t, distance):
        self.setOpacity(t)


class _MouseNavFilter(QObject):
    """Mouse back/forward (X1/X2) buttons anywhere in the main window."""

    def __init__(self, window):
        super().__init__(window)
        self.window = window

    def eventFilter(self, obj, event):
        if event.type() == QEvent.MouseButtonPress and event.button() in (Qt.BackButton, Qt.ForwardButton):
            w = obj.window() if isinstance(obj, QWidget) else None
            if w is self.window:
                (self.window.go_back if event.button() == Qt.BackButton else self.window.go_forward)()
                return True
        return False


class _Bridge(QObject):
    status = Signal(str, str, str)      # proxy id, state, message (loop thread -> UI)
    notify = Signal(str)
    firewall = Signal(object, str)


class Controller:
    def __init__(self, server, show_start_notice, glob, graph):
        self.server = server
        self.glob = glob
        self.graph = graph
        self.proxies = graph_mod.compile(graph)
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
        self.tray = QSystemTrayIcon(make_icon())
        self.tray.setToolTip(APP_NAME)
        self.menu = SystemTrayMenu()
        self.act_show = Action(FIF.VIEW, "", self.menu)
        self.act_stop_all = Action(FIF.PAUSE, "", self.menu)
        self.act_startup = Action(FIF.POWER_BUTTON, "", self.menu)
        self.act_hidden = Action(FIF.INFO, "", self.menu)
        self.act_restart = Action(FIF.SYNC, "", self.menu)
        self.act_exit = Action(FIF.CLOSE, "", self.menu)
        self._retranslate_tray()
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

        self.theme_listener = _ThemeListener()
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
        if self.graph["pc"]["applied"]:      # left applied by a crash: put the PC back first
            try:
                self._pc_restore_backup()
            except OSError:
                pass
        auto = [p for p in self.proxies if p["autostart"]]
        for p in auto:
            errs = config.check_proxy(p, [x for x in self.proxies if x["id"] != p["id"]])
            if errs:
                log(LOG_WARN, _t("[{name}] failed to start: {msg}").format(name=p["name"], msg=errs[0]))
                continue
            self.manager.start(p, self.glob)
        if show_start_notice:
            QTimer.singleShot(800, lambda: self.tray.showMessage(
                APP_NAME, _t("Running · {len} autostart proxies").format(len=len(auto)), QSystemTrayIcon.Information, 3000))

    # --- proxies (inbound blocks compiled from the graph) ---
    def proxy_by_id(self, pid):
        return next((p for p in self.proxies if p["id"] == pid), None)

    def statuses(self):
        return {p["id"]: self.manager.state(p["id"]) for p in self.proxies}

    def running_ids(self):
        return {pid for pid, (st, _) in self.statuses().items() if st == "running"}

    def refresh_lists(self):
        w = self.window
        st = self.statuses()
        w.proxies_page.refresh(self.graph, self.proxies, st)
        w.canvas_page.refresh(self.graph, self.proxies, st, self.pc_status())
        w.log_page.set_proxies(self.proxies)
        w.stats_page.set_proxies(self.proxies)
        w.test_page.set_proxies(self.proxies, self.running_ids())
        self._rebuild_tray_proxies()

    def _persist(self):
        if not config.save(self.glob, self.graph):
            info(self.window, "error", _t("Save failed"), _t("Could not save the settings file. Check the log."))

    def _recompile(self, refresh=True):
        """Graph changed: rebuild the runtime proxies, apply to running ones, save."""
        old = {p["id"]: p for p in self.proxies}
        self.proxies = graph_mod.compile(self.graph)
        new_ids = set()
        for p in self.proxies:
            new_ids.add(p["id"])
            if self.manager.state(p["id"])[0] not in ("running", "starting"):
                continue
            before = old.get(p["id"])
            if before is not None and all(before[k] == p[k] for k in ("name", "inbound", "outbound", "overrides")):
                continue
            if p["unlinked"]:
                self.manager.stop(p["id"])
            else:
                self.manager.apply(p, self.glob)
        for pid in old:
            if pid not in new_ids:
                self.manager.stop(pid)
        self._persist()
        self._sync_pc()
        if refresh:
            self.refresh_lists()

    def _unique_name(self, base):
        names = {n["name"] for n in self.graph["inbounds"] + self.graph["outbounds"]}
        if base not in names:
            return base
        i = 2
        while f"{base} {i}" in names:
            i += 1
        return f"{base} {i}"

    def _free_port(self, start):
        used = {n["inbound"]["port"] for n in self.graph["inbounds"]}
        port = start
        while port in used:
            port += 1
        return port

    def new_inbound(self, name, inbound, outbound=None, pos=None, edit=True):
        """Add an inbound block. outbound: None/direct -> the shared Direct block, otherwise a new outbound block."""
        ib = dict(inbound)
        ib["port"] = self._free_port(ib.get("port", 1080))
        pos = pos or graph_mod.next_free_pos(self.graph, "inbounds")
        if not outbound or outbound.get("type", "direct") == "direct":
            target = graph_mod.shared_direct(self.graph)
            fresh = None
        else:
            target = fresh = graph_mod.add_outbound(self.graph, outbound, None, [pos[0] + 460, pos[1]])
            fresh["name"] = self._unique_name(fresh["name"])
        n = graph_mod.add_inbound(self.graph, self._unique_name(name), ib, pos, target["id"])
        log(LOG_SETTING, _t("Created proxy '{name}'").format(name=n['name']))
        self._recompile()
        if edit:
            self.edit_node(fresh["id"] if fresh else n["id"])
        return n

    def new_outbound(self, outbound, pos=None, edit=True):
        pos = pos or graph_mod.next_free_pos(self.graph, "outbounds")
        n = graph_mod.add_outbound(self.graph, outbound, None, pos)
        n["name"] = self._unique_name(n["name"])
        log(LOG_SETTING, _t("Created block '{name}'").format(name=n['name']))
        self._recompile()
        if edit and n["outbound"]["type"] != "direct":
            self.edit_node(n["id"])
        return n

    def new_proxy(self, name, inbound, outbound):
        """List page templates: inbound + outbound (shared Direct or a new outbound block)."""
        self.new_inbound(name, inbound, outbound)

    def edit_node(self, nid):
        kind, n = graph_mod.find(self.graph, nid)
        if n is None:
            return
        w = self.window
        cur = w.stackedWidget.currentWidget()
        if cur is not w.editor:
            w.editor_return = cur if cur in (w.canvas_page, w.proxies_page) else w.canvas_page
        w.editor.load_node(kind, n)
        w.switchTo(w.editor)

    def edit_proxy(self, pid):
        self.edit_node(pid)

    def save_node(self, kind, node):
        key = kind + "s"
        self.graph[key] = [node if x["id"] == node["id"] else x for x in self.graph[key]]
        log(LOG_SETTING, _t("Saved proxy '{name}'").format(name=node['name']))
        self._recompile()

    def set_target(self, iid, tid):
        kind, n = graph_mod.find(self.graph, iid)
        if kind != "inbound" or n["target"] == tid:
            return True
        old = n["target"]
        n["target"] = tid
        p = next(x for x in graph_mod.compile(self.graph) if x["id"] == iid)
        cycle = config.find_cycle(p, graph_mod.compile(self.graph))
        if cycle:
            n["target"] = old
            info(self.window, "error", _t("Cannot connect"),
                 _t("The proxy chain loops: ") + " → ".join(cycle), 5000)
            self.refresh_lists()
            return False
        self._recompile()
        return True

    def set_via(self, oid, vid):
        """Outbound oid connects to its server through outbound vid (None = directly)."""
        kind, n = graph_mod.find(self.graph, oid)
        if kind != "outbound" or n["via"] == vid:
            return True
        if vid and n["outbound"]["type"] in graph_mod.NO_VIA:
            info(self.window, "error", _t("Cannot connect"),
                 _t("Hysteria2 uses QUIC (UDP), so it cannot go through another outbound")
                 if n["outbound"]["type"] == "hysteria2" else _t("Direct is the end of a chain"), 5000)
            self.refresh_lists()
            return False
        old = n["via"]
        n["via"] = vid
        if graph_mod.via_loop(self.graph, oid):
            n["via"] = old
            info(self.window, "error", _t("Cannot connect"),
                 _t("The proxy chain loops: ") + " → ".join(graph_mod.chain_names(self.graph, vid) + [n["name"]]), 5000)
            self.refresh_lists()
            return False
        log(LOG_SETTING, _t("Saved proxy '{name}'").format(name=n['name']))
        self._recompile()
        return True

    def move_nodes(self, positions):
        for kind in ("inbounds", "outbounds"):
            for n in self.graph[kind]:
                if n["id"] in positions:
                    n["pos"] = positions[n["id"]]
        if "__pc__" in positions:
            self.graph["pc"]["pos"] = positions["__pc__"]
        self._persist()

    def duplicate_node(self, nid):
        kind, src = graph_mod.find(self.graph, nid)
        if src is None:
            return
        n = copy.deepcopy(src)
        n["id"] = graph_mod.new_id()
        n["name"] = self._unique_name(src["name"] + _t(" copy"))
        n["pos"] = [src["pos"][0] + 40, src["pos"][1] + 40]
        if kind == "inbound":
            n["inbound"]["port"] = self._free_port(src["inbound"]["port"] + 1)
            n["autostart"] = False
        self.graph[kind + "s"].append(n)
        self._recompile()

    def duplicate_proxy(self, pid):
        self.duplicate_node(pid)

    def delete_node(self, nid):
        kind, n = graph_mod.find(self.graph, nid)
        if n is None:
            return
        graph_mod.remove(self.graph, nid)
        log(LOG_SETTING, _t("Deleted proxy '{name}'").format(name=n['name']))
        self._recompile()

    def delete_proxy(self, pid):
        self.delete_node(pid)

    def toggle_proxy(self, pid, on):
        p = self.proxy_by_id(pid)
        if p is None:
            return
        if on:
            errs = config.check_proxy(p, [x for x in self.proxies if x["id"] != pid])
            if errs:
                info(self.window, "error", _t("Cannot start '{name}'").format(name=p['name']), "\n".join(errs[:3]), 5000)
                self.refresh_lists()
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
        st = self.statuses()
        self.window.proxies_page.set_status(pid, (state, msg), p)
        self._sync_pc()
        self.window.canvas_page.update_states(self.proxies, st, self.pc_status())
        self.window.test_page.set_proxies(self.proxies, self.running_ids())
        self._rebuild_tray_proxies()
        if state == "error" and p is not None:
            self.on_notify(_t("'{name}' failed to start: {msg}").format(name=p['name'], msg=msg))

    # --- PC block: system proxy while the connected inbound runs ---
    def set_pc(self, iid):
        if self.graph["pc"]["inbound"] == iid:
            return
        self.graph["pc"]["inbound"] = iid
        p = self.proxy_by_id(iid) if iid else None
        if p is not None:
            _, err = system_proxy_server(p["inbound"])
            if err:
                info(self.window, "warning", _t("PC (system proxy)"), err, 6000)
        self._persist()
        self._sync_pc()
        self.refresh_lists()

    def pc_status(self):
        """(state, text): state = off | waiting | applied | error | unsupported."""
        pc = self.graph["pc"]
        p = self.proxy_by_id(pc["inbound"]) if pc["inbound"] else None
        if not system_proxy_supported():
            return "unsupported", _t("Windows only for now")
        if p is None:
            return "off", _t("Not connected - the PC uses its own settings")
        _, err = system_proxy_server(p["inbound"])
        if err:
            return "error", err
        if pc["applied"]:
            return "applied", _t("In use: {name} ({server})").format(name=p["name"], server=pc["server"])
        return "waiting", _t("Applied while '{name}' is running").format(name=p["name"])

    def _sync_pc(self):
        pc = self.graph["pc"]
        if not system_proxy_supported():
            return
        p = self.proxy_by_id(pc["inbound"]) if pc["inbound"] else None
        server = None
        if p is not None and self.manager.state(p["id"])[0] == "running":
            server, _ = system_proxy_server(p["inbound"])
        try:
            if server:
                if not pc["applied"]:
                    pc["backup"] = get_system_proxy()
                if not pc["applied"] or pc["server"] != server:
                    set_system_proxy(proxy_values_for(server, pc["backup"]))
                    pc["applied"], pc["server"] = True, server
                    self._persist()
                    log(LOG_SETTING, _t("System proxy set to {server}").format(server=server))
            elif pc["applied"]:
                self._pc_restore_backup()
                log(LOG_SETTING, _t("System proxy restored to the previous settings"))
        except OSError as e:
            log(LOG_ERROR, _t("Could not change the system proxy: {e}").format(e=e))

    def _pc_restore_backup(self):
        pc = self.graph["pc"]
        if pc["backup"] is not None:
            set_system_proxy(pc["backup"])
        pc["applied"], pc["server"] = False, ""
        self._persist()

    def pc_restore(self):
        """'Restore previous': disconnect the PC block and put back the settings from before ProxyDock changed them."""
        pc = self.graph["pc"]
        pc["inbound"] = None
        try:
            if pc["backup"] is not None:
                set_system_proxy(pc["backup"])
                log(LOG_SETTING, _t("System proxy restored to the previous settings"))
        except OSError as e:
            log(LOG_ERROR, _t("Could not change the system proxy: {e}").format(e=e))
        pc["applied"], pc["server"], pc["backup"] = False, "", None
        self._persist()
        self.refresh_lists()

    def pc_off(self):
        """'Off (default)': disconnect the PC block and turn the system proxy off."""
        pc = self.graph["pc"]
        pc["inbound"] = None
        try:
            set_system_proxy({"ProxyEnable": 0})
            log(LOG_SETTING, _t("System proxy turned off"))
        except OSError as e:
            log(LOG_ERROR, _t("Could not change the system proxy: {e}").format(e=e))
        pc["applied"], pc["server"], pc["backup"] = False, "", None
        self._persist()
        self.refresh_lists()

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
            QTimer.singleShot(0, lambda code=g["language"]: self.change_language(code))
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

    # --- language: switch at once by rebuilding the window ---
    def _retranslate_tray(self):
        self.act_stop_all.setText(_t("Stop all proxies"))
        self.act_startup.setText(_t("Start at login"))
        self.act_hidden.setText(_t("Hide startup notification"))
        self.act_restart.setText(_t("Restart"))
        self.act_exit.setText(_t("Quit"))
        self._sync_show_action(self.window.isVisible() if hasattr(self, "window") else False)

    def change_language(self, code):
        from .. import i18n
        from PySide6.QtCore import QLocale
        from qfluentwidgets import FluentTranslator
        i18n.set_language(code)
        app = QApplication.instance()
        old_tr = getattr(app, "_fluent_translator", None)
        if old_tr is not None:
            app.removeTranslator(old_tr)
        locale = QLocale(i18n.current())
        QLocale.setDefault(locale)
        app._fluent_translator = FluentTranslator(locale)
        app.installTranslator(app._fluent_translator)
        load_fonts(app, self.glob)          # Japanese/Chinese use the system font of that language
        self.rebuild_window()
        log(LOG_SETTING, _t("Language changed"))

    def rebuild_window(self):
        """Recreate the main window (all pages) in the current language, keeping position, page, logs and graphs."""
        old = self.window
        try:
            old.visibilityChanged.disconnect(self._sync_show_action)
        except (RuntimeError, TypeError):
            pass
        visible, maximized = old.isVisible(), old.isMaximized()
        geometry = old.saveGeometry()
        cur = old.stackedWidget.currentWidget()
        page = cur.objectName() if cur is not None and cur is not old.editor else "canvasInterface"
        if cur is old.editor and old.editor_return is not None:
            page = old.editor_return.objectName()
        entries = list(old.log_page.entries)
        history = old.stats_page.history
        self.window = MainWindow(self)
        w = self.window
        w.visibilityChanged.connect(self._sync_show_action)
        w.restoreGeometry(geometry)
        w.log_page.add_entries(entries)
        w.stats_page.history = history
        target = next((p for p in (w.canvas_page, w.proxies_page, w.log_page, w.stats_page, w.test_page,
                                   w.help_page, w.license_page, w.settings_page) if p.objectName() == page), w.canvas_page)
        w.stackedWidget.setCurrentWidget(target, popOut=False)
        w.navigationInterface.setCurrentItem(target.objectName())
        self.refresh_lists()
        self.refresh_states()
        if IS_WIN:
            self.refresh_firewall()
        self._retranslate_tray()
        self._rebuild_tray_proxies()
        if visible:
            if maximized:
                w.showMaximized()
            else:
                w.show()
            w.raise_()
            w.activateWindow()
        old.hide()
        old.deleteLater()

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
        if self.graph["pc"]["applied"]:
            try:
                self._pc_restore_backup()
            except OSError:
                pass
        self.manager.stop_all(2)
        self.drain_logs()
        self.tray.hide()
        common.hard_exit(0)


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
    from .. import __version__
    parser.add_argument("--version", action="version", version=f"ProxyDock {__version__}")
    args = parser.parse_args()

    glob, graph = config.load()
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
    from .. import __version__
    app.setApplicationVersion(__version__)
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
    log(LOG_INFO, _t("{APP_NAME} started - {len} proxies").format(APP_NAME=APP_NAME, len=len(graph["inbounds"])))
    controller = Controller(single, show_start_notice=not args.hidden, glob=glob, graph=graph)
    _ = controller
    sys.exit(app.exec())
