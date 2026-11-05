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
- Do not push or create PRs unless assigned. Leave unrelated changes untouched.

## Direction and model allocation

- Target Rust + egui/eframe, SQLite, native dialogs, and structured file-operation
  plans. Preserve existing behavior and import formats; do not start from a blank
  feature set. Keep the Python application available during the port.
- GLM 5.3 Flash: research and source exploration.
- Astra: architecture and planning.
- Kimi K3.1: reviews and UI design; HTML mockups precede Rust UI implementation.
- DeepSeek Flash and GLM 5.3 Flash: smaller implementation tasks and local checks.
- Luna with high reasoning: difficult implementation tasks, sparingly.
- Use the requested models; report unavailable models instead of substituting.
- AI provider authentication should be delegated to an existing external harness;
  the harness choice remains open. File-operation validation belongs to the app.

## Reference clones

`gitclones/` contains ignored shallow, sparse source-reference clones. They are
read-only inputs, not part of the product or its Git history. Preserve their
upstream identity and consult source in addition to README claims. Do not edit,
commit, or push from these clones.
