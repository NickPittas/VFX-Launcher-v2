# -*- mode: python ; coding: utf-8 -*-

block_cipher = None

import os
import sys
from pathlib import Path

# Get the project root directory
project_root = os.path.abspath(SPECPATH)

a = Analysis(
    ['ui_slick/main_slick.py'],
    pathex=[project_root],
    binaries=[],
    datas=[
        # Include icons folder (contains all icons including star icons)
        ('ui_slick/icons', 'ui_slick/icons'),
        # Include QSS stylesheet
        ('ui_slick/styles.qss', 'ui_slick'),
    ],
    hiddenimports=[
        'PySide6.QtCore',
        'PySide6.QtGui',
        'PySide6.QtWidgets',
        'PySide6.QtSvg',
        'watchdog',
        'watchdog.observers',
        'watchdog.events',
        'sqlite3',
    ],
    hookspath=[],
    hooksconfig={},
    runtime_hooks=[],
    excludes=[],
    win_no_prefer_redirects=False,
    win_private_assemblies=False,
    cipher=block_cipher,
    noarchive=False,
)

pyz = PYZ(a.pure, a.zipped_data, cipher=block_cipher)

# Create a one-file executable to avoid decompression issues
exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.zipfiles,
    a.datas,
    [],
    name='VFX_Launcher',
    debug=False,
    bootloader_ignore_signals=False,
    strip=False,
    upx=False,  # Disable UPX compression to avoid decompression errors
    upx_exclude=[],
    runtime_tmpdir=None,
    console=False,  # Set to False for GUI app (no console window)
    disable_windowed_traceback=False,
    argv_emulation=False,
    target_arch=None,
    codesign_identity=None,
    entitlements_file=None,
    icon='ui_slick/icons/icon_3.ico',  # Application icon (ICO format for Windows)
)

