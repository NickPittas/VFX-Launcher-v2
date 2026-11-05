//! P1 catalog: project registration, scan reconciliation and next-view reads.
//!
//! Packet P1 of the Rust migration (`PLAN.md` packet P1, `CONTRACTS.md` §2),
//! built on F2 [`crate::storage`] (one worker owns the writable SQLite
//! connection), F1 [`crate::paths`] (native path identity) and F4
//! [`crate::discovery`] (explicit-coverage snapshots).
//!
//! Every database operation in this module is submitted through
//! [`crate::storage::Catalog`] and returns a [`Pending`] the caller polls, so no
//! egui frame runs SQL. Filesystem work is kept out of the caller's frame too:
//! [`plan_registration`] is a pure preworker function (existence, readability
//! and the legacy literal-`Projects` parent identity), and reconciliation's
//! absence checks run inside the catalog transaction that needs their evidence.
//!
//! Preserved legacy behavior (`ui_slick/async_workers.py`:
//! `canonical_project_path`, `register_project_folders`;
//! `ui_slick/scanner.py`: `_update_database`):
//!
//! - Registration is one short serialized operation with per-root
//!   added/skipped/failed results, ordered exactly like the request. Roots are
//!   compared by native identity, and a root already in the catalog is skipped
//!   instead of duplicated.
//! - A folder whose final component is literally `Projects` registers the
//!   **parent** as the project identity while the `Projects` folder itself stays
//!   the explicit scan scope. No other widening happens: a scan only ever
//!   covers roots that were registered.
//! - A cancelled or unavailable scan never deletes catalog rows, and stale
//!   membership removal (pruning) additionally requires `Complete` coverage,
//!   `Proven` root continuity and a definite `NotFound` per file.
//! - One physical file has one `files` row and one membership row per project,
//!   so overlapping projects share it instead of stealing it: scanning project
//!   A never touches project B's membership or its catalog attribution.
//! - Version text keeps its raw digits; `_v<1-4 digits>` is the Launcher preset
//!   and AEP keeps the legacy last-number fallback
//!   (`ui_slick/scanner.py::_parse_version_from_filename`). Unknown versions are
//!   stored and remain visible rather than being invented as `_v1`.
//! - Grouping/ordering follows `ui_slick/file_handlers.py::get_base_name`,
//!   `group_by_timeline`, `group_versioned_files` and
//!   `core/utils.py::get_latest_version_file`: files are bucketed by the
//!   timeline and shot folders below the topmost `Nuke`/`After Effects` anchor,
//!   `_v<digits>` is stripped from the base name, and the newest version is
//!   chosen by parsed version, then modification time, then a stable spelling.
//!
//! Deliberate deviations, all documented rather than silent:
//!
//! - `_parse_version_from_filename` returns `"1"` when nothing matches; this
//!   module stores no version instead (`CONTRACTS.md` §2: raw text plus
//!   optional parsed integer, unknown versions stay visible).
//! - Only a live (non-tombstoned) user id may register, favorite, mark recent,
//!   log access or remove; no code here assumes `id == 1` or an `admin` row.
//! - Removal is a soft remove: `removed_at` is stamped, memberships, favorites
//!   and recents for that project are deleted in the same transaction, and the
//!   `files` rows and their history stay. Nothing on disk is ever deleted.
//!   Scan-folder registrations are kept so re-adding the same root revives the
//!   project instead of failing the `UNIQUE (root_platform, root_storage)`
//!   constraint — re-add handling that a hard delete would not need.
//! - A file name that is not valid UTF-8 keeps its native path bytes and
//!   identity, but `files.filename` cannot hold it: the column stays empty, the
//!   file is reported in the reconcile summary, and grouping uses the native
//!   bytes so two different non-UTF-8 names are never merged.
//! - Pruning deletes membership rows only. Orphan `files` rows are retained for
//!   historical attribution (`CONTRACTS.md` §2); garbage collection is not part
//!   of this packet.
//! - A cancellation is a result, never an I/O error: a cancellation observed at
//!   any boundary — including after the writes ran — rolls the reconciliation
//!   transaction back and returns an `applied == false, cancelled == true`
//!   summary, so the catalog keeps nothing while the caller can still say what
//!   happened.
//! - A size or modification time that the catalog columns cannot represent is
//!   reported against its path and leaves the stored value untouched (`COALESCE`
//!   in the update); nothing is truncated or wrapped.
//! - Listing/filtering happens in Rust over the live project rows (catalogs are
//!   small) so case folding uses [`crate::paths::fold_native_text`] instead of
//!   SQLite's ASCII-only `lower()`.
//!
//! Deliberate limits:
//!
//! - [`JobControl`] is honoured as a cancellation checkpoint, not a pause gate:
//!   `JobControl::checkpoint` blocks while paused, which would stall the single
//!   catalog worker for every other request. Pause belongs to the scan worker.
//! - Reconciliation's absence checks stat the filesystem on the catalog worker,
//!   because the prune decision and its evidence must be one transaction. They
//!   also require the snapshot's root identity and a *fresh* `inspect_root` of the
//!   scan scope to agree immediately before pruning; `Proven` continuity alone is
//!   treated as a caller claim, not as evidence. Discovery reports `Uncertain`, so
//!   no stat of an unreachable share runs today.
//! - Nothing here creates, renames or deletes project files, opens a launch
//!   command (P2) or builds a structure (S1). Model selection, execution and UI
//!   wiring are later packets.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::discovery::{Coverage, RootContinuity, ScanPurpose, ScanSnapshot};
use crate::jobs::JobControl;
use crate::paths::{
    PathCategory, PathIssue, PathKey, PathRelation, fold_native_text, inspect_root, path_key,
    relation,
};
use crate::storage::{
    Catalog, FileId, PathColumns, Pending, ProjectId, StorageCategory, StorageError, StorageResult,
    TransactionOutcome, UserId, utc_timestamp_now,
};

/// Which catalog listing the caller wants. `Favorites` and `Recents` are always
/// scoped to one selected user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectList {
    All,
    Favorites,
    Recents,
}

/// The two tool file views the desktop UI shows. Filtering is by the stored
/// `filetype` column, not by re-inspecting the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    Nuke,
    AfterEffects,
}

impl FileKind {
    fn extension(self) -> &'static str {
        match self {
            Self::Nuke => "nk",
            Self::AfterEffects => "aep",
        }
    }

    fn matches(self, filetype: Option<&str>) -> bool {
        filetype.is_some_and(|value| value.eq_ignore_ascii_case(self.extension()))
    }
}

/// One explicit scan scope of a project. `path` is `None` for a spelling this
/// build cannot resolve (imported foreign platform), which stays listed and is
/// never scanned by guesswork.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanFolder {
    pub path: Option<PathBuf>,
    pub display: Option<String>,
}

/// A live project record with its identity root, its registered scan scopes and
/// the selected user's favorite/recent state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub root: Option<PathBuf>,
    pub root_display: Option<String>,
    pub added_by: Option<UserId>,
    pub added_at: String,
    pub scan_folders: Vec<ScanFolder>,
    pub favorite: bool,
    pub accessed_at: Option<String>,
}

/// Explicit scan scopes of one project, as native paths plus unresolved display
/// spellings. Callers scan `folders` and must report `unresolved` instead of
/// widening to a parent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanScopes {
    pub project: ProjectId,
    pub root: Option<PathBuf>,
    pub folders: Vec<PathBuf>,
    pub unresolved: Vec<String>,
}

/// One validated registration candidate: the identity root, the explicit scan
/// scope belonging to it, and the project name taken from the identity root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistrationCandidate {
    pub identity_root: PathBuf,
    pub scan_folder: PathBuf,
    pub name: String,
}

/// Result of the preworker validation step: what the catalog worker may insert,
/// and what was rejected before any SQL ran.
#[derive(Clone, Debug, Default)]
pub struct RegistrationPlan {
    pub candidates: Vec<RegistrationCandidate>,
    pub failed: Vec<PathIssue>,
}

/// A project the registration request added or extended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddedProject {
    pub root: PathBuf,
    pub project: ProjectId,
    /// The root already existed as a removed project and was revived.
    pub revived: bool,
    /// A new scan scope was recorded for this project.
    pub scan_folder_added: bool,
}

