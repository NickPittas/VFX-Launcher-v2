"""
Scanner Module for VFX Launcher

Handles all scanning logic, completely separated from UI concerns:
1. First scans project path for folders matching target folder names from settings
2. Then scans those matched folders for .nk and .aep files 
3. Runs parallel scans using QThreadPool
4. Provides signals for progress and found files
5. Updates database with all findings

This implementation strictly follows the project requirements.
"""

import os
import time
import logging
import warnings
import configparser
from datetime import datetime
from PySide6.QtCore import QObject, QRunnable, Signal, Slot, QThreadPool, QTimer

# Configure logger
logger = logging.getLogger(__name__)

class ScannerSignals(QObject):
    """Signals emitted by scanner workers"""
    started = Signal()
    progress = Signal(int, str, str)  # percent, message, eta
    folder_found = Signal(str)  # full path of matching target folder
    file_found = Signal(dict)  # file info dictionary
    log = Signal(str)  # log message
    finished = Signal(list)  # list of all files/folders found
    db_update_complete = Signal(str)  # project path when DB update is done
    error = Signal(str)  # error message
    scan_complete = Signal(str)  # project path

class ScannerSignalRelay(QObject):
    """
    Relay that routes worker signals to the GUI thread.

    Worker signals are connected to this relay's signals. The relay is a
    QObject created on the GUI thread, so Qt delivers each hop as a queued
    invocation: the relay re-emits on the GUI thread, and the plain Python
    callbacks connected to it (which are not thread-safe) only ever run there.
    """
    progress = Signal(int, str, str)  # percent, message, eta
    log = Signal(str)  # log message
    file_found = Signal(dict)  # file info dictionary
    folder_found = Signal(str)  # full path of matching target folder
    error = Signal(str)  # error message
    finished = Signal(list)  # list of files/folders found


class FolderMatchWorker(QRunnable):
    """
    Worker that searches for folders matching target names.
    First step in the scanning process.
    """
    def __init__(self, project_path, target_folder_names):
        super().__init__()
        self.project_path = project_path
        self.target_folder_names = [name.lower() for name in target_folder_names]
        self.signals = ScannerSignals()
        self._stop_requested = False

    def stop(self):
        self._stop_requested = True

    def _estimate_eta(self, start_time, current, total):
        """Estimate time remaining based on progress"""
        if current == 0 or total == 0:
            return "--:--"
        elapsed = time.time() - start_time
        estimated_total = elapsed * (total / current)
        remaining = estimated_total - elapsed
        if remaining < 0:
            return "00:00"
        minutes = int(remaining / 60)
        seconds = int(remaining % 60)
        return f"{minutes:02d}:{seconds:02d}"

    @Slot()
    def run(self):
        try:
            self.signals.started.emit()
            self.signals.log.emit(f"Starting folder match scan in: {self.project_path}")
            self.signals.progress.emit(0, f"Initializing scan of {os.path.basename(self.project_path)}...", "--:--")

            start_time = time.time()
            matching_folders = []

            # First, check if project path itself is a match (less common)
            basename = os.path.basename(self.project_path).lower()
            if basename in self.target_folder_names:
                matching_folders.append(self.project_path)
                self.signals.folder_found.emit(self.project_path)
                self.signals.log.emit(f"✓ Project root is a target folder: {self.project_path}")
            
            # Fast directory-only scan using subprocess (similar to DIR /B /AD)
            # This is much faster than os.walk() for large directory trees
            import subprocess
            import platform

            self.signals.log.emit("⚡ Starting fast directory scan...")
            self.signals.progress.emit(5, f"Analyzing {os.path.basename(self.project_path)}...", "--:--")
            
            directories_found = set()
            is_windows = platform.system() == "Windows"
            
            try:
                # Use fast OS-specific command to list directories only
                if is_windows:
                    # Windows: Use DIR /B /S /AD which only lists directories
                    # Normalize path for Windows command line
                    norm_path = os.path.normpath(self.project_path)
                    
                    # Run command directly with shell=True for proper path handling
                    cmd = f'dir /b /s /ad "{norm_path}"'
                    self.signals.log.emit(f"Running command: {cmd}")
                    
                    process = subprocess.Popen(
                        cmd,
                        stdout=subprocess.PIPE,
                        stderr=subprocess.STDOUT,
                        text=True,
                        shell=True
                    )
                else:
                    # Unix/Linux/Mac: Use find to list directories only
                    process = subprocess.Popen(
                        ["find", self.project_path, "-type", "d"],
                        stdout=subprocess.PIPE,
                        stderr=subprocess.STDOUT,
                        text=True
                    )
                
                # Process output line by line
                dir_count = 0
                last_update_time = start_time
                for line in process.stdout:
                    if self._stop_requested:
                        process.terminate()
                        self.signals.log.emit("⚠ Folder match scan cancelled")
                        break

                    # Clean the path and ensure it's valid
                    directory = line.strip()

                    # Debug output - show first few and periodic updates
                    if dir_count < 5 or dir_count % 500 == 0:
                        self.signals.log.emit(f"📁 Found directory: {directory}")

                    if directory and os.path.isdir(directory):
                        directories_found.add(directory)
                        dir_count += 1

                        # Update progress more frequently (every 50 dirs or every 2 seconds)
                        current_time = time.time()
                        if dir_count % 50 == 0 or (current_time - last_update_time) >= 2.0:
                            elapsed = current_time - start_time
                            rate = dir_count / elapsed if elapsed > 0 else 0
                            # Progress increases from 0-30% during directory discovery
                            # Use logarithmic scale since we don't know total count
                            progress = min(int(5 + (dir_count / 100) * 25), 30)
                            # ETA is hard to estimate without knowing total, so show scan rate instead
                            self.signals.progress.emit(progress, f"Scanning... found {dir_count} directories ({rate:.0f}/sec)", "--:--")
                            last_update_time = current_time
                
                # Wait for process to complete and check for errors
                # (stderr is merged into stdout above, so there is no separate
                # pipe to drain - reading one while looping stdout would deadlock)
                return_code = process.wait()
                if return_code != 0:
                    self.signals.log.emit(f"⚠ Directory scan process failed with code {return_code}")

                elapsed = time.time() - start_time
                self.signals.log.emit(f"✓ Directory scan complete, found {dir_count} directories in {elapsed:.1f}s")
                
                # If we didn't find any directories, something went wrong
                if len(directories_found) == 0:
                    self.signals.log.emit("Warning: No directories found! Falling back to standard method.")
                    # Fall back to direct directory checking
                    for root, dirs, _ in os.walk(self.project_path):
                        if self._stop_requested:
                            break
                        for d in dirs:
                            full_path = os.path.join(root, d)
                            if os.path.isdir(full_path):
                                directories_found.add(full_path)
                                dir_count += 1
                                if dir_count % 100 == 0:
                                    self.signals.progress.emit(30, f"Found {dir_count} directories (fallback)...", "--:--")
            except Exception as e:
                self.signals.log.emit(f"Error during directory scan: {str(e)}")
                # Fall back to os.walk if subprocess fails
                self.signals.log.emit("Falling back to standard directory scan...")
                for root, dirs, _ in os.walk(self.project_path):
                    if self._stop_requested:
                        break
                    for d in dirs:
                        full_path = os.path.join(root, d)
                        directories_found.add(full_path)
            
            # Now check which directories match our target names
            self.signals.log.emit(f"🔍 Processing {len(directories_found)} directories for target matches...")
            self.signals.progress.emit(50, f"Matching directories to targets...", "--:--")

            # Match directories against target names
            total_folders_checked = 0
            match_start_time = time.time()
            last_match_update = match_start_time

            for directory in directories_found:
                if self._stop_requested:
                    self.signals.log.emit("⚠ Folder match scan cancelled")
                    break

                total_folders_checked += 1

                # Update progress more frequently (every 500 dirs or every 2 seconds)
                current_time = time.time()
                if total_folders_checked % 500 == 0 or (current_time - last_match_update) >= 2.0:
                    # Progress from 30-50% during matching phase
                    progress = min(30 + int((total_folders_checked / len(directories_found)) * 20), 50)
                    eta = self._estimate_eta(match_start_time, total_folders_checked, len(directories_found))
                    self.signals.progress.emit(progress, f"Matching {total_folders_checked}/{len(directories_found)} directories...", eta)
                    last_match_update = current_time

                # Get directory name for matching
                dirname = os.path.basename(directory).lower()
                if dirname in self.target_folder_names:
                    matching_folders.append(directory)
                    self.signals.folder_found.emit(directory)
                    self.signals.log.emit(f"✓ Found target folder: {directory}")
            
            # Report completion of folder matching phase (50% of total scan)
            elapsed = time.time() - start_time
            self.signals.log.emit(f"✓ Folder match scan complete. Found {len(matching_folders)} matching folders in {elapsed:.1f} seconds")
            self.signals.progress.emit(50, f"Found {len(matching_folders)} target folders, starting file scan...", "00:00")
            self.signals.finished.emit(matching_folders)

        except Exception as e:
            self.signals.log.emit(f"❌ Error in folder match scan: {str(e)}")
            self.signals.error.emit(f"Error in folder match scan: {str(e)}")


