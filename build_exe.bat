@echo off
REM Build script for VFX Launcher executable

echo ========================================
echo VFX Launcher - Build Executable
echo ========================================
echo.

REM Check if PyInstaller is installed
python -c "import PyInstaller" 2>nul
if errorlevel 1 (
    echo PyInstaller not found. Installing...
    pip install pyinstaller
    if errorlevel 1 (
        echo Failed to install PyInstaller
        pause
        exit /b 1
    )
)

echo Cleaning previous build...
if exist build rmdir /s /q build
if exist dist rmdir /s /q dist

echo.
echo Building executable...
pyinstaller vfx_launcher.spec

if errorlevel 1 (
    echo.
    echo Build failed!
    pause
    exit /b 1
)

echo.
echo ========================================
echo Build completed successfully!
echo ========================================
echo.
echo Executable location: dist\VFX_Launcher.exe
echo File size:
dir dist\VFX_Launcher.exe | find "VFX_Launcher.exe"
echo.
echo The executable is a single-file application.
echo No additional files or folders are needed.
echo.
pause

