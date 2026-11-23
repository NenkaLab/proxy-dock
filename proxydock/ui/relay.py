"""Pass-through block editor: lane count, empty rows (gaps) and, for a routed block, which output each input goes to.

Shown inside the editor page (ui/proxies.py EditorInterface) when a pass-through block is edited. The same wiring
can also be changed on the canvas (links on the outside, dragging inside a routed block).
"""
import copy

from PySide6.QtCore import Qt
from PySide6.QtWidgets import QHBoxLayout, QVBoxLayout, QWidget
from qfluentwidgets import BodyLabel, CaptionLabel, ComboBox, SimpleCardWidget, SpinBox, SubtitleLabel

from .. import graph as graph_mod
from ..i18n import _t


class LaneRow(SimpleCardWidget):
    """One lane: title + sources on the left, gap / inside wiring / target on the right."""

    def __init__(self, title, desc, parent=None):
        super().__init__(parent)
        col = QVBoxLayout()
        col.setSpacing(1)
        col.addWidget(BodyLabel(title, self))
        self.desc = CaptionLabel(desc, self)
        self.desc.setWordWrap(True)
        self.desc.setVisible(bool(desc))
        col.addWidget(self.desc)
        self.hl = QHBoxLayout(self)
        self.hl.setContentsMargins(16, 8, 16, 8)
        self.hl.setSpacing(10)
        self.hl.addLayout(col, 1)
        self.gap = SpinBox(self)
        self.gap.setRange(0, graph_mod.MAX_GAP)
        self.gap.setFixedWidth(120)
        self.gap.setToolTip(_t("Empty rows above this lane on the canvas"))
        self.hl.addWidget(CaptionLabel(_t("Gap"), self), 0, Qt.AlignVCenter)
        self.hl.addWidget(self.gap, 0, Qt.AlignVCenter)
        self.combo = ComboBox(self)
        self.combo.setFixedWidth(260)
        self.hl.addWidget(self.combo, 0, Qt.AlignVCenter)
        self.ids = []

    def set_choices(self, ids, labels, current):
        self.ids = ids
        self.combo.blockSignals(True)
        self.combo.clear()
        self.combo.addItems(labels)
        self.combo.setCurrentIndex(ids.index(current) if current in ids else 0)
        self.combo.blockSignals(False)

    def value(self):
        i = self.combo.currentIndex()
        return self.ids[i] if 0 <= i < len(self.ids) else None


