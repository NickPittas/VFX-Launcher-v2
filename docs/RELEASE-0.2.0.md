# VFX Launcher 0.2.0

Linux x86_64 AppImage of the Rust/egui merged workspace, plus a Windows x64
portable package:

- Projects catalog, version selection, user settings and legacy import.
- Project templates, clients, named shots/sequences and Nuke script generation.
- Incoming inventory, exact frame grouping, editable routing and Pi proposals.
- Copy by file type with source-relative paths and reviewed conflict policies.
- Manual and scheduled backups using the shared journaled transfer engine.
- Resizable Incoming tables, exact-frame inspection and per-file results.

This is a migration preview, not a claim of complete legacy parity or complete
runtime verification. See `docs/migration/IMPLEMENTATION-STATUS.md` and the native
evidence documents for remaining work. External Nuke/viewer integrations and
advanced regex compatibility remain incomplete. The transfer executor supports Linux and Windows. Schedules run while the desktop application is open.

The Windows package includes native file creation, copy/move, all four conflict
policies and backup execution through the shared journaled executor. Windows
Move/Replace uses normal Windows completion semantics after file flush and native
no-clobber rename; sudden power-loss namespace durability is unconfirmed and is
recorded in the application and journal. ACLs and alternate data streams are not
copied. The Windows asset's exact source commit is recorded in its
`BUILD-INFO.txt`; the existing Linux asset remains the verified AppImage from
the `v0.2.0` tag.

The AppImage bundles GTK3 and its supporting libraries. GPU drivers, the desktop
session, Nuke and the optional Pi harness remain host applications. Existing
local application data is reused; the package does not import a Python database
automatically or replace the Python application.
