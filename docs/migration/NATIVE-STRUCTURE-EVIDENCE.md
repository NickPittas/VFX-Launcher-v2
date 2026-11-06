# Native structure operation — 2026-11-06

Disposable data and output only: `artifacts/verification/structure-complete-20261106/`.
The native binary was built at `a338a9e`; the default-discovery correction below
was source-checked during the session and requires the next binary rebuild.

- New project: Launcher preset, client Native, project Structure, captured local
  date 2026-11-06, named sequence SQ_NATIVE with shot SH010, Nuke enabled at
  24 fps / 1920×1080, proxy and ACES off.
- Preview and Check produced 39 ready actions, zero blocked. Add project ran
  through the actual journaled executor. The catalog records a succeeded
  structure operation and the project was registered and selected.
- Confirmed generated file:
  `projects/00001_Native_Structure_2026-11-06/Projects/Nuke/SQ_NATIVE/SH010/project/SQ_NATIVE_SH010_comp_v001.nk`.
  Its header/layout was read back. Nuke itself was not launched, so this does not
  establish Nuke acceptance.
- The first automatic project scan refused empty configured folder names. The
  missing default was corrected to the original source's `project, projects,
  Project, Projects`; an explicitly saved empty selection still refuses scanning.
  During this session the disposable settings were edited and saved through the
  native Settings page. Scan folders then recorded the script, and Projects
  displayed its SQ_NATIVE/SH010 group, version selector and adjacent Launch button.
- Existing-folder addition: checked Projects/Nuke in the actual folder tree,
  entered SQ_ADDED with SH020, previewed and checked two folder actions, then
  executed Add to project. Confirmed Projects/Nuke/SQ_ADDED/SH020 on disk. UI
  report and journal both say succeeded / 2 created / 0 failed.
- Journal totals: two succeeded structure runs, 41 created items. One real file
  and one project-file membership after scanning.

Screenshots under `artifacts/screenshots/structure-complete-*` show form entry,
review, actual creation progress, the registered project, discovered script,
existing-target tree editing, checked destination paths and the completed run.
File → Quit closed the native session normally.

Remaining verification: template/client administration, modified presets,
standalone-shot and Nuke-option variants, cancellation/failure/retry, and the
latest asynchronous resource-workspace changes. The new-project modal's review
area shares a small scroll region with its form; this is not a complete visual
sign-off. No automated tests, production data or production settings were used.
