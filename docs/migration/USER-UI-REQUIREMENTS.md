# User-directed desktop redesign

2026-11-05. These requirements supersede conflicting navigation, screen layout
and copy in PLAN.md, CONTRACTS.md, UI-DESIGN.md and the first HTML mockup.
Backend safety and migration obligations remain. This is a professional desktop
application prototype, not a dashboard or documentation page.

## Whole application

- Use task names people understand. Proposed navigation: Projects, Shots &
  Sequences, Organize Incoming, Copy by File Type, Back Up Projects; Settings at
  the bottom. Remove Activity as a primary page; keep progress, errors and recent
  results with the work they describe. Logs can live in Settings.
- No repeated page or panel titles where navigation/context already identifies
  the view. No heading-heavy layouts, bold everywhere, badges everywhere,
  decorative metric cards, instructional paragraphs or implementation jargon.
- Normal-weight readable text, restrained emphasis, clear group spacing and
  visibly distinct buttons, dropdowns, editable fields and resize handles.
- All columns and split panels must be user-resizable, with bounded minimums,
  keyboard-accessible resizing and persistent view sizes in the native app.
- Keep the principal execution action conspicuous and separate from secondary
  controls. Show clear current-stage progress for scanning, finding and copying,
  counts/current file and an inspectable file list. Keep pause/cancel where useful.
- Errors use plain-language summaries with collapsible details and a useful next
  action. Keep raw diagnostic text in those details, not primary product labels.
- Details and advanced settings appear when needed. Preserve useful behaviors;
  remove explanations of internal contracts from product flows.

## Projects

- Only two working panels: projects and the selected project's files.
- No project path in the catalog rows. Paths are available through tooltips and
  context-menu information. Show project names with normal type weight.
- New project creation belongs here, once, in a focused dialog/form. Keep naming,
  client/template options and optional sequences/shots/Nuke generation available
  without creating a duplicate project-creation page.
- Nuke/AE files use columns in this order: file, version, Launch, modified,
  last opened, by. Launch is adjacent to the version it launches.
- Reveal, copy path and information belong in a right-click context menu, with
  Shift+F10/Menu keyboard access. Remove duplicated toolbar/per-file action sets.

## Shots & Sequences (replaces Structure)

- Operates on an existing project only. Choose/inherit the current project.
- Direct Add sequence and Add shot actions, including standalone shots.
- Make the destination template clear, preview proposed additions, and provide
  an obvious Add to project execution action.
- Nuke script generation must have visible controls/action, fps, resolution,
  proxy and ACES options. A dropdown with no execution action is insufficient.
- Template/client administration can be a focused dialog, not another large page.

## Organize Incoming

- Source folder -> destination folder is the central model. Clearly distinguish
  the source tree, planned destination folders and proposed file changes.
- Replace unexplained routing-profile language with a clear folder-layout/rules
  choice and concise contextual help; show what selecting it actually changes.
- Remove the generic Item & plan review side panel and AI prose block. Put file
  edits/conflicts alongside the selected files and destinations.
- Optional AI uses simple actions such as Suggest folders and an optional
  instruction field. Connection status is concise. Provider/authentication and
  payload-contract explanations belong in Settings/developer documentation.
- One obvious Copy selected / Move selected action after review; no disguised
  action hidden among dropdowns. Retain exact sequence-member and conflict safety.

## Copy by File Type

- Say clearly: copies matching file types to another folder while keeping their
  folder structure and filenames.
- Source/destination, extension selection, file preview, progress and results are
  all on the same page with clear separation and collapsible sections.
- No Exact operation review / Latest run report tabs; they must not hide the
  running work or force navigation to see results.

## Back Up Projects

- Remove the four top metric widgets; use clear file counts near the relevant
  list/progress only if useful (e.g. 4 files to copy, 26 already up to date).
- Restore phase progress bars and file lists for scanning, finding and copying.
- Keep source/destination, exclusions and scheduling understandable; advanced
  worker/conflict settings may be collapsed. Prominent Back up now action.
- Explain failures in plain language, with collapsible technical details.

## Design process

- Apply the available visualize skill's relevant design principles: restrained
  typography, progressive disclosure, minimal redundant chrome, clear native
  control semantics and accessible feedback. Its in-conversation fragment rules
  do not convert this task-owned desktop HTML prototype into a website deliverable.
