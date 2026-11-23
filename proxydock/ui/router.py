"""Router block editor: ordered rules (conditions -> target block) plus a default target.

Shown inside the editor page (ui/proxies.py EditorInterface) when a router block is edited.
Two views of the same rules: Simple (one row per condition, program picker, rule templates) and Advanced (every
condition field at once).
"""
from ..i18n import _t
import copy
import ipaddress
import os

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QAbstractItemView, QFileDialog, QGridLayout, QHBoxLayout, QListWidgetItem, QVBoxLayout, QWidget
from qfluentwidgets import (
    Action, BodyLabel, CaptionLabel, ComboBox, DropDownPushButton, FluentIcon as FIF, LineEdit, ListWidget, MessageBoxBase,
    PushButton, RoundMenu, SearchLineEdit, SegmentedWidget, SimpleCardWidget, StrongBodyLabel, SubtitleLabel,
    TransparentToolButton,
)

from .. import graph as graph_mod

FIELDS = []         # (match key, label, placeholder)
PRIVATE = "ip_private"
MODE = {"advanced": False}      # view shared by all rule cards (Simple / Advanced), kept while the app runs


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


def templates():
    """(menu label, rule name, conditions, target block type or None) for 'Add rule'."""
    return [
        (_t("Empty rule"), None, {}, None),
        (_t("Programs..."), _t("Programs"), {"process_name": []}, None),
        (_t("Ads and trackers (Geosite)"), _t("Ads"), {"geosite": ["category-ads-all"]}, "block"),
        (_t("Local network (LAN, private IPs)"), _t("Local network"), {PRIVATE: True}, "direct"),
        (_t("Sites of one country (GeoIP + Geosite)"), _t("Country"), {"geoip": ["kr"], "geosite": ["kr"]}, None),
    ]


def match_summary(m, limit=48):
    """Short text of a rule's conditions, e.g. "Program: game.exe, steam.exe + GeoIP country: kr"."""
    if not FIELDS:
        _tr_fields()
    parts = [f"{label}: {', '.join(m[k])}" for k, label, _ in FIELDS if m.get(k)]
    if m.get(PRIVATE):
        parts.append(_t("Private network addresses"))
    text = " + ".join(parts)
    return text if len(text) <= limit else text[:limit - 1] + "…"


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


class ProgramPicker(MessageBoxBase):
    """Pick programs for a rule: the ones running now, or an executable file."""

    def __init__(self, parent, current):
        super().__init__(parent)
        from ..platform_utils import running_programs
        self.viewLayout.addWidget(SubtitleLabel(_t("Choose programs"), self))
        self.search = SearchLineEdit(self)
        self.search.setPlaceholderText(_t("Search running programs"))
        self.search.textChanged.connect(self._filter)
        self.viewLayout.addWidget(self.search)
        self.list = ListWidget(self)
        self.list.setSelectionMode(QAbstractItemView.MultiSelection)
        self.list.setMinimumSize(420, 320)
        have = {x.lower() for x in current}
        for name in running_programs():
            item = QListWidgetItem(name)
            self.list.addItem(item)
            item.setSelected(name.lower() in have)
        self.viewLayout.addWidget(self.list)
        b = PushButton(_t("Choose a program file..."), self, FIF.FOLDER)
        b.clicked.connect(self._browse)
        self.viewLayout.addWidget(b)
        note = CaptionLabel(_t("Rules match the program's file name, so it does not need to be running now."), self)
        note.setWordWrap(True)
        self.viewLayout.addWidget(note)
        self.extra = []
        self.yesButton.setText(_t("Add"))
        self.cancelButton.setText(_t("Cancel"))

    def _filter(self, text):
        t = text.strip().lower()
        for i in range(self.list.count()):
            item = self.list.item(i)
            item.setHidden(bool(t) and t not in item.text().lower())

    def _browse(self):
        path, _ = QFileDialog.getOpenFileName(self, _t("Choose a program file..."), "",
                                              _t("Programs (*.exe);;All files (*)") if os.name == "nt" else "")
        if path:
            self.extra.append(os.path.basename(path))
            self.accept()

    def chosen(self):
        return [self.list.item(i).text() for i in range(self.list.count()) if self.list.item(i).isSelected()] + self.extra


