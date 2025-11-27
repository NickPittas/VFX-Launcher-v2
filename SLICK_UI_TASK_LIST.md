# Slick UI Parity & Feature Task List

**RULE: Always update this file before proceeding with any next implementation step.**

## Task Priority Legend
- [HIGH] = Critical for user workflow or major missing feature
- [MEDIUM] = Important for usability, but not blocking core flows
- [LOW] = Nice-to-have, polish, or new features

## Task List (Sorted by Importance)

### [HIGH] 1. Project Browser: Add Project Button & Functionality ✅
- Add a button to open a folder picker and add a new project to the database.
- UI: Button in Project Browser panel.
- DB: Requires working database connection.
- **Dependencies:** Database connection must be functional.
- **Status:** Complete. Button implemented, async add and refresh logic in place.

### [HIGH] 2. Project Browser: Remove Project Button & Functionality ✅
- Add a button/context menu to remove the selected project from the database.
- UI: Button or context menu in Project Browser panel.
- **Dependencies:** Project list must be loaded/displayed.
- **Status:** Complete. Robust selection, proxy/source mapping, and error handling implemented. Extensive logging added for diagnostics. Removal now works for all valid selections and updates UI/database correctly.

### [HIGH] 3. Project Browser: Refresh Button ✅
- Add a button to reload the project list from the database.
- UI: Button in Project Browser panel.
- **Dependencies:** Project list must be loaded/displayed.
- **Status:** Complete. Button added, triggers async reload and logs actions. UI now supports full add/remove/refresh cycle.

### [HIGH] 4. Project Browser: Version Dropdown (Per-File Version Selection)
- Implement a custom delegate for version selection in the project tree view.
- UI: Version column with dropdown in tree view.
- **Dependencies:** Project list and file details must be loaded.

### [HIGH] 5. User Management: Add User Dialog & Functionality ✅
- Add dialog for admin to add new users (username, role, etc.).
- UI: Add button and dialog in User Management panel.
-- **Dependencies:** Database connection; admin access control.
- **Status:** Complete. Classic dialog logic reused for robust validation and DB integration. Async reload after user creation.

### [HIGH] 6. User Management: Edit User Dialog & Functionality ✅
- Add dialog for editing users (username, role, etc.).
- UI: Edit button/context menu and dialog in User Management panel.
- **Dependencies:** User list must be loaded/displayed.
- **Status:** Complete. Classic dialog logic reused for robust validation and DB integration. Context menu added for edit action. Async reload after edit.

### [HIGH] 7. User Management: Delete User Functionality ✅
- Add button/context menu for deleting users.
- UI: Delete button/context menu in User Management panel.
- **Dependencies:** User list must be loaded/displayed.
- **Status:** Complete. Classic logic reused, context menu action added, self-delete prevented. Async reload and robust error handling in place.

### [HIGH] 8. User Management: Admin-Only Access Control ✅
- Ensure only admin users can access user management features.
- **Dependencies:** User authentication and role management.
- **Status:** Complete. Only admin users can access user management features, with robust error handling and user feedback. Classic logic reused. (Always update this file before proceeding with any next step.)

### [HIGH] 9. Settings Panel: INI File Integration ✅
- Implement reading/writing of app_settings.ini for configuration.
- **Dependencies:** None.
- **Status:** Complete. Settings panel now reads and writes app_settings.ini, with robust error handling and user feedback.

### [HIGH] 10. Settings Panel: Executable Path Selectors ✅
- Add file pickers for Nuke, After Effects, and project directories.
- **Dependencies:** INI file integration.
- **Status:** Complete. File pickers for Nuke/After Effects executables and project directories are implemented in the settings panel, with robust error handling and modern UI.

### [MEDIUM] 11. Settings Panel: Path Validation ✅
- Validate paths for executables and project directories.
- UI: Show warnings or errors if invalid paths are entered.
- **Dependencies:** Executable path selectors.
- **Status:** Complete. Path validation for executables is robust; project directories now accept folder names (not paths), matching classic UI logic. Errors are shown for empty input only, as per legacy behavior.

### [MEDIUM] 12. Settings Panel: Tabbed Layout ✅
- [x] Implement tabbed layout for settings sections using QTabWidget (Applications, Projects, Advanced tabs).
- [x] Add modern styling (QSS): rounded corners, flat backgrounds, subtle shadows, highlight active tab.
- [x] Add icons to tabs and file picker buttons.
- [x] Ensure all controls have tooltips and are keyboard accessible.
- [x] Add inline error labels under fields for validation.
- [x] Add high-contrast toggle in Advanced tab.
- **Dependencies:** Basic settings panel functional.
- **Status:** Complete. All major features and subtasks implemented. Only minor visual/accessibility polish may remain.
- **ALWAYS:** Before implementing or connecting button actions, check the classic UI to ensure all functions match legacy behavior and logic.

