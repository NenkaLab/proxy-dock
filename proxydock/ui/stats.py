"""Statistics page: proxy selector (total / per proxy) + tiles + throughput graph. The graph shows download and
upload of the selection (Combined), one line per running proxy card (Per proxy) or one line per outbound block
the traffic leaves through (Per exit); the last two show download + upload together."""
from ..i18n import _t
from collections import deque

from PySide6.QtCore import QPointF, Qt
from PySide6.QtGui import QColor, QPainter, QPainterPath, QPen
from PySide6.QtWidgets import QGridLayout, QHBoxLayout, QVBoxLayout, QWidget
from qfluentwidgets import (
    CaptionLabel, SegmentedWidget, SimpleCardWidget, SubtitleLabel, TitleLabel, isDarkTheme, themeColor,
)

from ..common import fmt_rate, fmt_size
from .common import ProxyCombo

POINTS = 60
# line colours for cards without a colour of their own (the canvas palette)
LINE_COLORS = ("#4f8ef7", "#e8912d", "#57ab5a", "#b76de0", "#e5534b", "#39c5bb", "#d9b92f", "#e26ba5", "#7c6cf2", "#9aa0a6")


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
        self.lines = None       # Per proxy: [(name, "#rrggbb", values)]
        self.setMinimumHeight(180)

    def set_series(self, up, down):
        self.up, self.down, self.lines = list(up), list(down), None
        if self.isVisible():
            self.update()

    def set_lines(self, lines, empty=None):
        """One line each: [(name, colour, values)]; empty = the text shown when there is no line."""
        self.lines = [(n, c, list(v)) for n, c, v in lines]
        self.empty_text = empty
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
        if self.lines is not None:
            top = max([max(v) for _, _, v in self.lines] + [1024.0])
        else:
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

        f = p.font()
        f.setPixelSize(12)
        p.setFont(f)
        if self.lines is not None:
            legend = [(name, QColor(c)) for name, c, _ in self.lines]
            for _, c, values in self.lines:
                series(values, QColor(c), False)
        else:
            down_c = themeColor()
            up_c = QColor("#E3008C") if dark else QColor("#B4009E")
            series(self.down, down_c, True)
            series(self.up, up_c, False)
            legend = [(_t("Download"), down_c), (_t("Upload"), up_c)]
        p.setPen(muted)
        p.drawText(4, 16, _t("max {fmt_rate}").format(fmt_rate=fmt_rate(top)))
        if self.lines is not None and not self.lines:
            p.drawText(r, Qt.AlignCenter, getattr(self, "empty_text", None) or _t("No proxy is running"))
        metrics = p.fontMetrics()                   # legend from the right edge
        x = self.width() - 4
        for label, c in reversed(legend):
            w = metrics.horizontalAdvance(label) + 26
            x -= w
            if x < 160:                             # (no room: the rest are left out)
                break
            p.fillRect(x, 7, 10, 10, c)
            p.setPen(muted)
            p.drawText(x + 16, 16, label)
        p.end()


