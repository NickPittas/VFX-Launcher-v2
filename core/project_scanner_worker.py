"""
ProjectScannerWorker Module

Implements QRunnable for single-project async scan, filtering subfolders by settings, and emitting progress/log signals.
"""

import os
import re
import time
import logging
from PySide6.QtCore import QObject, Signal, QRunnable, Slot

# Configure logger
logger = logging.getLogger(__name__)

class ScanWorkerSignals(QObject):
    progress = Signal(int, str, str)  # percent, message, eta
    log = Signal(str)
    finished = Signal(list)
    error = Signal(str)
    cancelled = Signal()
    file_found = Signal(dict)  # NEW: emits each found file as dict

class ProjectScannerWorker(QRunnable):
    """
    QRunnable for scanning a single project, filtering subfolders by settings, and reporting progress/logs.
    Uses its own DatabaseManager instance per thread for SQLite thread safety.
    """
    def __init__(self, db_path=None, project_path=None, settings_path=None, file_extensions=None):
        super().__init__()
        self.db_path = db_path  # Store only the db path, not the manager
        self.project_path = project_path
        self.settings_path = settings_path
        self.file_extensions = file_extensions or ['.nk', '.aep']
        self.signals = ScanWorkerSignals()
        self._stop_requested = False
        self.target_folder_names = ['project']  # Default fallback
        if self.settings_path:
            try:
                import configparser
                config = configparser.ConfigParser()
                config.read(self.settings_path)
                raw_names = config.get('Projects', 'project_directories', fallback='project')
                logger.info(f"[ScanWorker] Loaded subfolder names from settings: {raw_names}")
                self.target_folder_names = [n.strip().lower() for n in raw_names.split(',') if n.strip()]
            except Exception as e:
                logger.error(f"[ScanWorker] Error reading project_directories from settings: {e}")
        logger.info(f"[ScanWorker] Using subfolder names for scan: {self.target_folder_names}")

    def stop(self):
        self._stop_requested = True

    @Slot()
    def run(self):
        try:
            all_files = []
            start_time = time.time()
            percent = 0
            eta = self._estimate_eta(start_time, 0, 1)
            self.signals.progress.emit(percent, f"Scanning: {self.project_path}", eta)
            logger.info(f"[ScanWorker] Scanning root folder for .aep/.nk files: {self.project_path}")
            # QUICK SCAN MODE: No DB, no settings
            if self.db_path is None:
                # Only scan the given project_path (or folder), do not try to find subfolders
                root_files = self._scan_files(self.project_path)
                logger.info(f"[ScanWorker] Found {len(root_files)} files in root: {root_files}")
                all_files.extend(root_files)
                self.signals.progress.emit(100, "Quick scan complete.", "00:00")
                self.signals.finished.emit(all_files)
                return
            # NORMAL MODE: Use DB and settings
            from core.database import DatabaseManager
            db_manager = DatabaseManager(self.db_path)
            self.signals.log.emit(f"Starting scan for project: {self.project_path}")
            logger.info(f"[ScanWorker] Scanning project root: {self.project_path}")
            scan_dirs = self._find_target_subfolders(self.project_path)
            logger.info(f"[ScanWorker] Found {len(scan_dirs)} matching subfolders: {scan_dirs}")
            # IMPORTANT: Do NOT scan the root folder directly - only scan target subfolders
            # Skip root folder scanning - all scans must be in target folders only
            logger.info(f"[ScanWorker] Not scanning root folder directly - only target subfolders")
            # Now scan any matching subfolders
            total_dirs = len(scan_dirs)
            for i, scan_dir in enumerate(scan_dirs):
                if self._stop_requested:
                    logger.info(f"[ScanWorker] Scan cancelled by user.")
                    self.signals.cancelled.emit()
                    return
                percent = int((i + 1) / (total_dirs + 1) * 100)
                eta = self._estimate_eta(start_time, i + 1, total_dirs + 1)
                self.signals.progress.emit(percent, f"Scanning: {scan_dir}", eta)
                logger.info(f"[ScanWorker] Recursively scanning for .aep/.nk files in: {scan_dir}")
                files = self._scan_files(scan_dir)
                logger.info(f"[ScanWorker] Found {len(files)} files (recursive) in {scan_dir}: {files}")
                all_files.extend(files)
            self.signals.finished.emit(all_files)
            # Sync database: add new files, remove missing
            updated, removed = self._sync_database(all_files)
            self.signals.progress.emit(100, "Scan complete.", "00:00")
            self.signals.log.emit(f"Scan complete. {len(all_files)} files found. {updated} updated, {removed} removed.")
            self.signals.finished.emit(all_files)
        except Exception as e:
            msg = f"Error during scan: {str(e)}"
            logger.error(msg)
            self.signals.error.emit(msg)

    def _find_target_subfolders(self, root):
        """
        Recursively find all subfolders whose name matches any in self.target_folder_names (case-insensitive).
        Args:
            root (str): Root directory to search from
        Returns:
            list: List of matching subfolder absolute paths
        """
        logger.info(f"[ScanWorker] Searching for subfolders in root: {root}")
        targets = []
        for dirpath, dirnames, _ in os.walk(root):
            if self._stop_requested:
                logger.info("[ScanWorker] Scan cancelled during subfolder search.")
                break
            for dirname in dirnames:
                if dirname.lower() in self.target_folder_names:
                    full_path = os.path.join(dirpath, dirname)
                    logger.info(f"[ScanWorker] Matched subfolder: {full_path}")
                    targets.append(full_path)
        return targets

    def _scan_files_in_folder(self, folder_path):
        """
        Scan for .aep and .nk files directly inside the given folder (non-recursive).
        Args:
            folder_path (str): Path to the folder
        Returns:
            list: List of found file paths
        """
        found_files = []
        try:
            for entry in os.scandir(folder_path):
                if entry.is_file() and os.path.splitext(entry.name)[1].lower() in ['.aep', '.nk']:
                    found_files.append(entry.path)
        except Exception as e:
            logger.error(f"Error scanning files in {folder_path}: {str(e)}")
        return found_files

    def _scan_files(self, folder):
        found = []
        file_count = 0
        start_time = time.time()
        scanned_files = 0
        
        # For root folder, we should only process direct target subfolders
        # For target subfolders (Projects, project, etc.), we should recursively process all content
        is_root_folder = folder == self.project_path
        folder_name = os.path.basename(folder).lower()
        is_target_folder = folder_name in self.target_folder_names
        
        # If this is the root folder and we're not supposed to scan it directly, return empty
        if is_root_folder and not is_target_folder:
            logger.info(f"[ScanWorker] Skipping root folder direct scan: {folder}")
            return found
        
        # If this is a subfolder but not a valid target and not under a target folder path, skip it
        if not is_root_folder and not is_target_folder:
            # Check if this is under a target folder path
            parent_is_target = False
            for target_name in self.target_folder_names:
                if f"\\{target_name}\\" in folder.lower() or f"/{target_name}/" in folder.lower():
                    parent_is_target = True
                    break
            
            if not parent_is_target:
                logger.info(f"[ScanWorker] Skipping non-target subfolder: {folder}")
                return found
            
        # Count files for progress (do a quick walk)
        estimated_total_files = 0
        try:
            for root, _, files in os.walk(folder):
                for f in files:
                    if os.path.splitext(f)[1].lower() in self.file_extensions:
                        estimated_total_files += 1
        except Exception as e:
            logger.error(f"[ScanWorker] Error counting files in {folder}: {e}")
            estimated_total_files = 10  # Fallback estimate
        
        if estimated_total_files == 0:
            estimated_total_files = 1  # Prevent stuck progress
        
        # Emit initial progress
        self.signals.progress.emit(0, f"Starting scan of {folder}", self._estimate_eta(start_time, 0, estimated_total_files))
        self.signals.log.emit(f"Starting recursive scan of directory: {folder}")
        
        # Now do the actual recursive scan
        import time as _time
        try:
            for root, _, files in os.walk(folder):
                if self._stop_requested:
                    logger.warning(f"[ScanWorker] Scan stopped by user during scanning")
                    break
                
                for f in files:
                    ext = os.path.splitext(f)[1].lower()
                    if ext in self.file_extensions:
                        path = os.path.join(root, f)
                        last_modified = os.path.getmtime(path)
                        version = self._parse_version_from_filename(f)
                        file_dict = {
                            'filename': f,
                            'filepath': path,
                            'filetype': ext[1:],
                            'last_modified': last_modified,
                            'version': version,
                            'project_path': self.project_path  # Always use the original project path
                        }
                        found.append(file_dict)
                        file_count += 1
                        scanned_files += 1
                        
                        # Emit incremental signal for UI
                        try:
                            self.signals.file_found.emit(file_dict)
                        except Exception as e:
                            logger.warning(f"[ScanWorker] Failed to emit file_found: {e}")
                            
                        # Update progress
                        if scanned_files % 10 == 0:  # Update less frequently for performance
                            percent = int((scanned_files / estimated_total_files) * 100)
                            percent = min(max(percent, 0), 100)
                            eta = self._estimate_eta(start_time, scanned_files, estimated_total_files)
                            self.signals.progress.emit(percent, f"Scanned {scanned_files} files", eta)
                            
                        # Yield to event loop occasionally
                        if scanned_files % 100 == 0:
                            _time.sleep(0.001)
        except Exception as e:
            logger.error(f"[ScanWorker] Error scanning folder {folder}: {e}")
            
        self.signals.log.emit(f"Finished directory: {folder}. Total files found so far: {file_count}")
        # Final progress/log
        eta = self._estimate_eta(start_time, scanned_files, estimated_total_files)
        self.signals.progress.emit(100, f"Completed folder: {folder}", eta)
        self.signals.log.emit(f"Completed scanning folder: {folder}. {file_count} files found.")
        return found


    def _parse_version_from_filename(self, filename):
        match = re.search(r'_v(\d+)', filename)
        return match.group(1) if match else ''

    def _sync_database(self, found_files):
        """
        Update the DB: add new files, remove missing, for this project.
        Returns: (updated_count, removed_count)
        Thread-safe: creates a new DatabaseManager in this thread.
        """
        if not self.db_path:
            # Quick scan mode: no DB sync
            return (0, 0)
        from core.database import DatabaseManager
        db_manager = DatabaseManager(self.db_path)
        # Always use canonical absolute path for lookup/creation
        import os
        canonical_path = os.path.abspath(self.project_path)
        project = db_manager.get_project_by_path(canonical_path)
        if not project:
            # Try raw project_path as fallback (legacy DBs)
            project = db_manager.get_project_by_path(self.project_path)
        if not project:
            # Log available project paths for diagnosis, but DO NOT create a new project
            all_projects = db_manager._execute_query("SELECT path FROM projects")
            logger.error(f"[ScanWorker] Project not found in DB for path {canonical_path}. Available: {[p['path'] for p in all_projects]}")
            raise Exception(f"Project not found in DB for path: {canonical_path}\nAvailable: {[p['path'] for p in all_projects]}")
        project_id = project['id']
        # Get current files in DB
        db_files = db_manager.get_project_files(project_id)
        db_filepaths = set(f['filepath'] for f in db_files)
        found_filepaths = set(f['filepath'] for f in found_files)
        # Add/update new files
        updated = 0
        for file in found_files:
            db_manager.add_project_file(
                project_id,
                file['filename'],
                file['filepath'],
                file.get('version', ''),
                file['filetype'],
                file['last_modified']
            )
            updated += 1
        # Remove missing files
        removed = 0
        for file in db_files:
            if file['filepath'] not in found_filepaths:
                db_manager.remove_project_file(file['id'])
                removed += 1
        return updated, removed

    def _estimate_eta(self, start_time, current, total):
        if current == 0:
            return "--:--"
        elapsed = time.time() - start_time
        per_item = elapsed / current
        remaining = (total - current) * per_item
        mins, secs = divmod(int(remaining), 60)
        return f"{mins:02}:{secs:02}"
