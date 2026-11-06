# Linux AppImage

The Rust application is distributed as `VFX-Launcher-0.3.2-x86_64.AppImage`.
No Python checkout or Rust installation is required to run it.

```sh
chmod +x VFX-Launcher-0.3.2-x86_64.AppImage
./VFX-Launcher-0.3.2-x86_64.AppImage
```

`--version`, `--help`, `--data-dir /absolute/path` and the backup CLI are available
through the same AppImage. On systems without working FUSE, use
`APPIMAGE_EXTRACT_AND_RUN=1 ./VFX-Launcher-0.3.2-x86_64.AppImage`.

The compatibility build uses Ubuntu 22.04 libraries (glibc 2.35), x86_64 and a
desktop OpenGL driver. File transfers require Linux 5.6+ and filesystems that
support the executor's no-clobber rename and sync operations. Network paths must
already be mounted. Nuke, Pi and file managers are launched with their original
host library/GTK environment. No bundled library paths are installed globally.

Keep the `.sha256` file beside the AppImage and run
`sha256sum --check VFX-Launcher-0.3.2-x86_64.AppImage.sha256` to verify it.
See [release notes](RELEASE-0.3.2.md) for current limitations.

## Build

Use Ubuntu 22.04 x86_64, Rust 1.95+ and the locked Cargo dependencies. Building on
a newer distribution can raise the minimum glibc requirement of the resulting
AppImage; do not label such a build as the Ubuntu 22.04 compatibility build.

Install build dependencies in the build environment:

```sh
sudo apt-get install build-essential pkg-config curl python3 ca-certificates \
  libgtk-3-dev libxkbcommon-x11-dev libwayland-dev libegl1-mesa-dev \
  libgl1-mesa-dev librsvg2-dev libgdk-pixbuf2.0-bin desktop-file-utils
```

From the repository root:

```sh
timeout 1200s cargo build --release --locked --manifest-path rust/Cargo.toml --bin vfx-launcher
timeout 420s tools/build-appimage.sh
```

The recipe downloads checksum-verified linuxdeploy and its GTK plugin into
`artifacts/packaging/downloads`, reuses the checked-in appimagetool runtime,
bundles GTK resources/dependencies and creates the AppImage plus checksum in
`artifacts/releases/`. Templates are compiled into the Rust binary. Build
directories remain under `artifacts/packaging/`. `CARGO_HOME` and
`CARGO_TARGET_DIR` can point to project-local caches for isolated builds.

The linuxdeploy continuous download is checksum pinned: if upstream replaces the
asset, the recipe refuses it. Updating that pin requires inspecting the new
tool; it is never silently accepted. No test suites run as part of packaging.
