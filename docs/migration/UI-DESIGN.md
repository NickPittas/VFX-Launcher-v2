# VFX Launcher desktop UI design

**Companion prototype:** [`ui-mockup.html`](ui-mockup.html)  
**Design baseline:** 2026-11-05, `migration/rust-egui-suite`  
**Status:** source-derived desktop interaction specification for the Rust + egui port. The prototype uses fictitious data and simulated actions; it is not native-runtime or legacy-parity evidence.

Earlier desktop evidence is recorded in
[VISUAL-EVIDENCE-V2.md](VISUAL-EVIDENCE-V2.md). The follow-up corrections below
are source/readback changes only: this revised composition has not been visually
checked or interactively exercised in this task. Native verification remains pending.

`USER-UI-REQUIREMENTS.md` takes precedence over earlier navigation, layout and visible copy. Backend rules in `PLAN.md` and `CONTRACTS.md` still apply, but implementation terminology does not belong in ordinary product screens.

## 1. Product shape

VFX Launcher is a professional desktop utility, not a website, analytics
dashboard or documentation browser. A compact horizontal workspace-tab row
contains exactly these primary destinations:

1. **Projects**
2. **Shots & Sequences**
3. **Organize Incoming**
4. **Copy by File Type**
5. **Back Up Projects**
6. **Settings**, grouped with the utility actions at the trailing edge

There is no primary Activity view. Progress, current files, results and recoverable errors stay with the operation that produced them. Settings contains diagnostic logs and collapsed prototype controls.

New project creation occurs only in the Projects dialog. Shots & Sequences operates on the current existing project and never offers a second project-creation workflow.

### Product principles

- Use familiar task names and plain-language outcomes.
- Give each view one obvious execution action, visually separated from editing and review controls.
- Do not repeat the navigation label as a page heading or repeat the same heading in nested panels.
- Use normal `400` body type and restrained `500` emphasis only. Avoid all-bold interfaces, decorative KPI cards, badge stacks and oversized icons.
- Show advanced, legacy and administrative behavior through focused dialogs, context menus and collapsible sections.
- Keep paths discoverable without making every row a path listing.
- A draft edit never silently preserves an earlier approval.
- Progress identifies the current stage, completed/total count and current file. A file list remains inspectable during and after the run.
- Cancellation and partial completion are never presented as success.

## 2. Desktop shell and visual system

### Shell

The shell is one bounded desktop window with a compact application menu row, a
narrow workspace-tab row, a compact global project picker and a docked work area.
There is no wide branded navigation sidebar, page heading stack or outer webpage
scroll. The approved labels remain: Projects, Shots & Sequences, Organize
Incoming, Copy by File Type, Back Up Projects and Settings.

The visual composition follows the inspected Maya Classic full-interface and
Nuke pane-menu references documented in `DCC-REFERENCES.md`: continuous tiled
workspaces, slim pane headers/tool rows, rectangular controls, thin square
splitters, dense trees/tables and bottom status/execution strips. These are
historical/interface-detail references, not current-version QA or a requirement
to invent a timeline or node graph.

The current-project picker is shared by Projects, Shots & Sequences and
project-scoped operations. Changing it preserves unrelated view drafts. The
attribution user and prototype status are shown quietly in the application
chrome; identity is not presented as authentication.

Working dock panes fill available window height and scroll internally. Shots &
Sequences and Organize Incoming always place their destination preview in a
bottom dock at supported desktop widths, separated by a vertical-resize handle.
Final execution controls are outside the scrolling/working region in a pinned
bottom strip. Backup retains its accepted content and behavior within the shared
rectangular dock chrome.

### Visual tokens

