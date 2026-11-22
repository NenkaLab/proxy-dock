"""통계 페이지: 프록시 드롭다운(전체 합계 / 프록시별) + 타일 + 처리량 그래프."""
from collections import deque

from PySide6.QtCore import QPointF
from PySide6.QtGui import QColor, QPainter, QPainterPath, QPen
from PySide6.QtWidgets import QGridLayout, QHBoxLayout, QVBoxLayout, QWidget
from qfluentwidgets import CaptionLabel, SimpleCardWidget, SubtitleLabel, TitleLabel, isDarkTheme, themeColor

from ..common import fmt_rate, fmt_size
from .common import ProxyCombo

POINTS = 60


class StatTile(SimpleCardWidget):
    def __init__(self, title, parent=None):
        super().__init__(parent)
        self.caption = CaptionLabel(title, self)
        self.value = SubtitleLabel("0", self)
        self.sub = CaptionLabel("", self)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(18, 14, 18, 14)
        lay.setSpacing(2)
        lay.addWidget(self.caption)
        lay.addWidget(self.value)
        lay.addWidget(self.sub)

    def set(self, value, sub=""):
        self.value.setText(value)
        self.sub.setText(sub)


class ThroughputChart(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.up = [0.0] * POINTS
        self.down = [0.0] * POINTS
        self.setMinimumHeight(180)

    def set_series(self, up, down):
        self.up, self.down = list(up), list(down)
        if self.isVisible():
            self.update()

    def paintEvent(self, _):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        dark = isDarkTheme()
        fg = QColor(255, 255, 255) if dark else QColor(0, 0, 0)
        grid = QColor(fg)
        grid.setAlpha(28)
        muted = QColor(fg)
        muted.setAlpha(150)
        r = self.rect().adjusted(4, 26, -4, -4)
        top = max(max(self.up), max(self.down), 1024.0)
        p.setPen(QPen(grid, 1))
        for i in range(5):
            y = r.top() + r.height() * i / 4
            p.drawLine(QPointF(r.left(), y), QPointF(r.right(), y))

        def series(values, color, fill):
            step = r.width() / (POINTS - 1)
            path = QPainterPath()
            for i, v in enumerate(values):
                pt = QPointF(r.left() + i * step, r.bottom() - r.height() * v / top)
                if i == 0:
                    path.moveTo(pt)
                else:
                    path.lineTo(pt)
            if fill:
                area = QPainterPath(path)
                area.lineTo(QPointF(r.right(), r.bottom()))
                area.lineTo(QPointF(r.left(), r.bottom()))
                c = QColor(color)
                c.setAlpha(50)
                p.fillPath(area, c)
            p.setPen(QPen(color, 2))
            p.drawPath(path)

        down_c = themeColor()
        up_c = QColor("#E3008C") if dark else QColor("#B4009E")
        series(self.down, down_c, True)
        series(self.up, up_c, False)
        p.setPen(muted)
        f = p.font()
        f.setPixelSize(12)
        p.setFont(f)
        p.drawText(4, 16, f"최대 {fmt_rate(top)}")
        x = self.width() - 200
        for label, c in (("다운로드", down_c), ("업로드", up_c)):
            p.fillRect(x, 7, 10, 10, c)
            p.setPen(muted)
            p.drawText(x + 16, 16, label)
            x += 90
        p.end()


class StatsInterface(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("statsInterface")
        self.history = {}       # id("" = 합계) -> (up deque, down deque)
        self.latest = {}        # id -> (Stats, up_rate, down_rate)

        title = TitleLabel("통계", self)
        self.proxy = ProxyCombo(self, "전체 합계", 240)
        self.proxy.currentIndexChanged.connect(lambda *_: self._show())
        self.t_tcp = StatTile("활성 TCP 연결", self)
        self.t_udp = StatTile("활성 UDP 세션", self)
        self.t_total = StatTile("누적 연결", self)
        self.t_down = StatTile("다운로드", self)
        self.t_up = StatTile("업로드", self)
        self.t_err = StatTile("차단 / 실패", self)
        grid = QGridLayout()
        grid.setSpacing(12)
        for i, t in enumerate((self.t_tcp, self.t_udp, self.t_total, self.t_down, self.t_up, self.t_err)):
            grid.addWidget(t, i // 3, i % 3)

        chart_card = SimpleCardWidget(self)
        self.chart = ThroughputChart(chart_card)
        cl = QVBoxLayout(chart_card)
        cl.setContentsMargins(18, 14, 18, 14)
        self.chart_title = CaptionLabel("처리량 (최근 60초)", chart_card)
        cl.addWidget(self.chart_title)
        cl.addWidget(self.chart, 1)

        header = QHBoxLayout()
        header.addWidget(title)
        header.addStretch(1)
        header.addWidget(self.proxy)
        root = QVBoxLayout(self)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(14)
        root.addLayout(header)
        root.addLayout(grid)
        root.addWidget(chart_card, 1)

    def set_proxies(self, proxies):
        self.proxy.set_proxies(proxies)

    def push(self, snapshots):
        """snapshots: {id: (Stats, up_rate, down_rate)}, "" 는 합계. 1초마다 호출."""
        for pid, (s, up, down) in snapshots.items():
            h = self.history.get(pid)
            if h is None:
                h = self.history[pid] = (deque([0.0] * POINTS, maxlen=POINTS), deque([0.0] * POINTS, maxlen=POINTS))
            h[0].append(up)
            h[1].append(down)
        self.latest = snapshots
        if self.isVisible():
            self._show()

    def showEvent(self, e):
        super().showEvent(e)
        self._show()

    def _show(self):
        pid = self.proxy.current_id() or ""
        got = self.latest.get(pid)
        if got is None:
            return
        s, up, down = got
        self.t_tcp.set(str(s.tcp_active))
        self.t_udp.set(str(s.udp_active))
        self.t_total.set(f"{s.total:,}")
        self.t_down.set(fmt_rate(down), f"누적 {fmt_size(s.down)}")
        self.t_up.set(fmt_rate(up), f"누적 {fmt_size(s.up)}")
        self.t_err.set(f"{s.blocked:,} / {s.failed:,}")
        h = self.history.get(pid)
        if h:
            self.chart.set_series(h[0], h[1])
