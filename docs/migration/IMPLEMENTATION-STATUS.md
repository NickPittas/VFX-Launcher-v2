# Implementation status

## Packet F0 — foundation (2026-11-05)

Branch: `migration/rust-egui-suite`. Sole writer for this packet.
Files created by this packet (nothing else was modified):

| File | Contents |
|---|---|
| `rust/Cargo.toml` | One `vfx-launcher` package with a `vfx_launcher` library plus the `vfx-launcher` binary. |
| `rust/src/lib.rs` | Library root; exposes `app` and `jobs` and re-exports `VfxLauncherApp`/`run_gui`. |
| `rust/src/main.rs` | GUI/CLI entry: no arguments starts the window, `--help` prints usage, every other command is rejected with exit code 2. |
| `rust/src/app.rs` | Application state, approved desktop composition, six views, dialogs, pinned action/status strip, coordinated non-blocking close. |
| `rust/src/jobs.rs` | `JobId`, `JobControl`, `JobEvents`, `Jobs` registry: worker identity, request-versus-acknowledged pause, bounded event transport, terminal-report/reap separation, non-blocking shutdown. |

`rust/Cargo.lock` was **not** created: only Cargo may generate it, and no build/check command was authorized in this packet.

## Dependencies and API sources

Pinned exactly (`=0.37.0`) because the egui API changed in this release series
and the plan requires pinned versions at implementation time.

| Dependency | Features | Source consulted |
|---|---|---|
| `eframe = "=0.37.0"` | `accesskit`, `default_fonts`, `glow`, `system_fonts`, `wayland`, `x11` (defaults off: no `wgpu`, no clipboard/link features until the UI that needs them lands) | https://docs.rs/eframe/0.37.0/eframe/trait.App.html, https://docs.rs/eframe/0.37.0/eframe/index.html, https://docs.rs/eframe/0.37.0/eframe/struct.NativeOptions.html, https://docs.rs/eframe/0.37.0/eframe/struct.CreationContext.html, https://raw.githubusercontent.com/emilk/egui/0.37.0/crates/eframe/src/epi.rs, https://raw.githubusercontent.com/emilk/egui/0.37.0/crates/eframe/src/lib.rs |
| `egui_extras = "=0.37.0"` | none (tables only) | https://docs.rs/egui_extras/0.37.0/egui_extras/struct.TableBuilder.html, https://docs.rs/egui_extras/0.37.0/egui_extras/struct.TableBody.html, https://docs.rs/egui_extras/0.37.0/egui_extras/struct.Column.html, https://raw.githubusercontent.com/emilk/egui/0.37.0/crates/egui_extras/src/table.rs |

`egui` is reached through the `eframe::egui` re-export, so no separate `egui`
dependency can drift from the renderer version. Toolchain present on this
machine: `rustc 1.99.1`.

Verified 0.37 API facts that this packet depends on (checked against the
0.37.0 crate sources, not assumed from older versions):
- `eframe::App` requires `fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame)`; `logic` and `on_exit` are provided. Native `on_exit` takes `Option<&glow::Context>`.
- Side/top/bottom docks are `egui::Panel::{left,right,top,bottom}(id).resizable(..).default_size(..).min_size(..).max_size(..).show(ui, ..)`; `SidePanel`/`TopBottomPanel` are gone. `CentralPanel::show` also takes `&mut Ui`.
- `egui::MenuBar::new().ui(ui, ..)` plus `ui.menu_button(..)`; menus close with `ui.close()`.
- `Ui::selectable_label` is no longer the public path: selection uses `Ui::selectable_value` or `Button::selected(bool)`.
- `Ui::send_viewport_cmd` does not exist in 0.37; close requests go through `ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close)`.
- `Context::set_style`/`set_visuals` are replaced by `Context::all_styles_mut`; the dark palette is installed with `Visuals::dark()` inside that closure.
- `TextEdit::hint_text` requires `impl IntoAtoms<'static>`, so hint text is `&'static str`.

Close-request API re-verified for the review corrections, from the installed
crate sources rather than memory:

