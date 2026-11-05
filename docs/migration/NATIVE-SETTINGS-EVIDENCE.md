# Native Settings verification — 2026-11-06

Coordinator built the integrated binary under a 30s hard limit, then used the
computer-use skill and calibrated project helpers against a GUI session capped
at 180s. Every control command was capped at 20s. Data was exclusively the
disposable `artifacts/verification/native-data` catalog/settings.

Observed:

- View → Settings opens editable application paths, Linux Nuke command and project
  folder names. Save/Cancel remain pinned at the bottom.
- Editing Nuke to `cancel-check` shows Unsaved changes; Cancel restores unset.
- Saving `nuke-preview-only` completes, clears the dirty state, and the disposable
  settings JSON contains exactly that program name.
- Saving `relative/nuke` is refused with a readable absolute-path/PATH-name error.
- Cancel, clearing the field, and Save restore the initial unset value. Readback
  after File → Quit confirms all application settings remain null; exit code 0.

Screenshots in `artifacts/screenshots/`: `settings-form-ready.png`,
`settings-dirty-footer.png`, `settings-cancel-restored.png`,
`settings-saved-footer.png`, `settings-invalid-path.png`, `settings-quit.png`.

Coordinator source corrections retain/join only finished picker workers, include
them in close coordination, report spawn/panic failure and refuse non-UTF-8 picked
paths instead of saving replacement characters. No desktop settings changed.

Not exercised: picker selection/close, save I/O failures, user switches during
saves, active-write shutdown, restart reload, Windows/macOS, or DCC launches.
No tests or test suites ran. Those lifecycle cases remain source-review claims.

Source-review follow-up: fixed stale identity acknowledgement, worker-disconnect
saving lock, and stale picker error. These fixes compile; no concurrency runtime
pass is claimed.
