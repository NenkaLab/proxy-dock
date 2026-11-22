"""Router block editor: ordered rules (conditions -> target block) plus a default target.

Shown inside the editor page (ui/proxies.py EditorInterface) when a router block is edited.
"""
from ..i18n import _t
import copy
import ipaddress

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QGridLayout, QHBoxLayout, QVBoxLayout, QWidget
from qfluentwidgets import (
    BodyLabel, CaptionLabel, ComboBox, FluentIcon as FIF, LineEdit, PushButton, SimpleCardWidget, StrongBodyLabel,
    TransparentToolButton,
)

from .. import graph as graph_mod

FIELDS = []         # (match key, label, placeholder)


def _tr_fields():
    FIELDS[:] = [
        ("domain_suffix", _t("Domain (with subdomains)"), "example.com, ads.example.net"),
        ("domain", _t("Domain (exact)"), "www.example.com"),
        ("domain_keyword", _t("Domain keyword"), "google"),
        ("ip_cidr", _t("IP / range"), "10.0.0.0/8, 1.1.1.1"),
        ("geoip", _t("GeoIP country"), "kr, jp, us"),
        ("geosite", _t("Geosite category"), "google, netflix, category-ads-all"),
        ("port", _t("Destination port"), "443, 8000-9000"),
        ("process_name", _t("Program"), "chrome.exe, game.exe"),
    ]


def split_list(text):
    return [x.strip() for x in text.replace(";", ",").split(",") if x.strip()]


def match_errors(name, m):
    """Problems in one rule's conditions (empty list = OK)."""
    from ..singbox.rulesets import valid_name
    errs = []
    for x in m.get("ip_cidr", []):
        try:
            ipaddress.ip_network(x, strict=False)
        except ValueError:
            errs.append(_t("Rule '{name}': '{value}' is not an IP or IP range").format(name=name, value=x))
    for x in m.get("port", []):
        a, _, b = x.partition("-")
        try:
            lo, hi = int(a), int(b or a)
            if not 1 <= lo <= hi <= 65535:
                raise ValueError
        except ValueError:
            errs.append(_t("Rule '{name}': '{value}' is not a port or port range (1-65535)").format(name=name, value=x))
    for x in m.get("geoip", []):
        if not valid_name(x):
            errs.append(_t("Rule '{name}': '{value}' is not a country code (e.g. kr)").format(name=name, value=x))
    for x in m.get("geosite", []):
        if not valid_name(x):
            errs.append(_t("Rule '{name}': '{value}' is not a Geosite category name").format(name=name, value=x))
    return errs


