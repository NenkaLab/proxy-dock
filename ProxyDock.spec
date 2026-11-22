# -*- mode: python ; coding: utf-8 -*-
# tools/build.py 가 이 spec 으로 빌드한다 (옵션은 여기서만 관리).
#  - Windows, Linux : 단일 실행 파일 (dist/<os>/ProxyDock[.exe])
#  - macOS          : 앱 번들 (dist/mac/ProxyDock.app)
# 출력 폴더는 build.py 가 --distpath 로 정한다.
import sys
from PyInstaller.utils.hooks import collect_data_files, collect_submodules

IS_MAC = sys.platform == "darwin"

# PySide6 전체를 끌어오지 않도록 쓰지 않는 Qt 모듈은 제외한다 (용량/시작 시간 감소)
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

DATAS = collect_data_files('qfluentwidgets') + collect_data_files('certifi') + [
    ('fonts/PretendardVariable.ttf', 'fonts'), ('fonts/JetBrainsMono.ttf', 'fonts'),
    ('licenses/*.txt', 'licenses'),
]

a = Analysis(
    ['proxydock.py'],
    pathex=[],
    binaries=[],
    datas=DATAS,
    hiddenimports=collect_submodules('proxydock') + collect_submodules('aioquic'),
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
