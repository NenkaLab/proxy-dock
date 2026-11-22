"""How-to page."""
from ..i18n import _t, on_language_change
from PySide6.QtCore import Qt
from PySide6.QtWidgets import QHBoxLayout, QVBoxLayout, QWidget
from qfluentwidgets import (
    BodyLabel, CaptionLabel, FluentIcon as FIF, IconWidget, PushButton, ScrollArea, SimpleCardWidget, StrongBodyLabel,
    TitleLabel,
)

from ..common import IS_WIN

STEPS = []


@on_language_change
def _tr_steps():
    STEPS[:] = [
        (FIF.TILES, _t("1. ProxyDock is a \"dock\" for proxies"),
         _t("Proxies are built from blocks. Inbound blocks accept connections on a port of this PC (SOCKS5 · SOCKS4 · HTTP · HTTPS · Shadowsocks · Hysteria2); outbound blocks decide how traffic reaches the destination (Direct, optionally from a chosen source IP · upstream HTTP/HTTPS/SOCKS4/SOCKS5 proxy · Shadowsocks tunnel · Hysteria2 tunnel).\nOn the Canvas, drag a line from an inbound's output to an outbound block. Several inbounds can share one outbound; linking an inbound into another inbound relays traffic through it.")),
        (FIF.ADD, _t("2. Create and run"),
         _t("Add blocks with 'Add inbound' / 'Add outbound' (or right-click the canvas), connect them, and double-click a block to edit it. The Proxy page shows the same blocks as a list.\nUse 'Check connection' in the editor to test the outbound and the inbound port before saving. Start/stop an inbound with its switch or right-click menu; 'Start with the app' starts it with ProxyDock.\nPC block: connect an HTTP inbound to it to use it as the Windows system proxy while that inbound runs. The previous settings are saved and put back when it stops or ProxyDock quits; 'Restore previous' and 'Off (default)' disconnect it.")),
        (FIF.GLOBE, _t("3. Use it in your apps"),
         _t("Copy the address (or link) with the copy button on a card and put it in your app's proxy settings.\n• Firefox: Settings → Network Settings → Manual proxy (for SOCKS5, check 'Proxy DNS when using SOCKS v5')\n• Chrome/Edge: append  --proxy-server=\"socks5://127.0.0.1:1080\"  to the shortcut target (http:// also works)\n• curl:  curl -x socks5h://127.0.0.1:1080 https://example.com   (HTTPS proxy: -x https://… --proxy-insecure)\n• Shadowsocks / Hysteria2 apps: paste the copied ss:// · hysteria2:// link")),
        (FIF.VPN, _t("4. Tunnels and upstream proxies"),
         _t("Changing the outbound hands connections over to another server.\n• Through a company proxy: inbound SOCKS5 (local) → outbound HTTP proxy (company address)\n• Encrypted tunnel: inbound SOCKS5 (local) → outbound Shadowsocks/Hysteria2 tunnel (remote server). The remote server can be a 'Shadowsocks server'/'Hysteria2 server' proxy in ProxyDock on another PC.\nUDP (QUIC, games, DNS) works when receiving via SOCKS5/Shadowsocks/Hysteria2 and sending via Direct/Shadowsocks/Hysteria2. Hysteria2 is a Python implementation, so expect about 100 Mbps.")),
        (FIF.SETTING, _t("5. Global and per-proxy settings"),
         _t("The 'Proxy defaults' on the Settings page (allowed domains, private network blocking, timeout, logging) apply to every proxy.\nIn the proxy editor, uncheck 'Use global' on an item to give that proxy its own value.\nChanges made while running: allowed domains, private network blocking and logging apply to new connections; the timeout applies to current connections too. Changing the inbound/outbound restarts that proxy and drops its connections.\nWhen chaining proxies (A's outbound = B's inbound), a chain that loops cannot be saved.")),
        (FIF.WIFI, _t("6. Use from / block other PCs"),
         (_t("Set the inbound address to 0.0.0.0 and (for SOCKS5/HTTP/HTTPS) turn on user authentication. If the firewall block below is on, turn it off. If you only use it on this PC, keeping the block on is safer (an administrator prompt appears)."))
         if IS_WIN else
         (_t("Set the inbound address to 0.0.0.0 and (for SOCKS5/HTTP/HTTPS) turn on user authentication. If an OS firewall (macOS firewall, ufw/firewalld, etc.) is on, allow the port."))),
        (FIF.MINIMIZE, _t("7. Closing and quitting"),
         _t("By default, X only hides the window and the proxies keep running in the tray.\nTo quit completely, right-click the tray icon → Quit. (Change this in Settings → Display → 'Quit with the X button')")),
    ]




class HelpInterface(ScrollArea):
    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        self.setObjectName("helpInterface")
        self.view = QWidget(self)
        self.setWidget(self.view)
        self.setWidgetResizable(True)
        self.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        self.enableTransparentBackground()

        root = QVBoxLayout(self.view)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(12)
        root.addWidget(TitleLabel(_t("How to use"), self.view))
        for i, (icon, title, body) in enumerate(STEPS):
            card = SimpleCardWidget(self.view)
            lay = QHBoxLayout(card)
            lay.setContentsMargins(20, 16, 20, 16)
            lay.setSpacing(16)
            ic = IconWidget(icon, card)
            ic.setFixedSize(20, 20)
            lay.addWidget(ic, 0, Qt.AlignTop)
            col = QVBoxLayout()
            col.setSpacing(4)
            col.addWidget(StrongBodyLabel(title, card))
            text = BodyLabel(body, card)
            text.setWordWrap(True)
            text.setTextInteractionFlags(Qt.TextSelectableByMouse)
            col.addWidget(text)
            lay.addLayout(col, 1)
            if i == 5 and IS_WIN:
                box = QVBoxLayout()
                box.setSpacing(6)
                self.fw_label = CaptionLabel(_t("Firewall block: checking…"), card)
                self.btn_fw_on = PushButton(_t("Turn block on"), card, FIF.CERTIFICATE)
                self.btn_fw_off = PushButton(_t("Turn block off"), card, FIF.CLOSE)
                self.btn_fw_on.clicked.connect(lambda: ctl.firewall(True))
                self.btn_fw_off.clicked.connect(lambda: ctl.firewall(False))
                box.addWidget(self.fw_label, 0, Qt.AlignRight)
                row = QHBoxLayout()
                row.addWidget(self.btn_fw_on)
                row.addWidget(self.btn_fw_off)
                box.addLayout(row)
                lay.addLayout(box)
            root.addWidget(card)
        root.addStretch(1)

    def set_firewall_status(self, state):
        if not hasattr(self, "fw_label"):
            return
        self.fw_label.setText(_t("Firewall block: ") + {True: _t("On"), False: _t("Off"), None: _t("Checking…")}[state])
        busy = state is None
        self.btn_fw_on.setEnabled(not busy and state is not True)
        self.btn_fw_off.setEnabled(not busy and state is not False)
