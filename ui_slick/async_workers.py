from PySide6.QtCore import QRunnable, Slot, Signal, QObject
from core.database import DatabaseManager

class WorkerSignals(QObject):
    finished = Signal(object)
    error = Signal(tuple)

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
