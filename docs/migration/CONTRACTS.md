# Shared Rust contracts

Companion to [PLAN.md](PLAN.md), 2026-11-05. These are proposed API/data contracts for one crate, not compiled Rust or claims about existing runtime behavior. Types are schematic: omit derives/boilerplate until implementation. Keep each feature's types in its owning module; do not create a universal DTO framework. Public interfaces use `Result<T, AppError>`; errors include operation, affected path/item, stable category and readable cause. Invalid input, unavailable root, stale plan, conflict, permission, unsupported format and I/O are distinct categories. Cancellation is a result state, not an I/O error.

## 1. Paths, traversal and identity

Runtime filesystem values use `PathBuf`, never display strings. `PathKey` is a private, comparable native lexical identity computed from an absolute path against an explicit base. POSIX preserves case and backslashes; Windows normalizes separators, dot components and case in the manner needed for legacy `normcase`, preserving UNC server/share and drive root. No filesystem canonicalization is required for offline catalog lookup. Do not equate a mounted Linux share and a Windows UNC path without an explicit user-supplied root mapping.

Keep the imported spelling, source platform and native resolved path separately. If the import platform is unknown or paths are foreign, retain them as unresolved rather than guessing a mount/drive conversion. Windows case-sensitive-directory collisions and different spellings of symlink/hardlink targets must be flagged before writes. Lexical identity is not proof that two paths refer to different files.

Proposed signatures:

```rust
fn path_key(path: &Path, base: &Path) -> Result<PathKey>;
fn validate_destination(root: &Path, relative: &Path) -> Result<PathBuf>;
fn walk(request: ScanRequest, control: &JobControl, emit: &EventSink)
    -> Result<ScanSnapshot>;
```

`validate_destination` rejects absolute relative-inputs, `..`, NUL, foreign drive/UNC prefixes, invalid platform names and escaping parent symlinks/reparse points. Check resolved containment again at execution; use no-follow/handle-relative platform facilities where available. Refuse unsafe operations when confinement cannot be established. No claim that a string-prefix check prevents races. Reject source=destination, source aliases and source/destination nesting for recursive copy/backup. Default walker does not follow directory symlinks; report skipped links and offer explicit source-root selection instead of silently escaping scope.

JSON path fields are UTF-8 strings. Unix non-UTF-8 paths may remain native in memory/catalog encoded bytes; portable export must report them unsupported rather than use lossy replacement. Catalog path columns store native encoding plus platform tag and a separate display spelling. This constraint applies consistently to config, jobs and AI requests.

```rust
enum ScanPurpose { Projects, Incoming, ExtensionCopy, Backup, StructureExport }
struct ScanRequest { root: PathBuf, purpose: ScanPurpose, recursive: bool,
                     suffixes: Vec<String>, excludes: Vec<String> }
struct FileEntry { path: PathBuf, stamp: FileStamp }
struct FileStamp { size: u64, modified: SystemTime /* plus file identity if available */ }
enum Coverage { Complete, Cancelled, Partial, RootUnavailable }
struct ScanSnapshot { root: PathBuf, entries: Vec<FileEntry>, coverage: Coverage,
                      errors: Vec<PathIssue>, root_identity: Option<RootIdentity> }
```

`PathIssue` contains path and cause; `RootIdentity` records observable filesystem/root identity, not an assumed reliable SMB connection ID. A root that still exists can be an empty mountpoint after disconnect. Without trustworthy continuity evidence, suppress destructive catalog reconciliation and report why. Complete means all requested directories enumerated successfully; a failed subtree is never an empty result. Discovery itself never deletes catalog rows.

Tool filter defaults: project scan `.nk`/`.aep`, explicitly excluding `.nk~`/`.nk.autosave`; extension copy `.nk`/`.autosave` with editable case-insensitive suffixes; backup `.nk`/`.aep`, case-insensitive; incoming uses supported configured filename patterns and file/sequence discovery. Keep registration discovery immediate-child-only and sorted by case-folded name then full path. Scan-all queues project requests serially.

## 2. Projects and catalog reconciliation

