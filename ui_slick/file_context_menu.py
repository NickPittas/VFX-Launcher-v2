"""
File Context Menu Module - Manages context menu operations for file browser items
"""
import os
import sys
import subprocess
import logging
from PySide6.QtWidgets import QMenu, QMessageBox, QApplication
from PySide6.QtCore import Qt, Signal

logger = logging.getLogger(__name__)

def open_in_file_manager(path):
    """Open a file or folder in the system file manager (Windows, macOS, Linux)."""
    if hasattr(os, 'startfile'):
        os.startfile(path)
    else:
        subprocess.Popen(['open' if sys.platform == 'darwin' else 'xdg-open', path])

class FileContextMenuManager:
    """Manages context menu operations for file items"""
    
    def __init__(self, parent=None):
        """Initialize with parent widget for UI operations"""
        self.parent = parent
        
    def show_context_menu(self, tree_widget, pos, file_action_callback=None):
        """
        Show context menu for the item at the specified position
        
        Args:
            tree_widget: The QTreeWidget containing the item
            pos: The position where the context menu was requested
            file_action_callback: Callback function for file operations (open, etc.)
        
        Returns:
            True if context menu was shown, False otherwise
        """
        try:
            # Get the item at the position
            item = tree_widget.itemAt(pos)
            if not item:
                logger.debug(f"[FileContextMenu] No item found at position: {pos}")
                return False
                
            # Debug information
            logger.debug(f"[FileContextMenu] Context menu requested for item: {item.text(0)}")
            
            # Try different approaches to get the filepath
            filepath = None
            
            # First try the standard data role
            filepath = item.data(0, 256)
            
            # If that fails, try the tooltip which often contains the path
            if not filepath:
                filepath = item.toolTip(0)
                
            # Log the result
            if filepath:
                logger.debug(f"[FileContextMenu] Found filepath: {filepath}")
            else:
                logger.warning(f"[FileContextMenu] Could not retrieve filepath for item: {item.text(0)}")
                return False
                
            # Create menu
            menu = QMenu(self.parent)
            
            # Open file action (same as double-click)
            open_action = menu.addAction("Open")
            if file_action_callback:
                open_action.triggered.connect(lambda checked=False, path=filepath: file_action_callback(path))
            
            # Open containing folder action
            open_folder_action = menu.addAction("Open Containing Folder")
            open_folder_action.triggered.connect(lambda checked=False, path=filepath: self.open_containing_folder(path))
            
            # Add separator
            menu.addSeparator()
            
            # Copy file path action
            copy_path_action = menu.addAction("Copy File Path")
            copy_path_action.triggered.connect(lambda checked=False, path=filepath: self.copy_to_clipboard(path))
            
            # Show file info action
            file_info_action = menu.addAction("Show File Info")
            file_info_action.triggered.connect(lambda checked=False, path=filepath: self.show_file_info(path))
            
            # Show the menu
            menu.exec(tree_widget.viewport().mapToGlobal(pos))
            return True
            
        except Exception as e:
            logger.error(f"[FileContextMenu] Error showing context menu: {str(e)}")
            return False
            
    def open_containing_folder(self, filepath):
        """Open the folder containing the file"""
        try:
            # Get the directory path
            folder_path = os.path.dirname(filepath)
            
            # Check if folder exists
            if not os.path.isdir(folder_path):
                QMessageBox.warning(self.parent, "Folder Not Found", f"The folder does not exist:\n{folder_path}")
                return False
                
            # Open the folder in the system file manager
            open_in_file_manager(folder_path)
            logger.info(f"[FileContextMenu] Opened containing folder: {folder_path}")
            return True
            
        except Exception as e:
            logger.error(f"[FileContextMenu] Error opening containing folder: {str(e)}")
            QMessageBox.warning(self.parent, "Error", f"Failed to open folder:\n{str(e)}")
            return False
            
    def copy_to_clipboard(self, text):
        """Copy text to clipboard"""
        try:
            # Get clipboard from QApplication
            clipboard = QApplication.clipboard()
            # Set text to clipboard
            clipboard.setText(text)
            logger.info(f"[FileContextMenu] Copied to clipboard: {text}")
            return True
        except Exception as e:
            logger.error(f"[FileContextMenu] Error copying to clipboard: {str(e)}")
            return False
            
    def show_file_info(self, filepath):
        """Show detailed file information"""
        try:
            # Check if file exists
            if not os.path.isfile(filepath):
                QMessageBox.warning(self.parent, "File Not Found", f"The file does not exist:\n{filepath}")
                return False
                
            # Get file stats
            file_stat = os.stat(filepath)
            file_size = file_stat.st_size
            mod_time = file_stat.st_mtime
            create_time = file_stat.st_ctime
            
            # Format timestamps
            from datetime import datetime
            mod_time_str = datetime.fromtimestamp(mod_time).strftime('%Y-%m-%d %H:%M:%S')
            create_time_str = datetime.fromtimestamp(create_time).strftime('%Y-%m-%d %H:%M:%S')
            
            # Format file size
            if file_size < 1024:
                size_str = f"{file_size} bytes"
            elif file_size < 1024*1024:
                size_str = f"{file_size/1024:.2f} KB"
            else:
                size_str = f"{file_size/(1024*1024):.2f} MB"
            
            # Show info dialog
            QMessageBox.information(
                self.parent,
                "File Information",
                f"Name: {os.path.basename(filepath)}\n"
                f"Path: {os.path.dirname(filepath)}\n\n"
                f"Size: {size_str}\n"
                f"Created: {create_time_str}\n"
                f"Modified: {mod_time_str}"
            )
            return True
            
        except Exception as e:
            logger.error(f"[FileContextMenu] Error showing file info: {str(e)}")
            QMessageBox.warning(self.parent, "Error", f"Failed to get file information:\n{str(e)}")
            return False
