"""Qt file/folder pickers for the Slick UI.

Why a wrapper: on Linux, QFileDialog's default (native) path defers to a
platform/portal dialog helper. On some setups (KDE + AppImage/PyInstaller,
no matching platformtheme plugin) that helper never surfaces a window: the
call blocks on a blank Qt shell with no widgets (verified 2026-09-02 —
0 child widgets, uniform-color capture). Forcing DontUseNativeDialog makes
Qt compose its own fully-functional widgets dialog, identical in dev and
frozen builds, and consistent with the app stylesheet.

All browse buttons must go through these helpers instead of calling
QFileDialog directly.
"""
from PySide6.QtWidgets import QFileDialog


def pick_directory(parent=None, title="Select Folder", start_dir=""):
    """Folder picker. Returns the chosen absolute path or '' if cancelled."""
    options = QFileDialog.Option.DontUseNativeDialog | QFileDialog.Option.ShowDirsOnly
    return QFileDialog.getExistingDirectory(parent, title, start_dir, options)


def pick_open_file(parent=None, title="Select File", start_dir="", name_filter="All Files (*)"):
    """Single-file picker. Returns the chosen path or '' if cancelled."""
    options = QFileDialog.Option.DontUseNativeDialog
    path, _ = QFileDialog.getOpenFileName(parent, title, start_dir, name_filter, options=options)
    return path
