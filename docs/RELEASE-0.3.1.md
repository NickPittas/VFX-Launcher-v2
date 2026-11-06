# VFX Launcher 0.3.1

- Fix a shared scan/validation mismatch: links to files inside the selected
  source folder now copy from their captured, contained targets while retaining
  the original destination names. Execution still opens targets without following
  links and rechecks the reviewed file identity and metadata. Outside-root links
  remain refused. This was a confirmed source defect, not a confirmed diagnosis
  of the user's SMB directories.
- Execution buttons are allocated before long status labels so they remain
  visible and accessible in the bottom strip.
- Backup uses its own Run backup action label.
- Disabled Copy/Backup actions show their blocking reason directly; disabled
  controls also show tooltips. Item validation refusals expose the actual cause.
- Duplicate source-drive continuity messages are combined into one advisory
  behind collapsed review details. This advisory does not prohibit copying.

The published 0.3.0 AppImage's backup dry-run validated the disposable nested
project fixture with both reported continuity warnings (three operations,
exit 0). This confirms the warnings alone do not disable execution. It does
not establish the cause of the user's disabled UI state or SMB runtime parity.
No production files or databases were used and no desktop interaction occurred.

A second disposable fixture reproduced a blocked batch in 0.3.0 containing a
regular file and an in-root file link. The corrected 0.3.1 build validated it,
copied both files successfully (62 bytes), and both destination hashes matched
the source. An outside-root link was refused in a separate dry-run. The UI
keeps original selected names in review rows/results while execution journals
the captured target. This verifies the specific link correction locally;
it does not prove that links caused the user's reported SMB failure.

Per-file validation, identity checks, conflict handling and stale-plan guards
remain enforced. Live GUI and SMB transfer verification are still pending.
The media/Nuke runtime limitations in 0.3.0 continue to apply.