/// Why a root in the request produced no catalog change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkipReason {
    /// The project and this exact scan scope were already registered.
    AlreadyRegistered,
    /// The same root and scope appeared earlier in this request.
    DuplicateInRequest,
}

impl SkipReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::AlreadyRegistered => "already registered",
            Self::DuplicateInRequest => "repeated in this request",
        }
    }
}

/// A root that was deliberately not registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedRoot {
    pub root: PathBuf,
    pub project: Option<ProjectId>,
    pub reason: SkipReason,
}

/// Per-root outcome of one multi-add registration request.
#[derive(Clone, Debug, Default)]
pub struct RegistrationSummary {
    pub added: Vec<AddedProject>,
    pub skipped: Vec<SkippedRoot>,
    pub failed: Vec<PathIssue>,
}

/// Counts and diagnostics of one committed reconciliation.
#[derive(Clone, Debug)]
pub struct ReconcileSummary {
    pub project: ProjectId,
    pub scope: PathBuf,
    pub coverage: Coverage,
    pub continuity: RootContinuity,
    /// False when nothing was written (cancelled scan, unavailable root).
    pub applied: bool,
    pub cancelled: bool,
    /// Entries the snapshot offered inside the registered scope.
    pub observed: usize,
    pub files_created: usize,
    pub files_updated: usize,
    pub memberships_added: usize,
    pub memberships_refreshed: usize,
    pub memberships_pruned: usize,
    /// In-scope memberships the prune pass kept (the file still exists, its stat
    /// failed, or its spelling is unresolved). Zero when the prune pass did not
    /// run at all — `prune_suppressed` says why.
    pub memberships_retained: usize,
    pub prune_suppressed: Option<String>,
    pub issues: Vec<PathIssue>,
}

/// Parsed Launcher version: the raw digits, the integer they spell and their
/// digit count (padding), e.g. `v003` → `("003", 3, 3)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionInfo {
    pub text: String,
    pub number: Option<i64>,
    pub padding: usize,
}

/// One catalog file belonging to a project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectFile {
    pub file: FileId,
    pub path: Option<PathBuf>,
    pub display: Option<String>,
    /// `None` when the native file name is not valid UTF-8; the path bytes above
    /// still identify it.
    pub filename: Option<String>,
    pub version: Option<VersionInfo>,
    pub filetype: Option<String>,
    pub size_bytes: Option<u64>,
    pub modified_unix_ns: Option<i64>,
    pub last_seen_at: Option<String>,
}

/// Files of one project that share a timeline, a shot and a version-group base
/// name. `files` is ordered newest-first, so `latest` is the head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileGroup {
    /// Timeline folder below the topmost `Nuke` / `After Effects` anchor, empty
    /// when the files sit in the shot folder directly. `None` when the folder
    /// name is not valid UTF-8.
    pub timeline: Option<String>,
    /// Shot folder immediately below the timeline. `None` when the folder name is
    /// not valid UTF-8; the group is still separate from every other bucket.
    pub shot: Option<String>,
    /// Group base spelling; `None` when the base name is not valid UTF-8.
    pub base_name: Option<String>,
    pub filetype: Option<String>,
    pub files: Vec<ProjectFile>,
    pub latest: Option<FileId>,
}

/// What a removal unregistered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemovalSummary {
    pub project: ProjectId,
    pub memberships_removed: usize,
    pub favorites_removed: usize,
    pub recents_removed: usize,
}

/// Validates registration roots without touching the catalog.
///
/// Run this on a worker thread (the caller's preworker), never in a UI frame:
/// it stats and opens each directory. Existence, directory shape and
/// readability are checked here so the catalog transaction only inserts what the
/// filesystem confirmed. A folder named exactly `Projects` contributes its
/// parent as identity root and itself as the scan scope; every other folder is
/// both. Existing catalog rows are not consulted — an already registered root is
/// skipped later, and an existing offline project keeps its record.
pub fn plan_registration(roots: &[PathBuf]) -> RegistrationPlan {
    let mut plan = RegistrationPlan::default();
    for root in roots {
        match plan_one_root(root) {
            Ok(candidate) => plan.candidates.push(candidate),
            Err(issue) => plan.failed.push(issue),
        }
    }
    plan
}

fn plan_one_root(root: &Path) -> Result<RegistrationCandidate, PathIssue> {
    if !root.is_absolute() {
        return Err(PathIssue::new(
            root,
            PathCategory::InvalidInput,
            "registration root must be an absolute path",
        ));
    }
    inspect_root(root).map_err(issue_from_path_error)?;
    if let Err(error) = fs::read_dir(root) {
        let category = if error.kind() == ErrorKind::PermissionDenied {
            PathCategory::Permission
        } else {
            PathCategory::Unavailable
        };
        return Err(PathIssue::new(
            root,
            category,
            format!("registration root cannot be listed: {error}"),
        ));
    }
    let (identity_root, scan_folder) = if is_literal_projects_folder(root) {
        let parent = root.parent().filter(|value| !value.as_os_str().is_empty());
        match parent {
            Some(parent) => {
                inspect_root(parent).map_err(issue_from_path_error)?;
                (parent.to_path_buf(), root.to_path_buf())
            }
            None => {
                return Err(PathIssue::new(
                    root,
                    PathCategory::InvalidInput,
                    "a 'Projects' folder needs its project parent, which this path does not have",
                ));
            }
        }
    } else {
        (root.to_path_buf(), root.to_path_buf())
    };
    let name = match identity_root.file_name().and_then(|value| value.to_str()) {
        Some(text) if !text.trim().is_empty() => text.to_string(),
        _ => {
            return Err(PathIssue::new(
                &identity_root,
                PathCategory::Unsupported,
                "the project folder name is not valid UTF-8 text; rename the folder or register its parent",
            ));
        }
    };
    Ok(RegistrationCandidate {
        identity_root,
        scan_folder,
        name,
    })
}

/// True for the literal legacy folder name the scanner treated specially
/// (`os.path.basename(project_path) == "Projects"`).
fn is_literal_projects_folder(root: &Path) -> bool {
    root.file_name()
        .is_some_and(|name| name.as_encoded_bytes() == b"Projects")
}

/// Registers every validated candidate in one catalog transaction and reports
/// per-root added/skipped/failed results.
///
/// `user` must be a live identity: it becomes `projects.added_by`. Rejecting the
/// caller rolls back the whole request, while per-root path problems (reported
/// in `plan.failed`) never block the other roots.
pub fn register_projects(
    catalog: &Catalog,
    user: UserId,
    plan: RegistrationPlan,
) -> StorageResult<Pending<RegistrationSummary>> {
    catalog.transaction(move |transaction| register_projects_tx(transaction, user, plan))
}

fn register_projects_tx(
    transaction: &Transaction<'_>,
    user: UserId,
    plan: RegistrationPlan,
) -> StorageResult<RegistrationSummary> {
    require_live_user(transaction, user)?;
    let now = utc_timestamp_now();
    let mut summary = RegistrationSummary {
        added: Vec::new(),
        skipped: Vec::new(),
        failed: plan.failed,
    };
    let mut seen: HashSet<(PathKey, PathKey)> = HashSet::new();
    for candidate in plan.candidates {
        let (root_key, root_columns) = match native_columns(&candidate.identity_root) {
            Ok(value) => value,
            Err(issue) => {
                summary.failed.push(issue);
                continue;
            }
        };
        let (scope_key, scope_columns) = match native_columns(&candidate.scan_folder) {
            Ok(value) => value,
            Err(issue) => {
                summary.failed.push(issue);
                continue;
            }
        };
        if !seen.insert((root_key.clone(), scope_key.clone())) {
            summary.skipped.push(SkippedRoot {
                root: candidate.scan_folder,
                project: project_id_by_root(transaction, &root_key)?,
                reason: SkipReason::DuplicateInRequest,
            });
            continue;
        }
        let existing = project_by_root(transaction, &root_key)?;
        match existing {
            None => {
                let id = insert_project(transaction, &candidate.name, &root_columns, user, &now)?;
                insert_scan_folder(transaction, id, &scope_columns, &now)?;
                summary.added.push(AddedProject {
                    root: candidate.scan_folder,
                    project: id,
                    revived: false,
                    scan_folder_added: true,
                });
            }
            Some((id, Some(_removed_at))) => {
                transaction
                    .execute(
                        "UPDATE projects SET removed_at = NULL WHERE id = ?1",
                        params![id],
                    )
                    .map_err(|error| sql_error("could not revive the removed project", error))?;
                let added_folder =
                    insert_scan_folder_if_absent(transaction, id, &scope_columns, &now)?;
                summary.added.push(AddedProject {
                    root: candidate.scan_folder,
                    project: id,
                    revived: true,
                    scan_folder_added: added_folder,
                });
            }
            Some((id, None)) => {
                if scan_folder_registered(transaction, id, &scope_key)? {
                    summary.skipped.push(SkippedRoot {
                        root: candidate.scan_folder,
                        project: Some(id),
                        reason: SkipReason::AlreadyRegistered,
                    });
                } else {
                    insert_scan_folder(transaction, id, &scope_columns, &now)?;
                    summary.added.push(AddedProject {
                        root: candidate.scan_folder,
                        project: id,
                        revived: false,
                        scan_folder_added: true,
                    });
                }
            }
        }
    }
    Ok(summary)
}

