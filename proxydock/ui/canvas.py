"""Canvas page: blueprint-style block graph (NodeGraphQt).

Blocks: inbound (accepts connections on this PC), outbound (Direct / upstream proxy / tunnel) and PC (system proxy).
Drag from an inbound's output to an outbound block, to another inbound (relay) or to the PC block.
The canvas is a view of ctl.graph: every change goes through the controller, which saves and applies it.
"""
import json
import os

os.environ.setdefault("QT_PREFERRED_BINDING", "PySide6")     # NodeGraphQt uses Qt.py

from NodeGraphQt import BaseNode, NodeBaseWidget, NodeGraph
from NodeGraphQt.constants import PipeEnum, PortEnum
from NodeGraphQt.qgraphics.node_abstract import AbstractNodeItem
from NodeGraphQt.qgraphics.node_base import NodeItem
from NodeGraphQt.qgraphics.port import PortItem
from PySide6.QtCore import QEvent, QObject, QPointF, QRectF, QSize, Qt, QTimer
from PySide6.QtGui import (
    QColor, QFont, QFontMetrics, QIcon, QKeyEvent, QKeySequence, QPainter, QPainterPath, QPen, QPixmap, QPolygonF,
    QShortcut, QTextCursor, QTextOption,
)
from PySide6.QtWidgets import (
    QApplication, QGraphicsItem, QGraphicsPathItem, QGraphicsTextItem, QHBoxLayout, QLabel, QVBoxLayout, QWidget,
)
from qfluentwidgets import (
    Action, CaptionLabel, CommandBar, FluentIcon as FIF, RoundMenu, TitleLabel, TransparentDropDownPushButton,
    isDarkTheme, qconfig,
)

from ..common import INBOUND_TYPES, host_port
from ..i18n import _t, on_language_change
from .common import STATE_TEXT, info
from .proxies import OUTBOUND_NEW, PcBar, confirm_delete, outbound_summary

PC_ID = "__pc__"
IN_COLOR = (34, 74, 128)
OUT_COLOR = (30, 100, 74)
PC_COLOR = (92, 58, 132)
PORT_COLOR = (120, 170, 235)
VIA_COLOR = (235, 190, 110)
ROUTER_COLOR = (128, 86, 30)
IF_COLOR = (112, 60, 96)
THEN_COLOR = (120, 210, 120)
ELSE_COLOR = (230, 120, 120)
GROUP_COLOR = (24, 96, 112)
MEMBER_COLOR = (110, 210, 200)
RULE_COLOR = (240, 170, 90)
RELAY_COLOR = (64, 68, 82)
LANE_COLOR = (190, 196, 210)
WIRELESS_COLOR = (44, 84, 92)
BROKEN_COLOR = (70, 72, 78)
FADED = 0.22                # opacity of the other blocks while a wireless pair is selected
CHECK_STYLE = ("QCheckBox {{ color: {fg}; background: transparent; font-size: 9pt; {extra} }}"
               "QCheckBox::indicator {{ width: 11px; height: 11px; border: 1px solid #B8B8B8; border-radius: 3px;"
               " background: rgba(0,0,0,0.25); }}"
               "QCheckBox::indicator:checked {{ background: #6CCB5F; border-color: #6CCB5F; }}")


PALETTE = []         # (hex, name): block colours offered by the swatch menu


@on_language_change
def _tr_palette():
    PALETTE[:] = [("#e5534b", _t("Red")), ("#e8912d", _t("Orange")), ("#d9b92f", _t("Yellow")),
                  ("#57ab5a", _t("Green")), ("#39c5bb", _t("Teal")), ("#4f8ef7", _t("Blue")),
                  ("#7c6cf2", _t("Indigo")), ("#b76de0", _t("Purple")), ("#e26ba5", _t("Pink")),
                  ("#9aa0a6", _t("Gray"))]


def color_name(color):
    """Palette name of a colour ('Default' for ''), else its #rrggbb."""
    if not color:
        return _t("Default")
    return next((name for hexc, name in PALETTE if hexc == color), color)


def _rgba(color):
    """'#rrggbb' -> (r, g, b, 255); '' -> the default link colour."""
    if not color:
        return tuple(PipeEnum.COLOR.value)
    c = QColor(color)
    return (c.red(), c.green(), c.blue(), 255)


NEW_INBOUND = []


@on_language_change
def _tr_new_inbound():
    NEW_INBOUND[:] = [(t, label) for t, label in INBOUND_TYPES.items()]


DEFAULT_PORTS = {"socks5": 1080, "socks4": 1084, "http": 8080, "https": 8443, "shadowsocks": 8388, "hysteria2": 8443,
                 "wireguard": 51820, "forward": 2222,
                 "tun": 30300}       # (a TUN card has no port; any free number keeps the port checks quiet)


class InfoWidget(NodeBaseWidget):
    """Read-only text inside a block (type, address, state)."""

    def __init__(self, parent=None, lines=2):
        super().__init__(parent, name="info", label="")
        label = QLabel()
        label.setTextFormat(Qt.RichText)
        label.setWordWrap(True)
        label.setFixedWidth(230)
        label.setFixedHeight(lines * 17 + 6)
        label.setAlignment(Qt.AlignLeft | Qt.AlignTop)
        label.setStyleSheet("QLabel { color: #DADADA; background: transparent; font-size: 9pt; }")
        self.set_custom_widget(label)

    def get_value(self):
        return self.get_custom_widget().text()

    def set_value(self, value):
        self.get_custom_widget().setText(value)


class SwitchesWidget(NodeBaseWidget):
    """Checkboxes inside an inbound block: run, start with the app, restart automatically."""

    def __init__(self, parent=None):
        super().__init__(parent, name="switches", label="")
        from PySide6.QtWidgets import QCheckBox
        box = QWidget()
        box.setAttribute(Qt.WA_TranslucentBackground)
        lay = QHBoxLayout(box)
        lay.setContentsMargins(0, 2, 0, 0)
        lay.setSpacing(10)
        self.run = QCheckBox(_t("Run"))
        self.auto = QCheckBox(_t("At start"))
        self.restart = QCheckBox(_t("Auto-restart"))
        self.auto.setToolTip(_t("Start with the app"))
        self.restart.setToolTip(_t("Restart automatically"))
        for cb in (self.run, self.auto, self.restart):
            cb.setStyleSheet(CHECK_STYLE.format(fg="#DADADA", extra=""))
            lay.addWidget(cb)
        lay.addStretch(1)
        box.setFixedWidth(230)
        self.set_custom_widget(box)

    def get_value(self):
        return ""

    def set_value(self, value):
        pass

    def show(self, running, autostart, auto_restart):
        for cb, v in ((self.run, running), (self.auto, autostart), (self.restart, auto_restart)):
            cb.blockSignals(True)
            cb.setChecked(bool(v))
            cb.blockSignals(False)


class ToggleWidget(NodeBaseWidget):
    """[On] [Off] buttons inside an IF block in switch mode: the lit one is the side traffic takes
    (on -> 'then', off -> 'else'). Clicking calls on_set(on)."""
    ON_FILL, OFF_FILL = "#3FA34D", "#C9463F"

    def __init__(self, parent=None):
        super().__init__(parent, name="toggle", label="")
        from PySide6.QtWidgets import QPushButton
        box = QWidget()
        box.setAttribute(Qt.WA_TranslucentBackground)
        lay = QHBoxLayout(box)
        lay.setContentsMargins(0, 3, 0, 3)
        lay.setSpacing(6)
        self.btn_on = QPushButton(_t("Switched on"))
        self.btn_off = QPushButton(_t("Switched off"))
        self.btn_on.setToolTip(_t("Send everything to 'then'"))
        self.btn_off.setToolTip(_t("Send everything to 'else'"))
        for b in (self.btn_on, self.btn_off):
            b.setCursor(Qt.PointingHandCursor)
            b.setFixedHeight(26)
            lay.addWidget(b, 1)
        box.setFixedWidth(230)
        self.set_custom_widget(box)
        self.on = False
        self.on_set = None
        self.btn_on.clicked.connect(lambda: self._pick(True))
        self.btn_off.clicked.connect(lambda: self._pick(False))
        self.show(False)

    def get_value(self):
        return ""

    def set_value(self, value):
        pass

    @staticmethod
    def _style(fill):
        if fill:            # the side in use: filled
            return (f"QPushButton {{ background: {fill}; color: white; border: 1px solid {fill}; border-radius: 5px;"
                    " font-size: 9pt; font-weight: bold; }")
        return ("QPushButton { background: rgba(0,0,0,0.22); color: #A8A8A8; border: 1px solid rgba(255,255,255,0.22);"
                " border-radius: 5px; font-size: 9pt; }"
                "QPushButton:hover { color: #F0F0F0; border-color: rgba(255,255,255,0.55); }")

    def _pick(self, on):
        if on == self.on:
            return
        self.show(on)
        cb = self.on_set
        if cb:
            QTimer.singleShot(0, lambda: cb(on))      # (the canvas may be redrawn: leave the click first)

    def show(self, on):
        self.on = bool(on)
        self.btn_on.setStyleSheet(self._style(self.ON_FILL if self.on else None))
        self.btn_off.setStyleSheet(self._style(None if self.on else self.OFF_FILL))


class BlockItem(NodeItem):
    """Node item with a small colour swatch at the right of the title bar; clicking it calls on_swatch(screen pos).
    out_swatches puts one swatch per output there instead (an IF: then / else), left to right."""
    SWATCH = 12.0

    def __init__(self, name="node", parent=None):
        super().__init__(name, parent)
        self.swatch = ""            # the block's own colour ('#rrggbb') or '' (automatic)
        self.swatch_fill = ""       # the colour it really uses (own, or followed from incoming links)
        self.swatch_mixed = False   # swatch_fill is a mix of incoming colours
        self.on_swatch = None
        self.out_swatches = []      # [{"key", "own", "fill", "mixed", "on_click", "tip"}] (instead of the block swatch)
        self.badge = None           # (text, "#rrggbb", tooltip): a small label left of the swatches (latency)

    def swatch_rect(self, k=0):
        """The k-th swatch from the right."""
        return QRectF(self._width - (self.SWATCH + 5) * (k + 1) - 2, 5, self.SWATCH, self.SWATCH)

    def swatches(self):
        """[(rect, own, fill, mixed, callback, tooltip)] of every swatch on the title bar."""
        if self.out_swatches:
            n = len(self.out_swatches)
            return [(self.swatch_rect(n - 1 - i), x["own"], x["fill"], x["mixed"], x["on_click"], x.get("tip", ""))
                    for i, x in enumerate(self.out_swatches)]
        if self.on_swatch is None:
            return []
        return [(self.swatch_rect(), self.swatch, self.swatch_fill, self.swatch_mixed, self.on_swatch, "")]

    BADGE_FONT = 7.5

    def badge_rect(self):
        if not self.badge:
            return None
        font = QFont(self.scene().font() if self.scene() else QFont())
        font.setPointSizeF(self.BADGE_FONT)
        w = QFontMetrics(font).horizontalAdvance(self.badge[0]) + 10
        right = self._width - (self.SWATCH + 5) * len(self.swatches()) - 6
        return QRectF(right - w, 4.5, w, 14)

    def _paint_badge(self, painter):
        r = self.badge_rect()
        if r is None:
            return
        text, color = self.badge[0], QColor(self.badge[1])
        painter.save()
        painter.setRenderHint(QPainter.Antialiasing, True)
        fill = QColor(color)
        fill.setAlpha(60)
        painter.setBrush(fill)
        painter.setPen(QPen(color, 1.0))
        painter.drawRoundedRect(r, 7, 7)
        font = painter.font()
        font.setPointSizeF(self.BADGE_FONT)
        font.setBold(True)
        painter.setFont(font)
        painter.setPen(QColor(255, 255, 255))
        painter.drawText(r, Qt.AlignCenter, text)
        painter.restore()

    def paint(self, painter, option, widget):
        super().paint(painter, option, widget)
        self._paint_badge(painter)
        sw = self.swatches()
        if not sw:
            return
        painter.save()
        painter.setRenderHint(QPainter.Antialiasing, True)
        for r, own, fill, mixed, _, _ in sw:
            painter.setBrush(QColor(*_rgba(own or fill)))
            # own colour: solid border; automatic (followed): dotted border
            painter.setPen(QPen(QColor(255, 255, 255, 200 if own else 130), 1.0, Qt.SolidLine if own else Qt.DotLine))
            painter.drawRoundedRect(r, 2.5, 2.5)
            if mixed and not own:                       # a mix: a small diagonal mark
                painter.setPen(QPen(QColor(255, 255, 255, 170), 1.0))
                painter.drawLine(r.bottomLeft() + QPointF(2.5, -2.5), r.topRight() + QPointF(-2.5, 2.5))
        painter.restore()

    def swatch_at(self, pos):
        return next((x for x in self.swatches() if x[0].contains(pos)), None)

    def mousePressEvent(self, event):
        hit = self.swatch_at(event.pos()) if event.button() == Qt.LeftButton else None
        if hit is not None:
            event.accept()
            cb, at = hit[4], event.screenPos()
            QTimer.singleShot(0, lambda: cb(at))
            return
        super().mousePressEvent(event)

    def hoverMoveEvent(self, event):
        hit = self.swatch_at(event.pos())
        from PySide6.QtWidgets import QToolTip
        if hit is not None and hit[5]:
            QToolTip.showText(event.screenPos(), hit[5])
        elif self.badge and self.badge[2] and self.badge_rect().contains(event.pos()):
            QToolTip.showText(event.screenPos(), self.badge[2])
        super().hoverMoveEvent(event)


