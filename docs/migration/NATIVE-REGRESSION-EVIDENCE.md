# Reported native regressions: verification in progress

2026-11-06. Disposable catalog and files under artifacts/verification only.

## Observed

An existing disposable project was scanned after adding one AE autosave inside
`Adobe After Effects Auto-Save` and one explicitly named `auto-save` project beside
it. The After Effects tab displayed only the original ordinary AE project.
The completed scan summary exposed Dismiss; clicking it removed the summary.

- [AE autosaves filtered](../../artifacts/screenshots/ae-autosaves-filtered.png)
- [Scan summary dismissed](../../artifacts/screenshots/project-scan-dismissed.png)

Copy by File Type scanned 12,000 synthetic `.nk` files without executing a copy.
Captures show Counting matching files with a live count, then the separate
Scanning phase with denominator 12,000, then completion. The local scan was too
fast to capture an intermediate determinate percentage; no such screenshot pass
is claimed. Every processed file emits progress, coalesced by the existing job
transport. Normal File → Quit exited 0.

- [Counting phase](../../artifacts/screenshots/count-copy-phase-1.png)
- [Processing denominator](../../artifacts/screenshots/count-copy-phase-2.png)
- [Completed scan](../../artifacts/screenshots/count-copy-finished.png)

The first registration attempt did not finish GTK folder selection before its
180-second process limit. It establishes no registration or scan pass. The later
checks used an already registered disposable project and typed copy roots.

## Source changes awaiting further verification

The native project-folder matcher now follows the original folder discovery →
configured-name matching → counting → file-processing phases, without shell
find/dir. It respects Settings folder names, skips nested duplicate target scopes,
and retains explicit partial/cancelled coverage. Its registered-Projects shortcut
keeps the selected scan scope. This later addition is compile-verified only.

User creation/rename/removal, project unregister, opened-by/time queries, history
inspection and operation actor journaling are now wired and compile. These new
controls still require their own native/manual verification. The new Pi adapter
is compile-verified but not yet connected to Incoming or exercised with a model.
No test suites or global tests were created or run.
