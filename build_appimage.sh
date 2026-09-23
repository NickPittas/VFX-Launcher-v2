#!/usr/bin/env bash
# Build VFX_Launcher as an AppImage (Linux x86_64)
# Requires: pip install --user pyinstaller; appimagetool is downloaded automatically
set -euo pipefail
cd "$(dirname "$0")"

APPIMAGETOOL=build-tools/appimagetool-x86_64.AppImage
if [ ! -x "$APPIMAGETOOL" ]; then
    mkdir -p build-tools
    curl -L -o "$APPIMAGETOOL" \
        https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage
    chmod +x "$APPIMAGETOOL"
fi

# Regenerate the Structure/ dir manifest (PyInstaller drops empty dirs, so the
# panel needs this list to recreate the preset in frozen builds)
if [ -d ui_slick/Structure ]; then
    (cd ui_slick/Structure && find . -type d -printf '%P\n' | sed '/^$/d' | sort) > ui_slick/structure.txt
fi

python -m PyInstaller --noconfirm --clean vfx_launcher.spec
APPDIR=dist/VFX_Launcher.AppDir
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin"
cp dist/VFX_Launcher "$APPDIR/usr/bin/"
cp ui_slick/icons/icon_3_512.png "$APPDIR/vfx-launcher.png"

cat > "$APPDIR/vfx-launcher.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=VFX Launcher
Comment=Manage and launch VFX projects
Exec=VFX_Launcher
Icon=vfx-launcher
Categories=AudioVideo;
Terminal=false
EOF

cat > "$APPDIR/AppRun" <<'EOF'
#!/bin/sh
exec "$(dirname "$0")/usr/bin/VFX_Launcher" "$@"
EOF
chmod +x "$APPDIR/AppRun"

ARCH=x86_64 "$APPIMAGETOOL" --appimage-extract-and-run "$APPDIR" dist/VFX_Launcher-x86_64.AppImage
echo "Built: dist/VFX_Launcher-x86_64.AppImage"