| Role | Prototype value | Native intent |
|---|---|---|
| Window | `#101419` | Central window background |
| Surface | `#171d24` | Panels and dialogs |
| Raised/control surface | `#1d2530` / `#222c37` | Inputs, buttons, hovered controls |
| Selected surface | `#203c56` | Current navigation item and selected row |
| Border | `#34404c` | Structural separators |
| Strong border | `#596979` | Editable controls and menu outlines |
| Primary text | `#edf1f5` | Essential content |
| Secondary text | `#b2bdc9` | Labels and supporting context |
| Quiet text | `#8f9ba8` | Nonessential annotations |
| Accent | `#83b7ef` | Links and selected affordances |
| Primary action | `#397fb7` | The single conspicuous execution button |
| Keyboard focus | `#ffd166` | Visible 2 px focus outline |
| Success | `#82d6aa` | Paired with explicit text |
| Warning | `#edc47c` | Paired with explicit text |
| Error | `#f2a0a0` | Paired with explicit text |

Use the native system font stack. Monospace is limited to paths, filenames, file patterns, commands and logs. Sentence-case table headers use normal weight. Status never depends on color alone.

Controls retain recognizable desktop semantics:

- Filled, outlined or quiet buttons are visually distinct.
- Editable fields have an interior fill and strong border.
- Dropdowns are native `select` equivalents in egui.
- Checkboxes and radio labels include a larger clickable target than the glyph.
- Split handles and column grips are visible on hover/focus and have keyboard behavior.
- Tables use quiet horizontal separators rather than heavy grid lines or card-like rows.

## 3. Shared interaction model

### Review and execution

Operation state follows:

```text
Draft → current preview reviewed → Validate → Ready → Execute → Result
```

The visible prototype uses human labels such as **Check additions** and **Validate selection**. Ready always names the exact reviewed content or count. An execution button remains unavailable until its current revision is Ready.

| State | Display | Available actions |
|---|---|---|
| Draft | What needs review and why | Edit, scan, preview, validate |
| Preview stale | “Regenerate the preview after this change” | Regenerate, then validate; no execution |
| Validation problem | Affected file/field and useful next action | Correct, remove or reselect; no execution |
| Ready | Exact selected count and conflict choice | Execute or edit; editing returns to Draft |
| Running | Current stage, current file, completed/total | Pause where useful, Cancel; source/destination/rules/selection locked |
| Paused | Safe-boundary acknowledgement | Resume or Cancel; plan remains locked |
| Cancelling | Cooperative stop in progress | Wait; no second run |
| Complete | Copied/moved/created/skipped counts | Inspect the file list and result details |
| Partial | Completed work plus explicit failed/blocked work | Inspect the affected file and error; start a fresh run after correction |
| Cancelled | Completed, skipped and unfinished counts | Inspect completed work; no rollback or success claim |
| Stale | “Changed since review” | Review current inputs and validate again |

An active job disables its associated fieldsets and execution controls, not merely a global banner. Terminal Cancelled/Partial results require a new review; an old Run button does not silently reuse the same approval.

### Operation progress and results

Incoming, file-type copy and backup each own their progress and report area on the same page. No review/report tabs or separate Activity navigation are required to understand a run.

A progress area contains:

- stage label;
- completed and total counts;
- bounded progress bar;
- current filename or source-relative path;
- file list with per-item state;
- Pause/Resume and Cancel where useful;
- collapsible result and technical details.

Plain-language summaries come first. Raw diagnostics appear only inside an expanded details section.

### Split panels and table columns

All split handles are actual controls, not decorative lines:

- Pointer drag changes the adjacent panel size.
- `ArrowLeft`/`ArrowRight` resize by a small increment.
- `Shift+ArrowLeft`/`Shift+ArrowRight` resize faster.
- `Home`/`End` choose bounded minimum/maximum sizes for major splitters.
- `aria-orientation="vertical"`, `aria-valuenow`, and minimum/maximum values expose separator behavior.

Every table header has a keyboard-focusable column grip with the same arrow-key convention. Columns have bounded minimums so content and controls cannot disappear completely.

The native app persists panel widths, column widths, selected project, active file type and relevant view state. Restored dimensions are clamped to the current available desktop viewport so controls remain reachable.