/// Lists live projects for `user` in the requested view, filtered by a
/// case-folded substring over the project name and root spelling.
///
/// Ordering is deterministic: name (case-folded, then exact bytes, then id) for
/// `All`/`Favorites`, and access time descending with the same tie-break for
/// `Recents`. A deleted user may still list projects; only writes require a live
/// identity.
pub fn list_projects(
    catalog: &Catalog,
    user: UserId,
    list: ProjectList,
    filter: Option<String>,
) -> StorageResult<Pending<Vec<Project>>> {
    catalog.submit(move |connection| {
        let mut projects = read_live_projects(connection, user)?;
        if let Some(filter) = &filter {
            let needle = fold_native_text(filter.as_bytes());
            if !needle.is_empty() {
                projects.retain(|project| {
                    contains_folded(&project.name, &needle)
                        || project
                            .root_display
                            .as_deref()
                            .is_some_and(|text| contains_folded(text, &needle))
                });
            }
        }
        match list {
            ProjectList::Favorites => projects.retain(|project| project.favorite),
            ProjectList::Recents => {
                projects.retain(|project| project.accessed_at.is_some());
                projects.sort_by(|a, b| {
                    let left = a.accessed_at.as_deref().unwrap_or("");
                    let right = b.accessed_at.as_deref().unwrap_or("");
                    right
                        .cmp(left)
                        .then_with(|| name_order(a, b))
                        .then_with(|| a.id.cmp(&b.id))
                });
            }
            ProjectList::All => {}
        }
        if list != ProjectList::Recents {
            projects.sort_by(|a, b| name_order(a, b).then_with(|| a.id.cmp(&b.id)));
        }
        Ok(projects)
    })
}

/// Registered scan scopes of one live project. Scanning only these roots is what
/// keeps discovery from widening beyond what the user registered.
pub fn scan_scopes(catalog: &Catalog, project: ProjectId) -> StorageResult<Pending<ScanScopes>> {
    catalog.submit(move |connection| {
        require_live_project(connection, project)?;
        let root = project_by_id(connection, project)?;
        let mut folders = Vec::new();
        let mut unresolved = Vec::new();
        for columns in scan_folder_columns(connection, project)? {
            match columns.decode() {
                Some(path) => folders.push(path),
                None => unresolved.push(columns.display.clone().unwrap_or_else(|| {
                    format!(
                        "<unresolved {} spelling, {} bytes>",
                        columns.platform,
                        columns.spelling.len()
                    )
                })),
            }
        }
        Ok(ScanScopes {
            project,
            root,
            folders,
            unresolved,
        })
    })
}

/// Adds or removes the user's favorite for one live project and returns the
/// resulting state.
pub fn set_favorite(
    catalog: &Catalog,
    user: UserId,
    project: ProjectId,
    favorite: bool,
) -> StorageResult<Pending<bool>> {
    catalog.transaction(move |transaction| {
        require_live_user(transaction, user)?;
        require_live_project(transaction, project)?;
        if favorite {
            transaction
                .execute(
                    "INSERT INTO favorites (user_id, project_id, created_at) VALUES (?1, ?2, ?3) ON CONFLICT(user_id, project_id) DO NOTHING",
                    params![user, project, utc_timestamp_now()],
                )
                .map_err(|error| sql_error("could not add the favorite", error))?;
        } else {
            transaction
                .execute(
                    "DELETE FROM favorites WHERE user_id = ?1 AND project_id = ?2",
                    params![user, project],
                )
                .map_err(|error| sql_error("could not remove the favorite", error))?;
        }
        Ok(favorite)
    })
}

/// Records that `user` opened the project and returns the new access timestamp.
pub fn mark_recent(
    catalog: &Catalog,
    user: UserId,
    project: ProjectId,
) -> StorageResult<Pending<String>> {
    catalog.transaction(move |transaction| {
        require_live_user(transaction, user)?;
        require_live_project(transaction, project)?;
        let now = utc_timestamp_now();
        transaction
            .execute(
                "INSERT INTO recents (user_id, project_id, accessed_at) VALUES (?1, ?2, ?3) ON CONFLICT(user_id, project_id) DO UPDATE SET accessed_at = excluded.accessed_at",
                params![user, project, now],
            )
            .map_err(|error| sql_error("could not record the recent project", error))?;
        Ok(now)
    })
}

/// Records that `user` opened `file`.
pub fn log_file_access(
    catalog: &Catalog,
    user: UserId,
    file: FileId,
) -> StorageResult<Pending<()>> {
    catalog.transaction(move |transaction| {
        require_live_user(transaction, user)?;
        transaction
            .execute(
                "INSERT INTO file_access_log (file_id, user_id, accessed_at) VALUES (?1, ?2, ?3)",
                params![file, user, utc_timestamp_now()],
            )
            .map_err(|error| sql_error("could not record the file access", error))?;
        Ok(())
    })
}

/// Unregisters a project without touching its folder.
///
/// One transaction stamps `removed_at`, deletes the project's memberships, its
/// favorites and its recents, and writes one activity row for the acting user.
/// Shared files, their history and any other project's membership stay intact,
/// and no media is deleted.
pub fn remove_project(
    catalog: &Catalog,
    user: UserId,
    project: ProjectId,
) -> StorageResult<Pending<RemovalSummary>> {
    catalog.transaction(move |transaction| {
        let username = require_live_user(transaction, user)?;
        let name = require_live_project(transaction, project)?;
        let now = utc_timestamp_now();
        let memberships_removed = transaction
            .execute(
                "DELETE FROM project_files WHERE project_id = ?1",
                params![project],
            )
            .map_err(|error| sql_error("could not unregister the project files", error))?;
        let favorites_removed = transaction
            .execute(
                "DELETE FROM favorites WHERE project_id = ?1",
                params![project],
            )
            .map_err(|error| sql_error("could not clear the project favorites", error))?;
        let recents_removed = transaction
            .execute("DELETE FROM recents WHERE project_id = ?1", params![project])
            .map_err(|error| sql_error("could not clear the recent entry", error))?;
        transaction
            .execute(
                "UPDATE projects SET removed_at = ?2 WHERE id = ?1 AND removed_at IS NULL",
                params![project, now],
            )
            .map_err(|error| sql_error("could not unregister the project", error))?;
        transaction
            .execute(
                "INSERT INTO user_activity_log (user_id, username, action_type, description, at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    user,
                    username,
                    "project_removed",
                    format!("Removed project {name:?} from the catalog"),
                    now
                ],
            )
            .map_err(|error| sql_error("could not record the removal", error))?;
        Ok(RemovalSummary {
            project,
            memberships_removed,
            favorites_removed,
            recents_removed,
        })
    })
}

