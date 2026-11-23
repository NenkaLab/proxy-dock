"""Connection test page: pick a running proxy and connect to the test sites (built in + added by the user)."""
from ..i18n import _t, on_language_change
import threading
import time
from concurrent.futures import ThreadPoolExecutor

from PySide6.QtCore import QObject, Qt, QTimer, Signal
from PySide6.QtWidgets import QHBoxLayout, QTableWidgetItem, QVBoxLayout, QWidget
from qfluentwidgets import (
    Action, CaptionLabel, FluentIcon as FIF, LineEdit, PrimaryPushButton, PushButton, RoundMenu, TableWidget, TitleLabel,
)

from .. import config
from ..common import LOG_FAIL, LOG_INFO, LOG_TCP, log
from ..probe import BRIDGE_ID, PROBE_COOLDOWN, PROBE_TIMEOUT, PROBE_WORKERS, TEST_SITES, bridge_for, bridge_port, \
    probe_site
from .common import ProxyCombo, info, level_color


class _Signals(QObject):
    result = Signal(int, object)
    done = Signal(str)


COLS = []


@on_language_change
def _tr_cols():
    COLS[:] = [_t("Region"), _t("Site"), _t("Address"), _t("Result"), _t("Connect"), _t("All")]


class ConnTestInterface(QWidget):

    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        self.setObjectName("connTestInterface")
        self.ctl = ctl
        self.running = False
        self.cooldown = 0
        self.results = {}
        self.sig = _Signals()
        self.sig.result.connect(self._on_result)
        self.sig.done.connect(self._on_done)

        title = TitleLabel(_t("Connection test"), self)
        self.proxy = ProxyCombo(self, None, 240)
        self.proxy.currentIndexChanged.connect(lambda *_: self._sync_button())
        self.summary = CaptionLabel("", self)
        self.btn = PrimaryPushButton(_t("Start test"), self, FIF.SPEED_HIGH)
        self.btn.clicked.connect(self._on_click)

        self.table = TableWidget(self)
        self.table.setColumnCount(len(COLS))
        self.table.setHorizontalHeaderLabels(COLS)
        self.table.verticalHeader().hide()
        self.table.setEditTriggers(TableWidget.NoEditTriggers)
        self.table.setSelectionMode(TableWidget.NoSelection)
        self.table.setBorderVisible(True)
        self.table.setBorderRadius(8)
        self.table.setWordWrap(False)
        hdr = self.table.horizontalHeader()
        from .common import Fonts
        for c, w in enumerate((80, 140, 200, 0, 110, 90)):      # (wider with a larger text size)
            if w:
                self.table.setColumnWidth(c, round(w * Fonts.text_scale / 100))
        hdr.setSectionResizeMode(3, hdr.ResizeMode.Stretch)
        self.table.setContextMenuPolicy(Qt.CustomContextMenu)
        self.table.customContextMenuRequested.connect(self._row_menu)
        self.ed_site = LineEdit(self)
        self.ed_site.setPlaceholderText(_t("Add a site, e.g. example.com"))
        self.ed_site.setFixedWidth(280)
        self.ed_site.returnPressed.connect(self.add_site)
        self.btn_add = PushButton(_t("Add"), self, FIF.ADD)
        self.btn_add.clicked.connect(self.add_site)
        self._fill_rows()

        header = QHBoxLayout()
        header.addWidget(title)
        header.addStretch(1)
        header.addWidget(self.proxy)
        header.addWidget(self.btn)
        root = QVBoxLayout(self)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(10)
        sub = QHBoxLayout()
        sub.addWidget(self.summary, 1)
        sub.addWidget(self.ed_site)
        sub.addWidget(self.btn_add)
        root.addLayout(header)
        root.addLayout(sub)
        root.addWidget(self.table, 1)

        # against rapid clicks: wait briefly after a click and start once (debounce), then a cooldown (throttle)
        self._debounce = QTimer(self)
        self._debounce.setSingleShot(True)
        self._debounce.setInterval(300)
        self._debounce.timeout.connect(self._start)
        self._cool_timer = QTimer(self)
        self._cool_timer.setInterval(1000)
        self._cool_timer.timeout.connect(self._cool_tick)
        self._sync_button()

    # --- sites: the built-in list plus the ones the user added ---
    def sites(self):
        """[(region, name, host, added by the user)]."""
        return [(g, n, h, False) for g, n, h in TEST_SITES] + \
               [(_t("Added"), h, h, True) for h in self.ctl.glob.get("test_sites", [])]

    def _fill_rows(self):
        sites = self.sites()
        self.table.setRowCount(len(sites))
        for r, (group, name, host, _) in enumerate(sites):
            for c, text in enumerate((group, name, host, "-", "", "")):
                self.table.setItem(r, c, QTableWidgetItem(text))

    def add_site(self):
        if self.running:
            return
        host = config.test_site_host(self.ed_site.text())
        if host is None:
            info(self.window(), "warning", _t("Cannot add"), _t("Enter a site name like example.com"), 4000)
            return
        if host in [h for _, _, h, _ in self.sites()]:
            info(self.window(), "info", _t("Already in the list"), host, 3000)
            self.ed_site.clear()
            return
        if len(self.ctl.glob.get("test_sites", [])) >= config.MAX_TEST_SITES:
            info(self.window(), "warning", _t("Cannot add"),
                 _t("Up to {n} added sites").format(n=config.MAX_TEST_SITES), 4000)
            return
        self.ctl.set_test_sites(self.ctl.glob.get("test_sites", []) + [host])
        self.ed_site.clear()
        self.results.clear()
        self._fill_rows()
        self._sync_button()

    def remove_site(self, host):
        if self.running:
            return
        self.ctl.set_test_sites([h for h in self.ctl.glob.get("test_sites", []) if h != host])
        self.results.clear()
        self._fill_rows()
        self._sync_button()

    def _row_menu(self, pos):
        r = self.table.rowAt(pos.y())
        sites = self.sites()
        if not 0 <= r < len(sites) or not sites[r][3]:
            return
        menu = RoundMenu(parent=self)
        a = Action(FIF.DELETE, _t("Remove from the test"), menu)
        a.setEnabled(not self.running)
        a.triggered.connect(lambda: self.remove_site(sites[r][2]))
        menu.addAction(a)
        menu.exec(self.table.viewport().mapToGlobal(pos))

    def set_proxies(self, proxies, running_ids):
        self.proxy.set_proxies(proxies, only=running_ids)
        self._sync_button()

    def _sync_button(self):
        if self.running or self.cooldown > 0:
            return
        has = bool(self.proxy.current_id())
        self.btn.setEnabled(has)
        if has:
            p = self.ctl.proxy_by_id(self.proxy.current_id())
            self.summary.setText(
                _t("Connects to {len} sites over HTTPS through '{name}' ({PROBE_WORKERS} at a time, up to {PROBE_TIMEOUT}s per site).").format(name=p['name'], len=len(self.sites()), PROBE_WORKERS=PROBE_WORKERS, PROBE_TIMEOUT=PROBE_TIMEOUT) if p else "")
        else:
            self.summary.setText(_t("No proxy is running. Start one on the Proxies page first."))

    def _on_click(self):
        if self.running or self.cooldown > 0 or not self.proxy.current_id():
            return
        self.btn.setEnabled(False)
        self.btn.setText(_t("Preparing…"))
        self._debounce.start()

    def _start(self):
        proxy = self.ctl.proxy_by_id(self.proxy.current_id())
        if proxy is None:
            self._cool_tick()
            return
        self.running = True
        self.results.clear()
        self.btn.setText(_t("Testing…"))
        self.ed_site.setEnabled(False)
        self.btn_add.setEnabled(False)
        self.hosts = [h for _, _, h, _ in self.sites()]
        for r in range(len(self.hosts)):
            self._set_row(r, _t("Waiting…"), "", "", None)
        self.summary.setText(_t("Testing '{name}'… 0 / {len}").format(name=proxy['name'], len=len(self.hosts)))
        log(LOG_INFO, _t("Connection test started: {name} ({len} sites)").format(name=proxy['name'], len=len(self.hosts)))
        hosts = list(self.hosts)
        mgr, glob = self.ctl.manager, self.ctl.glob

        def work():
            err = ""
            try:
                if not mgr.start(bridge_for(proxy), glob).result(15):
                    raise RuntimeError(mgr.state(BRIDGE_ID)[1] or _t("Could not create the test connection"))
                port = bridge_port(mgr)
                with ThreadPoolExecutor(max_workers=PROBE_WORKERS) as ex:
                    futs = {ex.submit(probe_site, host, port): r for r, host in enumerate(hosts)}
                    for f in futs:
                        self.sig.result.emit(futs[f], f.result())
            except Exception as e:
                err = str(e)
            finally:
                try:
                    mgr.stop(BRIDGE_ID).result(5)
                except Exception:
                    pass
            self.sig.done.emit(err)
        threading.Thread(target=work, daemon=True).start()

    def _set_row(self, r, result, conn, total, ok):
        for c, text in ((3, result), (4, conn), (5, total)):
            self.table.item(r, c).setText(text)
        item = self.table.item(r, 3)
        if ok is None:
            item.setData(Qt.ForegroundRole, None)
        else:
            item.setForeground(level_color(LOG_TCP if ok else LOG_FAIL))

    def _on_result(self, r, res):
        self.results[r] = res
        ms = lambda v: f"{v:.0f} ms" if v is not None else "-"
        if res["ok"]:
            text = _t("Connected · {note}").format(note=res['note']) if res.get("note") else _t("OK · HTTP {status}").format(status=res['status'])
        else:
            text = _t("Failed · {error}").format(error=res['error'])
        self._set_row(r, text, ms(res["connect_ms"]), ms(res["total_ms"]), res["ok"])
        self.summary.setText(_t("Testing… {len} / {len2}").format(len=len(self.results), len2=len(self.hosts)))

    def _on_done(self, err):
        self.running = False
        self.ed_site.setEnabled(True)
        self.btn_add.setEnabled(True)
        if err:
            self.summary.setText(_t("Could not start the test: {err}").format(err=err))
            log(LOG_FAIL, _t("Connection test failed: {err}").format(err=err))
        else:
            oks = [v for v in self.results.values() if v["ok"]]
            avg = sum(v["total_ms"] for v in oks) / len(oks) if oks else 0
            msg = _t("Succeeded {len} / {len2}").format(len=len(oks), len2=len(self.hosts)) + (_t(" · average {avg:.0f} ms").format(avg=avg) if oks else "")
            self.summary.setText(msg + _t("  ·  finished at {strftime}").format(strftime=time.strftime('%H:%M:%S')))
            log(LOG_INFO, _t("Connection test finished: {msg}").format(msg=msg))
        self.cooldown = PROBE_COOLDOWN
        self._cool_tick()
        self._cool_timer.start()

    def _cool_tick(self):
        if self.cooldown <= 0:
            self._cool_timer.stop()
            self.btn.setText(_t("Test again") if self.results else _t("Start test"))
            self._sync_button()
            return
        self.btn.setText(_t("Test again ({cooldown}s)").format(cooldown=self.cooldown))
        self.cooldown -= 1
