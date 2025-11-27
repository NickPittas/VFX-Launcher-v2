# VFX Launcher - Build and Installation Instructions

## Prerequisites

### For Building the Executable

1. **Python 3.8 or higher**
   - Download from: https://www.python.org/downloads/

2. **Required Python packages**
   ```bash
   pip install -r requirements.txt
   pip install pyinstaller
   ```

3. **For creating the installer (optional)**
   - Download and install Inno Setup: https://jrsoftware.org/isdl.php

---

## Building the Executable

### Method 1: Using the Build Script (Recommended)

1. Open Command Prompt in the project directory
2. Run the build script:
   ```bash
   build_exe.bat
   ```
3. The executable will be created in `dist\VFX_Launcher\VFX_Launcher.exe`

### Method 2: Manual Build

1. Open Command Prompt in the project directory
2. Run PyInstaller:
   ```bash
   pyinstaller vfx_launcher.spec
   ```
3. The executable will be created in `dist\VFX_Launcher\VFX_Launcher.exe`

---

## Creating the Windows Installer

### Prerequisites
- Inno Setup must be installed
- The executable must be built first (see above)

### Steps

1. Open `installer.iss` in Inno Setup Compiler
2. Click **Build** > **Compile**
3. The installer will be created in `installer_output\VFX_Launcher_Setup.exe`

### What the Installer Does

- Installs the application to `C:\Program Files\VFX Launcher\`
- Creates desktop and start menu shortcuts
- Prompts for database location (default: `C:\ProgramData\VFX_Launcher\`)
- Creates a `config.ini` file with the database path
- Sets up proper permissions for the database directory

---

## Running the Application

### Development Mode
```bash
python ui_slick/main_slick.py
```

### Standalone Executable
- Run `dist\VFX_Launcher\VFX_Launcher.exe`
- Or install using the installer and run from Start Menu/Desktop

---

## Configuration

The application uses a `config.ini` file to store settings:

```ini
[Database]
Path=C:\ProgramData\VFX_Launcher\vfx_launcher.db

[Paths]
DataDir=C:\ProgramData\VFX_Launcher
```

### Config File Locations (searched in order)

1. **Installed version**: `C:\Program Files\VFX Launcher\config.ini`
2. **Development version**: `<project_root>\config.ini`
3. **Default**: Creates in `%APPDATA%\VFX_Launcher\`

---

## Database Location

### Recommended Locations

**For single-user installations:**
- `C:\Users\<username>\AppData\Roaming\VFX_Launcher\vfx_launcher.db`

**For multi-user installations (shared database):**
- Network drive: `\\server\shared\VFX_Launcher\vfx_launcher.db`
- Local shared folder: `C:\ProgramData\VFX_Launcher\vfx_launcher.db`

### Changing Database Location

1. Edit `config.ini` in the application directory
2. Update the `Path` value under `[Database]`
3. Restart the application

---

## Troubleshooting

### Build Issues

**"PyInstaller not found"**
```bash
pip install pyinstaller
```

**"Module not found" errors during build**
```bash
pip install -r requirements.txt
```

**Icon not found**
- Ensure `ui_slick/icons/icon_3_512.png` exists
- Check that all icon files are present in `ui_slick/icons/`

### Runtime Issues

**"Database file not found"**
- Check `config.ini` for correct database path
- Ensure the database directory exists and has write permissions

**Application won't start**
- Check logs in `<data_dir>\logs\`
- Run from command line to see error messages

---

## Distribution

### Files to Include

When distributing the installer:
- `VFX_Launcher_Setup.exe` (from `installer_output\`)

When distributing the portable version:
- Entire `dist\VFX_Launcher\` folder
- Create a `config.ini` file with appropriate paths

---

## Notes

- The application requires Windows 7 or higher
- First run will create the database and necessary directories
- Icons are embedded in the executable
- The application can run without installation (portable mode)