```rust
type ProjectId = i64;
type UserId = i64;
struct Project { id: ProjectId, name: String, root: PathBuf,
                 scan_folders: Vec<PathBuf> }
struct RegistrationSummary { added: Vec<ProjectId>, skipped: Vec<PathBuf>,
                             failed: Vec<PathIssue> }
fn register_projects(store: &mut Store, roots: &[PathBuf], user: UserId)
    -> Result<RegistrationSummary>;
fn reconcile_scan(store: &mut Store, project: ProjectId, scan: ScanSnapshot)
    -> Result<ReconcileSummary>;
```

Registration is a short serialized catalog operation with per-root results, preserving legacy noncancellable registration semantics. Cancel discovery before registration; do not imply already registered rows were undone. Registration validates existence/readability, but retains an existing offline project's catalog record. Scan roots named exactly `Projects` use the parent project identity, as in the source.

Reconciliation commits observed files transactionally. It may prune only within the snapshot's declared scope when coverage is Complete, cancellation remains unset at commit, root continuity is proven and each absent file returns a definite NotFound. Permission/network/metadata errors suppress pruning. Foreign scope and other project memberships are untouched. Proposed change: a physical file can belong to multiple projects using membership rows; scanning one project never steals it from another. Imported duplicate rows retain origin mappings and history, even when normalized into one file.

Version data retains raw text plus optional parsed integer and padding; `_v(\d+)` is the Launcher preset, separate from incoming extraction configuration. Latest selection orders parsed version, then modification time, then stable path/ID; unknown versions stay visible. UI retains grouping and manual version selection. Removing a project deletes memberships/favorites/recents transactionally, preserves shared files and necessary historical attribution, and never deletes media. User deletion preserves historical identity through a retained/tombstoned attribution record.

## 3. Storage and import

One storage worker owns the writable rusqlite connection. Workers send typed requests through a bounded, non-blocking queue and receive owned results; do not share a mutable connection or block an egui frame on it. That worker is reached through cloneable request-only handles that share its queue and connection; admission belongs to the catalog owner alone, so explicit shutdown or owner drop stops new submissions for every live handle while accepted work drains and the worker exits. Local SQLite: foreign keys on, WAL only when the target is provably local (rollback otherwise), `synchronous = FULL` for WAL and `EXTRA` for rollback on that single writable connection so journal removal is synced too, busy timeout 5 seconds; expose busy errors, and report the effective journal mode and synchronous level in `CatalogInfo` as configuration evidence, not as a network or power-loss guarantee. New schema uses `PRAGMA user_version` with transactional migrations. Detect/reject known network DB locations, default to a local app-data directory, and document that undetectable remote mounts cannot be certified safe by the path alone.

Minimum tables: users, projects, project_scan_folders, files, project_files memberships, favorites, recents, file_access_log, user_activity_log, import_origins and operation_runs/items. Use unique native keys for projects/files and unique `(project_id,file_id)` memberships. Store per-item operation state for interrupted-run reporting; no automatic replay. History timestamp strings from legacy remain raw when timezone is unknown. New timestamps are UTC with an explicit offset/format.

The legacy import adapter must recognize:

| Table/input | Required legacy data |
|---|---|
| `users` | `id, username, is_admin, created_at` |
| `projects` | `id, name, path, added_by, added_at`; optional `folders` is newline-separated text, not JSON |
| `project_files` | `id, project_id, filename, filepath, version, filetype, last_modified`; filepath is not SQL-unique |
| `recent_projects` | `id, user_id, project_id, accessed_at` |
| `favorite_projects` | `id, user_id, project_id`; not the active UI's only favorite store |
| `file_access_log` | `id, file_id, user_id, accessed_at` |
| `user_activity_log` | `id, username, action_type, description, timestamp` |
| INI | Paths: nuke_path, after_effects_path, nuke_launch_cmd_linux, DataDir; Projects: project_directories comma-separated names, scan_interval_minutes; Database: db_path; Logging: log_level, log_file |
| Favorites | `favorites_<username>.json`: unversioned array of numeric IDs and/or strings; resolve IDs against the selected legacy database, paths against its import platform |
| Other JSON | FolderStructurer clients array; CleanIncomings patterns and both profile forms; NAS job fields below; organizer nested objects with `__files__` arrays |

