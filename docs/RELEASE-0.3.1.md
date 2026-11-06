# VFX Launcher 0.3.1

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

Per-file validation, identity checks, conflict handling and stale-plan guards
remain enforced. Live GUI and SMB transfer verification are still pending.
The media/Nuke runtime limitations in 0.3.0 continue to apply.
