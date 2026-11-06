# VFX Launcher 0.3.4

- Recognize newer ARRI clip names such as A_0007C011_251203_124400_h1l8D.mxf
  and use the configured ARRI folder rule. Upgrade only the unchanged shipped
  ARRI pattern on load; preserve edited patterns and all folder layouts.
- Match dot-prefixed folder keywords such as .cube to file extensions,
  case-insensitively. A custom Footage/LUTs rule containing .cube now works;
  conflicting extension destinations remain ambiguous until resolved.
- Read resolution during Incoming scans with four workers. Native image headers
  are used where supported; video/other image dimensions require host ffprobe.
  Read one representative frame per sequence. Metadata fills missing filename
  resolution tags; explicit edits remain authoritative. Cache results for rule
  rematching, show read failures, and include resolution in reviewed Pi context.
- Parse ffprobe dimensions as JSON so camera side-data cannot break parsing.
  The same correction applies to media previews.
- Populate Suggest folders model choices from Pi's available model list, with
  refresh and visible discovery errors. Prepare local request context
  automatically; only the explicit Send to Pi action submits it. Unavailable
  explicit model choices never silently fall back to another model.

Feature-local checks used disposable PNG/MXF/LUT inputs and confirmed filename
routing, extension conflicts, preservation of custom rules, resolution probing,
manual overrides, and actual Pi model discovery. No generation request was sent.
Native UI and provider generation remain unverified. No real project media or
production catalog was used for validation. Previous runtime limitations apply.