class InNode(BaseNode):
    __identifier__ = "proxydock"
    NODE_NAME = "Inbound"

    def __init__(self):
        super().__init__(BlockItem)
        self.add_input(_t("relay in"), multi_input=True, color=PORT_COLOR)
        self.add_output(_t("out"), multi_output=True, color=PORT_COLOR)
        self.add_custom_widget(InfoWidget(self.view, lines=3))
        self.add_custom_widget(SwitchesWidget(self.view))


class OutNode(BaseNode):
    """Outbound that can go through another outbound ("via" output)."""
    __identifier__ = "proxydock"
    NODE_NAME = "Outbound"

    def __init__(self):
        super().__init__(BlockItem)
        self.add_input(_t("in"), multi_input=True, color=PORT_COLOR)
        self.add_output(_t("via"), multi_output=False, color=VIA_COLOR)
        self.add_custom_widget(InfoWidget(self.view, lines=3))


class EndNode(BaseNode):
    """Outbound that ends a chain: Direct, Hysteria2 (QUIC cannot be tunnelled)."""
    __identifier__ = "proxydock"
    NODE_NAME = "Outbound "

    def __init__(self):
        super().__init__(BlockItem)
        self.add_input(_t("in"), multi_input=True, color=PORT_COLOR)
        self.add_custom_widget(InfoWidget(self.view, lines=3))


class GroupNode(BaseNode):
    """Group outbound: the 'members' output links to the outbound blocks it uses."""
    __identifier__ = "proxydock"
    NODE_NAME = "Group"

    def __init__(self):
        super().__init__(BlockItem)
        self.add_input(_t("in"), multi_input=True, color=PORT_COLOR)
        self.add_output(_t("members"), multi_output=True, color=MEMBER_COLOR)
        self.add_custom_widget(InfoWidget(self.view, lines=3))


class RouterNode(BaseNode):
    """Router: one output per rule plus 'default' (added when the canvas is built)."""
    __identifier__ = "proxydock"
    NODE_NAME = "Router"

    def __init__(self):
        super().__init__(BlockItem)
        self.add_input(_t("in"), multi_input=True, color=PORT_COLOR)
        self.add_custom_widget(InfoWidget(self.view, lines=3))


class RelayItem(BlockItem):
    """Pass-through block: lanes on both sides (with optional empty rows above a lane), the inside wiring drawn
    between them, and a footer to click for one more lane."""
    FOOT = 22.0
    MIN_W = 150.0
    # inner lane dots: this far inside the block from the side ports - past a port's click area (half of
    # PortEnum.SIZE + CLICK_FALLOFF), so pressing a dot starts an inside wire, not a link out of the port
    INSET = (PortEnum.SIZE.value + PortEnum.CLICK_FALLOFF.value) / 2 + 12

    def __init__(self, name="node", parent=None):
        super().__init__(name, parent)
        self.in_gaps, self.out_gaps = [], []
        self.links = []         # [(input index, output index)] wired inside
        self.link_colors = []   # colour per input lane ('' = default) for the inside wiring
        self.link_mixed = []    # per input lane: True = a mix of merging colours (drawn dashed)
        self.routed = False
        self.on_add = None      # callback: footer clicked
        self.on_wire = None     # callback(input index, output index or None): wired inside by dragging (routed)
        self.footer_text = ""
        self.drag = None        # (side "in"/"out", lane index, current point) while dragging a wire inside
        self.setAcceptHoverEvents(True)

    @staticmethod
    def _step():
        return PortEnum.SIZE.value + 1

    def _rows(self):
        return max(len(self.inputs) + sum(self.in_gaps[:len(self.inputs)]),
                   len(self.outputs) + sum(self.out_gaps[:len(self.outputs)]), 1)

    def _calc_size_horizontal(self):
        w, _ = super()._calc_size_horizontal()
        return max(w, self.MIN_W), self._rows() * self._step() + self.FOOT

    def _align_ports_horizontal(self, v_offset):
        for ports, gaps, right in ((self.inputs, self.in_gaps, False), (self.outputs, self.out_gaps, True)):
            y = v_offset
            for i, port in enumerate(ports):
                y += (gaps[i] if i < len(gaps) else 0) * self._step()
                pw = port.boundingRect().width()
                port.setPos(self._width - pw / 2 if right else -pw / 2, y)
                y += self._step()

    def _center(self, port):
        return port.mapToItem(self, port.boundingRect().center())

    def lane_points(self):
        """([inner input dot], [inner output dot]) in item coordinates."""
        ins = [self._center(p) + QPointF(self.INSET, 0) for p in self.inputs]
        outs = [self._center(p) - QPointF(self.INSET, 0) for p in self.outputs]
        return ins, outs

    def footer_rect(self):
        r = self.boundingRect()
        return QRectF(r.left() + 4, r.bottom() - self.FOOT, r.width() - 8, self.FOOT - 3)

    def paint(self, painter, option, widget):
        super().paint(painter, option, widget)
        painter.save()
        painter.setRenderHint(QPainter.Antialiasing, True)
        ins, outs = self.lane_points()
        lane = QColor(*LANE_COLOR)
        for i, o in self.links:
            if i < len(ins) and o < len(outs):
                c = self.link_colors[i] if i < len(self.link_colors) else ""
                color = QColor(c) if c else lane
                mixed = i < len(self.link_mixed) and self.link_mixed[i]
                painter.setPen(QPen(color, 1.6, Qt.DashLine if mixed else Qt.SolidLine))
                a, b = ins[i], outs[o]
                path = QPainterPath(a)
                path.lineTo(b if self.routed else b - QPointF(5, 0))
                painter.setBrush(Qt.NoBrush)
                painter.drawPath(path)
                if not self.routed:         # arrow head
                    painter.setBrush(color)
                    painter.drawPolygon(QPolygonF([b, b + QPointF(-7, -4), b + QPointF(-7, 4)]))
        if self.routed:                     # inner dots (the ends of the inside wiring); unwired inputs in amber
            wired = {i for i, _ in self.links}
            for k, pt in enumerate(ins):
                painter.setPen(QPen(lane if k in wired else QColor(232, 179, 57), 1.4))
                painter.setBrush(QColor(*RELAY_COLOR))
                painter.drawEllipse(pt, 4.0, 4.0)
            painter.setPen(QPen(lane, 1.4))
            for pt in outs:
                painter.drawEllipse(pt, 4.0, 4.0)
        self._paint_extra(painter, ins, outs)
        f = self.footer_rect()
        painter.setPen(QPen(QColor(255, 255, 255, 60), 1, Qt.DashLine))
        painter.setBrush(Qt.NoBrush)
        painter.drawRoundedRect(f, 3, 3)
        painter.setPen(QColor(200, 200, 200))
        font = painter.font()
        font.setPointSizeF(8)
        painter.setFont(font)
        painter.drawText(f, Qt.AlignCenter, self.footer_text)
        painter.restore()

    def _paint_extra(self, painter, ins, outs):
        """The wire being dragged inside the block, and the dot it would land on."""
        if not self.drag:
            return
        side, k, pt = self.drag
        start = (ins if side == "in" else outs)[k]
        hit = self.dot_at(pt, "out" if side == "in" else "in")
        end = (outs if side == "in" else ins)[hit] if hit is not None else pt
        painter.setPen(QPen(QColor(108, 203, 95), 2.0, Qt.DashLine if hit is None else Qt.SolidLine))
        painter.setBrush(Qt.NoBrush)
        painter.drawLine(start, end)
        if hit is not None:
            painter.setBrush(QColor(108, 203, 95))
            painter.drawEllipse(end, 5.0, 5.0)

    DOT_HIT = 8.0

    def dot_at(self, pos, side=None):
        """Index of the inner lane dot at pos (on side "in"/"out", or either: returns (side, index)), else None."""
        if not self.routed:
            return None
        ins, outs = self.lane_points()
        for s, pts in (("in", ins), ("out", outs)):
            if side not in (None, s):
                continue
            for k, pt in enumerate(pts):
                if (pt - pos).manhattanLength() <= self.DOT_HIT * 1.4:
                    return (s, k) if side is None else k
        return None

    def mousePressEvent(self, event):
        if event.button() == Qt.LeftButton and self.on_add and self.footer_rect().contains(event.pos()):
            event.accept()
            QTimer.singleShot(0, self.on_add)
            return
        if event.button() == Qt.LeftButton and self.on_wire:
            hit = self.dot_at(event.pos())
            if hit is not None:             # start a wire from an inner dot (instead of moving the block)
                self.drag = (hit[0], hit[1], event.pos())
                event.accept()
                self.update()
                return
        super().mousePressEvent(event)

    def mouseMoveEvent(self, event):
        if self.drag:
            self.drag = (self.drag[0], self.drag[1], event.pos())
            event.accept()
            self.update()
            return
        super().mouseMoveEvent(event)

    def mouseReleaseEvent(self, event):
        if self.drag:
            side, k, start_pt = self.drag
            self.drag = None
            event.accept()
            self.update()
            hit = self.dot_at(event.pos(), "out" if side == "in" else "in")
            ins, outs = self.lane_points()
            origin = (ins if side == "in" else outs)[k]
            if hit is None and (event.pos() - origin).manhattanLength() < self.DOT_HIT * 2:
                return                      # a click on a dot without dragging: nothing changes
            if side == "in":
                wire = (k, hit)             # dropped on nothing: input k is unwired
            elif hit is None:
                return                      # from an output to nowhere: nothing to change
            else:
                wire = (hit, k)
            cb = self.on_wire
            QTimer.singleShot(0, lambda: cb(*wire))     # (the canvas is rebuilt: leave this event first)
            return
        super().mouseReleaseEvent(event)

    def hoverMoveEvent(self, event):
        self.setCursor(Qt.CrossCursor if self.dot_at(event.pos()) is not None else Qt.ArrowCursor)
        super().hoverMoveEvent(event)

    def hoverLeaveEvent(self, event):
        self.unsetCursor()
        super().hoverLeaveEvent(event)


class RelayNode(BaseNode):
    """Pass-through: lanes are added when the canvas is built."""
    __identifier__ = "proxydock"
    NODE_NAME = "Pass-through"

    def __init__(self):
        super().__init__(RelayItem)


