"""Licenses page."""
from ..i18n import _t
import os
import sys

from PySide6.QtWidgets import QHBoxLayout, QVBoxLayout, QWidget
from qfluentwidgets import CaptionLabel, ComboBox, FluentIcon as FIF, PushButton, TitleLabel

from ..platform_utils import open_path
from .common import LICENSE_DIR, TextView, info, make_mono_font, resource_path

LICENSES = [
    (_t("Overview"), "README.txt"),
    ("Pretendard · OFL 1.1", "Pretendard-OFL-1.1.txt"),
    ("JetBrains Mono · OFL 1.1", "JetBrainsMono-OFL-1.1.txt"),
    ("PySide6-Fluent-Widgets · GPL v3", "PySide6-Fluent-Widgets-GPL-3.0.txt"),
    ("Qt 6 · PySide6 · Frameless Window · LGPL v3", "LGPL-3.0.txt"),
    ("NodeGraphQt · MIT", "NodeGraphQt-MIT.txt"),
    ("Qt.py · MIT", "Qt.py-MIT.txt"),
    ("aioquic · BSD 3-Clause", "aioquic-BSD-3-Clause.txt"),
    ("pylsqpack · BSD 3-Clause", "pylsqpack-BSD-3-Clause.txt"),
    (_t("cryptography · Apache 2.0 / BSD (includes OpenSSL)"), "cryptography-Apache-2.0-BSD.txt"),
    ("pyOpenSSL · Apache 2.0", "pyOpenSSL-Apache-2.0.txt"),
    ("service-identity · MIT", "service-identity-MIT.txt"),
    ("attrs · MIT", "attrs-MIT.txt"),
    ("certifi · MPL 2.0", "certifi-MPL-2.0.txt"),
    ("darkdetect · BSD 3-Clause", "darkdetect-BSD-3-Clause.txt"),
    ("Python · PSF", "Python-PSF.txt"),
    (_t("PyInstaller · GPL v2+ (bootloader exception)"), "PyInstaller.txt"),
]


def license_dir():
    """Use the licenses folder next to the executable if present, otherwise the bundled resources."""
    if getattr(sys, "frozen", False):
        beside = os.path.join(os.path.dirname(sys.executable), LICENSE_DIR)
        if os.path.isdir(beside):
            return beside
    return resource_path(LICENSE_DIR)


class LicenseInterface(QWidget):
    def __init__(self, glob, parent=None):
        super().__init__(parent)
        self.setObjectName("licenseInterface")
        title = TitleLabel(_t("Licenses"), self)
        self.combo = ComboBox(self)
        self.combo.addItems([name for name, _ in LICENSES])
        self.combo.setFixedWidth(340)
        btn_folder = PushButton(_t("Open folder"), self, FIF.FOLDER)
        btn_folder.clicked.connect(self._open_dir)
        desc = CaptionLabel(_t("Licenses of the fonts and open source components included in this program. The same texts are in the {LICENSE_DIR} folder.").format(LICENSE_DIR=LICENSE_DIR), self)
        desc.setWordWrap(True)
        self.view = TextView(self)
        self.view.setFont(make_mono_font(glob))
        header = QHBoxLayout()
        header.setSpacing(8)
        header.addWidget(title)
        header.addStretch(1)
        header.addWidget(self.combo)
        header.addWidget(btn_folder)
        root = QVBoxLayout(self)
        root.setContentsMargins(36, 20, 36, 28)
        root.setSpacing(10)
        root.addLayout(header)
        root.addWidget(desc)
        root.addWidget(self.view, 1)
        self.combo.currentIndexChanged.connect(self._show)
        self._show(0)

    def _show(self, index):
        path = os.path.join(license_dir(), LICENSES[max(index, 0)][1])
        try:
            with open(path, encoding="utf-8") as f:
                text = f.read()
        except OSError as e:
            text = _t("Cannot open the license file: {path}\n{e}").format(path=path, e=e)
        self.view.setPlainText(text)
        self.view.verticalScrollBar().setValue(0)

    def _open_dir(self):
        try:
            open_path(license_dir())
        except OSError as e:
            info(self.window(), "error", _t("Cannot open the folder"), str(e))
