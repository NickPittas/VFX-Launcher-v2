# VFX-Launcher-v2 — Code Review Findings

Date: 2026-09-02. Static review; every issue verified by reading the cited lines in the current working tree. Line numbers may drift after edits.

## Architecture (as-is)

Active app: `ui_slick/main_slick.py` → `SlickMainWindow` (sidebar + stacked panels: ProjectBrowser/FilesBrowser, NewProject, Users, Settings, Logs). Active scanner: `ui_slick/scanner.py` (`Scanner` + QRunnable workers) driven through `ui_slick/scan_progress_dialog.py` (`ScanManager`). SQLite via `core/database.py` (`DatabaseManager`), config via `core/config.py`. A large parallel "classic" layer is dead: `main2.py` imports a nonexistent `ui/` package, and nothing imports `core/app_launcher.py`, `core/file_watcher.py`, `core/project_scanner.py`, or `core/project_scanner_worker.py`.

## CRITICAL — user-facing breakage in the active app

1. **File search filter is a silent no-op** — `ui_slick/files_browser.py:335`
   `_on_search_changed` calls `self.display_filtered_files(filtered_files)`; the method is `_display_filtered_files` (`files_browser.py:610`). The `AttributeError` is swallowed by the `except` at 336–337, so typing in the search box logs an error and does nothing.

2. **Users panel: Add/Edit User always crashes** — `ui_slick/user_management.py:145,167` (+ `main_window.py:76`)
   `from ui.user_management import NewUserDialog` / `EditUserDialog` — the `ui/` package does not exist → `ModuleNotFoundError` on first use. Compounding: `main_window.py:76` passes the `DatabaseManager` *object* where `UserManagement.__init__(self, db_path, ...)` (`user_management.py:48`) expects a path; `user_management.py:147,169` then calls `DatabaseManager(self.db_path)` on that object → `TypeError` even if the import were fixed.

