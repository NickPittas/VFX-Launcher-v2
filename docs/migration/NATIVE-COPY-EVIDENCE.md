# Copy by File Type: first native manual operation

2026-11-06, Linux, disposable inputs and catalog under `artifacts/verification/`.
This records one feature-local manual workflow, not a test suite or production
qualification.

## Observed operation

The native page scanned `artifacts/verification/copy-manual-20261106-a/source`
for `.nk` and `.autosave`. It found three matching files, including uppercase
`.NK` in `seq/SH010/`, and excluded `notes.txt`. Validation with Skip reported
two files to copy, one existing file to skip, and two folders to create.

Copy selected completed with 100% progress. The table showed two Copied and one
Skipped. Direct content readback confirmed both nested destinations matched
their sources, the existing destination retained `existing destination stays`,
`notes.txt` was absent from the destination, and all four source files remained.
Relative folder depth and filenames were preserved. File → Quit exited normally.

## Screenshots

- [Scanned files](../../artifacts/screenshots/copy-scanned.png)
- [Validated selection](../../artifacts/screenshots/copy-validated.png)
- [Completed native operation](../../artifacts/screenshots/copy-completed.png)
- [Completed bottom strip](../../artifacts/screenshots/copy-result-strip.png)
- [Corrected folder fields](../../artifacts/screenshots/copy-layout-corrected.png)

The first rendering exposed path fields compressed by the setup grid; explicit
field allocation corrected this and a follow-up screenshot confirms the fields
are usable. The progress/results dock's default and minimum height were enlarged
and its scroll area fills available space; expanded-report readability after that
change has not been verified. The bounded layout-only follow-up ended at its
60-second process limit; the earlier operation session quit normally.

## Implementation and limits

Scanning, validation and execution use tracked workers. The selected manifest
retains the original coverage; partial scans cannot become executable by selecting
a subset. Changing roots, extensions, policy or selection invalidates validation.
Execution consumes the validated plan once and uses the reviewed native executor.
Closing keeps catalog admission open until tracked operation workers finish.
Picked non-Unicode roots are refused rather than converted into another path.

The integrated library and binary compile without warnings. No automated tests,
production data, DCC launch or desktop configuration change was used.

Replace, Keep both, Preserve existing, Move, pause/cancel, close during execution,
network storage and non-Linux platforms remain unverified. Operation runs have no
actor column: a live user gates this page but is not persisted in its run journal.
The UI integration still requires independent source review.