class StatsInterface(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("statsInterface")
        self.history = {}       # id ("" = total) -> (up deque, down deque)
        self.latest = {}        # id -> (Stats, up_rate, down_rate)
        self.proxies, self.colors = [], {}
        self.exits = []         # [(outbound block id, name, colour)]
        self.exit_history = {}  # outbound block id -> deque of down + up per second

        title = TitleLabel(_t("Statistics"), self)
        self.proxy = ProxyCombo(self, _t("Total"), 240)
        self.proxy.currentIndexChanged.connect(lambda *_: self._show())
        self.mode = SegmentedWidget(self)               # graph: the selection, or a line per card
        self.mode.addItem("combined", _t("Combined"), lambda: self._show())
        self.mode.addItem("per_proxy", _t("Per proxy"), lambda: self._show())
        self.mode.addItem("per_exit", _t("Per exit"), lambda: self._show())
        self.mode.setCurrentItem("combined")
        self.t_tcp = StatTile(_t("Active TCP connections"), self)
        self.t_udp = StatTile(_t("Active UDP sessions"), self)
        self.t_total = StatTile(_t("Total connections"), self)
        self.t_down = StatTile(_t("Download"), self)
        self.t_up = StatTile(_t("Upload"), self)
        self.t_err = StatTile(_t("Blocked / failed"), self)
        grid = QGridLayout()
        grid.setSpacing(12)
        for i, t in enumerate((self.t_tcp, self.t_udp, self.t_total, self.t_down, self.t_up, self.t_err)):
            grid.addWidget(t, i // 3, i % 3)

        chart_card = SimpleCardWidget(self)
        self.chart = ThroughputChart(chart_card)
        cl = QVBoxLayout(chart_card)
        cl.setContentsMargins(18, 14, 18, 14)
        self.chart_title = CaptionLabel(_t("Throughput (last 60 s)"), chart_card)
        cl.addWidget(self.chart, 1)

        header = QHBoxLayout()
        header.addWidget(title)
        header.addStretch(1)
        header.addWidget(self.proxy)
        chart_head = QHBoxLayout()
        chart_head.addWidget(self.chart_title)
        chart_head.addStretch(1)
        chart_head.addWidget(self.mode)
        cl.insertLayout(0, chart_head)
        root = QVBoxLayout(self)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(14)
        root.addLayout(header)
        root.addLayout(grid)
        root.addWidget(chart_card, 1)

    def set_proxies(self, proxies, colors=None):
        """proxies: the cards; colors: {card id: '#rrggbb'} (their canvas colours, '' = none)."""
        self.proxy.set_proxies(proxies)
        self.proxies, self.colors = list(proxies), dict(colors or {})

    def per_proxy(self):
        return self.mode.currentRouteKey() == "per_proxy"

    def per_exit(self):
        return self.mode.currentRouteKey() == "per_exit"

    def set_exits(self, exits):
        """exits: [(outbound block id, name, '#rrggbb' or '')] that can carry traffic."""
        self.exits = list(exits)

    def push_exits(self, rates):
        """rates: {outbound block id: (up/s, down/s)}, once a second."""
        for oid, (up, down) in rates.items():
            h = self.exit_history.get(oid)
            if h is None:
                h = self.exit_history[oid] = deque([0.0] * POINTS, maxlen=POINTS)
            h.append(up + down)
        for oid, h in self.exit_history.items():        # (no traffic this second)
            if oid not in rates:
                h.append(0.0)
        if self.isVisible() and self.per_exit():
            self._show()

    def exit_lines(self):
        """[(name, colour, down + up per second)] of the outbound blocks that carried traffic in the last minute."""
        out, k = [], 0
        for oid, name, color in self.exits:
            h = self.exit_history.get(oid)
            if h is None or not any(h):
                continue
            if not color:
                color = LINE_COLORS[k % len(LINE_COLORS)]
                k += 1
            out.append((name, color, list(h)))
        return out

    def card_lines(self):
        """[(name, colour, down + up per second)] of the cards with traffic history that are still there."""
        out, k = [], 0
        for p in self.proxies:
            h = self.history.get(p["id"])
            if h is None or p["id"] not in self.latest:
                continue
            color = self.colors.get(p["id"]) or LINE_COLORS[k % len(LINE_COLORS)]
            k += 0 if self.colors.get(p["id"]) else 1
            out.append((p["name"], color, [u + d for u, d in zip(h[0], h[1])]))
        return out

    def push(self, snapshots):
        """snapshots: {id: (Stats, up_rate, down_rate)}, "" is the total. Called once a second."""
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
        self.t_down.set(fmt_rate(down), _t("total {fmt_size}").format(fmt_size=fmt_size(s.down)))
        self.t_up.set(fmt_rate(up), _t("total {fmt_size}").format(fmt_size=fmt_size(s.up)))
        self.t_err.set(f"{s.blocked:,} / {s.failed:,}")
        if self.per_proxy():
            self.chart.set_lines(self.card_lines())
            return
        if self.per_exit():
            self.chart.set_lines(self.exit_lines(), _t("No traffic in the last minute"))
            return
        h = self.history.get(pid)
        if h:
            self.chart.set_series(h[0], h[1])
