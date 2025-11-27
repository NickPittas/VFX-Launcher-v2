"""
Scan Progress Dialog for VFX Launcher

Dedicated UI for displaying scan progress:
- Progress bar with percentage and ETA
- Log window showing scanned folders and files
- Real-time updates
- Cancelable operations
"""

import os
import logging
from PySide6.QtCore import Qt, Signal
from PySide6.QtWidgets import (
    QDialog, QVBoxLayout, QHBoxLayout, QLabel, QProgressBar,
    QPushButton, QTextEdit, QSplitter, QTabWidget, QTreeWidget,
    QTreeWidgetItem, QWidget
)
from PySide6.QtGui import QFont, QTextCursor

# Configure logger
logger = logging.getLogger(__name__)

class ScanProgressDialog(QDialog):
    """
    Dialog for showing scan progress with:
    - Progress bar with percentage and ETA
    - Tabbed interface for logs and results
    - Cancelable operation
    """
    cancel_requested = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWindowTitle("Scanning Project Files")
        self.setMinimumSize(700, 500)
        self.setModal(True)
        # Initialize variables
        self.folder_count = 0
        self.file_count = 0
        self.matching_folders = []
        self.found_files = []
        # Setup UI
        self._setup_ui()
        
    def get_signals(self):
        """Return a signal handler object that scanner can use to update this dialog"""
        from ui_slick.scanner import ScannerSignals
        signals = ScannerSignals()
        
        # Connect the signals to the dialog methods
        signals.progress.connect(self.update_progress)
        signals.log.connect(self.append_log)  # Use existing method
        signals.folder_found.connect(self.show_folder_found)  # Use existing method
        signals.file_found.connect(self.show_file_found)  # Use existing method
        signals.error.connect(self.append_error)  # Add this method
        
        return signals
        
    def append_error(self, error_message):
        """Add an error message to the log"""
        # Format with timestamp and red text
        formatted = f"<span style='color:red'>[{self._get_timestamp()}] ERROR: {error_message}</span>"
        self.append_log(formatted, html=True)
        
    def _setup_ui(self):
        """Set up the UI components"""
        layout = QVBoxLayout(self)
        
        # Header with title and progress info
        header_layout = QVBoxLayout()

        self.title_label = QLabel("Scanning Project Files")
        self.title_label.setStyleSheet("font-size: 16px; font-weight: bold;")
        header_layout.addWidget(self.title_label)

        # Current directory label (shows what's being scanned right now)
        self.current_dir_label = QLabel("Initializing...")
        self.current_dir_label.setStyleSheet("font-size: 11px; color: #666; font-style: italic;")
        header_layout.addWidget(self.current_dir_label)

        # Progress bar with labels
        progress_layout = QHBoxLayout()

        self.progress_label = QLabel("Initializing scan...")
        progress_layout.addWidget(self.progress_label, 1)

        self.eta_label = QLabel("ETA: --:--")
        self.eta_label.setStyleSheet("font-weight: bold;")
        progress_layout.addWidget(self.eta_label)

        header_layout.addLayout(progress_layout)

        self.progress_bar = QProgressBar()
        self.progress_bar.setRange(0, 100)
        self.progress_bar.setTextVisible(True)
        self.progress_bar.setFormat("%p%")
        header_layout.addWidget(self.progress_bar)
        
        layout.addLayout(header_layout)
        
        # Tabs for logs and results
        self.tab_widget = QTabWidget()
        
        # Log tab
        self.log_text = QTextEdit()
        self.log_text.setReadOnly(True)
        self.log_text.setLineWrapMode(QTextEdit.NoWrap)
        self.log_text.setStyleSheet("font-family: monospace;")
        self.tab_widget.addTab(self.log_text, "Log")
        
        # Matching folders tab
        self.folders_tree = QTreeWidget()
        self.folders_tree.setHeaderLabels(["Matching Folders"])
        self.folders_tree.setColumnCount(1)
        self.tab_widget.addTab(self.folders_tree, "Matching Folders (0)")
        
        # Found files tab
        self.files_tree = QTreeWidget()
        self.files_tree.setHeaderLabels(["Filename", "Path", "Type", "Version"])
        self.files_tree.setColumnCount(4)
        self.tab_widget.addTab(self.files_tree, "Found Files (0)")
        
        layout.addWidget(self.tab_widget, 1)
        
        # Buttons at bottom
        button_layout = QHBoxLayout()
        button_layout.addStretch(1)

        self.cancel_button = QPushButton("Cancel")
        self.cancel_button.setToolTip("Cancel the current scan operation")
        self.cancel_button.clicked.connect(self._on_cancel)
        button_layout.addWidget(self.cancel_button)

        layout.addLayout(button_layout)

    def update_progress(self, percent, message, eta):
        """Update progress bar and labels"""
        logger.info(f"[ScanProgressDialog] update_progress called: {percent}%, {message}, ETA: {eta}")
        self.progress_bar.setValue(percent)
        self.progress_label.setText(message)
        self.eta_label.setText(f"ETA: {eta}")

        # Extract current directory from message if present (format: "[x/y] directory")
        # This helps show what's currently being scanned
        if "]" in message:
            # Extract the part after the bracket
            parts = message.split("]", 1)
            if len(parts) > 1:
                current_info = parts[1].strip()
                self.current_dir_label.setText(f"Current: {current_info}")
        else:
            self.current_dir_label.setText(message)

        # If progress is 100%, change Cancel button to Close button
        if percent == 100:
            logger.info(f"[ScanProgressDialog] Progress reached 100%, converting to close button")
            self.current_dir_label.setText("Scan complete")
            self._convert_to_close_button()
        else:
            logger.debug(f"[ScanProgressDialog] Progress at {percent}%, not converting button yet")
        
    def append_log(self, text, html=False):
        """Append text to log window"""
        timestamp = self._get_timestamp()
        if html:
            # If HTML is already formatted, just append it
            self.log_text.append(text)
        else:
            # Otherwise format with timestamp
            self.log_text.append(f"[{timestamp}] {text}")
        self.log_text.moveCursor(QTextCursor.End)
        self.log_text.ensureCursorVisible()
        
    def add_log(self, text, level="info"):
        """Add a log entry with level formatting (compatibility method)"""
        # Map level to color
        colors = {
            "info": "black",
            "warning": "orange",
            "error": "red",
            "success": "green"
        }
        color = colors.get(level, "black")
        
        # Format with HTML if it's not an info level
        if level != "info":
            formatted = f"<span style='color:{color}'>[{self._get_timestamp()}] {level.upper()}: {text}</span>"
            self.append_log(formatted, html=True)
        else:
            # Normal append for info level
            self.append_log(text)
        
    def scan_complete(self):
        """Handle scan completion"""
        # Update progress to 100%
        self.update_progress(100, "Scan complete", "--:--")
        # Convert cancel button to close button
        self._convert_to_close_button()
        # Add completion message
        self.append_log(f"Scan complete. Found {self.folder_count} folders and {self.file_count} files.")
        # Show a summary in the title
        self.title_label.setText(f"Scan Complete: {self.folder_count} folders, {self.file_count} files")
        
    def _convert_to_close_button(self):
        """Convert the Cancel button to a Close button"""
        # Disconnect all existing signals
        try:
            self.cancel_button.clicked.disconnect()
        except (RuntimeError, TypeError):
            # No connections to disconnect
            pass

        # Change text, tooltip, and connect to accept (close dialog)
        self.cancel_button.setText("Close")
        self.cancel_button.setToolTip("Close this dialog")
        self.cancel_button.setEnabled(True)  # Re-enable in case it was disabled
        self.cancel_button.clicked.connect(self.accept)
        
    def _on_cancel(self):
        """Handle cancel button click"""
        # Confirm cancellation
        from PySide6.QtWidgets import QMessageBox
        confirm = QMessageBox.question(
            self, 
            "Cancel Scan", 
            "Are you sure you want to cancel the current scan?",
            QMessageBox.Yes | QMessageBox.No,
            QMessageBox.No
        )
        
        if confirm == QMessageBox.Yes:
            # Emit signal to cancel the scan
            self.append_log("Cancelling scan...")
            self.cancel_requested.emit()
            
            # Change button to indicate cancellation in progress
            self.cancel_button.setText("Cancelling...")
            self.cancel_button.setEnabled(False)
        
    def _get_timestamp(self):
        """Get current timestamp for log entries"""
        from datetime import datetime
        return datetime.now().strftime("%H:%M:%S")
        
    def show_folder_found(self, folder_path):
        """Add a found folder to the folders tree"""
        self.folder_count += 1
        self.matching_folders.append(folder_path)
        
        # Add to tree
        item = QTreeWidgetItem([folder_path])
        self.folders_tree.addTopLevelItem(item)
        
        # Update tab text
        self.tab_widget.setTabText(1, f"Matching Folders ({self.folder_count})")
        
    def show_file_found(self, file_dict):
        """Add a found file to the files tree"""
        self.file_count += 1
        self.found_files.append(file_dict)
        
        # Add to tree
        filename = file_dict.get('filename', '')
        filepath = file_dict.get('filepath', '')
        filetype = file_dict.get('filetype', '')
        version = file_dict.get('version', '')
        
        item = QTreeWidgetItem([filename, filepath, filetype, version])
        self.files_tree.addTopLevelItem(item)
        
        # Update tab text
        self.tab_widget.setTabText(2, f"Found Files ({self.file_count})")
        
    def _on_cancel(self):
        """Handle cancel button click"""
        # If text is "Close", just accept the dialog
        if self.cancel_button.text() == "Close":
            self.accept()
            return
        
        # Otherwise, emit the cancel signal
        self.cancel_requested.emit()
        self.cancel_button.setEnabled(False)
        self.progress_label.setText("Cancelling scan...")
        self.append_log("Cancelling scan...")
        
    def _convert_to_close_button(self):
        """Convert cancel button to close button"""
        self.cancel_button.setText("Close")
        self.cancel_button.setEnabled(True)
        self.cancel_button.setStyleSheet("QPushButton { background-color: #4CAF50; color: white; }")


