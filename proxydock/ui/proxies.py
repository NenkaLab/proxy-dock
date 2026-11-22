"""Proxies page: list (cards) + editor.

A proxy = inbound (how it accepts connections on a port of this PC) + outbound (how it reaches the destination) + local settings.
"""
from ..i18n import _t, on_language_change
import copy
import os
import threading

from PySide6.QtCore import Qt, Signal
from PySide6.QtGui import QColor, QPainter
from PySide6.QtWidgets import QApplication, QFileDialog, QHBoxLayout, QLabel, QVBoxLayout, QWidget
from qfluentwidgets import (
    Action, BodyLabel, CaptionLabel, CardWidget, CheckBox, ComboBox, DropDownPushButton, FluentIcon as FIF, LineEdit,
    MessageBox,
    PasswordLineEdit, PrimaryDropDownPushButton, PrimaryPushButton, PushButton, RoundMenu, ScrollArea,
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
    if t == "shadowsocks":
        return ss_link(host, ib["port"], ib["ss_method"], ib["ss_password"], p["name"])
    if t == "hysteria2":
        return hy2_link(host, ib["port"], ib["hy2_password"], insecure=not ib["tls_cert"], name=p["name"])
    scheme = {"socks5": "socks5", "socks4": "socks4a", "http": "http", "https": "https"}[t]
    cred = ""
    if t in ("socks5", "http", "https") and ib["auth_enabled"]:
        cred = f"{ib['username']}:{ib['password']}@"
    return f"{scheme}://{cred}{host_port(host, ib['port'])}"


def summary(p):
    ib, ob = p["inbound"], p["outbound"]
    left = f"{INBOUND_TYPES[ib['type']]} {host_port(ib['host'], ib['port'])}"
    if "target_name" in p:
        right = p["target_name"]
    else:
        right = "Direct" if ob["type"] == "direct" else f"{OUTBOUND_TYPES[ob['type']]} {host_port(ob['host'], ob['port'])}"
    return f"{left}  →  {right}"


def outbound_summary(ob):
    if ob["type"] == "direct":
        return _t("Direct from this PC") + (f" · {ob['bind']}" if ob.get("bind") else "")
    return f"{OUTBOUND_TYPES[ob['type']]} · {host_port(ob['host'], ob['port']) if ob['host'] else _t('server not set')}"


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
        self.desc.setText(outbound_summary(node["outbound"]) + ("  ⇢ " + " ⇢ ".join(chain) if chain else ""))
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
        s_in.add(Row(_t("Listen address · port"), _t("127.0.0.1 = this PC only, 0.0.0.0 = allow other PCs (check authentication and firewall)"), v)
                 .right(self.ed_in_host, self.sp_in_port, btn_check))
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
        self.in_note = CaptionLabel("", v)
        self.in_note.setWordWrap(True)
        s_in.add(self.in_note)

        # outbound
        s_out = Section(_t("Outbound (how traffic reaches the destination)"), v)
        self.cb_out = ComboBox(v)
        self.out_types = list(OUTBOUND_TYPES)
        self.cb_out.addItems([OUTBOUND_TYPES[t] for t in self.out_types])
        self.cb_out.setFixedWidth(200)
        s_out.add(Row(_t("Type"), _t("Direct = connect straight from this PC. The others go through another proxy/server"), v).right(self.cb_out))
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
        self.ed_out_user = LineEdit(v)
        self.ed_out_user.setPlaceholderText(_t("Username (leave empty if none)"))
        self.ed_out_user.setFixedWidth(170)
        self.ed_out_pass = PasswordLineEdit(v)
        self.ed_out_pass.setPlaceholderText(_t("Password"))
        self.ed_out_pass.setFixedWidth(180)
        self.r_out_cred = s_out.add(Row(_t("Upstream proxy account"), "", v).right(self.ed_out_user, self.ed_out_pass))
        self.sw_out_verify = _switch(v, _t("Verify"), _t("Off"))
        self.r_out_verify = s_out.add(Row(_t("Verify server certificate"), _t("Turn off for servers with self-signed certificates (pinning the fingerprint is recommended)"), v)
                                      .right(self.sw_out_verify))
        self.ed_out_sni = LineEdit(v)
        self.ed_out_sni.setPlaceholderText(_t("Empty = server address"))
        self.ed_out_sni.setFixedWidth(220)
        self.r_out_sni = s_out.add(Row(_t("SNI (server name)"), "", v).right(self.ed_out_sni))
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
        for key, title, desc, w in (
            ("allowed_domains", _t("Allowed domains"), _t("Comma separated · * = all · *.example.com = including subdomains"), self.ed_allow),
            ("block_private", _t("Block private network destinations"), _t("Refuse connections to loopback and private IPs"), self.sw_private),
            ("timeout", _t("Idle timeout (s)"), "", self.sp_timeout),
            ("log_connections", _t("Log connections"), "", self.sw_logconn),
            ("log_close", _t("Log connection close details"), "", self.sw_logclose),
        ):
            follow = CheckBox(_t("Use global"), v)
            follow.stateChanged.connect(lambda *_, k=key: self._sync_override(k))
            self.ov[key] = (follow, w)
            s_local.add(Row(title, desc, v).right(follow, w))

        header = QHBoxLayout()
        header.addWidget(self.title)
        header.addStretch(1)
        header.addWidget(self.btn_cancel)
        header.addWidget(self.btn_save)
        root = QVBoxLayout(v)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(18)
        root.addLayout(header)
        self.sections = {"basic": s_basic, "in": s_in, "link": s_link, "out": s_out, "check": s_check, "local": s_local}
        for s in (s_basic, s_in, s_link, s_out, s_check, s_local):
            root.addWidget(s)
        root.addStretch(1)

        self.cb_in.currentIndexChanged.connect(lambda *_: self._sync())
        self.cb_out.currentIndexChanged.connect(lambda *_: self._sync())
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
        """Edit an inbound or outbound block of the graph."""
        from .. import config as cfg
        self.kind = kind
        self.node = copy.deepcopy(node)
        n = self.node
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
        self.ed_out_bind.setText(ob.get("bind", ""))
        glob = self.ctl.glob
        for key, (follow, w) in self.ov.items():
            follow.blockSignals(True)
            follow.setChecked(ov.get(key) is None)
            follow.blockSignals(False)
            self._set_value(key, ov[key] if ov.get(key) is not None else glob[key])
            w.setEnabled(ov.get(key) is not None)
        if not inbound:
            g = self.ctl.graph
            others = [x for x in g["outbounds"] if x["id"] != n["id"] and x["outbound"]["type"] != "direct"]
            self.vias = [None] + [x["id"] for x in others]
            self.cb_via.clear()
            self.cb_via.addItems([_t("(directly)")] + [x["name"] for x in others])
            self.cb_via.setCurrentIndex(self.vias.index(n.get("via")) if n.get("via") in self.vias else 0)
        if inbound:
            g = self.ctl.graph
            self.targets = [None] + [x["id"] for x in g["outbounds"]] + [x["id"] for x in g["inbounds"] if x["id"] != n["id"]]
            labels = [_t("(not connected)")] + [x["name"] for x in g["outbounds"]] + \
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

    def _set_value(self, key, val):
        w = self.ov[key][1]
        if key == "allowed_domains":
            w.setText(", ".join(val))
        elif key == "timeout":
            w.setValue(val)
        else:
            w.setChecked(val)

    def _get_value(self, key):
        w = self.ov[key][1]
        if key == "allowed_domains":
            return [d.strip() for d in w.text().replace(";", ",").split(",") if d.strip()] or ["*"]
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
        self.r_in_auth.setVisible(can_auth)
        self.r_in_cred.setVisible(can_auth and self.sw_in_auth.isChecked())
        self.r_cert.setVisible(it in ("https", "hysteria2"))
        self.r_key.setVisible(it in ("https", "hysteria2"))
        self.r_in_ss_method.setVisible(it == "shadowsocks")
        self.r_in_ss_pw.setVisible(it == "shadowsocks")
        self.r_in_hy.setVisible(it == "hysteria2")
        notes = {
            "socks5": _t("The most widely used proxy. Relays both TCP and UDP (QUIC, DNS, etc.)."),
            "socks4": _t("SOCKS4/4a. TCP only, no password authentication (the user ID is ignored). Use it on this PC only."),
            "http": _t("For the 'HTTP proxy' setting of browsers and the OS. HTTPS sites are tunnelled with CONNECT. TCP only."),
            "https": _t("Wraps the HTTP proxy in TLS. Set it as an 'HTTPS proxy' in the client (e.g. curl -x https://…)."),
            "shadowsocks": _t("Encrypted tunnel server for Shadowsocks apps on other devices. TCP+UDP. Copy the ss:// link with the button on the card."),
            "hysteria2": _t("Encrypted tunnel server over QUIC (UDP). TCP+UDP. Copy the hysteria2:// link with the button on the card. Python implementation, about 100 Mbps."),
        }
        self.in_note.setText(notes[it])
        remote = ot != "direct"
        self.r_out_server.setVisible(remote)
        self.r_out_bind.setVisible(not remote)
        self.r_out_via.setVisible(ot not in ("direct", "hysteria2"))
        self.r_out_cred.setVisible(ot in ("http", "https", "socks4", "socks5"))
        self.ed_out_pass.setVisible(ot != "socks4")
        self.r_out_verify.setVisible(ot in ("https", "hysteria2"))
        self.r_out_sni.setVisible(ot in ("https", "hysteria2"))
        self.r_out_pin.setVisible(ot == "hysteria2")
        self.r_out_ss_method.setVisible(ot == "shadowsocks")
        self.r_out_ss_pw.setVisible(ot == "shadowsocks")
        self.r_out_hy.setVisible(ot == "hysteria2")
        out_notes = {
            "direct": _t("Connects to the destination directly from this PC."),
            "http": _t("Connects through an upstream HTTP proxy with CONNECT. UDP cannot be sent."),
            "https": _t("Connects through an upstream HTTPS proxy (TLS) with CONNECT. UDP cannot be sent."),
            "socks4": _t("Through an upstream SOCKS4a proxy. No UDP or IPv6 destinations."),
            "socks5": _t("Through an upstream SOCKS5 proxy (TCP). UDP cannot be sent."),
            "shadowsocks": _t("Encrypted tunnel to a remote Shadowsocks server (TCP+UDP)."),
            "hysteria2": _t("QUIC tunnel to a remote Hysteria2 server (TCP+UDP). Bandwidth settings (Brutal), obfuscation and port hopping are not supported."),
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
        if self.kind == "inbound":
            n["autostart"] = self.sw_auto.isChecked()
            ib = n["inbound"]
            ib.update(type=self.in_types[self.cb_in.currentIndex()], host=self.ed_in_host.text().strip(),
                      port=self.sp_in_port.value(), auth_enabled=self.sw_in_auth.isChecked(),
                      username=self.ed_in_user.text(), password=self.ed_in_pass.text(),
                      tls_cert=self.ed_cert.text().strip(), tls_key=self.ed_key.text().strip(),
                      ss_method=self.ss_methods[self.cb_in_ss.currentIndex()], ss_password=self.ed_in_ss_pw.text().strip(),
                      hy2_password=self.ed_in_hy.text())
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
                             bind=self.ed_out_bind.text().strip() if ot == "direct" else "")
        idx = self.cb_via.currentIndex()
        n["via"] = self.vias[idx] if 0 <= idx < len(self.vias) and ot not in graph_mod.NO_VIA else None
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
        p["outbound"]["via"] = (graph_mod.resolve_outbound(g, n["id"]) or {}).get("via")
        return p, []

    def _errors(self, n, for_check=False):
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
