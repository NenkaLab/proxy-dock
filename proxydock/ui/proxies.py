"""Proxies page: list (cards) + editor.

A proxy = inbound (how it accepts connections on a port of this PC) + outbound (how it reaches the destination) + local settings.
"""
from ..i18n import _t, on_language_change
import copy
import os
import threading
import time

from PySide6.QtCore import Qt, Signal
from PySide6.QtGui import QColor, QPainter
from PySide6.QtWidgets import QApplication, QFileDialog, QHBoxLayout, QLabel, QVBoxLayout, QWidget
from qfluentwidgets import (
    Action, BodyLabel, CaptionLabel, CardWidget, CheckBox, ComboBox, DropDownPushButton, FluentIcon as FIF, LineEdit,
    MessageBox,
    EditableComboBox, ListWidget, PasswordLineEdit, PrimaryDropDownPushButton, PrimaryPushButton, PushButton, RoundMenu, ScrollArea,
    SimpleCardWidget, SpinBox, StrongBodyLabel, SubtitleLabel, SwitchButton, TitleLabel, TransparentToolButton,
    isDarkTheme,
)

from .. import config
from ..common import INBOUND_TYPES, OUTBOUND_TYPES, host_port, is_loopback_host
from ..platform_utils import check_port, local_lan_ip
from .common import STATE_COLORS, STATE_TEXT, info


# --- shared ---

def client_url(p):
    """Address/link to put into a client app."""
    from ..hysteria2 import hy2_link
    from ..ss import ss_link
    ib = p["inbound"]
    host = ib["host"]
    if host in ("0.0.0.0", "::"):
        host = local_lan_ip()
    t = ib["type"]
    if t == "tun":
        return ""
    if t == "shadowsocks":
        return ss_link(host, ib["port"], ib["ss_method"], ib["ss_password"], p["name"])
    if t == "hysteria2":
        return hy2_link(host, ib["port"], ib["hy2_password"], insecure=not ib["tls_cert"], name=p["name"],
                        obfs=ib.get("hy2_obfs_password", ""))
    scheme = {"socks5": "socks5", "socks4": "socks4a", "http": "http", "https": "https"}[t]
    cred = ""
    if t in ("socks5", "http", "https") and ib["auth_enabled"]:
        cred = f"{ib['username']}:{ib['password']}@"
    return f"{scheme}://{cred}{host_port(host, ib['port'])}"


def summary(p):
    ib, ob = p["inbound"], p["outbound"]
    from ..common import inbound_where
    left = f"{INBOUND_TYPES[ib['type']]} {inbound_where(ib)}"
    if "target_name" in p:
        right = p["target_name"]
    else:
        right = "Direct" if ob["type"] == "direct" else OUTBOUND_TYPES["block"] if ob["type"] == "block" \
            else f"{OUTBOUND_TYPES[ob['type']]} {host_port(ob['host'], ob['port'])}"
    return f"{left}  →  {right}"


def outbound_summary(ob, members=None):
    """members: names of a group's members (in order)."""
    if ob["type"] == "block":
        return _t("Refuses every connection (use it as a router target)")
    if ob["type"] == "group":
        mode = _t("Fastest") if ob.get("group_mode") != "failover" else _t("Failover in order")
        return f"{OUTBOUND_TYPES['group']} · {mode} · " + (" / ".join(members) if members else _t("no members"))
    iface = f" · ⇢ {ob['interface']}" if ob.get("interface") else ""
    if ob["type"] == "direct":
        return _t("Direct from this PC") + (f" · {ob['bind']}" if ob.get("bind") else "") + iface
    return f"{OUTBOUND_TYPES[ob['type']]} · {host_port(ob['host'], ob['port']) if ob['host'] else _t('server not set')}" + iface


def dns_combo(parent):
    """Editable DNS server field with common choices (empty = the PC's DNS)."""
    cb = EditableComboBox(parent)
    cb.addItems(config.DNS_PRESETS)
    cb.setPlaceholderText(_t("The PC's DNS"))
    cb.setFixedWidth(300)
    cb.setText("")
    return cb


def network_interfaces():
    """[(name, label)] of the network interfaces that are up (loopback excluded), e.g. ("Tailscale", "Tailscale · 100.x")."""
    from PySide6.QtNetwork import QAbstractSocket, QNetworkInterface as Q
    out = []
    for i in Q.allInterfaces():
        f = i.flags()
        if not (f & Q.IsUp) or f & Q.IsLoopBack:
            continue
        ips = [a.ip().toString() for a in i.addressEntries()
               if a.ip().protocol() == QAbstractSocket.IPv4Protocol and not a.ip().toString().startswith("169.254.")]
        name = i.humanReadableName()
        out.append((name, f"{name} · {ips[0]}" if ips else name))
    return sorted(out, key=lambda x: x[0].lower())


OUTBOUND_NEW = []


@on_language_change
def _tr_outbound_new():
    OUTBOUND_NEW[:] = [
        ("direct", _t("Direct (optionally from a chosen source IP)")),
        ("http", _t("Upstream HTTP proxy")),
        ("https", _t("Upstream HTTPS proxy")),
        ("socks5", _t("Upstream SOCKS5 proxy")),
        ("socks4", _t("Upstream SOCKS4 proxy")),
        ("shadowsocks", _t("Shadowsocks tunnel (remote server)")),
        ("hysteria2", _t("Hysteria2 tunnel (remote server)")),
        ("wireguard", _t("WireGuard tunnel (no VPN app needed)")),
        ("vless", _t("VLESS tunnel (TLS / REALITY)")),
        ("trojan", _t("Trojan tunnel")),
        ("tuic", _t("TUIC tunnel (QUIC)")),
        ("subscription", _t("Subscription (a provider's server list from a URL)")),
        ("group", _t("Group (fastest server or failover between several outbounds)")),
        ("block", _t("Block (refuse connections, for router rules)")),
    ]




class PcBar(SimpleCardWidget):
    """PC (system proxy) status with 'Restore previous' and 'Off (default)' buttons."""

    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        self.ctl = ctl
        self.dot = Dot(self)
        self.title = StrongBodyLabel(_t("PC (system proxy)"), self)
        self.text = CaptionLabel("", self)
        self.btn_restore = PushButton(_t("Restore previous"), self, FIF.HISTORY)
        self.btn_restore.setToolTip(_t("Disconnect and put back the system proxy settings from before ProxyDock changed them"))
        self.btn_restore.clicked.connect(ctl.pc_restore)
        self.btn_off = PushButton(_t("Off (default)"), self, FIF.CANCEL)
        self.btn_off.setToolTip(_t("Disconnect and turn the system proxy off"))
        self.btn_off.clicked.connect(ctl.pc_off)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(16, 8, 12, 8)
        lay.setSpacing(10)
        lay.addWidget(self.dot, 0, Qt.AlignVCenter)
        lay.addWidget(self.title, 0, Qt.AlignVCenter)
        lay.addWidget(self.text, 1, Qt.AlignVCenter)
        lay.addWidget(self.btn_restore)
        lay.addWidget(self.btn_off)

    def set_status(self, status):
        state, text = status
        self.dot.set_color({"applied": "#2E9E4F", "waiting": "#C99A06", "error": "#C42B1C"}.get(state, "#8A8A8A"))
        self.text.setText(text)
        supported = state != "unsupported"
        self.btn_restore.setEnabled(supported)
        self.btn_off.setEnabled(supported)


