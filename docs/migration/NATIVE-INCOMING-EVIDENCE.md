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

The live page still exposes duplicate technical continuity warnings and verbose
footage-fallback explanations. Those need concise labels with collapsible details.
The active progress label counts directory operations as files; the final report
correctly separates the seven copied files. This is not full visual sign-off.
