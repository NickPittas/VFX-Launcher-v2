# Current implementation queue

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
   or runtime pass. No executor work exists yet.
2. **X1b is next: establish native execution primitives before writing it.**
   The user's priority of validation and journal completion has been handled.
   X1b is: platform-safe staged copy/move that revalidates each reviewed item,
   commits without clobbering, records durable outcomes and cancellation, and
   retains the source when removal fails.
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
   restore the approved version selector before calling Projects UI complete.
5. Continue remaining port: legacy import, settings editing, structure/Nuke
   generation, incoming patterns/routing, extension-copy UI, backup/scheduling,
   external AI harness and final packaging/integration.

No test suites until all features are implemented. No global tests before the
app is functional and verified. Continue bounded, feature-local checks only;
all work and artifacts remain inside this project.