class ScanManager:
    """
    Manager class to handle scanning operations with UI feedback.
    - Coordinates Scanner and ScanProgressDialog
    - Provides high-level scanning API for UI components
    - Supports both full scans and quick scans
    """
    def __init__(self, db_manager=None, parent=None):
        from ui_slick.scanner import Scanner
        
        self.scanner = Scanner(db_manager)
        self.parent = parent
        self.db_manager = db_manager
        self.dialog = None
        
    def scan_project(self, project_path, on_finished=None, quick_scan=False):
        """
        Start scanning a project with UI feedback
        
        Args:
            project_path: Path to project root
            on_finished: Callback when scan is complete (project_path, found_files)
            quick_scan: If True, perform a quick scan of existing folders only
        """
        from ui_slick.scanner import Scanner
        
        # Create progress dialog with appropriate title
        scan_type = "Quick Scan" if quick_scan else "Full Scan"
        self.dialog = ScanProgressDialog(self.parent)
        self.dialog.setWindowTitle(f"{scan_type}: {os.path.basename(project_path)}")
        self.dialog.title_label.setText(f"{scan_type}: {os.path.basename(project_path)}")
        self.dialog.show()
        
        # Connect cancel button
        self.dialog.cancel_requested.connect(self.scanner.stop)
        
        # Store callback
        self.on_finished_callback = on_finished
        
        # Prepare scan arguments
        scan_args = {
            'project_path': project_path,
            'on_progress': self.dialog.update_progress,
            'on_folder_found': self.dialog.show_folder_found,
            'on_file_found': self.dialog.show_file_found,
            'on_log': self.dialog.append_log,
            'on_finished': self._on_scan_finished,
            'on_error': self._on_scan_error
        }
        
        # Start the appropriate scan type
        if quick_scan:
            self.dialog.append_log(f"Starting quick scan of existing folders in {project_path}")
            success = self.scanner.quick_scan_project(**scan_args)
        else:
            self.dialog.append_log(f"Starting full scan of project at {project_path}")
            success = self.scanner.scan_project(**scan_args)
        
        if not success:
            self.dialog.append_log(f"Failed to start scan for project: {project_path}")
            self.dialog.update_progress(100, "Scan failed", "00:00")
            
        return success
        
    def _on_scan_finished(self, project_path, found_files):
        """Handle scan completion"""
        logger.info(f"[ScanManager] _on_scan_finished called with {len(found_files)} files")
        if self.dialog:
            logger.info(f"[ScanManager] Updating dialog progress to 100%")
            self.dialog.update_progress(100, "Scan complete", "00:00")
            self.dialog.append_log(f"Scan complete. Found {len(found_files)} files.")

            # Convert cancel button to close button
            logger.info(f"[ScanManager] Converting cancel button to close button")
            self.dialog._convert_to_close_button()

            # Automatically switch to the files tab if files were found
            if found_files:
                self.dialog.tab_widget.setCurrentIndex(2)  # Files tab

        # Call user callback
        if self.on_finished_callback:
            logger.info(f"[ScanManager] Calling user on_finished callback")
            self.on_finished_callback(project_path, found_files)
        logger.info(f"[ScanManager] _on_scan_finished completed")
            
    def _on_scan_error(self, message):
        """Handle scan error"""
        if self.dialog:
            self.dialog.append_log(f"ERROR: {message}")
            self.dialog.update_progress(100, "Scan failed", "00:00")
