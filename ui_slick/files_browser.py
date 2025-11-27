"""
Files Browser Module (Slick UI) - Enhanced with modularity
"""
import os
import logging
from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QTreeWidget, QTreeWidgetItem,
    QTabWidget, QLabel, QHBoxLayout, QLineEdit, QPushButton,
    QMessageBox, QDialog, QApplication
)
from PySide6.QtCore import Signal, Qt, QThreadPool, QSize, QTimer
from PySide6.QtGui import QIcon
from PySide6.QtSvg import QSvgRenderer  # Required for SVG icon support

# Import modular components
from .tree_manager import FileTreeManager
from .file_handlers import FileHandler
from .scan_progress_dialog import ScanProgressDialog
from .file_context_menu import FileContextMenuManager

logger = logging.getLogger(__name__)

class FilesBrowser(QWidget):
    """Modern file browser with version support for VFX files"""
    file_launched = Signal(str)  # Emits filepath when a file is launched
    file_selected = Signal(dict)  # Emits file data when selected

    def __init__(self, db_manager=None, user_data=None, parent=None):
        super().__init__(parent)
        self.db_manager = db_manager
        self.user_data = user_data
        self._setup_ui()
        self._connect_signals()
        self._last_files = []
        self._thread_pool = QThreadPool()
        
        # Create tree managers
        self.nuke_tree_manager = FileTreeManager(self.nuke_tree)
        self.aep_tree_manager = FileTreeManager(self.aep_tree)
        
        # Create context menu manager
        self.context_menu_manager = FileContextMenuManager(self)
        
        # Track current project path and files
        self.current_project_path = None
        self.current_project_id = None
        self.scan_dialog = None
        self.scan_timer = None
        
    def _setup_ui(self):
        """Set up the UI components"""
        try:
            main_layout = QVBoxLayout(self)
            self.setLayout(main_layout)
            
            # Header with message and search
            header_layout = QHBoxLayout()
            
            # Message label
            self.message_label = QLabel("Select a project to view files")
            self.message_label.setStyleSheet("font-weight: 500; font-size: 14px; color: #f0f0f0;")
            header_layout.addWidget(self.message_label)
            
            # Search box
            self.search_box = QLineEdit()
            self.search_box.setPlaceholderText("Search files...")
            self.search_box.setMaximumWidth(200)
            header_layout.addWidget(self.search_box)
            
            # Scan button
            self.scan_button = QPushButton("Scan for Updates")
            self.scan_button.setToolTip("Scan for new or updated files in this project")
            self.scan_button.setFixedWidth(130)
            self.scan_button.setStyleSheet("""
                QPushButton {
                    background-color: #2563eb;
                    color: white;
                    border-radius: 4px;
                    padding: 5px 10px;
                    font-weight: 500;
                }
                QPushButton:hover {
                    background-color: #1d4ed8;
                }
                QPushButton:pressed {
                    background-color: #1e40af;
                }
            """)
            header_layout.addWidget(self.scan_button)
            
            main_layout.addLayout(header_layout)
            
            # Tab widget for file types
            self.tab_widget = QTabWidget()
            main_layout.addWidget(self.tab_widget)
            
            # Nuke tab with properly configured tree
            self.nuke_tree = QTreeWidget()
            self.nuke_tree.setColumnCount(3)
            self.nuke_tree.setHeaderLabels(["Nuke Files", "Last Opened", "Opened By"])
            self.nuke_tree.setSelectionMode(QTreeWidget.ExtendedSelection)
            self._configure_tree_widget(self.nuke_tree)

            # Add Nuke tab with icon
            nuke_icon_path = os.path.join(os.path.dirname(__file__), "icons", "nuke.svg")
            if os.path.exists(nuke_icon_path):
                nuke_icon = QIcon(nuke_icon_path)
                logger.info(f"Loaded Nuke tab icon from: {nuke_icon_path}")
            else:
                nuke_icon = QIcon()
                logger.warning(f"Nuke icon not found: {nuke_icon_path}")
            self.tab_widget.addTab(self.nuke_tree, nuke_icon, "Nuke Files")

            # AEP tab with properly configured tree
            self.aep_tree = QTreeWidget()
            self.aep_tree.setColumnCount(3)
            self.aep_tree.setHeaderLabels(["After Effects Files", "Last Opened", "Opened By"])
            self.aep_tree.setSelectionMode(QTreeWidget.ExtendedSelection)
            self._configure_tree_widget(self.aep_tree)

            # Add AEP tab with icon
            ae_icon_path = os.path.join(os.path.dirname(__file__), "icons", "after-effects.svg")
            if os.path.exists(ae_icon_path):
                ae_icon = QIcon(ae_icon_path)
                logger.info(f"Loaded AE tab icon from: {ae_icon_path}")
            else:
                ae_icon = QIcon()
                logger.warning(f"AE icon not found: {ae_icon_path}")
            self.tab_widget.addTab(self.aep_tree, ae_icon, "AEP Files")
            
            # Apply stylesheet
            self._apply_stylesheet()
        
        except Exception as e:
            logger.error(f"[FilesBrowser] Error setting up UI: {str(e)}")
    
    def _connect_signals(self):
        """Connect UI signals to slots"""
        try:
            # Tree item double click
            self.nuke_tree.itemDoubleClicked.connect(self._on_file_double_clicked)
            self.aep_tree.itemDoubleClicked.connect(self._on_file_double_clicked)
            
            # Set up context menus for tree widgets
            self.nuke_tree.setContextMenuPolicy(Qt.CustomContextMenu)
            self.aep_tree.setContextMenuPolicy(Qt.CustomContextMenu)
            self.nuke_tree.customContextMenuRequested.connect(lambda pos: self._show_context_menu(self.nuke_tree, pos))
            self.aep_tree.customContextMenuRequested.connect(lambda pos: self._show_context_menu(self.aep_tree, pos))
            
            # Search box
            self.search_box.textChanged.connect(self._on_search_changed)
            
            # Scan button
            self.scan_button.clicked.connect(self._on_scan_button_clicked)
        
        except Exception as e:
            logger.error(f"[FilesBrowser] Error connecting signals: {str(e)}")
    
    def _configure_tree_widget(self, tree_widget):
        """Configure a tree widget with proper expand/collapse behavior"""
        try:
            # Set properties for proper expand/collapse functionality
            tree_widget.setRootIsDecorated(True)  # Show expand/collapse decorations
            tree_widget.setItemsExpandable(True)  # Allow items to be expanded
            tree_widget.setIndentation(20)  # Proper indentation for hierarchy
            tree_widget.setAnimated(True)  # Smooth animations for expand/collapse
            tree_widget.setUniformRowHeights(False)  # Allow custom row heights
            tree_widget.setAlternatingRowColors(False)  # Consistent row colors

            # Set minimum column widths to prevent them from being too small
            tree_widget.header().setMinimumSectionSize(500)  # Minimum 500px for any column

            # Ensure proper spacing and layout - very wide columns for long filenames
            tree_widget.setColumnWidth(0, 3000)  # File name column - 5x wider for very long filenames
            tree_widget.setColumnWidth(1, 180)  # Last Opened column
            tree_widget.setColumnWidth(2, 150)  # Opened By column
            tree_widget.header().setStretchLastSection(False)  # Don't stretch - use fixed widths
            tree_widget.setIconSize(QSize(16, 16))  # Consistent icon size

            # Custom properties for expand/collapse indicators
            tree_widget.setExpandsOnDoubleClick(False)  # Don't expand on file double-click

            logger.info(f"Configured tree widget with filename column width: {tree_widget.columnWidth(0)}px")

        except Exception as e:
            logger.error(f"[FilesBrowser] Error configuring tree widget: {str(e)}")
    
    def _apply_stylesheet(self):
        """Apply stylesheet to components"""
        try:
            # Tree widgets - dark theme with proper expand/collapse arrows
            tree_style = """
            QTreeWidget {
                border: 1px solid #444444;
                border-radius: 4px;
                background-color: #2a2a2a;
                color: #f0f0f0;
                show-decoration-selected: 1;
            }
            QTreeWidget::item {
                padding: 6px 4px;
                border-bottom: 1px solid #333333;
            }
            QTreeWidget::item:selected {
                background-color: #3d3d3d;
                color: #ffffff;
            }
            QTreeWidget::branch {
                background-color: #2a2a2a;
            }
            QTreeWidget::branch:has-children:!has-siblings:closed,
            QTreeWidget::branch:closed:has-children:has-siblings {
                image: url(:/images/down_arrow_disabled.png);
            }
            QTreeWidget::branch:open:has-children:!has-siblings,
            QTreeWidget::branch:open:has-children:has-siblings {
                image: url(:/images/down_arrow.png);
            }
            QHeaderView::section {
                background-color: #252525;
                color: #f0f0f0;
                border: 1px solid #444444;
                padding: 4px;
            }
            """
            
            # Apply style to both trees
            self.nuke_tree.setStyleSheet(tree_style)
            self.aep_tree.setStyleSheet(tree_style)
            
            # Search box - dark theme
            self.search_box.setStyleSheet("""
            QLineEdit {
                padding: 5px;
                border: 1px solid #444444;
                border-radius: 4px;
                background-color: #333333;
                color: #f0f0f0;
            }
            """)
            
            # Message label - dark theme
            self.message_label.setStyleSheet("""
                font-weight: 500;
                font-size: 14px;
                color: #f0f0f0;
            """)
        
        except Exception as e:
            logger.error(f"[FilesBrowser] Error applying stylesheet: {str(e)}")
    
    def _on_file_double_clicked(self, item, column):
        """Handle file item double click to launch file"""
        try:
            # Get filepath and file_id from item data
            filepath = item.data(0, 256)
            file_id = item.data(0, 257)

            if filepath:
                # Log file access if we have db_manager and user_data
                if self.db_manager and self.user_data and file_id:
                    user_id = self.user_data.get('id')
                    if user_id:
                        self.db_manager.log_file_access(file_id, user_id)
                        logger.info(f"[FilesBrowser] Logged file access: file_id={file_id}, user_id={user_id}")

                        # Refresh the file display to show updated access info
                        self._refresh_current_files()

                self._on_open_file(filepath)
        except Exception as e:
            logger.error(f"[FilesBrowser] Error handling file double click: {str(e)}")
            
    def _on_open_file(self, filepath):
        """Open the file using the appropriate application"""
        try:
            # Emit signal to launch the file
            logger.info(f"[FilesBrowser] Opening file: {filepath}")
            self.file_launched.emit(filepath)
        except Exception as e:
            logger.error(f"[FilesBrowser] Error opening file: {str(e)}")
            QMessageBox.warning(self, "Error", f"Failed to open file:\n{str(e)}")
            
    def _show_context_menu(self, tree_widget, pos):
        """Show context menu for file tree item"""
        try:
            # Get the item at the position to extract file_id
            item = tree_widget.itemAt(pos)

            # Create a wrapper callback that includes file_id
            def open_file_with_logging(filepath):
                if item:
                    file_id = item.data(0, 257)
                    # Log file access if we have db_manager and user_data
                    if self.db_manager and self.user_data and file_id:
                        user_id = self.user_data.get('id')
                        if user_id:
                            self.db_manager.log_file_access(file_id, user_id)
                            logger.info(f"[FilesBrowser] Logged file access from context menu: file_id={file_id}, user_id={user_id}")

                            # Refresh the file display to show updated access info
                            self._refresh_current_files()

                self._on_open_file(filepath)

            # Use the context menu manager to show the context menu
            self.context_menu_manager.show_context_menu(
                tree_widget,
                pos,
                file_action_callback=open_file_with_logging
            )
        except Exception as e:
            logger.error(f"[FilesBrowser] Error showing context menu: {str(e)}")
            
    # These functions are now handled by FileContextMenuManager
    
    def _on_search_changed(self, text):
        """Filter files based on search text"""
        if not text:
            # Reset to show all files
            self.display_files(self._last_files)
            return
            
        try:
            # Filter files based on search text
            text = text.lower()
            filtered_files = [
                f for f in self._last_files 
                if text in f.get('filename', '').lower() or 
                   text in f.get('filepath', '').lower() or 
                   text in f.get('shot_name', '').lower()
            ]
            
            # Display filtered files
            self.display_filtered_files(filtered_files)
        except Exception as e:
            logger.error(f"[FilesBrowser] Error filtering files: {str(e)}")
    
    def set_current_project(self, project_path, files=None):
        """Set the current project path and optionally display files"""
        try:
            logger.info(f"[FilesBrowser] Setting current project path to: {project_path}")
            self.current_project_path = project_path

            # Get and store the project_id
            if self.db_manager and project_path:
                project = self.db_manager.get_project_by_path(project_path)
                if project:
                    self.current_project_id = project['id']
                    logger.info(f"[FilesBrowser] Stored project_id: {self.current_project_id}")
                else:
                    self.current_project_id = None
            else:
                self.current_project_id = None

            # Update the message label with the project name
            if project_path:
                project_name = os.path.basename(project_path)
                self.message_label.setText(f"Project: {project_name}")

                # Store the path in the parent window for persistence
                if self.parent() and hasattr(self.parent(), 'current_project_path'):
                    self.parent().current_project_path = project_path
            else:
                self.message_label.setText("Select a project to view files")

            # If files were provided, display them
            if files:
                self.display_files(files)
        except Exception as e:
            logger.error(f"[FilesBrowser] Error setting current project: {str(e)}")

    def _refresh_current_files(self):
        """Reload files from database to refresh access info"""
        try:
            if self.db_manager and self.current_project_id:
                logger.info(f"[FilesBrowser] Refreshing files for project_id: {self.current_project_id}")

                # Reload files with updated access info
                files = self.db_manager.get_files_with_access_info(self.current_project_id)

                # Add base_name to files (same as main_window does)
                for file in files:
                    filename = file.get('filename', '')
                    # Extract base name (without version)
                    import re
                    base_name = re.sub(r'_v\d{1,4}', '', filename)
                    base_name = os.path.splitext(base_name)[0]
                    file['base_name'] = base_name

                # Display the refreshed files
                self.display_files(files)
                logger.info(f"[FilesBrowser] Files refreshed successfully")
            else:
                logger.warning(f"[FilesBrowser] Cannot refresh: db_manager={self.db_manager is not None}, project_id={self.current_project_id}")
        except Exception as e:
            logger.error(f"[FilesBrowser] Error refreshing files: {str(e)}")

    def _on_scan_button_clicked(self):
        """
        Trigger a scan for new or updated files in the current project.
        Only scans existing known folders without redoing the folder matching phase.
        """
        try:
            # Look for project path - use a simpler, more direct approach
            # First check our local attribute
            project_path = None
            
            if hasattr(self, 'current_project_path') and self.current_project_path:
                project_path = self.current_project_path
                logger.info(f"[FilesBrowser] Using stored project path: {project_path}")
            # Try to get it directly from parent's current_project_path
            elif self.parent() and hasattr(self.parent(), 'current_project_path'):
                project_path = self.parent().current_project_path
                self.current_project_path = project_path  # Store it for future use
                logger.info(f"[FilesBrowser] Using parent's project path: {project_path}")
                
            # Try to get it from currently selected project in the project browser
            elif self.parent() and hasattr(self.parent(), 'current_project_id'):
                # Get the ID from the parent and use it for debugging
                project_id = self.parent().current_project_id
                logger.info(f"[FilesBrowser] Found project ID in parent: {project_id}, but no direct path available")
                
                # Try to get a reference to the project browser to access its selected project
                if hasattr(self.parent(), 'project_browser'):
                    project_browser = self.parent().project_browser
                    if hasattr(project_browser, 'get_selected_project'):
                        project = project_browser.get_selected_project()
                        if project and 'path' in project:
                            project_path = project['path']
                            self.current_project_path = project_path  # Store it for future use
                            logger.info(f"[FilesBrowser] Retrieved project path from project browser: {project_path}")
            
            # Last resort: Check the main_window.py's current_project_id
            elif self.parent() and hasattr(self.parent(), 'db_manager'):
                try:
                    # Try to get the path directly from the database
                    if hasattr(self.parent(), 'current_project_id') and self.parent().current_project_id:
                        project_id = self.parent().current_project_id
                        db_manager = self.parent().db_manager
                        
                        # Directly query the database
                        with db_manager.get_connection() as conn:
                            cursor = conn.cursor()
                            cursor.execute("SELECT path FROM projects WHERE id = ?", (project_id,))
                            result = cursor.fetchone()
                            
                            if result and 'path' in result:
                                project_path = result['path']
                                self.current_project_path = project_path  # Store it for future use
                                logger.info(f"[FilesBrowser] Retrieved project path directly from database: {project_path}")
                except Exception as e:
                    logger.error(f"[FilesBrowser] Error getting project path from database: {str(e)}")
            
            # Final check before proceeding
            if not project_path:
                logger.warning("[FilesBrowser] No project loaded, can't determine project path for scan")
                QMessageBox.warning(self, "Scan Failed", "No project loaded. Please open a project first.")
                return
                
            # Confirm scan
            project_name = os.path.basename(project_path)
            reply = QMessageBox.question(
                self, 
                "Scan Project", 
                f"Scan '{project_name}' for new or updated files?",
                QMessageBox.Yes | QMessageBox.No
            )
            
            if reply != QMessageBox.Yes:
                return
                
            # Import Scanner only when needed to avoid circular imports
            from ui_slick.scanner import Scanner
            
            # Get database manager from parent window if available
            db_manager = None
            if self.parent() and hasattr(self.parent(), 'db_manager'):
                db_manager = self.parent().db_manager
                logger.info(f"[FilesBrowser] Using database manager from parent")
            
            # If no database manager from parent, create our own
            if not db_manager:
                try:
                    from core.database import DatabaseManager
                    db_path = os.path.join(os.path.dirname(__file__), '..', 'vfx_launcher.db')
                    db_manager = DatabaseManager(db_path)
                    logger.info(f"[FilesBrowser] Created new database manager with path: {db_path}")
                except Exception as e:
                    logger.error(f"[FilesBrowser] Failed to create database manager: {str(e)}")
            
            # Initialize scanner with database manager
            scanner = Scanner(db_manager=db_manager)
            
            # Create and show progress dialog
            self.scan_dialog = ScanProgressDialog(self)
            self.scan_dialog.setWindowTitle(f"Scanning: {project_name}")
            
            # Set up timeout timer
            self.scan_timer = QTimer(self)
            self.scan_timer.setSingleShot(True)
            self.scan_timer.timeout.connect(self._on_scan_timeout)
            self.scan_timer.start(600000)  # 10 minute timeout
            
            # Show the dialog
            self.scan_dialog.show()
            
            # Store scanner instance to enable cancellation
            self.current_scanner = scanner
            
            # Connect cancel button to scanner's stop method
            if hasattr(self.scan_dialog, 'cancel_requested'):
                self.scan_dialog.cancel_requested.connect(self._cancel_current_scan)
            
            # Look for correct project path - check for "Projects" subfolder first
            projects_subfolder = os.path.join(project_path, "Projects")
            if os.path.isdir(projects_subfolder):
                logger.info(f"[FilesBrowser] Found 'Projects' subfolder, using it for scan: {projects_subfolder}")
                scan_path = projects_subfolder
            else:
                logger.info(f"[FilesBrowser] No 'Projects' subfolder found, using project root: {project_path}")
                scan_path = project_path
            
            # Start the scan with proper callbacks
            scanner.quick_scan_project(
                scan_path,  # Use the correct project path with Projects subfolder
                on_progress=self.scan_dialog.update_progress if hasattr(self.scan_dialog, 'update_progress') else None,
                on_folder_found=None,  # We're only scanning known folders
                on_file_found=self.scan_dialog.on_file_found if hasattr(self.scan_dialog, 'on_file_found') else None,
                on_log=self.scan_dialog.add_log if hasattr(self.scan_dialog, 'add_log') else None,
                on_finished=self._on_scan_completed,  # Scanner passes (project_path, files) to this callback
                on_error=lambda e: self.scan_dialog.add_log(f"Error: {str(e)}", "error") if hasattr(self.scan_dialog, 'add_log') else None
            )
            
        except Exception as e:
            logger.error(f"[FilesBrowser] Error starting scan: {str(e)}")
            QMessageBox.critical(self, "Scan Error", f"Error starting scan: {str(e)}")
            if hasattr(self, 'scan_dialog') and self.scan_dialog:
                self.scan_dialog.accept()
                
    def _on_scan_completed(self, project_path, files):
        """Called when scanner completes the scan"""
        try:
            logger.info(f"[FilesBrowser] Scan completed for {project_path} with {len(files)} files found")
            
            # Stop the timeout timer
            if hasattr(self, 'scan_timer') and self.scan_timer:
                self.scan_timer.stop()
            
            # Update dialog to show completion
            if hasattr(self, 'scan_dialog') and self.scan_dialog:
                if hasattr(self.scan_dialog, 'add_log'):
                    self.scan_dialog.add_log(f"Scan complete: {len(files)} files found")
                if hasattr(self.scan_dialog, '_convert_to_close_button'):
                    self.scan_dialog._convert_to_close_button()
            
            # Log all files for debugging
            logger.debug(f"[FilesBrowser] Files found in scan: {files[:10]}..." if len(files) > 10 else f"[FilesBrowser] Files found in scan: {files}")
            
            # Process and display the files
            self.display_files(files)
            
            # Show success message
            self.message_label.setText(f"Scan complete: {len(files)} files found")
            
        except Exception as e:
            logger.error(f"[FilesBrowser] Error processing scan results: {str(e)}")
            if hasattr(self, 'scan_dialog') and self.scan_dialog:
                self.scan_dialog.add_log(f"Error processing results: {str(e)}", "error")
                
    def _on_scan_timeout(self):
        """Handle scan operation timeout"""
        if hasattr(self, 'scan_dialog') and self.scan_dialog:
            self.scan_dialog.add_log("Scan operation timed out after 10 minutes", "error")
            self.scan_dialog._convert_to_close_button()
            
        logger.error("[FilesBrowser] Scan operation timed out after 10 minutes")
        
    def _cancel_current_scan(self):
        """Cancel the current scanning operation"""
        try:
            logger.info("[FilesBrowser] User requested scan cancellation")
            
            # Stop the scanner if it exists
            if hasattr(self, 'current_scanner') and self.current_scanner:
                logger.info("[FilesBrowser] Stopping scanner...")
                self.current_scanner._stop_requested = True
                
                # Attempt to stop any active workers
                if hasattr(self.current_scanner, 'folder_match_worker') and self.current_scanner.folder_match_worker:
                    self.current_scanner.folder_match_worker.stop()
                
                if hasattr(self.current_scanner, 'file_search_workers'):
                    for worker in self.current_scanner.file_search_workers:
                        if worker:
                            worker.stop()
                            
                # Log the cancellation
                if hasattr(self, 'scan_dialog') and self.scan_dialog and hasattr(self.scan_dialog, 'add_log'):
                    self.scan_dialog.add_log("Scan cancellation requested. Stopping operations...")
                    
                # Convert to close button after cancellation
                if hasattr(self, 'scan_dialog') and self.scan_dialog and hasattr(self.scan_dialog, '_convert_to_close_button'):
                    self.scan_dialog._convert_to_close_button()
        except Exception as e:
            logger.error(f"[FilesBrowser] Error cancelling scan: {str(e)}")
            if hasattr(self, 'scan_dialog') and self.scan_dialog and hasattr(self.scan_dialog, 'add_log'):
                self.scan_dialog.add_log(f"Error during cancellation: {str(e)}", "error")
    
    def _display_filtered_files(self, files):
        """Display filtered files without updating _last_files"""
        try:
            # Clear tree widgets
            self.nuke_tree_manager.clear()
            self.aep_tree_manager.clear()
            
            # Update message
            self.message_label.setText(f"Found {len(files)} matching files")
            
            # Filter files by type
            nuke_files = [f for f in files if f.get('filetype', '').lower() == 'nk']
            aep_files = [f for f in files if f.get('filetype', '').lower() == 'aep']
            
            # Populate trees
            self.nuke_tree_manager.populate_tree(nuke_files)
            self.aep_tree_manager.populate_tree(aep_files)
        
        except Exception as e:
            logger.error(f"[FilesBrowser] Error displaying filtered files: {str(e)}")
    
    def display_files(self, files):
        """Display files in the tree widgets"""
        try:
            # Log received files count for debugging
            logger.debug(f"[FilesBrowser] display_files called with {len(files)} files")
            
            # Store files for filtering/search
            self._last_files = files
            
            # Clear trees
            self.nuke_tree_manager.clear()
            self.aep_tree_manager.clear()
            
            if not files:
                self.message_label.setText("No files found in selected project")
                return
            
            self.message_label.setText(f"Displaying {len(files)} files")
            
            # Filter files by type
            nuke_files = [f for f in files if f.get('filetype', '').lower() == 'nk']
            aep_files = [f for f in files if f.get('filetype', '').lower() == 'aep']
            
            # Log counts by file type
            logger.debug(f"[FilesBrowser] Found {len(nuke_files)} Nuke files and {len(aep_files)} AEP files")
            
            # Populate trees with files, handling versions
            self.nuke_tree_manager.populate_tree(nuke_files)
            self.aep_tree_manager.populate_tree(aep_files)
            
            # Select first tab with files
            if nuke_files:
                self.tab_widget.setCurrentIndex(0)
            elif aep_files:
                self.tab_widget.setCurrentIndex(1)

            # Let the trees use their default collapsed state as configured in tree_manager.py
            # Groups will remain collapsed until the user expands them

            # Reset horizontal scroll to left for both trees
            self.nuke_tree.horizontalScrollBar().setValue(0)
            self.aep_tree.horizontalScrollBar().setValue(0)

        except Exception as e:
            logger.error(f"[FilesBrowser] Error displaying files: {str(e)}")
    
    def clear(self):
        """Clear all tree widgets"""
        try:
            self.nuke_tree_manager.clear()
            self.aep_tree_manager.clear()
            self._last_files = []
            self.message_label.setText("Select a project to view files")
        except Exception as e:
            logger.error(f"[FilesBrowser] Error clearing trees: {str(e)}")

    def save_column_state(self):
        """Save the column widths and state for both trees"""
        try:
            nuke_header_state = self.nuke_tree.header().saveState()
            aep_header_state = self.aep_tree.header().saveState()
            return {
                'nuke': nuke_header_state,
                'aep': aep_header_state
            }
        except Exception as e:
            logger.error(f"Error saving files browser column state: {e}")
            return None

    def restore_column_state(self, state):
        """Restore the column widths and state for both trees"""
        try:
            if state:
                if 'nuke' in state and state['nuke']:
                    result = self.nuke_tree.header().restoreState(state['nuke'])
                    logger.info(f"Nuke tree column state restored: {result}")

                    # Ensure all columns are visible
                    for col in range(self.nuke_tree.columnCount()):
                        self.nuke_tree.setColumnHidden(col, False)

                    # FORCE filename column to be wide - override any saved state
                    self.nuke_tree.setColumnWidth(0, 3000)
                    logger.info("Forced Nuke filename column to 3000px")

                    # Reset horizontal scroll to left
                    self.nuke_tree.horizontalScrollBar().setValue(0)
                else:
                    logger.warning("No column state to restore for Nuke tree")

                if 'aep' in state and state['aep']:
                    result = self.aep_tree.header().restoreState(state['aep'])
                    logger.info(f"AEP tree column state restored: {result}")

                    # Ensure all columns are visible
                    for col in range(self.aep_tree.columnCount()):
                        self.aep_tree.setColumnHidden(col, False)

                    # FORCE filename column to be wide - override any saved state
                    self.aep_tree.setColumnWidth(0, 3000)
                    logger.info("Forced AEP filename column to 3000px")

                    # Reset horizontal scroll to left
                    self.aep_tree.horizontalScrollBar().setValue(0)
                else:
                    logger.warning("No column state to restore for AEP tree")
            else:
                logger.warning("No column state dictionary provided")
        except Exception as e:
            logger.error(f"Error restoring files browser column state: {e}")