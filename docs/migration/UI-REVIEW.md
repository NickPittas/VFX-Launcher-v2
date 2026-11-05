# HTML UI proposal review

**Date:** 2026-11-05  
**Verdict:** Changes required before this proposal is used as the Rust UI baseline.

## Evidence and limits

Reviewed `PLAN.md`, `CONTRACTS.md`, `UI-DESIGN.md`, `VISUAL-EVIDENCE.md`, and `ui-mockup.html`. Inspected all eight listed PNGs: Projects, Structure, Incoming 1600, Transfers, Backup, Settings, Activity, and Incoming 1280.

This is a static visual/source review. No interactions were tested and no Rust/runtime, backend, fixed-height scrolling, or DPI behavior is claimed. The captures are full-document renders with the embed-only CSS changes recorded in `VISUAL-EVIDENCE.md`; simulated HTML actions are not product functionality.

## High severity

### 1. Incoming selection, conflict, and exact plan describe different operations

**Locations:** `ui-mockup.html:357–366`, `:380`, `:505`; `html-incoming-1600.png`, `html-incoming-1280.png`, `html-transfers-1600.png`.

- Both Incoming conflict captures name `Tidal_Promo_03.aep` in the job dock, but that file is absent from the Incoming mapping table. It belongs to the Extension copy fixture.
- “Plan #184 · Incoming copy” includes that AEP with a `/mnt/Transfer/Northwind_scripts/...` destination, mixing an Extension copy item into an Incoming plan.
- Incoming says three selected items contain 202 exact files: beauty 101 + camera 1 + cryptomatte 100. Plan #184 shows beauty and camera but omits cryptomatte while adding the unrelated AEP.
- The scan summary says 126 files although the two visible sequences alone contain 201 actual members.

A user cannot determine which sources, destinations, or conflict they are approving. Use one coherent fixture shared by scan summary, selection, plan review, and dock. Keep the AEP only in Extension copy; make the Incoming conflict an actual selected Incoming destination. Either include all 202 selected members in the Incoming plan or change the selection and every derived total together.

### 2. Execution and editing actions are not gated by the displayed state

**Locations:** `ui-mockup.html:307`, `:375`, `:380`, `:517–534`; `UI-DESIGN.md:228–239`; `html-structure-1600.png`, `html-transfers-1600.png`.

Structure visibly says **Draft · unsaved** while **Create reviewed items** remains enabled, contrary to “Draft … no execution yet.” `applyScenario()` changes banners, empty views, and dock controls only; it does not gate Create, Validate & start, or plan editors for stale, conflict, running, or terminal states. Transfers also exposes **Validate & start reviewed copy** from Extension copy rather than requiring the exact-review/ready state.

Implement one explicit action-state matrix in the proposal: Draft permits preview/validate only; Start requires the exact reviewed revision to be Ready; stale/invalid/conflicting plans block Start with a nearby reason; Running locks plan edits; Partial/Cancelled/Failed offer a fresh plan rather than reuse. Do not rely on a toast after activation to supply the gate.

### 3. Run outcomes and counts contradict one another

**Locations:** `ui-mockup.html:381`, `:414–415`, `:511`; `html-transfers-1600.png`, `html-activity-1600.png`; `UI-DESIGN.md:193`, `:239`.

Run #183’s item report says the beauty sequence has **3 cancelled**, while its metrics, Activity summary, dock, and diagnostic log contain no cancelled count (`cancelled=0`). Its two skipped items are not itemized. Activity run #180 has 38 directories created and two scripts failed but is labelled **Failed**, whereas run #183 uses **Partial** for completed work plus failure.

These inconsistencies obscure what completed, what remains at the source, and what needs recovery. Drive every report view and log from one `RunReport` fixture with reconciled counts and item rows for every terminal state. Use Partial when completed items coexist with failures; reserve Failed for a run with no successful work. Keep `CopiedSourceRetained` distinct from Moved.