Imports are staged into a new local catalog/config area, validated, then adopted atomically where possible. Open legacy SQLite read-only using a consistent SQLite snapshot; do not byte-copy a live DB and ignore its journals. No schema initialization against the source. Keep original IDs where unambiguous, otherwise persist explicit source-ID→new-ID maps before importing foreign keys. Preserve orphan/raw/conflicting records in the import report for resolution rather than silently discard. Same source identity and row IDs must not duplicate entries on re-import; do not auto-merge unrelated databases. A staged import failure leaves current settings/catalog selected and source untouched.

Favorites import takes the union of resolvable sidecar and SQL records, deduplicated per user/project, with provenance and unresolved entries reported. Numeric strings try the selected DB ID first; absolute path strings use path resolution. No guessed basename match. Preserve unused INI values in import metadata but mark scan interval/log file inactive unless implemented. A malformed input never silently resets existing settings.

New JSON envelope: `{ "schema_version": 1, ... }`. Settings contain application argv, project directory names/root preferences, selected user and log level; UI state separately contains geometry, panels, columns and last selection. Templates/patterns/profiles remain separate editable resources. Use sibling temp files, flush and atomic replacement for saves, preserving the last good file on failure. Temp files belong beside destination in deployed operation; all development verification copies stay within project artifacts. Never save credentials here.

## 4. Structure creation

```rust
enum NamingPreset { LauncherDated, FolderStructurer }
struct ShotGroup { sequence: Option<String>, shots: Vec<String> }
struct CreateProject { parent: PathBuf, number: Option<String>, client: Option<String>,
    name: String, date: Option<String>, naming: NamingPreset,
    template: TemplateId, shots: Vec<ShotGroup>, nuke: Option<NukeSettings> }
struct NukeSettings { fps: f64, width: u32, height: u32, proxy: bool, aces: bool }
fn plan_structure(input: &CreateProject, template: &StructureTemplate)
    -> Result<OperationPlan>;
fn plan_add_shots(project: &Path, shots: &[ShotGroup], template: &StructureTemplate,
                  nuke: Option<&NukeSettings>) -> Result<OperationPlan>;
```

`StructureTemplate` has ID/name, ordered relative directories, optional Sphere/Kent folder-label mapping and explicit generated-file rules. Import directory templates and Launcher manifest without flattening their distinctions. Template editing modifies app-owned templates, never originals unless separately requested. Clients are a string list, trimmed/deduplicated without losing Unicode display spelling. `TemplateId` is a stable local ID, not a filesystem path.

Default naming is the Launcher dated preset; FolderStructurer preset retains optional number/client and reference creation rules. Preview captures the chosen date once so execution cannot change the name at midnight. Next number reads immediate directories; errors block auto-numbering rather than suggest `00001` from an unreadable root. Exclusive root creation handles concurrent numbering collisions; request a new preview on conflict. Reject path separators/traversal in names; preserve legitimate Unicode. Add-to-existing is additive and never overwrites existing scripts by default. Nuke text uses the reference `common/nuke_template.py` content and options after focused source reading; escape values for that format. Preview and execution consume the same plan.

## 5. Incoming and sequence contracts

```rust
struct Sequence { directory: PathBuf, prefix: String, suffix: String,
                  padding: usize, members: Vec<SequenceMember> }
struct SequenceMember { frame: i64, file: FileEntry }
struct Tags { shot: Option<String>, task: Option<String>, asset: Option<String>,
              stage: Option<String>, resolution: Option<String>, version: Option<String> }
enum MappingState { Matched, FootageFallback, Ambiguous, Unmatched, Invalid }
struct MappingProposal { item_id: u64, sources: Vec<FileEntry>, tags: Tags,
    destination_relative: Option<PathBuf>, state: MappingState,
    alternatives: Vec<PathBuf>, reason: String }
fn propose_incoming(items: &[IncomingItem], patterns: &PatternSet,
                    profile: &IncomingProfile) -> Result<Vec<MappingProposal>>;
```