- `eframe` gives the close event to `App::ui` (and to `App::logic`) as `ViewportEvent::Close` in the frame input; `InputState::viewport()` returns `&ViewportInfo` and `ViewportInfo::close_requested()` is true for that frame (`egui-0.37.0/src/data/input/viewport_info.rs`, `src/input_state/mod.rs`).
- The cancel path is `ViewportCommand::CancelClose`, and `ViewportCommand::Close` is what a backend turns into the next frame's close event (`egui-0.37.0/src/viewport.rs`, `egui-winit-0.37.0/src/lib.rs`).
- `eframe` cancels the close only when `CancelClose` is present in the same frame's viewport output, for both the painted path and the hidden-window `run_logic` path (`eframe-0.37.0/src/native/epi_integration.rs`, `epi.rs`).
- `App::logic(&mut self, ctx: &egui::Context, frame: &mut Frame)` runs before `App::ui` and also while the window is hidden, so polling and close interception do not depend on a painted frame.
- `Context::request_repaint_after(Duration)` is the heartbeat used while a worker is unreaped.

## F0 review corrections (2026-11-05)

Second pass over `docs/migration/FOUNDATION-REVIEW.md`, same packet, same two
source files plus this document. The P3 execution-strip corrections were made by
the coordinator before integration and are preserved: `Copy selected` is the
file-type action, and Incoming keeps Copy/Move plus `Validate selection` and its
single changing execution label in the pinned strip, with no separate pane-level
Move action.

| Finding | Change |
|---|---|
| P1 worker exit without wakeup | `Drop for JobEvents` is the unwind-safe worker-scope exit guard: it stores the sink-gone flag and requests a repaint on normal return, silent return and panic unwind, after any already-sent event. The `Failed` disconnection fallback is unchanged, and `Jobs::poll` keeps a 100 ms repaint heartbeat while any worker is unreaped to cover the final race between that wakeup and the thread becoming joinable. |
| P1 UI-thread blocking close | `on_exit` calls the new non-blocking `Jobs::begin_shutdown`. `App::logic` calls `coordinate_close`, which cancels the close request with `ViewportCommand::CancelClose` (including a repeated request), signals cancellation, keeps polling with only `is_finished()` handles joined, and re-issues `ViewportCommand::Close` once no worker is unreaped. Nothing sleeps or joins a live worker, and the strip reports the wait. |
| P1 terminal event made a live worker dismissible | `JobStatus` now carries `outcome` (terminal report) and `reaped` (handle joined) separately. `is_busy` means unreaped, `Dismiss` requires terminal *and* reaped, `clear_finished` retains terminal-but-unreaped jobs, and the join preserves the outcome. Such a job renders `Finishing…`. |
| P2 unbounded event path | The `mpsc` channel is gone. Transport is one replaceable latest-progress slot (a throttled wakeup is deferred, not dropped), one diagnostic queue bounded at 64 with an omitted counter, and one independent terminal slot. Draining is finite per frame (32 diagnostics per job plus one progress read), terminal is applied first, and the UI reports the omitted count. No worker send can block or grow without bound. |
| P2 premature `Paused` | Requested pause and acknowledged checkpoint are separate flags. `Pausing` is shown until `checkpoint()` acknowledges, which also repaints; `Paused` is shown once the worker is idle at the boundary; `Resume` is offered in both states and repaints. The mutex/predicate wake protocol is unchanged. |

Limits that remain by design: a worker blocked in OS I/O cannot be interrupted, so
closing waits and says so rather than promising a timeout; unfinished-run
persistence is still a later packet. F1 paths and F4 discovery are **not**
implemented here, and this pass adds no tests, no build and no runtime claim.

Coordinator source inspection additionally corrected a terminal-read/producer-exit
race: after observing sink exit or joining the worker, drain rechecks the terminal
slot before synthesizing failure. Pending diagnostics continue scheduling frames
after worker reaping, and new jobs are rejected once shutdown begins. Formatting
checks pass; these corrections have not been executed in a native runtime.

## Implemented behavior

Application shell (`app.rs`):