class NumberEdit(QGraphicsTextItem):
    """Inline editor over a wireless block's number: Enter or clicking away applies, Esc cancels."""

    def __init__(self, text, font, rect, on_done, parent):
        super().__init__(text, parent)
        self.on_done = on_done
        self.finished = False
        self.setFont(font)
        self.setDefaultTextColor(QColor(255, 255, 255))
        self.setTextInteractionFlags(Qt.TextEditorInteraction)
        self.document().setDocumentMargin(1)
        self.setTextWidth(rect.width())
        self.document().setDefaultTextOption(QTextOption(Qt.AlignCenter))
        self.setPos(rect.left(), rect.center().y() - self.boundingRect().height() / 2)
        self.setZValue(5)

    def start(self):
        self.setFocus(Qt.MouseFocusReason)
        cursor = self.textCursor()
        cursor.select(QTextCursor.Document)
        self.setTextCursor(cursor)

    def paint(self, painter, option, widget):
        painter.save()
        painter.setPen(QPen(QColor(108, 203, 95), 1.4))
        painter.setBrush(QColor(0, 0, 0, 110))
        painter.drawRoundedRect(self.boundingRect(), 3, 3)
        painter.restore()
        super().paint(painter, option, widget)

    def _finish(self, apply):
        if self.finished:
            return
        self.finished = True
        text = self.toPlainText().strip().lstrip("#").strip()
        cb = self.on_done
        QTimer.singleShot(0, lambda: cb(text if apply else None))      # (the canvas may be redrawn: leave first)

    def keyPressEvent(self, event):
        if event.key() in (Qt.Key_Return, Qt.Key_Enter):
            self._finish(True)
            return
        if event.key() == Qt.Key_Escape:
            self._finish(False)
            return
        super().keyPressEvent(event)

    def focusOutEvent(self, event):
        super().focusOutEvent(event)
        self._finish(True)


class WirelessItem(BlockItem):
    """One half of a wireless pair: its title on the title bar, the pair's number in large type (double-click it to
    change it) and, under it, where the traffic comes from or goes. A half whose partner was deleted is drawn greyed
    out with a dashed amber border."""
    MIN_W, MIN_H = 170.0, 76.0

    def __init__(self, name="node", parent=None):
        super().__init__(name, parent)
        self.pair_name = ""         # "#3"
        self.info_text = ""
        self.broken = False
        self.on_moved = None        # callback while the block is dragged (the pair's arc follows)
        self.on_number = None       # callback(text, or None if cancelled) after the number was edited
        self.number_edit = None
        self.setFlag(QGraphicsItem.ItemSendsGeometryChanges, True)

    def _calc_size_horizontal(self):
        w, h = super()._calc_size_horizontal()
        return max(w, self.MIN_W), max(h, self.MIN_H)

    def _body(self):
        r = self.boundingRect()
        return QRectF(r.left() + 14, r.top() + 22, r.width() - 28, r.height() - 26)

    def number_rect(self):
        b = self._body()
        return QRectF(b.left() + b.width() * 0.25, b.top(), b.width() * 0.5, 28)

    def _number_font(self):
        font = self.scene().font() if self.scene() else QFont()
        font.setPointSizeF(15)
        font.setBold(True)
        return font

    def paint(self, painter, option, widget):
        super().paint(painter, option, widget)
        painter.save()
        painter.setRenderHint(QPainter.Antialiasing, True)
        r = self.boundingRect()
        body = self._body()
        font = painter.font()
        font.setPointSizeF(15)
        font.setBold(True)
        painter.setFont(font)
        painter.setPen(QColor(150, 150, 150) if self.broken else QColor(240, 240, 240))
        if self.number_edit is None:
            painter.drawText(QRectF(body.left(), body.top(), body.width(), 28), Qt.AlignCenter, self.pair_name)
        font.setPointSizeF(8)
        font.setBold(False)
        painter.setFont(font)
        painter.setPen(QColor(232, 179, 57) if self.broken else QColor(205, 205, 205))
        text = QFontMetrics(font).elidedText(self.info_text, Qt.ElideRight, int(body.width()))
        painter.drawText(QRectF(body.left(), body.top() + 28, body.width(), 18), Qt.AlignCenter, text)
        if self.broken:
            painter.setPen(QPen(QColor(232, 179, 57), 1.4, Qt.DashLine))
            painter.setBrush(Qt.NoBrush)
            painter.drawRoundedRect(r.adjusted(1, 1, -1, -1), 4, 4)
        painter.restore()

    def itemChange(self, change, value):
        out = super().itemChange(change, value)
        if change == QGraphicsItem.ItemPositionHasChanged and self.on_moved:
            self.on_moved()
        return out

    def mouseDoubleClickEvent(self, event):
        if event.button() == Qt.LeftButton and self.on_number and self.number_rect().contains(event.pos()):
            event.accept()
            self.edit_number()
            return
        super().mouseDoubleClickEvent(event)

    def edit_number(self):
        """Open the inline editor over the number."""
        if self.number_edit is not None or self.on_number is None:
            return

        def done(text):
            edit, self.number_edit = self.number_edit, None
            if edit is not None and edit.scene() is not None:
                edit.scene().removeItem(edit)
            self.update()
            if self.on_number:
                self.on_number(text)
        self.number_edit = NumberEdit(self.pair_name.lstrip("#"), self._number_font(), self.number_rect(), done, self)
        self.update()
        self.number_edit.start()


class WirelessInNode(BaseNode):
    """Input half of a wireless pair (its port is added when the canvas is built)."""
    __identifier__ = "proxydock"
    NODE_NAME = "Wireless in"

    def __init__(self):
        super().__init__(WirelessItem)


class WirelessOutNode(BaseNode):
    """Output half of a wireless pair."""
    __identifier__ = "proxydock"
    NODE_NAME = "Wireless out"

    def __init__(self):
        super().__init__(WirelessItem)


def split_mid(mid):
    """(model id, half): a wireless pair's two blocks are '<relay id>~in' / '<relay id>~out' on the canvas
    (half "in"/"out"); every other block is its model id (half None)."""
    if isinstance(mid, str) and mid.endswith(("~in", "~out")):
        rid, half = mid.rsplit("~", 1)
        return rid, half
    return mid, None


class PcNode(BaseNode):
    __identifier__ = "proxydock"
    NODE_NAME = "PC"

    def __init__(self):
        super().__init__()
        self.add_input(_t("in"), multi_input=False, color=PORT_COLOR)
        self.add_custom_widget(InfoWidget(self.view, lines=3))


def _esc(text):
    return (text or "").replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def _line(color, text):
    return f'<span style="color:{color}">{_esc(text)}</span>'


NAV_KEYS = (Qt.Key_Left, Qt.Key_Right, Qt.Key_Up, Qt.Key_Down)


class AddButton(TransparentDropDownPushButton):
    """Toolbar 'Add ...' drop-down that can shrink to its icon (compact); the button places its icon from its
    minimum width, so a compact one reports the compact width."""
    COMPACT_W = 52
    compact = False
    full_text = None
    # (no __init__ of its own: qfluentwidgets buttons call __init__ again with other arguments)

    def set_compact(self, compact):
        if self.full_text is None:
            self.full_text = self.text()
            self.setToolTip(self.full_text)
        self.compact = compact
        self.setText("" if compact else self.full_text)
        if compact:
            self.setFixedWidth(self.COMPACT_W)
        else:
            self.setMinimumWidth(0)
            self.setMaximumWidth(16777215)
            self.adjustSize()

    def minimumSizeHint(self):
        if self.compact:
            return QSize(self.COMPACT_W, super().minimumSizeHint().height())
        return super().minimumSizeHint()


class _ViewerFilter(QObject):
    """Right-click on the canvas: our own (Fluent) menu instead of NodeGraphQt's. Arrow keys walk the links."""

    def __init__(self, page):
        super().__init__(page)
        self.page = page

    def eventFilter(self, obj, event):
        if event.type() == QEvent.ContextMenu:
            self.page.context_menu(event.pos(), event.globalPos())
            return True
        if event.type() == QEvent.KeyPress and event.key() in NAV_KEYS and event.modifiers() in (
                Qt.NoModifier, Qt.KeypadModifier):
            if self._typing(obj):
                return False        # editing a title or a text field inside a block: its own keys
            return self.page.nav_key(event.key())
        return False

    @staticmethod
    def _typing(viewer):
        """True while a block title or a text input inside a block is being edited. (Clicking a block's body
        only focuses its read-only info text: the arrows still walk the links then.)"""
        from PySide6.QtWidgets import (
            QAbstractSpinBox, QComboBox, QGraphicsProxyWidget, QGraphicsTextItem, QLineEdit, QPlainTextEdit, QTextEdit,
        )
        focus = viewer.scene().focusItem() if viewer.scene() else None
        if isinstance(focus, QGraphicsTextItem):
            return bool(focus.textInteractionFlags() & Qt.TextEditable)
        if isinstance(focus, QGraphicsProxyWidget) and focus.widget() is not None:
            w = focus.widget().focusWidget() or focus.widget()
            return isinstance(w, (QLineEdit, QAbstractSpinBox, QComboBox, QTextEdit, QPlainTextEdit))
        return False