class FileSearchWorker(QRunnable):
    """
    Worker that searches for .nk and .aep files in a given folder.
    Second step in the scanning process.
    """
    def __init__(self, project_path, folder_path, file_extensions=None):
        super().__init__()
        self.project_path = project_path  # Original project path (for database)
        self.folder_path = folder_path    # Target folder to scan
        self.file_extensions = file_extensions or ['.nk', '.aep']
        self.signals = ScannerSignals()
        self._stop_requested = False

    def stop(self):
        self._stop_requested = True

    def _parse_version_from_filename(self, filename):
        """
        Extract version number from filename using regex.
        For AEP files: if no _v pattern found, use the last number in the filename (1-5 digits).
        For other files: only use _v pattern.
        """
        import re
        # First try the standard _v pattern (works for all file types)
        version_match = re.search(r'_v(\d{1,4})', filename)
        if version_match:
            return version_match.group(1)

        # For AEP files only, try to extract the last number in the filename
        if filename.lower().endswith('.aep'):
            # Remove the extension first
            name_without_ext = os.path.splitext(filename)[0]
            # Find the last number (1-5 digits) in the filename
            last_number_match = re.search(r'(\d{1,5})(?!.*\d)', name_without_ext)
            if last_number_match:
                return last_number_match.group(1)

        return "1"  # Default to version 1 if not found

    def _estimate_eta(self, start_time, current, total):
        """Estimate time remaining based on progress"""
        if current == 0:
            return "--:--"
        elapsed = time.time() - start_time
        estimated_total = elapsed * (total / current)
        remaining = estimated_total - elapsed
        minutes = int(remaining / 60)
        seconds = int(remaining % 60)
        return f"{minutes:02d}:{seconds:02d}"

    @Slot()
    def run(self):
        try:
            self.signals.started.emit()
            folder_name = os.path.basename(self.folder_path)
            self.signals.log.emit(f"📂 Starting file scan in: {folder_name}")
            self.signals.log.emit(f"🔍 Looking for: {', '.join(self.file_extensions)}")
            self.signals.progress.emit(0, f"Analyzing {folder_name}...", "--:--")
            
            start_time = time.time()
            found_files = []
            
            # Log the folder structure we're about to scan
            folder_name = os.path.basename(self.folder_path)
            self.signals.log.emit(f"📊 Analyzing folder structure in: {folder_name}")

            # First, count files and log folder structure for progress estimation
            total_files_estimate = 0
            folders_found = 0
            analysis_start = time.time()

            # Walk through the directory structure
            for root, dirs, files in os.walk(self.folder_path):
                if self._stop_requested:
                    self.signals.log.emit("⚠ Scan cancelled by user")
                    break

                # Log each subfolder we find (but limit to prevent spam)
                folders_found += 1
                if folders_found <= 20 or folders_found % 10 == 0:  # Log first 20 and then every 10th
                    rel_path = os.path.relpath(root, self.folder_path)
                    if rel_path == ".":
                        rel_path = "<root>"
                    self.signals.log.emit(f"  📁 Subfolder: {rel_path}")

                # Count matching files in this folder
                matching_files_in_folder = 0
                for filename in files:
                    ext = os.path.splitext(filename)[1].lower()
                    if ext in self.file_extensions:
                        matching_files_in_folder += 1
                        total_files_estimate += 1

                # Log summary for this folder
                if matching_files_in_folder > 0:
                    rel_path = os.path.relpath(root, self.folder_path)
                    if rel_path == ".":
                        rel_path = "<root>"
                    self.signals.log.emit(f"  ✓ Found {matching_files_in_folder} files in {rel_path}")

            analysis_time = time.time() - analysis_start
            self.signals.log.emit(f"✓ Analysis complete: {total_files_estimate} files in {folders_found} folders ({analysis_time:.1f}s)")
            
            if total_files_estimate == 0:
                self.signals.log.emit(f"⚠ No matching files found in {folder_name}")
                total_files_estimate = 1  # Prevent division by zero

            self.signals.log.emit(f"📝 Processing {total_files_estimate} files in {folder_name}")

            # Now do the actual scan
            self.signals.progress.emit(5, f"Processing files in {folder_name}...", "--:--")
            processed_files = 0
            processed_files_in_current_folder = 0
            current_folder = ""
            last_progress_update = time.time()
            
            for root, _, files in os.walk(self.folder_path):
                if self._stop_requested:
                    self.signals.log.emit("⚠ File processing cancelled by user")
                    break

                # Track when we change folders for better logging
                if root != current_folder:
                    # Log summary of previous folder if we processed files there
                    if current_folder and processed_files_in_current_folder > 0:
                        rel_path = os.path.relpath(current_folder, self.folder_path)
                        if rel_path == ".":
                            rel_path = "<root>"
                        self.signals.log.emit(f"  ✓ Completed {processed_files_in_current_folder} files in {rel_path}")

                    # Reset for new folder
                    current_folder = root
                    processed_files_in_current_folder = 0

                    # Log that we're starting a new folder
                    rel_path = os.path.relpath(root, self.folder_path)
                    if rel_path == ".":
                        rel_path = "<root>"
                    self.signals.log.emit(f"📂 Processing: {rel_path}")

                # Get matching files in this folder (excluding autosave/backup files)
                matching_files = [
                    f for f in files
                    if os.path.splitext(f)[1].lower() in self.file_extensions
                    and not f.endswith('.nk~')
                    and not f.endswith('.nk.autosave')
                ]
                if matching_files:
                    self.signals.log.emit(f"  📄 {len(matching_files)} files to process")
                
                for filename in files:
                    if self._stop_requested:
                        self.signals.log.emit("⚠ File processing cancelled by user")
                        break

                    # Skip Nuke autosave and backup files
                    if filename.endswith('.nk~') or filename.endswith('.nk.autosave'):
                        continue

                    # Check file extension
                    ext = os.path.splitext(filename)[1].lower()
                    if ext not in self.file_extensions:
                        continue

                    # Log the file we're processing (but limit to prevent spam)
                    processed_files += 1
                    processed_files_in_current_folder += 1

                    # Log every 10th file or if it's a milestone
                    if processed_files <= 10 or processed_files % 10 == 0 or processed_files == total_files_estimate:
                        self.signals.log.emit(f"  📄 [{processed_files}/{total_files_estimate}] {filename}")

                    # Update progress more frequently (every file or every 1 second)
                    current_time = time.time()
                    should_update = (current_time - last_progress_update) >= 1.0 or processed_files % 5 == 0

                    if should_update:
                        # Emit 0-100% progress for this individual folder
                        progress_percent = min(int((processed_files / total_files_estimate) * 100), 100)
                        eta = self._estimate_eta(start_time, processed_files, total_files_estimate)
                        rel_path = os.path.relpath(root, self.folder_path)
                        if rel_path == ".":
                            rel_path = folder_name
                        self.signals.progress.emit(progress_percent, f"[{processed_files}/{total_files_estimate}] {rel_path}", eta)
                        last_progress_update = current_time
                    
                    # Parse file metadata
                    try:
                        filepath = os.path.join(root, filename)
                        last_modified = os.path.getmtime(filepath)
                        version = self._parse_version_from_filename(filename)

                        # Create file info dictionary
                        file_info = {
                            'filename': filename,
                            'filepath': filepath,
                            'filetype': ext[1:],  # Remove leading dot
                            'last_modified': last_modified,
                            'version': version,
                            'project_path': self.project_path  # Original project path
                        }

                        found_files.append(file_info)
                        self.signals.file_found.emit(file_info)
                    except Exception as e:
                        error_msg = f"❌ Error processing {filename}: {str(e)}"
                        self.signals.log.emit(error_msg)
                        self.signals.error.emit(error_msg)

            # Report completion
            elapsed = time.time() - start_time
            rate = len(found_files) / elapsed if elapsed > 0 else 0
            self.signals.log.emit(f"✓ File scan complete: {len(found_files)} files in {elapsed:.1f}s ({rate:.1f} files/sec)")
            self.signals.progress.emit(100, f"Complete: {len(found_files)} files found", "00:00")
            self.signals.finished.emit(found_files)

        except Exception as e:
            error_msg = f"❌ Error in file scan: {str(e)}"
            self.signals.log.emit(error_msg)
            self.signals.error.emit(error_msg)