Incoming source and planned-file panes share equal top/bottom edges. Their
destination tree is always in the lower dock, including at 1280 and 1440 widths;
it is not promoted to a third side column on wide screens. Shots & Sequences uses
the same lower-dock rule. Horizontal splitters use Left/Right; lower-dock
splitters use Up/Down, with Shift acceleration and Home/End bounds. Other working
splits stack only when their minimum usable widths cannot coexist. Tables contain
their own horizontal overflow rather than clipping the application.

## 4. Projects

### Two working panels

Projects has only:

1. **Project list** — All/Favorites/Recent tabs, search, project name, file count and availability.
2. **Selected project files** — Nuke/After Effects tabs, search and the file table.

Project rows never display their folder path. A full path is available through the row tooltip and the right-click menu. Project names use normal text weight. Offline records remain visible without implying that their cached file list is fresh.

The top toolbar starts with the blue **Scan folders** action, followed by New
project and Add existing folder; project search is a text filter only. It contains
no repeated path/action toolbar for individual files. The two working panes share
the same top and bottom edges and use internal table scrolling.

### File table and versions

Nuke and After Effects tables use this exact column order:

```text
File | Version | Launch | Modified | Last opened | By
```

Version is a dropdown. Launch is immediately adjacent to that version and acts on the selected value. In the prototype, choosing a version updates the selected-file message; Launch updates Last opened and identifies the simulated launch. Native launch passes the selected full path directly to the configured application without shell interpolation.

**Reveal, Copy path and File/Project information exist only in the right-click context menu**, opened with the pointer or `Shift+F10`/Menu on the focused row. There are no duplicated per-file Reveal/Copy/Info buttons or global shortcuts that bypass this model.

Copy path opens a selectable full-path view in the offline prototype. Native implementation uses the platform clipboard and reports clipboard errors without truncating the value. Unregister is a project context action and explicitly states that only launcher membership is removed; project folders are not deleted.

### New project dialog

New project is a focused modal with:

- destination parent;
- project number;
- client with searchable history;
- project name;
- folder template;
- one live full-folder preview;
- direct Add sequence, Add shot and Add standalone shot controls;
- optional Nuke generation;
- fps, resolution, proxy and ACES settings;
- an actual **Add project** dialog action in a pinned dialog footer, outside the
  dialog's scrolling form body.

Adding a project updates the catalog, current-project picker and selection. Nuke options are disabled when generation is unchecked. Existing projects are not edited by this dialog.

## 5. Shots & Sequences

This view adds content to the current existing project. It cannot create another
project root. Templates & clients remains a focused administration dialog.

### Docked editor

The upper working row contains a checkable **Existing project folders** tree and
the **Sequences and shots** editor. Add sequence and Add shot controls live inside
that editor, not in the application toolbar. The destination template and Nuke
settings are below the working editors, inside the bottom **Destination preview**
dock at all supported desktop widths. A keyboard-resizable horizontal separator
changes the lower dock height.

### Original-source behavior restored

These source files were read before the correction:

- `gitclones/FolderStructurer/UI/Add_to_Existing_Project/Add_to_Existing_Project.html`;
- existing-state helpers in `gitclones/FolderStructurer/UI/assets/app.js`;
- `gitclones/FolderStructurer/widgets/shot_sequence_widgets.py`;
- `gitclones/FolderStructurer/tabs/add_to_existing_project_tab.py`.

`renderFolderTreeWithCheckboxes` gives every existing folder a checkbox and
relative `data-path`. `addExistingSequence` creates an editable sequence input,
Remove and a per-sequence Add shot control; `addExistingShot(seqId)` inserts an
editable, removable shot into that particular sequence. Separate orphan-shot
functions provide editable standalone shots. `updateExistingPreview` and
`createExistingFolders` gather the same checked folders, typed sequences/shots,
standalone shots, FPS, width/height, proxy, ACES, Nuke-generation and
parent-folder-as-sequence options. Creation requires a project, at least one
checked destination and actual additions.