/// Commits one discovery snapshot into the catalog for `project`.
///
/// Requirements checked before any write, each with a readable cause on failure:
/// the project is live, `scope` is one of its registered scan folders, the
/// snapshot's declared root is that same scope, and the snapshot purpose is
/// `ScanPurpose::Projects`. Anything else is refused, so a forged or foreign
/// snapshot cannot widen the catalog.
///
/// Then:
/// - a `Cancelled` coverage or an already-cancelled [`JobControl`] writes nothing
///   (`applied == false`, transaction rolled back), matching the legacy scanner,
///   which skipped the database update on cancellation;
/// - `RootUnavailable` writes nothing;
/// - otherwise observed files and memberships are upserted in this transaction,
///   files lexically outside the scope are reported and skipped, and other
///   projects' memberships are never touched;
/// - a cancellation observed *after* those writes also rolls them back: the
///   summary then reports `cancelled`, `applied == false` and zero mutation
///   counts, so no caller can mistake discarded work for committed work;
/// - pruning runs only with `Complete` coverage, `Proven` continuity, the scan
///   root's identity re-proven against the snapshot immediately before pruning,
///   and a definite `NotFound` per file. Any other stat error retains the
///   membership.
pub fn reconcile_scan(
    catalog: &Catalog,
    project: ProjectId,
    scope: PathBuf,
    snapshot: ScanSnapshot,
    control: JobControl,
) -> StorageResult<Pending<ReconcileSummary>> {
    catalog.transaction_or_rollback(move |transaction| {
        let (summary, keep) = reconcile_scan_tx(transaction, project, &scope, &snapshot, &control)?;
        Ok(if keep {
            TransactionOutcome::Commit(summary)
        } else {
            TransactionOutcome::Rollback(summary)
        })
    })
}

/// Returns the summary and whether the transaction may commit.
///
/// A cancellation observed at any boundary — including after the writes already
/// ran — returns `false`, so the catalog keeps nothing and the caller still
/// receives a readable cancellation result rather than a failure.
fn reconcile_scan_tx(
    transaction: &Transaction<'_>,
    project: ProjectId,
    scope: &Path,
    snapshot: &ScanSnapshot,
    control: &JobControl,
) -> StorageResult<(ReconcileSummary, bool)> {
    require_live_project(transaction, project)?;
    let (scope_key, _) = native_columns(scope).map_err(issue_to_error)?;
    let registered: Vec<PathKey> = scan_folder_columns(transaction, project)?
        .into_iter()
        .filter_map(|columns| columns.decode())
        .filter_map(|path| native_columns(&path).ok().map(|(key, _)| key))
        .collect();
    if !registered.iter().any(|key| *key == scope_key) {
        return Err(StorageError::new(
            StorageCategory::InvalidInput,
            Some(scope),
            format!("the scan root is not a registered scan folder of project {project}"),
        ));
    }
    let (snapshot_key, _) = native_columns(&snapshot.root).map_err(issue_to_error)?;
    if snapshot_key != scope_key {
        return Err(StorageError::new(
            StorageCategory::InvalidInput,
            Some(&snapshot.root),
            format!(
                "the snapshot root does not match the registered scan scope of project {project}; refusing to reconcile it"
            ),
        ));
    }
    if snapshot.purpose != ScanPurpose::Projects {
        return Err(StorageError::new(
            StorageCategory::InvalidInput,
            Some(&snapshot.root),
            "the snapshot was produced for a different scan purpose; project reconciliation needs the project preset",
        ));
    }

    let mut summary = ReconcileSummary {
        project,
        scope: scope.to_path_buf(),
        coverage: snapshot.coverage,
        continuity: snapshot.continuity,
        applied: false,
        cancelled: false,
        observed: 0,
        files_created: 0,
        files_updated: 0,
        memberships_added: 0,
        memberships_refreshed: 0,
        memberships_pruned: 0,
        memberships_retained: 0,
        prune_suppressed: None,
        issues: Vec::new(),
    };

    if snapshot.coverage == Coverage::Cancelled || control.is_cancelled() {
        return Ok((
            cancelled_summary(
                summary,
                "the scan was cancelled; observed files were not written",
            ),
            false,
        ));
    }
    if snapshot.coverage == Coverage::RootUnavailable {
        summary.prune_suppressed =
            Some("the root was unavailable; observed files were not written".to_string());
        return Ok((summary, true));
    }

    let observations = collect_observations(snapshot, &scope_key, &mut summary);
    summary.observed = observations.len();
    if control.is_cancelled() {
        return Ok((
            cancelled_summary(
                summary,
                "cancelled at the commit checkpoint; observed files were not written",
            ),
            false,
        ));
    }

    let now = utc_timestamp_now();
    let write_cancelled = apply_observations(
        transaction,
        project,
        &observations,
        &now,
        &mut summary,
        control,
    )?;
    if write_cancelled || control.is_cancelled() {
        return Ok((cancelled_summary(summary, ROLLED_BACK_REASON), false));
    }
    summary.applied = true;

    // `RootContinuity::Proven` is a claim made by the caller; pruning also needs the
    // continuity to still hold right now, re-proven from the live filesystem.
    let continuity_verified = snapshot.coverage == Coverage::Complete
        && snapshot.continuity == RootContinuity::Proven
        && snapshot.root_identity.as_ref().is_some_and(|identity| {
            inspect_root(scope)
                .is_ok_and(|current| current.compare_identity(identity) == Some(true))
        });
    if control.is_cancelled() {
        return Ok((cancelled_summary(summary, ROLLED_BACK_REASON), false));
    }
    let prune_allowed = snapshot.coverage == Coverage::Complete
        && snapshot.continuity == RootContinuity::Proven
        && continuity_verified;
    if !prune_allowed {
        summary.prune_suppressed = Some(
            match (snapshot.coverage, snapshot.continuity, continuity_verified) {
                (Coverage::Complete, RootContinuity::Proven, false) => {
                    "the root identity could not be re-proven immediately before pruning; unseen files were retained"
                }
                (Coverage::Complete, _, _) => {
                    "root continuity was not proven; unseen files were retained"
                }
                (Coverage::Partial, _, _) => {
                    "the scan was incomplete; unseen files were retained"
                }
                (Coverage::Cancelled | Coverage::RootUnavailable, _, _) => {
                    "no trusted coverage; unseen files were retained"
                }
            }
            .to_string(),
        );
        return Ok((summary, true));
    }
    let prune_cancelled = prune_absent_memberships(
        transaction,
        project,
        &scope_key,
        &observations,
        &mut summary,
        control,
    )?;
    if prune_cancelled || control.is_cancelled() {
        return Ok((cancelled_summary(summary, ROLLED_BACK_REASON), false));
    }
    Ok((summary, true))
}

const ROLLED_BACK_REASON: &str =
    "the scan was cancelled during the catalog write; nothing was kept";

/// A cancellation that rolls the transaction back. The mutation counters describe
/// work the rollback discarded, so they are cleared: `applied == false` must never
/// be read next to counts that look committed.
fn cancelled_summary(mut summary: ReconcileSummary, reason: &str) -> ReconcileSummary {
    summary.cancelled = true;
    summary.applied = false;
    summary.files_created = 0;
    summary.files_updated = 0;
    summary.memberships_added = 0;
    summary.memberships_refreshed = 0;
    summary.memberships_pruned = 0;
    summary.memberships_retained = 0;
    summary.prune_suppressed = Some(reason.to_string());
    summary
}

/// One file to upsert: its catalog columns and its derived display metadata.
struct Observation {
    columns: PathColumns,
    filename: String,
    filetype: Option<String>,
    size_bytes: Option<i64>,
    modified_unix_ns: Option<i64>,
    version: Option<ParsedVersion>,
}

fn collect_observations(
    snapshot: &ScanSnapshot,
    scope_key: &PathKey,
    summary: &mut ReconcileSummary,
) -> Vec<Observation> {
    let mut seen: HashSet<PathKey> = HashSet::new();
    let mut observations = Vec::with_capacity(snapshot.entries.len());
    for entry in &snapshot.entries {
        let (key, columns) = match native_columns(&entry.path) {
            Ok(value) => value,
            Err(issue) => {
                summary.issues.push(issue);
                continue;
            }
        };
        if !within(scope_key, &key) {
            summary.issues.push(PathIssue::new(
                &entry.path,
                PathCategory::Escape,
                "the observed path is outside the registered scan scope; it was not recorded",
            ));
            continue;
        }
        if !seen.insert(key.clone()) {
            continue;
        }
        let filename = entry
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();
        if filename.is_empty() {
            summary.issues.push(PathIssue::new(
                &entry.path,
                PathCategory::Unsupported,
                "the file name is not valid UTF-8 text; the record keeps its native path and no display name",
            ));
        }
        let version = parse_version(&filename);
        observations.push(Observation {
            columns,
            filename,
            filetype: filetype_of(&entry.path),
            size_bytes: size_column(entry.stamp.size, &entry.path, summary),
            modified_unix_ns: modified_column(entry.stamp.modified, &entry.path, summary),
            version,
        });
    }
    observations
}

