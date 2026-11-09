# VFX Launcher 0.4.0

The user-approved Keyloom visual refresh for the native Rust desktop:

- Neutral gray panels and recessed fields, with consistent borders and text.
- Shared blue primary buttons with hover, pressed and disabled states.
- Compact control sizing, spacing and active workspace tab underlines.
- Embedded Noto Sans and IBM Plex Mono fonts for consistent typography.
- Unified status colors and neutral progress indicators across workspaces.
- Font license notices included in Linux and Windows packages.

The UI branch was approved from a running Linux preview and merged into main.
Projects, Shots & Sequences, Incoming, Copy by File Type, Backup and Settings
retain their existing workflow and file-operation behavior. All 0.3.4 fixes
are included. Earlier screenshots in the guides predate this visual refresh.

Linux x86_64 AppImage and Windows x64 portable ZIP are built from the same
source. Windows uses the established native kamino-3 build process with static
CRT. The Linux build uses Ubuntu 22.04 libraries. No test suites run as part
of packaging; version/help and archive/checksum checks are distinct from GUI
or workflow verification. The Linux preview approval does not establish every
page, DPI setting or Windows rendering behavior. Earlier media/provider,
network-storage and power-loss verification limits continue to apply.
