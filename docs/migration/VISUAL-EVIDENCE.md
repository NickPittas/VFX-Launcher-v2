# HTML mockup visual evidence

Rendered 2026-11-05 from `ui-mockup.html` using T3 `html_preview`.
These are HTML proposal screenshots, not Rust application screenshots.
All image bytes were saved directly inside project `artifacts/`; no external
screenshot save path was used. The screenshot directory is Git-ignored.

For renderer embedding only, `100vh` was replaced with a 900 px minimum,
document backgrounds were transparent, and the application minimum width was
1180 px. The source HTML was unchanged. Captures show full document height,
not proof of fixed-height desktop scrolling or DPI behavior.

| Screenshot under `artifacts/screenshots/` | State | Dimensions |
| --- | --- | --- |
| html-projects-1600.png | Ready | 1600 x 916 |
| html-structure-1600.png | Ready | 1600 x 1162 |
| html-incoming-1600.png | Conflict | 1600 x 1096 |
| html-transfers-1600.png | Partial | 1600 x 916 |
| html-backup-1600.png | Ready | 1600 x 999 |
| html-settings-1600.png | Ready | 1600 x 916 |
| html-activity-1600.png | Failed | 1600 x 916 |
| html-incoming-1280.png | Conflict | 1280 x 1250 |

All eight renderer calls returned an empty consoleMessages array. Navigation
and scenario selection were invoked through the mockup's existing event
handlers for the non-default captures. This establishes only those exercised
rendered states; full keyboard, dialogs, focus, scrolling and dynamic-state
behavior have not been verified.

Initial issues requiring review: Incoming remains cramped in three columns at
1280 px despite the design specifying an inspector below the table at narrower
desktop sizes; its conflict dock references an AEP file absent from Incoming;
Settings states shared writable SQLite is unsupported even though that product
decision remains open. These are proposal defects, not runtime findings.

T3 interactive preview accepted the HTML in chunks, but preview_snapshot failed
repeatedly with `Preview automation snapshot timed out after 15000ms.` Static
renderer evidence was used instead. No server, build, test suite or production
file operation was used. Later native egui screenshots remain required.
