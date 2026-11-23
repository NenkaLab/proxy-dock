"""Canvas page: blueprint-style block graph (NodeGraphQt).

Blocks: inbound (accepts connections on this PC), outbound (Direct / upstream proxy / tunnel) and PC (system proxy).
Drag from an inbound's output to an outbound block, to another inbound (relay) or to the PC block.
The canvas is a view of ctl.graph: every change goes through the controller, which saves and applies it.
"""
import json
import os

os.environ.setdefault("QT_PREFERRED_BINDING", "PySide6")     # NodeGraphQt uses Qt.py

from NodeGraphQt import BaseNode, NodeBaseWidget, NodeGraph
from NodeGraphQt.constants import PortEnum
from NodeGraphQt.qgraphics.node_abstract import AbstractNodeItem
from NodeGraphQt.qgraphics.node_base import NodeItem
from PySide6.QtCore import QEvent, QObject, QPointF, QRectF, Qt, QTimer
from PySide6.QtGui import QColor, QKeySequence, QPainter, QPainterPath, QPen, QPolygonF, QShortcut
from PySide6.QtWidgets import QHBoxLayout, QLabel, QVBoxLayout, QWidget
from qfluentwidgets import (
    Action, CaptionLabel, DropDownPushButton, FluentIcon as FIF, PrimaryDropDownPushButton, PushButton, RoundMenu,
    TitleLabel, isDarkTheme, qconfig,
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
CHECK_STYLE = ("QCheckBox {{ color: {fg}; background: transparent; font-size: 9pt; {extra} }}"
               "QCheckBox::indicator {{ width: 11px; height: 11px; border: 1px solid #B8B8B8; border-radius: 3px;"
               " background: rgba(0,0,0,0.25); }}"
               "QCheckBox::indicator:checked {{ background: #6CCB5F; border-color: #6CCB5F; }}")


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
    """One checkbox inside an IF block in switch mode."""

    def __init__(self, parent=None):
        super().__init__(parent, name="toggle", label="")
        from PySide6.QtWidgets import QCheckBox
        self.cb = QCheckBox(_t("Switch on (send to 'then')"))
        self.cb.setStyleSheet(CHECK_STYLE.format(fg="#F0F0F0", extra="font-weight: bold;"))
        self.set_custom_widget(self.cb)

    def get_value(self):
        return ""

    def set_value(self, value):
        pass

    def show(self, on):
        self.cb.blockSignals(True)
        self.cb.setChecked(bool(on))
        self.cb.blockSignals(False)


class InNode(BaseNode):
    __identifier__ = "proxydock"
    NODE_NAME = "Inbound"

    def __init__(self):
        super().__init__()
        self.add_input(_t("relay in"), multi_input=True, color=PORT_COLOR)
        self.add_output(_t("out"), multi_output=True, color=PORT_COLOR)
        self.add_custom_widget(InfoWidget(self.view, lines=3))
        self.add_custom_widget(SwitchesWidget(self.view))


class OutNode(BaseNode):
    """Outbound that can go through another outbound ("via" output)."""
    __identifier__ = "proxydock"
    NODE_NAME = "Outbound"

    def __init__(self):
        super().__init__()
        self.add_input(_t("in"), multi_input=True, color=PORT_COLOR)
        self.add_output(_t("via"), multi_output=False, color=VIA_COLOR)
        self.add_custom_widget(InfoWidget(self.view, lines=3))


class EndNode(BaseNode):
    """Outbound that ends a chain: Direct, Hysteria2 (QUIC cannot be tunnelled)."""
    __identifier__ = "proxydock"
    NODE_NAME = "Outbound "

    def __init__(self):
        super().__init__()
        self.add_input(_t("in"), multi_input=True, color=PORT_COLOR)
        self.add_custom_widget(InfoWidget(self.view, lines=3))


class GroupNode(BaseNode):
    """Group outbound: the 'members' output links to the outbound blocks it uses."""
    __identifier__ = "proxydock"
    NODE_NAME = "Group"

    def __init__(self):
        super().__init__()
        self.add_input(_t("in"), multi_input=True, color=PORT_COLOR)
        self.add_output(_t("members"), multi_output=True, color=MEMBER_COLOR)
        self.add_custom_widget(InfoWidget(self.view, lines=3))


class RouterNode(BaseNode):
    """Router: one output per rule plus 'default' (added when the canvas is built)."""
    __identifier__ = "proxydock"
    NODE_NAME = "Router"

    def __init__(self):
        super().__init__()
        self.add_input(_t("in"), multi_input=True, color=PORT_COLOR)
        self.add_custom_widget(InfoWidget(self.view, lines=3))


class RelayItem(NodeItem):
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
        painter.setPen(QPen(lane, 1.6))
        for i, o in self.links:
            if i < len(ins) and o < len(outs):
                a, b = ins[i], outs[o]
                path = QPainterPath(a)
                path.lineTo(b if self.routed else b - QPointF(5, 0))
                painter.setBrush(Qt.NoBrush)
                painter.drawPath(path)
                if not self.routed:         # arrow head
                    painter.setBrush(lane)
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


class _ViewerFilter(QObject):
    """Right-click on the canvas: our own (Fluent) menu instead of NodeGraphQt's."""

    def __init__(self, page):
        super().__init__(page)
        self.page = page

    def eventFilter(self, obj, event):
        if event.type() == QEvent.ContextMenu:
            self.page.context_menu(event.pos(), event.globalPos())
            return True
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
        self.graph.register_nodes([InNode, OutNode, EndNode, GroupNode, RouterNode, RelayNode, PcNode])
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
        viewer = self.graph.viewer()
        viewer.moved_nodes.connect(self._on_moved)
        self._filter = _ViewerFilter(self)
        viewer.installEventFilter(self._filter)
        QShortcut(QKeySequence.Delete, viewer, activated=self.delete_selected)

        title = TitleLabel(_t("Canvas"), self)
        self.btn_in = PrimaryDropDownPushButton(FIF.ADD, _t("Add inbound"), self)
        self.btn_in.setMenu(self._inbound_menu())
        self.btn_out = DropDownPushButton(FIF.SEND, _t("Add outbound"), self)
        self.btn_out.setMenu(self._outbound_menu())
        self.btn_router = DropDownPushButton(FIF.SHARE, _t("Add router"), self)
        self.btn_router.setMenu(self._router_menu())
        self.btn_fit = PushButton(_t("Fit"), self, FIF.ZOOM)
        self.btn_fit.clicked.connect(self.fit)
        self.btn_stop = PushButton(_t("Stop all"), self, FIF.PAUSE)
        self.btn_stop.clicked.connect(ctl.stop_all)
        hint = CaptionLabel(_t("Drag from an inbound's output to an outbound block, to another inbound (relay) or to the PC. "
                               "Double-click a block to edit it, right-click for more, Delete removes the selection.") + " "
                           + _t("An outbound's 'via' output chains it through another outbound.") + " "
                           + _t("A router sends each rule's traffic out of its own output.") + " "
                           + _t("A group's 'members' output uses several outbounds as one (fastest or failover).") + " "
                           + _t("Alt+Shift+drag cuts links."), self)
        hint.setWordWrap(True)
        self.pc_bar = PcBar(ctl, self)

        header = QHBoxLayout()
        header.addWidget(title)
        header.addStretch(1)
        for b in (self.btn_fit, self.btn_stop, self.btn_router, self.btn_out, self.btn_in):
            header.addWidget(b)
        root = QVBoxLayout(self)
        root.setContentsMargins(36, 20, 36, 20)
        root.setSpacing(10)
        root.addLayout(header)
        root.addWidget(hint)
        root.addWidget(self.pc_bar)
        root.addWidget(self.graph.widget, 1)
        self._apply_theme()
        qconfig.themeChanged.connect(self._on_theme_changed)

    def _on_theme_changed(self, *_):
        self._apply_theme()

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
        menu.addActions([a, b, c])
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
        node = None
        for item in viewer.items(pos):
            while item is not None and not isinstance(item, AbstractNodeItem):
                item = item.parentItem()
            if item is not None:
                node = self.graph.get_node_by_id(item.id)
                break
        menu = RoundMenu(parent=self)
        p = [scene_pos.x(), scene_pos.y()]
        if node is None:
            menu.addMenu(self._inbound_menu(p, menu))
            menu.addMenu(self._outbound_menu(p, menu))
            menu.addMenu(self._router_menu(p, menu))
            menu.addSeparator()
            fit = Action(FIF.ZOOM, _t("Fit"), menu)
            fit.triggered.connect(self.fit)
            menu.addAction(fit)
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

    def delete_selected(self):
        ids = [self.mid_of.get(n.id) for n in self.graph.selected_nodes()]
        ids = [i for i in ids if i and i != PC_ID]
        if ids:
            confirm_delete(self.ctl, ids, self.window())

    def fit(self):
        if self.graph.all_nodes():
            self._fit_all()

    def _fit_all(self):
        self.graph.select_all()
        self.graph.fit_to_selection()
        self.graph.clear_selection()

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
        return json.dumps([[n["id"], n["name"], n["target"]] for n in graph["inbounds"]] +
                          [[n["id"], n["name"], n["via"], n["outbound"]["type"], n.get("members", [])] for n in graph["outbounds"]] +
                          [[n["id"], n["name"], n["default"], n.get("switch"), [[x["id"], x["name"], x["target"]] for x in n["rules"]]]
                           for n in graph.get("routers", [])] +
                          [[n["id"], n["name"], n["routed"], [[x["id"], x["gap"], x["out"]] for x in n["inputs"]],
                            [[x["id"], x["gap"], x["target"]] for x in n["outputs"]]] for n in graph.get("relays", [])] +
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
            g.clear_session()
            self.node_of, self.mid_of, self.rule_of = {}, {}, {}
            self.in_lane_of, self.out_lane_of, self.lane_port = {}, {}, {}

            def make(kind, mid, name, pos, color):
                node = g.create_node(f"proxydock.{kind}", name=name, pos=list(pos), color=color,
                                     text_color=(235, 235, 235), selected=False, push_undo=False)
                self.node_of[mid] = node
                self.mid_of[node.id] = mid
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
                if r.get("switch"):
                    tw = ToggleWidget(node.view)
                    node.add_custom_widget(tw)
                    tw.cb.toggled.connect(lambda on, rid=r["id"]: self.ctl.set_switch(rid, on))
            for r in graph.get("relays", []):
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
                node = self.node_of[r["id"]]
                for i, x in enumerate(r["outputs"]):
                    dst = port_in(x["target"])
                    if dst is not None:
                        node.output(i).connect_to(dst, push_undo=False)
            if graph["pc"]["inbound"] in self.node_of:
                self.node_of[graph["pc"]["inbound"]].output(0).connect_to(self.node_of[PC_ID].input(0), push_undo=False)
            g.clear_undo_stack()
        finally:
            self.syncing = False
        if not self.fitted and self.isVisible():
            self.fitted = True
            self._fit_all()

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
            _, d = graph_mod.find(cg, r["default"]) if r["default"] else (None, None)
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
            node.view.setToolTip("\n".join(tips))
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
        src = self.mid_of.get(out_port.node().id)
        dst = self.mid_of.get(in_port.node().id)
        if not src or not dst:
            return
        from .. import graph as graph_mod
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
        src = self.mid_of.get(out_port.node().id)
        dst = self.mid_of.get(in_port.node().id)
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

    def _on_double_clicked(self, node):
        mid = self.mid_of.get(node.id)
        if mid and mid != PC_ID:
            QTimer.singleShot(0, self, lambda: self.ctl.edit_node(mid))

    def _on_moved(self, *_):
        positions = {}
        for nid, node in self.node_of.items():
            x, y = node.pos()
            positions[nid] = [round(x, 1), round(y, 1)]
        self.ctl.move_nodes(positions)