class Dot(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.color = QColor("#8A8A8A")
        self.setFixedSize(12, 12)

    def set_color(self, c):
        self.color = QColor(c)
        self.update()

    def paintEvent(self, _):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        p.setPen(Qt.NoPen)
        p.setBrush(self.color)
        p.drawEllipse(1, 1, 10, 10)
        p.end()


# --- list ---

TEMPLATES = []


@on_language_change
def _tr_templates():
    TEMPLATES[:] = [
        (_t("Local SOCKS5"), _t("SOCKS5 proxy for this PC"), {"type": "socks5", "port": 1080}, {"type": "direct"}),
        (_t("Local HTTP"), _t("For HTTP proxy settings in browsers and apps"), {"type": "http", "port": 8080}, {"type": "direct"}),
        (_t("HTTPS proxy"), _t("HTTP proxy wrapped in TLS (self-signed certificate generated)"), {"type": "https", "port": 8443},
         {"type": "direct"}),
        ("SOCKS4", _t("For older programs (no authentication)"), {"type": "socks4", "port": 1084}, {"type": "direct"}),
        (_t("Shadowsocks server"), _t("Shadowsocks apps on other devices connect to it"), {"type": "shadowsocks", "host": "0.0.0.0", "port": 8388},
         {"type": "direct"}),
        (_t("Hysteria2 server"), _t("Hysteria2 apps on other devices connect to it (QUIC)"), {"type": "hysteria2", "host": "0.0.0.0", "port": 8443},
         {"type": "direct"}),
        (_t("Local → upstream proxy"), _t("Local SOCKS5, forwarded to a company/other HTTP or SOCKS proxy"), {"type": "socks5", "port": 1080},
         {"type": "http"}),
        (_t("Local → Shadowsocks tunnel"), _t("Local SOCKS5, encrypted to a remote Shadowsocks server"), {"type": "socks5", "port": 1080},
         {"type": "shadowsocks"}),
        (_t("Local → Hysteria2 tunnel"), _t("Local SOCKS5, to a remote Hysteria2 server (QUIC)"), {"type": "socks5", "port": 1080},
         {"type": "hysteria2"}),
    ]




class ProxyCard(CardWidget):
    def __init__(self, page, proxy, parent=None):
        super().__init__(parent)
        self.page = page
        self.pid = proxy["id"]
        self.dot = Dot(self)
        self.name = StrongBodyLabel("", self)
        self.desc = CaptionLabel("", self)
        self.state = CaptionLabel("", self)
        self.switch = SwitchButton(self)
        self.switch.setOnText(_t("Run"))
        self.switch.setOffText(_t("Stop"))
        self.switch.checkedChanged.connect(lambda on: page.ctl.toggle_proxy(self.pid, on))
        self.btn_copy = TransparentToolButton(FIF.COPY, self)
        self.btn_copy.setToolTip(_t("Copy address/link for clients"))
        self.btn_copy.clicked.connect(lambda: page.copy_url(self.pid))
        self.btn_edit = TransparentToolButton(FIF.EDIT, self)
        self.btn_edit.setToolTip(_t("Edit"))
        self.btn_edit.clicked.connect(lambda: page.ctl.edit_proxy(self.pid))
        self.btn_more = TransparentToolButton(FIF.MORE, self)
        self.btn_more.clicked.connect(self._menu)
        self.clicked.connect(lambda: page.ctl.edit_proxy(self.pid))

        text = QVBoxLayout()
        text.setSpacing(2)
        top = QHBoxLayout()
        top.setSpacing(8)
        top.addWidget(self.dot, 0, Qt.AlignVCenter)
        top.addWidget(self.name)
        top.addWidget(self.state)
        top.addStretch(1)
        text.addLayout(top)
        text.addWidget(self.desc)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(18, 12, 12, 12)
        lay.addLayout(text, 1)
        for w in (self.switch, self.btn_copy, self.btn_edit, self.btn_more):
            lay.addWidget(w, 0, Qt.AlignVCenter)
        self.update_from(proxy, ("stopped", ""))

    def update_from(self, proxy, status):
        state, msg = status
        self.name.setText(proxy["name"] + (_t("   · autostart") if proxy["autostart"] else ""))
        self.desc.setText(summary(proxy))
        full = STATE_TEXT.get(state, state) + (f" - {msg}" if msg else "")
        fm = self.state.fontMetrics()
        self.state.setText(fm.elidedText(full, Qt.ElideRight, 520))
        self.state.setToolTip(full if len(full) > 30 else "")
        self.dot.set_color(STATE_COLORS.get(state, "#8A8A8A"))
        self.switch.blockSignals(True)
        self.switch.setChecked(state in ("running", "starting"))
        self.switch.blockSignals(False)

    def _menu(self):
        m = RoundMenu(parent=self)
        a1 = Action(FIF.COPY, _t("Duplicate"), m)
        a1.triggered.connect(lambda: self.page.ctl.duplicate_proxy(self.pid))
        a3 = Action(FIF.GLOBE, _t("Use as the PC proxy"), m)
        a3.triggered.connect(lambda: self.page.ctl.set_pc(self.pid))
        a2 = Action(FIF.DELETE, _t("Delete"), m)
        a2.triggered.connect(lambda: self.page.confirm_delete(self.pid))
        m.addActions([a1, a3, a2])
        m.exec(self.btn_more.mapToGlobal(self.btn_more.rect().bottomLeft()))


class OutboundCard(CardWidget):
    def __init__(self, page, node, parent=None):
        super().__init__(parent)
        self.page = page
        self.nid = node["id"]
        self.icon = TransparentToolButton(FIF.SEND, self)
        self.icon.setEnabled(False)
        self.name = StrongBodyLabel("", self)
        self.desc = CaptionLabel("", self)
        self.used = CaptionLabel("", self)
        self.btn_edit = TransparentToolButton(FIF.EDIT, self)
        self.btn_edit.setToolTip(_t("Edit"))
        self.btn_edit.clicked.connect(lambda: page.ctl.edit_node(self.nid))
        self.btn_more = TransparentToolButton(FIF.MORE, self)
        self.btn_more.clicked.connect(self._menu)
        self.clicked.connect(lambda: page.ctl.edit_node(self.nid))
        text = QVBoxLayout()
        text.setSpacing(2)
        top = QHBoxLayout()
        top.setSpacing(8)
        top.addWidget(self.name)
        top.addWidget(self.used)
        top.addStretch(1)
        text.addLayout(top)
        text.addWidget(self.desc)
        lay = QHBoxLayout(self)
        lay.setContentsMargins(12, 10, 12, 10)
        lay.addWidget(self.icon, 0, Qt.AlignVCenter)
        lay.addLayout(text, 1)
        lay.addWidget(self.btn_edit, 0, Qt.AlignVCenter)
        lay.addWidget(self.btn_more, 0, Qt.AlignVCenter)

    def update_from(self, node, user_names):
        self.name.setText(node["name"])
        from .. import graph as graph_mod
        chain = graph_mod.chain_names(self.page.ctl.graph, node["via"]) if node.get("via") else []
        g = self.page.ctl.graph
        members = [graph_mod.find(g, m)[1]["name"] for m in node.get("members", []) if graph_mod.find(g, m)[1]]
        self.desc.setText(outbound_summary(node["outbound"], members) + ("  ⇢ " + " ⇢ ".join(chain) if chain else ""))
        self.used.setText(_t("used by: {names}").format(names=", ".join(user_names)) if user_names else _t("not used"))

    def _menu(self):
        m = RoundMenu(parent=self)
        a1 = Action(FIF.COPY, _t("Duplicate"), m)
        a1.triggered.connect(lambda: self.page.ctl.duplicate_node(self.nid))
        a2 = Action(FIF.DELETE, _t("Delete"), m)
        a2.triggered.connect(lambda: self.page.confirm_delete(self.nid))
        m.addActions([a1, a2])
        m.exec(self.btn_more.mapToGlobal(self.btn_more.rect().bottomLeft()))


class ProxiesInterface(ScrollArea):
    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        self.setObjectName("proxiesInterface")
        self.ctl = ctl
        self.cards = {}
        self.view = QWidget(self)
        self.setWidget(self.view)
        self.setWidgetResizable(True)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        self.enableTransparentBackground()

        title = TitleLabel(_t("Proxy"), self.view)
        self.btn_new = PrimaryDropDownPushButton(FIF.ADD, _t("New proxy"), self.view)
        menu = RoundMenu(parent=self.btn_new)
        for name, desc, ib, ob in TEMPLATES:
            act = Action(f"{name}  —  {desc}", menu)
            act.triggered.connect(lambda _=False, n=name, i=ib, o=ob: ctl.new_proxy(n, i, o))
            menu.addAction(act)
        self.btn_new.setMenu(menu)
        self.btn_new_out = DropDownPushButton(FIF.SEND, _t("New outbound"), self.view)
        omenu = RoundMenu(parent=self.btn_new_out)
        for t, label in OUTBOUND_NEW:
            act = Action(label, omenu)
            act.triggered.connect(lambda _=False, t=t: ctl.new_outbound({"type": t}))
            omenu.addAction(act)
        self.btn_new_out.setMenu(omenu)
        self.out_cards = {}
        self.pc_bar = PcBar(ctl, self.view)
        self.btn_stop = PushButton(_t("Stop all"), self.view, FIF.PAUSE)
        self.btn_stop.clicked.connect(ctl.stop_all)
        self.empty = BodyLabel(_t("No proxies yet. Create one with 'New proxy'."), self.view)

        header = QHBoxLayout()
        header.addWidget(title)
        header.addStretch(1)
        header.addWidget(self.btn_stop)
        header.addWidget(self.btn_new_out)
        header.addWidget(self.btn_new)
        self.hint = CaptionLabel(_t("Proxy = inbound (how it accepts connections on a port of this PC) → outbound (how it reaches the destination). Click a card to edit; use the switch to start/stop."), self.view)
        self.hint.setWordWrap(True)
        self.list = QVBoxLayout()
        self.list.setSpacing(8)
        self.out_list = QVBoxLayout()
        self.out_list.setSpacing(8)
        self.out_empty = CaptionLabel(_t("No outbound blocks."), self.view)
        root = QVBoxLayout(self.view)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(12)
        root.addLayout(header)
        root.addWidget(self.hint)
        root.addWidget(self.pc_bar)
        root.addWidget(SubtitleLabel(_t("Inbound"), self.view))
        root.addWidget(self.empty)
        root.addLayout(self.list)
        root.addSpacing(8)
        root.addWidget(SubtitleLabel(_t("Outbound"), self.view))
        root.addWidget(self.out_empty)
        root.addLayout(self.out_list)
        root.addStretch(1)

    def refresh(self, graph, proxies, statuses):
        from .. import graph as graph_mod
        out_ids = [n["id"] for n in graph["outbounds"]]
        for nid in list(self.out_cards):
            if nid not in out_ids:
                self.out_cards.pop(nid).deleteLater()
        for i, n in enumerate(graph["outbounds"]):
            card = self.out_cards.get(n["id"])
            if card is None:
                card = self.out_cards[n["id"]] = OutboundCard(self, n, self.view)
            self.out_list.removeWidget(card)
            self.out_list.insertWidget(i, card)
            card.update_from(n, [u["name"] for u in graph_mod.users(graph, n["id"])])
        self.out_empty.setVisible(not graph["outbounds"])
        self.pc_bar.set_status(self.ctl.pc_status())
        ids = [p["id"] for p in proxies]
        for pid in list(self.cards):
            if pid not in ids:
                self.cards.pop(pid).deleteLater()
        for i, p in enumerate(proxies):
            card = self.cards.get(p["id"])
            if card is None:
                card = self.cards[p["id"]] = ProxyCard(self, p, self.view)
            self.list.removeWidget(card)
            self.list.insertWidget(i, card)
            card.update_from(p, statuses.get(p["id"], ("stopped", "")))
        self.empty.setVisible(not proxies)

    def set_status(self, pid, status, proxy):
        card = self.cards.get(pid)
        if card is not None and proxy is not None:
            card.update_from(proxy, status)

    def copy_url(self, pid):
        p = self.ctl.proxy_by_id(pid)
        if p is None:
            return
        url = client_url(p)
        if not url:
            info(self.window(), "info", _t("Nothing to copy"),
                 _t("A TUN card captures apps on this PC directly; there is no address for clients"), 3500)
            return
        QApplication.clipboard().setText(url)
        shown = url if len(url) < 70 else url[:67] + "…"
        info(self.window(), "success", _t("Copied"), shown, 2500)

    def confirm_delete(self, nid):
        confirm_delete(self.ctl, [nid], self.window())


def confirm_delete(ctl, ids, parent):
    """Ask, then delete blocks (inbound blocks are stopped first)."""
    from .. import graph as graph_mod
    names = [n["name"] for n in (graph_mod.find(ctl.graph, i)[1] for i in ids) if n is not None]
    if not names:
        return
    box = MessageBox(_t("Delete proxy"), _t("Delete '{name}'? It will be stopped if it is running.").format(name=", ".join(names)), parent)
    box.yesButton.setText(_t("Delete"))
    box.cancelButton.setText(_t("Cancel"))
    if box.exec():
        for i in ids:
            ctl.delete_node(i)


# --- editor ---

class Section(QWidget):
    """Title + list of cards (hidden cards take no space)."""

    def __init__(self, title, parent=None):
        super().__init__(parent)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(6)
        lay.addWidget(SubtitleLabel(title, self))
        self.lay = lay

    def add(self, w):
        self.lay.addWidget(w)
        return w


class Row(SimpleCardWidget):
    """One settings row: title/description + widgets on the right."""

    def __init__(self, title, desc="", parent=None):
        super().__init__(parent)
        self.title = BodyLabel(title, self)
        self.desc = CaptionLabel(desc, self)
        self.desc.setWordWrap(True)
        self.desc.setVisible(bool(desc))
        col = QVBoxLayout()
        col.setSpacing(1)
        col.addWidget(self.title)
        col.addWidget(self.desc)
        self.hl = QHBoxLayout(self)
        self.hl.setContentsMargins(16, 10, 16, 10)
        self.hl.setSpacing(10)
        self.hl.addLayout(col, 1)

    def right(self, *widgets):
        for w in widgets:
            self.hl.addWidget(w, 0, Qt.AlignVCenter)
        return self


def _switch(parent, on=None, off=None):
    on = _t("On") if on is None else on
    off = _t("Off") if off is None else off
    sw = SwitchButton(parent)
    sw.setOnText(on)
    sw.setOffText(off)
    return sw


class EditorInterface(ScrollArea):
    """Proxy editor (not in the navigation menu; opened from the list)."""

    closed = Signal()
    check_done = Signal(object, object)     # (outbound result, inbound result)
    sub_done = Signal(object, str)          # (subscription fields or None, note / error)
    detect_done = Signal(object, str)       # (certificate info or None, error)

    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        from ..ss import SS_METHODS
        self.setObjectName("editorInterface")
        self.ctl = ctl
        self.kind = "inbound"
        self.node = None
        self.ss_methods = list(SS_METHODS)
        self.view = QWidget(self)
        self.setWidget(self.view)
        self.setWidgetResizable(True)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        self.enableTransparentBackground()
        v = self.view

        self.title = TitleLabel(_t("Edit proxy"), v)
        self.btn_cancel = PushButton(_t("Cancel"), v)
        self.btn_cancel.clicked.connect(self.cancel)
        self.btn_save = PrimaryPushButton(_t("Save"), v, FIF.SAVE)
        self.btn_save.clicked.connect(self.save)

        # general
        s_basic = Section(_t("General"), v)
        self.ed_name = LineEdit(v)
        self.ed_name.setFixedWidth(260)
        s_basic.add(Row(_t("Name"), parent=v).right(self.ed_name))
        self.sw_auto = _switch(v)
        self.r_auto = s_basic.add(Row(_t("Start with the app"), _t("Starts this proxy when ProxyDock starts"), v).right(self.sw_auto))

        # inbound
        s_in = Section(_t("Inbound (how this PC accepts connections)"), v)
        self.cb_in = ComboBox(v)
        self.in_types = list(INBOUND_TYPES)
        self.cb_in.addItems([INBOUND_TYPES[t] for t in self.in_types])
        self.cb_in.setFixedWidth(200)
        self.r_in_type = s_in.add(Row(_t("Protocol"), "", v).right(self.cb_in))
        self.ed_in_host = LineEdit(v)
        self.ed_in_host.setFixedWidth(170)
        self.sp_in_port = SpinBox(v)
        self.sp_in_port.setRange(1, 65535)
        self.sp_in_port.setFixedWidth(150)
        btn_check = PushButton(_t("Check availability"), v, FIF.SEARCH)
        btn_check.clicked.connect(self._check_port)
        self.r_in_listen = s_in.add(Row(_t("Listen address · port"), _t("127.0.0.1 = this PC only, 0.0.0.0 = allow other PCs (check authentication and firewall)"), v)
                                    .right(self.ed_in_host, self.sp_in_port, btn_check))
        from ..common import TUN_STACKS
        self.tun_stacks = list(TUN_STACKS)
        self.cb_tun_stack = ComboBox(v)
        self.cb_tun_stack.addItems([_t("mixed (recommended)"), "system", "gvisor"])
        self.cb_tun_stack.setFixedWidth(200)
        self.r_tun_stack = s_in.add(Row(_t("Network stack"), _t("How sing-box handles the captured packets. Try another one if some apps misbehave"), v)
                                    .right(self.cb_tun_stack))
        self.sw_tun_strict = _switch(v)
        self.r_tun_strict = s_in.add(Row(_t("Strict route"), _t("Also stops traffic that tries to go around the TUN. Can break IPv6 on Windows; leave off unless needed"), v)
                                     .right(self.sw_tun_strict))
        self.sw_tun_fakeip = _switch(v)
        self.r_tun_fakeip = s_in.add(Row(_t("Fake-IP DNS"), _t("Apps get placeholder addresses (198.18.x.x) at once and the real name is looked up where the traffic leaves (e.g. at the VPN server), so names do not leak to the local network. Needs the sing-box engine"), v)
                                     .right(self.sw_tun_fakeip))
        self.sw_in_auth = _switch(v)
        self.r_in_auth = s_in.add(Row(_t("User authentication"), "", v).right(self.sw_in_auth))
        self.ed_in_user = LineEdit(v)
        self.ed_in_user.setPlaceholderText(_t("Username"))
        self.ed_in_user.setFixedWidth(150)
        self.ed_in_pass = PasswordLineEdit(v)
        self.ed_in_pass.setPlaceholderText(_t("Password"))
        self.ed_in_pass.setFixedWidth(180)
        self.r_in_cred = s_in.add(Row(_t("Account"), "", v).right(self.ed_in_user, self.ed_in_pass))
        self.ed_cert = LineEdit(v)
        self.ed_cert.setPlaceholderText(_t("Leave empty to generate"))
        self.ed_cert.setFixedWidth(220)
        b1 = PushButton(_t("Browse"), v, FIF.FOLDER)
        b1.clicked.connect(lambda: self._pick(self.ed_cert, _t("Certificates (*.crt *.pem *.cer);;All files (*)")))
        self.r_cert = s_in.add(Row(_t("TLS certificate"), _t("PEM format. If empty, a self-signed certificate is generated (clients must skip certificate verification)"), v)
                               .right(self.ed_cert, b1))
        self.ed_key = LineEdit(v)
        self.ed_key.setPlaceholderText(_t("Leave empty to generate"))
        self.ed_key.setFixedWidth(220)
        b2 = PushButton(_t("Browse"), v, FIF.FOLDER)
        b2.clicked.connect(lambda: self._pick(self.ed_key, _t("Private keys (*.key *.pem);;All files (*)")))
        self.r_key = s_in.add(Row(_t("TLS private key"), "", v).right(self.ed_key, b2))
        self.cb_in_ss = ComboBox(v)
        self.cb_in_ss.addItems(self._ss_labels())
        self.cb_in_ss.setFixedWidth(330)
        self.r_in_ss_method = s_in.add(Row(_t("Cipher"), _t("Must match the client"), v).right(self.cb_in_ss))
        self.ed_in_ss_pw = PasswordLineEdit(v)
        self.ed_in_ss_pw.setFixedWidth(260)
        g1 = PushButton(_t("Generate"), v, FIF.SYNC)
        g1.clicked.connect(lambda: self._gen_ss(self.cb_in_ss, self.ed_in_ss_pw))
        self.r_in_ss_pw = s_in.add(Row(_t("Shadowsocks password"), _t("2022 ciphers use a base64 key - click 'Generate' after changing the cipher"), v)
                                   .right(self.ed_in_ss_pw, g1))
        self.ed_in_hy = PasswordLineEdit(v)
        self.ed_in_hy.setFixedWidth(260)
        g2 = PushButton(_t("Generate"), v, FIF.SYNC)
        g2.clicked.connect(lambda: self.ed_in_hy.setText(_token()))
        self.r_in_hy = s_in.add(Row(_t("Hysteria2 password"), _t("At least 8 characters"), v).right(self.ed_in_hy, g2))
        self.ed_in_obfs = PasswordLineEdit(v)
        self.ed_in_obfs.setPlaceholderText(_t("(off)"))
        self.ed_in_obfs.setFixedWidth(260)
        g3 = PushButton(_t("Generate"), v, FIF.SYNC)
        g3.clicked.connect(lambda: self.ed_in_obfs.setText(_token()))
        self.r_in_obfs = s_in.add(Row(_t("Salamander obfuscation"), _t("Makes the traffic look like random data. Clients need the same password. Needs the sing-box engine"), v)
                                  .right(self.ed_in_obfs, g3))
        self.sp_in_up = SpinBox(v)
        self.sp_in_up.setRange(0, 100000)
        self.sp_in_up.setFixedWidth(150)
        self.sp_in_down = SpinBox(v)
        self.sp_in_down.setRange(0, 100000)
        self.sp_in_down.setFixedWidth(150)
        self.r_in_bw = s_in.add(Row(_t("Speed limit up · down (Mbps)"), _t("Upper limit per client. 0 = no limit"), v)
                                .right(self.sp_in_up, self.sp_in_down))
        self.in_note = CaptionLabel("", v)
        self.in_note.setWordWrap(True)
        s_in.add(self.in_note)

        # outbound
        s_out = Section(_t("Outbound (how traffic reaches the destination)"), v)
        self.cb_out = ComboBox(v)
        self.out_types = list(OUTBOUND_TYPES)
        self.cb_out.addItems([OUTBOUND_TYPES[t] for t in self.out_types])
        self.cb_out.setFixedWidth(200)
        b_link = PushButton(_t("Paste share link"), v, FIF.PASTE)
        b_link.clicked.connect(self._paste_link)
        s_out.add(Row(_t("Type"), _t("Direct = connect straight from this PC. The others go through another proxy/server"), v)
                  .right(b_link, self.cb_out))
        self.ed_out_host = LineEdit(v)
        self.ed_out_host.setPlaceholderText(_t("Server address"))
        self.ed_out_host.setFixedWidth(220)
        self.sp_out_port = SpinBox(v)
        self.sp_out_port.setRange(0, 65535)
        self.sp_out_port.setFixedWidth(150)
        self.r_out_server = s_out.add(Row(_t("Server address · port"), "", v).right(self.ed_out_host, self.sp_out_port))
        self.cb_via = ComboBox(v)
        self.cb_via.setFixedWidth(320)
        self.r_out_via = s_out.add(Row(_t("Through"), _t("Connect to this server through another outbound block (e.g. a tunnel through the company proxy). Also editable on the canvas."), v)
                                   .right(self.cb_via))
        self.vias = []
        self.ed_out_bind = LineEdit(v)
        self.ed_out_bind.setPlaceholderText(_t("Automatic"))
        self.ed_out_bind.setFixedWidth(220)
        self.r_out_bind = s_out.add(Row(_t("Source address"), _t("Connect from this IP of the PC (choose a network card / VPN). Empty = automatic"), v)
                                    .right(self.ed_out_bind))
        self.cb_out_iface = ComboBox(v)
        self.cb_out_iface.setFixedWidth(320)
        b_iface = TransparentToolButton(FIF.SYNC, v)
        b_iface.setToolTip(_t("Refresh the list"))
        b_iface.clicked.connect(lambda: self._fill_ifaces(self._iface_value()))
        self.r_out_iface = s_out.add(Row(_t("Network interface"), _t("Leave through this network card, e.g. Tailscale, WireGuard, a VPN app or Wi-Fi, without changing the PC's routes. Needs the sing-box engine."), v)
                                     .right(self.cb_out_iface, b_iface))
        self.ifaces = [""]
        self.ed_out_user = LineEdit(v)
        self.ed_out_user.setPlaceholderText(_t("Username (leave empty if none)"))
        self.ed_out_user.setFixedWidth(170)
        self.ed_out_pass = PasswordLineEdit(v)
        self.ed_out_pass.setPlaceholderText(_t("Password"))
        self.ed_out_pass.setFixedWidth(180)
        self.r_out_cred = s_out.add(Row(_t("Upstream proxy account"), "", v).right(self.ed_out_user, self.ed_out_pass))
        # subscription
        self.ed_sub_url = PasswordLineEdit(v)
        self.ed_sub_url.setPlaceholderText("https://...")
        self.ed_sub_url.setFixedWidth(330)
        self.btn_sub_update = PushButton(_t("Update now"), v, FIF.SYNC)
        self.btn_sub_update.clicked.connect(self._sub_update)
        self.r_sub_url = s_out.add(Row(_t("Subscription address"), "", v).right(self.ed_sub_url, self.btn_sub_update))
        self.sub_modes = ["auto", "select"]
        self.cb_sub_mode = ComboBox(v)
        self.cb_sub_mode.addItems([_t("Fastest automatically"), _t("Pick a server")])
        self.cb_sub_mode.setFixedWidth(200)
        self.cb_sub_server = ComboBox(v)
        self.cb_sub_server.setFixedWidth(260)
        self.cb_sub_server.setMaxVisibleItems(15)
        self.r_sub_mode = s_out.add(Row(_t("Server"), _t("Fastest automatically measures the servers every few minutes and moves away from ones that stop working"), v)
                                    .right(self.cb_sub_mode, self.cb_sub_server))
        self.sp_sub_hours = SpinBox(v)
        self.sp_sub_hours.setRange(0, 168)
        self.sp_sub_hours.setFixedWidth(150)
        self.r_sub_hours = s_out.add(Row(_t("Update every (hours)"), _t("0 = only when you click 'Update now'"), v).right(self.sp_sub_hours))
        self.sub_done.connect(self._on_sub_done)
        # group
        self.grp_modes = ["auto", "failover"]
        self.cb_grp_mode = ComboBox(v)
        self.cb_grp_mode.addItems([_t("Fastest automatically"), _t("Failover in order")])
        self.cb_grp_mode.setFixedWidth(220)
        self.r_grp_mode = s_out.add(Row(_t("Choose"), "", v).right(self.cb_grp_mode))
        self.lst_grp = ListWidget(v)
        self.lst_grp.setFixedSize(330, 150)
        b_up = TransparentToolButton(FIF.UP, v)
        b_up.setToolTip(_t("Move up"))
        b_up.clicked.connect(lambda: self._grp_move(-1))
        b_down = TransparentToolButton(FIF.DOWN, v)
        b_down.setToolTip(_t("Move down"))
        b_down.clicked.connect(lambda: self._grp_move(1))
        b_rm = TransparentToolButton(FIF.DELETE, v)
        b_rm.setToolTip(_t("Remove from the group"))
        b_rm.clicked.connect(self._grp_remove)
        col = QWidget(v)
        cl = QVBoxLayout(col)
        cl.setContentsMargins(0, 0, 0, 0)
        for b in (b_up, b_down, b_rm):
            cl.addWidget(b)
        self.cb_grp_add = ComboBox(v)
        self.cb_grp_add.setFixedWidth(330)
        self.cb_grp_add.currentIndexChanged.connect(self._grp_add)
        self.r_grp_members = s_out.add(Row(_t("Members"), _t("Outbound blocks this group uses, first one on top. You can also connect them on the canvas from the group's 'members' output."), v)
                                       .right(self.lst_grp, col))
        self.r_grp_add = s_out.add(Row(_t("Add a member"), "", v).right(self.cb_grp_add))
        self.grp_members, self.grp_choices = [], []
        self.ed_grp_url = LineEdit(v)
        self.ed_grp_url.setFixedWidth(330)
        self.sp_grp_interval = SpinBox(v)
        self.sp_grp_interval.setRange(10, 3600)
        self.sp_grp_interval.setFixedWidth(150)
        self.r_grp_test = s_out.add(Row(_t("Test address · interval (s)"), _t("Each member is measured by loading this address through it"), v)
                                    .right(self.ed_grp_url, self.sp_grp_interval))
        self.sp_grp_tol = SpinBox(v)
        self.sp_grp_tol.setRange(0, 5000)
        self.sp_grp_tol.setFixedWidth(150)
        self.r_grp_tol = s_out.add(Row(_t("Tolerance (ms)"), _t("Switch only when another member is at least this much faster (avoids switching back and forth)"), v)
                                   .right(self.sp_grp_tol))
        self._sub = {"sub_servers": "", "sub_updated": 0}
        # VLESS / Trojan / TUIC
        self.ed_out_uuid = LineEdit(v)
        self.ed_out_uuid.setPlaceholderText("0b5b8d9e-xxxx-xxxx-xxxx-xxxxxxxxxxxx")
        self.ed_out_uuid.setFixedWidth(330)
        self.r_out_uuid = s_out.add(Row(_t("User UUID"), "", v).right(self.ed_out_uuid))
        self.securities = ["none", "tls", "reality"]
        self.cb_out_sec = ComboBox(v)
        self.cb_out_sec.addItems([_t("None"), "TLS", "REALITY"])
        self.cb_out_sec.setFixedWidth(150)
        self.flows = ["", "xtls-rprx-vision"]
        self.cb_out_flow = ComboBox(v)
        self.cb_out_flow.addItems([_t("(no flow)"), "xtls-rprx-vision"])
        self.cb_out_flow.setFixedWidth(180)
        self.r_out_sec = s_out.add(Row(_t("Security · flow"), _t("Use what the server is set up with (REALITY usually with xtls-rprx-vision)"), v)
                                   .right(self.cb_out_sec, self.cb_out_flow))
        self.ed_out_pbk = LineEdit(v)
        self.ed_out_pbk.setPlaceholderText(_t("Public key (pbk)"))
        self.ed_out_pbk.setFixedWidth(330)
        self.ed_out_sid = LineEdit(v)
        self.ed_out_sid.setPlaceholderText(_t("Short ID (sid)"))
        self.ed_out_sid.setFixedWidth(140)
        self.r_out_reality = s_out.add(Row(_t("REALITY"), _t("The server's public key and short ID. SNI = the site the server imitates"), v)
                                       .right(self.ed_out_pbk, self.ed_out_sid))
        self.fingerprints = ["", "chrome", "firefox", "safari", "edge", "ios", "android", "random", "randomized"]
        self.cb_out_fp = ComboBox(v)
        self.cb_out_fp.addItems([_t("(none)")] + self.fingerprints[1:])
        self.cb_out_fp.setFixedWidth(150)
        self.ed_out_alpn = LineEdit(v)
        self.ed_out_alpn.setPlaceholderText(_t("ALPN (e.g. h2, http/1.1)"))
        self.ed_out_alpn.setFixedWidth(180)
        self.r_out_fp = s_out.add(Row(_t("Fingerprint · ALPN"), _t("Look like this browser's TLS (uTLS). Empty ALPN = default"), v)
                                  .right(self.cb_out_fp, self.ed_out_alpn))
        self.transports = ["tcp", "ws", "grpc"]
        self.cb_out_tr = ComboBox(v)
        self.cb_out_tr.addItems(["TCP", "WebSocket", "gRPC"])
        self.cb_out_tr.setFixedWidth(130)
        self.ed_out_tr_path = LineEdit(v)
        self.ed_out_tr_path.setPlaceholderText(_t("Path / service name"))
        self.ed_out_tr_path.setFixedWidth(160)
        self.ed_out_tr_host = LineEdit(v)
        self.ed_out_tr_host.setPlaceholderText(_t("Host header"))
        self.ed_out_tr_host.setFixedWidth(160)
        self.r_out_tr = s_out.add(Row(_t("Transport"), _t("WebSocket and gRPC are used behind CDNs. Path for WebSocket, service name for gRPC"), v)
                                  .right(self.cb_out_tr, self.ed_out_tr_path, self.ed_out_tr_host))
        self.congestions = ["bbr", "cubic", "new_reno"]
        self.cb_out_cc = ComboBox(v)
        self.cb_out_cc.addItems(self.congestions)
        self.cb_out_cc.setFixedWidth(130)
        self.relays = ["native", "quic"]
        self.cb_out_relay = ComboBox(v)
        self.cb_out_relay.addItems(self.relays)
        self.cb_out_relay.setFixedWidth(130)
        self.r_out_tuic = s_out.add(Row(_t("Congestion control · UDP relay"), _t("Use what the TUIC server expects"), v)
                                    .right(self.cb_out_cc, self.cb_out_relay))
        self.sw_out_verify = _switch(v, _t("Verify"), _t("Off"))
        self.r_out_verify = s_out.add(Row(_t("Verify server certificate"), _t("Turn off for servers with self-signed certificates (pinning the fingerprint is recommended)"), v)
                                      .right(self.sw_out_verify))
        self.ed_out_sni = LineEdit(v)
        self.ed_out_sni.setPlaceholderText(_t("Empty = server address"))
        self.ed_out_sni.setFixedWidth(220)
        self.btn_detect = PushButton(_t("Detect from server"), v, FIF.SEARCH)
        self.btn_detect.setToolTip(_t("Read the server's certificate and use its name"))
        self.btn_detect.clicked.connect(self._detect_sni)
        self.detect_done.connect(self._on_detect_done)
        self.r_out_sni = s_out.add(Row(_t("SNI (server name)"), _t("Servers that check the name (e.g. the official Hysteria2 server) refuse a wrong one. Connecting by IP? Click 'Detect from server'."), v)
                                   .right(self.ed_out_sni, self.btn_detect))
        self.ed_out_pin = LineEdit(v)
        self.ed_out_pin.setPlaceholderText(_t("SHA-256 (optional)"))
        self.ed_out_pin.setFixedWidth(300)
        self.r_out_pin = s_out.add(Row(_t("Pin certificate fingerprint"), _t("If set, only a certificate with this fingerprint is accepted"), v).right(self.ed_out_pin))
        self.cb_out_ss = ComboBox(v)
        self.cb_out_ss.addItems(self._ss_labels())
        self.cb_out_ss.setFixedWidth(330)
        self.r_out_ss_method = s_out.add(Row(_t("Cipher"), _t("Must match the server"), v).right(self.cb_out_ss))
        self.ed_out_ss_pw = PasswordLineEdit(v)
        self.ed_out_ss_pw.setFixedWidth(260)
        self.r_out_ss_pw = s_out.add(Row(_t("Shadowsocks password"), "", v).right(self.ed_out_ss_pw))
        self.ed_out_hy = PasswordLineEdit(v)
        self.ed_out_hy.setFixedWidth(260)
        self.r_out_hy = s_out.add(Row(_t("Hysteria2 password"), "", v).right(self.ed_out_hy))
        self.ed_out_obfs = PasswordLineEdit(v)
        self.ed_out_obfs.setPlaceholderText(_t("(off)"))
        self.ed_out_obfs.setFixedWidth(260)
        self.r_out_obfs = s_out.add(Row(_t("Salamander obfuscation"), _t("Only if the server uses it (obfs-password in the link)"), v)
                                    .right(self.ed_out_obfs))
        self.sp_out_up = SpinBox(v)
        self.sp_out_up.setRange(0, 100000)
        self.sp_out_up.setFixedWidth(150)
        self.sp_out_down = SpinBox(v)
        self.sp_out_down.setRange(0, 100000)
        self.sp_out_down.setFixedWidth(150)
        self.r_out_bw = s_out.add(Row(_t("Bandwidth up · down (Mbps)"), _t("Brutal: enter your real line speed to push through lossy networks. 0 = automatic (BBR)"), v)
                                  .right(self.sp_out_up, self.sp_out_down))
        self.ed_out_hop = LineEdit(v)
        self.ed_out_hop.setPlaceholderText("20000-30000")
        self.ed_out_hop.setFixedWidth(200)
        self.sp_out_hop = SpinBox(v)
        self.sp_out_hop.setRange(5, 3600)
        self.sp_out_hop.setFixedWidth(150)
        self.r_out_hop = s_out.add(Row(_t("Port hopping · interval (s)"), _t("Ports the server listens on, e.g. 20000-30000 or 443,5000-6000. Empty = off"), v)
                                   .right(self.ed_out_hop, self.sp_out_hop))
        # WireGuard
        b_wg_import = PushButton(_t("Import .conf…"), v, FIF.DOCUMENT)
        b_wg_import.clicked.connect(self._wg_import)
        self.r_wg_import = s_out.add(Row(_t("WireGuard config file"), _t("Fill everything in from a .conf file from your VPN provider or WireGuard server"), v)
                                     .right(b_wg_import))
        self.ed_wg_key = PasswordLineEdit(v)
        self.ed_wg_key.setFixedWidth(330)
        b_wg_gen = PushButton(_t("Generate"), v, FIF.SYNC)
        b_wg_gen.clicked.connect(self._wg_generate)
        self.r_wg_key = s_out.add(Row(_t("Private key"), "", v).right(self.ed_wg_key, b_wg_gen))
        self.ed_wg_key.textChanged.connect(lambda *_: self._wg_show_public())
        self.ed_wg_addr = LineEdit(v)
        self.ed_wg_addr.setPlaceholderText("10.0.0.2/32, fd00::2/128")
        self.ed_wg_addr.setFixedWidth(330)
        self.r_wg_addr = s_out.add(Row(_t("Tunnel address"), _t("This side's address inside the tunnel (Address in the .conf)"), v).right(self.ed_wg_addr))
        self.ed_wg_peer = LineEdit(v)
        self.ed_wg_peer.setFixedWidth(330)
        self.r_wg_peer = s_out.add(Row(_t("Server public key"), _t("PublicKey of the [Peer]"), v).right(self.ed_wg_peer))
        self.ed_wg_psk = PasswordLineEdit(v)
        self.ed_wg_psk.setPlaceholderText(_t("(optional)"))
        self.ed_wg_psk.setFixedWidth(330)
        self.r_wg_psk = s_out.add(Row(_t("Preshared key"), "", v).right(self.ed_wg_psk))
        self.ed_wg_allowed = LineEdit(v)
        self.ed_wg_allowed.setFixedWidth(330)
        self.r_wg_allowed = s_out.add(Row(_t("Allowed IPs"), _t("Destinations sent into the tunnel. 0.0.0.0/0, ::/0 = everything"), v).right(self.ed_wg_allowed))
        self.sp_wg_mtu = SpinBox(v)
        self.sp_wg_mtu.setRange(576, 9200)
        self.sp_wg_mtu.setFixedWidth(150)
        self.sp_wg_keep = SpinBox(v)
        self.sp_wg_keep.setRange(0, 3600)
        self.sp_wg_keep.setFixedWidth(150)
        self.r_wg_mtu = s_out.add(Row(_t("MTU · keepalive (s)"), _t("Keepalive 0 = off. 25 helps behind NAT"), v).right(self.sp_wg_mtu, self.sp_wg_keep))
        self.out_note = CaptionLabel("", v)
        self.out_note.setWordWrap(True)
        s_out.add(self.out_note)
        # connect to (inbound blocks)
        s_link = Section(_t("Connect to"), v)
        self.cb_target = ComboBox(v)
        self.cb_target.setFixedWidth(320)
        s_link.add(Row(_t("Outbound block"), _t("Where this inbound sends its traffic. Choosing another inbound relays into it. Also editable on the canvas."), v)
                   .right(self.cb_target))
        self.targets = []

        # connection check (both kinds)
        s_check = Section(_t("Check connection"), v)
        self.btn_probe = PushButton(_t("Check connection"), v, FIF.SPEED_HIGH)
        self.btn_probe.clicked.connect(self._probe)
        self.r_probe = s_check.add(Row(_t("Check connection"), CHECK_DESC, v).right(self.btn_probe))
        self.check_done.connect(self._on_probe_done)
        self.probing = False
        self._probe_token = 0

        # local settings (take precedence over global)
        s_local = Section(_t("Local settings (uncheck to use a value just for this proxy)"), v)
        self.ov = {}
        self.ed_allow = LineEdit(v)
        self.ed_allow.setFixedWidth(280)
        self.sw_private = _switch(v)
        self.sp_timeout = SpinBox(v)
        self.sp_timeout.setRange(10, 86400)
        self.sp_timeout.setFixedWidth(150)
        self.sw_logconn = _switch(v)
        self.sw_logclose = _switch(v)
        self.cb_dns = dns_combo(v)
        self.sw_dns_via = _switch(v)
        for key, title, desc, w in (
            ("allowed_domains", _t("Allowed domains"), _t("Comma separated · * = all · *.example.com = including subdomains"), self.ed_allow),
            ("block_private", _t("Block private network destinations"), _t("Refuse connections to loopback and private IPs"), self.sw_private),
            ("timeout", _t("Idle timeout (s)"), "", self.sp_timeout),
            ("log_connections", _t("Log connections"), "", self.sw_logconn),
            ("log_close", _t("Log connection close details"), "", self.sw_logclose),
            ("dns_server", _t("DNS server"), _t("Empty = the PC's DNS. Otherwise 1.1.1.1 (UDP), tcp://, tls:// (DoT), https://…/dns-query (DoH), quic:// or h3://. Used where ProxyDock looks names up itself (Direct, IP rules, TUN). Needs the sing-box engine"), self.cb_dns),
            ("dns_via_outbound", _t("DNS through the outbound"), _t("The card's own lookups go through its tunnel or upstream instead of leaving this PC directly (needs a DNS server above)"), self.sw_dns_via),
        ):
            follow = CheckBox(_t("Use global"), v)
            follow.stateChanged.connect(lambda *_, k=key: self._sync_override(k))
            self.ov[key] = (follow, w)
            s_local.add(Row(title, desc, v).right(follow, w))

        # router blocks
        from .router import RouterSection
        self.s_router = RouterSection(ctl, v)

        header = QHBoxLayout()
        header.addWidget(self.title)
        header.addStretch(1)
        header.addWidget(self.btn_cancel)
        header.addWidget(self.btn_save)
        root = QVBoxLayout(v)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(18)
        root.addLayout(header)
        self.sections = {"basic": s_basic, "in": s_in, "link": s_link, "out": s_out, "check": s_check, "local": s_local,
                         "router": self.s_router}
        for s in (s_basic, s_in, s_link, s_out, s_check, s_local, self.s_router):
            root.addWidget(s)
        root.addStretch(1)

        self.cb_in.currentIndexChanged.connect(lambda *_: self._sync())
        self.cb_out.currentIndexChanged.connect(lambda *_: self._sync())
        self.cb_via.currentIndexChanged.connect(lambda *_: self._sync())
        self.cb_out_sec.currentIndexChanged.connect(lambda *_: self._sync())
        self.cb_out_tr.currentIndexChanged.connect(lambda *_: self._sync())
        self.cb_sub_mode.currentIndexChanged.connect(lambda *_: self._sync())
        self.cb_grp_mode.currentIndexChanged.connect(lambda *_: self._sync())
        self.sw_in_auth.checkedChanged.connect(lambda *_: self._sync())

    def _ss_labels(self):
        from ..ss import SS_METHODS
        return [m + (_t("  (recommended)") if SS_METHODS[m][2] else _t("  (legacy compatible)")) for m in self.ss_methods]

    def _gen_ss(self, combo, edit):
        from ..ss import ss_generate_password
        edit.setText(ss_generate_password(self.ss_methods[max(combo.currentIndex(), 0)]))

    def _pick(self, edit, filt):
        path, _ = QFileDialog.getOpenFileName(self, _t("Choose file"), os.path.dirname(edit.text()) or "", filt)
        if path:
            edit.setText(path)

    # --- display ---
    def load_node(self, kind, node):
        """Edit an inbound, outbound or router block of the graph."""
        from .. import config as cfg
        self.kind = kind
        self.node = copy.deepcopy(node)
        n = self.node
        if kind == "router":
            for key, sec in self.sections.items():
                sec.setVisible(key in ("basic", "router"))
            self.r_auto.setVisible(False)
            self.title.setText(_t("Edit router"))
            self.ed_name.setText(n["name"])
            self.s_router.load(n)
            self.verticalScrollBar().setValue(0)
            return
        inbound = kind == "inbound"
        for key, sec in self.sections.items():
            sec.setVisible(key in (("basic", "in", "link", "check", "local") if inbound else ("basic", "out", "check")))
        self.r_auto.setVisible(inbound)
        self.title.setText(_t("Edit inbound") if inbound else _t("Edit outbound"))
        self.ed_name.setText(n["name"])
        ib = n["inbound"] if inbound else dict(cfg.INBOUND_DEFAULTS)
        ob = n["outbound"] if not inbound else dict(cfg.OUTBOUND_DEFAULTS)
        ov = n.get("overrides") or {k: None for k in cfg.OVERRIDABLE}
        self.sw_auto.setChecked(n.get("autostart", False))
        self.cb_in.setCurrentIndex(self.in_types.index(ib["type"]))
        self.ed_in_host.setText(ib["host"])
        self.sp_in_port.setValue(ib["port"])
        self.sw_in_auth.setChecked(ib["auth_enabled"])
        self.ed_in_user.setText(ib["username"])
        self.ed_in_pass.setText(ib["password"])
        self.ed_cert.setText(ib["tls_cert"])
        self.ed_key.setText(ib["tls_key"])
        self.cb_in_ss.setCurrentIndex(self.ss_methods.index(ib["ss_method"]))
        self.ed_in_ss_pw.setText(ib["ss_password"])
        self.ed_in_hy.setText(ib["hy2_password"])
        self.ed_in_obfs.setText(ib["hy2_obfs_password"])
        self.sp_in_up.setValue(ib["hy2_up_mbps"])
        self.sp_in_down.setValue(ib["hy2_down_mbps"])
        self.cb_tun_stack.setCurrentIndex(self.tun_stacks.index(ib["tun_stack"]))
        self.sw_tun_strict.setChecked(ib["tun_strict_route"])
        self.sw_tun_fakeip.setChecked(ib["tun_fakeip"])
        self._fill_outbound(ob)
        glob = self.ctl.glob
        for key, (follow, w) in self.ov.items():
            follow.blockSignals(True)
            follow.setChecked(ov.get(key) is None)
            follow.blockSignals(False)
            self._set_value(key, ov[key] if ov.get(key) is not None else glob[key])
            w.setEnabled(ov.get(key) is not None)
        self._after_fill(n, inbound, ib)

    def _fill_outbound(self, ob):
        """Put an outbound's settings into the outbound widgets (also used by 'Paste share link')."""
        self.cb_out.setCurrentIndex(self.out_types.index(ob["type"]))
        self.ed_out_host.setText(ob["host"])
        self.sp_out_port.setValue(ob["port"])
        self.ed_out_user.setText(ob["username"])
        self.ed_out_pass.setText(ob["password"])
        self.sw_out_verify.setChecked(ob["tls_verify"])
        self.ed_out_sni.setText(ob["sni"])
        self.ed_out_pin.setText(ob["pin_sha256"])
        self.cb_out_ss.setCurrentIndex(self.ss_methods.index(ob["ss_method"]))
        self.ed_out_ss_pw.setText(ob["ss_password"])
        self.ed_out_hy.setText(ob["hy2_password"])
        self.ed_out_obfs.setText(ob["hy2_obfs_password"])
        self.sp_out_up.setValue(ob["hy2_up_mbps"])
        self.sp_out_down.setValue(ob["hy2_down_mbps"])
        self.ed_out_hop.setText(ob["hy2_ports"])
        self.sp_out_hop.setValue(ob["hy2_hop_interval"])
        self.ed_wg_key.setText(ob["wg_private_key"])
        self.ed_wg_addr.setText(ob["wg_address"])
        self.ed_wg_peer.setText(ob["wg_peer_public_key"])
        self.ed_wg_psk.setText(ob["wg_preshared_key"])
        self.ed_wg_allowed.setText(ob["wg_allowed_ips"])
        self.sp_wg_mtu.setValue(ob["wg_mtu"])
        self.sp_wg_keep.setValue(ob["wg_keepalive"])
        self.ed_out_bind.setText(ob.get("bind", ""))
        self._fill_ifaces(ob.get("interface", ""))
        self.ed_out_uuid.setText(ob["uuid"])
        self.cb_out_sec.setCurrentIndex(self.securities.index(ob["security"]) if ob["security"] in self.securities else 1)
        self.cb_out_flow.setCurrentIndex(self.flows.index(ob["vless_flow"]) if ob["vless_flow"] in self.flows else 0)
        self.ed_out_pbk.setText(ob["reality_public_key"])
        self.ed_out_sid.setText(ob["reality_short_id"])
        self.cb_out_fp.setCurrentIndex(self.fingerprints.index(ob["fingerprint"]) if ob["fingerprint"] in self.fingerprints else 1)
        self.ed_out_alpn.setText(ob["alpn"])
        self.cb_out_tr.setCurrentIndex(self.transports.index(ob["transport"]) if ob["transport"] in self.transports else 0)
        self.ed_out_tr_path.setText(ob["transport_path"])
        self.ed_out_tr_host.setText(ob["transport_host"])
        self.cb_out_cc.setCurrentIndex(self.congestions.index(ob["tuic_congestion"]) if ob["tuic_congestion"] in self.congestions else 0)
        self.cb_out_relay.setCurrentIndex(self.relays.index(ob["tuic_udp_relay"]) if ob["tuic_udp_relay"] in self.relays else 0)
        self.ed_sub_url.setText(ob["sub_url"])
        self.cb_sub_mode.setCurrentIndex(self.sub_modes.index(ob["sub_mode"]) if ob["sub_mode"] in self.sub_modes else 0)
        self.sp_sub_hours.setValue(ob["sub_interval"])
        self._sub = {"sub_servers": ob["sub_servers"], "sub_updated": ob["sub_updated"]}
        self._fill_sub_servers(ob["sub_selected"])
        self.cb_grp_mode.setCurrentIndex(self.grp_modes.index(ob["group_mode"]) if ob["group_mode"] in self.grp_modes else 0)
        self.ed_grp_url.setText(ob["group_test_url"])
        self.sp_grp_interval.setValue(ob["group_interval"])
        self.sp_grp_tol.setValue(ob["group_tolerance"])

    def _detect_sni(self):
        n = self.collect()
        ob = n["outbound"]
        if not ob["host"] or not ob["port"]:
            info(self.window(), "error", _t("Detect from server"), _t("Enter the server address and port first"), 4000)
            return
        self.btn_detect.setEnabled(False)
        self.btn_detect.setText(_t("Connecting…"))

        def work():
            from ..certinfo import detect
            try:
                self.detect_done.emit(detect(ob), "")
            except ValueError as e:
                self.detect_done.emit(None, str(e))
        threading.Thread(target=work, daemon=True).start()

    def _on_detect_done(self, res, err):
        self.btn_detect.setEnabled(True)
        self.btn_detect.setText(_t("Detect from server"))
        if res is None:
            info(self.window(), "error", _t("Detect from server"), err, 6000)
            return
        fp = ":".join(res["sha256"][i:i + 2] for i in range(0, 16, 2)).upper() + "…"
        if res["sni"]:
            self.ed_out_sni.setText(res["sni"])
            info(self.window(), "success", _t("SNI set to {name}").format(name=res["sni"]),
                 _t("Names in the certificate: {names}").format(names=", ".join(res["names"])) + "\n"
                 + _t("Fingerprint (SHA-256): {fp}").format(fp=fp), 7000)
        else:
            info(self.window(), "warning", _t("No usable name in the certificate"),
                 (_t("Names in the certificate: {names}").format(names=", ".join(res["names"])) if res["names"]
                  else _t("The certificate has no DNS name")) + "\n" + _t("Fingerprint (SHA-256): {fp}").format(fp=fp), 7000)

    def _fill_sub_servers(self, selected=""):
        from ..subscription import servers_of
        names = [s["name"] for s in servers_of(self._sub)]
        self.sub_names = names
        self.cb_sub_server.clear()
        self.cb_sub_server.addItems(names or [_t("(no servers yet)")])
        self.cb_sub_server.setCurrentIndex(names.index(selected) if selected in names else 0)
        when = time.strftime("%Y-%m-%d %H:%M", time.localtime(self._sub["sub_updated"])) if self._sub["sub_updated"] else "-"
        d = self.r_sub_url.desc
        d.setText(_t("{count} servers · updated {when}").format(count=len(names), when=when))
        d.setVisible(True)

    def _sub_update(self):
        url = self.ed_sub_url.text().strip()
        if not url:
            info(self.window(), "error", _t("Subscription"), _t("Enter the subscription address (https://...)"), 4000)
            return
        self.btn_sub_update.setEnabled(False)
        self.btn_sub_update.setText(_t("Updating…"))

        def work():
            from ..subscription import update
            try:
                fields, note = update({"sub_url": url})
                self.sub_done.emit(fields, note)
            except ValueError as e:
                self.sub_done.emit(None, str(e))
        threading.Thread(target=work, daemon=True).start()

    def _on_sub_done(self, fields, note):
        self.btn_sub_update.setEnabled(True)
        self.btn_sub_update.setText(_t("Update now"))
        if fields is None:
            info(self.window(), "error", _t("Subscription"), note, 6000)
            return
        current = self.sub_names[self.cb_sub_server.currentIndex()] if getattr(self, "sub_names", None) else ""
        self._sub = fields
        self._fill_sub_servers(current)
        info(self.window(), "success", _t("Subscription"), note, 4000)

    def _paste_link(self):
        from ..links import parse
        text = QApplication.clipboard().text().strip()
        try:
            fields, name = parse(text)
        except ValueError as e:
            info(self.window(), "error", _t("Cannot use the clipboard"),
                 f"{e}\n" + _t("Copy a vless://, trojan://, tuic://, ss:// or hysteria2:// link first"), 6000)
            return
        ob = dict(config.OUTBOUND_DEFAULTS, **fields)
        self._fill_outbound(ob)
        if not self.ed_name.text().strip() or self.ed_name.text().startswith(tuple(OUTBOUND_TYPES.values())):
            self.ed_name.setText(name[:60])
        self._sync()
        info(self.window(), "success", _t("Filled in from the link"), name, 3000)

    def _after_fill(self, n, inbound, ib):
        if not inbound:
            g = self.ctl.graph
            others = [x for x in g["outbounds"] if x["id"] != n["id"] and x["outbound"]["type"] not in ("direct", "group")]
            self.vias = [None] + [x["id"] for x in others]
            self.cb_via.clear()
            self.cb_via.addItems([_t("(directly)")] + [x["name"] for x in others])
            self.cb_via.setCurrentIndex(self.vias.index(n.get("via")) if n.get("via") in self.vias else 0)
            self.grp_members = list(n.get("members") or [])
            self._grp_fill()
        if inbound:
            g = self.ctl.graph
            routers = g.get("routers", [])
            self.targets = [None] + [x["id"] for x in g["outbounds"]] + [x["id"] for x in routers] + \
                [x["id"] for x in g["inbounds"] if x["id"] != n["id"]]
            labels = [_t("(not connected)")] + [x["name"] for x in g["outbounds"]] + \
                [_t("Router {name}").format(name=x["name"]) for x in routers] + \
                [_t("relay → {name}").format(name=x["name"]) for x in g["inbounds"] if x["id"] != n["id"]]
            self.cb_target.clear()
            self.cb_target.addItems(labels)
            self.cb_target.setCurrentIndex(self.targets.index(n["target"]) if n["target"] in self.targets else 0)
            if ib["type"] == "shadowsocks" and not ib["ss_password"]:
                self._gen_ss(self.cb_in_ss, self.ed_in_ss_pw)
            if ib["type"] == "hysteria2" and not ib["hy2_password"]:
                self.ed_in_hy.setText(_token())
        self._probe_token += 1      # opening another block discards a check that is still running
        self._set_probe_text(CHECK_DESC if inbound else CHECK_DESC_OUT, None)
        self._sync()
        self.verticalScrollBar().setValue(0)

    # --- group members ---
    def _grp_fill(self):
        from .. import graph as graph_mod
        g = self.ctl.graph
        own = self.node["id"] if self.node else None
        self.grp_members = [m for m in self.grp_members if graph_mod.find(g, m)[0] == "outbound"]
        self.lst_grp.clear()
        for i, m in enumerate(self.grp_members):
            self.lst_grp.addItem(f"{i + 1}. {graph_mod.find(g, m)[1]['name']}")
        self.grp_choices = [None] + [x["id"] for x in g["outbounds"] if x["id"] != own and x["id"] not in self.grp_members
                                     and x["outbound"]["type"] not in graph_mod.NO_MEMBER]
        self.cb_grp_add.blockSignals(True)
        self.cb_grp_add.clear()
        self.cb_grp_add.addItems([_t("(choose an outbound block)")] + [graph_mod.find(g, x)[1]["name"] for x in self.grp_choices[1:]])
        self.cb_grp_add.setCurrentIndex(0)
        self.cb_grp_add.blockSignals(False)

    def _grp_add(self, idx):
        if 0 < idx < len(self.grp_choices):
            self.grp_members.append(self.grp_choices[idx])
            self._grp_fill()

    def _grp_move(self, step):
        i = self.lst_grp.currentRow()
        j = i + step
        if 0 <= i < len(self.grp_members) and 0 <= j < len(self.grp_members):
            self.grp_members[i], self.grp_members[j] = self.grp_members[j], self.grp_members[i]
            self._grp_fill()
            self.lst_grp.setCurrentRow(j)

    def _grp_remove(self):
        i = self.lst_grp.currentRow()
        if 0 <= i < len(self.grp_members):
            del self.grp_members[i]
            self._grp_fill()

    def _wg_generate(self):
        from ..wgconf import keypair
        self.ed_wg_key.setText(keypair()[0])

    def _wg_show_public(self):
        from ..wgconf import public_of
        pub = public_of(self.ed_wg_key.text())
        d = self.r_wg_key.desc
        d.setText(_t("Public key (give it to the server): {key}").format(key=pub) if pub else
                  _t("Paste the PrivateKey from the .conf, or click Generate"))
        d.setVisible(True)
        d.setTextInteractionFlags(Qt.TextSelectableByMouse)

    def _wg_import(self):
        from ..wgconf import parse_conf
        path, _ = QFileDialog.getOpenFileName(self, _t("Choose file"), "", _t("WireGuard config (*.conf);;All files (*)"))
        if not path:
            return
        try:
            with open(path, encoding="utf-8-sig") as f:
                ob, notes = parse_conf(f.read())
        except (OSError, ValueError, UnicodeDecodeError) as e:
            info(self.window(), "error", _t("Cannot import"), str(e), 6000)
            return
        self.cb_out.setCurrentIndex(self.out_types.index("wireguard"))
        self.ed_out_host.setText(ob["host"])
        self.sp_out_port.setValue(ob["port"])
        self.ed_wg_key.setText(ob["wg_private_key"])
        self.ed_wg_addr.setText(ob["wg_address"])
        self.ed_wg_peer.setText(ob["wg_peer_public_key"])
        self.ed_wg_psk.setText(ob["wg_preshared_key"])
        self.ed_wg_allowed.setText(ob["wg_allowed_ips"])
        self.sp_wg_mtu.setValue(ob["wg_mtu"])
        self.sp_wg_keep.setValue(ob["wg_keepalive"])
        if not self.ed_name.text().strip() or self.ed_name.text().startswith(OUTBOUND_TYPES["wireguard"]):
            self.ed_name.setText(os.path.splitext(os.path.basename(path))[0][:60])
        info(self.window(), "success", _t("Imported"), "\n".join(notes) or os.path.basename(path), 5000)

    def _fill_ifaces(self, current):
        names = network_interfaces()
        self.ifaces = [""] + [n for n, _ in names]
        labels = [_t("Automatic")] + [label for _, label in names]
        if current and current not in self.ifaces:          # saved name that is not up right now
            self.ifaces.append(current)
            labels.append(_t("{name} (not connected)").format(name=current))
        self.cb_out_iface.clear()
        self.cb_out_iface.addItems(labels)
        self.cb_out_iface.setCurrentIndex(self.ifaces.index(current) if current in self.ifaces else 0)

    def _iface_value(self):
        i = self.cb_out_iface.currentIndex()
        return self.ifaces[i] if 0 <= i < len(self.ifaces) else ""

    def _set_value(self, key, val):
        w = self.ov[key][1]
        if key == "allowed_domains":
            w.setText(", ".join(val))
        elif key == "dns_server":
            w.setText(val)
        elif key == "timeout":
            w.setValue(val)
        else:
            w.setChecked(val)

    def _get_value(self, key):
        w = self.ov[key][1]
        if key == "allowed_domains":
            return [d.strip() for d in w.text().replace(";", ",").split(",") if d.strip()] or ["*"]
        if key == "dns_server":
            return w.text().strip()
        if key == "timeout":
            return w.value()
        return w.isChecked()

    def _sync_override(self, key):
        follow, w = self.ov[key]
        if follow.isChecked():
            self._set_value(key, self.ctl.glob[key])
        w.setEnabled(not follow.isChecked())

    def _sync(self):
        it = self.in_types[max(self.cb_in.currentIndex(), 0)]
        ot = self.out_types[max(self.cb_out.currentIndex(), 0)]
        can_auth = it in ("socks5", "http", "https")
        self.r_in_listen.setVisible(it != "tun")
        self.r_tun_stack.setVisible(it == "tun")
        self.r_tun_strict.setVisible(it == "tun")
        self.r_tun_fakeip.setVisible(it == "tun")
        self.r_in_auth.setVisible(can_auth)
        self.r_in_cred.setVisible(can_auth and self.sw_in_auth.isChecked())
        self.r_cert.setVisible(it in ("https", "hysteria2"))
        self.r_key.setVisible(it in ("https", "hysteria2"))
        self.r_in_ss_method.setVisible(it == "shadowsocks")
        self.r_in_ss_pw.setVisible(it == "shadowsocks")
        self.r_in_hy.setVisible(it == "hysteria2")
        self.r_in_obfs.setVisible(it == "hysteria2")
        self.r_in_bw.setVisible(it == "hysteria2")
        notes = {
            "socks5": _t("The most widely used proxy. Relays both TCP and UDP (QUIC, DNS, etc.)."),
            "socks4": _t("SOCKS4/4a. TCP only, no password authentication (the user ID is ignored). Use it on this PC only."),
            "http": _t("For the 'HTTP proxy' setting of browsers and the OS. HTTPS sites are tunnelled with CONNECT. TCP only."),
            "https": _t("Wraps the HTTP proxy in TLS. Set it as an 'HTTPS proxy' in the client (e.g. curl -x https://…)."),
            "shadowsocks": _t("Encrypted tunnel server for Shadowsocks apps on other devices. TCP+UDP. Copy the ss:// link with the button on the card."),
            "hysteria2": _t("Encrypted tunnel server over QUIC (UDP). TCP+UDP. Copy the hysteria2:// link with the button on the card."),
            "tun": _t("A virtual network card that captures the traffic of every app on this PC, including apps without proxy settings, so router rules on programs work for all of them. Starting it asks for administrator permission (UAC) unless ProxyDock runs as administrator. One TUN card at a time. Needs the sing-box engine."),
        }
        self.in_note.setText(notes[it])
        remote = ot not in ("direct", "block", "subscription", "group")
        for r in (self.r_grp_mode, self.r_grp_members, self.r_grp_add, self.r_grp_test):
            r.setVisible(ot == "group")
        self.r_grp_tol.setVisible(ot == "group" and self.grp_modes[max(self.cb_grp_mode.currentIndex(), 0)] == "auto")
        self.r_grp_mode.desc.setText(_t("Uses the member that answers fastest, and moves away from members that stop working")
                                     if self.grp_modes[max(self.cb_grp_mode.currentIndex(), 0)] == "auto" else
                                     _t("Uses the first member that works; when it stops working, the next one. It stays there until that one fails too"))
        for r in (self.r_sub_url, self.r_sub_mode, self.r_sub_hours):
            r.setVisible(ot == "subscription")
        self.cb_sub_server.setVisible(self.sub_modes[max(self.cb_sub_mode.currentIndex(), 0)] == "select")
        self.r_out_server.setVisible(remote)
        self.r_out_bind.setVisible(ot == "direct")
        self.r_out_via.setVisible(ot not in ("direct", "hysteria2", "block", "subscription", "group"))
        vi = self.cb_via.currentIndex()
        has_via = ot not in ("direct", "hysteria2", "block") and 0 < vi < len(self.vias)
        self.r_out_iface.setVisible(ot not in ("block", "group") and not has_via)    # only the hop that dials out itself
        self.r_out_cred.setVisible(ot in ("http", "https", "socks4", "socks5", "trojan", "tuic"))
        self.ed_out_user.setVisible(ot not in ("trojan", "tuic"))
        self.ed_out_pass.setVisible(ot != "socks4")
        sec = self.securities[max(self.cb_out_sec.currentIndex(), 0)]
        tls = ot in ("https", "hysteria2", "trojan", "tuic") or (ot == "vless" and sec != "none")
        self.r_out_verify.setVisible(tls and not (ot == "vless" and sec == "reality"))
        self.r_out_sni.setVisible(tls)
        self.r_out_uuid.setVisible(ot in ("vless", "tuic"))
        self.r_out_sec.setVisible(ot == "vless")
        self.r_out_reality.setVisible(ot == "vless" and sec == "reality")
        self.r_out_fp.setVisible(ot in ("vless", "trojan") and sec != "none" or ot == "tuic")
        self.cb_out_fp.setVisible(ot != "tuic")
        self.r_out_tr.setVisible(ot in ("vless", "trojan"))
        tr = self.transports[max(self.cb_out_tr.currentIndex(), 0)]
        self.ed_out_tr_path.setVisible(tr != "tcp")
        self.ed_out_tr_host.setVisible(tr == "ws")
        self.r_out_tuic.setVisible(ot == "tuic")
        self.r_out_pin.setVisible(ot == "hysteria2")
        self.r_out_ss_method.setVisible(ot == "shadowsocks")
        self.r_out_ss_pw.setVisible(ot == "shadowsocks")
        self.r_out_hy.setVisible(ot == "hysteria2")
        for r in (self.r_out_obfs, self.r_out_bw, self.r_out_hop):
            r.setVisible(ot == "hysteria2")
        for r in (self.r_wg_import, self.r_wg_key, self.r_wg_addr, self.r_wg_peer, self.r_wg_psk, self.r_wg_allowed, self.r_wg_mtu):
            r.setVisible(ot == "wireguard")
        out_notes = {
            "direct": _t("Connects to the destination directly from this PC."),
            "http": _t("Connects through an upstream HTTP proxy with CONNECT. UDP cannot be sent."),
            "https": _t("Connects through an upstream HTTPS proxy (TLS) with CONNECT. UDP cannot be sent."),
            "socks4": _t("Through an upstream SOCKS4a proxy. No UDP or IPv6 destinations."),
            "socks5": _t("Through an upstream SOCKS5 proxy (TCP). UDP cannot be sent."),
            "shadowsocks": _t("Encrypted tunnel to a remote Shadowsocks server (TCP+UDP)."),
            "hysteria2": _t("QUIC tunnel to a remote Hysteria2 server (TCP+UDP). Bandwidth (Brutal), Salamander obfuscation and port hopping need the sing-box engine."),
            "block": _t("Refuses every connection. Connect router rules to it (e.g. ads, trackers). Needs the sing-box engine."),
            "vless": _t("VLESS tunnel (TCP+UDP) with TLS or REALITY, optionally over WebSocket/gRPC. Paste a vless:// link to fill everything in. Needs the sing-box engine."),
            "trojan": _t("Trojan tunnel over TLS (TCP+UDP), optionally over WebSocket/gRPC. Paste a trojan:// link to fill everything in. Needs the sing-box engine."),
            "subscription": _t("All servers of a provider's subscription link as one block. Pick a server or let it use the fastest one (it also moves away from servers that stop working). The server list is saved, so it works offline too. Needs the sing-box engine."),
            "group": _t("Uses several outbound blocks as one: the fastest, or the first that works (failover). Servers that stop working are skipped automatically. Needs the sing-box engine."),
            "tuic": _t("TUIC v5 tunnel over QUIC (TCP+UDP). Paste a tuic:// link to fill everything in. Needs the sing-box engine."),
            "wireguard": _t("WireGuard tunnel run inside ProxyDock (TCP+UDP): no VPN app and no change to the PC's network, so several can run at once. 'Through' another WireGuard block makes a double VPN. Needs the sing-box engine."),
        }
        note = out_notes[ot]
        if remote:
            note += _t(" Through an upstream, 'Block private network' only applies to literal IPs and localhost.")
        self.out_note.setText(note)

    # --- connection check ---
    def _probe(self):
        if self.probing:
            return
        n = self.collect()
        if self.kind == "outbound" and n["outbound"]["type"] == "block":
            self._set_probe_text(_t("A Block outbound refuses every connection, so there is nothing to check."), None)
            return
        errs = self._errors(n, for_check=True)
        if errs:
            info(self.window(), "error", _t("Please fix first"), "\n".join(errs[:4]), 6000)
            return
        self.probing = True
        self.btn_probe.setEnabled(False)
        self.btn_probe.setText(_t("Checking…"))
        self._set_probe_text(_t("Connecting through the outbound…"), None)
        p, _ = self._as_proxy(n)
        inbound = self.kind == "inbound"
        running = (inbound and self.ctl.manager.state(n["id"])[0] == "running"
                   and self.node["inbound"]["host"] == n["inbound"]["host"]
                   and self.node["inbound"]["port"] == n["inbound"]["port"])
        self._probe_ot = p["outbound"]["type"]
        mgr, glob = self.ctl.manager, self.ctl.glob
        token = self._probe_token

        def work():
            from ..probe import check_outbound
            out = check_outbound(mgr, p, glob)
            inb = _check_inbound(p, running) if inbound else None
            out["token"] = token
            self.check_done.emit(out, inb)
        threading.Thread(target=work, daemon=True).start()

    def _set_probe_text(self, text, ok):
        d = self.r_probe.desc
        d.setText(text)
        d.setVisible(True)
        if ok is None:
            d.setTextColor()
        else:
            d.setTextColor(QColor("#1E7F3C" if ok else "#C42B1C"), QColor("#6CCB5F" if ok else "#FF99A4"))

    def _on_probe_done(self, out, inb):
        self.probing = False
        self.btn_probe.setEnabled(True)
        self.btn_probe.setText(_t("Check connection"))
        if out.get("token") != self._probe_token or not self.isVisible():
            return
        ot = getattr(self, "_probe_ot", "direct")
        ms = lambda v: f"{v:.0f} ms" if v is not None else "-"
        if out.get("ok"):
            line = (_t("Outbound OK · {site} replied {value} · connect {ms} · total {ms2}").format(site=out['site'], value=out.get('status') or '-', ms=ms(out.get('connect_ms')), ms2=ms(out.get('total_ms'))))
            if out.get("note"):
                line += f" ({out['note']})"
        else:
            line = _t("Outbound failed · {value}").format(value=out.get('error') or _t("unknown error"))
            if out.get("detail"):
                line += f"\n  ↳ {out['detail']}"
            hint = _probe_hint(ot, out)
            if hint:
                line += f"\n  ↳ {hint}"
        in_ok = True
        if inb is not None:
            in_ok, in_msg = inb
            line += _t("\nInbound {value} · {in_msg}").format(value=_t("OK") if in_ok else _t("problem"), in_msg=in_msg)
        ok = bool(out.get("ok")) and in_ok
        self._set_probe_text(line, ok)
        self.ensureWidgetVisible(self.r_probe)
        info(self.window(), "success" if ok else "error", _t("Connection check ") + (_t("succeeded") if ok else _t("failed")),
             line.split("\n", 1)[0], 4000)

    def _check_port(self):
        host = self.ed_in_host.text().strip() or "127.0.0.1"
        port = self.sp_in_port.value()
        it = self.in_types[max(self.cb_in.currentIndex(), 0)]
        if self.node and self.kind == "inbound" and self.ctl.manager.state(self.node["id"])[0] == "running" \
                and self.node["inbound"]["port"] == port and self.node["inbound"]["host"] == host:
            info(self.window(), "success", _t("Port {port}").format(port=port), _t("This proxy is running on this port."), 3000)
            return
        ok, msg = check_port(host, port, udp=it == "hysteria2")
        info(self.window(), "success" if ok else "error", _t("Port {port} ").format(port=port) + (_t("available") if ok else _t("unavailable")), msg, 4000)

    # --- save ---
    def collect(self):
        """The edited block (inbound or outbound node)."""
        from .. import graph as graph_mod
        n = copy.deepcopy(self.node)
        n["name"] = self.ed_name.text().strip()
        if self.kind == "router":
            r = self.s_router.collect(n)
            out = graph_mod.validate_router_node(r)
            out["name"] = n["name"]         # keep empty names so the error check can report them
            for a, b in zip(out["rules"], r["rules"]):
                a["name"] = b["name"]
            return out
        if self.kind == "inbound":
            n["autostart"] = self.sw_auto.isChecked()
            ib = n["inbound"]
            ib.update(type=self.in_types[self.cb_in.currentIndex()], host=self.ed_in_host.text().strip(),
                      port=self.sp_in_port.value(), auth_enabled=self.sw_in_auth.isChecked(),
                      username=self.ed_in_user.text(), password=self.ed_in_pass.text(),
                      tls_cert=self.ed_cert.text().strip(), tls_key=self.ed_key.text().strip(),
                      ss_method=self.ss_methods[self.cb_in_ss.currentIndex()], ss_password=self.ed_in_ss_pw.text().strip(),
                      hy2_password=self.ed_in_hy.text(), hy2_obfs_password=self.ed_in_obfs.text(),
                      hy2_up_mbps=self.sp_in_up.value(), hy2_down_mbps=self.sp_in_down.value(),
                      tun_stack=self.tun_stacks[max(self.cb_tun_stack.currentIndex(), 0)],
                      tun_strict_route=self.sw_tun_strict.isChecked(), tun_fakeip=self.sw_tun_fakeip.isChecked())
            if ib["type"] not in ("socks5", "http", "https"):
                ib["auth_enabled"] = False
            for key, (follow, _w) in self.ov.items():
                n["overrides"][key] = None if follow.isChecked() else self._get_value(key)
            idx = self.cb_target.currentIndex()
            n["target"] = self.targets[idx] if 0 <= idx < len(self.targets) else None
            return graph_mod.validate_inbound_node(n) | {"target": n["target"]}
        ot = self.out_types[self.cb_out.currentIndex()]
        n["outbound"].update(type=ot, host=self.ed_out_host.text().strip(),
                             port=self.sp_out_port.value(), username=self.ed_out_user.text(),
                             password="" if ot == "socks4" else self.ed_out_pass.text(),
                             tls_verify=self.sw_out_verify.isChecked(), sni=self.ed_out_sni.text().strip(),
                             pin_sha256=self.ed_out_pin.text().strip(),
                             ss_method=self.ss_methods[self.cb_out_ss.currentIndex()],
                             ss_password=self.ed_out_ss_pw.text().strip(), hy2_password=self.ed_out_hy.text(),
                             hy2_obfs_password=self.ed_out_obfs.text(), hy2_up_mbps=self.sp_out_up.value(),
                             hy2_down_mbps=self.sp_out_down.value(), hy2_ports=self.ed_out_hop.text().strip(),
                             hy2_hop_interval=self.sp_out_hop.value(),
                             bind=self.ed_out_bind.text().strip() if ot == "direct" else "",
                             interface=self._iface_value() if ot not in ("block", "group") else "",
                             group_mode=self.grp_modes[max(self.cb_grp_mode.currentIndex(), 0)],
                             group_test_url=self.ed_grp_url.text().strip(), group_interval=self.sp_grp_interval.value(),
                             group_tolerance=self.sp_grp_tol.value(),
                             wg_private_key=self.ed_wg_key.text().strip(), wg_address=self.ed_wg_addr.text().strip(),
                             wg_peer_public_key=self.ed_wg_peer.text().strip(), wg_preshared_key=self.ed_wg_psk.text().strip(),
                             wg_allowed_ips=self.ed_wg_allowed.text().strip(), wg_mtu=self.sp_wg_mtu.value(),
                             wg_keepalive=self.sp_wg_keep.value(),
                             uuid=self.ed_out_uuid.text().strip(),
                             security=self.securities[max(self.cb_out_sec.currentIndex(), 0)],
                             vless_flow=self.flows[max(self.cb_out_flow.currentIndex(), 0)],
                             reality_public_key=self.ed_out_pbk.text().strip(), reality_short_id=self.ed_out_sid.text().strip(),
                             fingerprint=self.fingerprints[max(self.cb_out_fp.currentIndex(), 0)],
                             alpn=self.ed_out_alpn.text().strip(),
                             transport=self.transports[max(self.cb_out_tr.currentIndex(), 0)],
                             transport_path=self.ed_out_tr_path.text().strip(), transport_host=self.ed_out_tr_host.text().strip(),
                             tuic_congestion=self.congestions[max(self.cb_out_cc.currentIndex(), 0)],
                             tuic_udp_relay=self.relays[max(self.cb_out_relay.currentIndex(), 0)],
                             sub_url=self.ed_sub_url.text().strip(),
                             sub_mode=self.sub_modes[max(self.cb_sub_mode.currentIndex(), 0)],
                             sub_selected=(self.sub_names[self.cb_sub_server.currentIndex()]
                                           if getattr(self, "sub_names", None) and 0 <= self.cb_sub_server.currentIndex() < len(self.sub_names) else ""),
                             sub_interval=self.sp_sub_hours.value(), **self._sub)
        idx = self.cb_via.currentIndex()
        n["via"] = self.vias[idx] if 0 <= idx < len(self.vias) and ot not in graph_mod.NO_VIA else None
        n["members"] = list(self.grp_members) if ot == "group" else []
        if n["via"]:
            n["outbound"]["interface"] = ""     # a hop that goes through another block does not dial out itself
        return graph_mod.validate_outbound_node(n)

    def _as_proxy(self, n):
        """Runtime proxy for the edited block, compiled against the current graph (outbound: a stand-in inbound)."""
        from .. import graph as graph_mod  # noqa: F811
        if self.kind == "inbound":
            g = copy.deepcopy(self.ctl.graph)
            g["inbounds"] = [n if x["id"] == n["id"] else x for x in g["inbounds"]]
            compiled = graph_mod.compile(g)
            return next(x for x in compiled if x["id"] == n["id"]), [x for x in compiled if x["id"] != n["id"]]
        g = copy.deepcopy(self.ctl.graph)
        g["outbounds"] = [n if x["id"] == n["id"] else x for x in g["outbounds"]]
        p = config.validate_proxy({"id": "__edit__", "name": n["name"], "outbound": n["outbound"]})
        resolved = graph_mod.resolve_outbound(g, n["id"]) or {}
        p["outbound"]["via"] = resolved.get("via")
        if "members" in resolved:
            p["outbound"]["members"] = resolved["members"]
        return p, []

    def _errors(self, n, for_check=False):
        if self.kind == "router":
            return self.s_router.errors(n)
        if self.kind == "outbound":
            from .. import graph as graph_mod
            errs = [] if n["name"] else [_t("Enter a name")]
            g = copy.deepcopy(self.ctl.graph)
            g["outbounds"] = [n if x["id"] == n["id"] else x for x in g["outbounds"]]
            if graph_mod.via_loop(g, n["id"]):
                return errs + [_t("The proxy chain loops: ") + " → ".join(graph_mod.chain_names(g, n["via"]) + [n["name"]])]
            p, _ = self._as_proxy(n)
            return errs + config.outbound_errors(p["outbound"])
        p, others = self._as_proxy(n)
        errs = config.check_proxy(p, others)
        if not for_check:       # an unconnected inbound can be saved, it just cannot start
            errs = [e for e in errs if not p["unlinked"] or e != config_unlinked_error()]
        return errs

    def save(self):
        n = self.collect()
        errs = self._errors(n)
        if errs:
            info(self.window(), "error", _t("Please check"), "\n".join(errs[:4]), 6000)
            return
        if self.kind == "inbound":
            ib = n["inbound"]
            if not is_loopback_host(ib["host"]) and ib["type"] in ("socks5", "http", "https") and not ib["auth_enabled"]:
                info(self.window(), "warning", _t("Exposed without authentication"),
                     _t("Anyone on the same network can use this proxy. Turning on user authentication is recommended."), 6000)
        self.ctl.save_node(self.kind, n)
        self.closed.emit()

    def cancel(self):
        self.closed.emit()


def config_unlinked_error():
    return _t("Not connected to an outbound - connect it on the canvas or choose one in the editor")


CHECK_DESC_OUT = ""


@on_language_change
def _tr_check_desc_out():
    global CHECK_DESC_OUT
    CHECK_DESC_OUT = _t("Tests the current, unsaved settings: connects to an HTTPS site through this outbound.")


CHECK_DESC = ""


@on_language_change
def _tr_check_desc():
    global CHECK_DESC
    CHECK_DESC = _t("Tests the current, unsaved settings: connects to an HTTPS site through the outbound and checks that the inbound port can be opened (including the certificate).")




def _check_inbound(p, running):
    """Inbound check (on a worker thread). (ok, description)."""
    ib = p["inbound"]
    if ib["type"] == "tun":
        return True, _t("TUN: checked when it starts (needs administrator permission)")
    if running:
        return True, _t("Running on {host_port}").format(host_port=host_port(ib['host'], ib['port']))
    ok, msg = check_port(ib["host"], ib["port"], udp=ib["type"] == "hysteria2")
    if not ok:
        return False, _t("Port {port}: {msg}").format(port=ib['port'], msg=msg)
    if ib["type"] in ("https", "hysteria2") and ib["tls_cert"]:
        try:
            from ..tls import server_context
            server_context(ib["tls_cert"], ib["tls_key"])
        except Exception as e:
            return False, _t("Could not read the certificate/key: {e}").format(e=e)
    return True, _t("Port {port} available").format(port=ib['port']) + (_t(" · certificate OK") if ib["tls_cert"] else "")


def _probe_hint(ot, out):
    """Pick a hint from the failure kind (kind: seen by the bridge) and the reason code (reason: failure seen by the proxy)."""
    kind, reason = out.get("kind", ""), out.get("reason", "")
    detail = (out.get("detail") or "").lower()
    if ot == "shadowsocks" and kind in ("timeout", "tls", "closed"):
        return _t("Check that the cipher and password match the server (the server does not answer if they are wrong)")
    if ot == "hysteria2" and reason == "auth":
        return _t("Check the Hysteria2 password")
    if ot in ("http", "https", "socks5") and reason == "auth":
        return _t("Check the upstream proxy username/password")
    if ot in ("https", "hysteria2") and "certificate" in detail:
        return _t("If the server uses a self-signed certificate, turn off 'Verify server certificate' or pin its fingerprint")
    return ""


def _token():
    import secrets
    return secrets.token_urlsafe(18)
