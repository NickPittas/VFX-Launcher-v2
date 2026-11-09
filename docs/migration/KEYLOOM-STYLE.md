# Keyloom visual guide adaptation

Branch: `ui/keyloom-visual-style`. User-supplied component references:

- `/home/npittas/keyloom-round3/ui-audit/bible/specimen.html`
- `/home/npittas/keyloom-round3/ui-audit/bible/specimen@1920.png`

The supplied HTML is the component mockup for this pass. The native application
keeps its existing workspace composition, resizable panels, menus, file tables,
validation and worker behavior. This is a visual adaptation, not a pixel copy.

`rust/src/theme.rs` owns the common dark palette and control styles:

| Element | Adaptation |
| --- | --- |
| Surfaces | App #181818, panel #202020, raised #292929, fields #141414 |
| Boundaries | Divider #303030, control #757575, hover #A0A0A0 |
| Text | Primary #E8E8E8, secondary #B0B0B0, muted #969696 |
| Primary action | Blue #2D6BD0, hover #3572D8, pressed #1D4E9E |
| Status | Warning #E8A93A, error #F2665E, success #4CC38A |
| Typography | Embedded Noto Sans 12px; IBM Plex Mono 11px; headings 15px |
| Controls | 22px baseline, 26px primary actions, 3px corners, 1px borders |
| Spacing | 8px horizontal / 6px vertical, 12px dialog padding |
| Workspaces | 28px tabs with blue active underline |

Primary actions share hover/pressed/disabled styling across all workspaces.
Progress indicators use a neutral fill; errors and warnings no longer use the
older orange/red variants scattered through the UI. Native egui interactions,
focus handling and keyboard behavior are retained. Existing fonts remain as
fallbacks for glyphs absent from the supplied fonts. The copied font assets
retain their SIL Open Font License notices in both package recipes.

Verification: Linux library/binary Cargo check and native debug build pass.
The preview launched with the separate `artifacts/verification/keyloom-ui-preview`
data folder; `artifacts/screenshots/keyloom-ui-first-native.png` records the
initial native Settings view. The user approved the look on 2026-11-09 and
requested merging to main and a new release (0.4.0). This is visual approval,
not a fresh workflow, DPI or cross-platform rendering pass. No test suites
were created or run. Packaging checks are recorded separately for 0.4.0.
