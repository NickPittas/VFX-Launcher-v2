> Research handoff from GLM 5.3 Flash. This is the delegate's source inventory,
> preserved for planning; claims have not yet been independently reviewed or
> verified in a running application. Repository-state observations describe the
> agent's snapshot and may differ from the current migration branch. No tests or
> builds were run. Some legacy implementation details require reconciliation.

# VFX Launcher → Rust/egui Rewrite: Research Inventory

Read-only research complete. Scope honored: root only + `gitclones/` (coordinator-made clones) + capped `gh api` (pre-clone, allowed repos only, no clones by me). No writes, no tests/builds/delegates, all commands `timeout 10–30s`. Skipped as private: `CleanIncomings/operations.db`, `user_settings.json`. `app_settings.ini` read only because it is git-tracked (template).

---

## 1. Current Launcher (source-verified)

**Stack:** Python/PySide6 + SQLite. `requirements.txt` = `PySide6`, `watchdog` — **watchdog is dead** (no `core/file_watcher.py` in tree; declared in `requirements.txt:2` and `vfx_launcher.spec:35-37` only). Packaging: PyInstaller one-file (`vfx_launcher.spec`), AppImage (`build_appimage.sh`, recreates folders from `ui_slick/structure.txt`), Inno Setup (`installer.iss`).

**Entry:** `ui_slick/main_slick.py:main()` — logging to `$LOCALAPPDATA` (falls back `~/VFX_Launcher/logs` on Linux), `ConfigManager` → `DatabaseManager(db_path)` → `SlickMainWindow(db_manager, config_path, user_data)`. **Identity is hardcoded** `{"username": "admin", "is_admin": 1, "id": 1}` (main_slick.py ~L75) — no login; missing DB is recreated after a warning dialog.

**Architecture:** `SlickMainWindow` (`main_window.py`) = sidebar + QStackedWidget: Projects (splitter: ProjectBrowser 30% | FilesBrowser 70%), NewProject, Users, Settings, Logs. UI state via `QSettings("VFX_Launcher","SlickUI")` (geometry, splitter, panel index, sidebar, selected project, active tab, column states + `columns/cleared_v3` one-shot migration, main_window.py:40-48, 431-515).

### Workflows (file → function)

| Workflow | Location | Mechanics |
|---|---|---|
| Browse projects | `ui_slick/project_browser.py` (1161 L) | QStandardItemModel + per-tab proxies (`IdFilterProxyModel`), tabs All/Favorites/Recents (L703-730), star toggle via `FavoriteStarDelegate`, search filter, multi-select context menu: Open / Remove (multi, `db.delete_project` transactional) / **Archive (stub, L604)** / **Duplicate (stub, L645)** / Open Containing Folder |
| Add multiple projects | `project_browser.py:_on_add_multiple_projects` → `async_workers.py:discover_immediate_directories` (immediate children only) → `_FolderSelectionDialog` → `register_project_folders` (`canonical_project_path` = `normcase(normpath(abspath))`, IntegrityError-safe summary added/skipped/failed) |
| Scan | `scan_progress_dialog.py:ScanManager` → `scanner.py` (1433 L): two-phase — `FolderMatchWorker` (recursive folders named from config `project_directories`) → `FileSearchWorker` (`.nk/.aep`, **excludes `.nk~`, `.nk.autosave`**, L410-423) → `DbUpdateWorker._update_database` (L1284): "Projects" subfolder → parent-project path trick, upsert-by-filepath, version parse `_v(\d+)`, **deletion sweep guarded** against cancel/unmounted share (verified L1400-1425). Scan-all = sequential queue (`_start_scan_queue`) |
| Version selection & grouping | `tree_manager.py:FileTreeManager` + `file_widgets.py:VersionedFileWidget` (per-file version QComboBox), `file_handlers.py` (group_by_shot/group_by_timeline/group_versioned_files, regex `(?:SH|sh)[_-]?\d{2,3}`) |
| Launch | `files_browser.py:_on_file_double_clicked` → `log_file_access` → `main_window.py:_on_file_launched` (~L343-422): reads `app_settings.ini` fresh each time. Windows `.nk/.nknc` → `[nuke_path, --nukex, file]`; **Linux → `shlex.split(nuke_launch_cmd_linux) + [file]`**; `.aep/.aet` → `after_effects_path`; other → `os.startfile`/`xdg-open`; env sanitized by `core/utils.py:get_external_process_env` (strips PyInstaller `LD_LIBRARY_PATH`) |
| New project | `new_project_panel.py:_on_create`: `NNNNN_Client_Name_YYYY-MM-DD` (NAME_RE `^(\d+)_.*\d{4}-\d{2}-\d{2}`), next-code scan of root, fuzzy client autocomplete (`_subsequence`), template from `ui_slick/Structure/` with **fallback manifest `structure.txt` (33 dirs)**; `DEFAULT_ROOT = "/mnt/Projects"` hardcoded (L20); underscore validation; clients list in QSettings |
| Users | `user_management.py`: async load, add/edit/delete dialogs (no auth), `RoleBadgeDelegate`; `core/user_actions.py:UserActionLogger._log_action` persists to `user_activity_log` (best-effort) |
| Settings | `settings_panel.py`: app paths (**Windows-only `.exe` validation**, L157-166), project dir names, scan interval (**no timer consumes it — dead**), log level → writes `app_settings.ini` |
| Logs | `log_panel.py`: live handler → text view |
| Context ops | `file_context_menu.py`: Open / Open Containing Folder (cross-platform `open_in_file_manager`, fixed) / Copy Path / File Info |

