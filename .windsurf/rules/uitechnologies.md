---
trigger: always_on
---

1. Technology Exploration
A. Native PySide6 (Current)
Uses Qt’s QSS (Qt Style Sheets) for styling.
All UI is Python-based.
B. Hybrid/Web-based UI (Optional Future Path)
Use a webview (e.g., QtWebEngine) to embed a web UI.
Build UI in HTML/CSS/JS, using frameworks like React + Tailwind CSS for rapid, modern design.
Communicate with Python backend via local API or IPC.
C. Tailwind CSS & Modern Web UI
Tailwind CSS enables rapid, utility-first styling.
Can be used if you migrate parts of the UI to web technologies (e.g., Electron, or QtWebEngine).
2. Recommended Approach
Phase 1:

Modernize and refactor the PySide6 UI using QSS for theming and modular widgets.
Prepare codebase for possible hybrid/web UI in the future.
Phase 2 (Optional):

Prototype a web-based UI using React + Tailwind CSS, embedded in QtWebEngine if desired.
3. Files to Change/Touch
| File/Folder                      | Purpose                                        | Technology  | 
|----------------------------------|--------------------------------------------------------------|--------------------| 
| ui/main_window.py                | Main app window, layout overhaul, sidebar/tabs | PySide6/QSS | 
| ui/login.py                      | Login screen, modernize                        | PySide6/QSS | 
| ui/user_management.py            | User management, avatars, inline edit          | PySide6/QSS | 
| ui/settings_panel.py             | Settings, theme switcher                       | PySide6/QSS | 
| ui/project_browser.py            | Project cards/list, search/filter              | PySide6/QSS | 
| ui/log_panel.py                  | Log panel, color-coded logs                    | PySide6/QSS | 
| ui/styles.qss (new)              | Centralized stylesheet for consistent theming  | QSS         | 
| ui/web/ (new, optional)          | Web UI prototype: React, Tailwind, etc.        | HTML/CSS/JS | 
| main.py                          | (If embedding web UI) QtWebEngine integration  | PySide6/Web |

4. Task List
Phase 1: PySide6 Modernization
Centralize Styling
Create ui/styles.qss for global styles.
Refactor all UI files to use QSS classes.
UI Component Refactor
Update main_window.py to use a sidebar and tabbed layout.
Refactor each panel for modularity and modern design (cards, tables, search bars).
Add Modern UI Features
Implement loading spinners, toast notifications, and tooltips.
Add a theme switcher (dark/light).
Accessibility & Responsiveness
Ensure keyboard navigation and high-contrast support.
Non-Disruptive Changes
All changes should be incremental, behind feature flags or in new branches.
Maintain all existing functionality.
Phase 2: Web UI Prototype (Optional)
Set Up Web UI Directory
Create ui/web/ for React + Tailwind CSS app.
Build Core Components
Project browser, login, settings, log panel, etc.
Integrate with Python Backend
Use QtWebEngine or Electron, or run as standalone in browser.
Communicate via local API (Flask/FastAPI) or IPC.
Evaluate and Plan Migration
Compare user experience and decide on further migration.
5. Guide for Each Technology
PySide6 + QSS (Recommended for Now)
Use QSS for styling: similar to CSS, but for Qt widgets.
Modernize layouts: QHBoxLayout, QVBoxLayout, QStackedWidget for tabs.
Use Qt Designer for rapid prototyping if desired.
React + Tailwind (Optional, for Prototyping)
Use Create React App or Vite to scaffold.
Install Tailwind CSS for utility-first styling.
Build modular, reusable components.
Use Electron or QtWebEngine for desktop embedding if needed.
6. Example: Adding QSS Styling
Create ui/styles.qss:
css
CopyInsert
QWidget {
    font-family: 'Inter', Arial, sans-serif;
    background: #181a1b;
    color: #f0f0f0;
}
QPushButton {
    border-radius: 6px;
    padding: 8px 16px;
    background: #2563eb;
    color: #fff;
}
QPushButton:hover {
    background: #1d4ed8;
}
/* Add more as needed */
In each Python UI file:
python
CopyInsert
with open('ui/styles.qss', 'r') as f:
    self.setStyleSheet(f.read())
7. Summary Table
| Technology        | Where Used        | Migration Impact | Recommended? | 
|-------------------|-------------------|------------------|--------------| 
| PySide6 + QSS     | All current UI    | None             | Yes (now)    | 
| React + Tailwind  | ui/web/ prototype | Optional         | Maybe (future)| 
| QtWebEngine       | main.py           | Optional         | Maybe (future)|

Next Steps
Start with QSS-based modernization for a quick win and zero risk.
Prototype web UI only in a new directory, never replacing existing code until proven.