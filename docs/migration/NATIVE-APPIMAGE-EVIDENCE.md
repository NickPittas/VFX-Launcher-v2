# AppImage 0.2.0 — 2026-11-06

Artifact: `artifacts/releases/VFX-Launcher-0.2.0-x86_64.AppImage` (about 38 MB).
SHA-256: `b83823dd67fbd0c4e69bb2253d4fb575403da699c942b0e8d39da04dd8b57939`.

## Build

- Cargo package and lockfile version: 0.2.0. About and `--version` use this value.
- Optimized release built with Rust 1.99.1 against Ubuntu 22.04 libraries in a
  user-namespace environment under `artifacts/packaging/ubuntu22-rootfs`.
- Ubuntu Base archive SHA-256 verified against its published SHA256SUMS:
  `242cd8898b33ea806ef5f13b1076ed7c76f9f989d18384452f7166692438ff1a`.
- No host packages, portal configuration or desktop settings changed. Some
  daemon package post-install ownership operations were unavailable in the
  single-UID build namespace; the compiler, headers and runtime libraries were
  installed and used successfully. No daemon from that environment was started
  on the host.
- The release executable's highest required GLIBC symbol version is 2.35.
- Checksum-pinned linuxdeploy and GTK plugin bundled native libraries, schemas,
  loaders and copyright notices. Templates remain embedded in the executable.
- Matching X11 locale/Compose data is bundled with libxkbcommon. AppRun preserves
  host values before setting bundle paths; DCC/reveal and Pi commands restore
  those values before spawning external host tools.
- `build_appimage.sh` now builds Rust rather than the legacy Python executable.

Build logs: `artifacts/packaging/release-build.log`, `release-rebuild.log`,
`appimage-final-build.log`. Sources passed rustfmt, Cargo check, shell syntax,
desktop-entry validation and Git whitespace checks. No test suite ran.

## Packaged feature checks

- Normal AppImage invocation: `--version` returned `VFX Launcher 0.2.0`, exit 0.
- Extraction fallback: `APPIMAGE_EXTRACT_AND_RUN=1 ... --help`, exit 0.
- The final AppImage also returned its version inside the Ubuntu 22.04 rootfs
  using extraction mode, exit 0. This is a CLI compatibility check, not a GUI
  pass on an Ubuntu desktop.
- The `.sha256` file verified successfully.
- Backup dry-run against the disposable `appimage-0.2.0/source` and destination:
  one source file, one to copy, three reviewed operations, exit 0. No transfer
  executed. Existing mount-continuity warnings remained visible in its output.

## Desktop observation

Only disposable app data under `artifacts/verification/appimage-0.2.0` was used.
The first package opened its window and About (0.2.0) and its GTK3 chooser.
The chooser registered the disposable source folder. Its 300-second process cap
later expired (124); that session is not a successful normal-shutdown check.
It also exposed Compose warnings from newer host tables, corrected by bundling
matching tables before the final package was built.

The final package reopened the disposable workspace, scanned the dummy
`Projects/Nuke/PackageCheck_comp_v001.nk`, and displayed its grouped file row,
version selector and scan completion. The folder chooser opened at that source
and Cancel returned to the application. File → Quit exited 0. The final GUI log
is empty, including no earlier Compose warnings.

Screenshots under `artifacts/screenshots/`:

- `appimage-0.2.0-about-settled.png` — About in the initial package.
- `appimage-0.2.0-final-scan-complete.png` — final package's discovered file.
- `appimage-0.2.0-final-picker.png` — final package's native chooser.
- `appimage-0.2.0-final-quit-menu.png` — normal close action.

## Limits

No production data, full-application tests, actual Nuke/Pi launch from this
package, or GUI verification on other distributions/GPUs. This packages the
current migration preview; it does not resolve the outstanding feature gaps in
the implementation status. No Flatpak or public release upload was performed.