## 2. Compatibility obligations

**SQLite schema** (`core/database.py:_initialize_database` — authoritative DDL): `users`, `projects` (UNIQUE `path`; `folders TEXT` added at runtime via `ALTER TABLE` in `update_project_folders`), `project_files`, `recent_projects` (active: project_browser.py:963,1082), `favorite_projects` (**table exists but no UI callers — dead**), `file_access_log` (active), `user_activity_log`; default `admin` user seeded. No WAL/busy_timeout pragma (only `PRAGMA foreign_keys`). `add_project_file` upserts by filepath — filepath is a de-facto unique key with no UNIQUE constraint (multi-project overlap "fix" moves a file to the last-scanned project).

**INI config** `app_settings.ini` (configparser; frozen builds copy to `~/.config/VFX_Launcher/app_settings.ini`, `core/config.py:resolve_app_settings_path`): `[Paths] nuke_path / after_effects_path / nuke_launch_cmd_linux`; `[Projects] project_directories` (comma-separated folder **names**), `scan_interval_minutes` (dead); `[Database] db_path` (relative resolved vs config dir); `[Logging] log_level / log_file`(unused). Legacy DB fallback: if resolved path missing, reuse `$APPDATA/VFX_Launcher/vfx_launcher.db` (`get_database_path`).

**Side-car state:** favorites = per-user JSON `~/.vfx_launcher/favorites_<user>.json` (`favorites_manager.py`); UI state = QSettings; NAS-Backup config = `~/.config/NAS Backup Manager/config.json` + job JSON; FolderStructurer = `clients.json` + `Templates/` on disk; CleanIncomings = `config/patterns.json` + `config/profiles.json` + `user_settings.json`.

## 3. Five tools to absorb (local clones, source-verified)

1. **FolderStructurer** (PyQt5+WebEngine/QWebChannel): entry `main.py` (`AppWebPage.handle_*`), core `common/backend_api.py` (`get_default_sphere_config`/`get_default_kent_config`, `compute_next_project_number`, `build_project_root_name`, `build_structure_preview_text`, template CRUD, `load_clients`/`add_client`), `common/nuke_template.py:build_nuke_script_template` (embeds `.nk` XML; default `<seq>_<shot>_comp_v001.nk`), sequences/shots decoupled. **Superset of Launcher's New Project panel** + template editor + nk generation.
2. **CleanIncomings** (customtkinter, config-driven normalizer): `python/mapping.py:MappingGenerator` (filename-only matching — invariant per `IMPORTANT.md`), `patterns.json` (shot/task/resolution/version/asset/stage; ARRI/RED/Sony/Canon/DJI camera-file regexes), `profiles.json` (dest folder → pattern keys; mirrors standard tree: `3D/Renders`, `Projects/Nuke`, `Video/Footage`), `python/fileops.py` (1377 L copy/move incl. image sequences), `python/scanner.py`, preview tree, multi-stage progress.
3. **SearchFolderStructures**: `FIelOrganizer.py` (PyQt5 AI organizer — providers `OpenRouterLLM`/`LMStudioLLM`/`MistralLLM`/`OllamaLLM`, all sharing `invoke(prompt)`; results table; `move/copy_selected_files`; `expand_sequence_files`; KENT/Sphere templates), `FilelOrganizer_MT.py` (QThread worker variant, `langchain_ollama`), `ai_folder_scanner.py` (tkinter scan→JSON, sequence grouping), `folder_structure_to_json.py` (CLI folder→nested dict, files under `__files__` key). AI integration is the reference for the undecided external-harness choice.
4. **File-Extention-Copier** (tkinter, 216 L, `file-copier-ui.py`): extension-set walk (default `.nk`/`.autosave`), `shutil.copy2` preserving relative structure, thread+cancel, count progress. Trivial absorb.
5. **NAS-Backup**: engine `backup_manager.py:BackupEngine` — shell `find … -iname '*.aep' -o -iname '*.nk'` with `-prune` excludes (`_build_find_command`, **Linux-only**), mtime diff (`_should_copy`, stat-fail ⇒ copy), ThreadPoolExecutor 1–16, rename-before-overwrite `{n}_{stem}{suffix}` (`_rename_existing`), pause/resume/stop, run reports (totals only), config autosave, job import/export, QTimer scheduler (interval default 240 min / daily), headless `--dry-run`; Qt UI `backup_manager_qt.py`; exclude semantics = case-insensitive fnmatch on path components (`_is_excluded`).

