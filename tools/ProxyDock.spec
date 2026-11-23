# -*- mode: python ; coding: utf-8 -*-
# tools/build.py builds with this spec (options live only here).
#  - Windows, Linux : folder (dist/<os>/ProxyDock/ProxyDock[.exe] + _internal/). Not onefile: onefile unpacks
#                     ~280 MB to the temp folder on every start and leaves it there when the app is killed.
#  - macOS          : app bundle (dist/mac/ProxyDock.app)
# build.py sets the output folder with --distpath.
import os
import sys
from PyInstaller.utils.hooks import collect_data_files, collect_submodules

IS_MAC = sys.platform == "darwin"
ROOT = os.path.dirname(SPECPATH)      # this spec lives in tools/; sources are relative to the repo root


def src(*parts):
    return os.path.join(ROOT, *parts)


def app_version():
    import re
    with open(src('proxydock', '__init__.py'), encoding='utf-8') as f:
        return re.search(r'__version__ = "([^"]+)"', f.read()).group(1)


def windows_version_info():
    """File properties of ProxyDock.exe (Details tab)."""
    from PyInstaller.utils.win32.versioninfo import (
        FixedFileInfo, StringFileInfo, StringStruct, StringTable, VarFileInfo, VarStruct, VSVersionInfo)
    ver = app_version()
    nums = tuple((list(map(int, ver.split('-')[0].split('.'))) + [0, 0, 0, 0])[:4])
    return VSVersionInfo(
        ffi=FixedFileInfo(filevers=nums, prodvers=nums),
        kids=[StringFileInfo([StringTable('040904B0', [
            StringStruct('CompanyName', 'NenkaLab'),
            StringStruct('FileDescription', 'ProxyDock'),
            StringStruct('FileVersion', ver),
            StringStruct('InternalName', 'ProxyDock'),
            StringStruct('LegalCopyright', 'Copyright (C) 2026 NenkaLab - GPL-3.0'),
            StringStruct('OriginalFilename', 'ProxyDock.exe'),
            StringStruct('ProductName', 'ProxyDock'),
            StringStruct('ProductVersion', ver)])]),
              VarFileInfo([VarStruct('Translation', [0x0409, 1200])])])

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
    (src('fonts', 'PretendardJPVariable.ttf'), 'fonts'),
    (src('fonts', 'NotoSansSC.ttf'), 'fonts'), (src('fonts', 'NotoSansTC.ttf'), 'fonts'),
    (src('licenses', '*.txt'), 'licenses'), (src('proxydock', 'locales', '*.json'), 'proxydock/locales'),
]

def engine_binaries():
    """sing-box and the official Hysteria2 core for this OS/arch (bin/<os>-<arch>/, fetched by tools/build.py),
    placed at the same relative path inside the app so proxydock.singbox finds them."""
    sys.path.insert(0, ROOT)
    from proxydock.singbox import EXE, HY_EXE, platform_dir
    rel = os.path.join('bin', platform_dir())
    found = [(src(rel, name), rel) for name in (EXE, HY_EXE) if os.path.isfile(src(rel, name))]
    if len(found) != 2:
        raise SystemExit(f'{rel}: sing-box / hysteria missing - run python tools/fetch_singbox.py')
    return found


a = Analysis(
    [src('proxydock.py')],
    pathex=[ROOT],
    binaries=engine_binaries(),
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
            'CFBundleShortVersionString': app_version(),
            'CFBundleVersion': app_version(),
            'NSHighResolutionCapable': True,
            'LSMinimumSystemVersion': '12.0',
        },
    )
else:
    exe = EXE(
        pyz, a.scripts, [],
        exclude_binaries=True,
        name='ProxyDock',
        debug=False,
        bootloader_ignore_signals=False,
        strip=False,
        upx=False,
        console=False,
        disable_windowed_traceback=False,
        argv_emulation=False,
        target_arch=None,
        codesign_identity=None,
        entitlements_file=None,
        version=windows_version_info() if sys.platform == 'win32' else None,
    )
    coll = COLLECT(exe, a.binaries, a.datas, strip=False, upx=False, upx_exclude=[], name='ProxyDock')
