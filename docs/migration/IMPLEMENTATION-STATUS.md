# Implementation status

## Initial native Projects verification

See NATIVE-PROJECTS-EVIDENCE.md for bounded executable build, native launch,
disposable catalog restart, screenshots and idle-close evidence. Interactive
registration/scanning/launching were not verified because desktop input
automation failed. Coordinator corrected scan cancellation through reconcile,
partial outcome/count reporting, operation-strip wording, and settings-worker
draining during close. Latest library and binary cargo checks pass; no test
suite or global tests ran. Other workspaces remain unfinished.

## Coordinator F2 integration corrections

Catalog recognition now precedes persistent journal-mode changes. Mount
classification resolves existing ancestors before examining the mount table,
so a local symlink spelling cannot hide a known network target. This is still
snapshot evidence, with the documented mount-race/platform limits.
Settings load now applies the same value validation as save. Temporary config
files use exclusive creation, never truncating or following a pre-existing
temporary path; creation failure leaves that path untouched. No runtime pass is
claimed. The Linux library compile check passed after these corrections.

## Coordinator F1/F4 integration corrections

After the initial packet report, source inspection corrected missing imports,
cross-module metadata visibility, an unstable anonymous lifetime, Windows
optional metadata field conversion, reversed lexical containment labels,
root-level exclusions and suffix matching that had stripped the leading dot.
Wildcard exclusions now refuse the scan rather than ignoring the user's rule.
Repeated directory identities are skipped with Partial coverage to stop mount
cycles. Matching before/after root stat identities no longer promotes continuity
to Proven: discovery has no trusted continuous mount observation, so catalog
pruning remains disabled without additional evidence.

The paths module passed a standalone rustc metadata compilation. The new Rust
library passed `cargo check --offline --locked --manifest-path rust/Cargo.toml
--lib` after those fixes, on Linux. Cargo generated Cargo.lock; dependency cache,
temporary compiler files and target outputs were kept inside this project.
The first offline attempt lacked egui_extras; bounded dependency resolution and
the first compilation exposed the lifetime error, which was corrected before
the passing repeat. No tests, test suites, launches or screenshots were run.
This supersedes contradictory original packet descriptions below; Windows and
filesystem runtime behavior remain unverified.

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

## Packet F1 + F4 — paths and read-only discovery (2026-11-05)

Branch: `migration/rust-egui-suite`. Sole writer for this packet; no other
writer was active. Files written by this packet (nothing else was modified):

| File | Contents |
|---|---|
| `rust/src/paths.rs` | F1: module-owned `PathError`/`PathCategory`/`PathIssue`, private comparable `PathKey`, `path_key`, `normalize_lexical`, `relation`/`PathRelation`, `evidence_same_file`, `RootIdentity`/`FileIdentity`, `validate_destination`, `inspect_root`, `utf8_spelling`, `retain_imported`/`ImportedPath`/`PlatformTag`. |
| `rust/src/discovery.rs` | F4: `ScanPurpose`, `SuffixMode`, `ScanFilters`, `default_filters`, `ScanRequest`, `FileStamp`, `FileEntry`, `Coverage`, `RootContinuity`, `ScanSnapshot`, `ChildDirectories`, read-only `walk` and `child_directories`. |
| `rust/src/lib.rs` | Added `pub mod discovery;` and `pub mod paths;` with updated crate doc (integration seam only). |

## Implemented behavior — F1 paths

- `path_key(path, base)` computes an opaque, comparable lexical key. Relative
  inputs resolve against the explicit absolute `base` (never the process CWD);
  absolute inputs ignore `base`. POSIX keeps case, backslashes (a legal name
  byte) and non-UTF-8 bytes verbatim; `.` is dropped and `..` pops lexically
  with the root as its own parent (`normpath` semantics). Windows keys are the
  encoded bytes ASCII-case-folded, with the drive/UNC prefix preserved. No
  `canonicalize` anywhere: offline identity is purely lexical.