## 4. Overlap map (consolidation opportunities)

- **Standard folder template**: Launcher `structure.txt` ≈ FolderStructurer Sphere/KENT ≈ KENT.json ≈ CleanIncomings profiles → one canonical structure + template editor.
- **Project creation**: Launcher NewProject + FolderStructurer (client fuzzy DB, numbering, nk generation) → merge into one module.
- **.nk/.aep discovery**: Launcher scanner / NAS-Backup find / copier walk / ai_folder_scanner → one walker + filter engine.
- **Copy/move preserving structure**: copier + CleanIncomings fileops + NAS-Backup copy → one transfer engine with pluggable rename/conflict policy.
- **Version extraction** `_v(\d+)`: Launcher vs CleanIncomings version patterns → unify (mind zero-padding).
- **Sequence detection**: exists only in CleanIncomings/ai_scanner — Launcher lacks it; candidate to promote.
- **AI classification**: only SearchFolderStructures; provider abstraction is thin (`invoke(prompt)`) — maps cleanly onto "AI via external harness, undecided".

## 5. Cross-platform / file-transfer edge cases

- **Path canonicalization is load-bearing**: DB lookups depend on `canonical_project_path` (`async_workers.py:62`), `scanner._normalize_path/_canonical_path`, plus legacy raw-spelling fallbacks (scanner.py ~L1330). Rust must reproduce exactly or migrate DB rows.
- **Unmounted shares**: deletion sweep guard (scanner.py L1400-1425) — port the rule "skip sweep when cancelled or root unreachable"; NAS-Backup treats stat-failure as "copy".
- **Shell-outs**: NAS-Backup `find|tr` (Linux-only); Launcher scanner had `dir`/`find` fallbacks (CODE_REVIEW #23). Rust: pure directory walk, no shell.
- **Launch env hygiene**: `get_external_process_env` (PyInstaller leak) — not needed for a native Rust binary, but keep per-OS launch semantics (Windows `--nukex`; Linux full command line with file appended).
- **SQLite over SMB**: README claims network-shared DB is fine ([README claim]); source has no WAL/busy_timeout — multi-user over SMB is fragile and needs a deliberate strategy.
- **Autosave exclusions**: Launcher excludes `.nk~`/`.nk.autosave`; copier *targets* `.autosave` — reconcile per-tool intent.
- **Case/separator drift**: ext4 vs NTFS/SMB casing; `normcase` behavior is OS-dependent — a Rust `normcase` equivalent must be defined per-target.
- **Concurrent scans**: per-thread SQLite connections today (`DatabaseManager._thread_connections`, finalizer-closed); scan-all queue serializes scans. Rust: single write connection or serialized pool; keep scan-queue serialization semantics.
- **Windows-only validation**: settings `.exe` check rejects valid non-Windows executables (still present).

## 6. Stale-doc register (distinguish claims vs source)

- **README.md**: mostly stale — lists files that don't exist (`database_manager.py`, `config_manager.py`, `project_panel.py`, `user_panel.py`, `login.py`); claims "Folder Monitoring"/watchdog (dead), `config.ini` (file is `app_settings.ini`); schema section says `files`/`file_access_log(user)` — actual tables differ (`project_files`, plus 3 extra tables). DB schema in §2 above is the real contract.
- **CODE_REVIEW_FINDINGS.md (2026-09-02, 32 findings)**: many since fixed and verified in tree — search no-op (files_browser.py:331), users panel imports/ctor, scanner completion timer (`self._completion_timer`), cross-thread GUI bridge (`ScannerSignalRelay`), Linux startfile, transactional delete, escaped LIKE (`get_file_versions`), `update_project_folders` exists, connection pool removed, sweep guard. **Still open / to port consciously**: hardcoded admin identity, Archive/Duplicate stubs, dead `scan_interval_minutes`, hardcoded `/mnt/Projects`, `.exe` validation, `favorite_projects` table unused vs JSON favorites (two favorite models), `get_latest_version_file` tie-break, watchdog dependency unused, QSS `transition` warnings note.

## 7. Unknown decisions (for planning, not answerable from code)

1. AI harness/provider contract (external runner vs in-app; SearchFolderStructures `invoke()` shape is the only precedent).
2. Config strategy: keep readable/writable `app_settings.ini` (compat) vs new format + importer.
3. DB compatibility bar: same schema byte-for-byte vs migration on open (incl. `folders` column, canonical-path normalization of existing rows).
4. UI-state store replacement for QSettings (JSON in config dir recommended) and whether old QSettings values should be imported.
5. Identity: keep hardcoded admin or introduce real user selection (affects `user_id` FKs).
6. Watchdog: drop entirely vs implement the README-promised folder monitoring in Rust (notify crate).
7. Packaging targets: AppImage + MSI/Inno-equivalent + single exe; whether NAS-Backup headless CLI mode becomes a subcommand.
8. Suite shape: single app with tool tabs vs one binary with subcommands.

## 8. Recommended research inputs

**For Astra (planning):** verbatim DDL from `_initialize_database`; compat matrix from §2 (INI keys, legacy DB path fallback, favorites JSON, structure.txt manifest, canonical-path contract with legacy backslash fallbacks); scan algorithm contract (two-phase, exclusions, `_v(\d+)`, upsert-by-filepath, sweep-guard rules, "Projects"-parent trick, scan-all queueing); launch semantics table per OS/env; one capability spec per absorbed tool citing the functions in §3 (especially CleanIncomings `IMPORTANT.md` invariants: filename-only matching, all patterns in `patterns.json`, profiles hold only root+structure — carry these invariants into the Rust config design); NAS-Backup engine semantics (exclude fnmatch rules, `{n}_{stem}{suffix}` rename, report/job JSON shapes, scheduler); concurrency model mapping (Qt QThreadPool/SignalRelay → egui channels + poll); prior-findings register (§6) as a "don't regress" list; packaging mapping (spec/iss/AppImage → Rust equivalents). Note: `AGENTS.md` exists on branches `t3/rust-migration-plan-contracts` and `t3/merged-launcher-ui-mockup` (44 lines, matches coordinator constraints; main branch has none) — the two t3 branches are the only non-main refs and are placeholders, no plan/mockup content yet.

**For Kimi (UI designer, HTML mockup first):** screen inventory with current behaviors: Projects (3 tabs, star column, search, add/multi-add/scan-all buttons, status bar), Files (Nuke/AEP tabs, tree grouping, per-file version combos, search, Scan button, dbl-click launch, context menu: Open/Reveal/Copy Path/Info), New Project form (fields, live `NNNNN_Client_Name_YYYY-MM-DD` preview, validation), Users (table, role badges, initials avatars), Settings (3 tabs), Logs, Scan Progress dialog (progress+ETA+live log+cancel), multi-add folder-selection dialog; dark palette tokens from `ui_slick/styles.qss`; design aspirations from `.windsurf/rules/uirules.md` (toasts, breadcrumbs, skeleton/spinner states, badges, shortcuts) — already the stated direction, mockup should realize them; async/error states inventory (Launch Error, Project Not Found, registration summary added/skipped/failed, copy conflict previews); template-editor mockup for absorbed FolderStructurer tabs; pluggable AI panel placeholder (provider undecided — keep a provider-agnostic "Send to AI" affordance).

**Access notes:** no access failures. `gh api` worked before clones landed; all five clones present and readable under `gitclones/`. Sparse-clone nested dirs (e.g. `CleanIncomings/python|config`, `FolderStructurer/common`) materialized during the session and were used directly.