/// A file size that does not fit the catalog's 64-bit signed column is reported
/// against its path instead of being wrapped or silently dropped. The observation
/// keeps no size, so the update's `COALESCE` retains any previously known value.
fn size_column(size: u64, path: &Path, summary: &mut ReconcileSummary) -> Option<i64> {
    match i64::try_from(size) {
        Ok(value) => Some(value),
        Err(_) => {
            summary.issues.push(PathIssue::new(
                path,
                PathCategory::Unsupported,
                format!(
                    "the file size ({size} bytes) is outside the catalog's signed 64-bit range; no size was recorded"
                ),
            ));
            None
        }
    }
}

/// The modification time is optional at the source, and a representable
/// nanosecond value is required: an unrepresentable one is reported against its
/// path rather than truncated, and the update keeps the previously known value.
fn modified_column(
    modified: Option<SystemTime>,
    path: &Path,
    summary: &mut ReconcileSummary,
) -> Option<i64> {
    let modified = modified?;
    match unix_nanos(modified) {
        Ok(value) => Some(value),
        Err(_) => {
            summary.issues.push(PathIssue::new(
                path,
                PathCategory::Unsupported,
                "the modification time is not representable as signed 64-bit nanoseconds; no modification time was recorded",
            ));
            None
        }
    }
}

fn apply_observations(
    transaction: &Transaction<'_>,
    project: ProjectId,
    observations: &[Observation],
    now: &str,
    summary: &mut ReconcileSummary,
    control: &JobControl,
) -> StorageResult<bool> {
    let mut select_file = transaction
        .prepare("SELECT id FROM files WHERE platform = ?1 AND storage = ?2")
        .map_err(|error| sql_error("could not read the file catalog", error))?;
    let mut insert_file = transaction
        .prepare(
            "INSERT INTO files (storage, platform, spelling, display, filename, version, version_number, filetype, size_bytes, modified_unix_ns, modified_raw, first_seen_at, last_seen_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL, ?11, ?11)",
        )
        .map_err(|error| sql_error("could not prepare the file insert", error))?;
    let mut update_file = transaction
        .prepare(
            "UPDATE files SET filename = COALESCE(NULLIF(?2, ''), filename), version = COALESCE(?3, version), version_number = COALESCE(?4, version_number), filetype = COALESCE(?5, filetype), size_bytes = COALESCE(?6, size_bytes), modified_unix_ns = COALESCE(?7, modified_unix_ns), last_seen_at = ?8 WHERE id = ?1",
        )
        .map_err(|error| sql_error("could not prepare the file update", error))?;
    let mut select_membership = transaction
        .prepare("SELECT 1 FROM project_files WHERE project_id = ?1 AND file_id = ?2")
        .map_err(|error| sql_error("could not read the project membership", error))?;
    let mut upsert_membership = transaction
        .prepare(
            "INSERT INTO project_files (project_id, file_id, added_at, last_seen_at) VALUES (?1, ?2, ?3, ?3) ON CONFLICT(project_id, file_id) DO UPDATE SET last_seen_at = excluded.last_seen_at",
        )
        .map_err(|error| sql_error("could not prepare the membership insert", error))?;

    for observation in observations {
        // Cancellation is checked between items only; `checkpoint()` would block
        // while paused, which would stall every other catalog request.
        if control.is_cancelled() {
            return Ok(true);
        }
        let storage = observation
            .columns
            .storage
            .as_deref()
            .expect("native observations always carry a storage key");
        let version_text = observation
            .version
            .as_ref()
            .map(|value| value.text.as_str());
        let version_number = observation.version.as_ref().and_then(|value| value.number);
        let existing: Option<FileId> = select_file
            .query_row(params![observation.columns.platform, storage], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|error| sql_error("could not look up the file record", error))?;
        let file_id = match existing {
            Some(id) => {
                update_file
                    .execute(params![
                        id,
                        observation.filename,
                        version_text,
                        version_number,
                        observation.filetype,
                        observation.size_bytes,
                        observation.modified_unix_ns,
                        now
                    ])
                    .map_err(|error| sql_error("could not update the file record", error))?;
                summary.files_updated += 1;
                id
            }
            None => {
                insert_file
                    .execute(params![
                        storage,
                        observation.columns.platform,
                        observation.columns.spelling,
                        observation.columns.display,
                        observation.filename,
                        version_text,
                        version_number,
                        observation.filetype,
                        observation.size_bytes,
                        observation.modified_unix_ns,
                        now
                    ])
                    .map_err(|error| sql_error("could not record the file", error))?;
                summary.files_created += 1;
                transaction.last_insert_rowid()
            }
        };
        let known = select_membership
            .query_row(params![project, file_id], |_| Ok(()))
            .optional()
            .map_err(|error| sql_error("could not read the project membership", error))?
            .is_some();
        upsert_membership
            .execute(params![project, file_id, now])
            .map_err(|error| sql_error("could not record the project membership", error))?;
        if known {
            summary.memberships_refreshed += 1;
        } else {
            summary.memberships_added += 1;
        }
    }
    Ok(control.is_cancelled())
}

