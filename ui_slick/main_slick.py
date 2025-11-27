"""
Slick UI Launcher for VFX Project Launcher
Runs the modern PySide6-based UI with async panels and real DB integration.
"""
import sys
import logging
import os
from datetime import datetime

# Set up root logger to save to a log file in user's AppData directory
# This avoids permission issues when installed in Program Files
appdata_dir = os.path.join(os.environ.get('LOCALAPPDATA', os.path.expanduser('~')), 'VFX_Launcher')
log_dir = os.path.join(appdata_dir, 'logs')

# Create logs directory if it doesn't exist
if not os.path.exists(log_dir):
    os.makedirs(log_dir, exist_ok=True)

# Create log filename with timestamp
log_filename = os.path.join(log_dir, f'app_log_{datetime.now().strftime("%Y%m%d_%H%M%S")}.log')

# Configure root logger with both file and console handlers
root_logger = logging.getLogger()
root_logger.setLevel(logging.DEBUG)

# File handler - saves all logs to file
file_handler = logging.FileHandler(log_filename)
file_handler.setLevel(logging.DEBUG)
file_handler.setFormatter(logging.Formatter('%(asctime)s [%(levelname)s] %(name)s: %(message)s'))
root_logger.addHandler(file_handler)

# Console handler - for display in terminal
console_handler = logging.StreamHandler()
console_handler.setLevel(logging.DEBUG)
console_handler.setFormatter(logging.Formatter('%(asctime)s [%(levelname)s] %(message)s'))
root_logger.addHandler(console_handler)

# Get module logger
logger = logging.getLogger(__name__)
logger.info(f'[LOGGING] Logging system initialized. Saving logs to: {log_filename}')

"""
Slick UI Launcher for VFX Project Launcher
Runs the modern PySide6-based UI with async panels and real DB integration.
"""
import sys
import os
# Ensure parent directory is on sys.path for package imports
sys.path.insert(0, os.path.abspath(os.path.dirname(__file__)))
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), '..')))
from PySide6.QtWidgets import QApplication, QMainWindow, QStackedWidget, QDockWidget
from PySide6.QtCore import Qt
from PySide6.QtGui import QIcon
# Import our SlickMainWindow class from the main_window module
from ui_slick.main_window import SlickMainWindow
from core.database import DatabaseManager
from core.config import ConfigManager

def main():
    app = QApplication(sys.argv)

    # Set application icon globally
    icon_path = os.path.join(os.path.dirname(__file__), "icons", "icon_3_512.png")
    if os.path.exists(icon_path):
        app.setWindowIcon(QIcon(icon_path))

    # Load configuration
    config_manager = ConfigManager()
    db_path = config_manager.get_database_path()
    config_path = config_manager.get_config_dir()

    logger.info(f"Using database path: {db_path}")
    logger.info(f"Using config path: {config_path}")

    # Create database manager
    db_manager = DatabaseManager(db_path)
    
    # Default user data
    user_data = {"username": "admin", "is_admin": 1, "id": 1}
    
    # Create main window with proper parameters
    win = SlickMainWindow(db_manager, config_path, user_data)
    win.show()
    
    sys.exit(app.exec())

if __name__ == "__main__":
    main()
