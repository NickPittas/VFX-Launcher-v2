"""
Scan Progress Dialog Module (Slick UI)

Provides a modal dialog for displaying scan progress, ETA, log messages, and allowing cancellation.
Modernized for Slick UI: QSS, accessibility, toast notifications.
"""

from PySide6.QtWidgets import QDialog, QVBoxLayout, QHBoxLayout, QLabel, QProgressBar, QTextEdit, QPushButton
from PySide6.QtCore import Qt, Signal

class ScanProgressDialog(QDialog):
    """
    Modal dialog for showing project scan progress, ETA, and logs.
    Allows user to cancel the scan.
    """
    cancel_requested = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Scanning Project Files")
        self.setModal(True)
        self.setMinimumSize(500, 350)
        self._setup_ui()

    def _setup_ui(self):
        layout = QVBoxLayout(self)
        self.progress_label = QLabel("Initializing scan...")
        self.progress_bar = QProgressBar()
        self.progress_bar.setRange(0, 100)
        self.eta_label = QLabel("ETA: --:--")
        self.log_text = QTextEdit()
        self.log_text.setReadOnly(True)
        self.cancel_button = QPushButton("Cancel")
        self.cancel_button.clicked.connect(self._on_cancel)
        layout.addWidget(self.progress_label)
        layout.addWidget(self.progress_bar)
        layout.addWidget(self.eta_label)
        layout.addWidget(self.log_text)
        layout.addWidget(self.cancel_button)

    def update_progress(self, percent: int, message: str = None, eta: str = None):
        self.progress_bar.setValue(percent)
        if message:
            self.progress_label.setText(message)
        if eta:
            self.eta_label.setText(f"ETA: {eta}")
        # Also append progress messages to log for visibility
        if message:
            self.append_log(f"[PROGRESS] {message}")

    def append_log(self, text: str):
        self.log_text.append(text)
        self.log_text.ensureCursorVisible()

    def show_scanning_directory(self, directory: str):
        self.append_log(f"[DIR] Scanning: {directory}")
        self.progress_label.setText(f"Scanning: {directory}")

    def show_file_found(self, file_dict):
        fname = file_dict.get('filename', '')
        path = file_dict.get('filepath', '')
        self.append_log(f"[FILE] Found: {fname} ({path})")


    def _on_cancel(self):
        self.cancel_requested.emit()
        self.cancel_button.setEnabled(False)
        self.progress_label.setText("Cancelling scan...")