The Python widgets confirm live text edits and per-item insertion/removal.
`AddToExistingProjectTab.on_execute` uses `exist_ok=True` for a sequence folder,
so a typed shot can target an existing sequence; an existing shot is reported
and skipped rather than overwritten.

The mock restores those contracts:

- every existing destination folder can be checked independently;
- every sequence and shot name is typable and renameable;
- each sequence has its own Add shot;
- new sequence/shot inputs start blank rather than forcing synthetic names;
- standalone shots have their own Add shot and per-item Remove;
- the preview repeats the complete typed additions under every checked folder.

### Nuke settings and review

Visible controls include template, FPS, resolution presets and Custom width /
height, Nuke-script generation, proxy, ACES and **Parent folder as sequence in
filename**. Generated `.nk` names update in the preview. Disabling Nuke
generation disables its dependent options.

Existing `SEQ010/SH010` and its script remain visible and unchanged. Proposed
folders and scripts are marked as new; standalone shots are labelled standalone.
Any edit invalidates the previous review. **Regenerate preview** and **Check
additions** are separate required steps. **Add to project** stays disabled until
the current preview is checked and remains in the pinned bottom execution strip.
The simulated job locks edits while running and never mutates a real project.

Template ordering has Move up/Move down buttons in addition to any future drag
support. Client editing preserves Unicode names.

## 6. Organize Incoming

### Source to destination

The central model is visible in one row:

```text
Source folder → Destination folder → Folder layout
```

The layout choices have meaningful names:

- **Shot / task / version**
- **Department / shot / task**
- **One folder for all files**

“How does this work?” opens a concise rules dialog. It explains that renders go beside their shot and camera originals go under Video/Footage. The filename, not the parent folder name, determines task extraction. Ordered destination/keyword rules have Move up/down controls; raw filename patterns are collapsed.

Changing source, destination, layout or rules invalidates Ready and recomputes
proposed destinations. The blue top **Scan folders** action reports scanned
counts and returns to a reviewable Draft. Search inputs are text filters only.

### Working regions

The upper split contains equal-height, internally scrolling **Source** and
**Files and destinations** panes. **Destination preview** is always in the lower
dock at 1280, 1440 and other supported desktop widths, with a thin vertical
splitter. Existing content and planned additions share that tree. There is no
third wide-screen column or generic Item & plan inspector.

Source and destination Browse buttons open a navigable synthetic folder browser
with Up, Open, breadcrumbs, an editable path and Select folder. Choosing a folder
updates the path and invalidates validation. Native implementation will use real
filesystem browsing.

Source selection filters the visible list only. Rows use concise EXR/MXF type
and count labels. The beauty row shows an orange missing-frame warning only
because a gap exists. Exact frame filenames and the existing-file editor expand
on demand. Selecting a row chooses its contextual editor; the conflict policy
stays beside the affected destination and updates the preview.

### Coherent Incoming fixture

Default selection is:

| Item | Count and type | Destination |
|---|---:|---|
| `SH010_beauty_v003.####.exr` | EXR sequence · 101 frames; missing 1002, 1048, 1099 | `04_vfx/SH010/beauty/v003` |
| `A012C003_261105.mxf` | MXF video · 1 file · 2.8 GB | `Video/Footage/ARRI` |
| `SH010_cryptomatte_v003.####.exr` | EXR sequence · 100 frames | `04_vfx/SH010/cryptomatte/v003` |
| **Total** | **202 files in 3 selected items** | |

The one existing destination is exactly:

```text
04_vfx/SH010/beauty/v003/SH010_beauty_v003.1001.exr
```

That same member appears in the source sequence review, destination tree, conflict summary and run report. Default Skip produces **201 copies + 1 skip** when all 202 are selected. Keep both or Replace changes that derived count after a new validation. Deselecting an item updates item/member totals everywhere in this view.

