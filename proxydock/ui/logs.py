"""로그 페이지: 프록시 드롭다운 + 종류 필터 + 검색."""
from collections import deque

from PySide6.QtCore import QTimer
from PySide6.QtGui import QColor, QTextCharFormat, QTextCursor
from PySide6.QtWidgets import QApplication, QHBoxLayout, QVBoxLayout, QWidget
from qfluentwidgets import ComboBox, FluentIcon as FIF, PushButton, SearchLineEdit, TitleLabel, isDarkTheme, qconfig

from ..common import LEVEL_TAGS, MAX_LOG, format_line
from .common import LEVEL_FILTERS, LOG_COLORS_DARK, LOG_COLORS_LIGHT, ProxyCombo, TextView, info, make_mono_font


class LogInterface(QWidget):
    def __init__(self, glob, parent=None):
        super().__init__(parent)
        self.setObjectName("logInterface")
        self.entries = deque(maxlen=MAX_LOG * 4)     # 프록시별로 걸러 봐도 충분하도록 여유 있게
        self.dirty = False
        self._fmt_dark = None
        self._fmts = {}

        self.title = TitleLabel("로그", self)
        self.proxy = ProxyCombo(self, "전체 (앱 + 모든 프록시)", 230)
        self.search = SearchLineEdit(self)
        self.search.setPlaceholderText("검색")
        self.search.setFixedWidth(200)
        self.combo = ComboBox(self)
        self.combo.addItems([name for name, _ in LEVEL_FILTERS])
        self.combo.setFixedWidth(150)
        self.btn_clear = PushButton("지우기", self, FIF.DELETE)
        self.btn_copy = PushButton("복사", self, FIF.COPY)

        self.view = TextView(self)
        self.view.setMaximumBlockCount(MAX_LOG)
        self.apply_font(glob)

        header = QHBoxLayout()
        header.setSpacing(8)
        header.addWidget(self.title)
        header.addStretch(1)
        header.addWidget(self.proxy)
        header.addWidget(self.combo)
        header.addWidget(self.search)
        header.addWidget(self.btn_clear)
        header.addWidget(self.btn_copy)

        root = QVBoxLayout(self)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(14)
        root.addLayout(header)
        root.addWidget(self.view, 1)

        self._search_timer = QTimer(self)
        self._search_timer.setSingleShot(True)
        self._search_timer.setInterval(200)
        self._search_timer.timeout.connect(self.render)
        self.search.textChanged.connect(lambda *_: self._search_timer.start())
        self.combo.currentIndexChanged.connect(self.render)
        self.proxy.currentIndexChanged.connect(self.render)
        self.btn_clear.clicked.connect(self.clear)
        self.btn_copy.clicked.connect(self.copy_all)
        qconfig.themeChanged.connect(lambda *_: self.render())
        self.render()

    def set_proxies(self, proxies):
        self.proxy.set_proxies(proxies)

    def apply_font(self, glob):
        self.view.setFont(make_mono_font(glob))

    def _formats(self):
        dark = isDarkTheme()
        if dark != self._fmt_dark:
            colors = LOG_COLORS_DARK if dark else LOG_COLORS_LIGHT
            self._fmts = {}
            for level, color in colors.items():
                f = QTextCharFormat()
                f.setForeground(QColor(color))
                self._fmts[level] = f
            self._fmt_dark = dark
        return self._fmts

    def _matcher(self):
        want = LEVEL_FILTERS[self.combo.currentIndex()][1]
        pid = self.proxy.current_id() or ""
        q = self.search.text().strip().lower()
        return lambda e: (not pid or e[3] == pid) and (want is None or e[1] in want) and (
            not q or q in e[2].lower() or q == LEVEL_TAGS.get(e[1], "").lower() or q in e[4].lower())

    def _append(self, entries):
        if not entries:
            return
        doc = self.view.document()
        fmts = self._formats()
        default = QTextCharFormat()
        with_name = not self.proxy.current_id()
        cur = QTextCursor(doc)
        cur.movePosition(QTextCursor.End)
        cur.beginEditBlock()
        first = doc.isEmpty()
        for e in entries:
            if not first:
                cur.insertBlock()
            first = False
            cur.insertText(format_line(e, with_name), fmts.get(e[1], default))
        cur.endEditBlock()

    def render(self, *_):
        if not self.isVisible():
            self.dirty = True
            return
        self.dirty = False
        self.view.clear()
        m = self._matcher()
        self._append([e for e in self.entries if m(e)][-MAX_LOG:])
        sb = self.view.verticalScrollBar()
        sb.setValue(sb.maximum())

    def add_entries(self, batch):
        self.entries.extend(batch)
        if not self.isVisible():
            self.dirty = True   # 숨겨져 있으면 보일 때 한 번에 그린다
            return
        m = self._matcher()
        shown = [e for e in batch if m(e)]
        if not shown:
            return
        sb = self.view.verticalScrollBar()
        at_end = sb.value() >= sb.maximum() - 4
        self._append(shown[-MAX_LOG:])
        if at_end:
            sb.setValue(sb.maximum())

    def showEvent(self, event):
        super().showEvent(event)
        if self.dirty:
            self.render()

    def clear(self):
        pid = self.proxy.current_id()
        if pid:
            kept = [e for e in self.entries if e[3] != pid]
            self.entries.clear()
            self.entries.extend(kept)
        else:
            self.entries.clear()
        self.view.clear()

    def copy_all(self):
        QApplication.clipboard().setText(self.view.toPlainText())
        info(self.window(), "success", "복사됨", "표시된 로그를 클립보드에 복사했습니다.", 2000)