### 4. Incoming is still cramped at 1280 px and clips source identities

**Locations:** `ui-mockup.html:107`, `:215–218`, `:359–366`; `UI-DESIGN.md:168`; `html-incoming-1280.png` versus `html-incoming-1600.png`.

Confirmed. The three-column rule remains active at 1280 because stacking starts only at a 1240 px viewport, without accounting for the sidebar and content padding. The 1280 capture shows clipped source filenames, a severely wrapped inspector, and the mapping State column pushed out of the visible table area. Source-tree entries are plain spans with no full-path action, so the clipped names have no proposed recovery path.

Switch using available central-content width (a container query, or a viewport breakpoint that explicitly includes the sidebar) so the inspector is below the mapping table at 1280. Give every source row a keyboard-accessible selection and full-path/copy action; do not depend on clipped text or hover.

## Medium severity

### 5. The required Incoming target tree is missing

**Locations:** `PLAN.md:92`; `UI-DESIGN.md:168`; `ui-mockup.html:357–369`.

The plan requires source **and target** trees. The design and mockup provide only a source tree plus destination cells. This removes the contextual view of existing target folders and conflicts from Incoming review. Add a destination/target tree linked to the selected mapping and exact conflict, and reconcile `UI-DESIGN.md` with `PLAN.md`. This does not require media-browser scope.

### 6. Users & attribution omits selection and deletion required by CRUD parity

**Locations:** `PLAN.md:36`; `UI-DESIGN.md:215`; `ui-mockup.html:404` (source-only; this tab was not captured).

The panel offers Add and Edit only. Although it explains identity selection and deletion, there is no action to select another user for attribution or delete a user. Add **Use for attribution/Select** and **Delete** actions, with deletion confirmation describing retained/tombstoned historical attribution. Preserve the explicit “not authentication” boundary.

### 7. Advertised keyboard behavior is incomplete in the proposal source

**Locations:** `ui-mockup.html:359`, `:436`, `:442`, `:465–474`, `:483–498`, `:584–593`; `UI-DESIGN.md:272–296`.

Source inspection, not interaction testing, shows:

- The shortcut dialog advertises F6/Shift+F6 region cycling and Shift+F10/Menu context menus, but the key handler implements neither; Escape search clearing is also absent.
- `switchView()` calls `focus()` on a section with no `tabindex`, so that focus target is not established by the markup.
- Tabs do not implement roving `tabindex` or `aria-controls`/labelled tabpanel relationships.
- Ordered pattern/profile rows expose only a drag glyph, with no Move up/down action; source-tree rows likewise have no keyboard selection semantics.

Add the missing controls/handlers and focus restoration, including keyboard Move up/down for order-sensitive rules. These remain source findings until a later authorized interaction review; no keyboard pass is claimed here.

### 8. Settings presents an unresolved shared-catalog product decision as final

**Locations:** `ui-mockup.html:402`; `html-settings-1600.png`; `PLAN.md:101`; `UI-DESIGN.md:370`.

Confirmed. “Shared writable SQLite is not supported” accurately describes the current local-SQLite technical default, but appears as a settled product boundary while the concurrent shared-catalog requirement remains undecided. Reword it as: **“This proposal assumes a local-only writable catalog; network media roots remain supported. A concurrent shared multi-user catalog is an unresolved product/architecture decision.”** Do not imply that shared writable SQLite is safe or already planned.

## Low severity / documentation

### 9. Design verification status contradicts the evidence document

**Locations:** `UI-DESIGN.md:387`; `VISUAL-EVIDENCE.md:5–27`.

`UI-DESIGN.md` still says visual verification is pending and asks the coordinator to render screenshots, although eight captures now exist. After correcting the proposal, update the status to distinguish completed full-document HTML captures from still-unverified dialogs, keyboard interaction, fixed-height desktop scrolling, DPI, and native egui rendering. Do not convert the current evidence into a runtime or parity pass.
