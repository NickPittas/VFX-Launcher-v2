# Implementation status

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
