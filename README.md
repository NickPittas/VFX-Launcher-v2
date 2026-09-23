# VFX Project Launcher

A modern, slick Windows application for managing and launching VFX projects (Nuke and After Effects files) from a centralized SQLite database.

![VFX Launcher](ui_slick/icons/icon_3_512.png)

## 🎯 Purpose

A Python-based Windows application that helps VFX artists manage and quickly launch Nuke (.nk) and After Effects (.aep) projects directly from a centralized project database, featuring user logging, project scanning, version selection, and real-time monitoring.

## ✨ Features

### Core Functionalities
- 📁 **Project Management**: Select project from database, scan and track .nk and .aep files
- 🔍 **Smart Scanning**: Automatically scan directories for VFX project files with real-time progress
- 📊 **Version Management**: Auto-detect and allow manual selection of script/project versions
- ⭐ **Favorites**: Mark frequently used files as favorites for quick access
- 📈 **File Tracking**: Track when files were last opened and by whom
- 👥 **User Management**: Log actions (no passwords required), add/remove/edit users (admin function)
- 🚀 **Integration**: Automatically launch selected project with Nuke (using --nukex) or After Effects
- 📝 **Real-Time Logging**: Record actions and scan progress for troubleshooting and accountability
- 👁️ **Folder Monitoring**: Scan configured directories to detect and update new script versions
- 🎨 **Modern UI**: Clean, dark-themed interface with collapsible sidebar
- 💾 **Persistent State**: Remembers window size, column widths, sidebar state, and UI preferences
- 🌳 **File System Interaction**: Tree-view browsing, context menus, and detailed project info

## 📋 Requirements

### For End Users
- Windows 10/11
- Nuke and/or After Effects (for launching files)

### For Development
- Python 3.11+
- PySide6 for GUI
- SQLite database (lightweight, embedded, perfect for network-shared environment via SMB/QNAP)
- Watchdog library for file monitoring

## 🚀 Installation

### Option 1: Use Pre-built Executable (Recommended)

**Windows:** Download `VFX_Launcher.exe` from the
[latest release](https://github.com/NickPittas/VFX-Launcher-v2/releases) and run it.
Configure the database location on first run, then set paths to Nuke and
After Effects in Settings.

**Linux:** Download `VFX_Launcher-x86_64.AppImage` from the
[latest release](https://github.com/NickPittas/VFX-Launcher-v2/releases),
make it executable (`chmod +x`), and run it.

### Option 2: Use Installer
1. Download `VFX_Launcher_Setup.exe` from releases
2. Run the installer
3. Follow the installation wizard
4. Select database location (local or network share)
5. Launch from Start Menu or Desktop shortcut

### Option 3: Run from Source
```bash
# Clone the repository
git clone <repository-url>
cd VFX-Launcher

# Install dependencies
pip install -r requirements.txt

# Run the application
python ui_slick/main_slick.py
```

## 🔨 Building from Source

### Build Executable
```bash
# Run the build script
build_exe.bat

# Or manually with PyInstaller
python -m PyInstaller vfx_launcher.spec --clean
```

The executable will be created at `dist/VFX_Launcher.exe` (~47 MB single file).

### Build Installer
1. Install [Inno Setup](https://jrsoftware.org/isdl.php)
2. Open `installer.iss` in Inno Setup Compiler
3. Click "Compile" to create the installer
4. Installer will be created in `installer_output/VFX_Launcher_Setup.exe`

### Build Linux AppImage
```bash
./build_appimage.sh
```
The AppImage will be created at `dist/VFX_Launcher-x86_64.AppImage`. Requires
Python 3 with `pyinstaller` and `requirements.txt` installed; `appimagetool`
is downloaded automatically if missing.

> Note: the `ui_slick/Structure/` preset directory contains only empty folders
> and is not tracked by git. Builds without it use the committed
> `ui_slick/structure.txt` manifest instead (the app recreates the folders from
> it at runtime), so the AppImage is fully functional either way.

## ⚙️ Configuration

### Database Location
On first run, you'll be prompted to select a database location. You can choose:
- **Local**: `C:\Users\<username>\AppData\Local\VFX_Launcher\vfx_launcher.db`
- **Network**: `\\server\shared\vfx_launcher.db` (for multi-user setups)

The location is saved in `config.ini`.

### Application Paths
Configure paths to Nuke and After Effects executables in the Settings panel:
- Nuke: `C:\Program Files\Nuke14.0v5\Nuke14.0.exe`
- After Effects: `C:\Program Files\Adobe\Adobe After Effects 2024\Support Files\AfterFX.exe`

## 📖 Usage

1. **Add Projects**: Click "Add Project" to create a new project entry
2. **Scan Directories**: Use "Scan Directory" to automatically find VFX files
3. **Browse Files**: Navigate through projects and their file versions
4. **Launch Files**: Double-click a file to open it in the appropriate application
5. **Mark Favorites**: Click the star icon to mark files as favorites
6. **Collapse Sidebar**: Click the ☰ button to collapse/expand the navigation sidebar
7. **View Logs**: Access application logs from the Logs panel

## 📁 Project Structure

```
VFX-Launcher/
├── ui_slick/              # Main application code (Slick UI)
│   ├── main_slick.py      # Application entry point
│   ├── main_window.py     # Main window UI
│   ├── sidebar.py         # Collapsible sidebar navigation
│   ├── project_panel.py   # Project management panel
│   ├── user_panel.py      # User management panel
│   ├── settings_panel.py  # Settings configuration panel
│   ├── log_panel.py       # Log viewer panel
│   ├── icons/             # Application icons (PNG, ICO)
│   └── styles.qss         # Qt stylesheet (dark theme)
├── database_manager.py    # SQLite database operations
├── config_manager.py      # Configuration management (config.ini)
├── vfx_launcher.spec      # PyInstaller specification (single-file build)
├── installer.iss          # Inno Setup installer script
├── build_exe.bat          # Build script for executable
├── requirements.txt       # Python dependencies
├── .gitignore             # Git ignore rules
└── README.md              # This file
```

## 🗄️ Database Schema

The application uses SQLite with the following main tables:
- **projects**: Project information (name, description, root path)
- **files**: File entries with versions (path, project_id, version)
- **file_access_log**: Track file access history (file_id, user_id, timestamp)
- **users**: User profiles (name, email)

## 📝 Logs

Application logs are stored in:
```
C:\Users\<username>\AppData\Local\VFX_Launcher\logs\
```

Logs include:
- Application startup/shutdown
- File scanning progress
- File launches
- Database operations
- Errors and warnings

## 🎨 UI Features

### Collapsible Sidebar
- Click ☰ to toggle between expanded (200px) and collapsed (60px) states
- Collapsed state shows first letter of each section (P, U, S, L)
- State persists across sessions

### File Browser
- Star column for favorites
- Last Opened and Opened By columns
- Sortable columns
- Column widths persist across sessions

### Dark Theme
- Modern dark color scheme
- Consistent styling across all panels
- Custom QSS stylesheet

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

## 📄 License

[Add your license here]

## 👤 Author

[Add your name/organization here]

## 🙏 Acknowledgments

- Built with PySide6 (Qt for Python)
- Icons created with custom designs
- Inspired by modern VFX pipeline tools