3. **Scan completion may never fire; dialog can hang** — `ui_slick/scanner.py:737–758`
   `on_all_file_searches_complete()` creates `completion_timer = QTimer()` as a **local** — garbage-collected before its 100 ms tick is reliably delivered; and it is created on a QThreadPool thread (see #4), where timers can't run. If the timer dies, `on_progress(100, …)` / `on_finished(project_path, files)` (lines 746–753) never run: the scan dialog never flips to "Close" and `main_window._on_scan_finished` never reloads the DB.

4. **Cross-thread GUI updates throughout the scan path** — `ui_slick/scanner.py:540,543,660–663`; `ui_slick/scan_progress_dialog.py:337–340`
   Plain Python callables (bound widget methods and closures) are connected to worker `signals.progress/log/file_found` — direct connections, so the slots execute on the emitting QThreadPool thread. `dialog.update_progress` (QProgressBar), `append_log` (QTextEdit), and `show_file_found` (QTreeWidgetItem creation) all run off the GUI thread → "QObject::setAttribute / paint must be called on the main thread" warnings and intermittent crashes. `completed_folders`/`remaining_folders` counters in `scanner.py` are also mutated from multiple pool threads without a lock (double/never completion).
   Fix shape (for reference): bridge via a QObject signal relay on the main thread (queued connections) and store the timer as `self._completion_timer` (created on the GUI thread).

5. **Config split-brain: app reads a config file that doesn't exist and a key that's never set** — `core/config.py:51,63,78–80,91` vs `app_settings.ini`
   `ConfigManager._find_config_file()` looks for `config.ini`; the repo ships `app_settings.ini`. So startup always falls to `_set_defaults()`: DB path becomes `$APPDATA/VFX_Launcher/vfx_launcher.db` (on Linux `~/VFX_Launcher/…`, since `APPDATA` is unset — `config.py:73–76`), i.e. **not** the `./vfx_launcher.db` configured in `app_settings.ini [Database] db_path`. Even pointing at the right file wouldn't help: code reads key `Database/Path`, the ini defines `db_path`. Meanwhile `SettingsPanel` *writes* `app_settings.ini`, and only the scanner's folder-name list reads it — two divergent sources of truth for settings.

## HIGH

6. **All three project tabs share one filter proxy** — `ui_slick/project_browser.py:184–205,392–442`
   `tree`, `favorites_tree`, `recents_tree` are all `setModel(self.proxy_model)` (245). `_filter_projects` mutates the *shared* proxy's filter role/column/regex per tab, so switching tabs re-filters every tree and typing in search wipes the favorites/recents restriction (Favorites and Recents tabs can show identical rows). Needs one proxy per tree.

7. **"Open Containing Folder" always fails on Linux** — `ui_slick/project_browser.py:576`, `ui_slick/file_context_menu.py:101`
   Bare `os.startfile(folder_path)` — Windows-only API (`AttributeError` on Linux/macOS). `main_window.py:349–352` already has the correct cross-platform pattern (`hasattr(os, 'startfile')` → `xdg-open` fallback) to copy.

8. **Cancelled scan mass-deletes DB rows** — `core/project_scanner_worker.py:196–198,289–294` *(currently dead module — becomes data loss if wired back in)*
   Stop during `_scan_files` breaks the walk and returns partial results; `run()` still calls `_sync_database(all_files)`, whose removal pass (289–294) deletes every DB row not in the partial found set — with **no on-disk existence check** (unlike `ui_slick/scanner.py:1144`). A mid-scan cancel wipes legitimate rows.

9. **Deletion sweep can mass-delete when a share is unmounted** — `ui_slick/scanner.py:1138–1153` *(live path)*
   The sweep deletes rows whose normalized path fails `os.path.exists()`. If the project lives on a network share that's temporarily unmounted, every file row fails the check → all rows deleted on one scan.

10. **`ScannerManager.is_scanning` sticks True on error/stop** — `core/project_scanner.py:238–243,253–264` *(dead module)*
    Reset only in the `finished` handler; `error` or user stop leaves `is_scanning = True` forever → every later `start_scan` returns False.

11. **Launch path in `main_slick` assumes admin, DB silently re-created** — `ui_slick/main_slick.py:76–79`
    `user_data = {"username": "admin", "is_admin": 1, "id": 1}` — no login step (the classic app's `LoginDialog` died with `ui/`); a missing/moved DB is silently replaced by a fresh empty one (combined with #5, the app may quietly open a different DB than intended and show "no projects").

## MEDIUM

12. **SQLite connections are never closed; pool is a dead landmine** — `core/database.py:186–215`
    `get_connection()` caches per-thread connections in `_thread_connections` and the `finally` is a no-op `pass` — every distinct thread leaks an open connection for the process lifetime (watchdog/QRunnable threads multiply these). The `elif self.connection_pool:` branch (189–192) would hand a connection created in thread X to thread Y — `check_same_thread=True` guarantees `sqlite3.ProgrammingError` — but nothing ever appends to the pool, so it's dead code awaiting a victim.

13. **`delete_project` is not transactional** — `core/database.py:495–538`
    Four sequential autocommitted DELETEs (project_files, recent_projects, favorite_projects, projects). Failure partway leaves children already gone and the project row orphaned/inconsistent — no rollback.

14. **`get_file_versions` LIKE pattern unescaped + prefix over-match** — `core/database.py:525–543`
    `filename LIKE '{base}%'` — `_`/`%` in the base act as wildcards (`shot_01` matches `shotX01…`), and plain prefix matching returns unrelated files (`shot` matches `shot_final_v1.nk`). Needs `ESCAPE` clause + a version-token boundary.

15. **`get_all_logs` queries a table that is never created** — `core/database.py:29–42` vs `_initialize_database` (76–149)
    Schema creates users/projects/project_files/recent_projects/favorite_projects/file_access_log — no `logs`. The method always raises `OperationalError` and returns `[]` (callers can't distinguish "no logs" from "broken"). A stray duplicate class docstring at 44–47 suggests a bad merge.

16. **User action logging is a silent no-op** — `core/user_actions.py:209–224`
    `UserActionLogger` accepts `db_manager` but `_log_action` only writes to the Python log ("Future enhancement: persist to database"). The one live caller (`user_management.py:206–210`, on user removal) wraps it in `try/except: pass` — user actions are not durably recorded anywhere.

17. **`update_project_folders` called behind a `hasattr` guard that never passes** — `ui_slick/scanner.py:558–562`
    No such method exists anywhere in `core/database.py` (verified by search), so matched target folders are never persisted; quick scans silently fall back to weaker root-level name matching.

18. **Blocking I/O on the GUI thread per file launch** — `ui_slick/files_browser.py:264–267,372–393`
    Double-click does a synchronous `log_file_access` DB write + `_refresh_current_files` full DB read + tree rebuild before launching. Same pattern per tab switch (`project_browser.py:399` — synchronous `get_recent_projects`).

19. **Windows-path fallback lookups bake cross-platform path collisions in** — `ui_slick/main_window.py:323–341`, `ui_slick/scanner.py:988–1006,1044–1052`
    Project lookup tries `path.replace('/', '\\')` variants; `_normalize_path` only swaps separators (no `normpath`/case-folding), so the same directory can reach the DB under several spellings → duplicate project rows and `projects.path UNIQUE` blocking re-adds of the canonical spelling.

20. **`main_window.py:178` NameError landmine** — fallback branch `DatabaseManager(self.config_path)` references a name never imported in that module, and passes a config *directory* as a SQLite path. Currently unreachable (`self.db_manager` always set).

21. **Constructor-argument mismatches when wiring panels** — `ui_slick/main_window.py:74–77`
    `ProjectBrowser` gets `db_manager.db_path` (string), while `UserManagement`/`SettingsPanel` get the `DatabaseManager` object stored into a parameter named `db_path` (see #2; `SettingsPanel` masks it by re-resolving config itself and never using `self.db_path`).

22. **Config robustness** — `core/config.py:126–131,40–45`
    `save()` opens `self.config_file, 'w'` without `makedirs` (crash if dir missing); `config.read()` doesn't catch `configparser.Error` (malformed ini crashes startup); an existing-but-empty ini skips `_set_defaults()` so defaults silently don't apply.

23. **Scanner `dir`/`find` shell-outs** — `ui_slick/scanner.py:436–446` (~270–280)
    Windows `cmd /c dir /b /s /ad` string via `shell=True`; stderr is read only after `wait()`, so a chatty `find`/`dir` (permission warnings over a big/NFS tree) can fill the 64 KB pipe and deadlock the scan. Empty-result fallback triggers a full `os.walk` rescan, doubling work on legitimately empty or slow trees.

24. **`UserLoadWorker` duplicated with incompatible contracts** — `ui_slick/async_workers.py:46–71` vs `ui_slick/user_management.py:20–41`
    Two same-named QRunnables: one takes a path string, the other a manager instance. The active wiring only works by accident (see #2/#21). `LogLoadWorker` (`async_workers.py:17–30`) has zero callers.

## LOW

25. **CWD-relative icon path** — `ui_slick/tree_manager.py:70`: `QIcon("icons/folder_icon.png")` breaks when launched outside the repo (compare the module-dir pattern in `get_file_icon`, 57–63).
26. **Duplicate `currentIndexChanged` connection** — `ui_slick/tree_manager.py:239–247` double-connects the version combo already connected in `file_widgets.py:139–140`.
27. **Dead table-based log methods** — `ui_slick/log_panel.py:130–170`: `_populate_logs`/`_filter_logs`/`_export_logs` reference undefined `self.table` and unimported `QTableWidgetItem`/`QFileDialog`; would crash if ever called.
28. **Favorites fragility** — `ui_slick/favorites_manager.py:25–27` swallows all JSON write errors; delegate path can persist `str(None)` = `"None"` into favorites.json (no None guard), creating an unremovable phantom favorite.
29. **Hardcoded `/mnt/Projects`** — `ui_slick/new_project_panel.py:19–20,178–181`: project codes are always derived from `DEFAULT_ROOT` regardless of the user-chosen root (site assumption baked in).
30. **`setup_logger` clobbers root handlers** — `core/utils.py:26–62`: removes all existing root handlers; `main_slick.py` configures its own at import — whichever runs last wins.
31. **Version tie-break arbitrary** — `core/utils.py:118–138`: files without a version all key to 0; ties resolve to whichever sorted first.
32. **Dead-module bugs kept for the record** — `core/app_launcher.py`: `Popen(..., stdout=PIPE, stderr=PIPE)` never drained (launched app can block on a full pipe; GUI-thread `time.sleep(1)` at 110–118; POSIX zombies), `Path(filepath).as_posix()` at 144 contradicts the "keep native separators on Windows" rule five lines earlier in `launch_nuke`, and `open_containing_folder` (200–207) opens the *grandparent* when the file is missing (double `dirname`). `core/file_watcher.py`: watchdog callbacks write through the shared `db_manager` from observer threads, `restart()` (127–133) races the `run()` loop over `self.observer`/`is_running`, and `stop()` (243–247) never joins → "QThread destroyed while thread is still running" crash at shutdown.

## Dead / duplicate code inventory (verified: no importers)

- `main2.py` — imports nonexistent `ui.login`/`ui.main_window`; the classic entry point is dead.
- `core/project_scanner.py` — older parallel scanner (different progress signature); unreferenced.
- `core/project_scanner_worker.py` — per-project scanner; unreferenced (data-loss bug #8 lives here).
- `core/app_launcher.py` — unreferenced; active launch logic is in `main_window.py:323–352`.
- `core/file_watcher.py` — unreferenced; no live file watching exists.
- `ui_slick/progress_dialog.py` — duplicate of `scan_progress_dialog.py`; `ScanManager` imports only the latter.
- `ui_slick/async_workers.py::LogLoadWorker` + `core/database.py::get_all_logs` — a dead DB-logs feature pair (see #15).
- `ui_slick/log_panel.py:130–170` — dead methods (see #27).
- `core/database.py::connection_pool`/`max_pool_size` — never populated (see #12). Note `create_thread_safe_manager` IS used by `scanner.py`'s `DbUpdateWorker`.

## Hygiene notes

- `vfx_launcher.db` (136 KB, live data) and ~4.4 MB of freepik images are committed at the repo root.
- `db_check.py:11` hardcodes the relative DB path (wrong DB whenever the #5 default kicks in).
- `clean_orphan_projects.py` requires hand-editing `VALID_PROJECT_ROOTS` before it does anything (safe, but duplicated schema knowledge).

## What was checked and found clean

- SQL injection: none — all queries use `?` parameters throughout `core/database.py`.
- Favorites/recent/user CRUD helpers themselves are parameterized and reasonable.
- `vfx_launcher.spec` packaging is consistent with the dead-file layout (doesn't package `main2.py`).