Copy and Move are radio choices in the pinned bottom execution strip, with one
button whose label follows the choice. No second transfer action is hidden in a
menu. Running locks all edits. Pause/Resume and Cancel act on the visible
simulated run; cancellation reports completed/skipped/not-started counts without
calling the run successful. Progress, current file and collapsible run details
remain with the destination dock on the same page.

### Optional folder suggestions

A collapsed **Suggest folders** control contains one optional instruction and one Suggest folders action. Its concise status is “assistant disconnected.” No provider, login, API-key, payload or authentication explanation appears here.

The prototype applies a local folder-layout suggestion to the draft and explicitly requires review/validation. It makes no model, filesystem or network request. A native model adapter may propose draft destinations only; it cannot validate or execute them.

## 7. Copy by File Type

The only purpose line is:

> Copy selected file types and keep folders and filenames.

The top toolbar starts with blue **Scan folders**. Source, destination, extension
checkboxes, optional custom extension and the compact **Existing files** policy
share one setup area. Folder-scan and copy progress bars are both visible while
idle, labelled Ready/Waiting as appropriate, and update during their operations.

The collapsible file list, progress and collapsible results remain on the same
page. Validation is a distinct required action; Copy selected is pinned in the
bottom execution strip. There are no Exact review or Latest report tabs and no
competing Find files or Scan source label.

### Shared conflict fixture

The preview contains four selected files when `.nk`, `.autosave`, `.aep` and `.exr` are enabled. One is the same actual Incoming destination conflict:

```text
04_vfx/SH010/beauty/v003/SH010_beauty_v003.1001.exr
```

Default Skip therefore derives **4 selected · 3 will copy · 1 will be skipped**. Destination cells are source-relative and preserve both folder and filename. This file-type filter does not inherit project-scan autosave exclusions.

Changing an extension, custom suffix, file selection, source, destination or conflict policy invalidates Ready. Validate selection binds the current count and conflict choice. Copy selected updates each row to Copied or Skipped and keeps the result details below the progress area.

Cancellation shows copied/skipped/not-started counts and states that completed copies remain at the destination. It never displays a success result for unfinished files.

## 8. Back Up Projects

### Understandable setup

The default form contains only:

- project folder;
- backup folder;
- excluded names/folders;
- optional schedule while the GUI is open;
- **Back up now** as the conspicuous execution action.

Advanced options contain copies-at-a-time, existing-newer-file behavior and legacy job Import/Export. They do not compete with Back up now.

No four-metric dashboard row exists. Counts sit beside the relevant file list:

```text
4 to copy · 26 already up to date · 1 needs attention
```

The up-to-date group is collapsed. The file needing attention includes a plain-language summary and a Why disclosure beside its row.

### Three visible phases

One run area shows three simultaneous stage tracks:

1. **Scan folders** — 31 files discovered.
2. **Find changes** — 31 comparisons completed.
3. **Copy files** — 4 changed files processed.

The current file remains visible below the tracks. File rows update from pending to copied. The default completed result is **Partial: 4 copied, 26 already up to date, 1 needs attention**. The blocked file is not counted as copied.

Technical details explain that `SH090_comp_v002.nk` file information could not be read and suggest checking access before a fresh run. Cancellation during scanning/finding/copying does not claim backup success and preserves completed-copy reporting.

The backup remains additive/version-preserving. Existing destinations use the `1_name.ext` convention by default; no mirror-delete action is introduced.

## 9. Settings

Settings is the final navigation destination, not an operational dashboard.

### Applications

Nuke and After Effects application paths, plus a collapsed custom Nuke launch command, are editable here. Save updates visible prototype state. Native command arguments are passed directly without shell expansion.

### Project discovery

Folder names and the default project folder remain accessible. Legacy import is collapsed and describes a review-before-replacement flow rather than dumping schema or database details into the screen.

### Users