- `normalize_lexical(path, base)` returns the normalized native spelling as a
  `PathBuf` rebuilt without `push` separator heuristics (prefix `C:` + root
  `\`, UNC prefix + root, and POSIX roots are joined explicitly), preserving
  original case and non-UTF-8 bytes.
- Drive-relative Windows inputs (`C:foo`) are rejected (`InvalidInput`): their
  meaning depends on per-drive CWD state, which is never invented here. A
  non-absolute base is likewise rejected.
- `relation(a, b)` classifies `Same` / `FirstInsideSecond` /
  `SecondInsideFirst` / `Disjoint` on normalized spellings only; the doc
  states explicitly that lexical disjointness is not proof of distinct files.
- `evidence_same_file(a, b)` returns `Some` only from definite stat identity
  (POSIX dev+ino, Windows volume serial + file index) and `None` when either
  stat fails or evidence is missing. Identity comparison requires shared
  evidence: two identities carrying no common evidence kind never compare
  equal. Hardlinks are not universally detected.
- `validate_destination(root, relative)` rejects, in order: non-absolute root,
  empty `relative`, absolute `relative`, NUL, `.`/`..` components, drive/UNC
  prefixes (`ForeignPlatform`), and per-platform invalid names — on Windows
  reserved characters, control bytes, trailing dot/space and reserved device
  stems (CON/PRN/AUX/NUL/COM1-9/LPT1-9); on POSIX only a first component that
  parses as a Windows drive/UNC spelling is rejected as foreign (later
  components named like `C:\x` are legal POSIX names and preserved). It then
  stats the root (followed deliberately as the user's anchor; missing →
  `MissingRoot`), and walks every *existing* ancestor of the target with
  no-follow metadata: any symlink, or on Windows any reparse point
  (`FILE_ATTRIBUTE_REPARSE_POINT`), is `Escape`; any non-`NotFound` stat error
  refuses the operation — confinement that cannot be proven is an escape.
  Absent components pass; this is snapshot validation only and execution must
  re-check with no-follow facilities. The returned value is
  `root.join(relative)`.
- `inspect_root(path)` maps `NotFound` → `MissingRoot`, `PermissionDenied` →
  `Permission`, everything else → `Unavailable`, and never reads a failed stat
  as "the files are gone".
- `retain_imported(raw)` classifies spellings by distinctive markers only
  (leading `\\` → Windows, `X:`/`X:\`/`X:/` → Windows, leading `/` → POSIX,
  everything else `Unknown`) and sets `native: Some` only when the tag matches
  the current platform. Foreign/unknown spellings stay unresolved; no
  mount/drive mapping is invented. A mounted Linux share and a Windows UNC
  path are equal only via an explicit later mapping.
- `utf8_spelling(path)` is `Option<&str>`; there is no lossy path→String
  helper in the public API. The only lossy rendering is `Path::Display` inside
  diagnostic messages, which are never round-tripped.

## Implemented behavior — F4 discovery

- `walk(request, control, events) -> PathResult<ScanSnapshot>`: `Err` only for
  a non-absolute root. Every filesystem outcome is an `Ok` snapshot with
  explicit coverage. Iterative explicit-stack walker; per-directory entries
  are sorted by native name bytes so traversal and diagnostics are
  deterministic regardless of readdir order.
- Coverage: `RootUnavailable` when the pre-scan root stat fails (entries
  empty, identity `None`); `Cancelled` when any checkpoint reports
  cancellation (entries so far retained; the post-scan identity check is then
  skipped); `Partial` when any directory read, entry stat, broken symlink or
  unrepresentable mtime occurred, or when the post-scan identity check cannot
  re-prove the same root; `Complete` otherwise. Post-scan `inspect_root`
  compared against the pre-scan `RootIdentity` (dev+ino / volume+file index)
  sets `RootContinuity::Proven` or `Uncertain`; `Uncertain` is the default and
  the catalog packet must treat "not seen" as unverified, never deleted. A
  failed subtree never produces an empty successful result.
- Entry classification: `readdir` file type first; symlinks are resolved once
  (followed stat) so directory links become `DirectoryLink` — reported as an
  `Unsupported` `PathIssue` and never descended — and file links are stamped
  through their target (legacy `os.walk`+`getsize` parity; broken links
  degrade to `Partial`). Windows adds a reparse-attribute check (junctions and
  mount points are not descended) via `DirEntryExt`/`MetadataExt::file_attributes`.
  Unknown readdir types (POSIX `DT_UNKNOWN`) resolve through no-follow
  `DirEntry::metadata`. Non-directory special files (FIFOs etc.) are treated
  as files, matching legacy `os.walk`.
- Skipped directory links are recorded as issues but are policy, not failure:
  they do not by themselves turn `Complete` into `Partial`.
- Filters: `SuffixMode::LastExtension` mirrors the legacy project scan
  (`splitext(f)[1].lower() in {'.nk', '.aep'}`, so a file literally named
  `.nk` does not match), with explicit case-insensitive exclusion of names
  ending `.nk~`/`.nk.autosave`. `SuffixMode::NameSuffix` mirrors extension
  copy (`file.lower().endswith(ext.lower())`, defaults `.nk`/`.autosave`) and
  backup (`find -iname '*.nk' -o -iname '*.aep'`). All matching is
  ASCII-case-insensitive on native bytes (documented approximation of the
  legacy Unicode `lower()`). `default_filters(Incoming|StructureExport)` is
  `None` — their pattern/sequence discovery belongs to I1/I2 and is not
  approximated here.
- `excludes`: literal patterns match any single root-relative component or the
  full root-relative path, case-insensitively (legacy finder parity, minus
  wildcards). Patterns containing `*`/`?`/`[` are announced through a
  diagnostic and *ignored* rather than misapplied as literals; fnmatch
  semantics arrive with the backup packet. The root itself is never excluded,
  and excluded directories are pruned without descent.
- Progress/diagnostics/pause/cancel: `events.progress(JobPhase::Scanning, …)`
  per directory with the current path; every issue is both collected into
  `snapshot.errors` and emitted as a bounded diagnostic;
  `control.checkpoint()` runs between directories and every 128 entries
  (64 for child listings). The terminal `finish` mapping is deliberately left
  to the caller.
- `child_directories(root, control, events)`: immediate children only, no
  descent, directory symlinks included (legacy `scandir`+`is_dir` followed
  links when proposing registration candidates), sorted by case-folded name
  then full native path; same coverage vocabulary; no root identity is taken
  (registration is non-destructive by contract).
- Final ordering of `ScanSnapshot.entries` and `ChildDirectories.directories`
  is (case-folded display name, exact native path bytes): the fold is only an
  ordering aid, so no two distinct paths can compare equal and non-UTF-8
  names keep byte identity.

## Contract adaptations (deliberate, minimal)

- `EventSink` became `&JobEvents` and `walk` takes `&ScanRequest`; the F0
  seam is used as built (`progress`/`diagnostic`/`checkpoint`), and the
  terminal report stays with the worker body, matching this packet's scope.
- CONTRACTS' inline `suffixes`/`excludes` on `ScanRequest` became a
  `ScanFilters` value plus `SuffixMode`, because the three legacy tools use
  two genuinely different match styles.
- `ScanSnapshot` gained `purpose` and `RootContinuity`; both are consumed by
  later packets (P1 dispatch and prune gating) and are additive fields.
- `ScanPurpose::Incoming`/`StructureExport` exist but have no preset here.

## Deliberate limits of this packet

- `cfg(windows)` code paths (drive/UNC keys, reparse detection, reserved
  names, volume/file-index identity) were hand-traced only; they are not
  compiled on this Linux machine and no Windows behavior claim is made.
- ASCII-only case folding on Windows keys and suffix matching; legacy
  `normcase`/`lower()` folded full Unicode. Non-UTF-8 bytes keep exact byte
  identity everywhere.
- Verbatim `\\?\` spellings are anchored as std parses them; no Win32
  normalization equivalence is claimed for them.
- Wildcard excludes are announced and ignored, not applied.
- Bind-mount cycles could loop (legacy parity); cancellation remains the
  escape. No visited-identity cache.
- `walk` on `recursive == false` lists files in the root directory only;
  immediate-child *directories* are exclusively `child_directories`.
- Discovery is strictly read-only: the only fs calls in the module are
  `read_dir`, `metadata`, `symlink_metadata` and `DirEntry::{file_type,
  metadata}`. It never deletes or mutates files or catalog rows, and provides
  evidence (coverage + continuity + per-file identity) for later pruning
  decisions rather than making them.

## Verification actually performed

- `rustfmt --edition 2024` write + `--check` over all three files: clean, so
  all three files parse as valid Rust (this also syntax-checks the
  `cfg(windows)` blocks, which the Linux compiler would otherwise skip).
- Source hand-traces (paths): `"/mnt/nas/Projects/./foo/../bar"` over base
  `/mnt/nas` → key `[/ mnt nas Projects bar]`, spelling
  `/mnt/nas/Projects/bar`; `"/data/a\\b"` keeps `a\b` as one POSIX component,
  distinct from `/data/a/b`; `"Projects/foo"` over `/mnt/nas` anchors without
  CWD; `b"caf\xe9"` survives as raw bytes (`utf8_spelling` → `None`);
  POSIX `/data/Shot` ≠ `/data/shot` keys; Windows `D:\Shot` ≡ `d:/shot`
  (prefix folded, root normalized); `\\NAS\share\x` ≡ `//NAS/share/x`;
  `../../etc` over `/a/b` → `/etc`, and `..` at a root is ignored; relative
  `sub/../x` resolves against base only; drive-relative `C:foo` rejected.
- Source hand-traces (validation): `../x` and `/etc` rejected; `a\0b` rejected
  (NUL); `C:\x` / `C:x`(with separator or bare drive) rejected as foreign on
  POSIX; `shots/v01/file.nk` walks existing ancestors and passes with absent
  tails; a symlink (or Windows reparse point) anywhere in the existing chain
  → `Escape`; a permission error on an ancestor → refusal (fail closed);
  Windows `CON`, `COM1.bak`, trailing `shots ` and `a<b` rejected.
- Source hand-traces (filters): `Shot_v03.nk`, `SHOT_V03.NK`, `movie.aep`
  match the project preset; `shot.nk~` and `scene.nk.autosave` fail the
  `.nk`/`.aep` extension and hit the explicit exclusions; `.nk` (hidden file)
  does not match `LastExtension` (splitext parity); `auto.nk.autosave`
  matches ExtensionCopy's `.autosave` name suffix; `shot.nk~` does not match
  `.nk` name-suffix; backup preset equals `-iname '*.nk'`/`'*.aep'` for the
  shipped defaults.
- Source hand-traces (discovery): directory symlink → issue, no descent, no
  entry, coverage stays `Complete`; broken file symlink → `Unavailable` issue
  + `Partial`; unreadable subtree → `Partial` with siblings still collected;
  cancel mid-walk → entries-so-far + `Cancelled`; root missing at start →
  `RootUnavailable`; root replaced mid-scan → post-identity mismatch →
  `Uncertain` + `Partial`; FIFO included when the suffix matches; excluded
  `scratch` prunes descent; wildcard exclude emits a visible diagnostic and is
  ignored; sort `_z.nk` < `a.nk` < `b.nk` under ASCII-lowercase ordering with
  byte tie-break.
- Every command ran under `timeout 20s`/`timeout 30s`.
- **No build, `cargo check`, test, launch, screenshot, install, commit or
  push was run**, per this packet's limits. No compile-pass, runtime-pass or
  cross-platform claim is made; `Cargo.lock` still does not exist.

## Next integration (exact)

1. **P1 catalog** consumes this packet directly: `child_directories(root,
   control, events)` for Add/Multi-add (sort order matches the contract),
   `walk(&ScanRequest { root, purpose: ScanPurpose::Projects, recursive: true,
   filters: default_filters(Projects).expect("Projects preset") }, …)` for
   scans; prune-gating requires `coverage == Complete` **and** `continuity ==
   RootContinuity::Proven` **and** no cancellation; `FileStamp.identity`
   supports the multi-membership (no ownership stealing) design; `Paths`
   parent-of-`Projects` rule stays a P1 concern.
2. **UI wiring packet** enables the disabled `Scan folders` controls via the
   existing `Jobs::start(ctx, title, |job| …)` seam: call `walk` with
   `job.control`/`job.events`, then map coverage to the terminal report —
   `Complete` → `JobOutcome::Succeeded`, `Partial` → `Partial`, `Cancelled` →
   `Cancelled`, `RootUnavailable` → `Failed` with the snapshot's issues shown.
   No UI was touched in this packet and no placeholder was added.
3. **X1 transfers** must re-run `validate_destination` immediately before
   writes with no-follow facilities and treat `Escape`/`Permission`/
   `Unavailable` as refusals; `relation` + `evidence_same_file` are the
   intended source=destination/alias/nesting guards.
4. **B1 backup** replaces this packet's literal-exclude limitation with the
   fnmatch port and should keep the wildcard diagnostic as the unsupported-
   pattern path required by I1 parity.

Ownership note: this packet wrote only `rust/src/paths.rs`,
`rust/src/discovery.rs`, `rust/src/lib.rs` and this appended section. The
Python application, `gitclones/` references (read-only) and all other
documents were not modified.

## Packet F2 — storage and config (2026-11-05)

Branch `migration/rust-egui-suite`, shared checkout, sole writer for this packet.
Files written: `rust/src/storage.rs`, `rust/src/config.rs`, `rust/src/lib.rs`
(module declarations only), `rust/Cargo.toml`, Cargo-generated `rust/Cargo.lock`,
plus this section. In the same pass the coordinator extended ownership to
`rust/src/paths.rs` and `rust/src/discovery.rs` for the F1/F4 review findings
below. `app.rs`, `jobs.rs`, `main.rs`, the Python application, `gitclones/` and
all other documents were not modified. No UI integration, no legacy importer and
no tests were added.

## Dependencies (exact pins, verified against the installed crate sources)

| Dependency | Pin / features | Why | Source consulted |
|---|---|---|---|
| `rusqlite` | `=0.41.1`, features `["bundled"]` | Single writable SQLite catalog. `bundled` ships the amalgamation so the Linux and Windows targets need no system `sqlite3` or link flags. | Crates index `rusqlite` 0.41.1; extracted crate source in the project-local Cargo home: `src/pragma.rs` (`pragma_update`, `pragma_query_value`, `pragma_update_and_check`), `src/busy.rs` (`busy_timeout(Duration)`), `src/transaction.rs` (`Connection::transaction`, `unchecked_transaction`), `src/lib.rs` (`open_with_flags`), `src/error.rs` (`SqliteFailure`, `ErrorCode::{DatabaseBusy, DatabaseLocked, ConstraintViolation, CannotOpen, ReadOnly, DiskFull}`), `libsqlite3-sys-0.39.1/src/error.rs`. |
| `serde` | `=1.0.229`, features `["derive"]` | Versioned JSON envelopes. Same version already in the lock through the egui tree, so no duplicate copy. | Crates index; crate source in the same Cargo home. |
| `serde_json` | `=1.0.154` | Envelope parse/serialize and precise JSON error text. | Crates index `serde_json` 1.0.154. |

New transitive crates added to the lock by these pins: `libsqlite3-sys` 0.39.1
(bundled SQLite C sources, built by `cc`; `vcpkg` only for the non-bundled
path), `rsqlite-vfs`, `sqlite-wasm-rs` (wasm-only target support, not built for
Linux/Windows), `hashlink`, `fallible-iterator`,
`fallible-streaming-iterator`, `serde_json` and its `itoa`/`zmij`. `serde` /
`serde_derive` / `memchr` were already in the tree. Nothing else was added: no
async runtime, no ORM, no migration framework, no glob/casefold library, no
clock crate.

## Implemented behavior — `storage.rs`

**One worker, bounded queue, typed owned replies.** `Catalog::open(path)` only
spawns a named `vfx-catalog` thread and returns a handle; the open, validation,
migration and connection setup all happen on that thread. `Catalog::submit`
pushes a boxed request through a `sync_channel` bounded at 64 and returns a
`Pending<T>`; every typed method (`info`, `list_users`, `find_user`,
`create_user`, `update_user`, `delete_user`, `transaction`) builds on it.
`Pending::try_poll` is non-blocking and terminal-once, returning
`Waiting`/`Ready`/`Failed`/`Finished`, so an egui frame can poll without a
database call and a second poll never fabricates a value or a fake worker error.
Dropping a `Pending` discards only that reply; a dropping `Catalog` disconnects
the queue and the worker closes the connection (WAL checkpoint) on the way out.

**Explicit failure causes.** `StorageCategory` distinguishes worker startup,
worker gone/disconnected reply, full queue, busy/locked, unrecognized schema,
path, I/O, refused location, other database error and invalid request, each
carried with the affected path and a readable message. Submit-time validation
(blank/control/over-long username, empty update) fails synchronously before
anything is queued; database-side outcomes come back through the reply channel.
A caller closure that panics drops that reply (the caller sees `WorkerGone`)
instead of leaving the request pending forever.

**Open policy.** The path must be absolute; the parent directory is created;
`PRAGMA foreign_keys = ON` is applied and verified; the 5 s busy timeout is set
on the connection owned by the worker; `journal_mode = WAL` is applied only when
the containing mount can be certified local. On Linux that classification reads
`/proc/self/mountinfo`, picks the longest containing mount point (octal escapes
decoded) and refuses known network file systems (`cifs`, `smb2/3`, `nfs*`,
`fuse.sshfs`, `fuse.rclone`, `9p`, `ceph`, `glusterfs`, `davfs`, `s3fs`, …) with
a message naming the file system and mount point. Everywhere else — including
Windows — the location is `Unknown`: the catalog still opens, WAL is *not*
imposed, and that limitation is stated rather than guessed. A bind-mounted
remote directory inside a local mount is not distinguishable, so a "local"
answer is not a certificate.

**Schema identity and migration.** `PRAGMA application_id = 0x5646584C`
("VFXL") and `PRAGMA user_version = 1`. Creation and every future step run in
one transaction that also stamps `application_id` (fresh file) and the new
`user_version`, so a failed migration leaves the previous schema and version
untouched. Refusals, all without initializing or rewriting anything: a non-zero
foreign `application_id`; our `application_id` with a `user_version` above this
build's; a `user_version` with no migration path; a catalog missing an expected
table; and — the case that matters most — an existing database with tables, no
application id (`user_version` 0), which is exactly the legacy database layout,
reported with its table count and names. Only a database with no tables is
initialized.

**Schema version 1** creates `users` (tombstone column `deleted_at`, partial
unique index over live usernames), `projects` (native root columns, `added_by`
`ON DELETE SET NULL`, `removed_at`), `project_scan_folders`, `files` (version
text plus parsed number, size, `modified_unix_ns`, `modified_raw`),
`project_files` (unique `(project_id, file_id)` membership, reverse index),
`favorites`, `recents`, `file_access_log`, `user_activity_log` (raw username
retained for attribution), `import_origins` + `import_origin_rows` (the explicit
legacy-id → local-id map), `operation_runs` + `operation_items` (check-
constrained per-item states from `CONTRACTS.md` §6). CHECK constraints reject
blank/over-long usernames, non-boolean admin flags, unknown plan kinds and
unknown run/item states.

**Path storage.** `PathColumns` holds `storage` (`PathKey::storage_bytes`, the
compare key), the platform tag, the exact native spelling bytes and a separate
optional UTF-8 display spelling. Native encoding is POSIX raw bytes and Windows
UTF-16LE units of `encode_wide`, decoded back with `from_vec`/`from_wide` — no
lossy conversion and no `unsafe`. `PathColumns::unresolved` retains an imported
spelling with its detected platform and *no* catalog key, which is what F3 needs
for foreign data. `utc_timestamp_now`/`utc_timestamp` produce
`YYYY-MM-DDTHH:MM:SSZ` from std's clock only (no clock dependency).

**Users.** Create (trimmed, validated), update by coalesced field change (only
live users), tombstone delete (`deleted_at`, history kept, name released for a
new separate identity), list all with their tombstone state, and lookup by exact
username so the selected identity is restored without assuming `admin` is id 1.

**Transaction entry point for the next packet.** `Catalog::transaction(f)` runs
`f` inside one deferred transaction on the worker; `Ok` commits, `Err` or a
dropped early return rolls back, and the reply carries the outcome or the
mapped error (busy/constraint/I-O are distinguished). P1 registration and scan
commits and X1 run recording attach here; no P1 reconciliation is implemented.

## Implemented behavior — `config.rs`

Versioned envelopes (`schema_version: 1`) for settings and for the single
selected legacy import source. `Settings` holds external application paths and
the Linux Nuke command, project directory names, the selected username and log
level — no credentials, tokens or provider configuration. Path fields are
`JsonPath`, which *refuses* to serialize a non-UTF-8 native path instead of
replacing bytes. Load returns defaults only when the file is missing; malformed
JSON, a missing `schema_version`, a version this build does not support, or an
envelope that does not match the version (including unknown fields via
`deny_unknown_fields`) are readable errors and leave the file alone. Save
validates values first, then writes JSON to a sibling temporary file in the
destination directory, flushes and `sync_all`s it, renames it over the
destination, and syncs the directory on Unix; an encoding, write or rename
failure removes the temporary file and reports the cause, so the previous good
file survives. `default_data_dir()` resolves `$XDG_DATA_HOME/vfx-launcher` (or
`$HOME/.local/share/vfx-launcher`) / `%APPDATA%\VFX Launcher`, rejects relative
or empty environment values, and provides the settings, import-selection and
`catalog.sqlite3` paths. Nothing here reads or resolves the legacy INI, the
legacy database or the legacy favorites/JSON files.

## F1/F4 review corrections in the same pass

| Finding | Resolution in `paths.rs` / `discovery.rs` |
|---|---|
| P1 Windows rooted-relative paths | `anchored` rejects `!is_absolute() && has_root()` (e.g. `\temp`) with `InvalidInput` before any join, so the base anchor is never replaced; drive-relative `C:foo` rejection is unchanged. |
| P1 cancellation during listings | New `Listing` outcome (`Entries`/`Cancelled`/`Failed`) comes from `read_sorted(directory, control)`, which checkpoints **before** the read, every 128 entries while collecting, and **after** the read, so an empty directory cancelled before or after its read reports `Coverage::Cancelled` (walk/cancellation, child listing/cancellation) instead of a completed empty result. A blocked OS call still cannot be interrupted; cancellation lands on the next returned entry. |
| P1 non-regular files | `entry_shape`/`follow_link` return a new `EntryShape::Special` for FIFOs, sockets and device nodes (and links to them); `visit_entry` reports `Unsupported`, which degrades coverage to `Partial`. Only `is_file()` metadata — direct or through a file link — becomes a `FileEntry`. |
| P2 Windows identity evidence | Windows stamps re-stat `fs::metadata(entry.path())` because `DirEntry::metadata` does not populate `volume_serial_number`/`file_index`; `FileIdentity::from_metadata` now returns `Option`, `Some` only when the fields exist. `evidence_same_file` and the new `compare_file`/`compare_identity` return `None` for missing or disjoint evidence — no shared evidence is "unknown", never `Some(false)`; `same_file`/`same_identity` are `compare_… == Some(true)`. A failed Windows re-stat keeps the stamp without identity and reports the metadata failure. |
| P2 Unicode folding parity | New `paths::fold_native_text`: `to_lowercase()` over valid UTF-8 runs (legacy `normcase`/`lower()` parity for real text) and verbatim, `0xFF`-escaped preservation of ill-formed runs (non-UTF-8 POSIX bytes, WTF-8 unpaired surrogates). Valid folded text never contains `0xFF` and invalid runs are maximal, so the codec is injective: nothing is dropped, no replacement character appears, and two distinct spellings cannot fold together. Windows `PathKey`s use it (persisted catalog key format for schema 1 — a change needs a key migration); POSIX keys stay byte-exact. Discovery's suffix and exclude matching fold both sides with the same codec. |
| P2 `splitext` parity | `last_extension_matches` requires at least one non-dot byte before the final dot, so `.nk` and `..nk` have no extension while `a..nk` still matches `nk`; extension comparison uses the folded codec instead of ASCII-only `eq_ignore_ascii_case`. |

Kept as previously resolved: `RootContinuity` stays `Uncertain` for matching
before/after identities, wildcard excludes refuse the scan with a readable
reason, an excluded root is never excluded, repeated directory identities are
skipped and make coverage `Partial`.

Superseded earlier statements in this document: "Windows case folding is
ASCII-only", "`evidence_same_file` returns `Some` … `None` otherwise" (it is now
`None` also when no identity kind is shared), "non-directory special files …
are treated as files" / "FIFO included when the suffix matches", "suffix
matching is ASCII-only on native bytes", and the earlier `read_sorted` shape
that turned a listing failure into a plain `Err`.

## Verification actually performed

- `cargo fetch` once (network, 30 s limit) populated the project-local
  `artifacts/verification/cargo-home`; `rusqlite`/`serde_json` were not cached
  before, so this step needed network access.
- `cargo check --offline --locked --manifest-path rust/Cargo.toml --lib` on
  Linux (toolchain `rustc 1.99.1`) with
  `CARGO_HOME=artifacts/verification/cargo-home`,
  `CARGO_TARGET_DIR=rust/target`, `TMPDIR=artifacts/verification/tmp`:
  **passes with zero warnings and zero errors**. The first run compiled the
  bundled SQLite C amalgamation inside the same 30 s command and then reported
  one real error (`HashSet<String>::contains(&&str)`), which was fixed; the
  repeat with `--locked` is clean. `Cargo.lock` was updated only by Cargo.
- `rustfmt --edition 2024` write + `--check` on `config.rs`, `storage.rs`,
  `paths.rs`, `discovery.rs`, `lib.rs`: clean (also syntax-checks the
  `cfg(windows)` blocks the Linux compiler skips).
- Schema verification: the exact `SCHEMA_V1` text was extracted from
  `storage.rs` and executed **once** against a synthetic in-memory SQLite
  database with the system `sqlite3` CLI (no file database, no production data,
  no test file). Input `artifacts/verification/f2-schema-check.sql`, output
  `artifacts/verification/f2-schema-check.out`. Observed: DDL accepted;
  `application_id` 1447450700 and `user_version` 1 set inside the transaction;
  all 13 tables present; duplicate live username rejected by the partial unique
  index; tombstone-then-recreate accepted (2 rows, 1 live); blank username
  rejected by CHECK; `project_files` insert with unknown ids rejected by
  foreign keys; unknown `operation_runs.kind` rejected by CHECK; duplicate
  resolved project root rejected, while two unresolved (`NULL` storage) rows
  are allowed as designed.
- Source hand-traces (no execution), each traced through the code as written:
  - *Open/transaction*: empty file → `migrate(0, …)` transaction stamps
    `application_id`, runs `SCHEMA_V1`, sets `user_version = 1`, commits, then
    verifies tables. Legacy database (tables, `application_id` 0) → refused with
    table count/names, nothing written. Foreign `application_id` → refused.
    Our catalog with `user_version` 2 → refused as a downgrade. Our catalog with
    a dropped table → "missing tables: users". `Catalog::transaction` with a
    returning `Err` → `Transaction` dropped → rollback, reply `Failed`. Two
    concurrent connections: the second waits up to 5 s, then gets
    `SqliteFailure(DatabaseBusy)` mapped to `Busy` with the SQLite message.
  - *Request/reply*: queue at 64 → `try_send` `Full` → `QueueFull` error and no
    `Pending`; the catalog handle dropped → worker `recv` ends → connection
    closed; worker panic in a caller closure → reply sender dropped →
    `Pending::try_poll` → `Failed(WorkerGone)`; re-poll after `Ready` →
    `Finished`.
  - *Config*: missing file → defaults; `schema_version: 2` →
    `UnsupportedVersion`; `schema_version` absent → `InvalidJson`; unknown field
    → `InvalidValue` with the serde message; non-UTF-8 `nuke_path` → encoding
    error before any file is opened, previous file intact; rename failure →
    temporary removed, `Io`; kill during write → destination still holds the
    previous file. `SettingsStore::load` never rewrites and never falls back to
    defaults on a bad file.
  - *Paths/filters*: `\temp` rejected as rooted-relative; `C:foo` rejected as
    drive-relative; `..nk`/`.nk` have no extension, `a..nk` matches `nk`;
    `SHOT_V03.NK`, `카메라_v03.NK` match the project preset; `shot.nk~` /
    `scene.nk.autosave` still fail; a FIFO named `render.nk` produces an
    `Unsupported` issue and `Partial` instead of a `FileEntry`; cancelled
    before/after an empty directory read → `Cancelled`, not `Complete`.
- Every command ran under `timeout 20s`/`timeout 30s`. No tests, no test
  suites, no self-check harness left in the repository, no launches, no
  screenshots, no commits/pushes, no production database or incoming/backup data
  access, no system configuration change.

## Deliberate limits of this packet

- Windows `cfg` paths (UTF-16LE spelling codec, identity presence, reparse
  handling, `%APPDATA%` resolution) are hand-traced only; they are not compiled
  on this Linux machine and no Windows runtime claim is made.
- Network-mount refusal is best effort: `/proc/self/mountinfo` cannot reveal a
  bind-mounted remote directory, and non-Linux platforms report `Unknown`, which
  opens the catalog in the default journal mode rather than refusing or claiming
  WAL safety.
- WAL is not verified to be durable beyond SQLite's own guarantees; a
  filesystem that lies about `fsync`/locking is out of reach.
- The catalog is local-only by design. Shared multi-user catalogs, legacy import
  (F3), project registration/scan reconciliation (P1), transfer run recording
  (X1) and every settings UI (U3) remain unimplemented; nothing is claimed about
  them. `app.rs` still shows placeholder rows and no code path opens a catalog.
- `Schema version 1` is the first version, so the migration engine has one step;
  the transactional (version, statements) list is where version 2 attaches.
- Operation runs/items tables exist and are check-constrained but no writer for
  them exists yet, so interrupted-run recovery is still a later obligation.

## Next integration (exact)

1. **P1 catalog** (`rust/src/projects.rs`): open through `Catalog::open`/
   `Catalog::info`, register projects and scan folders inside
   `Catalog::transaction`, store roots with `PathColumns::native`, upsert
   `files` from `FileStamp` (`size` → `size_bytes`, `modified` →
   `modified_unix_ns`, version parsing → `version`/`version_number`) and
   memberships in `project_files`. Prune only with `Coverage::Complete` +
   `Proven` continuity (currently always `Uncertain`, which stays that way until
   discovery has trusted continuity evidence).
2. **F3 import** (`rust/src/import.rs`): record each source in `import_origins`
   and every source-id → local-id mapping in `import_origin_rows`, write
   unresolved spellings with `PathColumns::unresolved`, and select the source
   through `ImportStateStore`. Legacy source stays read-only.
3. **U3 settings** (`rust/src/ui/settings.rs`): `SettingsStore` +
   `default_data_dir()` own every settings write; the GUI never touches JSON
   directly, and a load error is displayed with its category/path instead of
   resetting the file.
4. **X1 transfers/backup**: persist intent through `operation_runs`/
   `operation_items` in one transaction before mutating files and outcomes after,
   and mark unfinished runs `interrupted` at startup.

## Packet P1 — catalog backend, plus storage/config review corrections (2026-11-05)

Branch `migration/rust-egui-suite`. Sole writer for this packet: one implementer
for `projects.rs`, and — after the read-only storage/config reviewer finished —
the same writer owned `storage.rs`, `config.rs`, `Cargo.toml` and `Cargo.lock` to
apply the review findings. `paths.rs`, `discovery.rs`, `jobs.rs`, `app.rs`,
`main.rs` and the reference clones were not modified.

| File | Change |
|---|---|
| `rust/src/projects.rs` | **New.** Catalog registration, scan reconciliation and the project/grouped-file views. |
| `rust/src/lib.rs` | Declares the `projects` module and names it in the crate documentation. Module declarations stay with the integration owner. |
| `rust/src/storage.rs` | Read-only-first recognition, canonical schema comparison, `appid=0` object rule, Windows catalog-location classification, retained worker handle with pollable shutdown, completion notifier. |
| `rust/src/config.rs` | `schema_version` error categories, Windows native replace with documented durability. |
| `rust/Cargo.toml`, `rust/Cargo.lock` | One `cfg(windows)`-only `windows-sys = "=0.61.2"` dependency (`Win32_Foundation`, `Win32_Storage_FileSystem`). Cargo resolved it from the already-present lock entry; no new crate version entered the graph. |

### Public catalog API (`rust/src/projects.rs`)

Everything returns `StorageResult<Pending<T>>` and runs on the single catalog
worker; nothing here executes SQL in an egui frame.

| API | Purpose |
|---|---|
| `plan_registration(&[PathBuf]) -> RegistrationPlan` | Preworker: stats and opens each candidate root, rejects missing/unreadable ones, and applies the legacy literal-`Projects` parent identity. No database access. |
| `register_projects(&Catalog, UserId, RegistrationPlan)` | One transaction; per-root `added` / `skipped` / `failed` in request order. |
| `list_projects(&Catalog, UserId, ProjectList, Option<String>)` | `All` / `Favorites` / `Recents` per selected user with a case-folded name/root filter and deterministic ordering. |
| `scan_scopes(&Catalog, ProjectId)` | Registered native scan folders plus unresolved display spellings; the only roots a scan may use. |
| `reconcile_scan(&Catalog, ProjectId, PathBuf, ScanSnapshot, JobControl)` | Validates scope, then commits observed files/memberships and (only with trusted coverage) prunes; always returns a `ReconcileSummary`. |
| `list_project_files(&Catalog, ProjectId, Option<FileKind>)` | Version-grouped files, newest first, with `latest` per group; optional Nuke/AEP view filter. |
| `set_favorite`, `mark_recent`, `log_file_access` | Per-user favorite/recent/access writes, live identity required. |
| `remove_project(&Catalog, UserId, ProjectId)` | Soft remove: `removed_at`, memberships, favorites, recents and one activity row; no filesystem change. |

### Preserved legacy behavior

- `ui_slick/async_workers.py::register_project_folders`: per-root results,
  already-registered roots skipped rather than duplicated, request order kept.
- `ui_slick/scanner.py::_update_database`: `os.path.basename(path) == "Projects"`
  registers the **parent** as project identity; files are upserted by path; the
  deleted-file sweep is skipped when the scan was cancelled or the root was
  unavailable, and each candidate is re-verified before removal.
- Current code still updates observed rows on a `Partial` scan and only skips
  the write on cancellation/unavailable roots, as the scanner did.
- `_parse_version_from_filename`: `_v<1-4 digits>` case-insensitive, and the
  `.aep` trailing-number fallback (last run, at most five digits).
- `ui_slick/file_handlers.py::get_base_name` / `group_versioned_files` and
  `core/utils.py::get_latest_version_file`: `_v<digits>` stripped from the group
  base name, `.aep` trailing-number stripping with `_`/`-` trim, latest = parsed
  version, then modification time, then a stable spelling.
- `core/database.py` favorite/recent semantics: recents upsert on access,
  favorites per user, `ORDER BY name` for favorites; project removal clears
  memberships, favorites and recents.

### Deliberate deviations (all documented in the module)

- `_parse_version_from_filename` returns `"1"` when nothing matches; the new
  catalog stores **no** version instead, so unknown versions stay visible rather
  than being invented (`CONTRACTS.md` §2).
- Only a live (non-tombstoned) user id may register, favorite, mark recent, log
  access or remove. No `id == 1` or `admin` assumption anywhere.
- `projects` carries `UNIQUE (root_platform, root_storage)`, so registration
  **revives** a soft-removed project with the same root instead of inserting a
  second row. Scan-folder rows are kept on removal for that revival; pruning
  only ever deletes `project_files` membership rows and never `files`, so shared
  files and their history survive.
- A file name that is not valid UTF-8 keeps its native identity and spelling
  columns, but `files.filename` cannot hold it: the column stays empty, the
  reconcile summary reports it, and grouping uses the native name bytes so two
  distinct non-UTF-8 names are never merged.
- Listing/filtering runs in Rust over the live rows so case folding uses
  `paths::fold_native_text` rather than SQLite's ASCII-only `lower()`.
- `JobControl` is honoured as a **cancellation** checkpoint on the catalog
  worker, not as a pause gate: `checkpoint()` blocks while paused, which would
  stall every other catalog request.
- Reconciliation's absence `stat` runs on the catalog worker so the prune
  decision and its evidence are one transaction. That loop is only reachable
  with `RootContinuity::Proven`, which discovery never reports today.

### Storage corrections from the review

| Finding | Correction |
|---|---|
| Foreign/legacy DB with a hot journal could be recovered before it was recognized | `read_header` reads the 100-byte SQLite header (magic, `user_version` at 60, `application_id` at 68) with **no** SQLite call, then `recognize_catalog` opens **`SQLITE_OPEN_READ_ONLY`** and refuses foreign ids, newer versions, unidentified object sets and any incomplete/empty-but-versioned state. A writable open only follows a recognized empty-or-owned state, and `inspect_identity` + `verify_catalog_schema` re-run on the writable connection, which narrows but does not close the replace-between-opens race. `immutable=1` is never used. |
| A hot journal on **our own** catalog would have been refused | Only when the header already proves this build's `application_id` does a read-only inspection failure with a database/I-O category fall through to the writable open (rollback needs write access); a latent schema objection still refuses before any write. |
| Schema verification was table names only | New `schema_objects` reads every non-internal `sqlite_schema` object (tables, indexes, views, triggers — not just tables) with whitespace-normalized DDL, and `verify_catalog_schema` compares it against the canonical schema built in an in-memory connection from `SCHEMA_STEPS`. `PRAGMA foreign_key_check` must also be clean. `EXPECTED_TABLES` is gone. Because the stored DDL text is the canonical text, a released step's SQL text must never be edited — a schema change is a new step and version. |
| `appid = 0` initialization was gated on tables only | `inspect_identity` now requires `user_version = 0` **and** zero non-internal `sqlite_schema` objects (views/indexes/triggers count), and refuses a `user_version != 0` with no objects. |
| Windows catalog location was unconditionally "unknown" | `classify_location` (cfg(windows)) refuses `Prefix::UNC` / `Prefix::VerbatimUNC` by spelling and asks `GetDriveTypeW` about the drive root: only `DRIVE_FIXED` is a verified local volume, `DRIVE_REMOTE` (mapped network volume) is refused with its root named, and removable/CD-ROM/RAM-disk/unknown/invalid-root stay `unknown` — which never implies WAL. Existing-ancestor resolution still runs first. |
| Worker thread handle was dropped; no wake for an idle UI | `Catalog` retains the `JoinHandle` and holds an `Arc<Mutex<Option<CompletionNotifier>>>`; `Catalog::set_completion_notifier` installs a framework-neutral `Fn()`, fired after every delivered reply and once when the worker queue ends. `Catalog::shutdown(self)` closes the request queue (accepted work drains, replies resolve) and returns a `CatalogShutdown` the GUI can poll with `try_finish()` or join from a non-UI thread with `join()`. `submit`/`transaction` stay non-blocking. Integration seam for the future UI: `catalog.set_completion_notifier(Some(Arc::new(move || ctx.request_repaint())))`. |

### Config corrections from the review

- `schema_version` categories: missing field → `UnsupportedVersion`; present but
  not a non-negative integer → `InvalidValue`; `InvalidJson` is now only for a
  document that cannot be parsed as JSON at all. Category docs updated.
- Windows saves now replace through `replace_file`: `ReplaceFileW` when the
  destination exists (keeps its identity/attributes/ACLs) and, on
  `ERROR_FILE_NOT_FOUND`, `MoveFileExW(..., MOVEFILE_WRITE_THROUGH)`. The temp
  file is still created with `create_new`, `write_all`ed and `sync_all`ed first,
  and any failure removes it and propagates the OS error. Durability is stated
  exactly: `REPLACEFILE_WRITE_THROUGH` is **documented as not supported**, and
  neither API documents flushing the parent directory entry, so Windows makes no
  directory-durability claim; Unix keeps `rename(2)` + directory `sync_all`.

### Verification actually performed

- `rustfmt --edition 2024` write on `projects.rs`, `lib.rs`, `storage.rs`,
  `config.rs`: clean, and it also syntax-checks the `cfg(windows)` blocks the
  Linux compiler skips.
- `cargo check --offline --locked --manifest-path rust/Cargo.toml --lib` with
  `CARGO_HOME=artifacts/verification/cargo-home`,
  `CARGO_TARGET_DIR=rust/target`, `TMPDIR=artifacts/verification/tmp`:
  **passes with zero warnings and zero errors** (`rustc 1.99.1`, Linux). The one
  intermediate warning (`ParsedVersion::padding` never read) was fixed by
  deriving padding from the stored version text instead. Cargo updated
  `Cargo.lock` by one line for the target-specific dependency; no crate version
  changed.
- `SCHEMA_V1` extracted verbatim from `storage.rs` and executed once with the
  system `sqlite3` CLI against `:memory:` (`artifacts/verification/p1-schema-v1.sql`,
  output `artifacts/verification/p1-schema-objects.out`): DDL accepted, 13 tables
  + 4 explicit indexes listed, which is the object set the canonical comparison
  expects.
- SQLite header offsets verified empirically on a synthetic database
  (`artifacts/verification/p1-header-probe.db`): bytes 60..64 = `00 00 00 01` for
  `user_version`, bytes 68..72 = `56 46 58 4c` = `0x5646584C` for
  `application_id`, matching `APPLICATION_ID`. The probe also confirmed SQLite
  stores `sqlite_schema.sql` as the statement text it was given, which is what
  the whitespace-normalized comparison relies on.
- Source hand-traces (no execution): registration of a fresh root, of an
  already-registered root, of a second scan scope on a live project, of a
  duplicate root inside one request, and of a soft-removed root (revival);
  reconcile of `Complete`/`Partial`/`Cancelled`/`RootUnavailable` snapshots,
  a snapshot whose declared root differs from the scope, a scope that is not
  registered, a non-project purpose, an entry lexically outside the scope, an
  overlapping second project (no membership touched), and pruning suppressed by
  `Uncertain` continuity; the group/version ordering against
  `get_base_name`/`get_latest_version_file`; the recognition matrix (foreign app
  id, newer version, `appid=0` with objects, `appid=0` with objects and a hot
  journal, empty file, our catalog with a hot journal, our catalog with a
  dropped index) through `read_header` → `recognize_catalog` →
  `inspect_identity` → `migrate` → `verify_catalog_schema`.
- Every command ran under `timeout 20s`/`timeout 30s`. **No tests, no test
  suites, no self-check harness, no launches, no screenshots, no installs, no
  production database or media access, no commits/pushes/PRs.**

### Deliberate limits of this packet

- The `cfg(windows)` code (`GetDriveTypeW` classification, `ReplaceFileW`
  replacement) was written against the official `GetDriveTypeW`, `ReplaceFileW`
  and `MoveFileExW` documentation and `windows-sys 0.61.2` sources, but it is
  **not compiled** on this Linux machine: `rustfmt` proves syntax only. No
  Windows runtime or compile claim is made.
- Canonical schema comparison is strict by design. A catalog created by a build
  that later edited a released step's SQL text would be refused; that is
  intentional, and the rule is documented next to `SCHEMA_STEPS`.
- `PRAGMA foreign_key_check` scans the catalog at open time. It is bounded by
  catalog size, not by media size, but a very large catalog pays a startup cost.
- Discovery still reports `RootContinuity::Uncertain` for every walk, so pruning
  remains unreachable in practice; P1 implements and gates it rather than
  enabling it.
- Registration validates existence/readability in `plan_registration` only. A
  root that disappears between that check and the transaction is still
  registered (it becomes an offline project), which is what legacy registration
  did and what keeps already-registered offline records intact.
- No UI is wired: `app.rs` still shows its placeholder rows. Add-folder,
  multi-add summary, scan-folder queue, favorites/recents and grouped file views
  are the next UI packet's work, together with P2 launch. No part of the app is
  claimed functional.
- Nothing here creates project folders, writes templates/scripts (S1/S2), copies
  or moves media (X1), imports legacy data (F3) or records operation runs.

### Next integration (exact)

1. **U1/P1 wiring**: construct the `Catalog` in application startup, pass
   `set_completion_notifier` an `Arc` that calls `ctx.request_repaint()`, and poll
   `list_projects`/`scan_scopes`/`list_project_files` `Pending` values from view
   state. `plan_registration` runs on a preworker thread; `register_projects`
   then reports `added`/`skipped`/`failed` per root. `Catalog::shutdown` returns
   a `CatalogShutdown` the GUI polls with `try_finish()` during close.
2. **P2 launch** (`rust/src/launch.rs`) consumes `ProjectFile`/`Project` values
   and `log_file_access`; it must not fetch these on the UI thread.
3. **F3 import** and **U3 settings** as previously listed; **X1** persists
   operation runs and reuses the same catalog worker.

## Packet U1 + P2 — Projects wiring, launch, and the five P1 review fixes (2026-11-05)

Branch `migration/rust-egui-suite`. Sole writer: one implementer. After the
read-only backend review returned, ownership extended from the UI files to
`projects.rs` and — for one small addition — `storage.rs`, so the review findings
are fixed in the same pass that consumes them.

| File | Change |
|---|---|
| `rust/src/project_workspace.rs` | **New.** All asynchronous Projects state: settings worker, catalog startup, user restore, project/file queries with stale-reply guards, registration pipeline, serial scan queue, launch/reveal workers, coordinated shutdown. |
| `rust/src/launch.rs` | **New (P2).** Native argv for Nuke/After Effects, environment sanitation, reveal. |
| `rust/src/app.rs` | Rewritten around the approved DCC composition; Projects and Settings wired to real state; other views keep explicit placeholders. |
| `rust/src/main.rs` | Lossless `args_os` parsing, `--data-dir <ABSOLUTE>`, `--help`, reject-unknown, exit codes. |
| `rust/src/lib.rs` | Declares `launch` and `project_workspace`; `run_gui(data_dir)`. |
| `rust/src/projects.rs` | The five P1 review findings (below). |
| `rust/src/storage.rs` | One addition: `TransactionOutcome` + `Catalog::transaction_or_rollback`. |
| `rust/Cargo.toml`, `rust/Cargo.lock` | `rfd = "=0.17.2"`, `shlex = "=2.0.1"`, eframe feature `clipboard`. |

### P1 review findings, and how each is fixed

| Finding | Fix |
|---|---|
| **HIGH** — `reconcile_scan_tx` applied observations and returned `Ok` even when cancelled; the non-pruning branch had no final cancel check and the pruning branch wrote before checking. | `reconcile_scan` now runs through `Catalog::transaction_or_rollback`. `reconcile_scan_tx` returns `(summary, keep)`; cancellation at **any** boundary (before the writes, inside the upsert loop, inside the prune loop, or at the final boundary after both) returns `keep = false`, which **rolls the transaction back**. `apply_observations` and `prune_absent_memberships` check `control.is_cancelled()` between items and return `true` when they stopped early. The delivered `ReconcileSummary` reports `cancelled = true`, `applied = false`, zeroed mutation counters and an explicit reason, so cancellation is a readable result, not an error and not a silent commit. `checkpoint()` is still never used on the catalog worker, so a paused job cannot stall other requests. |
| **MEDIUM** — grouping key used the base filename only. | `FileGroup` gained `timeline` and `shot`; the grouping key is `(timeline native bytes, shot native bytes, base name bytes)`. `bucket_of` reproduces `ui_slick/file_handlers.py::group_by_timeline` exactly: the **topmost** `Nuke`/`After Effects`/`AfterEffects` anchor that yields a timeline+shot pair wins (2 segments, then 1), and deeper nesting below the shot — artist folders such as `<shot>/nuke/script` — is deliberately ignored. No anchor → the legacy `group_by_shot` fallback (immediate parent, unless it is a plumbing name; then the `SH`/`sh` code matcher, then the first `_` part, then `Main`). Ordering is timeline → shot → base name, case-folded then byte-exact. The Projects table shows the bucket as a group separator row, so the approved `File | Version | Launch | Modified | Last opened | By` columns are unchanged. |
| **MEDIUM** — pruning trusted `RootContinuity::Proven` alone. | Pruning now additionally requires `snapshot.root_identity` to be present **and** a fresh `paths::inspect_root(scope)` whose `compare_identity` against the snapshot is `Some(true)`, evaluated immediately before pruning. Otherwise observations are kept and `prune_suppressed` says the identity could not be re-proven. Discovery still reports `Uncertain`, so no pruning path is reachable in this build. |
| **MEDIUM** — size/mtime conversions dropped overflow and overwrote known metadata with `NULL`. | `size_column`/`modified_column` push a `PathIssue` naming the path and the conversion cause (no wrapping, no truncation) and yield `None`. The file `UPDATE` now uses `size_bytes = COALESCE(?6, size_bytes)` and `modified_unix_ns = COALESCE(?7, modified_unix_ns)`, so a previously known value survives an unrepresentable reading. |
| **LOW** — the AEP version fallback only read a trailing digit run. | `parse_version` now takes the **last digit run anywhere in the stem** and keeps at most its last five digits, matching `re.search(r'(\d{1,5})(?!.*\d)', stem)`: `comp_12_final.aep` → version 12, `comp_123456.aep` → 23456. `base_name_bytes` is unchanged because the legacy `get_base_name` regex is anchored with `$` and therefore only matches a trailing run. |

### Implemented behavior — U1 Projects

- **Startup, off the UI thread.** `SettingsWorker` (own thread, command channel)
  creates nothing but loads `settings.json`; `Catalog::open` only spawns the
  storage worker. `info()` → `list_users()` → (fresh catalog only)
  `create_user("admin", true)` → project list. Every reply is a `Pending` polled
  in `logic`, never awaited in a frame. The catalog completion notifier is wired
  to `ctx.request_repaint()`.
- **Identity.** The recorded `selected_username` is restored by name against live
  users. `admin` is created only when the catalog has **no** identities; there is
  no `id == 1` assumption, no invented name, and a malformed settings file is
  reported and never overwritten (`save_settings` is a no-op until the load
  succeeds). A catalog with users but no recorded choice shows the toolbar
  chooser and gates writes instead of guessing.
- **Projects.** All / Favorites / Recent plus a debounced (250 ms) filter, all
  issued as `list_projects(user, tab, filter)`. Selecting a project loads
  `list_project_files` and records `mark_recent`; favorites toggle through
  `set_favorite` and re-read the list. **Stale replies** are dropped by
  `(request id, project id)` and by the list request id, so switching tab, filter,
  user or project can never display another selection's rows. No project counts
  are fabricated: the pane shows names only, with the folder path in the tooltip
  and context menu.
- **Add existing folder.** `rfd::FileDialog::pick_folders` runs on a worker
  thread (multi-selection when the platform supports it). `plan_registration`
  then runs on its own worker thread, and `register_projects` is polled as a
  `Pending`. The pane shows one summary line (`N added, M already registered, K
  failed`) with per-folder details collapsed underneath.
- **Scan folders.** The blue top action loads every project once, builds a queue
  of **registered** scan folders only, and walks them serially in a `jobs` worker
  using `discovery::default_filters(ScanPurpose::Projects)`. Progress and the
  current folder come from `JobEvents`; Pause/Cancel come from the existing strip.
  Each walk result is reconciled through `reconcile_scan` with that job's
  `JobControl`, and a cancelled walk records nothing (rollback path above).
- **Launch.** The row Launch control (the approved row exception) starts a worker
  that re-validates the file and the configured program, spawns, and only then
  does the UI submit `log_file_access`. Failures are shown with a readable
  summary; success says the launch **command started**, with the file path in the
  details, and never claims the application finished starting.
- **Panes.** `Panel::left` (resizable, bounded) beside the files table, both
  filling the same height; every table column is resizable; lists scroll
  internally. Right-click and `Shift+F10` on a focused row open the same menu:
  Launch, Reveal, Copy path, Information (projects: favorites, copy path,
  information). Copy uses `Context::copy_text` (eframe's `clipboard` feature).
- **Closing.** A close request is intercepted once: jobs are cancelled, the
  catalog handle is consumed by `Catalog::shutdown()`, and the window closes only
  when no worker is busy and `CatalogShutdown::try_finish()` reports the worker
  finished. Nothing joins on the UI thread; repaints keep running while the stop
  is pending; the handle is never dropped early, so accepted requests still
  resolve. `on_exit` only signals.

### Implemented behavior — P2 launch (`launch.rs`)

- `Tool::Nuke` on Linux: `shlex::split(nuke_launch_cmd_linux)` + the file appended
  as one `OsString` argument — the legacy `main_window.py::_launch_file` shape,
  tokenized by a POSIX lexer rather than a hand-written quote parser. Unbalanced
  quotes and an empty command are distinct, readable errors.
- `Tool::Nuke` elsewhere: `[nuke_path, "--nukex", file]`. `Tool::AfterEffects`:
  `[after_effects_path, file]`. No shell anywhere, so spaces and quotes in a file
  name stay inside one argument.
- Pre-spawn revalidation on the worker: the program must exist, be a file and
  (Unix) be executable; the file must still exist and be a file. A bare program
  name is a `PATH` lookup and is documented as unverifiable before spawn.
- Environment: the parent environment is inherited unchanged (licence variables
  survive). On Linux, `LD_LIBRARY_PATH_ORIG` restores `LD_LIBRARY_PATH` exactly as
  `core/utils.py::get_external_process_env` did; otherwise `LD_LIBRARY_PATH`,
  `PYTHONHOME`, `PYTHONPATH`, `QT_PLUGIN_PATH` and `QT_QPA_PLATFORM_PLUGIN_PATH`
  are removed **only** when an inherited bundle marker is present (`_MEIPASS` or an
  `_MEI*` path entry). Nothing is cleared wholesale.
- Reveal: Linux `xdg-open <containing folder>`, Windows
  `explorer.exe /select,"<path>"`, macOS `open -R`. Children are reaped by a
  detached thread so no zombie accumulates.

### Public API changes (exact)

```rust
// storage.rs — new
pub enum TransactionOutcome<T> { Commit(T), Rollback(T) }
impl Catalog { pub fn transaction_or_rollback<T, F>(&self, f: F) -> StorageResult<Pending<T>> }
// projects.rs — changed
pub struct FileGroup { pub timeline: Option<String>, pub shot: Option<String>,
                       pub base_name: Option<String>, pub filetype: Option<String>,
                       pub files: Vec<ProjectFile>, pub latest: Option<FileId> }
```
`reconcile_scan` keeps its signature; cancellation is reported inside the
summary. No other public backend signature changed.

### Verification actually performed

- `rustfmt --edition 2024` (write then `--check`) on `app.rs`, `main.rs`,
  `launch.rs`, `project_workspace.rs`, `projects.rs`, `storage.rs`, `lib.rs`:
  clean, which also syntax-checks the `cfg(windows)` reveal branch the Linux
  compiler skips.
- `cargo check --manifest-path rust/Cargo.toml --lib` and `--bin vfx-launcher`
  with `CARGO_HOME=artifacts/verification/cargo-home`,
  `CARGO_TARGET_DIR=rust/target`, `TMPDIR=artifacts/verification/tmp`: **pass,
  zero warnings, zero errors**. Every command ran under `timeout 30s`.
- Dependency resolution: `Cargo.lock` gained exactly `rfd 0.17.2`,
  `shlex 2.0.1`, `pollster 0.4.0`, `smithay-clipboard 0.7.3`,
  `smithay-client-toolkit 0.19.2`, `calloop 0.13.0/0.14.5`,
  `calloop-wayland-source`, `wayland-protocols-experimental`,
  `wayland-protocols-misc`. No existing crate version was removed or changed.
- Source hand-traces (no execution): settings-load-failure gating, empty-user
  catalog, restore-by-name, missing-name chooser, stale project/file replies
  across tab, filter, user and project switches, registration
  Dialog → Plan → Register → Summary, scan queue with a cancelled walk (rollback),
  unavailable root, launch failure/success ordering versus `log_file_access`,
  close coordination with a busy worker, and the bucket derivation against
  `group_by_timeline` for `Projects/Nuke/<timeline>/<shot>/<artist>/nuke/file.nk`
  (deeper folder ignored), a one-segment path, and the no-anchor fallback.
- **No tests, no test suites, no self-check scripts, no launches of Nuke, After
  Effects, the app itself, or any reveal, no production database/config access,
  no screenshots, no commits/pushes/PRs/worktrees.**

### Deliberate limits of this packet

- The application was not run, so no runtime or visual verification of the Rust
  UI exists; `DESIGN-APPROVAL.md` stays a design baseline, not a native
  acceptance. HTML screenshots remain the only visual evidence.
- The `cfg(windows)` reveal branch and the Windows `nuke_path` argv are written
  against the documented behavior but are **not compiled** on this Linux machine.
- Launch reports spawn acceptance only. It cannot observe whether the DCC
  application finished starting or loaded the file.
- The "Last opened" and "By" columns show `—` with an explanatory tooltip: the
  catalog has no per-file opening-history reader yet, and no value is invented.
- `begin_scan` clears finished job rows when the next scope starts, so the strip
  shows one scanning job rather than an unbounded history; per-scope results stay
  in the Projects pane's scan details.
- Settings editing, structure creation, incoming organization, file-type copy,
  backup, legacy import and operation history remain placeholders; their views
  stay visibly disabled.
