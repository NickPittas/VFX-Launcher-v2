# Current implementation queue

**Latest integration:** original executor findings 2 and 4 are implemented from
the isolated source-safety packet: declared source root retained and identity
checked, descendant parents resolved beneath its handle, full source entry and
copied-handle stamps rechecked after removal intent. Unknown source disposition
records a full-byte post-install failure. Finding 3's rename shortcut remains
removed. Original findings 1, 5, 6, 7 and 8 remain open; the executor stays unwired.

**Current executor ledger (2026-11-06):** the earlier claim that all eight
findings were fixed is incorrect. Finding 3 is now corrected: `transfer_file`
always stages Move as a copy and uses the single `RemovingSource` path. Findings
1, 2, 4, 5, 6, 7 and 8 remain incomplete; see the numbered ledger in
IMPLEMENTATION-STATUS.md. Do not wire or run transfers yet. No commit was made.

**Current blocker:** the executor still lacks safe Replace backup lifecycle,
source-root confinement, held-entry cleanup identity, exact staging recovery
intent, ancestor-chain verification, and post-install failure accounting. Only
Finding 3 was completed in this turn. Formatting and offline compile checks are
pending; no runtime or filesystem operation was performed. The executor is not
wired to the UI and must not be described as safe or runtime-verified.

This is the short live queue; IMPLEMENTATION-STATUS.md retains packet history.

1. **X1a reviewed again; coordinator fixed the five remaining findings.**
   The latest review confirmed the journal/token design can support a separately
   race-safe executor after correcting three blockers. All five findings are now
   corrected and compile-checked: source-retained-only runs can finish Failed,
   extension-copy plans create deduplicated parents in order, shared sources with
   any Move are blocked, inspection checks terminal-state rules, and hardlink
   membership uses identity hash sets rather than a quadratic scan.
   Earlier findings were also
   fixed in `rust/src/transfers.rs` (plus one documented read-only accessor in
   `paths.rs`): resolved link/mount containment for both roots and every source,
   cross-item hard-link blocking on positive identity evidence, per-effect journal
   transition whitelists with reviewed-intent and ordinal verification, expected
   identity comparison that fails closed, blocked-prerequisite propagation,
   directory identity instead of mtime, native non-UTF-8 numbered names, and
   labeled unresolved spellings. Coordinator also completed committed-byte totals,
   tagged-envelope-only row path decoding, journal consistency checks, and refusal
   of unknown envelopes during interruption marking. See IMPLEMENTATION-STATUS.md.
   These last five small fixes have source/compile verification, not a new review
   or runtime pass.
2. **X1b executor implemented and its eight review findings corrected; review
   and runtime pass open.**
   `rust/src/transfers/executor/linux.rs` now executes validated plans:
   intent persisted before the first mutation, destination root/ancestor
   identities revalidated by handle, per-item source revalidation, staged
   no-clobber copy with metadata preservation and post-copy stability check,
   same-device `renameat2` move with `CopiedSourceRetained` fallback, and the
   single displacement/restore window for Replace/PreserveExisting. The
   correction packet adds: durability-first backup deletion, a full pre-delete
   source stamp recheck, staging cleanup on every failure path (including journal
   failure), install evidence after the folder sync, a synced new folder,
   terminal outcomes for items a cancellation never reached, and a per-item
   parent-folder identity proof. Compiles without warnings under the offline
   prepared CARGO_HOME and is rustfmt-clean; no database, catalog row, filesystem
   operation or test ran, so behavior is unverified. Still open: independent
   review of the correction packet, transfer concurrency (currently sequential),
   UI/app wiring, and the live transfer pass on disposable folders.
3. **Projects review items implemented, interactive verification open.** Each
   launch now keeps its own reply, worker handle, file and captured identity, so
   concurrent launches neither replace each other nor re-attribute access records;
   accepted launch/reveal outcomes and their `log_file_access` replies drain before
   the catalog is consumed, with no frame or close path joining a live worker; the
   tracked workers cover the spawn only, so closing never waits for the DCC; and
   favorite writes keep their replies, report failures and refresh only their own
   user's list. The approved Nuke / After Effects tabs and file filter sit above
   the files pane and filter grouped rows without merging buckets. Coordinator's
   earlier fixes stay intact. Compiles; behavior still needs a live pass.
4. **Linux folder picker fixed in-app and exercised live.** The portal
   `FileChooser` preference is `flea;gtk`, and the 2026-11-05 21:36:54 request
   logged Vulkan initialization but never a QML-loaded/response event, so the flea
   provider hung. `rust/Cargo.toml` now selects rfd's GTK3 backend on Linux
   (`default-features = false`, `features = ["gtk3"]`) with native backends on
   Windows/macOS, and the wrong comment promising a zenity fallback is replaced.
   No portal config, service or desktop setting was touched. The dialog still runs
   on one worker thread, GTK lives on rfd's own thread, one picker at a time is
   still enforced, and no timeout thread can leave a pending request. The binary
   now links `libgtk-3.so.0`, so packaging must ship GTK3 3.24 (pkg-config
   3.24.52 here). The coordinator opened the GTK picker, selected a disposable
   project, registered it, scanned it, inspected separate timeline/shot buckets,
   switched to After Effects and quit through File. Screenshots and limits are in
   NATIVE-PROJECTS-WORKFLOW.md. Native versions currently appear as separate rows;
   The approved version selector has since been implemented and exercised live:
   choosing 001 updates Information to comp_v001.nk; latest defaults and compact
   adjacent columns were observed. See that report's follow-up. Launch/reveal and
   the broader lifecycle cases remain unverified.
5. Continue remaining port: legacy import, settings editing, structure/Nuke
   generation, incoming patterns/routing, extension-copy UI, backup/scheduling,
   external AI harness and final packaging/integration.

No test suites until all features are implemented. No global tests before the
app is functional and verified. Continue bounded, feature-local checks only;
all work and artifacts remain inside this project.