Users supports Add, Rename, Use for history and Delete. The selected name is used for favorites and file history. Deletion is unavailable for the sole/current selected user in the mock, and the persistent note states that earlier file history is retained. These are attribution controls, not sign-in or permission controls.

### Logs

Logs provide level filtering and Clear this view. Clearing affects display only and does not erase run results. The log fixture uses the same Incoming result: **201 copied, 1 skipped**, naming the shared EXR conflict.

### Prototype controls

Collapsed controls can reset the demo, show a stale Incoming review or show a partial backup result. They are outside normal product workflows and change only in-memory mock state. Production does not include them.

## 10. Keyboard, focus and accessible feedback

### Global keys

| Input | Behavior |
|---|---|
| Alt+1 … Alt+6 | Open Projects, Shots & Sequences, Organize Incoming, Copy by File Type, Back Up Projects or Settings |
| Ctrl+K | Focus the current view’s search field when one exists |
| F6 / Shift+F6 | Cycle current navigation, project picker and work region |
| Tab / Shift+Tab | Follow visual order without trapping focus in tables or trees |
| Escape | Close the topmost menu/dialog; otherwise clear a focused search |
| ? | Open the shortcut dialog when not typing in a field |

Global shortcuts never start a copy, move, project addition or backup. Text editing takes precedence over nonessential global keys.

### Component keys

| Component | Required behavior |
|---|---|
| Tabs | Arrow keys move and activate; Home/End choose first/last; roving tabindex keeps one tab stop |
| Context menus | Right-click or Shift+F10/Menu opens at the selected row; arrows navigate; Home/End jump; Escape closes and restores row focus |
| Trees | Buttons provide keyboard selection; selected state uses `aria-pressed` where appropriate |
| Splitters | Arrow keys resize; Shift accelerates; major splitters support Home/End bounds |
| Column grips | Arrow keys resize; Shift accelerates |
| Ordered rules | Move up/down buttons provide a keyboard equivalent to dragging |
| Dialogs | Native modal focus containment; initial focus enters the dialog; close restores the invoker |
| Checkboxes/radios | Space toggles the focused native control and never executes a plan |

Each view has a programmatic focus target on navigation. Dynamic selections and concise results use `role="status"`/`aria-live="polite"`; validation problems use explicit visible text. Progress bars have accessible names, min/max/current values and an adjacent textual count.

Toasts are not the sole evidence of an action. Navigation, selections, generated previews, validation state, progress, run outcomes, user edits and settings all update persistent on-page UI. Focus is not stolen on progress ticks or terminal events.

## 11. Native egui translation

- Map primary navigation to a bounded `SidePanel` and dispatch one active central view at a time.
- Use native-style `ComboBox`, `TextEdit`, checkbox/radio and button widgets; avoid web-like cards and hero typography.
- Use modal `Window` or platform dialogs with explicit focus restoration. Native OS dialogs remain appropriate for file/folder picking.
- Use `ScrollArea::show_rows` or another bounded/virtualized approach for exact sequence members and large file lists. Do not instantiate thousands of expanded tree widgets per frame.
- Render from typed application/job state. Feature workers own discovery and operations; the frame callback performs no filesystem walk, SQL work or transfer.
- Worker events update current phase, current path, counts and terminal item states. Coalesce progress without losing terminal outcomes.
- Use stable project/file/item IDs rather than displayed paths as selection identity.
- Persist panel/column sizes in versioned UI state and clamp restored values to the usable native viewport.
- Keep complete native paths for reveal, clipboard and information actions. Ellipsized cell text never becomes the copied or launched value.
- Reuse operation progress/report presentation across Incoming, file-type copy and backup without merging their discovery rules or execution semantics.

The HTML is self-contained and offline, with inline CSS/JavaScript, system fonts, no external assets, no browser storage, no network access and no real file/process/model calls. Its interactive state demonstrates product behavior only; backend validation and native side effects remain implementation responsibilities rather than visible product copy.
