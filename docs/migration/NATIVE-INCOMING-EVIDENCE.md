# Native Incoming operation — 2026-11-06

Disposable inputs, destination and catalog only:
`artifacts/verification/incoming-native-20261106/`.
The binary includes the full-inventory corrections at `40eb9f3` and desktop
persistence at `cf82af8`. No production media or settings were opened.

## Inventory, routing and copy

Entered source and destination through the native page and used Scan folders.
The source contains three payload files named as EXR frames 1001, 1003 and 1004,
a camera MXF, notes.txt, metadata.json, an extensionless README and .hidden.txt.
These are file-operation fixtures, not encoded media.

- Native inventory: 7 files, 5 items, 402 bytes; .hidden.txt excluded.
- Sequence: three actual frames, with missing frame 1002 shown in orange. No
  absent frame was added to the plan.
- Original Simple Project routing placed the beauty sequence under
  `3D/Renders/SH010/beauty/v003` and the four other files under `Video/Footage`.
- Validate selection: seven folders and seven files ready, zero blocked.
- Copy selected: succeeded, seven files copied, seven directories created,
  zero skipped/unfinished. Source files remain present.
- `diff -r` for the three frames and individual `cmp` for all four other files
  returned zero. Destination contains exactly those seven files.
- Journal: one succeeded Incoming run, seven copied + seven created item rows,
  402 committed bytes. Actor is admin; start and finish activity rows name admin.

## Pi folder suggestions

Opened Suggest folders, selected the existing Pi model
`qwen-token-plan/deepseek-v4.1-flash-1017`, prepared the request and opened its
context preview. It contains selected filenames, type, actual-frame counts,
sizes and relative layout; no payload contents are included.

Send to Pi returned five editable suggestions. Apply selected folders to draft
changed the destination preview, marked the retained completed report as belonging
to the previous draft, and disabled Copy until new validation. The catalog still
contains one operation run: applying suggestions performed no file operation.

Screenshots: `artifacts/screenshots/incoming-native-{scanned,validated,copy,completed,ai-context-open,ai-running,ai-result,ai-applied}.png`.
File → Quit exited zero. `data/window.ron` was written inside the disposable area.

## Remaining verification and UI work

This establishes the selected copy path and one real proposal-only model request.
Move, conflict policies, user-edited patterns/profiles, batch edits, pause/cancel,
failure recovery, native chooser shutdown and persisted-layout restoration still
need their own localized manual checks. No automated tests or test suites ran.

## Presentation follow-up

The integrated native build was opened again with the same disposable workspace.
The working list now has resizable File, Type, Frames, Destination and State
columns; single files leave Frames blank. The exact-files window listed only
1001, 1003 and 1004. Continuity warnings remain available in counted collapsibles,
and ordinary footage routing uses a quiet label with details on hover.

Two all-Skip runs completed through the native UI. The first exposed a headline
bug: seven existing folders were included in the skipped-file count. After the
correction, the next run showed seven skipped files, zero copied/moved and zero
unfinished. Expanding the results dock exposed all seven per-file rows with
destinations relative to the original run root. Both sessions exited normally.

Screenshots: `incoming-presentation-scan-result.png`,
`incoming-presentation-frames-open.png`, `incoming-corrected-skip-report.png`,
and `incoming-corrected-expanded-report.png` under `artifacts/screenshots/`.

At the minimum 1080px window width both folder controls remained accessible.
However, saved enlarged docks consumed too much vertical space and overlapped
the working-list/editor headings. Dynamic dock height limits were added after
this observation; their native recheck is recorded separately below. This is
still not full visual or workflow sign-off.

The next native session at 1080×900 confirmed distinct working-list and editor
headings after the dock limits (`incoming-dock-limits-scanned.png` and
`incoming-dock-limits-expanded.png`). Resizing docks remained responsive.
It also exposed an inaccessible State column when saved column widths exceeded
the pane. A horizontal scroll container was added; compilation is checked, but
that last correction has not yet received its native recheck. This session also
exited zero through File → Quit.
