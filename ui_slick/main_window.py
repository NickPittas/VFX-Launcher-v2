# Modern Main Window for VFX Launcher (PySide6)
# Uses sidebar navigation and modular tabbed panels
from PySide6.QtWidgets import QMainWindow, QWidget, QHBoxLayout, QStackedWidget, QApplication, QSplitter
from PySide6.QtCore import Qt, QSettings
from PySide6.QtGui import QIcon
from .sidebar import Sidebar
from .project_browser import ProjectBrowser
from .user_management import UserManagement
from .settings_panel import SettingsPanel
from .new_project_panel import NewProjectPanel
from .log_panel import LogPanel
import os
import logging

# Set up logger
logger = logging.getLogger(__name__)

class SlickMainWindow(QMainWindow):
    """
    Main application window for the Slick UI.
    Features sidebar navigation and modular, scalable panels.
    Loads QSS stylesheet for modern, web-inspired look.
    """
    def __init__(self, db_manager, config_path, user_data):
        super().__init__()
        self.setWindowTitle("VFX Project Launcher (Slick UI)")
        self.setMinimumSize(1200, 800)
        self.setObjectName("SlickMainWindow")

        # Set application icon
        icon_path = os.path.join(os.path.dirname(__file__), "icons", "icon_3_512.png")
        if os.path.exists(icon_path):
            self.setWindowIcon(QIcon(icon_path))

        self.db_manager = db_manager
        self.config_path = config_path
        self.user_data = user_data
        self.current_project_path = None  # Add this to track the current project path for scanning

        # Initialize settings
        self.settings = QSettings("VFX_Launcher", "SlickUI")

        # TEMPORARY: Clear old column settings that might be corrupted
        # Remove this after first run with new column defaults
        if self.settings.value("columns/cleared_v3") != "true":
            logger.info("Clearing old column settings to apply new 3000px defaults")
            self.settings.remove("columns/projectBrowser")
            self.settings.remove("columns/filesBrowser/nuke")
            self.settings.remove("columns/filesBrowser/aep")
            self.settings.setValue("columns/cleared_v3", "true")

        self._setup_ui()
        self._apply_stylesheet()
        self._restore_settings()

    def _setup_ui(self):
        """
        Set up the main layout with sidebar and stacked panels.
        """
        central = QWidget()
        layout = QHBoxLayout(central)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(0)

        # Sidebar navigation
        self.sidebar = Sidebar(self)
        self.sidebar.setFixedWidth(200)
        self.sidebar.navigation_requested.connect(self._on_navigate)

        # Stacked widget for panels
        self.stack = QStackedWidget()
        from .files_browser import FilesBrowser
        self.files_browser = FilesBrowser(self.db_manager, self.user_data, self)
        self.project_browser = ProjectBrowser(self.db_manager, self.user_data, self)
        self.new_project_panel = NewProjectPanel(self)
        self.user_management = UserManagement(self.db_manager, self)
        self.settings_panel = SettingsPanel(self.db_manager, self.user_data, self)
        self.log_panel = LogPanel(self)

        # Create a dedicated container for projects panel
        self.projects_panel = QWidget()
        projects_layout = QHBoxLayout(self.projects_panel)
        projects_layout.setContentsMargins(0, 0, 0, 0)
        projects_layout.setSpacing(0)
        
        # Create a QSplitter with VERY visible handle
        splitter = QSplitter(Qt.Horizontal)
        
        # Make the handle extremely obvious
        splitter.setHandleWidth(10)  # Nice wide handle
        splitter.setChildrenCollapsible(False)  # Prevent panels from collapsing
        
        # Add both panels to the splitter
        splitter.addWidget(self.project_browser)
        splitter.addWidget(self.files_browser)
        
        # Set initial sizes - 30% project browser, 70% files browser
        total_width = 1000  # Arbitrary reference width
        splitter.setSizes([int(total_width * 0.3), int(total_width * 0.7)])
        
        # Apply custom styling to make the handle VERY visible
        splitter.setStyleSheet("""
            QSplitter::handle {
                background-color: #2563eb; /* Bright blue handle */
                border: 1px solid #3b82f6;
                border-radius: 2px;
                margin: 2px;
            }
            QSplitter::handle:hover {
                background-color: #1d4ed8; /* Darker blue on hover */
                border: 1px solid #60a5fa;
            }
            QSplitter::handle:pressed {
                background-color: #1e40af; /* Even darker when pressed */
            }
        """)
        
        # Add the splitter to the layout
        projects_layout.addWidget(splitter)
        
        # Store the splitter for potential later use
        self.project_splitter = splitter

        # Add panels to stack in navigation order
        self.stack.addWidget(self.projects_panel)    # index 0
        self.stack.addWidget(self.new_project_panel) # index 1
        self.stack.addWidget(self.user_management)   # index 2
        self.stack.addWidget(self.settings_panel)    # index 3
        self.stack.addWidget(self.log_panel)         # index 4

        # Connect project browser to files browser
        self.project_browser.file_selected.connect(self._on_project_file_selected)
        self.project_browser.project_selected.connect(self._on_project_selected)
        self.project_browser.scan_finished.connect(self._on_scan_finished)
        self.files_browser.file_launched.connect(self._on_file_launched)
        
        # Make the database path available to all components for scanner integration
        self.db_path = self.db_manager.db_path

        layout.addWidget(self.sidebar)
        layout.addWidget(self.stack, 1)
        self.setCentralWidget(central)

        # Start with Projects panel
        self._on_navigate(0)

    def _on_navigate(self, index):
        """
        Switch to the selected panel based on sidebar navigation.
        """
        if 0 <= index < self.stack.count():
            self.stack.setCurrentIndex(index)

    def _add_base_name(self, files):
        import re
        def extract_base_name(filename):
            m = re.match(r"(.+?)(_v\d{2,4})?(\.[^.]+)?$", filename)
            if m:
                return m.group(1)
            return filename
        for file in files:
            filename = file.get('filename', '')
            file['base_name'] = extract_base_name(filename)
        logger.debug(f"[DIAG] _add_base_name processed files: {files}")
        return files

    def _on_project_selected(self, proj):
        """
        Handle project selection: set current project ID and show files from DB.
        """
        logger.info(f"Project selected: {proj['name']}")
        
        # Store the project path for scanning purposes
        project_path = proj['path']
        self.current_project_path = project_path  # Explicitly store at class level
        
        # Use project ID if available, else resolve from path
        db = self.db_manager
        project = db.get_project_by_path(project_path)
        
        if project:
            self.current_project_id = project['id']
            logger.info(f"Loaded project ID: {self.current_project_id}")
            files = db.get_files_with_access_info(self.current_project_id)
            logger.info(f"Loaded files for project: {len(files)}")
            files = self._add_base_name(files)
            
            # DIRECT INJECTION: Set the current project path into the files browser
            # This ensures the scan button will work properly
            if hasattr(self.files_browser, 'current_project_path'):
                self.files_browser.current_project_path = project_path
                logger.info(f"[MainWindow] Direct injection of project path to FilesBrowser: {project_path}")
            
            # Also use the normal method
            self.files_browser.set_current_project(project_path, files)
        else:
            self.current_project_id = None
            logger.warning("Project not found in database")
            
            # Still set the project path even if no files found
            if hasattr(self.files_browser, 'current_project_path'):
                self.files_browser.current_project_path = project_path
                
            self.files_browser.set_current_project(project_path, [])

    def _on_project_file_selected(self, nuke_files, aep_files):
        """
        Update FilesBrowser with files for the selected project.
        """
        logger.info("Project file selected, reloading files from database")
        if hasattr(self, 'current_project_id'):
            files = self.db_manager.get_files_with_access_info(self.current_project_id)
            logger.info(f"Loaded files for project: {len(files)}")
            files = self._add_base_name(files)
            logger.debug(f"[DIAG] About to call display_files with files: {files}")
            self.files_browser.display_files(files)
        else:
            logger.warning("No current project ID, cannot load files")

    def _on_scan_finished(self, project_path):
        """
        Called when the project scanner finishes a full scan.
        Reloads files from the database for the scanned project.
        Uses robust path normalization to ensure the project is properly found in the database.
        """
        import os
        import logging
        
        try:
            # Canonical lookup first; single fallback to the raw spelling for legacy DB rows
            canonical_path = os.path.normcase(os.path.normpath(os.path.abspath(project_path)))
            logging.info(f"[MainWindow] Scan finished for project path: {project_path}")
            logging.info(f"[MainWindow] Using canonical path for DB lookup: {canonical_path}")

            project = self.db_manager.get_project_by_path(canonical_path)
            if not project:
                logging.info(f"[MainWindow] Project not found with canonical path, trying raw: {project_path}")
                project = self.db_manager.get_project_by_path(project_path)
            # If project found in DB, load its files
            if project:
                logging.info(f"[MainWindow] Found project in DB: {project['name']} (ID: {project['id']})")
                files = self.db_manager.get_files_with_access_info(project['id'])
                
                if files:
                    logging.info(f"[MainWindow] Loaded {len(files)} files from DB for project {project['name']}")
                    
                    # Make sure all files have a project_path
                    for file in files:
                        if 'project_path' not in file:
                            file['project_path'] = project['path']
                    
                    # Add base_name to all files for versioning
                    files_with_base_names = self._add_base_name(files)
                    
                    # Update file browser display
                    self.files_browser.display_files(files_with_base_names)
                else:
                    logging.warning(f"[MainWindow] No files found in DB for project {project['name']} (ID: {project['id']})")
            else:
                # Log available project paths for diagnosis
                all_projects = self.db_manager._execute_query("SELECT id, name, path FROM projects")
                logging.error(f"[MainWindow] Project not found in DB for path {canonical_path}. Available: {[p['path'] for p in all_projects]}")
                
                from PySide6.QtWidgets import QMessageBox
                QMessageBox.warning(self, "Project Not Found", 
                                  f"The project at {project_path} was not found in the database after scanning. "
                                  f"Try refreshing the project list.")
        except Exception as e:
            import traceback
            logging.error(f"[MainWindow] Error in _on_scan_finished: {str(e)}\n{traceback.format_exc()}")

    def _on_file_launched(self, filepath):
        """
        Launch the selected file using the appropriate application based on file extension
        and app_settings.ini configuration.
        """
        import subprocess
        import configparser
        import os
        import sys
        import shlex
        import logging

        logging.info(f"Launching file: {filepath}")

        try:
            # Get file extension
            _, file_extension = os.path.splitext(filepath)
            file_extension = file_extension.lower()

            # Read application paths from app_settings.ini
            from core.config import resolve_app_settings_path
            config = configparser.ConfigParser()
            config_path = resolve_app_settings_path()
            
            if not os.path.exists(config_path):
                logging.warning(f"Settings file not found: {config_path}")
                raise FileNotFoundError(f"Settings file not found: {config_path}")
                
            config.read(config_path, encoding="utf-8")
            
            # Launch based on file extension
            if file_extension in ('.nk', '.nknc'):
                if sys.platform.startswith("linux"):
                    # Full command line (terminal wrapper/env allowed), file path is appended
                    nuke_cmd = config.get("Paths", "nuke_launch_cmd_linux", fallback="")

                    if not nuke_cmd:
                        logging.warning("Linux Nuke launch command not configured in settings")
                        raise ValueError("Linux Nuke launch command not configured in settings")

                    logging.info(f"Launching Nuke file on Linux: {filepath}")
                    subprocess.Popen(shlex.split(nuke_cmd) + [filepath])
                else:
                    # Launch with Nuke
                    nuke_path = config.get("Paths", "nuke_path", fallback="")

                    if not nuke_path:
                        logging.warning("Nuke path not configured in settings")
                        raise ValueError("Nuke path not configured in settings")

                    logging.info(f"Launching Nuke file with NukeX: {filepath}")
                    subprocess.Popen([nuke_path, '--nukex', filepath])
                
            elif file_extension in ('.aep', '.aet'):
                # Launch with After Effects
                ae_path = config.get("Paths", "after_effects_path", fallback="")
                
                if not ae_path:
                    logging.warning("After Effects path not configured in settings")
                    raise ValueError("After Effects path not configured in settings")
                    
                logging.info(f"Launching After Effects file: {filepath}")
                subprocess.Popen([ae_path, filepath])
                
            else:
                # Use default application for other file types
                logging.info(f"Launching file with default application: {filepath}")
                if hasattr(os, 'startfile'):
                    os.startfile(filepath)
                else:
                    subprocess.Popen(['xdg-open', filepath])
                
        except Exception as e:
            import traceback
            logging.error(f"Error launching file: {str(e)}\n{traceback.format_exc()}")
            
            from PySide6.QtWidgets import QMessageBox
            QMessageBox.warning(self, "Launch Error", 
                             f"Failed to launch file:\n{filepath}\n\nError: {str(e)}\n\n"
                             f"Please check that the application path is correctly configured in Settings.")


    def _apply_stylesheet(self):
        """
        Load and apply the QSS stylesheet for modern look.
        """
        qss_path = os.path.join(os.path.dirname(__file__), "styles.qss")
        if os.path.exists(qss_path):
            with open(qss_path, "r", encoding="utf-8") as f:
                self.setStyleSheet(f.read())
        else:
            print("[SlickMainWindow] Warning: styles.qss not found, using default Qt style.")

    def _save_settings(self):
        """Save UI state to settings"""
        try:
            # Save window geometry
            self.settings.setValue("geometry", self.saveGeometry())
            self.settings.setValue("windowState", self.saveState())

            # Save splitter state
            if hasattr(self, 'project_splitter'):
                self.settings.setValue("splitter/state", self.project_splitter.saveState())

            # Save current panel index
            self.settings.setValue("ui/currentPanel", self.stack.currentIndex())

            # Save sidebar collapsed state
            if hasattr(self.sidebar, 'is_sidebar_collapsed'):
                self.settings.setValue("ui/sidebarCollapsed", self.sidebar.is_sidebar_collapsed())

            # Save selected project ID
            if hasattr(self.project_browser, 'get_selected_project_id'):
                project_id = self.project_browser.get_selected_project_id()
                if project_id:
                    self.settings.setValue("ui/selectedProjectId", project_id)

            # Save active tab in file browser
            if hasattr(self.files_browser, 'tab_widget'):
                current_tab = self.files_browser.tab_widget.currentIndex()
                self.settings.setValue("ui/filesBrowserTab", current_tab)

            # Save column states
            if hasattr(self.project_browser, 'save_column_state'):
                project_columns = self.project_browser.save_column_state()
                if project_columns:
                    self.settings.setValue("columns/projectBrowser", project_columns)

            if hasattr(self.files_browser, 'save_column_state'):
                files_columns = self.files_browser.save_column_state()
                if files_columns:
                    self.settings.setValue("columns/filesBrowser/nuke", files_columns.get('nuke'))
                    self.settings.setValue("columns/filesBrowser/aep", files_columns.get('aep'))

            logger.info("UI settings saved successfully")
        except Exception as e:
            logger.error(f"Error saving settings: {e}")

    def _restore_settings(self):
        """Restore UI state from settings"""
        try:
            # Restore window geometry
            geometry = self.settings.value("geometry")
            if geometry:
                self.restoreGeometry(geometry)

            window_state = self.settings.value("windowState")
            if window_state:
                self.restoreState(window_state)

            # Restore splitter state
            if hasattr(self, 'project_splitter'):
                splitter_state = self.settings.value("splitter/state")
                if splitter_state:
                    self.project_splitter.restoreState(splitter_state)

            # Restore sidebar collapsed state
            sidebar_collapsed = self.settings.value("ui/sidebarCollapsed", False, type=bool)
            if sidebar_collapsed:
                self.sidebar.set_collapsed(True)

            # Restore current panel
            panel_index = self.settings.value("ui/currentPanel", 0, type=int)
            if 0 <= panel_index < self.stack.count():
                self.stack.setCurrentIndex(panel_index)
                self.sidebar.set_active_button(panel_index)

            # Restore file browser tab first (before column states)
            tab_index = self.settings.value("ui/filesBrowserTab", 0, type=int)
            if hasattr(self.files_browser, 'tab_widget'):
                if 0 <= tab_index < self.files_browser.tab_widget.count():
                    self.files_browser.tab_widget.setCurrentIndex(tab_index)

            # Restore column states (delayed to allow trees to fully initialize)
            from PySide6.QtCore import QTimer

            def restore_columns():
                try:
                    if hasattr(self.project_browser, 'restore_column_state'):
                        project_columns = self.settings.value("columns/projectBrowser")
                        if project_columns:
                            self.project_browser.restore_column_state(project_columns)
                            logger.info("Restored project browser columns from settings")
                        else:
                            logger.info("No saved project browser column state - using defaults")

                    if hasattr(self.files_browser, 'restore_column_state'):
                        nuke_columns = self.settings.value("columns/filesBrowser/nuke")
                        aep_columns = self.settings.value("columns/filesBrowser/aep")
                        if nuke_columns or aep_columns:
                            self.files_browser.restore_column_state({
                                'nuke': nuke_columns,
                                'aep': aep_columns
                            })
                            logger.info("Restored files browser columns from settings")
                        else:
                            logger.info("No saved files browser column state - using defaults")
                except Exception as e:
                    logger.error(f"Error in restore_columns: {e}")

            QTimer.singleShot(200, restore_columns)

            # Restore selected project (delayed to allow UI to fully initialize)
            project_id = self.settings.value("ui/selectedProjectId")
            if project_id:
                # Use QTimer to delay the selection until after the UI is fully loaded
                QTimer.singleShot(300, lambda: self._restore_project_selection(project_id))

            logger.info("UI settings restored successfully")
        except Exception as e:
            logger.error(f"Error restoring settings: {e}")

    def _restore_project_selection(self, project_id):
        """Restore the selected project in the project browser"""
        try:
            if hasattr(self.project_browser, 'select_project_by_id'):
                self.project_browser.select_project_by_id(project_id)
                logger.info(f"Restored project selection: {project_id}")
        except Exception as e:
            logger.error(f"Error restoring project selection: {e}")

    def closeEvent(self, event):
        """Handle window close event to save settings"""
        self._save_settings()
        event.accept()
