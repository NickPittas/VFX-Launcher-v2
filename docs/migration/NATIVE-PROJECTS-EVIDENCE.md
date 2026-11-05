# Native Projects evidence — initial launch

Update: the input blocker described below was diagnosed and corrected. See
DESKTOP-AUTOMATION.md for successful movement across all three screens and live
Launcher mouse/keyboard interaction. Registration/scanning still await the
separate folder-picker issue.

2026-11-05. Linux, native egui/eframe window. This is a limited launch/appearance
check, not workflow verification or approval of the Rust UI.

- Built the executable with bounded incremental `cargo build --offline --locked
  --bin vfx-launcher` commands. First 30-second attempt timed out while compiling
  dependencies; the next completed. Rebuilt after coordinator corrections.
- Launched with `--data-dir` pointing to
  `artifacts/verification/native-data`, then restarted with the same disposable
  catalog. Both windows opened and were closed through the compositor's native
  close request; both app processes exited with code 0.
- Current-source capture: `artifacts/screenshots/native-projects-empty-current.png`
  (2546 × 1400). Earlier launch captures are retained separately.
- Observed compact menus, horizontal workspaces, top Projects controls and
  adjacent empty panes. No populated-project visual pass is claimed. The window
  manager tiled the window; intended 1440 × 880 and narrow-window layout remain
  unchecked.
- GUI input automation failed: initially the ydotool socket was absent; starting
  the existing user service restored the socket, but pointer convergence still
  failed on the multi-monitor layout. A keyboard focus check did not establish
  reliable input. The service was stopped afterward, restoring its prior
  non-running state. No desktop configuration was edited. Compositor dispatcher
  syntax attempts also failed before the documented Lua window-close API worked.
- Consequently, Add existing folder, registration, scanning, favorite/version
  selection, launching, populated resize behavior and cancellation were NOT
  exercised through the GUI. Nuke/AE and the file manager were not launched.
- Disposable scanner inputs exist under
  `artifacts/verification/native-projects`; they were not registered or scanned.
  They are verification artifacts, not a test suite.

Coordinator source corrections after the delegate handoff:

- Retained the scan control through reconciliation and made Cancel scan reach
  it, including cancellation while the project queue is being fetched.
- Added a Cancel scan control while the full scan/reconcile queue is active.
- Partial scans now report Partial; terminal scan counts retain observed files.
- Restored Validate selection text in the unfinished operations strips.
- Retained the settings worker handle and made close wait nonblockingly for
  accepted settings writes to drain. Closing also cancels retained reconciliation
  control. These final lifecycle corrections compile; their active-work runtime
  behavior was not exercised by the idle-launch captures.

Some captures include pre-existing desktop crash notifications (including the
desktop portal). They are not evidence of a Launcher crash; both observed
Launcher processes exited with code 0.

No test suite or global tests were created or run. This evidence covers Linux
startup, disposable catalog restart and orderly idle close only. Windows,
interactive workflows, active-job close and actual application launching remain
unverified.
