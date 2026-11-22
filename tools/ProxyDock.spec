# -*- mode: python ; coding: utf-8 -*-
# tools/build.py builds with this spec (options live only here).
#  - Windows, Linux : single executable (dist/<os>/ProxyDock[.exe])
#  - macOS          : app bundle (dist/mac/ProxyDock.app)
# build.py sets the output folder with --distpath.
import os
import sys
from PyInstaller.utils.hooks import collect_data_files, collect_submodules

IS_MAC = sys.platform == "darwin"
ROOT = os.path.dirname(SPECPATH)      # this spec lives in tools/; sources are relative to the repo root


def src(*parts):
    return os.path.join(ROOT, *parts)

# exclude unused Qt modules so not all of PySide6 is pulled in (smaller size, faster startup)
EXCLUDES = [
    'PySide6.QtWebEngineCore', 'PySide6.QtWebEngineWidgets', 'PySide6.QtWebEngineQuick',
    'PySide6.QtWebChannel', 'PySide6.QtQuick', 'PySide6.QtQml', 'PySide6.QtQuickWidgets',
    'PySide6.Qt3DCore', 'PySide6.QtCharts', 'PySide6.QtDataVisualization',
    'PySide6.QtMultimedia', 'PySide6.QtMultimediaWidgets', 'PySide6.QtPdf',
    'PySide6.QtPdfWidgets', 'PySide6.QtSql', 'PySide6.QtTest', 'PySide6.QtDesigner',
    'PySide6.QtBluetooth', 'PySide6.QtPositioning', 'PySide6.QtLocation',
    'PySide6.QtSensors', 'PySide6.QtOpenGL', 'PySide6.QtHelp',
    'tkinter', 'scipy', 'numpy',
]

DATAS = collect_data_files('qfluentwidgets') + collect_data_files('certifi') + collect_data_files('NodeGraphQt') + [
    (src('fonts', 'PretendardVariable.ttf'), 'fonts'), (src('fonts', 'JetBrainsMono.ttf'), 'fonts'),
    (src('licenses', '*.txt'), 'licenses'), (src('proxydock', 'locales', '*.json'), 'proxydock/locales'),
]

a = Analysis(
    [src('proxydock.py')],
    pathex=[ROOT],
    binaries=[],
    datas=DATAS,
    hiddenimports=collect_submodules('proxydock') + collect_submodules('aioquic') + collect_submodules('NodeGraphQt') + ['Qt'],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=EXCLUDES,
    noarchive=False,
    optimize=0,
)
pyz = PYZ(a.pure)

if IS_MAC:
    exe = EXE(pyz, a.scripts, [], exclude_binaries=True, name='ProxyDock', debug=False, strip=False,
              upx=False, console=False, argv_emulation=False, target_arch=None,
              codesign_identity=None, entitlements_file=None)
    coll = COLLECT(exe, a.binaries, a.datas, strip=False, upx=False, name='ProxyDock')
    app = BUNDLE(
        coll,
        name='ProxyDock.app',
        icon=None,
        bundle_identifier='com.proxydock.app',
        info_plist={
            'CFBundleName': 'ProxyDock',
            'CFBundleDisplayName': 'ProxyDock',
            'NSHighResolutionCapable': True,
            'LSMinimumSystemVersion': '12.0',
        },
    )
else:
    exe = EXE(
        pyz, a.scripts, a.binaries, a.datas, [],
        name='ProxyDock',
        debug=False,
        bootloader_ignore_signals=False,
        strip=False,
        upx=False,
        upx_exclude=[],
        runtime_tmpdir=None,
        console=False,
        disable_windowed_traceback=False,
        argv_emulation=False,
        target_arch=None,
        codesign_identity=None,
        entitlements_file=None,
    )
