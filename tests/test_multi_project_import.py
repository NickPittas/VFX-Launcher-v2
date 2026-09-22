import os
import sys
import threading
import time
import unittest
from tempfile import TemporaryDirectory
from unittest.mock import Mock, patch

from PySide6.QtCore import QEvent, QThreadPool, QTimer
from PySide6.QtWidgets import QApplication, QDialog

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "..")))

from ui_slick.async_workers import (
    DirectoryDiscoveryWorker,
    ProjectRegistrationWorker,
    discover_immediate_directories,
    canonical_project_path,
    register_project_folders,
)
from ui_slick.project_browser import _FolderSelectionDialog, ProjectBrowser
from core.database import DatabaseManager


class FakeDatabase:
    def __init__(self, projects=None, failures=()):
        self.projects = list(projects or [])
        self.failures = set(failures)
        self.create_calls = []
        self.scan_calls = []
        self.get_all_projects_calls = 0

    def get_all_projects(self):
        self.get_all_projects_calls += 1
        return list(self.projects)

    def get_project_by_path(self, path):
        for project in self.projects:
            if project["path"] == path:
                return project
        return None

    def create_project(self, name, path, existing_ok=True):
        self.create_calls.append((name, path))
        if name in self.failures:
            raise OSError("simulated database failure")
        for project in self.projects:
            if project["path"] == path:
                return project["id"]
        project_id = len(self.projects) + 1
        self.projects.append({"id": project_id, "name": name, "path": path})
        return project_id


class MultiProjectImportTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.app = QApplication.instance() or QApplication([])

    def test_discovery_is_shallow_sorted_and_includes_hidden_directories(self):
        with TemporaryDirectory() as temp:
            root = os.path.join(temp, "root")
            os.makedirs(os.path.join(root, ".hidden"))
            os.makedirs(os.path.join(root, "zeta", "nested"))
            os.makedirs(os.path.join(root, "alpha"))
            open(os.path.join(root, "not-a-directory.txt"), "w").close()

            folders = discover_immediate_directories(root)

            self.assertEqual(
                [os.path.basename(path) for path in folders],
                [".hidden", "alpha", "zeta"],
            )
            self.assertNotIn(os.path.join(root, "zeta", "nested"), folders)

    def test_missing_root_reports_filesystem_failure(self):
        with TemporaryDirectory() as temp:
            with self.assertRaises(FileNotFoundError):
                discover_immediate_directories(os.path.join(temp, "gone"))

    def test_worker_cancellation_discards_partial_results(self):
        worker = DirectoryDiscoveryWorker("unused")
        outcomes = []

        class Entry:
            path = "/root/first"

            def is_dir(self):
                worker.cancel()
                return True

        class Entries:
            def __enter__(self):
                return iter([Entry(), Entry()])

            def __exit__(self, exc_type, exc, tb):
                return False

        worker.signals.finished.connect(lambda result: outcomes.append(("finished", result)))
        worker.signals.error.connect(lambda error: outcomes.append(("error", error)))
        worker.signals.cancelled.connect(lambda: outcomes.append(("cancelled", None)))

        with patch("ui_slick.async_workers.os.scandir", return_value=Entries()):
            worker.run()

        self.assertEqual(outcomes, [("cancelled", None)])

    def test_registration_skips_normalized_duplicates_and_deduplicates_selection(self):
        with TemporaryDirectory() as temp:
            existing = os.path.join(temp, "existing")
            new = os.path.join(temp, "new")
            os.makedirs(existing)
            os.makedirs(new)
            existing_alias = os.path.join(temp, "existing", "..", "existing")
            db = FakeDatabase([{"id": 7, "name": "old", "path": existing_alias}])

            summary = register_project_folders(db, [existing_alias, new, new])

            self.assertEqual(summary["added"], [canonical_project_path(new)])
            self.assertEqual(summary["skipped"], [canonical_project_path(existing), canonical_project_path(new)])
            self.assertEqual(summary["failed"], [])
            self.assertEqual(db.create_calls, [("new", canonical_project_path(new))])
            self.assertEqual(db.get_all_projects_calls, 1)

    def test_registration_aggregates_per_folder_failures(self):
        with TemporaryDirectory() as temp:
            good = os.path.join(temp, "good")
            bad = os.path.join(temp, "bad")
            os.makedirs(good)
            os.makedirs(bad)
            db = FakeDatabase(failures={"bad"})

            summary = register_project_folders(db, [good, bad])

            self.assertEqual(summary["added"], [canonical_project_path(good)])
            self.assertEqual(summary["failed"], [(canonical_project_path(bad), "simulated database failure")])
            self.assertEqual(db.get_all_projects_calls, 1)
            self.assertEqual(db.scan_calls, [])

    def test_registration_does_not_scan_nested_content(self):
        with TemporaryDirectory() as temp:
            selected = os.path.join(temp, "selected")
            os.makedirs(os.path.join(selected, "project", "deep"))
            db = FakeDatabase()

            summary = register_project_folders(db, [selected])

            self.assertEqual(len(summary["added"]), 1)
            self.assertEqual(len(db.create_calls), 1)
            self.assertEqual(db.create_calls[0][1], canonical_project_path(selected))
            self.assertEqual(db.scan_calls, [])

    def test_dialog_selection_controls_and_cancel_leave_selection_empty(self):
        dialog = _FolderSelectionDialog("/nas/root")
        dialog.set_folders(["/nas/root/a", "/nas/root/b"])

        self.assertFalse(dialog.add_button.isEnabled())
        dialog._select_all()
        self.assertTrue(dialog.add_button.isEnabled())
        dialog._select_none()
        self.assertFalse(dialog.add_button.isEnabled())
        dialog.reject()
        self.assertEqual(dialog.result(), QDialog.Rejected)
        self.assertEqual(dialog.selected_paths, [])
        dialog.deleteLater()
        self.app.processEvents()

    def test_qthreadpool_cancel_and_dialog_destroy_drop_late_result(self):
        cancel_event = threading.Event()
        started = threading.Event()
        cancelled = threading.Event()
        destroyed = threading.Event()
        dialog = _FolderSelectionDialog("/nas/root")
        dialog.destroyed.connect(lambda: destroyed.set())
        dialog.finished.connect(lambda _result: cancel_event.set())
        worker = DirectoryDiscoveryWorker("/nas/root", cancel_event)
        worker.signals.finished.connect(dialog.set_folders)
        worker.signals.cancelled.connect(cancelled.set)

        def blocked_discovery(_root, event):
            started.set()
            while not event.is_set():
                time.sleep(0.001)
            return None

        pool = QThreadPool()
        with patch("ui_slick.async_workers.discover_immediate_directories", blocked_discovery):
            pool.start(worker)
            self.assertTrue(started.wait(2))
            dialog.reject()
            dialog_result = dialog.result()
            dialog.deleteLater()
            self.app.processEvents()
            self.assertTrue(pool.waitForDone(2000))
        self.app.sendPostedEvents(None, QEvent.DeferredDelete)
        self.app.processEvents()

        self.assertTrue(cancelled.wait(1))
        self.assertTrue(destroyed.wait(1))
        self.assertEqual(dialog_result, QDialog.Rejected)

    def test_project_browser_destroyed_during_running_discovery_releases_job(self):
        with TemporaryDirectory() as temp:
            root = os.path.join(temp, "root")
            os.makedirs(root)
            db = DatabaseManager(os.path.join(temp, "browser.db"))
            with patch.object(ProjectBrowser, "_load_projects_async"):
                browser = ProjectBrowser(db, {"username": "destroy-running"})
            started = threading.Event()
            cancelled = threading.Event()

            def blocked_discovery(_root, event):
                started.set()
                while not event.is_set():
                    time.sleep(0.001)
                cancelled.set()
                return None

            def destroy_when_started():
                if started.is_set():
                    browser.deleteLater()
                else:
                    QTimer.singleShot(1, destroy_when_started)

            with patch("ui_slick.file_dialogs.pick_directory", return_value=root), patch(
                "ui_slick.async_workers.discover_immediate_directories", blocked_discovery
            ):
                QTimer.singleShot(0, destroy_when_started)
                browser._on_add_multiple_projects()
            self.app.processEvents()
            self.assertTrue(QThreadPool.globalInstance().waitForDone(2000))
            self.app.processEvents()

            self.assertTrue(started.is_set())
            self.assertTrue(cancelled.wait(1))
            self.assertIsNone(getattr(browser, "_multi_import_job", None))

    def test_project_browser_destroyed_after_discovery_releases_job(self):
        with TemporaryDirectory() as temp:
            root = os.path.join(temp, "root")
            child = os.path.join(root, "child")
            os.makedirs(child)
            db = DatabaseManager(os.path.join(temp, "browser.db"))
            with patch.object(ProjectBrowser, "_load_projects_async"):
                browser = ProjectBrowser(db, {"username": "destroy-terminal"})

            def destroy_after_terminal():
                job = getattr(browser, "_multi_import_job", None)
                if job is not None and job.terminal:
                    browser.deleteLater()
                else:
                    QTimer.singleShot(1, destroy_after_terminal)

            with patch("ui_slick.file_dialogs.pick_directory", return_value=root), patch(
                "ui_slick.async_workers.discover_immediate_directories", return_value=[child]
            ):
                QTimer.singleShot(0, destroy_after_terminal)
                browser._on_add_multiple_projects()
            self.app.processEvents()
            self.assertTrue(QThreadPool.globalInstance().waitForDone(2000))
            self.app.processEvents()

            self.assertIsNone(getattr(browser, "_multi_import_job", None))

    def test_registration_worker_creates_database_in_worker_and_returns_summary(self):
        with TemporaryDirectory() as temp:
            db_path = os.path.join(temp, "projects.db")
            folder = os.path.join(temp, "project")
            os.makedirs(folder)
            worker = ProjectRegistrationWorker(db_path, [folder])
            finished = []
            done = threading.Event()
            worker.signals.finished.connect(lambda summary: (finished.append(summary), done.set()))

            pool = QThreadPool()
            pool.start(worker)
            deadline = time.monotonic() + 5
            while not done.is_set() and time.monotonic() < deadline:
                self.app.processEvents()
                time.sleep(0.01)
            self.assertTrue(pool.waitForDone(5000))
            self.assertTrue(done.wait(1))

            self.assertEqual(len(finished), 1)
            self.assertEqual(finished[0]["added"], [canonical_project_path(folder)])
            projects = DatabaseManager(db_path).get_all_projects()
            self.assertEqual([project["path"] for project in projects], [canonical_project_path(folder)])

    def test_registration_race_classifies_unique_conflict_as_skipped(self):
        with TemporaryDirectory() as temp:
            db_path = os.path.join(temp, "projects.db")
            folder = os.path.join(temp, "project")
            os.makedirs(folder)
            DatabaseManager(db_path)
            barrier = threading.Barrier(2)
            summaries = []
            errors = []

            class SnapshotDatabase(DatabaseManager):
                def get_all_projects(self):
                    projects = super().get_all_projects()
                    barrier.wait(5)
                    return projects

            def add_from_snapshot():
                try:
                    summaries.append(register_project_folders(SnapshotDatabase(db_path), [folder]))
                except Exception as exc:
                    errors.append(exc)

            threads = [threading.Thread(target=add_from_snapshot) for _ in range(2)]
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join(10)

            self.assertEqual(errors, [])
            self.assertEqual(sorted(len(summary["added"]) for summary in summaries), [0, 1])
            self.assertEqual(sorted(len(summary["skipped"]) for summary in summaries), [0, 1])
            self.assertEqual([summary["failed"] for summary in summaries], [[], []])

    def test_registration_summary_refreshes_once_only_when_added(self):
        browser = ProjectBrowser.__new__(ProjectBrowser)
        browser._multi_registration_job = {"worker": object()}
        browser.loading_bar = Mock()
        browser.add_multiple_project_btn = Mock()
        browser._load_projects_async = Mock()
        browser._show_multi_registration_summary = Mock()

        ProjectBrowser._on_multi_registration_finished(browser, {
            "added": ["/new"], "skipped": ["/old"], "failed": [("/bad", "denied")],
        })
        browser._multi_registration_job = {"worker": object()}
        ProjectBrowser._on_multi_registration_finished(browser, {
            "added": [], "skipped": ["/old"], "failed": [],
        })

        self.assertEqual(browser._load_projects_async.call_count, 1)
        self.assertEqual(browser._show_multi_registration_summary.call_count, 2)

    def test_setup_error_reports_no_success(self):
        browser = ProjectBrowser.__new__(ProjectBrowser)
        browser._multi_registration_job = {"worker": object()}
        browser.loading_bar = Mock()
        browser.add_multiple_project_btn = Mock()
        with patch("PySide6.QtWidgets.QMessageBox.critical") as critical:
            ProjectBrowser._on_multi_registration_error(browser, (RuntimeError, RuntimeError("offline"), "trace"))

        self.assertIn("No projects were added", critical.call_args.args[2])
        self.assertIn("offline", critical.call_args.args[2])

    def test_summary_dialog_includes_all_failure_paths(self):
        browser = ProjectBrowser.__new__(ProjectBrowser)
        failures = [("/bad/a", "denied"), ("/bad/b", "gone")]
        with patch("PySide6.QtWidgets.QMessageBox") as message_box:
            ProjectBrowser._show_multi_registration_summary(browser, {
                "added": [], "skipped": ["/existing"], "failed": failures,
            })
            text = message_box.return_value.setText.call_args.args[0]

        self.assertIn("Added: 0", text)
        self.assertIn("Skipped existing: 1", text)
        self.assertIn("/bad/a: denied", text)
        self.assertIn("/bad/b: gone", text)


if __name__ == "__main__":
    unittest.main()
