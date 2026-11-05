# Desktop redesign evidence

2026-11-05. This evidence supersedes the first mockup captures listed in
VISUAL-EVIDENCE.md for the current UI proposal. UI-REVIEW.md reviews the previous
design and is historical; it is not a review of this redesign.

The HTML was rendered through T3 html_preview. All PNG bytes were written
directly under project artifacts/screenshots/. No external screenshot save path,
server, test suite, build or production file operation was used.

Embedding-only CSS replaces 100vh with 880px and makes the outer document
transparent; the product window stays opaque. Images are full-document captures,
not fixed-height/DPI or native egui verification.

| File under artifacts/screenshots/ | Width | Observed state |
| --- | --- | --- |
| html-v2-projects-1440.png | 1440 | Two panels; version then Launch; no catalog paths |
| html-v2-structure-1440.png | 1440 | Existing project additions; Nuke options; disabled Add before check |
| html-v2-incoming-1440.png | 1440 | Source, planned changes, destination; visible file-count column |
| html-v2-incoming-1280.png | 1280 | Destination below files; visible file-count column |
| html-v2-copy-1440.png | 1440 | File filters and operation list on same page |
| html-v2-backup-1440.png | 1440 | File list and prominent action; no metric cards |
| html-v2-settings-1440.png | 1440 | Compact application settings; logs accessible here |
| html-v2-new-project-1440.png | 1440 | New-project dialog from Projects |
| html-v2-backup-progress-1440.png | 1440 | Started simulated backup: scan/find/copy tracks, file list, Pause/Cancel |
| html-v2-shots-added-1280.png | 1280 | Add sequence, add shot, regenerate, check: Add to project enabled |

The parent inspected rendered images. Renderer calls returned no console errors.
Selected DOM event handlers were exercised inside rendering: navigation,
new-project dialog opening, sequence/shot addition, preview regeneration,
validation and backup start. This is feature-local HTML interaction evidence,
not native runtime verification or a global testing pass.

A source defect discovered during rendering was corrected: hidden tables were
initializing column widths before layout, producing overflow on first opening.
Tables now initialize resize handles only when visible, including after tab,
dialog and collapsible-section activation. The selection column retains its
compact width. Re-rendered Incoming measured table/available widths of 661/661
at 1440px and 789/789 at 1280px. All default non-Projects views were recaptured.

Remaining limits: full pointer-drag resizing, every keyboard/context-menu flow,
all dialog content, long-run state transitions, fixed-height scrolling, DPI,
platform behavior and native egui remain unverified. The prototype still uses
synthetic values and simulated actions. Earlier product/backend decisions stay
open as recorded in USER-UI-REQUIREMENTS.md and PLAN.md.
