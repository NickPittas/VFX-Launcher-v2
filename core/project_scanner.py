"""
Project Scanning Module

Purpose:
    Scans configured directories recursively for .nk and .aep files.
    Parses filenames to identify project names and version numbers.
    Updates database with file paths and versions.

Requirements:
    - Asynchronous operation (no UI blocking)
    - Accurate regex parsing for version detection (_v###)
    - Updates database with file paths and versions
"""

import os
import re
import logging
import time
from datetime import datetime
from PySide6.QtCore import QObject, Signal, QRunnable, QThreadPool, Slot

from core.utils import extract_version, get_base_filename

# Configure logger
logger = logging.getLogger(__name__)

class ScannerSignals(QObject):
    """
    Signals for the ProjectScanner class.
    """
    started = Signal()
    progress = Signal(str, int, int)  # message, current, total
    finished = Signal(list)  # list of scanned files
    error = Signal(str)  # error message


class ProjectScanner(QRunnable):
    """
    Scans directories for VFX project files (.nk and .aep).
    Runs in a separate thread to avoid blocking the UI.
    """
    
    def __init__(self, db_manager, directories, file_extensions=None):
        """
        Initialize the project scanner.
        
        Args:
            db_manager: Database manager instance
            directories (list): List of directories to scan
            file_extensions (list, optional): List of file extensions to scan for
        """
        super().__init__()
        self.db_manager = db_manager
        self.directories = directories
        self.file_extensions = file_extensions or ['.nk', '.aep']
        self.signals = ScannerSignals()
        self.stop_requested = False
    
    def stop(self):
        """Request the scanner to stop"""
        self.stop_requested = True
        
    @Slot()
    def run(self):
        """
        Main scanning method that runs in a separate thread.
        Scans all configured directories and updates the database.
        """
        try:
            self.signals.started.emit()
            logger.info(f"Starting project scan in directories: {', '.join(self.directories)}")
            
            all_files = []
            total_dirs = len(self.directories)
            
            for dir_index, directory in enumerate(self.directories):
                if self.stop_requested:
                    logger.info("Scan stopped by user request")
                    break
                    
                if not os.path.exists(directory):
                    if not directory.strip():
                        logger.info("[ProjectScanner] Skipping empty directory entry in configuration.")
                    else:
                        logger.info(f"[ProjectScanner] Directory not found (skipped): {directory}")
                    continue
                
                self.signals.progress.emit(f"Scanning {directory}...", dir_index, total_dirs)
                logger.info(f"Scanning directory: {directory}")
                
                # Scan this directory
                directory_files = self._scan_directory(directory)
                all_files.extend(directory_files)
            
            # Update database with found files
            updated_count = self._update_database(all_files)
            
            logger.info(f"Scan completed. Found {len(all_files)} files, updated {updated_count} in database.")
            self.signals.finished.emit(all_files)
            
        except Exception as e:
            error_msg = f"Error during project scan: {str(e)}"
            logger.error(error_msg)
            self.signals.error.emit(error_msg)
    
    def _scan_directory(self, directory):
        """
        Recursively scan a project root directory for VFX project files.
        Only associates files with the given root (never subfolders as projects).
        Args:
            directory (str): Project root directory to scan
        Returns:
            list: List of found file dictionaries (all linked to root)
        """
        found_files = []
        project_name = os.path.basename(directory)
        project_path = directory
        for root, dirs, files in os.walk(directory):
            if self.stop_requested:
                break
            for file in files:
                if self.stop_requested:
                    break
                file_ext = os.path.splitext(file)[1].lower()
                if file_ext in self.file_extensions:
                    filepath = os.path.join(root, file)
                    version = extract_version(file)
                    base_filename = get_base_filename(file)
                    last_modified = os.path.getmtime(filepath)
                    found_files.append({
                        'filename': file,
                        'filepath': filepath,
                        'version': version,
                        'filetype': file_ext[1:],
                        'last_modified': last_modified,
                        'project_name': project_name,
                        'project_path': project_path
                    })
        return found_files
    
    def _update_database(self, files):
        """
        Update database with found files.
        Only creates/checks projects at the configured root level.
        Adds logging for every project creation and file association.
        Args:
            files (list): List of file dictionaries
        Returns:
            int: Number of files updated in database
        """
        updated_count = 0
        for file_info in files:
            if self.stop_requested:
                break
            try:
                # Only create/check project at the root level
                project = self.db_manager.get_project_by_path(file_info['project_path'])
                if not project:
                    project_id = self.db_manager.create_project(
                        file_info['project_name'],
                        file_info['project_path']
                    )
                    logger.info(f"[DB] Created project: {file_info['project_name']} at {file_info['project_path']}")
                else:
                    project_id = project['id']
                # Add or update file
                if project_id:
                    self.db_manager.add_project_file(
                        project_id,
                        file_info['filename'],
                        file_info['filepath'],
                        file_info['version'],
                        file_info['filetype'],
                        file_info['last_modified']
                    )
                    logger.info(f"[DB] Associated file '{file_info['filename']}' with project '{file_info['project_name']}' ({file_info['project_path']})")
                    updated_count += 1
            except Exception as e:
                logger.error(f"Error updating database for file {file_info['filepath']}: {str(e)}")
        return updated_count

    # Database constraint suggestion (add to schema migration):
    #   - Make 'project_path' UNIQUE in the projects table to prevent duplicates.
    #   - Add a CHECK constraint to ensure project_path is always a configured root.
    #   - Optionally, add a foreign key constraint from files to projects.



class ProjectScannerManager:
    """
    Manages project scanning operations including running scans
    and handling the results.
    """
    
    def __init__(self, db_manager):
        """
        Initialize the scanner manager.
        
        Args:
            db_manager: Database manager instance
        """
        self.db_manager = db_manager
        self.thread_pool = QThreadPool()
        self.current_scanner = None
        self.is_scanning = False
        
        # Configure thread pool
        max_threads = min(8, QThreadPool.globalInstance().maxThreadCount())
        self.thread_pool.setMaxThreadCount(max_threads)
        logger.info(f"Scanner thread pool initialized with {max_threads} threads")
    
    def start_scan(self, directories, on_progress=None, on_finished=None, on_error=None):
        """
        Start scanning the specified directories.
        
        Args:
            directories (list): List of directories to scan
            on_progress (callable, optional): Callback for progress updates
            on_finished (callable, optional): Callback when scan is finished
            on_error (callable, optional): Callback when an error occurs
            
        Returns:
            bool: True if scan started, False if another scan is in progress
        """
        if self.is_scanning:
            logger.warning("Scan already in progress")
            return False
        
        # Create scanner
        self.current_scanner = ProjectScanner(self.db_manager, directories)
        self.is_scanning = True
        
        # Connect signals
        if on_progress:
            self.current_scanner.signals.progress.connect(on_progress)
        
        # Connect finished signal
        def on_scan_finished(files):
            self.is_scanning = False
            if on_finished:
                on_finished(files)
        
        self.current_scanner.signals.finished.connect(on_scan_finished)
        
        # Connect error signal
        if on_error:
            self.current_scanner.signals.error.connect(on_error)
        
        # Start scanner
        self.thread_pool.start(self.current_scanner)
        return True
    
    def stop_scan(self):
        """
        Stop the current scan.
        
        Returns:
            bool: True if stopped, False if no scan in progress
        """
        if not self.is_scanning or not self.current_scanner:
            return False
        
        self.current_scanner.stop()
        return True