- First redesign HTML, then inspect actual screenshots at desktop sizes. Keep
  every artifact inside this project. Do not claim native egui verification.
- No tests/test suites or global testing. Continue hard 10–30 second command caps.

## Confirmed model correction

The user's working Pi implementation model is
`qwen-token-plan/deepseek-v4.1-flash-1017` (DeepSeek V4.1 Flash 1017).
The failed `deepseek/deepseek-flash` endpoint is not the requested working model.

## Follow-up corrections after the second rendered prototype

These corrections override earlier ambiguity about action placement.

- The first-step action is consistently **Scan folders**, blue, at the top of
  Projects, Organize Incoming and Copy by File Type. Search text fields remain
  filters; do not use Find/Search/Scan as competing names for folder scanning.
- Final execution controls belong in a pinned, always-visible bottom bar.
  Launch stays beside Version as previously requested; opening New project is
  a top-level action whose actual create button is inside its dialog.
- Adjacent Projects panels must share top and bottom edges. Incoming source and
  planned-file panels must also have matching heights. Use internal scrolling.
- Shots & Sequences must preserve the actual original editor: checkable existing
  destination-folder tree, typed/editable sequence names, per-sequence Add shot,
  editable shot names, standalone shots, per-item removal and Nuke settings.
  Automatic numeric append with no naming/sequence choice is not parity.
- Read original `gitclones/FolderStructurer/UI/Add_to_Existing_Project/
  Add_to_Existing_Project.html` (particularly renderFolderTreeWithCheckboxes,
  addExistingSequence, addExistingShot, updateExistingPreview and
  createExistingFolders) and `widgets/shot_sequence_widgets.py` before editing.
- Sequence/shot Add controls live inside the editing/template panel. Both this
  page and Organize Incoming put destination preview below the working editors,
  at all supported desktop widths, not beside them on wider screens.
- Incoming destination selection must support actual prototype browsing and
  folder navigation, not merely focus/select the path input. The native version
  will use real filesystem browsing; the HTML uses navigable synthetic folders.
- Remove bulky row prose such as single camera file, actual members, no gaps.
  Use **frames** for sequence counts, concise file-type distinctions, and show
  missing-frame warnings only when a gap exists, in orange/red. Details expand
  on demand, rather than filling every row.
- Copy by File Type must have visible scanning/copy progress bars and a file
  list on the same page. Move conflict policy to a short labelled control in the
  settings area, not a full sentence preceding its execution buttons.
- Preserve Back Up Projects layout and behavior, which the user accepted.

User feedback image is saved inside the project at
`artifacts/screenshots/user-incoming-feedback.png`.

## Authoritative DCC desktop direction

The user explicitly rejects the web-page/dashboard composition, not merely its
colors or copy. Design masters named by the user: Nuke, Maya, Resolve, 3ds Studio,
After Effects, Blender, Cinema4D, Notch, Unread. Treat this as a native professional
production utility. HTML is only the temporary medium for mocking the egui UI.

- Compose a bounded desktop window with menu bar, compact workspace/tool strip,
  docked working panes, panel tabs and a persistent bottom action/status strip.
- Pane borders are structural square separators and draggable splitters; remove
  rounded cards, repeated section shells, page gutters and large page headings.
- Working trees/tables fill the available height, with internal scrolling.
  Adjacent panes share boundaries. Preview is a lower dock with a vertical resize
  handle; the overall content must not become a vertically scrolling web page.
- Use restrained neutral gray surfaces, normal-weight small desktop typography,
  compact controls, clear native focus/selection, and one restrained accent for
  the top Scan folders action and final execution controls.
- Use toolbar groups, context menus, collapsible properties and compact dialogs.
  No marketing layout, explanatory callout cards, dashboard metrics, decorative
  status badges or large blank areas below short floating content cards.
- Keep the previously specified behavior and bottom execution/top scan rules.
  Backup's accepted content and behavior remain; its surrounding application
  chrome may adopt the same DCC dock styling for consistency.
- Match the referenced applications' spatial grammar, not their unrelated
  features. Do not add a node graph, timeline, renderer or media browser.
