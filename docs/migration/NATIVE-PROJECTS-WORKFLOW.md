# Native Projects workflow — 2026-11-05

The coordinator used the computer-use skill and project-local calibrated input
helpers against the actual GTK-linked Rust binary. All data was disposable under
`artifacts/verification/native-data` and `native-projects`; no DCC was launched.
The first 180-second observation session expired; the second was closed through
File → Quit and exited with code 0. Individual control commands were capped at 20s.

Observed:

- Add existing folder opens the GTK folder chooser. Its window appeared after the
  first screenshot; the initial “Choosing folders” image alone was not a failure.
- Entering the synthetic Projects directory registered `00101_DesktopReview`:
  the native workspace reported “1 added”.
- Selecting the project and Scan folders completed a scan of four project files.
  Nuke shows TimelineA/SH010 and TimelineB/SH020 separately, including versions
  003, 001 and 002; the `.nk.autosave` file is absent.
- The After Effects tab shows `title_12_final.aep` in TimelineA/SH010.
- The panes share a common top/bottom boundary. At this wide tiled size the file
  column becomes excessively wide, separating filenames from their controls.
- Versions currently render as individual rows, not the approved selector. This
  is an unfinished UI requirement, not an approved design change.

Evidence under `artifacts/screenshots/`:

- `gtk-picker-visible.png`: live native chooser (contains incidental Recent list).
- `native-registered.png`: successful registration.
- `native-projects-populated.png`: Nuke buckets, versions and completed scan.
- `native-projects-after-effects.png`: functioning file-type tab.
- `native-quit-menu.png`: actual File menu used for orderly exit.

The unrelated crash notifications visible on the desktop were already present;
they are not Launcher failures. GTK emitted “Failed to measure available space:
The specified location is not supported” while displaying Recent; folder selection
and registration still completed.

Not exercised: DCC spawning, reveal, concurrent launches, user switching during a
launch, favorites failure, close during active work, version selection, Windows
or macOS, real media/network roots, or any transfers. No tests or suites ran.

## Version selector follow-up — 2026-11-05 22:44

The corrected native build now renders one row per FileGroup and a version
dropdown beside Launch. Its initial file column is 360 points, with trailing
space after the data columns rather than between the filename and its controls.
Columns remain resizable. Picks are scoped by project, timeline/shot, base and
file type; unavailable picks fall back to the latest visible version.

Actual manual observations with the same disposable catalog:

- `native-version-groups.png`: TimelineA/SH010 and TimelineB/SH020 each have one
  row, initially showing versions 003 and 002 respectively.
- `native-version-dropdown.png`: the first group offers 003 and 001.
- `native-version-context.png`: choosing 001 updates the row; the context menu opens.
- `native-version-information-detail.png`: Information identifies
  `Projects/Nuke/TimelineA/SH010/comp_v001.nk`, Version 001. Thus the file action
  targets the selected version, not the latest file.
- File → Quit exited normally (code 0).

The earlier separate-row/version-selector limitation is fixed. This follow-up
did not exercise launching, reveal, clipboard contents, keyboard-only selection,
rescan invalidation, or all window sizes. Build completed without warnings under
`timeout 30s`; input/screenshot commands used `timeout 20s`; the GUI was capped at
180 seconds. No tests or real project data were used.