class Scanner:
    """
    Main scanner class that orchestrates the two-phase scanning process:
    1. Find folders matching target names
    2. Find .nk and .aep files in those folders
    """
    
    def __init__(self, db_manager=None):
        """Initialize scanner with optional db_manager"""
        self.db_manager = db_manager
        self.thread_pool = QThreadPool()
        # Configure thread pool
        max_threads = min(8, QThreadPool.globalInstance().maxThreadCount())
        self.thread_pool.setMaxThreadCount(max_threads)
        logger.info(f"Scanner initialized with {max_threads} threads")
        
        # Track running workers
        self.folder_match_worker = None
        self.file_search_workers = []
        self._stop_requested = False
        # GUI-thread signal relay for the current scan (retained so worker
        # signals stay routed through it)
        self._signal_relay = None
        # Completion polling timer for the database update worker (retained
        # so it is never garbage-collected mid-scan)
        self._completion_timer = None
        # Folder-completion counter, mutated only on the GUI thread (via the
        # signal relay), so no locking is needed
        self.completed_folders = 0

        # Results storage
        self.matching_folders = []
        self.found_files = []

    def _ensure_signal_relay(self):
        """Return the signal relay for the current scan, creating it if needed"""
        if self._signal_relay is None:
            self._signal_relay = ScannerSignalRelay()
        return self._signal_relay

    def _stop_completion_timer(self):
        """Stop and drop the completion timer left over from a previous scan"""
        if self._completion_timer is not None:
            self._completion_timer.stop()
            self._completion_timer = None

    def _load_target_folder_names(self):
        """Load target folder names from app_settings.ini"""
        from core.config import resolve_app_settings_path

        # Default target folder names
        target_folder_names = ['project', 'projects', 'Project', 'Projects']

        # Resolve the active settings path (dev repo file or writable per-user
        # copy in frozen builds)
        settings_path = resolve_app_settings_path()

        # Read from settings if available
        if os.path.exists(settings_path):
            try:
                config = configparser.ConfigParser()
                config.read(settings_path)
                if config.has_option('Projects', 'project_directories'):
                    folder_names_raw = config.get('Projects', 'project_directories')
                    target_folder_names = [name.strip() for name in folder_names_raw.split(',')]
                logger.info(f"Loaded target folder names from settings: {target_folder_names}")
            except Exception as e:
                logger.error(f"Error reading settings: {e}")
        else:
            # Create default settings if missing
            try:
                config = configparser.ConfigParser()
                if not config.has_section('Projects'):
                    config.add_section('Projects')
                config.set('Projects', 'project_directories', 'project, projects, Project, Projects')
                os.makedirs(os.path.dirname(settings_path), exist_ok=True)
                with open(settings_path, 'w') as f:
                    config.write(f)
                logger.info(f"Created default settings at {settings_path}")
            except Exception as e:
                logger.error(f"Error creating default settings: {e}")

        return target_folder_names
        
        
    def scan_project(self, project_path, on_progress=None, on_folder_found=None, 
                     on_file_found=None, on_log=None, on_finished=None, on_error=None):
        """Start a full scan of a project (folder matching + file scanning)"""
        if not project_path or not os.path.isdir(project_path):
            if on_error:
                on_error(f"Invalid project path: {project_path}")
            return False
            
        # Normalize the project path for consistent handling
        project_path = self._normalize_path(project_path)
        logger.info(f"Starting full scan for normalized project path: {project_path}")
            
        self._stop_requested = False
        self.matching_folders = []
        self.found_files = []
        self._stop_completion_timer()

        # Load target folder names from settings
        target_folder_names = self._load_target_folder_names()

        # Create folder match worker
        self.folder_match_worker = FolderMatchWorker(project_path, target_folder_names)

        # Create the GUI-thread signal relay for this scan (retained on self,
        # never a local). Every worker signal is routed through it, and the
        # plain callbacks below are connected to the relay instead of to the
        # worker signals, so Qt delivers them queued and they only run on the
        # GUI thread.
        relay = self._signal_relay = ScannerSignalRelay()

        # Route every folder match worker signal through the relay
        self.folder_match_worker.signals.progress.connect(relay.progress)
        self.folder_match_worker.signals.log.connect(relay.log)
        self.folder_match_worker.signals.folder_found.connect(relay.folder_found)
        self.folder_match_worker.signals.error.connect(relay.error)
        self.folder_match_worker.signals.finished.connect(relay.finished)

        # Connect relay signals to the plain callbacks (GUI thread only)
        if on_progress:
            relay.progress.connect(on_progress)

        if on_log:
            relay.log.connect(on_log)

        if on_folder_found:
            relay.folder_found.connect(on_folder_found)

        if on_error:
            relay.error.connect(on_error)

        
        # Connect folder match completion to start file search
        def on_folder_match_complete(matching_folders):
            self.matching_folders = matching_folders
            if on_log:
                on_log(f"Found {len(matching_folders)} matching folders: {matching_folders}")
                
            # Save matching folders to database if db_manager available
            if self.db_manager:
                try:
                    project = self.db_manager.get_project_by_path(self._canonical_path(project_path))
                    if not project:
                        # Single fallback with the raw spelling as given (legacy DB rows)
                        project = self.db_manager.get_project_by_path(project_path)
                    if project:
                        self.db_manager.update_project_folders(project['id'], matching_folders)
                        if on_log:
                            on_log(f"Updated project folders in database for project ID {project['id']}")
                except Exception as e:
                    logger.error(f"Error updating project folders in database: {str(e)}")

            
            # Start file search in each matching folder
            if matching_folders:
                self._start_file_search(project_path, matching_folders, on_progress, 
                                      on_file_found, on_log, on_finished, on_error)
            else:
                if on_log:
                    on_log(f"No matching folders found in project: {project_path}")
                if on_finished:
                    on_finished(project_path, [])
        
        relay.finished.connect(on_folder_match_complete)
        
        # Start folder match worker
        self.thread_pool.start(self.folder_match_worker)
        return True
    
    def _start_file_search(self, project_path, matching_folders, on_progress=None, 
                          on_file_found=None, on_log=None, on_finished=None, on_error=None):
        """Start file search in each matching folder"""
        self.file_search_workers = []
        self.found_files = []
        remaining_folders = len(matching_folders)
        
        # If no folders to scan, we're done
        if remaining_folders == 0:
            if on_finished:
                on_finished(project_path, [])
            return
            
        # Track completion to know when all workers are done
        self.completed_folders = 0
        
        # Function to handle file search worker completion
        def on_file_search_complete(files_found):
            nonlocal remaining_folders
            
            # Update completion counter
            self.completed_folders += 1
            remaining_folders -= 1
            
            # Add found files to our collection
            self.found_files.extend(files_found)
            
            # Log progress
            if on_log:
                on_log(f"Completed scanning folder {self.completed_folders}/{len(matching_folders)}, found {len(files_found)} files")

            # Update overall progress (reserve last 5% for database update)
            if on_progress:
                progress = min(int((self.completed_folders / len(matching_folders)) * 95), 95)
                on_progress(progress, f"Scanned {self.completed_folders}/{len(matching_folders)} folders, {len(self.found_files)} files total", "00:00")
                
            # Check if all searches are complete
            if remaining_folders == 0:
                if on_log:
                    on_log(f"All folder scans complete. Found {len(self.found_files)} total files")
                # All searches are complete, update database
                on_all_file_searches_complete()
                
        # Create and start a worker for each matching folder
        if on_log:
            on_log(f"Starting file search in {len(matching_folders)} folders")

        # Get file extensions to scan for from settings or use defaults
        file_extensions = ['.nk', '.aep']

        # Get the GUI-thread signal relay for this scan (full scans created it
        # in scan_project; quick scans create it here on the GUI thread) and
        # rewire it for the file-search phase: disconnect the folder-match
        # receivers so each callback fires exactly once per event.
        relay = self._ensure_signal_relay()
        with warnings.catch_warnings():
            # PySide prints a RuntimeWarning when disconnecting a signal
            # that has no receivers yet
            warnings.simplefilter("ignore", RuntimeWarning)
            for relay_signal in (relay.progress, relay.log, relay.file_found,
                                 relay.folder_found, relay.error, relay.finished):
                try:
                    relay_signal.disconnect()
                except RuntimeError:
                    pass  # Signal had no connections yet

        total_folders = len(matching_folders)

        # Single progress handler shared by all file search workers. It runs
        # on the GUI thread via the relay, like every other callback below,
        # so reading self.completed_folders there needs no lock.
        if on_progress:
            def overall_progress_handler(percent, msg, eta):
                # Scale individual worker progress (0-100%) to overall progress (50-95%)
                # Each folder gets an equal share of the 45% range
                folder_share = 45 / total_folders
                # Use current completed_folders count from scanner
                base_progress = 50 + (self.completed_folders * folder_share)
                current_folder_progress = (percent / 100) * folder_share
                overall_progress = int(base_progress + current_folder_progress)
                overall_progress = min(overall_progress, 95)  # Cap at 95%

                # Show individual folder progress with ETA
                current_index = min(self.completed_folders + 1, total_folders)
                on_progress(overall_progress, f"Folder {current_index}/{total_folders}: {msg}", eta)
            relay.progress.connect(overall_progress_handler)

        relay.log.connect(on_log if on_log else lambda _: None)
        relay.file_found.connect(on_file_found if on_file_found else lambda _: None)
        relay.error.connect(on_error if on_error else lambda _: None)
        # The completed_folders/remaining_folders counters mutated by this
        # handler are only ever touched on the GUI thread because the relay
        # re-emits there - no locking needed.
        relay.finished.connect(on_file_search_complete)

        # Create and start workers for each folder
        for folder_path in matching_folders:
            if self._stop_requested:
                break

            # Create worker
            worker = FileSearchWorker(project_path, folder_path, file_extensions)
            self.file_search_workers.append(worker)

            # Route every worker signal through the GUI-thread relay
            worker.signals.progress.connect(relay.progress)
            worker.signals.log.connect(relay.log)
            worker.signals.file_found.connect(relay.file_found)
            worker.signals.error.connect(relay.error)
            worker.signals.finished.connect(relay.finished)

            # Start worker
            self.thread_pool.start(worker)

            if on_log:
                on_log(f"Started file search in: {folder_path}")

        
        # Function to handle completion of all file searches. Reached only via
        # on_file_search_complete, which the relay runs on the GUI thread, so
        # everything below (including the completion timer) executes there.
        def on_all_file_searches_complete():
            # Update database if db_manager available
            if self.db_manager:
                if on_log:
                    on_log(f"Starting database update with {len(self.found_files)} files (background task)...")
                
                # Create a worker to update database in background
                class DbUpdateWorker(QRunnable):
                    def __init__(self, scanner, project_path, files, db_path):
                        super().__init__()
                        self.scanner = scanner
                        self.project_path = project_path
                        self.files = files.copy()  # Make a copy to avoid thread safety issues
                        self.db_path = db_path
                        self.signals = ScannerSignals()
                        self.setAutoDelete(True)
                        self.completed = False

                    @Slot()
                    def run(self):
                        try:
                            # Create a new database manager for this thread
                            from core.database import DatabaseManager
                            thread_db = DatabaseManager.create_thread_safe_manager(self.db_path)

                            # Update database with the thread-safe connection
                            self.signals.log.emit(f"Starting database update with thread-safe connection...")
                            logger.info(f"[DbUpdateWorker] Starting database update for {self.project_path}")
                            self.scanner._update_database(self.project_path, self.files, thread_db=thread_db)

                            self.signals.log.emit(f"Database update complete with {len(self.files)} files")
                            logger.info(f"[DbUpdateWorker] Database update complete, setting completed flag")

                            # Set completion flag
                            self.completed = True
                            logger.info(f"[DbUpdateWorker] Completed flag set to True")

                        except Exception as e:
                            error_msg = f"Error updating database: {str(e)}"
                            logger.error(error_msg)
                            logger.error(f"[DbUpdateWorker] Exception details: {type(e).__name__}: {str(e)}")
                            import traceback
                            logger.error(f"[DbUpdateWorker] Traceback: {traceback.format_exc()}")
                            self.signals.error.emit(error_msg)
                            self.completed = True  # Mark as completed even on error
                
                # Create and run database update worker
                db_path = self.db_manager.db_path if self.db_manager else None
                db_worker = DbUpdateWorker(self, project_path, self.found_files, db_path)

                # Connect signals through the GUI-thread relay
                db_worker.signals.log.connect(relay.log)
                db_worker.signals.error.connect(relay.error)

                # Start database update worker
                logger.info(f"[Scanner] Starting database update worker in thread pool")
                self.thread_pool.start(db_worker)
                logger.info(f"[Scanner] Database update worker started")

                # Use a timer to poll for completion. The timer is retained on
                # self (never a bare local) so it is not garbage-collected
                # mid-scan and can be stopped when the next scan starts.
                self._stop_completion_timer()
                self._completion_timer = QTimer()
                self._completion_timer.setInterval(100)  # Check every 100ms

                def check_completion():
                    if db_worker.completed:
                        logger.info(f"[Scanner] ========== DB WORKER COMPLETED (detected by timer) ==========")
                        self._stop_completion_timer()

                        # Update progress to 100%
                        if on_progress:
                            logger.info(f"[Scanner] Calling on_progress with 100%")
                            on_progress(100, f"Scan complete: {len(self.found_files)} files found", "00:00")

                        # Call finished callback
                        if on_finished:
                            logger.info(f"[Scanner] Calling on_finished callback")
                            on_finished(project_path, self.found_files)

                        logger.info(f"[Scanner] Completion handling finished")

                self._completion_timer.timeout.connect(check_completion)
                self._completion_timer.start()
                logger.info(f"[Scanner] Completion timer started, polling every 100ms")
            else:
                # No DB manager, just signal completion
                if on_progress:
                    on_progress(100, f"Scan complete: {len(self.found_files)} files found", "00:00")
                if on_finished:
                    on_finished(project_path, self.found_files)
                    
    def stop(self):
        """Stop all scanning operations"""
        self._stop_requested = True
        
        # Stop folder match worker if running
        if self.folder_match_worker and hasattr(self.folder_match_worker, 'stop'):
            self.folder_match_worker.stop()
            
        # Stop all file search workers if running
        for worker in self.file_search_workers:
            if hasattr(worker, 'stop'):
                worker.stop()
                
        logger.info("Scanner stopping all operations")
                
    def quick_scan_project(self, project_path, on_progress=None, on_folder_found=None, 
                      on_file_found=None, on_log=None, on_finished=None, on_error=None):
        """
        Perform a quick scan of a project, only scanning files in already-known folders.
        This is optimized for finding new versions without redoing the folder matching phase.
        """
        if not project_path or not os.path.isdir(project_path):
            if on_error:
                on_error(f"Invalid project path: {project_path}")
            return False
        
        # Normalize the project path for consistent handling
        project_path = self._normalize_path(project_path)
        logger.info(f"Starting quick scan for normalized project path: {project_path}")
        
        # Check if this is a Projects subfolder and get the parent project path
        original_project_path = project_path
        parent_project_path = project_path
        if os.path.basename(project_path) == "Projects":
            parent_project_path = os.path.dirname(project_path)
            if on_log:
                on_log(f"Detected Projects subfolder, will also check parent path: {parent_project_path}")
        
        self._stop_requested = False
        self.found_files = []
        self._stop_completion_timer()

        # Fresh GUI-thread signal relay for this scan (quick scans wire it up
        # in _start_file_search, which runs on the GUI thread)
        self._signal_relay = ScannerSignalRelay()

        
        # Get the target folder names from settings
        target_folder_names = self._load_target_folder_names()
        
        # Get the already known matched folders for this project from the database
        matching_folders = []
        
        # Try to get existing folders from database based on project path
        if self.db_manager:
            try:
                # First, verify the project exists in the database
                project_id = None
                if on_log:
                    on_log(f"Checking database for project with path: {project_path}")
                    
                with self.db_manager.get_connection() as conn:
                    cursor = conn.cursor()
                    # Primary lookup uses the canonical form (how new project
                    # rows are stored); single fallback keeps the raw spelling
                    # as given for legacy database rows
                    cursor.execute("SELECT id FROM projects WHERE path = ?", (self._canonical_path(project_path),))
                    result = cursor.fetchone()
                    if not result:
                        cursor.execute("SELECT id FROM projects WHERE path = ?", (project_path,))
                        result = cursor.fetchone()

                    # If not found and this is a Projects subfolder, try the parent path
                    if not result and project_path != parent_project_path:
                        cursor.execute("SELECT id FROM projects WHERE path = ?", (self._canonical_path(parent_project_path),))
                        result = cursor.fetchone()
                        if not result:
                            cursor.execute("SELECT id FROM projects WHERE path = ?", (parent_project_path,))
                            result = cursor.fetchone()
                        if result and on_log:
                            on_log(f"Found project using parent path: {parent_project_path}")
                
                    if result:
                        project_id = result['id']
                        if on_log:
                            on_log(f"Found project in database with ID: {project_id}")
                    else:
                        # If still no exact match, check if this is a subfolder of any known project
                        cursor.execute("SELECT id, path FROM projects")
                        all_projects = cursor.fetchall()
                        if on_log:
                            on_log(f"Project not found with normalized path, trying parent relationship check")
                            
                        canonical_scan_path = self._canonical_path(project_path)
                        canonical_scan_parent = self._canonical_path(parent_project_path)
                        for proj in all_projects:
                            # Check if our path starts with the project path (is a subfolder)
                            normalized_proj_path = self._canonical_path(proj['path'])
                            if canonical_scan_path.startswith(normalized_proj_path + os.sep) or canonical_scan_parent.startswith(normalized_proj_path + os.sep):
                                project_id = proj['id']
                                if on_log:
                                    on_log(f"Found parent project in database with ID: {project_id} (path: {proj['path']})")
                                break
                            
                        if not project_id and on_log:
                            on_log(f"Project not found in database for quick scan: {project_path}. Available projects: {[p['path'] for p in all_projects]}")
            except Exception as e:
                if on_log:
                    on_log(f"Error accessing database: {str(e)}")
                if on_error:
                    on_error(f"Database error: {str(e)}")
        else:
            if on_log:
                on_log("Database manager not available, will scan without database information")
            # Don't treat this as an error, just continue with fallback approach
            # if on_error:
            #    on_error("Database manager not available for quick scan")
        # Preferred folder source: the folders column persisted by the last
        # full scan (newline-joined paths)
        if self.db_manager and 'project_id' in locals() and project_id:
            try:
                rows = self.db_manager._execute_query(
                    "SELECT folders FROM projects WHERE id = ?",
                    (project_id,)
                )
                stored_folders = []
                if rows and rows[0]['folders']:
                    stored_folders = [
                        folder for folder in rows[0]['folders'].split('\n')
                        if folder and os.path.isdir(folder)
                    ]

                if stored_folders:
                    matching_folders = stored_folders
                    if on_log:
                        on_log(f"Using {len(stored_folders)} stored folders from last full scan for project ID {project_id}")
                elif on_log:
                    on_log(f"No stored folders for project ID {project_id}, falling back to file paths")
            except Exception as e:
                if on_log:
                    on_log(f"Error reading stored folders for project ID {project_id}: {str(e)}")

        # Fallback: derive folders from the file paths already in the database
        if self.db_manager and 'project_id' in locals() and project_id and not matching_folders:
            try:
                if on_log:
                    on_log(f"Querying database for existing folders for project ID {project_id}")
                    
                with self.db_manager.get_connection() as conn:
                    cursor = conn.cursor()
                    # Get all filepaths first, then extract folder paths in Python
                    cursor.execute(
                        "SELECT filepath FROM project_files WHERE project_id = ?", 
                        (project_id,)
                    )
                    # Extract unique folder paths from file paths
                    db_folders = set()
                    for row in cursor.fetchall():
                        if row['filepath']:
                            folder_path = os.path.dirname(row['filepath'])
                            if folder_path:
                                db_folders.add(folder_path)
                    # Convert set to list for consistent handling
                    db_folders = list(db_folders)
                
                    # If this is a Projects subfolder and it's not already in the database,
                    # add it to ensure we scan it
                    if original_project_path not in db_folders and os.path.isdir(original_project_path):
                        db_folders.append(original_project_path)
                        if on_log:
                            on_log(f"Added current scan path to folders: {original_project_path}")
                    
                    if on_log:
                        on_log(f"Found {len(db_folders)} folder paths in database")
                        for folder in db_folders[:10]:  # Log up to 10 folders to avoid spam
                            on_log(f"Database folder: {folder}")
                        if len(db_folders) > 10:
                            on_log(f"... and {len(db_folders) - 10} more folders")
                
                    # Verify folders still exist
                    matching_folders = []
                    for folder in db_folders:
                        if os.path.isdir(folder):
                            matching_folders.append(folder)
                            if on_log:
                                on_log(f"Verified folder exists: {folder}")
                        else:
                            if on_log:
                                on_log(f"Folder no longer exists: {folder}")
                    
                    if on_log:
                        on_log(f"Quick scan: Using {len(matching_folders)} verified folders from database")
            except Exception as e:
                if on_log:
                    on_log(f"Error getting folders from database: {str(e)}")
                if on_error:
                    on_error(f"Database error: {str(e)}")
    
        # If no folders found in database, try direct match using target folder names
        if not matching_folders:
            if on_log:
                on_log("No existing folders found in database, checking for common target folders")
                on_log(f"Looking for folders matching these names: {', '.join(target_folder_names)}")
                
            # Fast check for common target folders directly under project path
            try:
                items = os.listdir(project_path)
                if on_log:
                    on_log(f"Found {len(items)} items in project root directory")
                
                for item in items:
                    item_path = os.path.join(project_path, item)
                    if os.path.isdir(item_path):
                        folder_name = os.path.basename(item_path).lower()
                        target_names_lower = [name.lower() for name in target_folder_names]
                        
                        # Check if this folder matches any target name
                        if folder_name in target_names_lower:
                            matching_folders.append(item_path)
                            if on_log:
                                on_log(f"Found matching target folder: {item_path} (matches '{folder_name}')")
                        else:
                            if on_log:
                                on_log(f"Skipping non-matching folder: {item_path}")
            except Exception as e:
                if on_log:
                    on_log(f"Error listing project directory: {str(e)}")
                if on_error:
                    on_error(f"File system error: {str(e)}")
    
        # If still no matching folders, check if the user REALLY wants to continue
        if not matching_folders:
            error_msg = f"WARNING: No target folders found for project: {project_path}"
            if on_log:
                on_log(error_msg)
                on_log("Scan would need to use the entire project root as fallback, which may be very slow")
                
            if on_error:
                # Signal an error instead of proceeding with a slow scan
                on_error(error_msg + ". Please check project structure and settings.")
                return False
                
            # Only as absolute last resort, use project root
            if on_log:
                on_log(f"CAUTION: Using project root as fallback: {project_path}")
            matching_folders = [project_path]
    
        # Start file scanning phase immediately
        # Note: We're calling the correct method _start_file_search
        # The on_finished callback from _start_file_search already has the correct signature (project_path, files)
        return self._start_file_search(
            project_path,
            matching_folders,
            on_progress,
            on_file_found,
            on_log,
            on_finished,  # Pass through directly - it already expects (project_path, files)
            on_error
        )
        
        # The _start_file_search method handles everything else - no need for duplicate code here

    def _normalize_path(self, path):
        """
        Normalize a path for consistent comparison.
        Handles different path formats and ensures consistent comparison.
        """
        if not path:
            return ""
            
        # Convert to string if needed
        path = str(path)
        
        # Replace backslashes with forward slashes
        path = path.replace('\\', '/')
        
        # Remove trailing slash if present
        if path.endswith('/') and len(path) > 1:
            path = path[:-1]
            
        return path

    def _canonical_path(self, path):
        """
        Canonical path form used for database lookups and new project rows:
        absolute, normalized, and case-folded per platform.
        """
        if not path:
            return ""
        return os.path.normcase(os.path.normpath(os.path.abspath(str(path))))
        
    def _update_database(self, project_path, files, thread_db=None):
        """
        Update database with found files.
        Args:
            project_path (str): Project path
            files (list): List of file dictionaries
            thread_db (DatabaseManager, optional): Thread-safe database manager
        """
        # Use provided thread_db connection or self.db_manager
        db_manager = thread_db or self.db_manager
        if not db_manager:
            logger.error("No database manager available for update")
            return
            
        # Check if this is a Projects subfolder and get the parent project path
        original_project_path = project_path
        parent_project_path = project_path
        if os.path.basename(project_path) == "Projects":
            parent_project_path = os.path.dirname(project_path)
            logger.info(f"Detected Projects subfolder, using parent path for project: {parent_project_path}")
        
        # Normalize paths for consistent comparison
        project_path = self._normalize_path(project_path)
        parent_project_path = self._normalize_path(parent_project_path)
        
        # Canonical form for the primary database lookup and for new rows
        canonical_parent_path = self._canonical_path(parent_project_path)

        # Primary lookup with the canonical form; single fallback with the
        # raw parent spelling as given (legacy database rows). No
        # backslash-swap or trailing-slash variants.
        project = db_manager.get_project_by_path(canonical_parent_path)
        if not project:
            project = db_manager.get_project_by_path(parent_project_path)

        # If project still not found, create it with the canonical path
        project_name = os.path.basename(parent_project_path)
        if not project:
            try:
                project_id = db_manager.create_project(project_name, canonical_parent_path)
                logger.info(f"Created new project in database: {project_name} ({canonical_parent_path})")
                project = {'id': project_id}
            except Exception as e:
                logger.error(f"Error creating project {project_name}: {str(e)}")
                return
        else:
            project_id = project['id']
        
        # Process all files
        current_filepaths = set()
        new_files = []
        updated_files = []
        
        # Get existing files to avoid duplicates and handle updates
        existing_files = db_manager.get_project_files(project_id)
        existing_paths = {}
        for file in existing_files:
            filepath = file.get('filepath')
            if filepath:
                # Normalize path for consistent comparison (use forward slashes)
                normalized_path = filepath.replace('\\', '/')
                existing_paths[normalized_path] = file

        # Process new/updated files
        for file_info in files:
            if self._stop_requested:
                break

            filepath = file_info.get('filepath')
            if not filepath:
                continue

            # Normalize filepath for consistent comparison
            normalized_filepath = filepath.replace('\\', '/')

            # Track current files for deletion handling
            current_filepaths.add(normalized_filepath)
            
            # Parse version from filename if not already set
            filename = file_info.get('filename', '')
            if not file_info.get('version'):
                # Use the same version extraction logic as _parse_version_from_filename
                file_info['version'] = self._parse_version_from_filename(filename)

            if normalized_filepath in existing_paths:
                # File exists, check if it needs updating
                existing = existing_paths[normalized_filepath]
                if (file_info.get('last_modified') != existing.get('last_modified') or
                    file_info.get('version') != existing.get('version')):
                    # Update file with new version/metadata using add_project_file (which handles updates)
                    try:
                        db_manager.add_project_file(
                            project_id,
                            file_info.get('filename'),
                            file_info.get('filepath'),
                            file_info.get('version'),
                            file_info.get('filetype'),
                            file_info.get('last_modified')
                        )
                        updated_files.append(filepath)
                        logger.debug(f"Updated file in DB: {filepath}")
                    except Exception as e:
                        logger.error(f"Error updating file {filepath}: {str(e)}")
            else:
                # New file, add it
                try:
                    db_manager.add_project_file(
                        project_id,
                        file_info.get('filename'),
                        file_info.get('filepath'),
                        file_info.get('version'),
                        file_info.get('filetype'),
                        file_info.get('last_modified')
                    )
                    new_files.append(filepath)
                    logger.debug(f"Added new file to DB: {filepath}")
                except Exception as e:
                    logger.error(f"Error adding file {filepath}: {str(e)}")
        
        # Handle deleted files - remove files that exist in DB but weren't found on disk
        deleted_files = []
        if self._stop_requested or not os.path.exists(original_project_path):
            # Skip the sweep entirely when the scan was cancelled or the
            # project path is unavailable (e.g. an unmounted network share):
            # in both cases the files were simply not seen, and sweeping
            # would mass-delete rows for files that still exist.
            logger.warning(
                f"Skipping deleted-file sweep for {original_project_path} "
                f"(stop_requested={self._stop_requested}, "
                f"path_exists={os.path.exists(original_project_path)})"
            )
        else:
            for db_filepath, db_file in existing_paths.items():
                if db_filepath not in current_filepaths:
                    try:
                        # Check if file exists on disk as a final verification
                        if not os.path.exists(db_filepath):
                            # File is in DB but not on disk, delete it
                            db_manager._execute_query(
                                "DELETE FROM project_files WHERE id = ?",
                                (db_file.get('id'),)
                            )
                            deleted_files.append(db_filepath)
                            logger.info(f"Removed file from DB (no longer exists): {db_filepath}")
                    except Exception as e:
                        logger.error(f"Error removing deleted file {db_filepath}: {str(e)}")
        # Log results
        logger.info(f"Database update complete: {len(new_files)} new, {len(updated_files)} updated, {len(deleted_files)} deleted")
    
    # End of Scanner class
