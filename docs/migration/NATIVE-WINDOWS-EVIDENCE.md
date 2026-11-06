# Windows native transfer verification — 2026-11-06

This supersedes the Windows execution limitation in the earlier preview packet.
The Linux AppImage at v0.2.0 remains unchanged; the Windows zip records its later
source commit in BUILD-INFO.txt. This is a feature-local verification record,
not complete legacy parity or a full application sign-off.

## Implementation and review

Linux and Windows use `transfers/executor/shared.rs`, preserving the reviewed
journal-first staging/displacement/install/removal lifecycle. Linux adapts
openat2/renameat2/fsync. Windows uses user-mode NtCreateFile with held parent
handles, one native basename at a time, no descendant reparses, same-volume
descent, exclusive staging and NtSetInformationFile(FileRenameInformation) with
replacement disabled. File identities come from held handles. Deletion uses
handle disposition and confirms the name is absent before reporting Removed.

The Win32 SetFileInformationByHandle rename wrapper returned error 87 for a
non-null RootDirectory during the first disposable operation. That run created
one folder, refused installation and recorded Failed(0); no source was lost.
The native NtSetInformationFile replacement keeps the held-directory anchor and
no-clobber policy. It passed the subsequent operations below.

Kimi reviewed both adapters and the shared executor. No high-severity safety
finding was found; two reporting findings were corrected: uncertain source
removal wording, and recovery paths naming already-restored/deleted backups.
Native Windows compilation and actual operations follow the review.

## Evidence

All Windows work ran on kamino-3, Windows x64/MSVC, Rust 1.99.1, with static CRT.
The dedicated checkout and disposable data were under
`C:\Users\Nick\VFX-Launcher-v2-release\0.2.0\artifacts`.
No production catalog, incoming files or backup content was used.

- Native Windows cargo check and optimized release build succeeded, no warnings.
- Backup dry-run: 2 files, 3 reviewed operations, no mutation.
- Backup copy: 2 copied, 52 bytes; nested folder and contents verified.
- Backup Preserve Existing: 1 copied, 1 up-to-date skipped, 39 bytes; original
  content retained as `seq/1_comp_v001.nk` and new content installed at its name.
- Native Incoming Move: 2 exact EXR frames, 42 bytes, 2 folders created; UI and
  journal say Moved, destination contents match and both source names are gone.
- Native Copy by File Type Replace: 1 file copied, 30 bytes; destination content
  updated, operation-owned replacement backup removed, older deliberately
  preserved file untouched.
- Keep Both: incoming content installed as `2_comp_v001.nk`, preserving existing
  `comp_v001.nk` and `1_comp_v001.nk`.
- Skip: existing file left untouched; journal records 2 skips (folder and file),
  zero committed bytes.
- Native New Project: 37 directories and one 1,989-byte Nuke script created;
  project registered, generated script observed on disk. Nuke was not launched.
- A source junction caused a partial scan and refused backup before execution.
- Normal GUI close drained workers/catalog. Persisted run counts, checkpoints,
  outcomes and byte totals inspected from the closed disposable catalog.
- Linux shared-adapter regression: backup copied 2 files/50 bytes; changed backup
  copied 1/32 bytes with Preserve Existing; source/destination hashes matched
  and the numbered original retained its prior content.

Screenshots in `artifacts/screenshots/`: `windows-move-complete.png`,
`windows-replace-complete.png`, `windows-keep-complete.png`,
`windows-skip-complete.png`, `windows-project-created.png`,
`windows-project-scanned.png`. Early captures still show the obsolete preview
banner; the final package removes it. Final-package capture is recorded separately.

Other evidence: `artifacts/verification/windows-0.2.0/journal-outcomes.txt`,
closed disposable catalog snapshots and remote backup/build logs.
No tests/test suites or mutation harness scripts were created or run.

## Limits

Windows uses the user's approved completion policy: flushed file contents and a
successful native rename/removal; directory-entry survival of sudden power loss
is unconfirmed. Validation, intent warnings, checkpoints and item outcomes retain
that limitation. Windows ACLs, alternate data streams and unsupported attributes
are not copied. Read-only/locked files can refuse removal or replacement.

Network shares, cross-volume Move, injected I/O/journal failures, crash/power-loss
recovery, pause/cancel under load, non-ASCII/native ill-formed filenames and
external Nuke/AE acceptance were not runtime verified. No hostile-writer or
whole-batch atomicity guarantee. GUI captures prove the recorded workflows only;
the New Project form's clipped lower controls at its initial size remain an
existing layout issue, not a transfer-backend sign-off.

## Final published package

Clean build source: `6a3b2cf3a21a3c112d9be5b05639795d0803f128`.
`VFX-Launcher-0.2.0-Windows-x86_64.zip`: 7,169,591 bytes; SHA-256
`07e0ef99ebe65b2336020d61e0ac8369d186fcd435e66f16344f6c3098acdb2c`.
BUILD-INFO.txt records `Uncommitted source changes: False`.
The extracted final executable copied the generated 1,989-byte Nuke script in a
separate disposable backup (run 3, succeeded). Native GUI startup, populated
Projects/version row and obsolete-banner removal are captured in
`artifacts/screenshots/windows-final-populated.png`; normal close was requested.
No external Nuke process was launched.