`IncomingItem` is a single file or Sequence. Groups require matching directory, prefix, suffix and padding; sort members numerically, retain gaps, do not infer missing frames or combine different versions. Sequence range/`####` names are display values only. The expanded transfer plan lists every actual member and its actual destination basename. Retain individual-file selection for ambiguous single-frame grouping.

`PatternSet` preserves category, order, raw expression, literal/regex mode and task label/capture behavior. Filename-only means neither parent paths nor project directory names participate. Preserve raw imported pattern strings and report dialect translation. No generic hardcoded camera/shot/task fallbacks. Incoming importer must inspect extraction helpers to settle existing per-category precedence; do not assume all task patterns return the matched substring, since named camera task keys route profiles. Unsupported regex is an actionable import error for the affected rule.

`IncomingProfile` is a name and ordered relative-path→keyword-list rules, imported from list or `{rules:[...]}`. Routing: exact task match first, exact asset next, conflicting partial matches become Ambiguous, otherwise use the configured footage path (or visible `unmapped_footage` fallback). Dynamic segments: shot/stage/task/asset/resolution/version; retain shot case, uppercase resolution, lowercase remaining components. The basename is retained unless an explicit edit says otherwise. Batch edits recompute proposals from edited tags and invalidate prior validation; no direct file writes. Ambiguous/invalid proposals cannot execute. Fallback destinations must be visible in review.

## 6. Shared operations and controls

```rust
enum ConflictPolicy { Skip, Replace, KeepBoth, PreserveExisting }
enum FileAction { Copy, Move }
enum PlannedAction {
    CreateDirectory { destination: PathBuf },
    CreateFile { destination: PathBuf, contents: Vec<u8> },
    Transfer { source: FileEntry, destination: PathBuf, action: FileAction },
}
struct OperationPlan { id: u64, revision: u64, destination_root: PathBuf,
    actions: Vec<PlannedAction>, conflict: ConflictPolicy, origin: PlanOrigin }
fn validate_plan(plan: &OperationPlan) -> Result<ValidationReport>;
fn plan_extension_copy(source_root: &Path, destination_root: &Path,
                       scan: ScanSnapshot) -> Result<OperationPlan>;
fn execute(plan: ValidatedPlan, control: &JobControl, emit: &EventSink)
    -> Result<RunReport>;
```

`PlanOrigin` is Structure/Incoming/ExtensionCopy/Backup/AI. `ValidationReport` contains item diagnostics, expected source/destination metadata, space estimates (unknown is explicit), conflicts and `ready: Option<ValidatedPlan>`. Only transfers constructs `ValidatedPlan`, with private fields; callers cannot bypass validation. It binds plan revision, selected policy and review-time destination evidence. Any edit or changed file state invalidates it. User review confirms the concrete plan; a schedule authorizes only its saved backup scope/policy and always generates and validates a fresh plan at run time. AI cannot construct a validated plan.

Extension-copy planning preserves each selected file's path relative to the source root under the destination root. It consumes discovery's suffix-filtered entries, reports incomplete scans, and uses the same conflict/metadata/cancellation rules as other transfers. It needs no separate module or copy loop. Structure actions run parent directories before children/generated files; an existing directory is accepted only for add-to-existing, while a new project root must be created exclusively. Failed prerequisites block dependent actions and remain visible in the report.

Reject duplicate destinations, escaping targets, source aliases, unsupported file kinds and conflicting source/destination chains before execution. Ordinary conflicts default Skip. Replace replaces only the reviewed destination after revalidation. KeepBoth chooses a free numbered incoming name; PreserveExisting reserves `1_<stem><suffix>`, then `2_...` for the old destination (NAS convention). Reservation/commit must not overwrite a newly raced file. Concurrent app workers serialize intersecting destinations; external races still require platform-safe commit primitives. If those primitives are unavailable, block unsafe replacement rather than silently weaken no-clobber behavior.

