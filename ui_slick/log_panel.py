"""
Modern Log Panel for Slick UI (restored original behavior)
Displays real-time logs from the Python logging module, with filter, clear, and auto-scroll features.
No database or async worker dependencies.
"""
import logging
from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QTextEdit, QPushButton, QComboBox, QLabel, QCheckBox
)
from PySide6.QtCore import Qt, Signal, Slot, QObject
from PySide6.QtGui import QTextCursor, QColor, QTextCharFormat, QFont
import logging

class LogHandler(QObject, logging.Handler):
    new_log_record = Signal(object)
    def __init__(self, parent=None):
        QObject.__init__(self, parent)
        logging.Handler.__init__(self)
        self.setFormatter(logging.Formatter('%(asctime)s - %(name)s - %(levelname)s - %(message)s'))
    def emit(self, record):
        self.new_log_record.emit(record)

class LogPanel(QWidget):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setObjectName("LogPanel")
        self.log_handler = LogHandler(self)
        self.log_handler.new_log_record.connect(self._on_new_log_record)
        root_logger = logging.getLogger()
        root_logger.addHandler(self.log_handler)
        self.log_colors = {
            logging.DEBUG: QColor(100, 100, 100),
            logging.INFO: QColor(0, 0, 0),
            logging.WARNING: QColor(255, 165, 0),
            logging.ERROR: QColor(255, 0, 0),
            logging.CRITICAL: QColor(128, 0, 128)
        }
        self.auto_scroll = True
        self.filter_level = logging.INFO
        self.max_log_lines = 1000
        self._setup_ui()

    def _setup_ui(self):
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(10)
        # Controls
        controls_layout = QHBoxLayout()
        # Level filter
        level_layout = QHBoxLayout()
        level_label = QLabel("Log Level:")
        self.level_combo = QComboBox()
        self.level_combo.addItem("DEBUG", logging.DEBUG)
        self.level_combo.addItem("INFO", logging.INFO)
        self.level_combo.addItem("WARNING", logging.WARNING)
        self.level_combo.addItem("ERROR", logging.ERROR)
        self.level_combo.addItem("CRITICAL", logging.CRITICAL)
        self.level_combo.setCurrentIndex(1)  # INFO by default
        self.level_combo.currentIndexChanged.connect(self._on_level_changed)
        level_layout.addWidget(level_label)
        level_layout.addWidget(self.level_combo)
        # Auto-scroll checkbox
        self.auto_scroll_check = QCheckBox("Auto-scroll")
        self.auto_scroll_check.setChecked(True)
        self.auto_scroll_check.toggled.connect(self._on_auto_scroll_toggled)
        # Clear button
        self.clear_button = QPushButton("Clear Logs")
        self.clear_button.clicked.connect(self._on_clear_logs)
        controls_layout.addLayout(level_layout)
        controls_layout.addWidget(self.auto_scroll_check)
        controls_layout.addStretch()
        controls_layout.addWidget(self.clear_button)
        layout.addLayout(controls_layout)
        # Log text display
        self.log_text = QTextEdit()
        self.log_text.setReadOnly(True)
        self.log_text.setLineWrapMode(QTextEdit.NoWrap)
        font = QFont("Courier New")
        font.setStyleHint(QFont.Monospace)
        self.log_text.setFont(font)
        layout.addWidget(self.log_text)

    @Slot(object)
    def _on_new_log_record(self, record):
        if record.levelno < self.filter_level:
            return
        message = self.log_handler.format(record)
        format = QTextCharFormat()
        format.setForeground(self.log_colors.get(record.levelno, QColor(0, 0, 0)))
        if record.levelno >= logging.WARNING:
            font = format.font()
            font.setBold(True)
            format.setFont(font)
        cursor = self.log_text.textCursor()
        cursor.movePosition(QTextCursor.End)
        cursor.insertText(message + '\n', format)
        if self.auto_scroll:
            self.log_text.setTextCursor(cursor)
            self.log_text.ensureCursorVisible()
        self._limit_log_lines()

    def _on_level_changed(self, index):
        self.filter_level = self.level_combo.currentData()
        self.log_text.clear()
        # If you want to re-display existing logs at new filter, you must store them.

    def _on_auto_scroll_toggled(self, checked):
        self.auto_scroll = checked
        if checked:
            cursor = self.log_text.textCursor()
            cursor.movePosition(QTextCursor.End)
            self.log_text.setTextCursor(cursor)
            self.log_text.ensureCursorVisible()

    def _on_clear_logs(self):
        self.log_text.clear()

    def _limit_log_lines(self):
        document = self.log_text.document()
        line_count = document.blockCount()
        if line_count > self.max_log_lines:
            cursor = QTextCursor(document)
            cursor.movePosition(QTextCursor.Start)
            cursor.movePosition(QTextCursor.Down, QTextCursor.KeepAnchor, line_count - self.max_log_lines)
            cursor.removeSelectedText()

    def set_max_log_lines(self, max_lines):
        self.max_log_lines = max_lines