class CondRow(QWidget):
    """Simple view: one condition = type + values (+ program picker)."""

    def __init__(self, card, key, values, parent=None):
        super().__init__(parent)
        self.card = card
        lay = QHBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(8)
        self.keys = [k for k, _, _ in FIELDS] + [PRIVATE]
        self.cb = ComboBox(self)
        self.cb.addItems([label for _, label, _ in FIELDS] + [_t("Private network addresses")])
        self.cb.setFixedWidth(220)
        self.ed = LineEdit(self)
        self.ed.setClearButtonEnabled(True)
        self.on = CaptionLabel(_t("LAN, loopback and other private IPs"), self)
        self.pick = PushButton(_t("Choose..."), self, FIF.APPLICATION)
        self.pick.clicked.connect(self.pick_programs)
        rm = TransparentToolButton(FIF.CLOSE, self)
        rm.setToolTip(_t("Remove condition"))
        rm.clicked.connect(lambda: card.remove_row(self))
        lay.addWidget(self.cb)
        lay.addWidget(self.ed, 1)
        lay.addWidget(self.on, 1)
        lay.addWidget(self.pick)
        lay.addWidget(rm)
        self.cb.setCurrentIndex(self.keys.index(key) if key in self.keys else 0)
        self.ed.setText(", ".join(values))
        self.cb.currentIndexChanged.connect(lambda *_: self._sync())
        self._sync()

    def key(self):
        return self.keys[max(self.cb.currentIndex(), 0)]

    def _sync(self):
        k = self.key()
        self.ed.setVisible(k != PRIVATE)
        self.on.setVisible(k == PRIVATE)
        self.pick.setVisible(k == "process_name")
        self.ed.setPlaceholderText(next((ph for key, _, ph in FIELDS if key == k), ""))

    def add_programs(self, names):
        cur = split_list(self.ed.text())
        low = {c.lower() for c in cur}
        for x in names:
            if x.lower() not in low:
                low.add(x.lower())
                cur.append(x)
        self.ed.setText(", ".join(cur))

    def pick_programs(self):
        dlg = ProgramPicker(self.window(), split_list(self.ed.text()))
        if dlg.exec():
            self.add_programs(dlg.chosen())


