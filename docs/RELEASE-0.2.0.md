# VFX Launcher 0.2.0

Linux x86_64 AppImage of the Rust/egui merged workspace, plus a Windows x64
portable preview:

- Projects catalog, version selection, user settings and legacy import.
- Project templates, clients, named shots/sequences and Nuke script generation.
- Incoming inventory, exact frame grouping, editable routing and Pi proposals.
- Copy by file type with source-relative paths and reviewed conflict policies.
- Manual and scheduled backups using the shared journaled transfer engine.
- Resizable Incoming tables, exact-frame inspection and per-file results.

This is a migration preview, not a claim of complete legacy parity or complete
runtime verification. See `docs/migration/IMPLEMENTATION-STATUS.md` and the native
evidence documents for remaining work. External Nuke/viewer integrations and
advanced regex compatibility remain incomplete. The transfer executor is Linux
only. Schedules run while the desktop application is open.

The Windows preview provides project browsing/scanning, settings and configured
application launching. File creation, copy/move and backup execution remain
unavailable on Windows and refuse without performing a transfer. This limitation
is shown in the application and in `README-Windows.md` inside the zip. It is not
Windows feature parity. The Windows asset's exact source commit is recorded in
its `BUILD-INFO.txt`; the existing Linux asset remains the verified AppImage from
the `v0.2.0` tag.

The AppImage bundles GTK3 and its supporting libraries. GPU drivers, the desktop
session, Nuke and the optional Pi harness remain host applications. Existing
local application data is reused; the package does not import a Python database
automatically or replace the Python application.