class RuleCard(SimpleCardWidget):
    """One rule: name, conditions, target and move/delete buttons."""

    def __init__(self, section, rule, parent=None):
        super().__init__(parent)
        self.section = section
        self.rule = copy.deepcopy(rule)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(16, 10, 16, 12)
        lay.setSpacing(6)
        top = QHBoxLayout()
        self.idx = StrongBodyLabel("", self)
        self.ed_name = LineEdit(self)
        self.ed_name.setFixedWidth(200)
        self.ed_name.setText(rule["name"])
        self.cb_target = ComboBox(self)
        self.cb_target.setFixedWidth(260)
        b_up = TransparentToolButton(FIF.UP, self)
        b_down = TransparentToolButton(FIF.DOWN, self)
        b_del = TransparentToolButton(FIF.DELETE, self)
        b_up.setToolTip(_t("Move up"))
        b_down.setToolTip(_t("Move down"))
        b_del.setToolTip(_t("Delete rule"))
        b_up.clicked.connect(lambda: section.move(self, -1))
        b_down.clicked.connect(lambda: section.move(self, 1))
        b_del.clicked.connect(lambda: section.remove(self))
        top.addWidget(self.idx)
        top.addWidget(self.ed_name)
        top.addWidget(BodyLabel("→", self))
        top.addWidget(self.cb_target)
        top.addStretch(1)
        for b in (b_up, b_down, b_del):
            top.addWidget(b)
        lay.addLayout(top)
        grid = QGridLayout()
        grid.setHorizontalSpacing(10)
        grid.setVerticalSpacing(4)
        self.edits = {}
        m = rule["match"]
        for i, (key, label, ph) in enumerate(FIELDS):
            ed = LineEdit(self)
            ed.setPlaceholderText(ph)
            ed.setText(", ".join(m.get(key, [])))
            ed.setClearButtonEnabled(True)
            self.edits[key] = ed
            grid.addWidget(CaptionLabel(label, self), i // 2, (i % 2) * 2)
            grid.addWidget(ed, i // 2, (i % 2) * 2 + 1)
        from .proxies import _switch
        self.sw_private = _switch(self)
        self.sw_private.setChecked(bool(m.get("ip_private")))
        row = len(FIELDS) // 2
        grid.addWidget(CaptionLabel(_t("Private network addresses"), self), row, 0)
        grid.addWidget(self.sw_private, row, 1, Qt.AlignLeft)
        grid.setColumnStretch(1, 1)
        grid.setColumnStretch(3, 1)
        lay.addLayout(grid)

    def set_index(self, i):
        self.idx.setText(f"{i + 1}.")

    def set_targets(self, ids, labels):
        self.target_ids = ids
        self.cb_target.clear()
        self.cb_target.addItems(labels)
        t = self.rule["target"]
        self.cb_target.setCurrentIndex(ids.index(t) if t in ids else 0)

    def collect(self):
        r = copy.deepcopy(self.rule)
        r["name"] = self.ed_name.text().strip()
        m = {k: split_list(ed.text()) for k, ed in self.edits.items()}
        m["ip_private"] = self.sw_private.isChecked()
        r["match"] = graph_mod.validate_match(m)
        i = self.cb_target.currentIndex()
        r["target"] = self.target_ids[i] if 0 <= i < len(self.target_ids) else None
        return r


class RouterSection(QWidget):
    """Rules of a router block + its default target."""

    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        _tr_fields()
        self.ctl = ctl
        self.cards = []
        self.router = None
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(8)
        from qfluentwidgets import SubtitleLabel
        lay.addWidget(SubtitleLabel(_t("Rules (checked from the top; the first match wins)"), self))
        note = CaptionLabel(_t("Inside one rule, any one of the address conditions (domains, IPs, GeoIP, Geosite, private "
                               "network) is enough; port and program must also match. Separate several values with commas. "
                               "Program rules work for connections from this PC. GeoIP and Geosite data is downloaded once a day."), self)
        note.setWordWrap(True)
        lay.addWidget(note)
        self.list = QVBoxLayout()
        self.list.setSpacing(8)
        lay.addLayout(self.list)
        btns = QHBoxLayout()
        b_add = PushButton(_t("Add rule"), self, FIF.ADD)
        b_add.clicked.connect(self.add_rule)
        btns.addWidget(b_add)
        btns.addStretch(1)
        lay.addLayout(btns)
        default_row = SimpleCardWidget(self)
        dl = QHBoxLayout(default_row)
        dl.setContentsMargins(16, 10, 16, 10)
        col = QVBoxLayout()
        col.addWidget(BodyLabel(_t("Default"), default_row))
        d = CaptionLabel(_t("Where traffic goes when no rule matches. Required."), default_row)
        col.addWidget(d)
        dl.addLayout(col, 1)
        self.cb_default = ComboBox(default_row)
        self.cb_default.setFixedWidth(260)
        dl.addWidget(self.cb_default)
        lay.addWidget(default_row)

    # --- targets: outbound blocks (Block included) and other routers ---
    def _targets(self):
        g = self.ctl.graph
        ids = [None] + [x["id"] for x in g["outbounds"]] + [x["id"] for x in g.get("routers", []) if x["id"] != self.router["id"]]
        labels = [_t("(not connected)")] + [x["name"] for x in g["outbounds"]] + \
            [_t("Router {name}").format(name=x["name"]) for x in g.get("routers", []) if x["id"] != self.router["id"]]
        return ids, labels

    def load(self, router):
        self.router = copy.deepcopy(router)
        for c in self.cards:
            c.setParent(None)
            c.deleteLater()
        self.cards = []
        for r in self.router["rules"]:
            self._add_card(r)
        ids, labels = self._targets()
        self.default_ids = ids
        self.cb_default.clear()
        self.cb_default.addItems(labels)
        d = self.router["default"]
        self.cb_default.setCurrentIndex(ids.index(d) if d in ids else 0)
        self._renumber()

    def _add_card(self, rule):
        card = RuleCard(self, rule, self)
        card.set_targets(*self._targets())
        self.cards.append(card)
        self.list.addWidget(card)
        return card

    def _renumber(self):
        for i, c in enumerate(self.cards):
            self.list.removeWidget(c)
            self.list.insertWidget(i, c)
            c.set_index(i)

    def add_rule(self):
        rule = {"id": graph_mod.new_id(), "name": _t("Rule {n}").format(n=len(self.cards) + 1), "match": {}, "target": None}
        card = self._add_card(rule)
        self._renumber()
        card.ed_name.setFocus()

    def move(self, card, step):
        i = self.cards.index(card)
        j = i + step
        if 0 <= j < len(self.cards):
            self.cards[i], self.cards[j] = self.cards[j], self.cards[i]
            self._renumber()

    def remove(self, card):
        self.cards.remove(card)
        card.setParent(None)
        card.deleteLater()
        self._renumber()

    def collect(self, node):
        n = copy.deepcopy(node)
        n["rules"] = [c.collect() for c in self.cards]
        i = self.cb_default.currentIndex()
        n["default"] = self.default_ids[i] if 0 <= i < len(self.default_ids) else None
        return n

    def errors(self, n):
        errs = [] if n["name"] else [_t("Enter a name")]
        if not n["default"]:
            errs.append(_t("Choose the default target (where traffic goes when no rule matches)"))
        for i, r in enumerate(n["rules"]):
            name = r["name"] or str(i + 1)
            if not r["name"]:
                errs.append(_t("Rule {n}: enter a name").format(n=i + 1))
            if r["target"] and not r["match"]:
                errs.append(_t("Rule '{name}': add at least one condition").format(name=name))
            errs += match_errors(name, r["match"])
        g = copy.deepcopy(self.ctl.graph)
        g["routers"] = [n if x["id"] == n["id"] else x for x in g.get("routers", [])]
        if graph_mod.router_loop(g, n["id"]):
            errs.append(_t("The routers point at each other in a loop"))
        return errs
