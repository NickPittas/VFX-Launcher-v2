# F0 foundation review

**Date:** 2026-11-05
**Disposition:** **Changes required before F0 sign-off.** The approved desktop shell is substantially reflected in the current source, but worker completion, shutdown, and queue behavior do not yet satisfy the F0 contracts. Later feature placeholders are not counted as defects.

## Ranked findings

### P1 — Silent worker exit or panic has no guaranteed UI wakeup

**Locations:** `rust/src/jobs.rs:259-268`, `rust/src/jobs.rs:306-320`, `rust/src/jobs.rs:398-406`; `rust/src/app.rs:193-195`.

`JobEvents::send` requests repaint, and `TrackedJob::drain` correctly synthesizes `Failed` on disconnection. However, a worker that returns without `finish()` or unwinds without sending another event drops its sender without requesting repaint. Since `Jobs::poll()` runs only from `App::ui`, the failure can remain undisplayed until unrelated input or window activity causes another frame.

**Action:** Add a worker-scope exit guard (or equivalent unwind-safe wrapper) that requests repaint on both normal return and panic unwind, then retain the existing disconnection-to-`Failed` fallback. Do not rely on a final progress send to provide this wakeup.

### P1 — Shutdown can block the UI thread indefinitely

**Locations:** `rust/src/app.rs:216-218`; `rust/src/jobs.rs:482-491`.

`on_exit` calls `shutdown`, which joins every worker synchronously. Cooperative cancellation cannot interrupt blocked network/OS I/O, and a worker that has not reached a checkpoint can prevent the native window from closing indefinitely. This conflicts with `CONTRACTS.md:157`, which explicitly requires shutdown not to block the UI thread.

**Action:** Make close a coordinated state: signal cancellation, cancel the immediate close request while workers remain, continue polling and joining only finished handles with scheduled repaints, and issue the final close after workers exit. Persistence of unfinished work remains a later packet; the F0 fix is the nonblocking lifecycle.

### P1 — A terminal event makes a live worker idle and dismissible

**Locations:** `rust/src/jobs.rs:235-242`, `rust/src/jobs.rs:328-334`, `rust/src/jobs.rs:443-444`, `rust/src/jobs.rs:477-490`.

Receiving `Finished` sets `outcome` immediately, so `is_busy()` becomes false even when `worker.is_finished()` is false. `finish(self)` consumes only the event sink; it does not prove that the worker closure has returned. `clear_finished()` then removes the registry entry and drops a possibly live `JoinHandle`, detaching the thread and discarding its control handle. Such a worker can continue cleanup—or any incorrectly placed post-`finish` work—without appearing in shutdown or busy-state tracking.

**Action:** Separate terminal outcome from worker reaping. Keep every job registered, controllable, and non-clearable until its handle is finished and joined; include an unjoined terminal worker in busy/exit tracking. Preserve the visible terminal report while waiting for that reap.

### P2 — The event path is not bounded, and progress is rate-limited rather than coalesced

**Locations:** `rust/src/jobs.rs:220-232`, `rust/src/jobs.rs:245-268`, `rust/src/jobs.rs:306-320`, `rust/src/jobs.rs:345-348`, `rust/src/jobs.rs:395`.

The channel is unbounded. Every diagnostic allocates and queues its message; `MAX_DIAGNOSTICS` caps only messages retained *after* the UI drains them, not producer growth. A stalled/hidden UI can therefore accumulate unlimited queued diagnostics, while progress rate limiting merely slows its own growth. The unbounded drain loop can also monopolize a frame when producers keep the receiver nonempty.

Progress calls inside the 100 ms window are discarded without retaining a latest value or scheduling a trailing update. Counts, phase, and current path can consequently remain stale for the rest of a long item; only the eventual terminal event is guaranteed to replace them.

**Action:** Keep at most one replaceable latest-progress event per job, including a trailing/latest update; bound queued diagnostics with explicit drop accounting; and provide a guaranteed terminal-event path that cannot be dropped behind diagnostics. Add a per-frame drain budget so a noisy worker cannot starve rendering.

### P2 — “Paused” is shown before the worker acknowledges a safe boundary

**Locations:** `rust/src/jobs.rs:130-137`, `rust/src/jobs.rs:171-194`, `rust/src/jobs.rs:325-326`; `rust/src/app.rs:316-322`, `rust/src/app.rs:876-877`; contract at `docs/migration/CONTRACTS.md:157`.

The corrected mutex/predicate protocol avoids the old lost-wakeup race, and cancel does wake a paused checkpoint. However, `status.paused` mirrors the requested atomic immediately. A worker still copying an item or blocked in I/O is therefore presented as “paused” before it reaches `checkpoint()`, contrary to the required Pausing/Cancelling acknowledgement model.

**Action:** Track pause request and safe-boundary acknowledgement separately. Show **Pausing** until the worker acknowledges the paused checkpoint; retain Resume/Cancel behavior and the existing wake protocol.

### P3 — Two pinned execution details diverge from the approved DCC baseline

**Locations:** `rust/src/app.rs:111-117`, `rust/src/app.rs:598-603`; approved references `docs/migration/ui-mockup.html:536-538` and `docs/migration/ui-mockup.html:575`.

Copy by File Type names its pinned action **Copy files**, while the approved action is **Copy selected**. Organize Incoming also places a separate **Move selected** control in the planned-files pane instead of keeping the Copy/Move choice and its single changing execution action in the pinned bottom strip. These are current shell/copy mismatches, not missing backend features.

**Action:** Rename the file-type action to **Copy selected**. Put Incoming’s Copy/Move choice in the bottom strip, update the one execution label from that choice, and remove the separate working-pane Move action.

## Confirmed source-level alignment

- Horizontal workspace tabs and trailing Settings are implemented at `rust/src/app.rs:224-272`; no obsolete primary sidebar remains.
- Projects, Shots & Sequences, and Organize Incoming use adjacent left/central panes with shared available bounds; their lower docks and working splits are resizable at `rust/src/app.rs:344-369`, `rust/src/app.rs:489-543`, and `rust/src/app.rs:547-608`. This is structural source confirmation only, not equal-height visual certification.
- Both existing Projects tables enable column resizing (`rust/src/app.rs:384-475`). Official `egui_extras` 0.37 source also confirms vertical table scrolling is enabled by default.
- Top **Scan folders** actions and the pinned bottom execution/status strip are present. The bottom strip is created before the central working view at `rust/src/app.rs:276-341`.
- The current condition-variable predicate updates and notifications are consistently protected by the wait mutex; no lost-wakeup defect was found in the corrected pause/resume/cancel protocol.
- No source-level mismatch was identified in the specific eframe/egui 0.37 APIs used here. The official 0.37 `App`, `Panel`, and `TableBuilder` sources were consulted, including `App::ui`, glow-specific `on_exit`, child `Panel::show`, resizable columns, and default table scrolling.

## F0 disposition and limits

F0 should remain open until the P1 lifecycle defects and the P2 queue/pause contract gaps are corrected and source-reviewed. The P3 execution-strip corrections are small and belong in the same foundation cleanup. No broader framework or architecture change is requested.

This was a source-only review of the assigned files and final DCC overrides. No source files were edited, and no build, `cargo check`, test/test script, application launch, dependency install, screenshot, server, worktree/clone, or production-data access was performed. Compilation, native runtime behavior, actual rendering, equal-height visuals, resize interaction, and accessibility remain unverified. Missing persistence, discovery, storage, transfers, scheduling, keyboard resizing, and other explicitly later-packet features were excluded from the defect list.
