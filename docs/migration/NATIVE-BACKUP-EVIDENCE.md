# Backup implementation and disposable evidence — 2026-11-06

The native backup page now uses the original NAS Backup workflow: collect `.nk`/`.aep` files with case-insensitive fnmatch exclusions, compare source/destination modification times with bounded workers, validate the changed-file manifest, and preserve older destination versions under numbered names before copying. Relative paths, native scan evidence and timestamps are retained. The existing journaled transfer engine performs mutations.

## Observed manually

- CLI dry run against `artifacts/verification/backup-manual-20261106`: 3 compared, 1 up to date, 2 changed; no catalog or destination writes during dry run.
- CLI execution in that disposable area: run 1 succeeded, 2 copied, 48 bytes. Readback showed the old `seq/comp.nk` at `seq/1_comp.nk`, new source contents at the original name, newer destination AE content unchanged, source files retained, and literal/character-class excluded folders plus unrelated media absent from the destination.
- Native application with `--data-dir artifacts/verification/backup-native-20261106` (absolute path supplied): Scan folders → Validate selection → Back up selected. One synthetic `.nk` copied, 32 bytes. Destination text read back correctly; the disposable SQLite journal recorded `kind=backup`, `state=succeeded`, actor `admin`, and 32 committed bytes.
- Native File → Quit exited normally (exit 0), before the 180-second observation cap.

Screenshots under `artifacts/screenshots/`: `backup-native-ready.png`, `backup-native-scanned.png`, `backup-native-validated.png`, `backup-native-completed.png`, `backup-native-report.png`. Persistent unrelated crash notifications are desktop notifications, not Launcher failures.

## Implemented but not yet exercised

Backup job import/export, atomic settings persistence, daily GUI scheduling and local DST handling. Imports keep schedules disabled pending review. These are source/compile claims only.

## Parallel execution and interval observation

The executor now accepts 1–16 workers. Directory prerequisites finish serially;
independent file actions use scoped workers with duplicated held descriptors,
shared immutable evidence and aggregate progress. Every worker joins before
finalization/error return. Cancellation settles unfinished ordinals individually.
Internal staging/displacement names avoid all reviewed destinations. The default
`execute` API remains one worker.

The four-worker CLI operation in `artifacts/verification/backup-parallel-20261106`
copied 18 synthetic files (2,640,000 bytes), preserving nine older destination
versions. Manual SHA-256 readback matched all 18 source/destination pairs. The
journal recorded 18 Copied and three Created directory outcomes; a subsequent
dry run reported 18 up to date and zero changes.

A native session using `backup-scheduled-20261106` started with a saved one-minute
interval. Without Scan/Validate/Copy clicks, the timer initiated all three stages
and copied `scheduled_comp.nk`, recording a successful backup run and 28 bytes.
`backup-scheduled-running.png` and `backup-scheduled-completed.png` show the native
timer and result; destination/journal readback confirmed them. File → Quit exited
normally. This establishes one ordinary interval trigger, not DST/busy/error cases.

Fresh source review found no parallel data-loss regression and one late-cancel
finalization gap. It is corrected: a completed run with no cancelled item retains
its actual terminal outcome even if cancellation is latched during worker joining.
The review also confirmed the six initial backup corrections. Stage cleanup now
refreshes changed permission bits from its held creating handle while retaining
the original device/inode, so metadata preservation does not prevent owned cleanup.

## Still incomplete

Non-Linux execution remains unsupported. Cancellation, pause, daily scheduling,
busy/pending schedule transitions, import/export dialogs, failed I/O and network
filesystems have not been manually verified for this backup workspace. This
document does not establish full application completion.

No test suite, automated test, production database or production media was used. Local library/binary Cargo checks and the native build passed without warnings.
