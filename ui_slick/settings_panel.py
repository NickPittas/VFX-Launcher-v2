# Modern Settings Panel for Slick UI
from PySide6.QtWidgets import (
    QWidget, QVBoxLayout, QHBoxLayout, QLabel, QComboBox, QPushButton, QCheckBox, QLineEdit, QFormLayout, QMessageBox
)
from PySide6.QtCore import Qt
import configparser
import os
from core.config import resolve_app_settings_path

class SettingsPanel(QWidget):
    """
    Modern Settings Panel for Slick UI.
    - Theme switcher (dark/light)
    - Scalable controls
    - User preferences (placeholder logic)
    """
    def __init__(self, db_manager, user_data, parent=None):
        super().__init__(parent)
        self.setObjectName("SettingsPanel")
        self.db_manager = db_manager
        self.db_path = db_manager.db_path
        self.user_data = user_data
        self.config_path = resolve_app_settings_path()
        self.config = configparser.ConfigParser()
        self._setup_ui()
        self._load_settings()

    def _setup_ui(self):
        from PySide6.QtWidgets import QTabWidget, QWidget, QStyle
        layout = QVBoxLayout(self)
        layout.setContentsMargins(0, 0, 0, 0)
        layout.setSpacing(16)
        title = QLabel("Settings")
        title.setObjectName("SettingsPanelTitle")
        title.setStyleSheet("font-size: 1.5em; font-weight: bold; margin-bottom: 16px;")
        layout.addWidget(title)

        tab_widget = QTabWidget()
        tab_widget.setObjectName("SettingsTabWidget")

        # --- Applications Tab ---
        apps_tab = QWidget()
        apps_layout = QVBoxLayout(apps_tab)
        form_apps = QFormLayout()
        self.nuke_path_input = QLineEdit()
        nuke_browse_btn = QPushButton("Browse")
        nuke_browse_btn.clicked.connect(self._on_browse_nuke)
        nuke_row = QHBoxLayout()
        nuke_row.addWidget(self.nuke_path_input)
        nuke_row.addWidget(nuke_browse_btn)
        form_apps.addRow("Nuke Path:", nuke_row)

        self.ae_path_input = QLineEdit()
        ae_browse_btn = QPushButton("Browse")
        ae_browse_btn.clicked.connect(self._on_browse_ae)
        ae_row = QHBoxLayout()
        ae_row.addWidget(self.ae_path_input)
        ae_row.addWidget(ae_browse_btn)
        form_apps.addRow("After Effects Path:", ae_row)

        self.nuke_linux_cmd_input = QLineEdit()
        self.nuke_linux_cmd_input.setPlaceholderText(
            "ghostty -e env QT_QPA_PLATFORM=xcb LC_NUMERIC=C /opt/Nuke17.0v1/Nuke17.0 --nukex"
        )
        self.nuke_linux_cmd_input.setToolTip("Full Linux command line; the .nk file path is appended at the end.")
        form_apps.addRow("Nuke Launch Cmd (Linux):", self.nuke_linux_cmd_input)
        apps_layout.addLayout(form_apps)
        apps_layout.addStretch(1)

        # --- Projects Tab ---
        projects_tab = QWidget()
        projects_layout = QVBoxLayout(projects_tab)
        form_projects = QFormLayout()
        self.project_dirs_input = QLineEdit()
        proj_browse_btn = QPushButton("Browse")
        proj_browse_btn.clicked.connect(self._on_browse_project_dirs)
        proj_row = QHBoxLayout()
        proj_row.addWidget(self.project_dirs_input)
        proj_row.addWidget(proj_browse_btn)
        form_projects.addRow("Project Dirs (names):", proj_row)
        help_label = QLabel(
            "Enter a comma- or semicolon-separated list of folder names to search for recursively.\n"
            "Example: project, projects, Project, Projects\n"
            "These are subfolder names, not paths."
        )
        form_projects.addRow("", help_label)
        self.scan_interval_input = QLineEdit()
        form_projects.addRow("Scan Interval (min):", self.scan_interval_input)
        projects_layout.addLayout(form_projects)
        projects_layout.addStretch(1)

        # --- Advanced Tab ---
        advanced_tab = QWidget()
        advanced_layout = QVBoxLayout(advanced_tab)
        form_adv = QFormLayout()
        self.log_level_input = QComboBox()
        self.log_level_input.addItems(["DEBUG", "INFO", "WARNING", "ERROR", "CRITICAL"])
        form_adv.addRow("Log Level:", self.log_level_input)
        self.font_size_input = QLineEdit("16")
        form_adv.addRow("Font Size (px):", self.font_size_input)
        self.high_contrast_chk = QCheckBox("High Contrast Mode")
        form_adv.addRow("", self.high_contrast_chk)
        advanced_layout.addLayout(form_adv)
        advanced_layout.addStretch(1)

        # --- Add Tabs with Icons ---
        style = self.style()
        tab_widget.addTab(apps_tab, style.standardIcon(QStyle.SP_ComputerIcon), "Applications")
        tab_widget.addTab(projects_tab, style.standardIcon(QStyle.SP_DirIcon), "Projects")
        tab_widget.addTab(advanced_tab, style.standardIcon(QStyle.SP_FileDialogDetailedView), "Advanced")
        layout.addWidget(tab_widget)

        # --- Save Button ---
        save_btn = QPushButton("Save Preferences")
        save_btn.clicked.connect(self._on_save)
        layout.addWidget(save_btn)
        layout.addStretch(1)

    def _on_theme_changed(self, theme):
        # Placeholder: In real app, reload QSS and persist preference
        if theme == "Light":
            QMessageBox.information(self, "Theme", "Light theme not implemented yet. Using dark mode.")
        else:
            pass  # Already dark mode by default

    def _load_settings(self):
        try:
            if not os.path.exists(self.config_path):
                return
            self.config.read(self.config_path, encoding="utf-8")
            # Paths
            self.nuke_path_input.setText(self.config.get("Paths", "nuke_path", fallback=""))
            self.ae_path_input.setText(self.config.get("Paths", "after_effects_path", fallback=""))
            self.nuke_linux_cmd_input.setText(self.config.get("Paths", "nuke_launch_cmd_linux", fallback=""))
            # Projects
            self.project_dirs_input.setText(self.config.get("Projects", "project_directories", fallback=""))
            self.scan_interval_input.setText(self.config.get("Projects", "scan_interval_minutes", fallback=""))
            # Logging
            log_level = self.config.get("Logging", "log_level", fallback="INFO")
            idx = self.log_level_input.findText(log_level)
            if idx >= 0:
                self.log_level_input.setCurrentIndex(idx)
            # Font size, high contrast (placeholder)
            # (Could add more INI fields as needed)
        except Exception as e:
            QMessageBox.critical(self, "Settings Error", f"Error loading settings: {e}")

    def _on_save(self):
        import os
        errors = []
        # Validate Nuke path
        nuke_path = self.nuke_path_input.text().strip()
        if nuke_path and not (os.path.isfile(nuke_path) and nuke_path.lower().endswith('.exe')):
            errors.append("Nuke path is not a valid executable.")
            self.nuke_path_input.setStyleSheet("border: 2px solid red;")
        else:
            self.nuke_path_input.setStyleSheet("")
        # Validate After Effects path
        ae_path = self.ae_path_input.text().strip()
        if ae_path and not (os.path.isfile(ae_path) and ae_path.lower().endswith('.exe')):
            errors.append("After Effects path is not a valid executable.")
            self.ae_path_input.setStyleSheet("border: 2px solid red;")
        else:
            self.ae_path_input.setStyleSheet("")
        # Validate project directories
        proj_dirs_raw = self.project_dirs_input.text().replace(';', ',')
        proj_dirs = [d.strip() for d in proj_dirs_raw.split(',') if d.strip()]
        if not proj_dirs:
            self.project_dirs_input.setStyleSheet("border: 2px solid red;")
            QMessageBox.warning(
                self,
                "Invalid Project Folder Names",
                "Please enter at least one folder name to search for recursively (e.g. project, projects)."
            )
            return
        else:
            self.project_dirs_input.setStyleSheet("")
        try:
            # Update config object
            if not self.config.has_section("Paths"):
                self.config.add_section("Paths")
            if not self.config.has_section("Projects"):
                self.config.add_section("Projects")
            if not self.config.has_section("Logging"):
                self.config.add_section("Logging")
            self.config.set("Paths", "nuke_path", nuke_path)
            self.config.set("Paths", "after_effects_path", ae_path)
            self.config.set("Paths", "nuke_launch_cmd_linux", self.nuke_linux_cmd_input.text().strip())
            # Always save as comma-separated for compatibility
            self.config.set("Projects", "project_directories", ', '.join(proj_dirs))
            self.config.set("Projects", "scan_interval_minutes", self.scan_interval_input.text())
            self.config.set("Logging", "log_level", self.log_level_input.currentText())
            # Save to file
            with open(self.config_path, "w", encoding="utf-8") as f:
                self.config.write(f)
            font_size = self.font_size_input.text()
            high_contrast = self.high_contrast_chk.isChecked()
            QMessageBox.information(self, "Settings Saved", f"Settings updated.\nFont size: {font_size}\nHigh Contrast: {high_contrast}")
        except Exception as e:
            QMessageBox.critical(self, "Settings Error", f"Error saving settings: {e}")

    def _on_browse_nuke(self):
        from .file_dialogs import pick_open_file
        path = pick_open_file(self, "Select Nuke Executable", "", "Executables (*.exe);;All Files (*)")
        if path:
            self.nuke_path_input.setText(path)

    def _on_browse_ae(self):
        from .file_dialogs import pick_open_file
        path = pick_open_file(self, "Select After Effects Executable", "", "Executables (*.exe);;All Files (*)")
        if path:
            self.ae_path_input.setText(path)

    def _on_browse_project_dirs(self):
        from .file_dialogs import pick_directory
        dir_path = pick_directory(self, "Select Project Directory")
        if dir_path:
            # Append or set, comma-separated
            text = self.project_dirs_input.text()
            if text:
                self.project_dirs_input.setText(text + "," + dir_path)
            else:
                self.project_dirs_input.setText(dir_path)