- Compact horizontal menu bar and workspace tabs, with Settings at the right, matching the approved baseline. Coordinator corrected the initial obsolete sidebar implementation before integration. Both project tables enable column resizing.
- Menu items all work and nothing is decorative: `File → Quit`, `View → <view>`, `Help → About` (version dialog).
- Six views with docked, resizable panes that share top and bottom edges, internal table scrolling and no page headings:
  - `Projects`: `Scan folders` primary action, project filter field, `New project` dialog, project list pane and selected-project file pane (`File`, `Version`, `Launch`, `Modified`, `Last opened`, `By`, Launch adjacent to Version).
  - `Shots & Sequences`: current-project selector, checkable-destination-tree pane placeholder, sequence/shot action row, preview dock under the editors.
  - `Organize Incoming`: source and destination folder rows, `Suggest folders`, source tree pane, planned-files pane, selected-file preview dock, `Copy selected` pinned in the bottom strip.
  - `Copy by File Type`: source/destination, file-type selection area, scanning and copying progress bars, results area on the same page.
  - `Back Up Projects`: source/destination, exclusions, schedule control, Scanning/Finding/Copying phase bars with counts, results area.
  - `Settings`: `Applications`, `Project discovery`, `Users`, `Logs` sections.
- Pinned bottom strip (`action_status_strip`): job progress bar with phase and counts, current path, plain-language state text (including `Pausing`, `Paused`, `cancelling`, `finishing` and the closing wait), `Pause`/`Resume`/`Cancel` for a live job, `Dismiss` only after the worker was reaped, the count of omitted diagnostics when the bounded queue dropped any, and the view's single primary execution control on the right. Incoming's Copy/Move choice and `Validate selection` stay in this strip, and the one execution label follows the choice (`Copy selected` / `Move selected`).
- Empty state everywhere: the project list, file list and every work pane hold no sample data and render an explicit empty message instead.

Worker plumbing (`jobs.rs`, implements `CONTRACTS.md` §6 for this packet):

- `JobId` is a process-local `u64` counter starting at 1 and never reused.
- `JobControl` shares cancel, pause-request and pause-acknowledged flags plus a `Condvar` wake. `pause` only raises the request; `checkpoint()` acknowledges it (which sets the acknowledged flag and repaints the UI) before blocking, so the strip shows **Pausing** until the worker is actually idle at a safe boundary and **Paused** afterwards. `resume` is accepted in either state, updates the predicate under the wait mutex (no lost wakeup), notifies, and requests a repaint. `cancel` wakes a paused worker and returns `Cancelled` from the next checkpoint.
- `JobEvents` is the producer side and is bounded by construction, with no channel: one replaceable latest-progress slot per job (the newest value always wins, and a throttled wakeup is deferred with `request_repaint_after` rather than dropped, so the latest value is always painted), one diagnostic queue capped at 64 pending entries with a drop counter that the UI reports as "N diagnostics omitted", and one terminal slot that progress and diagnostics can never displace or delay. `finish(self, outcome, progress)` writes that terminal slot and consumes the sink, so a worker cannot report twice.
- `Drop for JobEvents` is the worker-scope exit guard: it marks the sink gone and requests a repaint on normal return, on a silent return without `finish()`, and while a panic unwinds, and it runs after any event the worker already sent, so the disconnection is stored before the wakeup. `Jobs::poll` then synthesizes `Failed` for a sink that disappeared without a terminal report.
- `Jobs` owns the registry: `start` attaches the UI context, spawns a named standard-library thread and returns `io::Result<JobId>`. `poll` drains each job (terminal first, one coalesced progress read, at most 32 diagnostics per job per frame), reaps only handles where `JoinHandle::is_finished()` is already true, and keeps a `LIVE_POLL_INTERVAL` repaint heartbeat while any worker is unreaped so a missed wakeup cannot strand a report or a finished thread. `current`/`statuses` return owned snapshots, `cancel`/`pause`/`resume` target one id, `live_job_count` reports unreaped workers.
- A terminal report and an actual joined handle are separate fields: `JobStatus::is_terminal` versus `JobStatus::is_reaped`. A job that reported but is still running stays registered, stays controllable, keeps its visible outcome, is shown as `Finishing…`, and counts as busy. `clear_finished`/`Dismiss` retain such a job, so clearing can never detach a live thread; the outcome is preserved across the join.
- `begin_shutdown` is the coordinated, non-blocking stop: it signals cancellation to every worker and polls once. It never joins a live worker and never waits, so it is safe on the UI thread and from `on_exit`.
- No file I/O, no persistence and no scheduling in this module.

## Deliberate limits of this packet

