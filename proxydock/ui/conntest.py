"""연결 테스트 페이지: 실행 중인 프록시를 드롭다운으로 골라 21개 사이트에 접속해 본다."""
import threading
import time
from concurrent.futures import ThreadPoolExecutor

from PySide6.QtCore import QObject, Qt, QTimer, Signal
from PySide6.QtWidgets import QHBoxLayout, QTableWidgetItem, QVBoxLayout, QWidget
from qfluentwidgets import CaptionLabel, FluentIcon as FIF, PrimaryPushButton, TableWidget, TitleLabel

from ..common import LOG_FAIL, LOG_INFO, LOG_TCP, log
from ..probe import BRIDGE_ID, PROBE_COOLDOWN, PROBE_TIMEOUT, PROBE_WORKERS, TEST_SITES, bridge_for, bridge_port, \
    probe_site
from .common import ProxyCombo, level_color


class _Signals(QObject):
    result = Signal(int, object)
    done = Signal(str)


class ConnTestInterface(QWidget):
    COLS = ("구분", "사이트", "주소", "결과", "목적지 연결", "전체")

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

        title = TitleLabel("연결 테스트", self)
        self.proxy = ProxyCombo(self, None, 240)
        self.proxy.currentIndexChanged.connect(lambda *_: self._sync_button())
        self.summary = CaptionLabel("", self)
        self.btn = PrimaryPushButton("테스트 시작", self, FIF.SPEED_HIGH)
        self.btn.clicked.connect(self._on_click)

        self.table = TableWidget(self)
        self.table.setColumnCount(len(self.COLS))
        self.table.setHorizontalHeaderLabels(self.COLS)
        self.table.setRowCount(len(TEST_SITES))
        self.table.verticalHeader().hide()
        self.table.setEditTriggers(TableWidget.NoEditTriggers)
        self.table.setSelectionMode(TableWidget.NoSelection)
        self.table.setBorderVisible(True)
        self.table.setBorderRadius(8)
        self.table.setWordWrap(False)
        hdr = self.table.horizontalHeader()
        for c, w in enumerate((80, 130, 190, 0, 100, 90)):
            if w:
                self.table.setColumnWidth(c, w)
        hdr.setSectionResizeMode(3, hdr.ResizeMode.Stretch)
        for r, (group, name, host) in enumerate(TEST_SITES):
            for c, text in enumerate((group, name, host, "-", "", "")):
                self.table.setItem(r, c, QTableWidgetItem(text))

        header = QHBoxLayout()
        header.addWidget(title)
        header.addStretch(1)
        header.addWidget(self.proxy)
        header.addWidget(self.btn)
        root = QVBoxLayout(self)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(10)
        root.addLayout(header)
        root.addWidget(self.summary)
        root.addWidget(self.table, 1)

        # 연타 방지: 클릭 후 잠깐 기다렸다 한 번만 시작(디바운스), 끝난 뒤엔 쿨다운(쓰로틀)
        self._debounce = QTimer(self)
        self._debounce.setSingleShot(True)
        self._debounce.setInterval(300)
        self._debounce.timeout.connect(self._start)
        self._cool_timer = QTimer(self)
        self._cool_timer.setInterval(1000)
        self._cool_timer.timeout.connect(self._cool_tick)
        self._sync_button()

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
                f"'{p['name']}' 을(를) 거쳐 {len(TEST_SITES)}개 사이트에 HTTPS 로 접속해 봅니다 "
                f"(동시 {PROBE_WORKERS}개, 사이트당 최대 {PROBE_TIMEOUT}초)." if p else "")
        else:
            self.summary.setText("실행 중인 프록시가 없습니다. 프록시 화면에서 먼저 시작하세요.")

    def _on_click(self):
        if self.running or self.cooldown > 0 or not self.proxy.current_id():
            return
        self.btn.setEnabled(False)
        self.btn.setText("준비 중…")
        self._debounce.start()

    def _start(self):
        proxy = self.ctl.proxy_by_id(self.proxy.current_id())
        if proxy is None:
            self._cool_tick()
            return
        self.running = True
        self.results.clear()
        self.btn.setText("테스트 중…")
        for r in range(len(TEST_SITES)):
            self._set_row(r, "대기 중…", "", "", None)
        self.summary.setText(f"'{proxy['name']}' 테스트 중… 0 / {len(TEST_SITES)}")
        log(LOG_INFO, f"연결 테스트 시작: {proxy['name']} ({len(TEST_SITES)}개 사이트)")
        mgr, glob = self.ctl.manager, self.ctl.glob

        def work():
            err = ""
            try:
                if not mgr.start(bridge_for(proxy), glob).result(15):
                    raise RuntimeError(mgr.state(BRIDGE_ID)[1] or "테스트 연결을 만들지 못했습니다")
                port = bridge_port(mgr)
                with ThreadPoolExecutor(max_workers=PROBE_WORKERS) as ex:
                    futs = {ex.submit(probe_site, host, port): r for r, (_, _, host) in enumerate(TEST_SITES)}
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
            text = f"연결됨 · {res['note']}" if res.get("note") else f"성공 · HTTP {res['status']}"
        else:
            text = f"실패 · {res['error']}"
        self._set_row(r, text, ms(res["connect_ms"]), ms(res["total_ms"]), res["ok"])
        self.summary.setText(f"테스트 중… {len(self.results)} / {len(TEST_SITES)}")

    def _on_done(self, err):
        self.running = False
        if err:
            self.summary.setText(f"테스트를 시작하지 못했습니다: {err}")
            log(LOG_FAIL, f"연결 테스트 실패: {err}")
        else:
            oks = [v for v in self.results.values() if v["ok"]]
            avg = sum(v["total_ms"] for v in oks) / len(oks) if oks else 0
            msg = f"성공 {len(oks)} / {len(TEST_SITES)}" + (f" · 평균 {avg:.0f} ms" if oks else "")
            self.summary.setText(msg + f"  ·  {time.strftime('%H:%M:%S')} 완료")
            log(LOG_INFO, f"연결 테스트 완료: {msg}")
        self.cooldown = PROBE_COOLDOWN
        self._cool_tick()
        self._cool_timer.start()

    def _cool_tick(self):
        if self.cooldown <= 0:
            self._cool_timer.stop()
            self.btn.setText("다시 테스트" if self.results else "테스트 시작")
            self._sync_button()
            return
        self.btn.setText(f"다시 테스트 ({self.cooldown}초)")
        self.cooldown -= 1
