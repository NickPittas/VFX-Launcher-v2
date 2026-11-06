# AppImage 0.3.0 — 2026-11-06

Artifact: `artifacts/releases/VFX-Launcher-0.3.0-x86_64.AppImage` (39 MB).
SHA-256: `b2b0dfb00525c9ce92b88b95b6d602b2e4dba8fa33902317de659ed7b090975c`.

Built with the locked dependencies and Rust 1.99.1 in the existing Ubuntu 22.04
user-namespace build environment. Optimized compilation completed without
warnings. Packaging uses the existing checksum-pinned linuxdeploy/GTK plugin
and AppImage runtime, with bundled GTK libraries/resources and release notes.
Build logs are `artifacts/packaging/release-0.3.0-build.log` and
`artifacts/packaging/appimage-0.3.0-build.log`.

Checks completed:

- `cargo check --offline --locked --lib --bin vfx-launcher` for 0.3.0.
- Cargo formatting, packaging shell syntax and Git whitespace checks.
- Normal AppImage `--version`: `VFX Launcher 0.3.0`, exit 0.
- Extraction-mode `--help`: usage printed, exit 0.
- Extraction-mode `--version` inside Ubuntu 22.04: 0.3.0, exit 0.
- Generated SHA-256 sidecar verified successfully.

No GUI, Nuke/player launch, production-data operation or test suite was run
for this packaging request. CLI checks do not establish native rendering,
latest feature behavior or complete migration parity. The user is working on
the desktop; live verification remains pending. No Windows 0.3.0 build was made.