class RelaySection(QWidget):
    """Lanes of a pass-through block."""

    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        self.ctl = ctl
        self.relay = None
        self.in_rows, self.out_rows = [], []
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(8)
        lay.addWidget(SubtitleLabel(_t("Lanes"), self))
        note = CaptionLabel(_t("A pass-through only tidies up the wiring: what goes into a lane comes out of the lane it "
                               "is wired to, with no cost at run time. Straight: lane 1 → lane 1, lane 2 → lane 2, … "
                               "Routed: wire each input to any output (several inputs may share one output)."), self)
        note.setWordWrap(True)
        lay.addWidget(note)
        head = SimpleCardWidget(self)
        hl = QHBoxLayout(head)
        hl.setContentsMargins(16, 10, 16, 10)
        hl.setSpacing(10)
        hl.addWidget(BodyLabel(_t("Wiring"), head), 1)
        self.cb_mode = ComboBox(head)
        self.cb_mode.addItems([_t("Straight"), _t("Routed (wire lanes freely)")])
        self.cb_mode.setFixedWidth(240)
        hl.addWidget(self.cb_mode)
        hl.addWidget(CaptionLabel(_t("Inputs"), head))
        self.sp_in = SpinBox(head)
        self.sp_in.setRange(1, graph_mod.MAX_LANES)
        self.sp_in.setFixedWidth(120)
        hl.addWidget(self.sp_in)
        self.lb_out = CaptionLabel(_t("Outputs"), head)
        hl.addWidget(self.lb_out)
        self.sp_out = SpinBox(head)
        self.sp_out.setRange(1, graph_mod.MAX_LANES)
        self.sp_out.setFixedWidth(120)
        hl.addWidget(self.sp_out)
        lay.addWidget(head)
        lay.addWidget(SubtitleLabel(_t("Inputs"), self))
        self.in_box = QVBoxLayout()
        self.in_box.setSpacing(6)
        lay.addLayout(self.in_box)
        self.out_title = SubtitleLabel(_t("Outputs"), self)
        lay.addWidget(self.out_title)
        self.out_box = QVBoxLayout()
        self.out_box.setSpacing(6)
        lay.addLayout(self.out_box)
        self.cb_mode.currentIndexChanged.connect(lambda *_: self._reshape())
        self.sp_in.valueChanged.connect(lambda *_: self._reshape())
        self.sp_out.valueChanged.connect(lambda *_: self._reshape())

    # --- targets an output lane can lead to: outbound blocks, routers and other blocks' lanes ---
    def _targets(self):
        g = self.ctl.graph
        own = {x["id"] for x in self.relay["inputs"]}
        lanes = [(i, label) for i, label in graph_mod.lane_choices(g) if i not in own]
        ids = [None] + [x["id"] for x in g["outbounds"]] + [x["id"] for x in g.get("routers", [])] + [i for i, _ in lanes]
        labels = [_t("(not connected)")] + [x["name"] for x in g["outbounds"]] + \
            [(_t("IF {name}") if x.get("style") == "if" else _t("Router {name}")).format(name=x["name"])
             for x in g.get("routers", [])] + [label for _, label in lanes]
        return ids, labels

    def load(self, relay):
        self.relay = copy.deepcopy(relay)
        for w in (self.cb_mode, self.sp_in, self.sp_out):
            w.blockSignals(True)
        self.cb_mode.setCurrentIndex(1 if relay["routed"] else 0)
        self.sp_in.setValue(len(relay["inputs"]))
        self.sp_out.setValue(len(relay["outputs"]))
        for w in (self.cb_mode, self.sp_in, self.sp_out):
            w.blockSignals(False)
        self._build()

    def _read(self):
        """Widgets -> self.relay (gaps, inside wiring, targets)."""
        r = self.relay
        for row, x in zip(self.in_rows, r["inputs"]):
            x["gap"] = row.gap.value()
            if r["routed"]:
                x["out"] = row.value()
        for row, x in zip(self.out_rows, r["outputs"]):
            if r["routed"]:
                x["gap"] = row.gap.value()
            x["target"] = row.value()

    def _reshape(self):
        self._read()
        r = self.relay
        routed = self.cb_mode.currentIndex() == 1
        if routed != r["routed"]:
            r["routed"] = routed
            if not routed:          # back to straight: one output per input, wired in order
                r["outputs"] = r["outputs"][:len(r["inputs"])]
        graph_mod.set_lane_count(r, "inputs", self.sp_in.value())
        if routed:
            graph_mod.set_lane_count(r, "outputs", self.sp_out.value())
        self.relay = graph_mod.validate_relay_node(r)
        self.sp_out.blockSignals(True)
        self.sp_out.setValue(len(self.relay["outputs"]))
        self.sp_out.blockSignals(False)
        self._build()

    def _build(self):
        for row in self.in_rows + self.out_rows:
            row.setParent(None)
            row.deleteLater()
        self.in_rows, self.out_rows = [], []
        r, g = self.relay, self.ctl.graph
        routed = r["routed"]
        self.lb_out.setVisible(routed)
        self.sp_out.setVisible(routed)
        out_ids = [None] + [x["id"] for x in r["outputs"]]
        out_labels = [_t("(not wired)")] + [_t("→ output {n}").format(n=i + 1) for i in range(len(r["outputs"]))]
        for i, x in enumerate(r["inputs"]):
            src = graph_mod.lane_sources(g, x["id"])
            row = LaneRow(_t("Input {n}").format(n=i + 1),
                          _t("from: {names}").format(names=", ".join(src)) if src else _t("nothing linked yet"), self)
            row.gap.setValue(x["gap"])
            row.set_choices(out_ids, out_labels, x["out"])
            row.combo.setVisible(routed)
            self.in_box.addWidget(row)
            self.in_rows.append(row)
        t_ids, t_labels = self._targets()
        for i, x in enumerate(r["outputs"]):
            row = LaneRow(_t("Output {n}").format(n=i + 1), "", self)
            row.gap.setValue(x["gap"])
            row.gap.setVisible(routed)          # straight: the output side follows the input gaps
            row.set_choices(t_ids, t_labels, x["target"])
            self.out_box.addWidget(row)
            self.out_rows.append(row)

    def collect(self, node):
        self._read()
        n = copy.deepcopy(self.relay)
        n["name"] = node["name"]
        out = graph_mod.validate_relay_node(n)
        out["name"] = n["name"]             # keep an empty name so the error check can report it
        return out

    def errors(self, n):
        errs = [] if n["name"] else [_t("Enter a name")]
        g = copy.deepcopy(self.ctl.graph)
        g["relays"] = [n if x["id"] == n["id"] else x for x in g.get("relays", [])]
        if any(x["target"] and graph_mod.lane_loop(g, x["target"]) for x in n["outputs"]) or \
                any(graph_mod.router_loop(g, x["id"]) for x in g.get("routers", [])):
            errs.append(_t("The links would go round in a circle"))
        return errs
