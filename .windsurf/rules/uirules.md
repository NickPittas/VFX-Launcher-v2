---
trigger: always_on
---

1. Modern Visual Design
Adopt a clean, modern theme: Use a flat design with subtle shadows, rounded corners, and a consistent color palette (e.g., dark/light mode toggle).
Professional iconography: Integrate SVG icons for actions, projects, and statuses.
Consistent typography: Use a readable, modern font (e.g., Inter, Roboto).
2. Enhanced Navigation & Layout
Sidebar Navigation: Replace top-level menus with a collapsible sidebar for quick access to Projects, User Management, Settings, and Logs.
Breadcrumbs: Show the current navigation path (e.g., Home > Projects > ProjectX > Versions).
Tabbed Panels: Use tabs within main windows (e.g., Project Browser, Logs, Settings) for quick switching and less clutter.
3. Responsive & Asynchronous Interactions
Loading Indicators: Show spinners or skeleton screens during async operations (project scanning, launching apps, DB queries).
Non-blocking UI: Ensure all long-running tasks run in background threads (QThreadPool/QRunnable) and update UI on completion.
Toasts/Notifications: Display non-intrusive success/error messages at the bottom or corner of the screen.
4. Improved Project Browser
Search & Filter: Add a search bar and filters (by status, date, user) to quickly find projects.
Project Cards/List: Display projects as cards or in a sortable table with key info (name, status, last modified, actions).
Quick Actions: Hover or context menu for "Open", "Duplicate", "Archive", etc.
5. User Management & Settings
Profile Avatars: Show user photos or initials.
Role Badges: Visually indicate user roles (Admin, Artist, TD).
Inline Editing: Allow quick edits to user info or settings without opening new dialogs.
6. Logging & Feedback
Live Log Panel: Color-code log levels (info, warning, error), allow filtering/searching logs.
Export Logs: Button to export logs for debugging.
7. Accessibility & Usability
Keyboard Shortcuts: For common actions (e.g., Ctrl+N for new project).
Tooltips: Explain icons and actions on hover.
High-contrast mode: For visually impaired users.
8. Customization
User Preferences: Allow users to save layout, theme, and other preferences.
Drag-and-drop: Reorder projects, tabs, or panels.
9. Onboarding & Help
Welcome Tour: Guide new users through key features on first launch.
Contextual Help: Tooltips or help buttons for complex features.
10. Example: Main Window Layout

+---------------------------------------------------------------+
| [Sidebar]  |   [Main Area: Tabbed Panels]                     |
|            |  ---------------------------------------------   |
| Projects   |  | Project Browser | Logs | Settings | User |    |
| ---------- |  ---------------------------------------------   |
| User Mgmt  |  |  [Project Cards/List with Actions]         |  |
| Settings   |  ---------------------------------------------   |
| Logs       |                                                 |
+---------------------------------------------------------------+

Implementation Steps
Redesign ui/main_window.py to use a sidebar and tabbed main area.
Refactor each UI module (login.py, user_management.py, etc.) for consistent styling and asynchronous updates.
Add reusable components: loading spinner, toast notifications, search/filter bars.
Apply a modern stylesheet (QSS) and allow theme switching.
Test for responsiveness and accessibility.
