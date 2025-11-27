"""
VFX Project Launcher - Main Application Entry Point

Purpose:
    Initializes and launches the PySide6-based GUI for the VFX Project Launcher.
    Connects and integrates all UI modules and core logic components.

Requirements:
    - Loads application settings from app_settings.ini
    - Initializes logging
    - Sets up main PySide application window and login workflow
    - Launches GUI components: main window, logging, settings, user management, project browser
"""

import os
import sys
import logging
import configparser
from pathlib import Path

from PySide6.QtWidgets import QApplication, QSplashScreen, QMessageBox
from PySide6.QtGui import QPixmap, QFont
from PySide6.QtCore import Qt, QTimer

from core.database import DatabaseManager
from core.utils import setup_logger
from ui.login import LoginDialog
from ui.main_window import MainWindow


def main():
    """
    Main application entry point.
    """
    # Create QApplication instance
    app = QApplication(sys.argv)
    app.setApplicationName("VFX Project Launcher")
    app.setApplicationVersion("1.0")
    
    # Set default font
    default_font = QFont("Segoe UI", 9)
    app.setFont(default_font)
    
    # Determine application paths
    app_dir = os.path.dirname(os.path.abspath(__file__))
    config_path = os.path.join(app_dir, "app_settings.ini")
    
    # Load configuration
    config = configparser.ConfigParser()
    
    if os.path.exists(config_path):
        config.read(config_path)
    else:
        # Create default configuration if it doesn't exist
        config['Paths'] = {
            'nuke_path': 'C:\\Program Files\\Nuke13.2v1\\Nuke13.2.exe',
            'after_effects_path': 'C:\\Program Files\\Adobe\\Adobe After Effects 2023\\Support Files\\AfterFX.exe'
        }
        config['Projects'] = {
            'project_directories': 'D:\\Projects;E:\\VFX_Projects',
            'scan_interval_minutes': '1'
        }
        config['Database'] = {
            'db_path': './vfx_launcher.db'
        }
        config['Logging'] = {
            'log_level': 'INFO',
            'log_file': './vfx_launcher.log'
        }
        
        # Save default configuration
        os.makedirs(os.path.dirname(os.path.abspath(config_path)), exist_ok=True)
        with open(config_path, 'w') as config_file:
            config.write(config_file)
    
    # Initialize logging
    log_file = config.get('Logging', 'log_file', fallback='./vfx_launcher.log')
    log_level_str = config.get('Logging', 'log_level', fallback='INFO')
    log_level = getattr(logging, log_level_str, logging.INFO)
    
    log_path = os.path.join(app_dir, log_file)
    setup_logger(log_path, log_level)
    
    # Log application start
    logger = logging.getLogger(__name__)
    logger.info("==== VFX Project Launcher Starting ====")
    logger.info(f"Application directory: {app_dir}")
    logger.info(f"Configuration file: {config_path}")
    
    # Create splash screen
    # Note: In a real application, you would have an actual splash image
    splash = QSplashScreen(QPixmap())
    splash_font = QFont("Arial", 12)
    splash.setFont(splash_font)
    splash.showMessage("Initializing VFX Project Launcher...", 
                       Qt.AlignCenter | Qt.AlignBottom, Qt.black)
    splash.show()
    app.processEvents()
    
    # Initialize database
    try:
        db_path = config.get('Database', 'db_path', fallback='./vfx_launcher.db')
        db_path = os.path.join(app_dir, db_path)
        
        splash.showMessage("Connecting to database...", 
                         Qt.AlignCenter | Qt.AlignBottom, Qt.black)
        app.processEvents()
        
        db_manager = DatabaseManager(db_path)
        logger.info(f"Database initialized: {db_path}")
        
    except Exception as e:
        logger.critical(f"Failed to initialize database: {str(e)}")
        QMessageBox.critical(
            None,
            "Database Error",
            f"Failed to initialize database: {str(e)}"
        )
        return 1
    
    # Show login dialog
    splash.showMessage("Starting login...", 
                     Qt.AlignCenter | Qt.AlignBottom, Qt.black)
    app.processEvents()
    
    login_dialog = LoginDialog(db_manager)
    
    # Close splash and show login
    splash.finish(login_dialog)
    
    if login_dialog.exec():
        # Get user data from login
        user_data = login_dialog.get_user_data()
        
        if user_data:
            # Create and show main window
            main_window = MainWindow(db_manager, config_path, user_data)
            main_window.show()
            
            # Run the application
            return app.exec()
        else:
            logger.error("Login failed: No user data")
            return 1
    else:
        logger.info("Login cancelled")
        return 0


if __name__ == "__main__":
    sys.exit(main())
