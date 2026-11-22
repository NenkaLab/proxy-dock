"""라이선스 페이지."""
import os
import sys

from PySide6.QtWidgets import QHBoxLayout, QVBoxLayout, QWidget
from qfluentwidgets import CaptionLabel, ComboBox, FluentIcon as FIF, PushButton, TitleLabel

from ..platform_utils import open_path
from .common import LICENSE_DIR, TextView, info, make_mono_font, resource_path

LICENSES = [
    ("개요", "README.txt"),
    ("Pretendard · OFL 1.1", "Pretendard-OFL-1.1.txt"),
    ("JetBrains Mono · OFL 1.1", "JetBrainsMono-OFL-1.1.txt"),
    ("PySide6-Fluent-Widgets · GPL v3", "PySide6-Fluent-Widgets-GPL-3.0.txt"),
    ("Qt 6 · PySide6 · Frameless Window · LGPL v3", "LGPL-3.0.txt"),
    ("aioquic · BSD 3-Clause", "aioquic-BSD-3-Clause.txt"),
    ("pylsqpack · BSD 3-Clause", "pylsqpack-BSD-3-Clause.txt"),
    ("cryptography · Apache 2.0 / BSD (OpenSSL 포함)", "cryptography-Apache-2.0-BSD.txt"),
    ("pyOpenSSL · Apache 2.0", "pyOpenSSL-Apache-2.0.txt"),
    ("service-identity · MIT", "service-identity-MIT.txt"),
    ("attrs · MIT", "attrs-MIT.txt"),
    ("certifi · MPL 2.0", "certifi-MPL-2.0.txt"),
    ("darkdetect · BSD 3-Clause", "darkdetect-BSD-3-Clause.txt"),
    ("Python · PSF", "Python-PSF.txt"),
    ("PyInstaller · GPL v2+ (부트로더 예외)", "PyInstaller.txt"),
]


def license_dir():
    """실행 파일 옆 licenses 폴더가 있으면 그것을, 없으면 내장 리소스를 쓴다."""
    if getattr(sys, "frozen", False):
        beside = os.path.join(os.path.dirname(sys.executable), LICENSE_DIR)
        if os.path.isdir(beside):
            return beside
    return resource_path(LICENSE_DIR)


class LicenseInterface(QWidget):
    def __init__(self, glob, parent=None):
        super().__init__(parent)
        self.setObjectName("licenseInterface")
        title = TitleLabel("라이선스", self)
        self.combo = ComboBox(self)
        self.combo.addItems([name for name, _ in LICENSES])
        self.combo.setFixedWidth(340)
        btn_folder = PushButton("폴더 열기", self, FIF.FOLDER)
        btn_folder.clicked.connect(self._open_dir)
        desc = CaptionLabel("이 프로그램에 포함된 글꼴과 오픈소스 구성요소의 라이선스입니다. "
                            f"같은 내용이 {LICENSE_DIR} 폴더에 텍스트 파일로 들어 있습니다.", self)
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
            text = f"라이선스 파일을 열 수 없습니다: {path}\n{e}"
        self.view.setPlainText(text)
        self.view.verticalScrollBar().setValue(0)

    def _open_dir(self):
        try:
            open_path(license_dir())
        except OSError as e:
            info(self.window(), "error", "폴더를 열 수 없습니다", str(e))
