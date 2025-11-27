"""
Test script for enhanced scan progress reporting

This script tests the new progress reporting features:
- Frequent progress updates
- ETA calculations
- Current directory display
- Visual indicators in logs
- Error/warning visibility
"""

import sys
import os

# Add parent directory to path
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from PySide6.QtWidgets import QApplication, QPushButton, QVBoxLayout, QWidget, QFileDialog
from PySide6.QtCore import Qt
from core.database import DatabaseManager
from ui_slick.scan_progress_dialog import ScanManager

class TestWindow(QWidget):
    """Simple test window to trigger scans"""
    
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Scan Progress Test")
        self.setMinimumSize(400, 200)
        
        # Initialize database
        db_path = os.path.join(os.path.dirname(__file__), "vfx_launcher.db")
        self.db_manager = DatabaseManager(db_path)
        
        # Create scan manager
        self.scan_manager = ScanManager(self.db_manager, self)
        
        # Setup UI
        layout = QVBoxLayout(self)
        
        # Full scan button
        self.full_scan_btn = QPushButton("Test Full Scan")
        self.full_scan_btn.clicked.connect(self.test_full_scan)
        layout.addWidget(self.full_scan_btn)
        
        # Quick scan button
        self.quick_scan_btn = QPushButton("Test Quick Scan")
        self.quick_scan_btn.clicked.connect(self.test_quick_scan)
        layout.addWidget(self.quick_scan_btn)
        
        # Info label
        from PySide6.QtWidgets import QLabel
        info = QLabel(
            "Click a button to test scan progress reporting.\n\n"
            "Features to observe:\n"
            "• Frequent progress updates\n"
            "• ETA calculations\n"
            "• Current directory display\n"
            "• Visual indicators (✓, ❌, 📁, 📄)\n"
            "• Scan rate statistics"
        )
        info.setWordWrap(True)
        layout.addWidget(info)
        
    def test_full_scan(self):
        """Test full scan with progress reporting"""
        # Let user select a project directory
        project_path = QFileDialog.getExistingDirectory(
            self,
            "Select Project Directory to Scan",
            "",
            QFileDialog.ShowDirsOnly
        )
        
        if project_path:
            print(f"Starting full scan of: {project_path}")
            self.scan_manager.scan_project(
                project_path,
                on_finished=self.on_scan_finished,
                quick_scan=False
            )
    
    def test_quick_scan(self):
        """Test quick scan with progress reporting"""
        # Let user select a project directory
        project_path = QFileDialog.getExistingDirectory(
            self,
            "Select Project Directory for Quick Scan",
            "",
            QFileDialog.ShowDirsOnly
        )
        
        if project_path:
            print(f"Starting quick scan of: {project_path}")
            self.scan_manager.scan_project(
                project_path,
                on_finished=self.on_scan_finished,
                quick_scan=True
            )
    
    def on_scan_finished(self, project_path, found_files):
        """Handle scan completion"""
        print(f"Scan complete! Found {len(found_files)} files in {project_path}")

def main():
    """Run the test application"""
    app = QApplication(sys.argv)
    
    window = TestWindow()
    window.show()
    
    sys.exit(app.exec())

if __name__ == "__main__":
    main()

