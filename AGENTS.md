# VFX Launcher Rust migration

## Scope and execution limits

- Work only on VFX Launcher and behavior ported from FolderStructurer,
  CleanIncomings, SearchFolderStructures, File-Extention-Copier, and NAS-Backup.
  GalleryBrowser, KAssetManager, and other projects are outside scope.
- Searches must stay inside the assigned checkout and the explicit reference
  directories under `/home/npittas/VFX-Launcher-v2/gitclones/`. Never run
  `find /`, search the home directory, or expand to system-wide discovery.
- Every terminal command needs a hard 10–30 second process timeout, normally
  `timeout 20s` or `timeout 30s`. A tool yield timeout is not a process limit.
  Report any operation requiring a specific longer limit before running it.
- Exclude Git internals, generated outputs, environments, and dependency trees
  from source searches. Read only the files needed for the assigned task.
- Do not create test suites or use test-driven development. Test-suite creation
  waits until all features are implemented. No global tests until the app is
  fully functional and verified working. Only feature-local checks are allowed
  during implementation. Do not create tests without a later explicit assignment.
- Never use production databases or real incoming/backup files for validation.
- Concurrent agents that write files use orchestrator-managed worktrees with
  explicit file ownership. Do not create child checkouts in shell commands.
- All worktrees must be inside `/home/npittas/VFX-Launcher-v2/worktrees/`.
  Do not start isolated delegates if the orchestrator would place their checkout
  outside this project root; report the configuration limitation instead.
- Keep all task artifacts inside the project root, including temporary files,
  verification inputs/results, and UI/UX evidence. Save verification screenshots
  under `/home/npittas/VFX-Launcher-v2/artifacts/screenshots/` and other verification
  outputs under `artifacts/verification/`. UI/UX verification must include actual
  screenshots; do not claim visual verification from source inspection alone.
- Do not push or create PRs unless assigned. Leave unrelated changes untouched.

## Direction and model allocation

- Target Rust + egui/eframe, SQLite, native dialogs, and structured file-operation
  plans. Preserve existing behavior and import formats; do not start from a blank
  feature set. Keep the Python application available during the port.
- GLM 5.3 Flash: research and source exploration.
- Astra: architecture and planning.
- Kimi K3.1: reviews and UI design; HTML mockups precede Rust UI implementation.
- DeepSeek Flash and GLM 5.3 Flash: smaller implementation tasks and local checks.
- The confirmed working Pi DeepSeek model is
  `qwen-token-plan/deepseek-v4.1-flash-1017`. Do not reuse the failed
  `deepseek/deepseek-flash` endpoint for this assignment.
- Luna with high reasoning: difficult implementation tasks, sparingly.
- Use the requested models; report unavailable models instead of substituting.
- AI provider authentication should be delegated to an existing external harness;
  the harness choice remains open. File-operation validation belongs to the app.
- `docs/migration/USER-UI-REQUIREMENTS.md` records the user's desktop redesign
  requirements and supersedes conflicting UI layout/copy in earlier documents.
  No redundant headings, jargon-heavy explanations, duplicate project-creation
  screens, or primary Activity page. Resizable panels/columns, context menus,
  conspicuous execution controls, progress and file lists are required.
- The DCC desktop prototype is user-approved. See
  `docs/migration/DESIGN-APPROVAL.md`; implement this baseline rather than redesign
  it. Approval does not constitute native/runtime validation or settle open
  backend product decisions.

## Reference clones

`gitclones/` contains ignored shallow, sparse source-reference clones. They are
read-only inputs, not part of the product or its Git history. Preserve their
upstream identity and consult source in addition to README claims. Do not edit,
commit, or push from these clones.

## Live desktop automation

Read the computer-use skill, then use this checkout's `tools/computer-use/`
helpers for movement, clicks, keys and screenshots. The external skill helpers
assume 1:1 mouse gain and symbolic ydotool keys; this machine has flat
acceleration with sensitivity 0.35 (1.35 gain), and ydotool requires numeric
keycodes. The project helpers compensate without changing desktop settings,
verify pointer landing before clicking, and keep screenshots inside the project.
Use global desktop coordinates; screenshot captures force scale 1. Leave the
enabled ydotool user service running for automation. See
`docs/migration/DESKTOP-AUTOMATION.md` for the verified three-monitor results.
