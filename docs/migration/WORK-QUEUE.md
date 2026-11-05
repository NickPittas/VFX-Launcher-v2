# Current implementation queue

This is the short live queue; IMPLEMENTATION-STATUS.md retains packet history.

1. **X1a foundation implemented, awaiting review.** `rust/src/transfers.rs` now
   has extension-copy planning with preserved source root/evidence, read-only
   validation (concrete destinations, all four conflict policies, duplicate and
   source/dest-chain rejection, stamp/kind/absence expectations), lossless
   tagged path envelopes, validated-intent persistence and the DB-only journal
   (start, item outcomes, finalization, interrupted-run marking, inspection).
   Compile-checked only; see IMPLEMENTATION-STATUS.md. Review this before X1b.
2. **Then X1b:** platform-safe staged copy/move execution that revalidates each
   reviewed item, commits without clobbering, records durable outcomes and
   cancellation, and retains the source when removal fails.
3. **Projects review still open:** capture initiating identity for each launch;
   retain concurrent launch replies; coordinate launch/reveal outcomes and
   access logging before catalog shutdown; report favorite-write failures;
   Add the approved file search and Nuke/After Effects tabs. Coordinator fixed
   the cancellation gap after root inspection, stale rows on user change,
   signed timestamp formatting in rows/info, Menu-key context access and
   empty-state guidance contrast. These source changes compile; interactive
   behavior remains to be verified.
4. **Live Projects verification:** input automation is now fixed across all
   three screens (DESKTOP-AUTOMATION.md). Native folder-picker diagnosis is
   complete: the portal `FileChooser` preference is `flea;gtk`; the flea picker
   request at 2026-11-05 21:36:54 logged Vulkan initialization but never a
   QML-loaded/response event, unlike earlier successful starts, so the flea
   provider hung; the Hyprland screenshot-portal crash is unrelated. Config and
   journal were verified and pkg-config GTK3 3.24.52 is available. The next
   separate, app-scoped fix is to select rfd's GTK3 backend on Linux with no
   global portal/config/service changes, correct the Cargo comment that wrongly
   promises a zenity fallback on a portal hang, and avoid nested timeout threads
   that leave an infinite chooser request or permit duplicate picker launches;
   then verify the live picker and registration/scanning/grouping/version
   selection with disposable data. Not part of X1a and not implemented here.
5. Continue remaining port: legacy import, settings editing, structure/Nuke
   generation, incoming patterns/routing, extension-copy UI, backup/scheduling,
   external AI harness and final packaging/integration.

No test suites until all features are implemented. No global tests before the
app is functional and verified. Continue bounded, feature-local checks only;
all work and artifacts remain inside this project.
