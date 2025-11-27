Here’s a **single, comprehensive, detailed prompt** you can directly feed into **Windsurf AI** to generate the entire foundational structure of your VFX Project Launcher application at once.
*(You can paste this directly or slightly adjust based on the actual prompt length limit.)*

---

## 🚀 Comprehensive Prompt for Windsurf AI

### **Using the provided files and adhering strictly to their instructions:**

* `architecture.md` (module and directory structure)
* `bestpractices.md` (coding style and practices)
* `rules.md` (specific instructions for coding and consistency)
* `SQLite_schema.md` (database structure)
* `app_settings.ini` (default configuration)
* `README.md` (documentation)
* `tasklist.md` (detailed breakdown of each task)

---

### **Generate the following Python modules with clearly defined purposes and detailed implementations as per provided documentation:**

### **1. Main Application Entry Point**

**File:** `main.py`

**Purpose:**

* Initializes and launches the PySide6-based GUI.
* Connects and integrates all UI modules and core logic.

**Requirements:**

* Loads application settings from `app_settings.ini`.
* Initializes logging.
* Sets up main PySide application window and login workflow.
* Launches GUI components: main window, logging, settings, user management, project browser.

---

### **2. Database Management Module**

**File:** `core/database.py`

**Purpose:**

* Handles SQLite database initialization, connection pooling, and schema management.
* Provides reusable CRUD functions.

**Requirements:**

* Automatically create the database and tables if not present (use schema from `SQLite_schema.md`).
* Robust error handling and logging.

---

### **3. Project Scanning Module**

**File:** `core/project_scanner.py`

**Purpose:**

* Scans configured directories (from settings) recursively for `.nk` and `.aep` files.
* Parses filenames to identify project names and version numbers.

**Requirements:**

* Asynchronous operation (no UI blocking).
* Accurate regex parsing for version detection (`_v###`).
* Updates database with file paths and versions.

---

### **4. File Watching Module**

**File:** `core/file_watcher.py`

**Purpose:**

* Uses Watchdog to monitor configured directories for new or changed files.

**Requirements:**

* Efficient background thread (check interval \~1 min).
* Real-time database updating and GUI refreshing.

---

### **5. Application Launching Module**

**File:** `core/app_launcher.py`

**Purpose:**

* Opens selected `.nk` (with `--nukex`) or `.aep` files in respective software applications (Nuke, After Effects).

**Requirements:**

* Subprocess calls.
* Path verification and logging.

---

### **6. Utilities Module**

**File:** `core/utils.py`

**Purpose:**

* Contains general-purpose helper functions, regex parsers, timestamp handling, path manipulation, and common logic.

**Requirements:**

* Reusable, modular functions.
* Thoroughly commented.

---

### **7. User Actions Module**

**File:** `core/user_actions.py`

**Purpose:**

* Logs user activities such as opening files, modifying projects, etc.

**Requirements:**

* Simple activity logging tied to username and timestamp.
* No authentication, only logging user actions.

---

### **8. User Interface: Main Window**

**File:** `ui/main_window.py`

**Purpose:**

* Main GUI window post-login with project list, version selection, and file launching.

**Requirements:**

* Tree-view file structure, sortable columns (name, version, last modified).
* Right-click context menus (open folder, show details).
* Favorites and recent projects display.
* Responsive UI (multithreading for updates).

---

### **9. User Interface: Login Screen**

**File:** `ui/login.py`

**Purpose:**

* User selection for logging purposes.

**Requirements:**

* Simple username selection dropdown, connected to users table.

---

### **10. User Interface: User Management Panel**

**File:** `ui/user_management.py`

**Purpose:**

* Allows admin users to manage users (create, edit, delete).

**Requirements:**

* CRUD operations on users.
* Simple admin flag management.

---

### **11. User Interface: Settings Panel**

**File:** `ui/settings_panel.py`

**Purpose:**

* GUI for setting paths to Nuke/After Effects executables and project directories.

**Requirements:**

* INI file read/write (`app_settings.ini`).
* Path validations.

---

### **12. User Interface: Project Browser**

**File:** `ui/project_browser.py`

**Purpose:**

* Advanced project selection tree-view UI with version selection dropdown.

**Requirements:**

* Clear folder-based structure.
* Interactive file version dropdown selection.

---

### **13. User Interface: Logging Panel**

**File:** `ui/log_panel.py`

**Purpose:**

* Real-time log viewer embedded in main UI.

**Requirements:**

* Tail and display logs generated from the Python logging module.
* Clear, readable UI component.

---

### **14. Requirements File**

**File:** `requirements.txt`

**Contents:**

```plaintext
PySide6
watchdog
sqlite3
```

---

### **Implementation Rules for Windsurf AI (Important!):**

* Follow modularity explicitly (one responsibility per file).
* Ensure all UI elements use PySide6.
* Use multithreading (PySide `QThreadPool`) for async tasks.
* Fully implement SQLite schema from `SQLite_schema.md`.
* Adhere strictly to provided best practices (`bestpractices.md`) and rules (`rules.md`).
* Include clear, robust error handling.
* Comprehensive logging as per provided rules and best practices.

---

### **Ensure File Consistency and Readability**

* Comment generously and clearly.
* All modules must clearly show their purpose at the top.
* Include explanatory comments referencing architecture and requirements directly.

---

### ⚠️ **Final Checklist before generating files:**

* [ ] Read all provided `.md` files and adhere strictly to instructions.
* [ ] Confirm SQLite tables match exactly `SQLite_schema.md`.
* [ ] Strict adherence to modular architecture (`architecture.md`).
* [ ] Best practices and guidelines (`bestpractices.md` and `rules.md`) applied.
* [ ] Default configuration from provided `app_settings.ini`.

---

**Now generate the entire VFX Project Launcher codebase following this comprehensive prompt.**
