---
trigger: always_on
---

VFXProjectLauncher/
│
├── main.py                     # Application entry-point
├── requirements.txt            # Python dependencies
│
├── ui/                         # UI components
│   ├── login.py
│   ├── main_window.py
│   ├── user_management.py
│   ├── settings_panel.py
│   ├── project_browser.py
│   └── log_panel.py
│
├── core/                       # Core functionalities (modular)
│   ├── database.py
│   ├── project_scanner.py
│   ├── file_watcher.py
│   ├── user_actions.py
│   ├── app_launcher.py
│   └── utils.py
│
├── config/                     # Settings and configuration files
│   └── app_settings.ini
│
└── logs/                       # Real-time logging
    └── app_log.log