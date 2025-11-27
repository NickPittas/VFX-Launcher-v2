"""
File Watching Module

Purpose:
    Uses Watchdog to monitor configured directories for new or changed files.
    Triggers updates to the database when file changes are detected.

Requirements:
    - Efficient background thread (check interval ~1 min)
    - Real-time database updating and GUI refreshing
"""

import os
import time
import logging
from watchdog.observers import Observer
from watchdog.events import FileSystemEventHandler
from PySide6.QtCore import QObject, Signal, QThread, QMutex

from core.utils import extract_version, get_base_filename

# Configure logger
logger = logging.getLogger(__name__)


class FileEventHandler(FileSystemEventHandler):
    """
    Handles file system events detected by watchdog.
    """
    
    def __init__(self, db_manager, file_extensions=None):
        """
        Initialize the file event handler.
        
        Args:
            db_manager: Database manager instance
            file_extensions (list, optional): List of file extensions to monitor
        """
        super().__init__()
        self.db_manager = db_manager
        self.file_extensions = file_extensions or ['.nk', '.aep']
    
    def on_created(self, event):
        """
        Handle file creation event.
        
        Args:
            event: File system event
        """
        if event.is_directory:
            return
        
        if self._is_target_file(event.src_path):
            logger.info(f"New file detected: {event.src_path}")
            self._process_file(event.src_path)
    
    def on_modified(self, event):
        """
        Handle file modification event.
        
        Args:
            event: File system event
        """
        if event.is_directory:
            return
        
        if self._is_target_file(event.src_path):
            logger.info(f"Modified file detected: {event.src_path}")
            self._process_file(event.src_path)
    
    def on_moved(self, event):
        """
        Handle file move/rename event.
        
        Args:
            event: File system event
        """
        if event.is_directory:
            return
        
        # Check if destination is a target file
        if self._is_target_file(event.dest_path):
            logger.info(f"File moved/renamed to: {event.dest_path}")
            self._process_file(event.dest_path)
    
    def _is_target_file(self, filepath):
        """
        Check if the file has one of the target extensions.
        
        Args:
            filepath (str): Path to the file
            
        Returns:
            bool: True if file has target extension
        """
        _, ext = os.path.splitext(filepath)
        return ext.lower() in self.file_extensions
    
    def _process_file(self, filepath):
        """
        Process a detected file.
        Update project and file information in the database.
        
        Args:
            filepath (str): Path to the file
        """
        try:
            filename = os.path.basename(filepath)
            directory = os.path.dirname(filepath)
            project_name = os.path.basename(directory)
            
            # Extract version and file type
            version = extract_version(filename)
            file_ext = os.path.splitext(filename)[1].lower()
            filetype = file_ext[1:]  # Remove the dot
            
            # Get last modified time
            last_modified = os.path.getmtime(filepath)
            
            # Check if project exists
            project = self.db_manager.get_project_by_path(directory)
            
            # Create project if it doesn't exist
            if not project:
                project_id = self.db_manager.create_project(project_name, directory)
            else:
                project_id = project['id']
            
            # Add or update file
            if project_id:
                self.db_manager.add_project_file(
                    project_id,
                    filename,
                    filepath,
                    version,
                    filetype,
                    last_modified
                )
        
        except Exception as e:
            logger.error(f"Error processing file {filepath}: {str(e)}")


class FileWatcherSignals(QObject):
    """
    Signals for the FileWatcher class.
    """
    fileChanged = Signal(str)  # filepath
    error = Signal(str)  # error message


class FileWatcher(QThread):
    """
    Watches directories for file changes.
    Runs in a separate thread to avoid blocking the UI.
    """
    
    def __init__(self, db_manager, directories=None, interval=60):
        """
        Initialize the file watcher.
        
        Args:
            db_manager: Database manager instance
            directories (list, optional): List of directories to watch
            interval (int, optional): Check interval in seconds
        """
        super().__init__()
        self.db_manager = db_manager
        self.directories = directories or []
        self.interval = interval
        self.signals = FileWatcherSignals()
        self.mutex = QMutex()
        self.observer = None
        self.event_handler = None
        self.is_running = False
        self.is_paused = False
    
    def set_directories(self, directories):
        """
        Set directories to watch.
        
        Args:
            directories (list): List of directories to watch
        """
        self.mutex.lock()
        self.directories = directories
        self.mutex.unlock()
        
        # Restart observer if already running
        if self.is_running and not self.is_paused:
            self.restart()
    
    def set_interval(self, interval):
        """
        Set check interval.
        
        Args:
            interval (int): Check interval in seconds
        """
        self.mutex.lock()
        self.interval = interval
        self.mutex.unlock()
    
    def run(self):
        """
        Main thread method.
        Starts the watchdog observer and schedules the directories for watching.
        """
        self.is_running = True
        self.is_paused = False
        
        try:
            self.event_handler = FileEventHandler(self.db_manager)
            self.observer = Observer()
            
            # Schedule directories for watching
            for directory in self.directories:
                if os.path.exists(directory):
                    self.observer.schedule(self.event_handler, directory, recursive=True)
                    logger.info(f"Watching directory: {directory}")
                else:
                    logger.warning(f"Directory not found, cannot watch: {directory}")
            
            # Start observer
            self.observer.start()
            logger.info("File watcher started")
            
            # Keep thread alive
            while self.is_running:
                time.sleep(1)
                
                # Check if directories have changed
                if self.is_paused:
                    time.sleep(1)
                    continue
            
            # Stop observer when thread is stopped
            if self.observer and self.observer.is_alive():
                self.observer.stop()
                self.observer.join()
                logger.info("File watcher stopped")
        
        except Exception as e:
            error_msg = f"Error in file watcher: {str(e)}"
            logger.error(error_msg)
            self.signals.error.emit(error_msg)
            
            # Try to stop observer
            if self.observer and self.observer.is_alive():
                self.observer.stop()
                self.observer.join()
        
        finally:
            self.is_running = False
    
    def stop(self):
        """
        Stop the file watcher.
        """
        self.is_running = False
        logger.info("File watcher stop requested")
    
    def pause(self):
        """
        Pause the file watcher.
        """
        self.is_paused = True
        logger.info("File watcher paused")
    
    def resume(self):
        """
        Resume the file watcher.
        """
        self.is_paused = False
        logger.info("File watcher resumed")
    
    def restart(self):
        """
        Restart the file watcher.
        """
        if self.observer and self.observer.is_alive():
            self.observer.stop()
            self.observer.join()
        
        self.start()
        logger.info("File watcher restarted")
