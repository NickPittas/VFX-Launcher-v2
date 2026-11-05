# Approved desktop UI baseline

The user approved the DCC workspace prototype with "Approved 100%" on 2026-11-05.

The baseline is `ui-mockup.html` after the corrections documented in
VISUAL-EVIDENCE-DCC.md, read together with USER-UI-REQUIREMENTS.md and UI-DESIGN.md.
Use its docked composition, workspace tabs, original named sequence/shot editor,
equal-height adjacent panes, lower previews, compact Scan folders toolbars,
resizable panels/columns and pinned execution controls for the Rust port.

The approved artifact is a design/interaction prototype. Approval is not native
runtime verification, feature-completeness certification, or acceptance of open
backend decisions such as shared catalog architecture or AI harness choice.

Begin the implementation packets in PLAN.md. Preserve the existing Python
application, reference clones and user data. No test suites until all features
are implemented; no global tests before the app is functional and verified.
Continue feature-local checks only, bounded searches and hard command timeouts.