class CanvasInterface(QWidget):
    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        self.setObjectName("canvasInterface")
        self.ctl = ctl
        self.syncing = False
        self.sig = None
        self.fitted = False
        self.node_of = {}           # model id -> NodeGraphQt node
        self.mid_of = {}            # NodeGraphQt node id -> model id

        self.graph = NodeGraph()
        self.graph.register_nodes([InNode, OutNode, EndNode, GroupNode, RouterNode, RelayNode, WirelessInNode,
                                   WirelessOutNode, PcNode])
        self.rule_of = {}           # (NodeGraphQt node id, output port name) -> rule id or "default"
        self.in_lane_of = {}        # (NodeGraphQt node id, input port name) -> pass-through input lane id
        self.out_lane_of = {}       # (NodeGraphQt node id, output port name) -> pass-through output lane id
        self.lane_port = {}         # pass-through input lane id -> its NodeGraphQt input port
        # cycles between blocks are fine (a pass-through can sit before and after a router); the controller
        # refuses links that really loop
        self.graph.set_acyclic(False)
        self.graph.disable_context_menu(True)
        self.graph.port_connected.connect(self._on_connected)
        self.graph.port_disconnected.connect(self._on_disconnected)
        self.graph.node_double_clicked.connect(self._on_double_clicked)
        self.graph.node_selection_changed.connect(lambda *_: self._on_selection())
        self.graph.viewer().node_name_changed.connect(self._on_title_edited)
        self._focus = None          # wireless pair shown (the rest faded, an arc between its halves)
        self._arc = None
        self._nav = None            # ([NodeGraphQt node id, ...], index): the choices of the last arrow step
        self._came_from = {}        # NodeGraphQt node id -> the node an arrow step came from (going back returns there)
        viewer = self.graph.viewer()
        viewer.moved_nodes.connect(self._on_moved)
        self._filter = _ViewerFilter(self)
        viewer.installEventFilter(self._filter)
        QShortcut(QKeySequence.Delete, viewer, activated=self.delete_selected)

        title = TitleLabel(_t("Canvas"), self)
        # "Add ..." drop-downs: the first group of the toolbar
        self.btn_in = AddButton(FIF.ADD, _t("Add inbound"), self)
        self.btn_in.setMenu(self._inbound_menu())
        self.btn_out = AddButton(FIF.SEND, _t("Add outbound"), self)
        self.btn_out.setMenu(self._outbound_menu())
        self.btn_router = AddButton(FIF.SHARE, _t("Add router"), self)
        self.btn_router.setMenu(self._router_menu())
        self._build_toolbar()
        # Ctrl+Y and Ctrl+Shift+Z redo on every OS (the platform's Redo is one of them: Cmd+Shift+Z on macOS).
        # A key bound twice would be ambiguous and do nothing, so each key is bound once.
        seen = set()
        for keys, fn in ((QKeySequence.Undo, ctl.undo), (QKeySequence.Redo, ctl.redo), (QKeySequence("Ctrl+Y"), ctl.redo),
                         (QKeySequence("Ctrl+Shift+Z"), ctl.redo), (QKeySequence("F2"), self.rename_selected),
                         (QKeySequence("Ctrl+D"), self.duplicate_selected)):
            seq = QKeySequence(keys)
            if seq.toString() in seen:
                continue
            seen.add(seq.toString())
            QShortcut(seq, self, activated=lambda fn=fn, k=seq: self._undo_key(fn, k),
                      context=Qt.WidgetWithChildrenShortcut)
        hint = CaptionLabel(_t("Drag from an output to an input to link blocks. Double-click a block to edit it."), self)
        hint.setWordWrap(True)
        self.pc_bar = PcBar(ctl, self)

        header = QHBoxLayout()
        header.addWidget(title)
        header.addStretch(1)
        header.addWidget(self.status_label, 0, Qt.AlignBottom | Qt.AlignRight)     # what is selected / progress
        self.sync_undo(ctl.can_undo(), ctl.can_redo())
        root = QVBoxLayout(self)
        root.setContentsMargins(36, 20, 36, 20)
        root.setSpacing(10)
        root.addLayout(header)
        root.addWidget(hint)
        root.addWidget(self.pc_bar)
        root.addWidget(self.toolbar)
        root.addWidget(self.graph.widget, 1)
        self.sync_toolbar()
        self._apply_theme()
        qconfig.themeChanged.connect(self._on_theme_changed)

    def _on_theme_changed(self, *_):
        self._apply_theme()
        self._redo_icon()

    # --- menus ---
    def _inbound_menu(self, pos=None, parent=None):
        menu = RoundMenu(_t("Add inbound"), parent or self)
        for t, label in NEW_INBOUND:
            act = Action(label, menu)
            act.triggered.connect(lambda _=False, t=t: self.add_inbound(t, pos))
            menu.addAction(act)
        return menu

    def _router_menu(self, pos=None, parent=None):
        menu = RoundMenu(_t("Add router"), parent or self)
        a = Action(FIF.SHARE, _t("Router (several rules)"), menu)
        a.triggered.connect(lambda: self.ctl.new_router(pos))
        b = Action(FIF.QUESTION, _t("IF (one condition: then / else)"), menu)
        b.triggered.connect(lambda: self.ctl.new_router(pos, style="if"))
        c = Action(FIF.ALIGNMENT, _t("Pass-through (tidy up the wiring)"), menu)
        c.triggered.connect(lambda: self.ctl.new_relay(pos))
        w = Action(FIF.WIFI, _t("Wireless pair (linked without a wire)"), menu)
        w.triggered.connect(lambda: self.ctl.new_wireless(pos))
        menu.addActions([a, b, c, w])
        return menu

    def _outbound_menu(self, pos=None, parent=None):
        menu = RoundMenu(_t("Add outbound"), parent or self)
        link = Action(FIF.PASTE, _t("From a share link (clipboard)"), menu)
        link.triggered.connect(lambda: self.add_from_link(pos))
        menu.addAction(link)
        menu.addSeparator()
        for t, label in OUTBOUND_NEW:
            act = Action(label, menu)
            act.triggered.connect(lambda _=False, t=t: self.ctl.new_outbound({"type": t}, pos))
            menu.addAction(act)
        return menu

    def add_from_link(self, pos=None):
        from PySide6.QtWidgets import QApplication
        from ..links import parse
        try:
            fields, name = parse(QApplication.clipboard().text())
        except ValueError as e:
            info(self.window(), "error", _t("Cannot use the clipboard"),
                 f"{e}\n" + _t("Copy a vless://, vmess://, trojan://, tuic://, ss:// or hysteria2:// link first"), 6000)
            return
        n = self.ctl.new_outbound(fields, pos, edit=False)
        self.ctl.rename_node(n["id"], name)
        self.ctl.edit_node(n["id"])

    def add_inbound(self, t, pos=None):
        host = "0.0.0.0" if t in ("shadowsocks", "hysteria2", "wireguard") else "127.0.0.1"
        self.ctl.new_inbound(INBOUND_TYPES[t], {"type": t, "host": host, "port": DEFAULT_PORTS[t]}, None, pos, edit=False)

    def context_menu(self, pos, global_pos):
        viewer = self.graph.viewer()
        scene_pos = viewer.mapToScene(pos)
        node, port_item = None, None
        for item in viewer.items(pos):
            if port_item is None and isinstance(item, PortItem):
                port_item = item
            while item is not None and not isinstance(item, AbstractNodeItem):
                item = item.parentItem()
            if item is not None:
                node = self.graph.get_node_by_id(item.id)
                break
        if node is not None and port_item is not None and (self._port_color_menu(node, port_item, global_pos)
                                                           or self._lane_menu(node, port_item, global_pos)):
            return
        menu = RoundMenu(parent=self)
        p = [scene_pos.x(), scene_pos.y()]
        if node is None:
            menu.addMenu(self._inbound_menu(p, menu))
            menu.addMenu(self._outbound_menu(p, menu))
            menu.addMenu(self._router_menu(p, menu))
            menu.addSeparator()
            fit = Action(FIF.ZOOM, _t("Fit"), menu)
            fit.triggered.connect(self.fit)
            lat = Action(FIF.SPEED_HIGH, _t("Test latency"), menu)
            lat.triggered.connect(lambda: self.ctl.test_latency())
            undo = Action(FIF.RETURN, _t("Undo"), menu)
            undo.triggered.connect(self.ctl.undo)
            undo.setEnabled(self.ctl.can_undo())
            redo = Action(self.btn_redo.icon(), _t("Redo"), menu)
            redo.triggered.connect(self.ctl.redo)
            redo.setEnabled(self.ctl.can_redo())
            menu.addActions([fit, lat])
            menu.addSeparator()
            menu.addActions([undo, redo])
        elif split_mid(self.mid_of.get(node.id))[1]:
            self._wireless_menu(menu, *split_mid(self.mid_of.get(node.id)))
        else:
            mid = self.mid_of.get(node.id)
            if mid == PC_ID:
                for icon, text, fn in ((FIF.HISTORY, _t("Restore previous"), self.ctl.pc_restore),
                                       (FIF.CANCEL, _t("Off (default)"), self.ctl.pc_off)):
                    a = Action(icon, text, menu)
                    a.triggered.connect(fn)
                    menu.addAction(a)
            else:
                proxy = self.ctl.proxy_by_id(mid)
                edit = Action(FIF.EDIT, _t("Edit"), menu)
                edit.triggered.connect(lambda: self.ctl.edit_node(mid))
                menu.addAction(edit)
                if proxy is not None:
                    running = self.ctl.manager.state(mid)[0] in ("running", "starting")
                    run = Action(FIF.PAUSE if running else FIF.PLAY, _t("Stop") if running else _t("Run"), menu)
                    run.triggered.connect(lambda: self.ctl.toggle_proxy(mid, not running))
                    copy_link = Action(FIF.COPY, _t("Copy address/link for clients"), menu)
                    copy_link.triggered.connect(lambda: self.ctl.window.proxies_page.copy_url(mid))
                    pc = Action(FIF.GLOBE, _t("Use as the PC proxy"), menu)
                    pc.triggered.connect(lambda: self.ctl.set_pc(mid))
                    menu.addActions([run, copy_link, pc])
                    if proxy.get("target"):
                        cut = Action(FIF.REMOVE, _t("Disconnect"), menu)
                        cut.triggered.connect(lambda: self.ctl.set_target(mid, None))
                        menu.addAction(cut)
                from .. import graph as graph_mod
                kind, gn = graph_mod.find(self.ctl.graph, mid)
                if kind == "relay":
                    add = Action(FIF.ADD, _t("Add a lane"), menu)
                    add.triggered.connect(lambda: self.ctl.add_lane(mid))
                    menu.addAction(add)
                    if gn["routed"]:
                        add_out = Action(FIF.ADD, _t("Add an output lane"), menu)
                        add_out.triggered.connect(lambda: self.ctl.add_lane(mid, "outputs"))
                        menu.addAction(add_out)
                    menu.addMenu(self._remove_lane_menu(gn, menu))
                if kind == "outbound" and gn["outbound"]["type"] != "block":
                    lat = Action(FIF.SPEED_HIGH, _t("Test latency"), menu)
                    lat.triggered.connect(lambda: self.ctl.test_latency([mid]))
                    menu.addAction(lat)
                if self.outputs_of(mid):
                    menu.addMenu(self._output_colors_menu(mid, menu))
                if kind == "outbound" and gn["via"]:
                    cut = Action(FIF.REMOVE, _t("Disconnect"), menu)
                    cut.triggered.connect(lambda: self.ctl.set_via(mid, None))
                    menu.addAction(cut)
                from .proxies import member_menu
                sub = member_menu(self.ctl, mid, menu)
                if sub is not None:
                    menu.addMenu(sub)
                dup = Action(FIF.COPY, _t("Duplicate"), menu)
                dup.triggered.connect(lambda: self.ctl.duplicate_node(mid))
                delete = Action(FIF.DELETE, _t("Delete"), menu)
                delete.triggered.connect(lambda: confirm_delete(self.ctl, [mid], self.window()))
                menu.addSeparator()
                menu.addActions([dup, delete])
        menu.exec(global_pos)

    def _wireless_menu(self, menu, rid, half):
        """Right-click on a half of a wireless pair."""
        from .. import graph as graph_mod
        _, r = graph_mod.find(self.ctl.graph, rid)
        if r is None:
            return
        edit = Action(FIF.EDIT, _t("Edit"), menu)
        edit.triggered.connect(lambda: self.ctl.edit_node(rid))
        menu.addAction(edit)
        if r.get("broken"):
            fix = Action(FIF.SYNC, _t("Recreate the partner"), menu)
            fix.triggered.connect(lambda: self.ctl.repair_wireless(rid))
            menu.addAction(fix)
        else:
            go = Action(FIF.WIFI, _t("Go to the partner"), menu)
            go.triggered.connect(lambda: self.go_to_partner(rid, half))
            menu.addAction(go)
        dup = Action(FIF.COPY, _t("Duplicate"), menu)
        dup.triggered.connect(lambda: self.ctl.duplicate_node(rid))
        delete = Action(FIF.DELETE, _t("Delete"), menu)
        delete.triggered.connect(lambda: confirm_delete(self.ctl, [], self.window(), [(rid, half)]))
        menu.addSeparator()
        menu.addActions([dup, delete])

    # --- keyboard: arrows follow the links ---
    def neighbours(self, node, forward):
        """Blocks linked out of node (forward) or into it, in port order (top to bottom within a port).
        The other half of a wireless pair counts as linked."""
        out, seen = [], {node.id}
        rid, half = split_mid(self.mid_of.get(node.id))
        if half == ("in" if forward else "out"):
            partner = self.node_of.get(f"{rid}~{'out' if forward else 'in'}")
            if partner is not None:
                out.append(partner)
                seen.add(partner.id)
        for port in (node.output_ports() if forward else node.input_ports()):
            for other in sorted(port.connected_ports(), key=lambda p: p.node().view.scenePos().y()):
                n = other.node()
                if n.id not in seen:
                    seen.add(n.id)
                    out.append(n)
        return out

    def nav_key(self, key):
        """With one block selected: → selects the block it links to, ← the one linking into it (back where the
        last step came from, if that is one of them); ↑/↓ switch to the other choices of that step, or else
        move to the nearest block above / below.
        Returns True if the key was used."""
        sel = self.graph.selected_nodes()
        if len(sel) != 1:
            return False
        cur = sel[0]
        if key in (Qt.Key_Left, Qt.Key_Right):
            options = self.neighbours(cur, key == Qt.Key_Right)
            back = self._came_from.get(cur.id)
            options.sort(key=lambda n: n.id != back)        # (stable: the rest keep their order)
            if options:
                self._nav = ([n.id for n in options], 0)
                self._came_from[options[0].id] = cur.id
                self._select(options[0])
            return True
        if not (self._nav and cur.id in self._nav[0] and len(self._nav[0]) > 1):
            near = self.nearest_block(cur, key == Qt.Key_Up)      # no choices to switch: the block above / below
            if near is None:
                return True
            self._nav = None
            self._select(near)
            return True
        if self._nav and cur.id in self._nav[0] and len(self._nav[0]) > 1:
            ids = self._nav[0]
            i = (ids.index(cur.id) + (1 if key == Qt.Key_Down else -1)) % len(ids)
            node = self.graph.get_node_by_id(ids[i])
            if node is not None:
                prev = self._came_from.get(cur.id)
                if prev:
                    self._came_from[node.id] = prev
                self._nav = (ids, i)
                self._select(node)
                return True
        return False

    def nearest_block(self, node, up):
        """The closest block above (up) or below node on the canvas; one in the same column wins over one beside."""
        here = node.view.sceneBoundingRect()
        best, best_score = None, None
        for other in self.graph.all_nodes():
            if other is node:
                continue
            r = other.view.sceneBoundingRect()
            dy = (here.top() - r.bottom()) if up else (r.top() - here.bottom())
            if dy < -here.height() / 2:            # not above / below
                continue
            overlap = min(here.right(), r.right()) - max(here.left(), r.left())
            dx = 0 if overlap > 0 else abs(r.center().x() - here.center().x())
            score = max(dy, 0) + 3 * dx
            if best_score is None or score < best_score:
                best, best_score = other, score
        return best

    def _select(self, node):
        """Select node only and bring it into view if it is off screen."""
        self.graph.clear_selection()
        node.set_selected(True)
        viewer = self.graph.viewer()
        seen = viewer.mapToScene(viewer.viewport().rect()).boundingRect()
        if not seen.contains(node.view.sceneBoundingRect()):
            self.graph.center_on([node])
        self._on_selection()

    def go_to_partner(self, rid, half):
        """Select the other half of wireless pair rid and centre the view on it."""
        other = self.node_of.get(f"{rid}~{'out' if half == 'in' else 'in'}")
        if other is None:
            return
        self.graph.clear_selection()
        other.set_selected(True)
        self.graph.center_on([other])
        self._on_selection()

    # --- wireless pair focus: the other blocks fade and an arc joins the two halves ---
    def _on_selection(self):
        from .. import graph as graph_mod
        self.sync_toolbar()
        pair = None
        for n in self.graph.selected_nodes():
            rid, half = split_mid(self.mid_of.get(n.id))
            if half:
                _, r = graph_mod.find(self.ctl.graph, rid)
                if r is not None and not r.get("broken"):
                    pair = rid
                break
        if pair != self._focus:
            self._set_focus(pair)

    def _set_focus(self, rid):
        self._clear_focus()
        a, b = (self.node_of.get(f"{rid}~in"), self.node_of.get(f"{rid}~out")) if rid else (None, None)
        if a is None or b is None:
            return
        self._focus = rid
        keep = {a.view, b.view}
        for node in self.graph.all_nodes():
            if node.view not in keep:
                node.view.setOpacity(FADED)
        for pipe in self.graph.viewer().all_pipes():
            ends = {p.node for p in (pipe.input_port, pipe.output_port) if p is not None}
            if not ends & keep:
                pipe.setOpacity(FADED)
        from .. import graph as graph_mod
        color = graph_mod.wire_colors(self.ctl.graph).get(graph_mod.find(self.ctl.graph, rid)[1]["outputs"][0]["id"], ("",))[0]
        self._arc = QGraphicsPathItem()
        self._arc.setPen(QPen(QColor(*_rgba(color)), 2.2, Qt.DashLine, Qt.RoundCap))
        self._arc.setZValue(10000)
        self._arc.setAcceptedMouseButtons(Qt.NoButton)
        self.graph.viewer().scene().addItem(self._arc)
        self._update_arc()

    def _update_arc(self):
        """Parabola from the right side of the input half to the left side of the output half."""
        if self._arc is None or self._focus is None:
            return
        a, b = self.node_of.get(f"{self._focus}~in"), self.node_of.get(f"{self._focus}~out")
        if a is None or b is None:
            return
        ra, rb = a.view.sceneBoundingRect(), b.view.sceneBoundingRect()
        p1, p2 = QPointF(ra.right(), ra.center().y()), QPointF(rb.left(), rb.center().y())
        dist = ((p2.x() - p1.x()) ** 2 + (p2.y() - p1.y()) ** 2) ** 0.5
        top = min(p1.y(), p2.y()) - max(60.0, dist * 0.35)
        path = QPainterPath(p1)
        path.quadTo(QPointF((p1.x() + p2.x()) / 2, top), p2)
        path.addEllipse(p1, 4, 4)
        path.addEllipse(p2, 4, 4)
        self._arc.setPath(path)

    def _clear_focus(self):
        if self._arc is not None:
            if self._arc.scene() is not None:
                self._arc.scene().removeItem(self._arc)
            self._arc = None
        if self._focus is not None:
            for node in self.graph.all_nodes():
                node.view.setOpacity(1.0)
            for pipe in self.graph.viewer().all_pipes():
                pipe.setOpacity(1.0)
        self._focus = None

    def _port_color_menu(self, node, port_item, global_pos):
        """Right-click on an output of a router / IF: that output's colour. True if shown."""
        mid = split_mid(self.mid_of.get(node.id))[0]
        port = next((p for p in node.output_ports() if p.view is port_item), None)
        if port is None or not mid:
            return False
        key = self.rule_of.get((node.id, port.name())) or self.out_lane_of.get((node.id, port.name()))
        label = dict(self.outputs_of(mid)).get(key)
        if label is None:
            return False
        self.output_color_menu(mid, key, global_pos, _t("Link colour") + " · " + label)
        return True

    def _remove_lane_menu(self, r, parent):
        """'Remove a lane' submenu of pass-through r: straight = lane n (both sides), routed = each input and output."""
        menu = RoundMenu(_t("Remove a lane"), parent)
        menu.setIcon(FIF.REMOVE.icon())
        if r["routed"]:
            items = [(x["id"], _t("Input {n}").format(n=i + 1)) for i, x in enumerate(r["inputs"])] + \
                    [(x["id"], _t("Output {n}").format(n=i + 1)) for i, x in enumerate(r["outputs"])]
        else:
            items = [(x["id"], _t("Lane {n}").format(n=i + 1)) for i, x in enumerate(r["inputs"])]
        for lane_id, label in items:
            a = Action(label, menu)
            a.triggered.connect(lambda _=False, lid=lane_id: self.ctl.remove_lane(r["id"], lid))
            menu.addAction(a)
        return menu

    def _lane_menu(self, node, port_item, global_pos):
        """Right-click on a pass-through lane (port): remove it. True if shown."""
        from .. import graph as graph_mod
        mid = self.mid_of.get(node.id)
        kind, r = graph_mod.find(self.ctl.graph, mid)
        if kind != "relay" or r.get("wireless"):
            return False
        port = next((p for p in node.input_ports() + node.output_ports() if p.view is port_item), None)
        if port is None:
            return False
        lane = self.in_lane_of.get((node.id, port.name())) or self.out_lane_of.get((node.id, port.name()))
        if lane is None:
            return False
        menu = RoundMenu(parent=self)
        a = Action(FIF.REMOVE, _t("Remove this lane"), menu)
        a.triggered.connect(lambda: self.ctl.remove_lane(mid, lane))
        a.setEnabled(len(r["inputs" if lane in {x["id"] for x in r["inputs"]} else "outputs"]) > 1)
        menu.addAction(a)
        menu.exec(global_pos)
        return True

    def delete_selected(self):
        from .. import graph as graph_mod
        viewer = self.graph.viewer()
        if _ViewerFilter._typing(viewer):           # (the Delete shortcut caught a key meant for the text)
            QApplication.sendEvent(viewer.scene(), QKeyEvent(QEvent.KeyPress, Qt.Key_Delete, Qt.NoModifier))
            return
        ids, halves = [], {}
        for n in self.graph.selected_nodes():
            rid, half = split_mid(self.mid_of.get(n.id))
            if not rid or rid == PC_ID:
                continue
            if half:
                halves.setdefault(rid, []).append(half)
            elif rid not in ids:
                ids.append(rid)
        lone = []
        for rid, hs in halves.items():      # both halves (or the half left over): the whole pair goes
            _, r = graph_mod.find(self.ctl.graph, rid)
            if r is not None and (len(hs) == 2 or r.get("broken")):
                ids.append(rid)
            elif r is not None:
                lone.append((rid, hs[0]))
        if ids or lone:
            confirm_delete(self.ctl, ids, self.window(), lone)

    def fit(self):
        if self.graph.all_nodes():
            self._fit_all()

    def _fit_all(self):
        self.graph.select_all()
        self.graph.fit_to_selection()
        self.graph.clear_selection()

    # --- toolbar: like a document editor's, the block actions light up for the selection ---
    def _build_toolbar(self):
        ctl = self.ctl
        bar = self.toolbar = CommandBar(self)
        bar.setToolButtonStyle(Qt.ToolButtonIconOnly)
        bar.setButtonTight(True)        # (icon buttons as narrow as the icon: everything fits in one row)

        def act(icon, text, fn, keys=""):
            a = Action(icon, text, bar)
            a.setToolTip(text + (f" ({keys})" if keys else ""))
            a.triggered.connect(fn)
            return a
        self.btn_undo = act(QIcon(), _t("Undo"), ctl.undo, "Ctrl+Z")
        self.btn_redo = act(QIcon(), _t("Redo"), ctl.redo, "Ctrl+Y")
        self._redo_icon()
        self.act_edit = act(FIF.EDIT, _t("Edit"), self.edit_selected, _t("double-click"))
        self.act_rename = act(FIF.LABEL, _t("Rename"), self.rename_selected, "F2")
        self.act_duplicate = act(FIF.COPY, _t("Duplicate"), self.duplicate_selected, "Ctrl+D")
        self.act_delete = act(FIF.DELETE, _t("Delete"), self.delete_selected, "Delete")
        self.act_unlink = act(FIF.REMOVE_FROM, _t("Disconnect all links"), self.disconnect_selected)
        self.act_run = act(FIF.PLAY, _t("Run"), self.toggle_selected)
        self.act_prev = act(FIF.LEFT_ARROW, _t("Previous block (linking into it)"), lambda: self.nav_key(Qt.Key_Left), "←")
        self.act_next = act(FIF.RIGHT_ARROW, _t("Next block (it links to)"), lambda: self.nav_key(Qt.Key_Right), "→")
        self.act_up = act(FIF.UP, _t("Block above (or the other choice)"), lambda: self.nav_key(Qt.Key_Up), "↑")
        self.act_down = act(FIF.DOWN, _t("Block below (or the other choice)"), lambda: self.nav_key(Qt.Key_Down), "↓")
        self.btn_latency = act(FIF.SPEED_HIGH, _t("Test latency"), self.latency_selected)
        self.btn_fit = act(FIF.ZOOM, _t("Fit"), self.fit_view)
        self.btn_stop = act(FIF.PAUSE, _t("Stop all"), ctl.stop_all)
        self._add_buttons = (self.btn_in, self.btn_out, self.btn_router)
        for b in self._add_buttons:
            b.set_compact(False)        # (remembers the name, sets it as the tooltip)
        self._add_full_w = sum(b.width() for b in self._add_buttons)
        self._compact = False
        for b in self._add_buttons:
            bar.addWidget(b)
        bar.addSeparator()
        for group in ((self.btn_undo, self.btn_redo),
                      (self.act_edit, self.act_rename, self.act_duplicate, self.act_delete, self.act_unlink, self.act_run),
                      (self.act_prev, self.act_next, self.act_up, self.act_down),
                      (self.btn_latency, self.btn_fit, self.btn_stop)):
            if bar.actions():
                bar.addSeparator()
            bar.addActions(list(group))
        self.status_label = CaptionLabel("", self)
        self.status_label.setTextColor(QColor(96, 96, 96), QColor(170, 170, 170))

    @staticmethod
    def _curved_arrow(redo):
        """Undo / redo icon: a curved arrow (Fluent has none); drawn in the theme's text colour."""
        size = 64
        color = QColor(255, 255, 255) if isDarkTheme() else QColor(32, 32, 32)
        icon = QIcon()
        for mode, alpha in ((QIcon.Normal, 230), (QIcon.Disabled, 80)):
            pm = QPixmap(size, size)
            pm.fill(Qt.transparent)
            p = QPainter(pm)
            p.setRenderHint(QPainter.Antialiasing, True)
            if redo:
                p.translate(size, 0)
                p.scale(-1, 1)
            c = QColor(color)
            c.setAlpha(alpha)
            p.setPen(QPen(c, 5.5, Qt.SolidLine, Qt.RoundCap, Qt.RoundJoin))
            path = QPainterPath(QPointF(16, 26))          # hook from the left, round over the top, down the right
            path.cubicTo(QPointF(26, 14), QPointF(48, 14), QPointF(52, 32))
            path.cubicTo(QPointF(55, 44), QPointF(46, 52), QPointF(34, 52))
            p.drawPath(path)
            p.setBrush(c)
            p.setPen(Qt.NoPen)
            p.drawPolygon(QPolygonF([QPointF(8, 30), QPointF(22, 12), QPointF(26, 34)]))
            p.end()
            icon.addPixmap(pm, mode)
        return icon

    def _fit_toolbar(self):
        """The 'Add ...' buttons show their names when the toolbar has room for them and every other button,
        else only their icons (the name stays as the tooltip)."""
        bar = self.toolbar
        others = bar.suitableWidth() - sum(b.width() for b in self._add_buttons)
        compact = others + self._add_full_w > bar.width()
        if compact == self._compact:
            return
        self._compact = compact
        for b in self._add_buttons:
            b.set_compact(compact)
        bar.updateGeometry()
        bar.update()

    def resizeEvent(self, event):
        super().resizeEvent(event)
        QTimer.singleShot(0, self._fit_toolbar)

    def _redo_icon(self):
        """(Re)draw the undo / redo icons, e.g. after the theme changed."""
        self.btn_undo.setIcon(self._curved_arrow(False))
        self.btn_redo.setIcon(self._curved_arrow(True))

    def _selection(self):
        """[(canvas id, model id, half)] of the selected blocks."""
        out = []
        for n in self.graph.selected_nodes():
            mid = self.mid_of.get(n.id)
            if mid:
                out.append((mid,) + split_mid(mid))
        return out

    def sync_toolbar(self):
        """Enable what the selection allows (nothing selected: only undo / redo, latency, fit, stop all)."""
        from .. import graph as graph_mod
        sel = self._selection()
        blocks = [x for x in sel if x[0] != PC_ID]
        one = sel[0] if len(sel) == 1 else None
        kind = graph_mod.find(self.ctl.graph, one[1])[0] if one and one[0] != PC_ID else None
        self.act_edit.setEnabled(kind is not None)
        self.act_rename.setEnabled(kind is not None)
        self.act_duplicate.setEnabled(bool(blocks))
        self.act_delete.setEnabled(bool(blocks))
        self.act_unlink.setEnabled(bool(sel))
        running = kind == "inbound" and self.ctl.manager.state(one[1])[0] in ("running", "starting")
        self.act_run.setEnabled(kind == "inbound")
        self.act_run.setIcon(FIF.PAUSE if running else FIF.PLAY)
        self.act_run.setText(_t("Stop") if running else _t("Run"))
        self.act_run.setToolTip(self.act_run.text())
        node = self.graph.get_node_by_id(self.node_of[one[0]].id) if one and one[0] in self.node_of else None
        self.act_prev.setEnabled(node is not None and bool(self.neighbours(node, False)))
        self.act_next.setEnabled(node is not None and bool(self.neighbours(node, True)))
        choices = node is not None and self._nav is not None and node.id in self._nav[0] and len(self._nav[0]) > 1
        self.act_up.setEnabled(choices or (node is not None and self.nearest_block(node, True) is not None))
        self.act_down.setEnabled(choices or (node is not None and self.nearest_block(node, False) is not None))
        outs = [x for x in blocks if graph_mod.find(self.ctl.graph, x[1])[0] == "outbound"]
        if not self.ctl.latency_busy:
            self.btn_latency.setToolTip(_t("Measure the selected outbound blocks") if outs else
                                        _t("Measure every outbound block (an HTTPS request through it)"))
        if self.ctl.latency_busy:
            pass                            # (latency_progress shows the progress)
        elif one:
            title = self.node_of[one[0]].view.name if one[0] in self.node_of else ""
            self.status_label.setText(_t("Selected: {name}").format(name=title))
        elif sel:
            self.status_label.setText(_t("{n} blocks selected").format(n=len(sel)))
        else:
            self.status_label.setText("")

    def _one(self):
        sel = self._selection()
        return sel[0] if len(sel) == 1 and sel[0][0] != PC_ID else None

    def edit_selected(self):
        one = self._one()
        if one:
            self.ctl.edit_node(one[1])

    def rename_selected(self):
        """Edit the selected block's title right on its title bar (Enter applies, Esc cancels)."""
        one = self._one()
        node = self.node_of.get(one[0]) if one else None
        if node is None:
            return
        text = node.view._text_item
        text.set_editable(True)
        text.setFocus(Qt.OtherFocusReason)
        cursor = text.textCursor()
        cursor.select(QTextCursor.Document)
        text.setTextCursor(cursor)

    def duplicate_selected(self):
        ids = []
        for _, mid, _ in self._selection():
            if mid != PC_ID and mid not in ids:
                ids.append(mid)
        for mid in ids:                 # (one turn of the event loop: one undo step)
            self.ctl.duplicate_node(mid)

    def disconnect_selected(self):
        self.ctl.disconnect_all([(mid, half) for _, mid, half in self._selection()])

    def toggle_selected(self):
        one = self._one()
        if one:
            running = self.ctl.manager.state(one[1])[0] in ("running", "starting")
            self.ctl.toggle_proxy(one[1], not running)
            QTimer.singleShot(300, self.sync_toolbar)

    def latency_selected(self):
        """Measure the selected outbound blocks, or all of them when none is selected."""
        from .. import graph as graph_mod
        ids = [mid for _, mid, _ in self._selection() if graph_mod.find(self.ctl.graph, mid)[0] == "outbound"
               and graph_mod.find(self.ctl.graph, mid)[1]["outbound"]["type"] != "block"]
        self.ctl.test_latency(ids or None)

    def fit_view(self):
        """Fit the selection, or everything when nothing is selected."""
        if self.graph.selected_nodes():
            self.graph.fit_to_selection()
        else:
            self.fit()

    def sync_undo(self, can_undo, can_redo):
        self.btn_undo.setEnabled(can_undo)
        self.btn_redo.setEnabled(can_redo)

    def _undo_key(self, fn, keys):
        """Ctrl+Z / Ctrl+Y: undo / redo the last change, or the typing while a title or number is being edited."""
        viewer = self.graph.viewer()
        if _ViewerFilter._typing(viewer):
            seq = QKeySequence(keys)[0]
            QApplication.sendEvent(viewer.scene(), QKeyEvent(QEvent.KeyPress, seq.key(), seq.keyboardModifiers()))
            return
        fn()

    # --- latency badges on outbound blocks ---
    def latency_progress(self, done, total):
        """done/total while a latency test runs, (None, None) when it is over."""
        if done is None:
            self.btn_latency.setEnabled(True)
            self.sync_toolbar()
        else:
            self.btn_latency.setEnabled(False)
            self.status_label.setText(_t("Testing… {done}/{total}").format(done=done, total=total))

    def update_latency(self):
        """Badges from ctl.latency: green / amber / red by time, ✕ when the block did not respond, … while testing.
        A result is shown only while the block (and its chain) is unchanged."""
        from .. import probe
        cg = self.ctl.graph
        for n in cg["outbounds"]:
            node = self.node_of.get(n["id"])
            if node is None:
                continue
            res = self.ctl.latency.get(n["id"])
            if n["id"] in self.ctl.latency_busy:
                badge = ("…", "#9AA0A6", _t("Testing…"))
            elif res is None or res["key"] != probe.latency_key(cg, n["id"]):
                badge = None
            elif res["ok"]:
                ms = res["ms"]
                badge = (f"{ms} ms", "#3FA34D" if ms < 400 else "#D9A21B" if ms < 1000 else "#E5534B",
                         _t("HTTPS request through this block took {ms} ms").format(ms=ms))
            else:
                badge = ("✕", "#E5534B", _t("Not responding: {error}").format(error=res["error"] or "-"))
            if node.view.badge != badge:
                node.view.badge = badge
                node.view.update()

    def _apply_theme(self):
        dark = isDarkTheme()
        if dark:
            self.graph.set_background_color(32, 32, 32)
            self.graph.set_grid_color(48, 48, 48)
        else:
            self.graph.set_background_color(236, 238, 241)
            self.graph.set_grid_color(214, 217, 222)
        border = "rgba(255,255,255,0.10)" if dark else "rgba(0,0,0,0.12)"
        self.graph.viewer().setStyleSheet(f"QGraphicsView {{ border: 1px solid {border}; border-radius: 8px; }}")

    # --- model -> view ---
    def _signature(self, graph):
        return json.dumps([[n["id"], n["name"], n["target"], n.get("color")] for n in graph["inbounds"]] +
                          [[n["id"], n["name"], n["via"], n["outbound"]["type"], n.get("members", []), n.get("color")]
                           for n in graph["outbounds"]] +
                          [[n["id"], n["name"], n["default"], n.get("switch"),
                            [[x["id"], x["name"], x["target"], x.get("color")] for x in n["rules"]],
                            n.get("color"), n.get("default_color")] for n in graph.get("routers", [])] +
                          [[n["id"], n["name"], n["routed"], [[x["id"], x["gap"], x["out"]] for x in n["inputs"]],
                            [[x["id"], x["gap"], x["target"], x.get("color")] for x in n["outputs"]], n.get("color"),
                            n.get("wireless"), n.get("broken"), n.get("number"), n.get("name_out")]
                           for n in graph.get("relays", [])] +
                          [graph["pc"]["inbound"]])

    def refresh(self, graph, proxies, statuses, pc_status):
        sig = self._signature(graph)
        if sig != self.sig:
            self.sig = sig
            self._rebuild(graph)
        self.update_states(proxies, statuses, pc_status)
        self.pc_bar.set_status(pc_status)

    def _rebuild(self, graph):
        g = self.graph
        self.syncing = True
        try:
            self._clear_focus()
            self._nav, self._came_from = None, {}
            g.clear_session()
            self.node_of, self.mid_of, self.rule_of = {}, {}, {}
            self.in_lane_of, self.out_lane_of, self.lane_port = {}, {}, {}

            def make(kind, mid, title, pos, color):
                # NodeGraphQt keeps node names unique and would quietly add " 1" to a repeated one: every node is
                # named by its canvas id instead, and only the title bar shows the block's title
                node = g.create_node(f"proxydock.{kind}", name=mid, pos=list(pos), color=color,
                                     text_color=(235, 235, 235), selected=False, push_undo=False)
                node.view.name = title
                node.view.draw_node()
                self.node_of[mid] = node
                self.mid_of[node.id] = mid
                if mid != PC_ID:
                    model = split_mid(mid)[0]
                    gkind, gn = graph_mod.find(graph, model)
                    node.view.swatch = gn.get("color", "") if gn else ""
                    if gkind != "relay":    # (a pass-through's lanes follow what comes in: no colour to pick)
                        node.view.on_swatch = lambda at, mid=model: self.color_menu(mid, at)
                return node

            from .. import graph as graph_mod
            for n in graph["outbounds"]:
                if n["outbound"]["type"] == "group":
                    make("GroupNode", n["id"], n["name"], n["pos"], GROUP_COLOR)
                    continue
                make("EndNode" if n["outbound"]["type"] in graph_mod.NO_VIA else "OutNode",
                     n["id"], n["name"], n["pos"], OUT_COLOR)
            for n in graph["inbounds"]:
                node = make("InNode", n["id"], n["name"], n["pos"], IN_COLOR)
                sw = node.get_widget("switches")
                sw.run.toggled.connect(lambda on, mid=n["id"]: self.ctl.toggle_proxy(mid, on))
                sw.auto.toggled.connect(lambda on, mid=n["id"]: self.ctl.set_flag(mid, "autostart", on))
                sw.restart.toggled.connect(lambda on, mid=n["id"]: self.ctl.set_flag(mid, "auto_restart", on))
            for r in graph.get("routers", []):
                is_if = r.get("style") == "if"
                node = make("RouterNode", r["id"], r["name"], r["pos"], IF_COLOR if is_if else ROUTER_COLOR)
                for i, rule in enumerate(r["rules"]):
                    name = _t("then") if is_if else f"{i + 1}. {rule['name']}"
                    node.add_output(name, multi_output=False, color=THEN_COLOR if is_if else RULE_COLOR)
                    self.rule_of[(node.id, name)] = rule["id"]
                default = _t("else") if is_if else _t("default")
                node.add_output(default, multi_output=False, color=ELSE_COLOR if is_if else PORT_COLOR)
                self.rule_of[(node.id, default)] = "default"
                if is_if:               # an IF colours then and else on their own: two swatches
                    node.view.on_swatch = None
                    node.view.out_swatches = [
                        {"key": k, "own": "", "fill": "", "mixed": False, "tip": tip,
                         "on_click": lambda at, rid=r["id"], k=k, tip=tip: self.output_color_menu(rid, k, at, tip)}
                        for k, tip in ((r["rules"][0]["id"], _t("Link colour") + " · " + _t("then")),
                                       ("default", _t("Link colour") + " · " + _t("else")))]
                if r.get("switch"):
                    tw = ToggleWidget(node.view)
                    node.add_custom_widget(tw)
                    tw.on_set = lambda on, rid=r["id"]: self.ctl.set_switch(rid, on)
            for r in graph.get("relays", []):
                if r.get("wireless"):
                    broken = r.get("broken")
                    for half, kind, title, pos in (
                            ("in", "WirelessInNode", r["name"], r["pos"]),
                            ("out", "WirelessOutNode", r.get("name_out", ""), r.get("pos_out", r["pos"]))):
                        if broken == half:
                            continue
                        node = make(kind, f"{r['id']}~{half}", title, pos, BROKEN_COLOR if broken else WIRELESS_COLOR)
                        item = node.view
                        item.pair_name, item.broken, item.on_moved = f"#{r['number']}", bool(broken), self._update_arc
                        item.on_number = lambda text, rid=r["id"]: self._apply_number(rid, text)
                        if half == "in":
                            port = node.add_input("in", multi_input=True, display_name=False, color=LANE_COLOR)
                            self.in_lane_of[(node.id, "in")] = r["inputs"][0]["id"]
                            if not broken:
                                self.lane_port[r["inputs"][0]["id"]] = port
                        else:
                            node.add_output("out", multi_output=False, display_name=False, color=LANE_COLOR)
                            self.out_lane_of[(node.id, "out")] = r["outputs"][0]["id"]
                        item.draw_node()
                    continue
                node = make("RelayNode", r["id"], r["name"], r["pos"], RELAY_COLOR)
                self._setup_relay(node, r)
            make("PcNode", PC_ID, _t("PC (system proxy)"), graph["pc"]["pos"], PC_COLOR)

            def port_in(tid):
                if tid in self.lane_port:
                    return self.lane_port[tid]
                node = self.node_of.get(tid) if tid else None
                return node.input(0) if node is not None and node.inputs() else None
            for n in graph["inbounds"]:
                dst = port_in(n["target"])
                if dst is not None:
                    self.node_of[n["id"]].output(0).connect_to(dst, push_undo=False)
            for n in graph["outbounds"]:
                if n["via"] and n["via"] in self.node_of and self.node_of[n["id"]].outputs():
                    self.node_of[n["id"]].output(0).connect_to(self.node_of[n["via"]].input(0), push_undo=False)
                for m in n.get("members", []):
                    if m in self.node_of:
                        self.node_of[n["id"]].output(0).connect_to(self.node_of[m].input(0), push_undo=False)
            for r in graph.get("routers", []):
                node = self.node_of[r["id"]]
                for i, (key, tid) in enumerate([(x["id"], x["target"]) for x in r["rules"]] + [("default", r["default"])]):
                    dst = port_in(tid)
                    if dst is not None and i < len(node.outputs()):
                        node.output(i).connect_to(dst, push_undo=False)
            for r in graph.get("relays", []):
                node = self.node_of.get(r["id"]) or self.node_of.get(r["id"] + "~out")
                if node is None:
                    continue
                for i, x in enumerate(r["outputs"]):
                    dst = port_in(x["target"])
                    if dst is not None:
                        node.output(i).connect_to(dst, push_undo=False)
            if graph["pc"]["inbound"] in self.node_of:
                self.node_of[graph["pc"]["inbound"]].output(0).connect_to(self.node_of[PC_ID].input(0), push_undo=False)
            self._paint_wires(graph)
            g.clear_undo_stack()
        finally:
            self.syncing = False
        if not self.fitted and self.isVisible():
            self.fitted = True
            self._fit_all()
        self.sync_toolbar()

    def _paint_wires(self, graph):
        """Links take the colour of the block they leave; pass-through lanes hand on what comes in."""
        from .. import graph as graph_mod
        lanes = graph_mod.wire_colors(graph)
        for mid, node in self.node_of.items():
            if mid == PC_ID:
                continue
            mid = split_mid(mid)[0]
            kind, n = graph_mod.find(graph, mid)
            if n is None:
                continue
            if kind == "relay" and not n.get("wireless"):
                node.view.link_colors = [lanes[x["id"]][0] for x in n["inputs"]]
                node.view.link_mixed = [len(lanes[x["id"]][1]) > 1 for x in n["inputs"]]
            own_color, own_parts = lanes.get(mid, (n.get("color", ""), ()))
            node.view.swatch_fill = own_color
            node.view.swatch_mixed = len(own_parts) > 1
            for sw in node.view.out_swatches:
                sw["own"] = graph_mod.output_color(graph, mid, sw["key"])
                sw["fill"], parts = lanes.get(graph_mod.router_out_key(mid, sw["key"]), ("", ()))
                sw["mixed"] = len(parts) > 1
            if kind != "relay":             # (a pass-through's tooltip lists its lanes, set in update_states)
                node.view.setToolTip(
                    _t("Link colour: mix of {colors}").format(colors=" + ".join(color_name(c) for c in own_parts))
                    if len(own_parts) > 1 and not n.get("color") else "")
            node.view.update()
            for port in node.output_ports():
                lane = self.out_lane_of.get((node.id, port.name()))
                rule = self.rule_of.get((node.id, port.name()))
                if rule is not None:
                    lane = graph_mod.router_out_key(mid, rule)
                color, parts = lanes[lane] if lane in lanes else (own_color, own_parts)
                for pipe in port.view.connected_pipes:
                    pipe.color = _rgba(color)
                    pipe.style = PipeEnum.DRAW_TYPE_DASHED.value if len(parts) > 1 else PipeEnum.DRAW_TYPE_DEFAULT.value
                    pipe.reset()

    def _fill_color_menu(self, menu, cur, apply, auto_label):
        """Palette, the automatic choice (auto_label) and a custom colour; apply(colour) sets it ('' = automatic)."""
        def icon(color):
            pm = QPixmap(16, 16)
            pm.fill(QColor(*_rgba(color)))
            return QIcon(pm)
        for hexc, label in PALETTE:
            a = Action(icon(hexc), ("✓ " if hexc == cur else "") + label, menu)
            a.triggered.connect(lambda _=False, c=hexc: apply(c))
            menu.addAction(a)
        menu.addSeparator()
        d = Action(icon(""), ("✓ " if not cur else "") + auto_label, menu)
        d.triggered.connect(lambda: apply(""))
        custom = Action(FIF.PALETTE, _t("Custom colour…"), menu)
        custom.triggered.connect(lambda: self._pick_color(cur, apply))
        menu.addActions([d, custom])
        return menu

    def color_menu(self, mid, at):
        """Swatch clicked: palette, default, or any colour."""
        from .. import graph as graph_mod
        _, n = graph_mod.find(self.ctl.graph, mid)
        if n is None:
            return
        menu = self._fill_color_menu(RoundMenu(parent=self), n.get("color", ""),
                                     lambda c: self.ctl.set_color(mid, c), _t("Automatic (follow the incoming links)"))
        menu.exec(at.toPoint() if hasattr(at, "toPoint") else at)

    def _output_auto_label(self, mid):
        """What an output without its own colour uses: the block's colour (an IF has none: the incoming links)."""
        from .. import graph as graph_mod
        kind, n = graph_mod.find(self.ctl.graph, mid)
        if kind == "router" and n.get("style") == "if":
            return _t("Automatic (follow the incoming links)")
        return _t("Same as the block")

    def output_color_menu(self, mid, key, at, title=""):
        """Colour of one output of a router / IF (rule id or "default") or pass-through (output lane id)."""
        from .. import graph as graph_mod
        menu = RoundMenu(parent=self)
        if title:
            head = Action(title, menu)
            head.setEnabled(False)
            menu.addAction(head)
            menu.addSeparator()
        self._fill_color_menu(menu, graph_mod.output_color(self.ctl.graph, mid, key),
                              lambda c: self.ctl.set_output_color(mid, key, c), self._output_auto_label(mid))
        menu.exec(at.toPoint() if hasattr(at, "toPoint") else at)

    def outputs_of(self, mid):
        """[(output key, label)] of a router / IF block, in port order ([] for other blocks)."""
        from .. import graph as graph_mod
        kind, n = graph_mod.find(self.ctl.graph, mid)
        if kind == "router":
            if n.get("style") == "if":
                return [(n["rules"][0]["id"], _t("then")), ("default", _t("else"))]
            return [(x["id"], f"{i + 1}. {x['name']}") for i, x in enumerate(n["rules"])] + [("default", _t("default"))]
        return []

    def _output_colors_menu(self, mid, parent):
        """Submenu with a colour menu per output."""
        from .. import graph as graph_mod
        menu = RoundMenu(_t("Output link colours"), parent)
        menu.setIcon(FIF.PALETTE.icon())
        auto = self._output_auto_label(mid)
        for key, label in self.outputs_of(mid):
            sub = RoundMenu(label, menu)
            self._fill_color_menu(sub, graph_mod.output_color(self.ctl.graph, mid, key),
                                  lambda c, key=key: self.ctl.set_output_color(mid, key, c), auto)
            menu.addMenu(sub)
        return menu

    def _pick_color(self, cur, apply):
        from .common import ColorPicker
        dlg = ColorPicker(QColor(cur) if cur else QColor(*_rgba("")[:3]), _t("Link colour"), self.window())
        dlg.colorChanged.connect(lambda c: apply(c.name()))
        dlg.exec()

    def _setup_relay(self, node, r):
        """Lanes (ports), gaps and inside wiring of pass-through node from relay r."""
        item = node.view
        item.in_gaps = [x["gap"] for x in r["inputs"]]
        item.out_gaps = [x["gap"] for x in r["outputs"]]
        out_index = {x["id"]: i for i, x in enumerate(r["outputs"])}
        item.links = [(i, out_index[x["out"]]) for i, x in enumerate(r["inputs"]) if x["out"] in out_index]
        item.routed = r["routed"]
        item.footer_text = "+ " + _t("Click to add a lane")
        item.on_add = lambda rid=r["id"]: self.ctl.add_lane(rid)
        if r["routed"]:
            ins, outs = [x["id"] for x in r["inputs"]], [x["id"] for x in r["outputs"]]
            item.on_wire = lambda i, o, rid=r["id"]: self._apply_wire(rid, ins[i], outs[o] if o is not None else None)
        for i, x in enumerate(r["inputs"]):
            name = f"in {i + 1}"
            port = node.add_input(name, multi_input=True, display_name=False, color=LANE_COLOR)
            self.in_lane_of[(node.id, name)] = x["id"]
            self.lane_port[x["id"]] = port
        for i, x in enumerate(r["outputs"]):
            name = f"out {i + 1}"
            node.add_output(name, multi_output=False, display_name=False, color=LANE_COLOR)
            self.out_lane_of[(node.id, name)] = x["id"]
        item.draw_node()

    def showEvent(self, event):
        super().showEvent(event)
        if not self.fitted and self.node_of:
            self.fitted = True
            self._fit_all()

    def update_states(self, proxies, statuses, pc_status):
        from .. import graph as graph_mod
        self.update_latency()
        self.sync_toolbar()
        cg = self.ctl.graph
        for p in proxies:
            node = self.node_of.get(p["id"])
            if node is None:
                continue
            ib = p["inbound"]
            state, msg = statuses.get(p["id"], ("stopped", ""))
            from ..common import inbound_where
            lines = [_line("#DADADA", f"{INBOUND_TYPES[ib['type']]} · {inbound_where(ib)}")]
            if p["unlinked"]:
                lines.append(_line("#E8B339", "⚠ " + _t("not connected")))
            color = {"running": "#6CCB5F", "starting": "#E8B339", "error": "#FF8A80"}.get(state, "#9A9A9A")
            text = STATE_TEXT.get(state, state) + (f" - {msg}" if msg else "")
            lines.append(_line(color, ("● " if state == "running" else "○ ") + (text[:60] + "…" if len(text) > 60 else text)))
            node.get_widget("info").set_value("<br>".join(lines))
            node.get_widget("switches").show(state in ("running", "starting"), p["autostart"], p.get("auto_restart"))
        for n in cg["outbounds"]:
            node = self.node_of.get(n["id"])
            if node is None:
                continue
            users = graph_mod.used_by(cg, n["id"])
            members = [graph_mod.find(cg, m)[1]["name"] for m in n.get("members", []) if graph_mod.find(cg, m)[1]]
            if n["outbound"]["type"] == "group":
                mode = {"failover": _t("Failover in order"), "select": _t("Manual")}.get(n["outbound"]["group_mode"], _t("Fastest"))
                if n["outbound"]["group_mode"] == "select" and n.get("members"):
                    sel = n["outbound"]["group_selected"] if n["outbound"]["group_selected"] in n["members"] else n["members"][0]
                    mode += " · " + graph_mod.find(cg, sel)[1]["name"]
                lines = [_line("#DADADA", f"{mode} · " + _t("{count} members").format(count=len(members)))]
                if not members:
                    lines.append(_line("#E8B339", "⚠ " + _t("no members")))
            else:
                lines = [_line("#DADADA", outbound_summary(n["outbound"]))]
            if n["via"]:
                lines.append(_line("#EBBE6E", "⇢ " + " ⇢ ".join(graph_mod.chain_names(cg, n["via"]))))
            lines.append(_line("#9A9A9A", _t("used by: {names}").format(names=", ".join(users))
                               if users else _t("not used")))
            node.get_widget("info").set_value("<br>".join(lines))
        for r in cg.get("routers", []):
            node = self.node_of.get(r["id"])
            if node is None:
                continue
            users = graph_mod.users(cg, r["id"])
            # (the default may lead through pass-through lanes: name the block at their end)
            _, d = graph_mod.find(cg, graph_mod.follow(cg, r["default"])) if r["default"] else (None, None)
            if r.get("switch"):
                lines = [_line("#6CCB5F" if r.get("switch_on") else "#9A9A9A",
                               _t("Switch: on → then") if r.get("switch_on") else _t("Switch: off → else"))]
                tw = node.get_widget("toggle")
                if tw is not None:
                    tw.show(r.get("switch_on"))
            elif r.get("style") == "if":
                from .router import match_summary
                lines = [_line("#DADADA", "IF " + (match_summary(r["rules"][0]["match"]) or _t("(no condition)")))]
            else:
                lines = [_line("#DADADA", _t("{count} rules").format(count=len(r["rules"])) + " · "
                               + _t("default: {name}").format(name=d["name"] if d else "-"))]
            if not d:
                lines.append(_line("#E8B339", "⚠ " + _t("no default target")))
            lines.append(_line("#9A9A9A", _t("used by: {names}").format(names=", ".join(u["name"] for u in users))
                               if users else _t("not used")))
            node.get_widget("info").set_value("<br>".join(lines))
        for r in cg.get("relays", []):
            node = self.node_of.get(r["id"])
            if node is None:
                continue
            tips = [_t("Drag from an inner dot to a dot on the other side to wire the lanes; drag a wire off to unwire it."), ""] \
                if r["routed"] else []
            for i, x in enumerate(r["inputs"]):
                src = graph_mod.lane_sources(cg, x["id"])
                _, t = graph_mod.find(cg, graph_mod.follow(cg, x["id"]))
                tips.append(f"{i + 1}. " + (", ".join(src) or "-") + "  →  " + (t["name"] if t else _t("not connected")))
            lanes = graph_mod.wire_colors(cg)
            mixes = [(i, lanes[x["id"]][1]) for i, x in enumerate(r["outputs"]) if len(lanes[x["id"]][1]) > 1]
            if mixes:
                tips.append("")
                for i, parts in mixes:
                    tips.append(_t("Output {n}: mix of {colors}").format(n=i + 1, colors=" + ".join(color_name(c) for c in parts)))
            node.view.setToolTip("\n".join(tips))
        for r in cg.get("relays", []):
            if not r.get("wireless"):
                continue
            lane = r["inputs"][0]["id"]
            if r.get("broken"):
                info_in = info_out = "⚠ " + _t("partner deleted")
                tip = _t("The other half of this wireless pair was deleted, so nothing goes through it. "
                         "Right-click to recreate the partner or delete it.")
            else:
                src = graph_mod.lane_sources(cg, lane)
                _, t = graph_mod.find(cg, graph_mod.follow(cg, lane))
                dest = t["name"] if t else _t("not connected")
                info_in, info_out = "→ " + dest, "← " + (", ".join(src) or "-")
                tip = (_t("from: {names}").format(names=", ".join(src) or "-") + "\n→ " + dest + "\n\n"
                       + _t("Click to show its partner; right-click → Go to the partner."))
            for half, text in (("in", info_in), ("out", info_out)):
                node = self.node_of.get(f"{r['id']}~{half}")
                if node is not None:
                    node.view.info_text = text
                    node.view.setToolTip(tip)
                    node.view.update()
        pc = self.node_of.get(PC_ID)
        if pc is not None:
            state, text = pc_status
            color = {"applied": "#6CCB5F", "waiting": "#E8B339", "error": "#FF8A80"}.get(state, "#9A9A9A")
            pc.get_widget("info").set_value(_line(color, text))
        self.pc_bar.set_status(pc_status)

    # --- view -> model ---
    def _on_connected(self, in_port, out_port):
        if self.syncing:
            return
        src = split_mid(self.mid_of.get(out_port.node().id))[0]
        dst, dst_half = split_mid(self.mid_of.get(in_port.node().id))
        if not src or not dst:
            return
        from .. import graph as graph_mod
        for mid in (src, dst):
            k, r = graph_mod.find(self.ctl.graph, mid)
            if k == "relay" and r.get("broken"):
                self._reject(in_port, out_port, _t("This wireless block has no partner. Recreate the partner first."))
                return
        src_kind, src_node = graph_mod.find(self.ctl.graph, src)
        dst_kind, _ = graph_mod.find(self.ctl.graph, dst)
        lane = self.in_lane_of.get((in_port.node().id, in_port.name()))
        if lane:
            dst, dst_kind = lane, "lane"
        if src_kind == "relay":
            key = self.out_lane_of.get((out_port.node().id, out_port.name()))
            if dst_kind not in ("outbound", "router", "lane"):
                self._reject(in_port, out_port, _t("A pass-through lane can only go to an outbound block, a router or another pass-through"))
                return
            QTimer.singleShot(0, self, lambda: self._apply_lane(src, key, dst))
            return
        if src_kind == "router":
            key = self.rule_of.get((out_port.node().id, out_port.name()))
            if dst_kind not in ("outbound", "router", "lane"):
                self._reject(in_port, out_port, _t("A router rule can only go to an outbound block, another router or a pass-through"))
                return
            QTimer.singleShot(0, self, lambda: self._apply_rule(src, key, dst))
            return
        if src_kind == "outbound" and src_node["outbound"]["type"] == "group":
            if dst_kind != "outbound":
                self._reject(in_port, out_port, _t("A group's members can only be outbound blocks"))
                return
            QTimer.singleShot(0, self, lambda: self._apply_member(src, dst))
            return
        if src_kind == "outbound" and dst_kind == "router":
            self._reject(in_port, out_port, _t("An outbound's 'via' can only go to another outbound"))
            return
        if src_kind == "outbound":
            if dst_kind != "outbound":
                self._reject(in_port, out_port, _t("An outbound's 'via' can only go to another outbound"))
                return
            QTimer.singleShot(0, self, lambda: self._apply_via(src, dst))
            return
        if dst == PC_ID:
            QTimer.singleShot(0, self, lambda: self.ctl.set_pc(src))
            return
        # one traffic target per inbound: the new link replaces the old one (the PC link stays)
        self.syncing = True
        try:
            for other in list(out_port.connected_ports()):
                if other is not in_port and self.mid_of.get(other.node().id) != PC_ID:
                    out_port.disconnect_from(other, push_undo=False)
        finally:
            self.syncing = False
        # apply after NodeGraphQt has finished handling the drag (the update may redraw the whole canvas)
        QTimer.singleShot(0, self, lambda: self._apply_target(src, dst))

    def _reject(self, in_port, out_port, message):
        self.syncing = True
        try:
            out_port.disconnect_from(in_port, push_undo=False)
        finally:
            self.syncing = False
        info(self.window(), "warning", _t("Cannot connect"), message, 4000)

    def _apply_via(self, src, dst):
        if not self.ctl.set_via(src, dst):
            self.sig = None
            self.ctl.refresh_lists()

    def _apply_member(self, src, dst):
        if not self.ctl.add_member(src, dst):
            self.sig = None
            self.ctl.refresh_lists()

    def _apply_wire(self, rid, in_id, out_id):
        if not self.ctl.set_lane_map(rid, in_id, out_id):
            self.sig = None
            self.ctl.refresh_lists()

    def _apply_lane(self, src, key, dst):
        if key is None or not self.ctl.set_lane_target(src, key, dst):
            self.sig = None
            self.ctl.refresh_lists()

    def _apply_rule(self, src, key, dst):
        if key is None or not self.ctl.set_rule_target(src, key, dst):
            self.sig = None
            self.ctl.refresh_lists()

    def _apply_target(self, src, dst):
        if not self.ctl.set_target(src, dst):
            self.sig = None         # rejected (loop): redraw from the model
            self.ctl.refresh_lists()

    def _on_disconnected(self, in_port, out_port):
        if self.syncing:
            return
        src = split_mid(self.mid_of.get(out_port.node().id))[0]
        dst = split_mid(self.mid_of.get(in_port.node().id))[0]
        if not src or not dst:
            return
        dst = self.in_lane_of.get((in_port.node().id, in_port.name()), dst)
        key = self.rule_of.get((out_port.node().id, out_port.name())) or \
            self.out_lane_of.get((out_port.node().id, out_port.name()))
        QTimer.singleShot(0, self, lambda: self._apply_disconnect(src, dst, key))

    def _apply_disconnect(self, src, dst, key=None):
        from .. import graph as graph_mod
        kind, n = graph_mod.find(self.ctl.graph, src)
        if kind == "relay":
            lane = next((x for x in n["outputs"] if x["id"] == key), None)
            if lane is not None and lane["target"] == dst:
                self.ctl.set_lane_target(src, key, None)
            return
        if kind == "router":
            target = n["default"] if key == "default" else next((x["target"] for x in n["rules"] if x["id"] == key), None)
            if key and target == dst:
                self.ctl.set_rule_target(src, key, None)
            return
        if kind == "outbound":
            if dst in n.get("members", ()):
                self.ctl.remove_member(src, dst)
            elif n["via"] == dst:
                self.ctl.set_via(src, None)
            return
        if dst == PC_ID:
            if self.ctl.graph["pc"]["inbound"] == src:
                self.ctl.set_pc(None)
            return
        _, n = graph_mod.find(self.ctl.graph, src)
        if n is not None and n["target"] == dst:
            self.ctl.set_target(src, None)

    def _on_title_edited(self, node_id, name):
        """A block's title was edited on the canvas (double-click the title): rename the block (a wireless half:
        just its title)."""
        if self.syncing:
            return
        mid, half = split_mid(self.mid_of.get(node_id))
        if not mid:
            return
        name = str(name).strip()
        if half:
            QTimer.singleShot(0, self, lambda: self.ctl.set_wireless_title(mid, half, name))
            return
        QTimer.singleShot(0, self, lambda: self._apply_rename(mid, name))

    def _apply_number(self, rid, text):
        """The number of a wireless pair was edited on its block (None = cancelled)."""
        if text is None:
            return
        try:
            number = int(text)
        except ValueError:
            number = -1
        self.ctl.set_wireless_number(rid, number)

    def _apply_rename(self, mid, name):
        from .. import graph as graph_mod
        _, n = graph_mod.find(self.ctl.graph, mid)
        if n is not None and name and name[:60] != n["name"]:
            self.ctl.rename_node(mid, name)
            return
        self.sig = None             # nothing to change (PC block, empty or same name): show the model's title again
        self.ctl.refresh_lists()

    def _on_double_clicked(self, node):
        mid = split_mid(self.mid_of.get(node.id))[0]
        if mid and mid != PC_ID:
            QTimer.singleShot(0, self, lambda: self.ctl.edit_node(mid))

    def _on_moved(self, *_):
        positions = {}
        for nid, node in self.node_of.items():
            x, y = node.pos()
            rid, half = split_mid(nid)
            positions[rid + "~out" if half == "out" else rid] = [round(x, 1), round(y, 1)]
        self.ctl.move_nodes(positions)
