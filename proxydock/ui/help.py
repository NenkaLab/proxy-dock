"""How-to page: proxydock/locales/help/<language>.md (English when a language has none), shown as Markdown."""
import os

from PySide6.QtCore import Qt
from PySide6.QtGui import QColor, QPalette, QTextCharFormat, QTextCursor, QTextFrameFormat, QTextTable
from PySide6.QtWidgets import QFrame, QTextBrowser, QVBoxLayout, QWidget
from qfluentwidgets import TitleLabel, isDarkTheme, qconfig

from .. import i18n
from ..i18n import _t


def help_markdown(code=None):
    """The how-to text (Markdown) for language code (default: the current one)."""
    folder = os.path.join(i18n.locale_dir(), "help")
    for c in (code or i18n.current(), i18n.DEFAULT_LANGUAGE):
        try:
            with open(os.path.join(folder, f"{c}.md"), encoding="utf-8") as f:
                return f.read()
        except OSError:
            continue
    return ""


class HelpInterface(QWidget):
    def __init__(self, ctl, parent=None):
        super().__init__(parent)
        self.setObjectName("helpInterface")
        root = QVBoxLayout(self)
        root.setContentsMargins(36, 20, 36, 12)
        root.setSpacing(8)
        root.addWidget(TitleLabel(_t("How to use"), self))
        self.view = QTextBrowser(self)
        self.view.setFrameShape(QFrame.NoFrame)
        self.view.setOpenExternalLinks(True)
        self.view.setHorizontalScrollBarPolicy(Qt.ScrollBarAlwaysOff)
        self.view.viewport().setAutoFillBackground(False)
        self.view.setStyleSheet("QTextBrowser { background: transparent; border: none; }")
        from qfluentwidgets.components.widgets.scroll_bar import SmoothScrollDelegate
        self._scroll = SmoothScrollDelegate(self.view)          # (Fluent scroll bars)
        root.addWidget(self.view, 1)
        self._render()
        qconfig.themeChanged.connect(lambda *_: self._render())

    def _render(self):
        from qfluentwidgets.common import font as qf_font    # (the module's getFont follows the text size setting)
        dark = isDarkTheme()
        text = QColor(235, 235, 235) if dark else QColor(28, 28, 28)
        pal = self.view.palette()
        pal.setColor(QPalette.Text, text)
        pal.setColor(QPalette.Link, QColor(108, 203, 255) if dark else QColor(0, 95, 184))
        self.view.setPalette(pal)
        base = qf_font.getFont(15)
        self.view.setFont(base)
        doc = self.view.document()
        doc.setDocumentMargin(2)
        doc.setMarkdown(help_markdown())
        self._tidy(doc, base, QColor(255, 255, 255, 40) if dark else QColor(0, 0, 0, 30))
        self.view.moveCursor(QTextCursor.Start)
        self.view.verticalScrollBar().setValue(0)

    @staticmethod
    def _tidy(doc, base, line):
        """Headings with room above them, list items a little apart, tables with light rules only."""
        cur = QTextCursor(doc)
        block = doc.begin()
        while block.isValid():
            cur.setPosition(block.position())
            fmt = block.blockFormat()
            if fmt.headingLevel():
                fmt.setTopMargin(0 if block == doc.begin() else 22)
                fmt.setBottomMargin(6)
                cur.setBlockFormat(fmt)
                cur.select(QTextCursor.LineUnderCursor)
                ch = QTextCharFormat()
                ch.setFontPointSize(base.pointSizeF() * 1.3 if base.pointSizeF() > 0 else 0)
                if base.pixelSize() > 0:
                    ch.setProperty(QTextCharFormat.FontPixelSize, round(base.pixelSize() * 1.3))
                ch.setFontWeight(600)
                cur.mergeCharFormat(ch)
            elif block.textList() is not None:
                fmt.setBottomMargin(4)
                cur.setBlockFormat(fmt)
            block = block.next()
        for frame in doc.rootFrame().childFrames():
            if isinstance(frame, QTextTable):
                tf = frame.format()
                tf.setBorder(0)
                tf.setBorderStyle(QTextFrameFormat.BorderStyle_None)
                tf.setCellSpacing(0)
                tf.setCellPadding(7)
                tf.setTopMargin(6)
                frame.setFormat(tf)
                for r in range(frame.rows()):
                    for c in range(frame.columns()):
                        cell = frame.cellAt(r, c)
                        cf = cell.format().toTableCellFormat()
                        cf.setBottomBorder(1)
                        cf.setBottomBorderBrush(line)
                        cf.setBottomBorderStyle(QTextFrameFormat.BorderStyle_Solid)
                        cf.setRightPadding(28)
                        cell.setFormat(cf)

    def set_firewall_status(self, state):
        """(The firewall rule lives on the Settings page.)"""