fn prune_absent_memberships(
    transaction: &Transaction<'_>,
    project: ProjectId,
    scope_key: &PathKey,
    observations: &[Observation],
    summary: &mut ReconcileSummary,
    control: &JobControl,
) -> StorageResult<bool> {
    let mut observed: HashSet<FileId> = HashSet::with_capacity(observations.len());
    {
        let mut statement = transaction
            .prepare("SELECT id FROM files WHERE platform = ?1 AND storage = ?2")
            .map_err(|error| sql_error("could not read the file catalog", error))?;
        for observation in observations {
            let Some(storage) = observation.columns.storage.as_deref() else {
                continue;
            };
            let id: Option<FileId> = statement
                .query_row(params![observation.columns.platform, storage], |row| {
                    row.get(0)
                })
                .optional()
                .map_err(|error| sql_error("could not read the file record", error))?;
            if let Some(id) = id {
                observed.insert(id);
            }
        }
    }

    let mut candidates: Vec<(FileId, PathBuf)> = Vec::new();
    {
        let mut statement = transaction
            .prepare(
                "SELECT pf.file_id, f.storage, f.platform, f.spelling FROM project_files pf JOIN files f ON f.id = pf.file_id WHERE pf.project_id = ?1 ORDER BY pf.file_id",
            )
            .map_err(|error| sql_error("could not read the project files", error))?;
        let rows = statement
            .query_map(params![project], |row| {
                Ok((
                    row.get::<_, FileId>(0)?,
                    row.get::<_, Option<Vec<u8>>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })
            .map_err(|error| sql_error("could not read the project files", error))?;
        for row in rows {
            let (file_id, storage, platform, spelling) =
                row.map_err(|error| sql_error("could not read a project file", error))?;
            if observed.contains(&file_id) {
                continue;
            }
            let columns = PathColumns {
                storage,
                platform: platform_name(&platform)?,
                spelling,
                display: None,
            };
            match columns.decode() {
                None => summary.memberships_retained += 1,
                Some(path) => match native_columns(&path) {
                    Ok((key, _)) if within(scope_key, &key) => candidates.push((file_id, path)),
                    Ok(_) => {}
                    Err(_) => summary.memberships_retained += 1,
                },
            }
        }
    }

    let mut delete = transaction
        .prepare("DELETE FROM project_files WHERE project_id = ?1 AND file_id = ?2")
        .map_err(|error| sql_error("could not prepare the membership removal", error))?;
    for (file_id, path) in candidates {
        if control.is_cancelled() {
            return Ok(true);
        }
        match fs::metadata(&path) {
            Ok(_) => {
                summary.memberships_retained += 1;
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                delete
                    .execute(params![project, file_id])
                    .map_err(|error| sql_error("could not remove the membership", error))?;
                summary.memberships_pruned += 1;
            }
            Err(error) => {
                let category = if error.kind() == ErrorKind::PermissionDenied {
                    PathCategory::Permission
                } else {
                    PathCategory::Unavailable
                };
                summary.issues.push(PathIssue::new(
                    &path,
                    category,
                    format!("the file could not be verified as absent; its membership was retained ({error})"),
                ));
                summary.memberships_retained += 1;
            }
        }
    }
    Ok(control.is_cancelled())
}

/// Lists one project's files grouped by timeline, shot and version base name,
/// newest first.
pub fn list_project_files(
    catalog: &Catalog,
    project: ProjectId,
    kind: Option<FileKind>,
) -> StorageResult<Pending<Vec<FileGroup>>> {
    catalog.submit(move |connection| {
        require_live_project(connection, project)?;
        let mut files = read_project_files(connection, project)?;
        files.retain(|file| {
            !file
                .path
                .as_deref()
                .is_some_and(crate::discovery::is_project_autosave)
        });
        if let Some(kind) = kind {
            files.retain(|file| kind.matches(file.filetype.as_deref()));
        }
        Ok(group_files(files))
    })
}

fn read_project_files(
    connection: &Connection,
    project: ProjectId,
) -> StorageResult<Vec<ProjectFile>> {
    let mut statement = connection
        .prepare(
            "SELECT f.id, f.storage, f.platform, f.spelling, f.display, f.filename, f.version, f.version_number, f.filetype, f.size_bytes, f.modified_unix_ns, f.last_seen_at FROM project_files pf JOIN files f ON f.id = pf.file_id WHERE pf.project_id = ?1 ORDER BY f.id",
        )
        .map_err(|error| sql_error("could not read the project files", error))?;
    let rows = statement
        .query_map(params![project], |row| {
            Ok((
                row.get::<_, FileId>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<String>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, Option<String>>(11)?,
            ))
        })
        .map_err(|error| sql_error("could not read the project files", error))?;
    let mut files = Vec::new();
    for row in rows {
        let (
            file,
            storage,
            platform,
            spelling,
            display,
            filename,
            version,
            version_number,
            filetype,
            size_bytes,
            modified_unix_ns,
            last_seen_at,
        ) = row.map_err(|error| sql_error("could not read a project file", error))?;
        let columns = PathColumns {
            storage,
            platform: platform_name(&platform)?,
            spelling,
            display: display.clone(),
        };
        files.push(ProjectFile {
            file,
            path: columns.decode(),
            display,
            filename: Some(filename).filter(|text| !text.is_empty()),
            version: version.map(|text| version_info(text, version_number)),
            filetype,
            size_bytes: size_bytes.and_then(|value| u64::try_from(value).ok()),
            modified_unix_ns,
            last_seen_at,
        });
    }
    Ok(files)
}

/// Groups files by base name and orders each group newest-first.
fn group_files(files: Vec<ProjectFile>) -> Vec<FileGroup> {
    let mut groups: HashMap<(GroupBucket, Vec<u8>), Vec<ProjectFile>> = HashMap::new();
    for file in files {
        let name_bytes: Vec<u8> = match file.path.as_deref().and_then(Path::file_name) {
            Some(name) => name.as_encoded_bytes().to_vec(),
            None => file
                .filename
                .as_deref()
                .map(|text| text.as_bytes().to_vec())
                .unwrap_or_default(),
        };
        let bucket = match file.path.as_deref() {
            Some(path) => bucket_of(path),
            // An unresolved spelling has no folder chain to anchor on; it keeps its
            // own bucket so it never merges into a scanned timeline/shot group.
            None => GroupBucket {
                timeline: Vec::new(),
                shot: fallback_shot(&[], &name_bytes),
            },
        };
        groups
            .entry((bucket, base_name_bytes(&name_bytes)))
            .or_default()
            .push(file);
    }
    let mut ordered: Vec<((GroupBucket, Vec<u8>), Vec<ProjectFile>)> = groups.into_iter().collect();
    ordered.sort_by(|a, b| group_order(&a.0).cmp(&group_order(&b.0)));
    ordered
        .into_iter()
        .map(|((bucket, base), mut files)| {
            files.sort_by(|a, b| {
                let left = version_order(a);
                let right = version_order(b);
                right
                    .0
                    .cmp(&left.0)
                    .then_with(|| right.1.cmp(&left.1))
                    .then_with(|| file_name_order(a).cmp(&file_name_order(b)))
                    .then_with(|| a.file.cmp(&b.file))
            });
            let latest = files.first().map(|file| file.file);
            FileGroup {
                timeline: String::from_utf8(bucket.timeline).ok(),
                shot: String::from_utf8(bucket.shot).ok(),
                base_name: String::from_utf8(base).ok(),
                filetype: files.first().and_then(|file| file.filetype.clone()),
                files,
                latest,
            }
        })
        .collect()
}

/// The timeline/shot bucket the legacy tree built for a file. Grouping keys use
/// the native bytes so two different non-UTF-8 folder names never merge; only the
/// display spellings in [`FileGroup`] are optional text.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct GroupBucket {
    timeline: Vec<u8>,
    shot: Vec<u8>,
}

/// Timeline and shot for one path, exactly as
/// `ui_slick/file_handlers.py::group_by_timeline` derives them:
/// `<anchor>/<timeline>/<shot>/.../file.nk`, where the anchor is the **topmost**
/// `Nuke` / `After Effects` folder that still yields a timeline+shot pair. Extra
/// nesting below the shot (artist folders such as `<shot>/nuke/script`) is
/// deliberately ignored, so it never becomes a bucket of its own.
fn bucket_of(path: &Path) -> GroupBucket {
    let mut parts: Vec<&[u8]> = Vec::new();
    for component in path.components() {
        if let Component::Normal(name) = component {
            parts.push(name.as_encoded_bytes());
        }
    }
    // The last component is the file itself.
    if !parts.is_empty() {
        parts.pop();
    }

    for needed in [2usize, 1] {
        for anchor in 0..parts.len() {
            if !is_scan_anchor(parts[anchor]) {
                continue;
            }
            let relative = &parts[anchor + 1..];
            if relative.len() < needed {
                continue;
            }
            return if relative.len() >= 2 {
                GroupBucket {
                    timeline: relative[0].to_vec(),
                    shot: relative[1].to_vec(),
                }
            } else {
                GroupBucket {
                    timeline: Vec::new(),
                    shot: relative[0].to_vec(),
                }
            };
        }
    }

    let parent = parts.last().copied().unwrap_or_default();
    let file_name = path
        .file_name()
        .map(|name| name.as_encoded_bytes())
        .unwrap_or_default();
    GroupBucket {
        timeline: Vec::new(),
        shot: fallback_shot(parent, file_name),
    }
}

/// Folder names the legacy code treated as a project tool root.
fn is_scan_anchor(name: &[u8]) -> bool {
    let folded = fold_native_text(name);
    folded == b"nuke" || folded == b"after effects" || folded == b"aftereffects"
}

/// Shot name when no anchor produced one: the immediate parent folder, unless that
/// folder is a plumbing/tool name — then the filename decides, as in
/// `group_by_shot`. The deeper artist folder is never consulted.
fn fallback_shot(parent: &[u8], file_name: &[u8]) -> Vec<u8> {
    const PLUMBING: [&[u8]; 8] = [
        b"nk",
        b"aep",
        b"shots",
        b"assets",
        b"nuke",
        b"after effects",
        b"script",
        b"scripts",
    ];
    let folder = fold_native_text(parent);
    let mut shot = if folder.is_empty() || PLUMBING.contains(&folder.as_slice()) {
        let base = match file_name.windows(2).position(|window| window == b"_v") {
            Some(index) => &file_name[..index],
            None => file_name,
        };
        shot_code(base).unwrap_or_else(|| {
            base.split(|byte| *byte == b'_')
                .next()
                .unwrap_or_default()
                .to_vec()
        })
    } else {
        parent.to_vec()
    };
    if shot.is_empty() {
        shot = b"Main".to_vec();
    }
    shot
}

/// The legacy `((?:SH|sh)[_-]?\d{2,3})` shot-code search: the first `SH`/`sh`
/// followed by an optional separator and two or three digits.
fn shot_code(base: &[u8]) -> Option<Vec<u8>> {
    let mut index = 0;
    while index + 2 < base.len() {
        if matches!(&base[index..index + 2], b"SH" | b"sh") {
            let mut cursor = index + 2;
            if matches!(base.get(cursor), Some(b'_') | Some(b'-')) {
                cursor += 1;
            }
            let mut digits = 0;
            while digits < 3 && base.get(cursor + digits).is_some_and(u8::is_ascii_digit) {
                digits += 1;
            }
            if digits >= 2 {
                return Some(base[index..cursor + digits].to_vec());
            }
        }
        index += 1;
    }
    None
}

/// Timeline, then shot, then base name — each case-folded first and then compared
/// byte-exactly, so the order is stable and never case-dependent.
fn group_order(key: &(GroupBucket, Vec<u8>)) -> [Vec<u8>; 6] {
    let (bucket, base) = key;
    [
        fold_native_text(&bucket.timeline),
        bucket.timeline.clone(),
        fold_native_text(&bucket.shot),
        bucket.shot.clone(),
        fold_native_text(base),
        base.clone(),
    ]
}

fn version_order(file: &ProjectFile) -> (i64, i64) {
    (
        file.version
            .as_ref()
            .and_then(|value| value.number)
            .unwrap_or(0),
        file.modified_unix_ns.unwrap_or(0),
    )
}

fn file_name_order(file: &ProjectFile) -> Vec<u8> {
    match file.path.as_deref().and_then(Path::file_name) {
        Some(name) => fold_native_text(name.as_encoded_bytes()),
        None => fold_native_text(file.filename.as_deref().unwrap_or_default().as_bytes()),
    }
}

fn read_live_projects(connection: &Connection, user: UserId) -> StorageResult<Vec<Project>> {
    let favorites = user_project_ids(
        connection,
        "SELECT project_id FROM favorites WHERE user_id = ?1",
        user,
    )?;
    let recents = user_recents(connection, user)?;
    let mut folders = scan_folders_by_project(connection)?;
    let mut statement = connection
        .prepare("SELECT id, name, root_storage, root_platform, root_spelling, root_display, added_by, added_at FROM projects WHERE removed_at IS NULL ORDER BY id")
        .map_err(|error| sql_error("could not read the project list", error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, ProjectId>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<UserId>>(6)?,
                row.get::<_, String>(7)?,
            ))
        })
        .map_err(|error| sql_error("could not read the project list", error))?;
    let mut projects = Vec::new();
    for row in rows {
        let (id, name, storage, platform, spelling, root_display, added_by, added_at) =
            row.map_err(|error| sql_error("could not read a project", error))?;
        let root = PathColumns {
            storage,
            platform: platform_name(&platform)?,
            spelling,
            display: root_display.clone(),
        }
        .decode();
        projects.push(Project {
            id,
            name,
            root,
            root_display,
            added_by,
            added_at,
            scan_folders: folders.remove(&id).unwrap_or_default(),
            favorite: favorites.contains(&id),
            accessed_at: recents.get(&id).cloned(),
        });
    }
    Ok(projects)
}