#### Subtasks:
- [x] Refactor to QTabWidget with three tabs: Applications, Projects, Advanced
- [x] Move each logical group into its tab
- [x] Apply QSS for modern look and accessibility
- [x] Add icons and tooltips
- [x] Add inline error labels for validation
- [x] Add keyboard shortcuts for tabs
- [x] Add high-contrast toggle in Advanced
- [x] Confirm all button actions match classic UI logic

> Progress: Settings panel tabbed layout is complete. Only minor visual/accessibility polish may be added as needed.

### [LOW] 13. Project Browser: Favorite/Unfavorite Functionality
    - [x] Create per-user FavoritesManager (JSON config, no DB) for loading, saving, and toggling favorites. ✅
    - [x] Integrate FavoritesManager into Project Browser UI (star icon column, toggle logic, filter control). ✅
    - [x] Add a favorite toggle (star icon) to each project in the Project Browser. ✅
    - [x] Implement filtering to show only favorite projects. ✅
    - **Favorites are stored per-user in a config file (JSON) in the user's config directory, not in the DB.**
    - On login or ProjectBrowser init, load the user's favorites file.
    - On favorite/unfavorite, update the file immediately.
    - Add a star icon column to the project tree (filled if favorite, outline if not).
    - Clicking the star toggles favorite status (with instant UI feedback).
    - Add a “Show Favorites Only” filter control above the project list.
    - Ensure all logic matches classic UI where relevant, but with config-file persistence.
    - Accessibility: Tooltip on the star, keyboard accessible, visual feedback.
    - **No DB dependency for favorites—fully config-based.**

> Progress: Favorites UI (star, config, filter) is fully implemented and working as designed.


### [LOW] 14. Project Browser: Open Containing Folder Action
- [x] Add context menu action to open the folder containing a project/file. 
- [x] Use os.startfile (Windows), xdg-open (Linux), open (macOS). (Windows only, as per codebase)
- [x] Show error message if folder does not exist or cannot be opened. 

> Progress: Feature implemented and tested on Windows. Shows clear error if folder is missing or cannot be opened.


### [MED] 15. Project Browser: Multi-Select and Bulk Actions
- Allow users to select multiple projects in the project browser (Ctrl/Cmd or Shift+Click).
- Enable context menu actions (Remove, Archive, etc.) to operate on all selected projects.
- Show confirmation dialogs for bulk actions.
- Ensure UI feedback and error handling for partial failures.
- Keyboard accessibility for multi-select.

> Next: Implement multi-select support and update context menu logic.
- Add support for user-uploaded avatars (not just initials).
- **Dependencies:** User list must be loaded/displayed.

### [HIGH] 16. Database Integration: Fix incorrect table references [COMPLETED]
- [x] Fixed references to the non-existent "files" table, changing them to "project_files" to match the database schema.
- [x] Updated SQL queries to properly extract folder paths from file paths.
- [x] Fixed issues that prevented files from being written to and read from the database.

> Progress: Database integration is now fully functional. Files are correctly written to and read from the database.

### [MEDIUM] 17. Project Browser: UI Improvements [COMPLETED]
- [x] Made columns in the project browser resizable for better user experience.
- [x] Fixed the search functionality in project browser.
- [x] Ensured the "Show Favorites Only" button works correctly.
- [x] Allowed user to hide/show columns as needed.
- [x] Set better default column widths for improved readability.

> Progress: Project browser is now more user-friendly with resizable columns and proper filtering.

### [MEDIUM] 18. File Browser: Fix Double Spacing Issue [COMPLETED]
- [x] Removed the code that was adding spaces between each character in filenames.
- [x] Fixed display issues in the file browser tree items.
- [x] Ensured text is properly rendered in consistent font spacing.

> Progress: File browser now displays filenames correctly without artificial character spacing.

### [HIGH] 19. Project Scan: Robust Progress/Log/ETA Reporting ✅
- [x] Emit progress and log signals frequently during scanning (start/end of each folder, every N files, errors, ETA updates).
- [x] UI: Update progress bar, ETA, and log for every signal.
- [x] Show current directory being scanned, number of files found, and total progress.
- [x] Ensure errors and warnings appear in log and UI.
- [x] Never break or regress the existing logic: first scan for target folders (from settings), then scan only those folders for files. Rescans must only rescan found folders, never the full project.

> Progress: Complete! Enhanced progress reporting with:
> - More frequent progress updates (every 50 dirs, every 5 files, or every 1-2 seconds)
> - Better ETA calculations based on actual scan rate
> - Current directory display in progress dialog
> - Visual indicators (✓, ❌, 📁, 📄, etc.) in log messages
> - Scan rate statistics (files/sec, dirs/sec)
> - Improved error and warning visibility

### [LOW] 16. General: Accessibility Improvements
- Keyboard shortcuts, tooltips, high-contrast mode polish, etc.
- **Dependencies:** All panels implemented.

---

## Task Completion Protocol
- After completing each task, remove it from this file.
- Always read this file before starting the next implementation.
- Tasks must be completed in order (top-to-bottom, by priority and dependency).
