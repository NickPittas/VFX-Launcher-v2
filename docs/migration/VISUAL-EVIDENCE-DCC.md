# DCC workspace proposal: rendered evidence

2026-11-05. This records the current prototype after the user's DCC direction.
The first design review and V2 screenshots describe superseded proposals.

T3 html_preview rendered a bounded 800px product window at widths 1280 and
1440px. Embedding replaced 100vh with 800px, used transparent outer document
backgrounds and auto document height; product pane geometry remained bounded.
Images include the renderer's 8px gutter on each edge (816px capture height).
No browser server, test suite, build or production file operation was used.
All PNG bytes were saved directly inside this project.

Screenshots in artifacts/screenshots/:

- html-dcc-final-projects-1280.png: corrected menu/tab layout and equal-height
  Projects panes. Both pane bounds measured top 135.84px, bottom 808px.
- html-dcc-shots-1280.png: editable EP010 / EP020 sequences, per-sequence shot
  controls, checked destination folders, lower preview dock and pinned execution.
- html-dcc-final-incoming-1280.png: equal-height source/mapping panes, lower
  destination dock, compact frame/type rows and orange missing-frame warning.
  Pane bounds both measured top 197.08px, bottom 534px; execution strip measured
  top 769px, bottom 808px. Warning computed color rgb(237,196,124).
- html-dcc-destination-1440.png: navigated destination folder picker to
  /mnt/Projects/00199_Drifting_Plates, selected it, and observed updated path and
  preview root with Copy disabled pending a fresh check.
- html-dcc-copy-1440.png: Scan folders, same-page list, visible scan/copy tracks,
  compact Existing files selector and pinned bottom execution.
- html-dcc-backup-1440.png: accepted backup content retained in shared dock chrome.
- html-dcc-projects-1440.png and html-dcc-incoming-1280.png: intermediate captures
  before the final warning-color/measurement correction; prefer final images.

Exercised prototype interactions through existing DOM event handlers:

1. Typed EP010 into the first sequence; added and named EP020 as a second.
2. Added Hero_A specifically to the first sequence and Wide_B to the second.
3. Regenerated and checked the preview. It contained EP010_Hero_A and EP020_Wide_B
   in the proper paths, and Add to project became enabled.
4. Opened the Incoming destination picker, navigated Up, opened the second
   synthetic project and selected it. Destination input and tree root matched;
   execution was invalidated.

Parent corrections after render: old brand CSS stacked menu items vertically;
old sidebar padding/gap displaced workspace tabs; toolbar margin left a web-page
gap; the destination tree root was hardcoded; missing-frame warning color was
overridden by secondary text. These were corrected and affected views rendered
again. An initial measurement-only snippet assumed Projects had an action-strip
element, produced a null-element error, and was corrected; final measurements
completed without console errors. This was not a prototype initialization error.

Limits: all operations use synthetic data. No native egui or filesystem behavior
is verified. Full pointer-drag/keyboard/docking interaction, every menu/dialog,
all validation paths, other viewport heights/DPI and platform behavior remain
unverified. Application-menu words are presentational prototype chrome, not a
claim that a complete native menu system exists. This report is not user design
acceptance or a full feature-parity pass.