- No data source exists yet, so `Projects`, incoming, copy and backup lists are structurally complete but always empty. The `ProjectRow`/`FileRow` structs in `app.rs` are presentation placeholders and are replaced by catalog types in P1.
- Every control whose backend is a later packet is rendered **disabled** with the hover text "Not available in this build." rather than silently doing nothing: `Scan folders`, `Browse…`, `Suggest folders`, `Templates…`, `Add sequence`, `Add shot`, `Remove`, `Advanced…`, `Manual only`, per-row `Launch`, dialog `Create`, and each pinned primary action.
- Not implemented here: native folder browsing, path handling, folder scanning, catalog/storage, project creation, launching Nuke/After Effects, structure editing, incoming proposals, file-type filtering, backup planning/scheduling, AI suggestions, logs, persisted view sizes and keyboard resize/context-menu access. View sizes therefore reset on restart (no persistence feature is enabled).
- `jobs` unfinished-run recording, interrupt inspection and schedule re-arming remain for the backup/scheduler packets; `begin_shutdown` only signals cancellation and reaps already-finished threads.
- Shutdown is non-blocking by design, not by timeout: a worker blocked in OS network/disk I/O still delays its own exit. The window stays open and the strip reports `Closing — waiting for N workers to stop`. No timeout, forced kill or instant-cancellation guarantee is implemented or claimed.
- If the process is terminated externally while a worker is blocked, the remaining thread handles are dropped with the process; no state is persisted for it yet.

## Verification actually performed

- `rustfmt --edition 2024` writes followed by `rustfmt --edition 2024 --check` on `rust/src/app.rs` and `rust/src/jobs.rs` after the review corrections: clean, and rustfmt parsing confirms both files are syntactically valid.
- Review-correction source facts re-checked against the installed 0.37.0 crate sources: `egui-0.37.0/src/data/input/viewport_info.rs` (`ViewportEvent::Close`, `ViewportInfo::close_requested`), `egui-0.37.0/src/input_state/mod.rs` (`InputState::viewport`), `egui-0.37.0/src/viewport.rs` (`ViewportCommand::{Close,CancelClose}`), `egui-0.37.0/src/context.rs` (`send_viewport_cmd`, `request_repaint_after`), `egui-winit-0.37.0/src/lib.rs` (a `Close` command becomes the next frame's close event), `eframe-0.37.0/src/native/epi_integration.rs` and `epi.rs` (same-frame `CancelClose` check on the painted and hidden `run_logic` paths, `App::logic` hook).
- Every command in this pass ran under a 20–30 s process limit (`timeout 20s`/`timeout 30s` or the tool's own process timeout).
- Public API, feature and method checks against the 0.37.0 crate sources listed above (raw source reads of `button.rs`, `panel.rs`, `ui.rs`, `menu.rs`, `window.rs`, `progress_bar.rs`, `text_edit/builder.rs`, `style.rs`, `response.rs`, `epi.rs`, `eframe/lib.rs`, `egui_extras/table.rs`).
- **No build, `cargo check`, launch, screenshot, dependency install or test was run**, per the packet's command limits. No compile-pass claim is made, and `Cargo.lock` does not exist yet. Visual verification of the composition is likewise not claimed.

## Next packet

1. **F1 Paths** — `rust/src/paths.rs`: native identity, canonical display spelling, containment/alias/traversal classification for POSIX and Windows drive/UNC paths, non-UTF-8 handling, missing/offline roots. Add `pub mod paths;` in `lib.rs` (integration-owned file).
2. **F4 Discovery** — `rust/src/discovery.rs`: immediate-child and recursive scans, symlinks, partial directory errors, cancelled scan and disconnected root, emitting progress, diagnostics and a terminal report through the existing `Jobs::start(ctx, title, |job| …)` + `JobContext { control, events }` seam (`JobEvents::{progress, diagnostic, finish}`), and returning explicit coverage. The `Scan folders` controls in `Projects`, `Organize Incoming`, `Copy by File Type` and `Back Up Projects` are the intended first callers and can be enabled in that packet.
3. Then F2 storage/config and P1/P2 (catalog + launch), which replace the placeholder rows and enable `New project`/per-row `Launch`.

Ownership note: this packet wrote only the five files listed at the top plus this status document; the review-correction pass touched only `rust/src/jobs.rs`, `rust/src/app.rs` and this document. The Python application, migration documents and `gitclones/` references were read but not modified.