Copy to a unique app-owned sibling temporary file, flush, check byte count and source metadata stability, preserve mtime/permissions where supported, then commit. Metadata-preservation failure must be visible. Move uses a safe same-filesystem rename when possible; cross-filesystem move copies and verifies before deleting the unchanged source. A failed source removal reports `CopiedSourceRetained`, not moved. Size/mtime checks are not a cryptographic content-integrity guarantee; no checksum-parity claim. PreserveExisting stages new content before moving the old destination aside; on commit failure attempt safe restoration and report retained recovery paths. Delete only temporary files this operation owns, never an arbitrary destination on cancellation.

Per-item states: Pending, Running, Copied, Moved, Created, Skipped, CopiedSourceRetained, Failed, Cancelled. Batch terminal outcome: Succeeded, Partial, Failed or Cancelled, with completed/failed/skipped/cancelled counts and bytes. Completed items stay completed on cancellation; no whole-batch atomicity or undo guarantee. Persist intent before mutation and outcomes after it. On restart mark unfinished runs Interrupted and inspect/report temporary/backup paths; never auto-delete or replay ambiguous work.

Review-time destination evidence is part of validated plan: observable identity of reviewed destination root plus identity of every existing ancestor directory reviewed items need, each recorded once, captured inside validating worker (never inside catalog worker), persisted in intent envelope, and exposed read-only for future executor. Generated directories stay ordinary `CreateDirectory` actions and gain identity only when execution creates them. Envelope field defaults when absent; missing or incomparable identity is unknown, never agreement. Journal without destination evidence stays inspectable and finalizable but can never start, so no older run becomes executable or replayed.

Item already Running may append checkpoints (staging, staged, displacing, installed, removing source) recording intent without committed bytes; checkpoints never make item terminal, only move forward, and earlier events plus recovery paths are retained. Terminal item states immutable: outcomes appended, never overwritten.

Failure or cancellation after complete destination install may record 0 or exactly reviewed byte count, never invented partial staging bytes; nonzero recorded failure or cancellation must carry diagnostic message or recovery path. Run byte totals include persisted committed bytes of terminal outcomes including those failure/cancellation cases; older unknown data stays unknown.

Future executor reaches catalog only through existing request-only single-worker, single-connection API; execution requests answered by that worker, never by delegating UI queue, UI transaction or coercion, and no admission, shutdown or UI-flow API is added for it.

Engineering limits, not approval flows: operations handle-anchored, no-follow, immediately revalidated, atomic no-clobber install; no claim against hostile privileged namespace or mount change. Nested mount below explicitly selected destination root fails closed once executor exists (selected root itself may be a mount). Namespace durability is platform/filesystem-conditional; when it cannot be established a move retains source. POSIX permission bits and mtime preserved; Windows ACL preservation not claimed; detectable failures surfaced before commit. SQLite and filesystem are not one transaction: interrupted state inspected only, never auto-replayed or auto-cleaned. WAL with `synchronous = FULL` (`EXTRA` in rollback mode) is configuration evidence for a local file, not a network or power-loss guarantee.

`JobId` is a monotonically increasing local identifier. `JobControl` uses shared cancel/pause flags plus a wake mechanism; cancel wakes paused workers. `JobEvent` carries job ID, phase, completed/total files, bytes (total optional), current path, diagnostic or terminal report. Bound/coalesce progress, never lose a terminal event, request repaint on updates. Pause/cancel is cooperative between chunks/items; blocked OS network I/O cannot promise immediate interruption. Display Pausing/Cancelling until acknowledged. Default transfer concurrency 4; NAS accepts 1–16. No nested per-sequence worker pools. Shutdown stops scheduling, signals active work, waits for safe item boundaries and records unfinished state; do not block the UI thread or claim instant cancellation.

## 7. Backup jobs and schedule

`BackupJob` preserves legacy JSON fields: `source`, `destination`, `last_browse_dir`, `excludes` (comma-separated string), `workers` (default 4, validate 1–16), `schedule_enabled` (false), `schedule_mode` (`interval`), `schedule_interval_minutes` (240), `schedule_time` (`02:00`). Accept optional `created_at`/`app` export metadata and preserve unknown fields in import metadata. New saves add schema version; legacy-compatible export keeps recognized legacy keys. Never activate an imported schedule until its roots/policy have been reviewed in the app.

