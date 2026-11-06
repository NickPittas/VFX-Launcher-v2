# Backup implementation and disposable evidence — 2026-11-06

The native backup page now uses the original NAS Backup workflow: collect `.nk`/`.aep` files with case-insensitive fnmatch exclusions, compare source/destination modification times with bounded workers, validate the changed-file manifest, and preserve older destination versions under numbered names before copying. Relative paths, native scan evidence and timestamps are retained. The existing journaled transfer engine performs mutations.

## Observed manually

- CLI dry run against `artifacts/verification/backup-manual-20261106`: 3 compared, 1 up to date, 2 changed; no catalog or destination writes during dry run.
- CLI execution in that disposable area: run 1 succeeded, 2 copied, 48 bytes. Readback showed the old `seq/comp.nk` at `seq/1_comp.nk`, new source contents at the original name, newer destination AE content unchanged, source files retained, and literal/character-class excluded folders plus unrelated media absent from the destination.
- Native application with `--data-dir artifacts/verification/backup-native-20261106` (absolute path supplied): Scan folders → Validate selection → Back up selected. One synthetic `.nk` copied, 32 bytes. Destination text read back correctly; the disposable SQLite journal recorded `kind=backup`, `state=succeeded`, actor `admin`, and 32 committed bytes.
- Native File → Quit exited normally (exit 0), before the 180-second observation cap.

Screenshots under `artifacts/screenshots/`: `backup-native-ready.png`, `backup-native-scanned.png`, `backup-native-validated.png`, `backup-native-completed.png`, `backup-native-report.png`. Persistent unrelated crash notifications are desktop notifications, not Launcher failures.

## Implemented but not yet exercised

Backup job import/export, atomic settings persistence, interval/daily GUI scheduling, local DST handling, and automatic fresh scan/validation before scheduled execution. Imports keep schedules disabled pending review. These are source/compile claims only.

## Still incomplete

Copy execution remains sequential; the worker control currently affects timestamp comparison only. Non-Linux execution remains unsupported. Cancellation, pause, scheduling, import/export dialogs, failed I/O and network filesystems have not been manually verified for this backup workspace. This document does not establish full application completion.

No test suite, automated test, production database or production media was used. Local library/binary Cargo checks and the native build passed without warnings.