fn scan_folders_by_project(
    connection: &Connection,
) -> StorageResult<HashMap<ProjectId, Vec<ScanFolder>>> {
    let mut statement = connection
        .prepare("SELECT project_id, storage, platform, spelling, display FROM project_scan_folders ORDER BY project_id, id")
        .map_err(|error| sql_error("could not read the scan folders", error))?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, ProjectId>(0)?,
                row.get::<_, Option<Vec<u8>>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .map_err(|error| sql_error("could not read the scan folders", error))?;
    let mut folders: HashMap<ProjectId, Vec<ScanFolder>> = HashMap::new();
    for row in rows {
        let (project, storage, platform, spelling, display) =
            row.map_err(|error| sql_error("could not read a scan folder", error))?;
        let columns = PathColumns {
            storage,
            platform: platform_name(&platform)?,
            spelling,
            display: display.clone(),
        };
        folders.entry(project).or_default().push(ScanFolder {
            path: columns.decode(),
            display,
        });
    }
    Ok(folders)
}

fn scan_folder_columns(
    connection: &Connection,
    project: ProjectId,
) -> StorageResult<Vec<PathColumns>> {
    let mut statement = connection
        .prepare("SELECT storage, platform, spelling, display FROM project_scan_folders WHERE project_id = ?1 ORDER BY id")
        .map_err(|error| sql_error("could not read the project scan folders", error))?;
    let rows = statement
        .query_map(params![project], |row| {
            Ok((
                row.get::<_, Option<Vec<u8>>>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(|error| sql_error("could not read the project scan folders", error))?;
    let mut folders = Vec::new();
    for row in rows {
        let (storage, platform, spelling, display) =
            row.map_err(|error| sql_error("could not read a scan folder", error))?;
        folders.push(PathColumns {
            storage,
            platform: platform_name(&platform)?,
            spelling,
            display,
        });
    }
    Ok(folders)
}

fn user_project_ids(
    connection: &Connection,
    sql: &str,
    user: UserId,
) -> StorageResult<HashSet<ProjectId>> {
    let mut statement = connection
        .prepare(sql)
        .map_err(|error| sql_error("could not read the user project list", error))?;
    let rows = statement
        .query_map(params![user], |row| row.get::<_, ProjectId>(0))
        .map_err(|error| sql_error("could not read the user project list", error))?;
    let mut ids = HashSet::new();
    for row in rows {
        ids.insert(row.map_err(|error| sql_error("could not read a user project", error))?);
    }
    Ok(ids)
}

fn user_recents(
    connection: &Connection,
    user: UserId,
) -> StorageResult<HashMap<ProjectId, String>> {
    let mut statement = connection
        .prepare("SELECT project_id, accessed_at FROM recents WHERE user_id = ?1")
        .map_err(|error| sql_error("could not read the recent projects", error))?;
    let rows = statement
        .query_map(params![user], |row| {
            Ok((row.get::<_, ProjectId>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|error| sql_error("could not read the recent projects", error))?;
    let mut recents = HashMap::new();
    for row in rows {
        let (project, accessed_at) =
            row.map_err(|error| sql_error("could not read a recent project", error))?;
        recents.insert(project, accessed_at);
    }
    Ok(recents)
}

fn project_by_root(
    connection: &Connection,
    root_key: &PathKey,
) -> StorageResult<Option<(ProjectId, Option<String>)>> {
    connection
        .query_row(
            "SELECT id, removed_at FROM projects WHERE root_platform = ?1 AND root_storage = ?2",
            params![crate::storage::CURRENT_PLATFORM, root_key.storage_bytes()],
            |row| {
                Ok((
                    row.get::<_, ProjectId>(0)?,
                    row.get::<_, Option<String>>(1)?,
                ))
            },
        )
        .optional()
        .map_err(|error| sql_error("could not look up the project", error))
}

fn project_id_by_root(
    connection: &Connection,
    root_key: &PathKey,
) -> StorageResult<Option<ProjectId>> {
    Ok(project_by_root(connection, root_key)?.map(|(id, _)| id))
}

fn insert_project(
    connection: &Connection,
    name: &str,
    root: &PathColumns,
    user: UserId,
    now: &str,
) -> StorageResult<ProjectId> {
    connection
        .execute(
            "INSERT INTO projects (name, root_storage, root_platform, root_spelling, root_display, added_by, added_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                name,
                root.storage,
                root.platform,
                root.spelling,
                root.display,
                user,
                now
            ],
        )
        .map_err(|error| sql_error("could not register the project", error))?;
    Ok(connection.last_insert_rowid())
}

fn insert_scan_folder(
    connection: &Connection,
    project: ProjectId,
    folder: &PathColumns,
    now: &str,
) -> StorageResult<()> {
    connection
        .execute(
            "INSERT INTO project_scan_folders (project_id, storage, platform, spelling, display, added_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                project,
                folder.storage,
                folder.platform,
                folder.spelling,
                folder.display,
                now
            ],
        )
        .map_err(|error| sql_error("could not record the project scan folder", error))?;
    Ok(())
}

fn insert_scan_folder_if_absent(
    connection: &Connection,
    project: ProjectId,
    folder: &PathColumns,
    now: &str,
) -> StorageResult<bool> {
    let Some(key) = folder
        .decode()
        .and_then(|path| native_columns(&path).ok().map(|(key, _)| key))
    else {
        insert_scan_folder(connection, project, folder, now)?;
        return Ok(true);
    };
    if scan_folder_registered(connection, project, &key)? {
        return Ok(false);
    }
    insert_scan_folder(connection, project, folder, now)?;
    Ok(true)
}

fn scan_folder_registered(
    connection: &Connection,
    project: ProjectId,
    scope_key: &PathKey,
) -> StorageResult<bool> {
    let found: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM project_scan_folders WHERE project_id = ?1 AND platform = ?2 AND storage = ?3",
            params![
                project,
                crate::storage::CURRENT_PLATFORM,
                scope_key.storage_bytes()
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| sql_error("could not check the registered scan folder", error))?;
    Ok(found.is_some())
}

fn project_by_id(connection: &Connection, project: ProjectId) -> StorageResult<Option<PathBuf>> {
    let row: Option<(Option<Vec<u8>>, String, Vec<u8>, Option<String>)> = connection
        .query_row(
            "SELECT root_storage, root_platform, root_spelling, root_display FROM projects WHERE id = ?1 AND removed_at IS NULL",
            params![project],
            |row| {
                Ok((
                    row.get::<_, Option<Vec<u8>>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|error| sql_error("could not read the project", error))?;
    match row {
        None => Ok(None),
        Some((storage, platform, spelling, display)) => Ok(PathColumns {
            storage,
            platform: platform_name(&platform)?,
            spelling,
            display,
        }
        .decode()),
    }
}

fn require_live_project(connection: &Connection, project: ProjectId) -> StorageResult<String> {
    let row: Option<(String, Option<String>)> = connection
        .query_row(
            "SELECT name, removed_at FROM projects WHERE id = ?1",
            params![project],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(|error| sql_error("could not read the project", error))?;
    match row {
        Some((name, None)) => Ok(name),
        Some((_, Some(_))) => Err(StorageError::new(
            StorageCategory::InvalidInput,
            None,
            format!("project {project} was removed; register it again before changing it"),
        )),
        None => Err(StorageError::new(
            StorageCategory::InvalidInput,
            None,
            format!("project {project} does not exist"),
        )),
    }
}

fn require_live_user(connection: &Connection, user: UserId) -> StorageResult<String> {
    let row: Option<(String, Option<String>)> = connection
        .query_row(
            "SELECT username, deleted_at FROM users WHERE id = ?1",
            params![user],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()
        .map_err(|error| sql_error("could not read the selected user", error))?;
    match row {
        Some((username, None)) => Ok(username),
        Some((_, Some(_))) => Err(StorageError::new(
            StorageCategory::InvalidInput,
            None,
            format!("user {user} was deleted; select a live identity"),
        )),
        None => Err(StorageError::new(
            StorageCategory::InvalidInput,
            None,
            format!("user {user} does not exist"),
        )),
    }
}

fn native_columns(path: &Path) -> Result<(PathKey, PathColumns), PathIssue> {
    let key = path_key(path, path).map_err(issue_from_path_error)?;
    Ok((key.clone(), PathColumns::native(path, &key)))
}

fn issue_from_path_error(error: crate::paths::PathError) -> PathIssue {
    PathIssue::new(
        error.path().unwrap_or(Path::new("")),
        error.category(),
        error.message().to_string(),
    )
}

fn issue_to_error(issue: PathIssue) -> StorageError {
    StorageError::new(
        StorageCategory::InvalidInput,
        Some(&issue.path),
        issue.message,
    )
}

fn sql_error(context: &str, error: rusqlite::Error) -> StorageError {
    StorageError::new(
        StorageCategory::Database,
        None,
        format!("{context}: {error}"),
    )
}

fn platform_name(value: &str) -> StorageResult<&'static str> {
    match value {
        "posix" => Ok("posix"),
        "windows" => Ok("windows"),
        "unknown" => Ok("unknown"),
        other => Err(StorageError::new(
            StorageCategory::InvalidSchema,
            None,
            format!("the catalog contains an unknown path platform tag {other:?}"),
        )),
    }
}

/// Lexical containment only: `path` is `scope` or lies inside it.
fn within(scope: &PathKey, path: &PathKey) -> bool {
    matches!(
        relation(scope, path),
        PathRelation::Same | PathRelation::SecondInsideFirst
    )
}

fn name_order(a: &Project, b: &Project) -> std::cmp::Ordering {
    fold_native_text(a.name.as_bytes())
        .cmp(&fold_native_text(b.name.as_bytes()))
        .then_with(|| a.name.as_bytes().cmp(b.name.as_bytes()))
}

fn contains_folded(text: &str, needle: &[u8]) -> bool {
    let haystack = fold_native_text(text.as_bytes());
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn filetype_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_lowercase)
}

/// Nanoseconds since the Unix epoch, or the reason the value cannot be stored.
/// Nothing is truncated, wrapped or clamped: an unrepresentable time is reported
/// by the caller and keeps the previously known catalog value.
fn unix_nanos(time: SystemTime) -> Result<i64, ()> {
    match time.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => i64::try_from(elapsed.as_nanos()).map_err(|_| ()),
        Err(error) => i64::try_from(error.duration().as_nanos())
            .ok()
            .and_then(i64::checked_neg)
            .ok_or(()),
    }
}

struct ParsedVersion {
    text: String,
    number: Option<i64>,
}

fn version_from_digits(digits: &[u8]) -> ParsedVersion {
    let text = String::from_utf8_lossy(digits).into_owned();
    ParsedVersion {
        number: text.parse::<i64>().ok(),
        text,
    }
}

fn version_info(text: String, number: Option<i64>) -> VersionInfo {
    let all_digits = !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    VersionInfo {
        number: number.or_else(|| {
            if all_digits {
                text.parse::<i64>().ok()
            } else {
                None
            }
        }),
        padding: if all_digits { text.len() } else { 0 },
        text,
    }
}

/// Launcher preset: the first `_v<1-4 digits>` (case-insensitive); for `.aep`
/// names only, the legacy trailing-number fallback. No version is invented.
fn parse_version(name: &str) -> Option<ParsedVersion> {
    let bytes = name.as_bytes();
    let mut index = 0;
    while index + 2 <= bytes.len() {
        if bytes[index] == b'_' && is_version_marker(bytes[index + 1]) {
            let start = index + 2;
            let mut end = start;
            while end < bytes.len() && end - start < 4 && bytes[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                return Some(version_from_digits(&bytes[start..end]));
            }
        }
        index += 1;
    }
    if !ends_with_aep(bytes) {
        return None;
    }
    // Legacy `extract_version` searches the whole stem for the *last* digit run
    // (`(\d{1,5})(?!.*\d)`) and keeps at most its last five digits, so
    // `comp_12_final.aep` has version 12 — not only a trailing number.
    let stem = &bytes[..bytes.len() - 4];
    let mut end = stem.len();
    while end > 0 && !stem[end - 1].is_ascii_digit() {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    let mut start = end;
    while start > 0 && stem[start - 1].is_ascii_digit() {
        start -= 1;
    }
    let run = &stem[start..end];
    let take = run.len().min(5);
    Some(version_from_digits(&run[run.len() - take..]))
}

fn is_version_marker(byte: u8) -> bool {
    byte == b'v' || byte == b'V'
}

fn ends_with_aep(bytes: &[u8]) -> bool {
    bytes.len() > 4 && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".aep")
}

/// Version-group base name: every `_v<1-4 digits>` removed; when nothing was
/// removed and the name ends in `.aep`, the trailing number (at most five
/// digits) and any trailing `_`/`-` are removed, keeping the extension.
fn base_name_bytes(name: &[u8]) -> Vec<u8> {
    let mut stripped = Vec::with_capacity(name.len());
    let mut index = 0;
    let mut removed = false;
    while index < name.len() {
        if name[index] == b'_' && index + 2 <= name.len() && is_version_marker(name[index + 1]) {
            let start = index + 2;
            let mut end = start;
            while end < name.len() && end - start < 4 && name[end].is_ascii_digit() {
                end += 1;
            }
            if end > start {
                removed = true;
                index = end;
                continue;
            }
        }
        stripped.push(name[index]);
        index += 1;
    }
    if removed || !ends_with_aep(name) {
        return stripped;
    }
    let stem = &name[..name.len() - 4];
    let mut start = stem.len();
    while start > 0 && stem[start - 1].is_ascii_digit() {
        start -= 1;
    }
    if start == stem.len() {
        return stripped;
    }
    let removed_len = (stem.len() - start).min(5);
    let mut base = stem[..stem.len() - removed_len].to_vec();
    while matches!(base.last(), Some(b'_') | Some(b'-')) {
        base.pop();
    }
    base.extend_from_slice(&name[name.len() - 4..]);
    base
}
