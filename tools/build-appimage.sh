#!/usr/bin/env bash
# Package an already-built release binary; see docs/LINUX-APPIMAGE.md.
set -euo pipefail
project_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$project_root"
[[ $(uname -m) == x86_64 ]] || { echo 'This recipe currently builds x86_64 only.' >&2; exit 1; }
downloads="$project_root/artifacts/packaging/downloads"
export TMPDIR="$project_root/artifacts/packaging/tmp"
mkdir -p "$downloads" "$TMPDIR" "$project_root/artifacts/releases"

fetch() {
    local destination=$1 checksum=$2 url=$3
    if [[ ! -f $destination ]]; then
        timeout 30s curl --fail --location --max-time 25 "$url" --output "$destination"
    fi
    printf '%s  %s\n' "$checksum" "$destination" | sha256sum --check --status || {
        echo "Tool checksum mismatch: $destination. Refusing to execute it." >&2; exit 1;
    }
    chmod +x "$destination"
}
fetch "$downloads/linuxdeploy-x86_64.AppImage" \
    df908b19f1ed1cdafa79508d415aa12d6c56ce4d56d4149aa8c09286718706e5 \
    https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
fetch "$downloads/linuxdeploy-plugin-gtk.sh" \
    b0f4cbc684a0103a9651f0955b635eaea0096b3a66c0f5a2c2aa337960375171 \
    https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/7a3fbc31a9e5075073ff8790f26effbac5f84453/linuxdeploy-plugin-gtk.sh
appimagetool="$project_root/build-tools/appimagetool-x86_64.AppImage"
printf '%s  %s\n' a6d71e2b6cd66f8e8d16c37ad164658985e0cf5fcaa950c90a482890cb9d13e0 "$appimagetool" | sha256sum --check --status

binary="${CARGO_TARGET_DIR:-$project_root/rust/target}/release/vfx-launcher"
version=$(timeout 20s cargo metadata --manifest-path rust/Cargo.toml --no-deps --format-version 1 --offline --locked |
    python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "vfx-launcher"))')
[[ $("$binary" --version) == "VFX Launcher $version" ]] || {
    echo 'Release binary is missing or has the wrong version. Build it first.' >&2; exit 1;
}
appdir=$(mktemp -d "$project_root/artifacts/packaging/VFX-Launcher-$version.AppDir.XXXXXX")
icon="$appdir/usr/share/icons/hicolor/256x256/apps/vfx-launcher.png"
install -Dm644 ui_slick/icons/icon_3_256.png "$icon"
export PATH="$downloads:$PATH" APPIMAGE_EXTRACT_AND_RUN=1 DEPLOY_GTK_VERSION=3
# GTK's linked libraries do not include the X11 keyboard library loaded by winit.
xkb_library="$(pkg-config --variable=libdir xkbcommon-x11)/libxkbcommon-x11.so.0"
timeout 180s "$downloads/linuxdeploy-x86_64.AppImage" \
    --appdir "$appdir" --executable "$binary" \
    --desktop-file packaging/linux/vfx-launcher.desktop --icon-file "$icon" \
    --library "$xkb_library" --plugin gtk
install -m755 packaging/linux/AppRun "$appdir/AppRun"
mkdir -p "$appdir/usr/share/X11"
cp -a /usr/share/X11/locale "$appdir/usr/share/X11/locale"
mkdir -p "$appdir/usr/lib/gio/modules" "$appdir/usr/share/doc/vfx-launcher"
printf '\nX-AppImage-Version=%s\n' "$version" >> "$appdir/usr/share/applications/vfx-launcher.desktop"
install -m644 docs/RELEASE-0.2.0.md "$appdir/usr/share/doc/vfx-launcher/RELEASE.md"
# Retain the copyright notices supplied with the Ubuntu build dependencies.
for notice in /usr/share/doc/*/copyright; do
    package=$(basename "$(dirname "$notice")")
    install -Dm644 "$notice" "$appdir/usr/share/doc/$package/copyright"
done
timeout 20s desktop-file-validate "$appdir/usr/share/applications/vfx-launcher.desktop"
# Reuse the checked-in tool's static runtime, avoiding an unpinned runtime download.
runtime="$downloads/runtime-x86_64"
offset=$("$appimagetool" --appimage-offset)
head -c "$offset" "$appimagetool" > "$runtime"
output="$project_root/artifacts/releases/VFX-Launcher-$version-x86_64.AppImage"
timeout 180s env ARCH=x86_64 VERSION="$version" "$appimagetool" \
    --runtime-file "$runtime" --no-appstream "$appdir" "$output"
chmod +x "$output"
(
    cd "$project_root/artifacts/releases"
    sha256sum "$(basename "$output")" > "$(basename "$output").sha256"
)
printf 'AppImage: %s\nAppDir: %s\n' "$output" "$appdir"
