Initial Setup:
 Setup virtual environment and install dependencies (requirements.txt).

 Initialize SQLite database (database.py).

User Management:
 Create UI for login (login.py).

 Create admin UI to manage users (user_management.py).

 Implement CRUD operations for user records (database.py).

Project Management:
 Create UI for main project selection (main_window.py).

 Add ability to add new projects to DB (main_window.py + database.py).

 Implement recursive project scanning based on configured directories (project_scanner.py).

 Implement file filtering logic (.nk and .aep) and version extraction (utils.py).

Version Management:
 Develop UI tree-view with project structure (project_browser.py).

 Implement version detection (_v### format) and display in dropdown (utils.py).

 Ensure latest version always selected by default.

App Launcher Integration:
 Implement launching Nuke (--nukex) or After Effects with selected file (app_launcher.py).

Real-Time Logging:
 Setup logging to write actions to log file (log_panel.py, Python logging module).

 Create UI panel to display logs in real-time (log_panel.py).

Settings Management:
 Develop settings UI (settings_panel.py).

 Save/load settings from INI (config/app_settings.ini).

File Watcher Implementation:
 Implement filesystem watcher using watchdog, periodic scan (~1 min intervals) (file_watcher.py).

 Auto-update project file tree with detected changes.

Favorites and Recent Projects:
 Implement tracking of recent projects opened.

 Add UI support to favorite/unfavorite projects.

Additional UI Enhancements:
 Context menu for additional file actions (Open in Explorer, details).

 Project tree-view with sortable columns (version, last modified).