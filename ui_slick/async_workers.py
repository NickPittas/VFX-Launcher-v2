import os
import sqlite3
import threading
import traceback

from PySide6.QtCore import QRunnable, Slot, Signal, QObject
from core.database import DatabaseManager


class WorkerSignals(QObject):
    finished = Signal(object)
    error = Signal(tuple)


class DirectoryDiscoverySignals(QObject):
    finished = Signal(object)
    error = Signal(tuple)
    cancelled = Signal()


def discover_immediate_directories(root, cancelled=None):
    """Return sorted immediate child directories without walking their contents."""
    directories = []
    with os.scandir(root) as entries:
        for entry in entries:
            if cancelled is not None and cancelled.is_set():
                return None
            if entry.is_dir():
                directories.append(os.path.abspath(entry.path))
    if cancelled is not None and cancelled.is_set():
        return None
    directories = sorted(directories, key=lambda path: (os.path.basename(path).casefold(), path))
    return None if cancelled is not None and cancelled.is_set() else directories


class DirectoryDiscoveryWorker(QRunnable):
    """Discover immediate child directories without touching the database."""

    def __init__(self, root, cancel_event=None):
        super().__init__()
        self.root = root
        self.signals = DirectoryDiscoverySignals()
        self._cancel_event = cancel_event or threading.Event()

    def cancel(self):
        self._cancel_event.set()

    @Slot()
    def run(self):
        try:
            directories = discover_immediate_directories(self.root, self._cancel_event)
            if directories is None or self._cancel_event.is_set():
                self.signals.cancelled.emit()
            else:
                self.signals.finished.emit(directories)
        except Exception as exc:
            if self._cancel_event.is_set():
                self.signals.cancelled.emit()
            else:
                self.signals.error.emit((type(exc), exc, traceback.format_exc()))

def canonical_project_path(path):
    """Use the same canonical form as the project scanner and DB lookups."""
    return os.path.normcase(os.path.normpath(os.path.abspath(str(path))))


def register_project_folders(db, folders):
    """Register selected folders, returning added/skipped/failed details."""
    existing = {
        canonical_project_path(project["path"])
        for project in db.get_all_projects()
        if project.get("path")
    }
    seen = set(existing)
    summary = {"added": [], "skipped": [], "failed": []}

    for folder in folders:
        path = canonical_project_path(folder)
        if path in seen:
            summary["skipped"].append(path)
            continue
        try:
            project_id = db.create_project(os.path.basename(path), path, existing_ok=False)
            if project_id is None:
                raise RuntimeError("database did not return a project id")
            seen.add(path)
            summary["added"].append(path)
        except sqlite3.IntegrityError as exc:
            try:
                existing_project = db.get_project_by_path(path)
            except Exception as lookup_exc:
                summary["failed"].append((path, f"{exc}; duplicate lookup failed: {lookup_exc}"))
            else:
                if existing_project:
                    seen.add(path)
                    summary["skipped"].append(path)
                else:
                    summary["failed"].append((path, str(exc)))
        except Exception as exc:
            summary["failed"].append((path, str(exc)))
    return summary


class ProjectRegistrationWorker(QRunnable):
    """Register selected folders in a worker; it is deliberately not cancellable."""

    def __init__(self, db_path, folders):
        super().__init__()
        self.db_path = db_path
        self.folders = list(folders)
        self.signals = WorkerSignals()

    @Slot()
    def run(self):
        try:
            db_manager = DatabaseManager(self.db_path)
            summary = register_project_folders(db_manager, self.folders)
            self.signals.finished.emit(summary)
        except Exception as exc:
            self.signals.error.emit((type(exc), exc, traceback.format_exc()))


class ProjectLoadWorker(QRunnable):
    """
    QRunnable to load projects from the database asynchronously.
    Emits finished(result) or error((exctype, value, traceback)).
    Accepts db_path (str), not a DatabaseManager instance.
    """
    def __init__(self, db_path):
        super().__init__()
        self.db_path = db_path
        self.signals = WorkerSignals()

    @Slot()
    def run(self):
        try:
            db_manager = DatabaseManager(self.db_path)
            projects = db_manager.get_all_projects()
            # Optionally add/override fields, but always preserve all originals
            result = []
            for p in projects:
                merged = dict(p)  # Start with all DB fields (including 'id')
                # Optionally add/override UI-specific fields
                merged["type"] = p.get("type", "Unknown")
                merged["version"] = p.get("version", "N/A")
                merged["last_modified"] = p.get("last_modified", p.get("created_at", ""))
                result.append(merged)
            self.signals.finished.emit(result)
        except Exception as e:
            import traceback
            self.signals.error.emit((type(e), e, traceback.format_exc()))