```rust
fn plan_backup(job: &BackupJob, scan: ScanSnapshot) -> Result<OperationPlan>;
fn next_due(schedule: &Schedule, now: DateTime, last_terminal: Option<DateTime>)
    -> Result<Option<DateTime>>;
```

Copy missing destination or strictly newer source mtime; equal/older sources skip, even when sizes differ (legacy mtime policy, not integrity checking). Stat errors block affected items rather than mean copy. Excludes are case-insensitive fnmatch-style patterns: slash-containing patterns match the normalized relative full path (also leading-slash form), other patterns match any component or whole relative path. Match Python fnmatch wildcard semantics deliberately; do not assume a Rust glob library's path-segment behavior is equivalent. Backup is additive/version-preserving, never a destination-deleting mirror.

One active backup job, manual or scheduled. Lifecycle: Idle → Due → Scanning → Validating → Copying → terminal → Idle; pause applies to active work, cancel scan prevents automatic copy. Errors and no-change scans are terminal too and re-arm the schedule. Due while busy coalesces to one pending run; disabling drops pending triggers, editing affects the next run only. Snapshot config for each run. Interval is measured from terminal completion (and from enable/startup for a fresh schedule), default 240 minutes. Daily time is local wall time, once per local date; DST nonexistent time advances to the first valid instant, repeated time runs once. On restart compute a future due time, no missed-run backlog. Scheduling exists only while the GUI is running; no daemon is implied.

CLI behavior in the same binary: `backup --job <json>` executes a fresh validated job; `backup --job <json> --dry-run` and `backup --source <path> --destination <path> --dry-run` plan only. Never start the scheduler headlessly. Exported jobs are scope/policy inputs, not arbitrary executable plans. Return 0 for successful/no-change/dry-run completion, nonzero for invalid/failed/partial/cancelled runs; finalize exact numeric mapping with the CLI packet. Totals-only text reports preserve NAS convention; detailed recovery/item state remains in local activity storage. No shell `find`, `tr` or robocopy dependency in the Rust engine.

## 8. AI boundary

Pi versus OpenCode remains undecided. The following is the app's semantic interface, not a claim that either harness supports this wire format:

```rust
struct AiRequest { request_id: u64, selected: Vec<AiItem>,
    structure: StructureDescription, instructions: String,
    previous_proposals: Vec<MappingProposal> }
struct AiSuggestion { item_id: u64, destination_relative: String, reason: String }
fn decode_suggestions(response: &str, request: &AiRequest)
    -> Result<Vec<AiSuggestion>>;
```

`AiItem` includes stable item ID, filename, permitted metadata and sequence member summary. Send no file content or provider secrets by default; show the exact context selected for sending. `StructureDescription` supports imported folder objects/`__files__`, templates and chosen destination scope. Initial suggestions and refinement return draft mappings. Reject unknown/duplicate item IDs, unbounded responses, malformed JSON, absolute paths, traversal and invented file sources. Treat filenames, structure text and model explanations as data, never commands. Bound input/output sizes and subprocess duration; cancellation terminates the owned request when the chosen harness allows it.

The future adapter delegates authentication/provider/model selection to the harness and maps its documented request/response lifecycle to these types. No app-owned API keys, login UI, shell execution of model text, filesystem tool permission or auto-apply. The model proposes operations; the user reviews them; the application resolves exact sources, validates paths/conflicts and executes through transfers. A failed/refused/cancelled harness response leaves the draft unchanged. Harness choice blocks live integration, not deterministic incoming mapping, structure import or proposal validation.

## Acceptance and evidence

These contracts require feature-local source checks and, when later assigned, disposable local operation checks; they do not authorize tests, build commands or launches in this planning task. UI acceptance requires actual screenshots under `artifacts/screenshots/`. Other development evidence and temporary inputs remain under `artifacts/verification/`. No production DB or incoming/backup files may be used for validation. Exact ABI, compiled type correctness, runtime safety, cross-platform semantics and visual quality remain unverified until their respective implementation packets are completed and checked.