class RuleCard(SimpleCardWidget):
    """One rule: name, target, conditions (Simple: a row per condition, Advanced: every field) and move/delete."""

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
        self.is_if = section.is_if
        self.then = BodyLabel(_t("then →"), self)
        top.addWidget(self.idx)
        top.addWidget(self.ed_name)
        top.addWidget(BodyLabel("→", self))
        top.addWidget(self.then)
        top.addWidget(self.cb_target)
        top.addStretch(1)
        for b in (b_up, b_down, b_del):
            top.addWidget(b)
            b.setVisible(not self.is_if)
        self.idx.setVisible(not self.is_if)
        self.ed_name.setVisible(not self.is_if)
        self.then.setVisible(self.is_if)
        top.itemAt(2).widget().setVisible(not self.is_if)       # the plain arrow
        lay.addLayout(top)
        # simple view
        self.simple = QWidget(self)
        sl = QVBoxLayout(self.simple)
        sl.setContentsMargins(0, 0, 0, 0)
        sl.setSpacing(4)
        self.rows_box = QVBoxLayout()
        self.rows_box.setSpacing(4)
        sl.addLayout(self.rows_box)
        ah = QHBoxLayout()
        add = PushButton(_t("Add condition"), self.simple, FIF.ADD)
        add.clicked.connect(lambda: self.add_row())
        ah.addWidget(add)
        hint = CaptionLabel(_t("One matching website / IP / country condition is enough; ports and programs must also match."), self.simple)
        hint.setWordWrap(True)
        ah.addWidget(hint, 1)
        sl.addLayout(ah)
        self.rows = []
        lay.addWidget(self.simple)
        # advanced view
        self.adv = QWidget(self)
        grid = QGridLayout(self.adv)
        grid.setContentsMargins(0, 0, 0, 0)
        grid.setHorizontalSpacing(10)
        grid.setVerticalSpacing(4)
        self.edits = {}
        for i, (key, label, ph) in enumerate(FIELDS):
            ed = LineEdit(self.adv)
            ed.setPlaceholderText(ph)
            ed.setClearButtonEnabled(True)
            self.edits[key] = ed
            grid.addWidget(CaptionLabel(label, self.adv), i // 2, (i % 2) * 2)
            grid.addWidget(ed, i // 2, (i % 2) * 2 + 1)
        from .proxies import _switch
        self.sw_private = _switch(self.adv)
        row = len(FIELDS) // 2
        grid.addWidget(CaptionLabel(_t("Private network addresses"), self.adv), row, 0)
        grid.addWidget(self.sw_private, row, 1, Qt.AlignLeft)
        grid.setColumnStretch(1, 1)
        grid.setColumnStretch(3, 1)
        lay.addWidget(self.adv)
        self.m = copy.deepcopy(rule["match"])
        self.advanced = None
        self.set_mode(MODE["advanced"])

    # --- views ---
    def read(self):
        """Conditions from the view that is shown."""
        if self.advanced:
            m = {k: split_list(ed.text()) for k, ed in self.edits.items()}
            m[PRIVATE] = self.sw_private.isChecked()
            return m
        m = {}
        for r in self.rows:
            k = r.key()
            if k == PRIVATE:
                m[PRIVATE] = True
            else:
                vals = m.setdefault(k, [])
                vals += [x for x in split_list(r.ed.text()) if x not in vals]
        return m

    def set_mode(self, advanced):
        if self.advanced is not None:
            self.m = self.read()
        self.advanced = advanced
        if advanced:
            for k, ed in self.edits.items():
                ed.setText(", ".join(self.m.get(k, [])))
            self.sw_private.setChecked(bool(self.m.get(PRIVATE)))
        else:
            for r in self.rows:
                r.setParent(None)
                r.deleteLater()
            self.rows = []
            for k, _, _ in FIELDS:
                if self.m.get(k):
                    self.add_row(k, self.m[k])
            if self.m.get(PRIVATE):
                self.add_row(PRIVATE)
            if not self.rows:
                self.add_row("domain_suffix")
        self.simple.setVisible(not advanced)
        self.adv.setVisible(advanced)

    def add_row(self, key=None, values=()):
        if key is None:
            used = {r.key() for r in self.rows}
            key = next((k for k, _, _ in FIELDS if k not in used), "domain_suffix")
        r = CondRow(self, key, list(values), self.simple)
        self.rows.append(r)
        self.rows_box.addWidget(r)
        return r

    def remove_row(self, row):
        self.rows.remove(row)
        row.setParent(None)
        row.deleteLater()

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
        r["match"] = graph_mod.validate_match(self.read())
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
        self.is_if = False
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(8)
        head = QHBoxLayout()
        self.head_title = SubtitleLabel(_t("Rules (checked from the top; the first match wins)"), self)
        head.addWidget(self.head_title)
        head.addStretch(1)
        self.mode = SegmentedWidget(self)
        self.mode.addItem("simple", _t("Simple"), lambda: self.set_mode(False))
        self.mode.addItem("advanced", _t("Advanced"), lambda: self.set_mode(True))
        self.mode.setCurrentItem("simple")
        head.addWidget(self.mode)
        lay.addLayout(head)
        note = CaptionLabel(_t("Inside one rule, any one of the address conditions (domains, IPs, GeoIP, Geosite, private "
                               "network) is enough; port and program must also match. Separate several values with commas. "
                               "Program rules work for connections from this PC. GeoIP and Geosite data is downloaded once a day."), self)
        note.setWordWrap(True)
        lay.addWidget(note)
        self.note = note
        self.list = QVBoxLayout()
        self.list.setSpacing(8)
        lay.addLayout(self.list)
        btns = QHBoxLayout()
        self.b_add = DropDownPushButton(FIF.ADD, _t("Add rule"), self)
        menu = RoundMenu(parent=self.b_add)
        for label, name, match, target in templates():
            act = Action(label, menu)
            act.triggered.connect(lambda _=False, n=name, m=match, t=target: self.add_rule(n, m, t))
            menu.addAction(act)
        self.b_add.setMenu(menu)
        btns.addWidget(self.b_add)
        btns.addStretch(1)
        lay.addLayout(btns)
        default_row = SimpleCardWidget(self)
        dl = QHBoxLayout(default_row)
        dl.setContentsMargins(16, 10, 16, 10)
        col = QVBoxLayout()
        self.default_title = BodyLabel(_t("Default"), default_row)
        col.addWidget(self.default_title)
        self.default_desc = CaptionLabel(_t("Where traffic goes when no rule matches. Required."), default_row)
        col.addWidget(self.default_desc)
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
        self.is_if = router.get("style") == "if"
        self.head_title.setText(_t("Condition") if self.is_if else _t("Rules (checked from the top; the first match wins)"))
        self.b_add.setVisible(not self.is_if)
        self.default_title.setText(_t("else") if self.is_if else _t("Default"))
        self.default_desc.setText(_t("Where traffic goes when the condition does not match. Required.") if self.is_if
                                  else _t("Where traffic goes when no rule matches. Required."))
        self.mode.setCurrentItem("advanced" if MODE["advanced"] else "simple")
        self.note.setVisible(MODE["advanced"])
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

    def set_mode(self, advanced):
        MODE["advanced"] = advanced
        self.note.setVisible(advanced)      # (the simple view shows a short hint in each rule)
        for c in self.cards:
            c.set_mode(advanced)

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

    def add_rule(self, name=None, match=None, target_type=None, pick=True):
        """New rule, optionally from a template; target_type picks the first outbound block of that type."""
        rule = {"id": graph_mod.new_id(), "name": name or _t("Rule {n}").format(n=len(self.cards) + 1),
                "match": copy.deepcopy(match or {}), "target": None}
        if target_type:
            rule["target"] = next((x["id"] for x in self.ctl.graph["outbounds"] if x["outbound"]["type"] == target_type), None)
        card = self._add_card(rule)
        self._renumber()
        if pick and match is not None and "process_name" in match and not match["process_name"]:
            if card.advanced:
                card.set_mode(False)
                self.set_mode(False)
                self.mode.setCurrentItem("simple")
            row = next((r for r in card.rows if r.key() == "process_name"), None) or card.add_row("process_name")
            row.pick_programs()
        else:
            card.ed_name.setFocus()
        return card

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
            if not r["name"] and not self.is_if:
                errs.append(_t("Rule {n}: enter a name").format(n=i + 1))
            if self.is_if and not r["match"]:
                errs.append(_t("IF: add at least one condition"))
            elif self.is_if and not r["target"]:
                errs.append(_t("IF: choose where 'then' goes"))
            elif r["target"] and not r["match"]:
                errs.append(_t("Rule '{name}': add at least one condition").format(name=name))
            errs += match_errors(name, r["match"])
        g = copy.deepcopy(self.ctl.graph)
        g["routers"] = [n if x["id"] == n["id"] else x for x in g.get("routers", [])]
        if graph_mod.router_loop(g, n["id"]):
            errs.append(_t("The routers point at each other in a loop"))
        return errs
