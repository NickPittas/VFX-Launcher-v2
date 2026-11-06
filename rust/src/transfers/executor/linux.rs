//! X1b: the Linux transfer executor.
//!
//! Execution is journal-first. The reviewed intent is persisted before the
//! first mutation, every step is revalidated against the reviewed evidence, and
//! outcomes are appended afterwards. Installs are staged in an app-owned
//! sibling file that is flushed and checked, then committed with
//! `renameat2(NOREPLACE)`, so a destination that appeared after review is never
//! clobbered and an interrupted run never leaves a half-written destination
//! under its reviewed name.
//!
//! The only window that mutates an existing reviewed destination is the
//! displacement/install/cleanup sequence used by Replace and PreserveExisting.
//! Its exact intent - staged file, destination and displaced name - is journaled
//! before the first rename; the old file is then moved to that reserved sibling
//! without clobbering and proved to still be the entry this item read. Install
//! evidence follows the folder sync while that file is still present. Replace
//! journals a separate cleanup step and deletes only an entry that still matches
//! the held identity; PreserveExisting keeps the file and reports it as a
//! recovery path. A failed commit restores the old file only while the held
//! identity proves it, with a no-clobber rename that is synced afterwards.
//!
//! Deliberate limits of this packet:
//! - Linux only; other platforms keep the unsupported-operation boundary.
//! - Directory prerequisites run in order, then up to 16 scoped file workers
//!   share the one catalog queue and aggregate progress. Every worker is joined
//!   before finalization or failure returns; reports retain reviewed ordinals.
//! - Pause/cancel is cooperative between items and copy chunks. A blocked OS
//!   read/write is not interrupted.
//! - Identity comparison around a name-based rename or unlink is a fresh check,
//!   not an atomic one: correctness assumes cooperating writers in the selected
//!   directories and makes no hostile-race claim.
//! - Sources are resolved only beneath the declared source root, whose recorded
//!   identity must agree with the held handle before any item runs; an item
//!   source is never reopened by absolute path.
//! - Source removal for Move re-proves the directory entry and the copied handle
//!   after the `RemovingSource` checkpoint, but a crash between install and
//!   removal leaves both files; the run is then interrupted and inspectable,
//!   never replayed automatically.

use super::super::native::{EntryIdentity, Root};
use super::super::{
    DestinationEvidence, DestinationExpectation, ItemCheckpoint, ItemEffect, ItemState,
    ItemValidation, JournaledPlan, NativeIdentity, PlannedAction, RunSummary, RunTerminal,
    SourceStamp, StoredPath, TransferCategory, TransferError, ValidatedPlan,
    append_item_checkpoint, finalize_run, mark_run_running, nanos_from_time,
    persist_validated_intent, update_item_outcome,
};
use crate::jobs::{Checkpoint, JobControl, JobEvents, JobPhase, JobProgress};
use crate::storage::{CatalogClient, StorageError};
use rustix::{
    fd::{AsFd, OwnedFd},
    fs::{FileType, Mode, Timestamps},
    io::Errno,
    time::Timespec,
};
use std::{
    collections::{HashMap, HashSet},
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

/// Chunk size for staged copies. Cancellation and progress are checked between
/// chunks, so this is also the longest uninterruptible write of one item.
const COPY_CHUNK: usize = 1 << 20;
const STAGING_SUFFIX: &str = ".vfxlauncher-stage";
const REPLACE_SUFFIX: &str = ".vfxlauncher-replace";
const MAX_BACKUP_ATTEMPTS: u32 = 9_999;

/// One item's durable terminal outcome, as recorded in the journal.
#[derive(Clone, Debug)]
pub struct ItemOutcome {
    pub ordinal: i64,
    pub state: ItemState,
    /// Committed destination bytes; zero unless the destination was installed.
    pub committed_bytes: u64,
    pub message: Option<String>,
    pub recovery: Vec<PathBuf>,
}

/// Result of one executed run. Counts come from the persisted items.
#[derive(Clone, Debug)]
pub struct RunReport {
    pub run_id: i64,
    pub summary: RunSummary,
    pub items: Vec<ItemOutcome>,
}

/// Executes a validated plan, journaling intent before the first mutation.
///
/// The catalog is reached only through its request-only client, and every reply
/// is awaited on this worker thread before the next filesystem step, so the run
/// is never executed against evidence the journal has not recorded yet.
pub fn execute_with_workers(
    catalog: &CatalogClient,
    validated: ValidatedPlan,
    control: &JobControl,
    events: &JobEvents,
    workers: usize,
) -> Result<RunReport, TransferError> {
    let total_items = validated.items().len() as u64;
    let total_bytes = plan_bytes(validated.items());
    let evidence = validated.destination_evidence().clone();
    let destination_root = validated.destination_root().to_path_buf();

    let journal = awaited(persist_validated_intent(catalog, validated))?;
    let run_id = journal.run_id();
    (|| {
        awaited(mark_run_running(catalog, run_id))?;

        // Generated-only plans read nothing, so they never need a source root. A plan
        // that copies or moves must confine every source to a declared root whose
        // recorded identity agrees with the held handle before the first item runs.
        let reads_sources = journal
            .validated()
            .items()
            .iter()
            .any(|item| matches!(item.effect, ItemEffect::Copy | ItemEffect::Move));
        let (source_root, source_root_path) = if reads_sources {
            match hold_source_root(journal.validated()) {
                Ok((root, path)) => (Some(root), Some(path)),
                Err(reason) => {
                    settle_refused(catalog, &journal, &reason)?;
                    return Err(refused(&reason));
                }
            }
        } else {
            (None, None)
        };

        let mut executor = Executor {
            catalog,
            journal: &journal,
            control,
            events,
            root: match Root::open(destination_root.as_os_str()) {
                Ok(root) => root,
                Err(error) => {
                    let reason = format!(
                        "the reviewed destination root {} cannot be opened: {error}",
                        destination_root.display()
                    );
                    settle_refused(catalog, &journal, &reason)?;
                    return Err(refused(&reason));
                }
            },
            source_root,
            source_root_path,
            destination_root,
            blocked_directories: Arc::new(HashSet::new()),
            verified_folders: Arc::new(HashMap::new()),
            staging_counter: 0,
            protected_names: Arc::new(
                journal
                    .validated()
                    .items()
                    .iter()
                    .flat_map(|item| [item.destination.as_ref(), item.displaced.as_ref()])
                    .flatten()
                    .map(|path| path.path().to_path_buf())
                    .collect(),
            ),
            total_items,
            total_bytes,
            progress: Arc::new(Mutex::new(RunProgress::default())),
            outcomes: Vec::with_capacity(journal.validated().items().len()),
        };

        if let Err(reason) = executor.revalidate_destination_evidence(&evidence) {
            settle_refused(catalog, &journal, &reason)?;
            return Err(refused(&reason));
        }

        let items = journal.validated().items().to_vec();
        let mut cancelled = false;
        if workers == 1 {
            for item in &items {
                if control.checkpoint() == Checkpoint::Cancelled || executor.process_one(item)? {
                    cancelled = true;
                    break;
                }
            }
        } else {
            // Directory dependencies finish before independent file actions enter the pool.
            for item in items
                .iter()
                .filter(|item| item.effect == ItemEffect::CreateDirectory)
            {
                if control.checkpoint() == Checkpoint::Cancelled || executor.process_one(item)? {
                    cancelled = true;
                    break;
                }
            }
            if !cancelled {
                cancelled = executor.parallel_files(&items, workers)?;
            }
        }

        let terminal = if cancelled {
            // Every remaining reviewed item gets its own terminal outcome, so the
            // returned report and the persisted run name the same items.
            let finished = executor
                .outcomes
                .iter()
                .map(|outcome| outcome.ordinal)
                .collect::<HashSet<_>>();
            for item in items
                .iter()
                .filter(|item| !finished.contains(&(item.index as i64)))
            {
                executor.record(
                    item,
                    ItemState::Cancelled,
                    0,
                    Some("cancelled before this item was attempted".to_string()),
                    Vec::new(),
                )?;
            }
            if executor
                .outcomes
                .iter()
                .any(|outcome| outcome.state == ItemState::Cancelled)
            {
                RunTerminal::Cancelled
            } else {
                terminal_for(&executor.outcomes)
            }
        } else {
            terminal_for(&executor.outcomes)
        };
        executor.outcomes.sort_by_key(|outcome| outcome.ordinal);
        let summary = awaited(finalize_run(catalog, run_id, terminal))?;
        Ok(RunReport {
            run_id,
            summary,
            items: executor.outcomes,
        })
    })()
    .map_err(|error: TransferError| error.with_run_id(run_id))
}

/// Sum of the reviewed byte counts, or `None` while any reviewed item has no
/// count: an unknown total never becomes zero by assumption.
fn plan_bytes(items: &[ItemValidation]) -> Option<u64> {
    items
        .iter()
        .filter(|item| item.effect != ItemEffect::Skip)
        .try_fold(0u64, |total, item| total.checked_add(item.bytes?))
}

fn success_state(effect: ItemEffect) -> ItemState {
    match effect {
        ItemEffect::Copy => ItemState::Copied,
        ItemEffect::Move => ItemState::Moved,
        ItemEffect::CreateFile | ItemEffect::CreateDirectory => ItemState::Created,
        ItemEffect::Skip => ItemState::Skipped,
    }
}

/// Terminal run state implied by the persisted item outcomes. Reserve `Failed`
/// for a run with no successful work; mixed runs are `Partial`.
fn terminal_for(outcomes: &[ItemOutcome]) -> RunTerminal {
    let unsuccessful = outcomes
        .iter()
        .filter(|item| {
            matches!(
                item.state,
                ItemState::Failed | ItemState::CopiedSourceRetained
            )
        })
        .count();
    let completed = outcomes
        .iter()
        .filter(|item| {
            matches!(
                item.state,
                ItemState::Copied | ItemState::Moved | ItemState::Created | ItemState::Skipped
            )
        })
        .count();
    if unsuccessful == 0 {
        RunTerminal::Succeeded
    } else if completed > 0 {
        RunTerminal::Partial
    } else {
        RunTerminal::Failed
    }
}

/// Refuses a run that cannot be executed against its reviewed evidence and
/// settles it honestly: every item is recorded failed, then the run is failed.
fn settle_refused(
    catalog: &CatalogClient,
    journal: &JournaledPlan,
    reason: &str,
) -> Result<(), TransferError> {
    for item in journal.validated().items() {
        awaited(update_item_outcome(
            catalog,
            journal,
            item.index as i64,
            ItemState::Failed,
            0,
            Some(reason.to_string()),
            &[],
        ))?;
    }
    awaited(finalize_run(catalog, journal.run_id(), RunTerminal::Failed))?;
    Ok(())
}

fn refused(reason: &str) -> TransferError {
    TransferError::new(TransferCategory::Refused, None, reason)
}

fn storage_refused(error: StorageError) -> TransferError {
    TransferError::new(
        TransferCategory::Unavailable,
        None,
        format!("the operation journal could not be updated: {error}"),
    )
}

/// Submits one journal request and waits for its reply on this worker thread.
fn awaited<T>(
    pending: crate::storage::StorageResult<crate::storage::Pending<T>>,
) -> Result<T, TransferError> {
    pending
        .map_err(storage_refused)?
        .wait()
        .map_err(storage_refused)
}

/// How one item finished before its terminal state is chosen. A success carries
/// the exact paths it deliberately retained, so a kept original is reportable
/// recovery evidence rather than a silent leftover.
enum Done {
    Succeeded {
        note: Option<String>,
        recovery: Vec<PathBuf>,
    },
    SourceRetained {
        message: String,
        recovery: Vec<PathBuf>,
    },
}

/// Per-item flow. A `Problem` or `Cancelled` becomes that item's terminal
/// outcome; `Abort` stops the run because the journal itself is unusable.
enum Flow {
    Problem(String, Vec<PathBuf>),
    /// The destination was installed and journaled, then the result could not be
    /// verified. The reviewed bytes are committed, so the item fails without
    /// discarding or re-claiming them.
    PostInstall {
        message: String,
        recovery: Vec<PathBuf>,
    },
    Cancelled(Vec<PathBuf>),
    Abort(TransferError),
}

impl From<TransferError> for Flow {
    fn from(error: TransferError) -> Self {
        Self::Abort(error)
    }
}

/// Result of re-checking a source that a Move is about to delete.
enum SourceCheck {
    /// The entry still names the copied file, unchanged since review.
    Unchanged(EntryIdentity),
    /// A known difference from the review or the copied file; the source is kept.
    Changed(String),
    /// The entry or the copied handle could not be compared, so removing the
    /// source cannot be proven safe.
    Unknown(String),
}

/// What one commit established: the note to report and the exact paths it kept.
/// Durability is not a flag here: a refused folder sync is a post-install
/// failure, never a successful note.
struct InstallOutcome {
    note: Option<String>,
    recovery: Vec<PathBuf>,
}

/// The single staging file one item created, with the identity read from the
/// handle that created it. Cleanup uses that held identity only, and never
/// touches the name after it has been published.
struct Stage {
    file: File,
    name: OsString,
    path: PathBuf,
    identity: EntryIdentity,
    published: bool,
}

fn problem(message: impl Into<String>) -> Flow {
    Flow::Problem(message.into(), Vec::new())
}

fn io_problem(what: &str, path: &Path, error: io::Error) -> Flow {
    Flow::Problem(
        format!("{what} {} could not be used: {error}", path.display()),
        Vec::new(),
    )
}

/// Identity without the permission bits, matching the device/inode identity the
/// review and every later proof use. Staged files are chmodded after creation, so
/// their held identity's mode no longer describes the installed entry.
fn same_entry(left: EntryIdentity, right: EntryIdentity) -> bool {
    left.device == right.device && left.inode == right.inode
}

#[derive(Default)]
struct RunProgress {
    completed: u64,
    committed: u64,
    staging: HashMap<PathBuf, u64>,
}

struct Executor<'a> {
    catalog: &'a CatalogClient,
    journal: &'a JournaledPlan,
    control: &'a JobControl,
    events: &'a JobEvents,
    root: Root,
    /// The declared source root, opened and identity-proven once per run. `None`
    /// for generated-only plans, which read no source at all.
    source_root: Option<Root>,
    source_root_path: Option<PathBuf>,
    destination_root: PathBuf,
    /// Destination-relative folders whose creation failed; children of these are
    /// failed without being attempted, so a blocked prerequisite stays visible.
    blocked_directories: Arc<HashSet<PathBuf>>,
    /// Folders this run has proven unchanged: the reviewed ancestors captured
    /// while validating, plus the folders it creates. Every parent is compared
    /// with this evidence immediately before an item mutates it, so a folder
    /// swapped after the run-level check is never written into.
    verified_folders: Arc<HashMap<PathBuf, NativeIdentity>>,
    staging_counter: u64,
    protected_names: Arc<HashSet<PathBuf>>,
    total_items: u64,
    total_bytes: Option<u64>,
    progress: Arc<Mutex<RunProgress>>,
    outcomes: Vec<ItemOutcome>,
}

struct StopOnPanic<'a>(&'a AtomicBool);
impl Drop for StopOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.store(true, Ordering::Release);
        }
    }
}

impl<'run> Executor<'run> {
    fn fork(&self) -> Result<Executor<'run>, TransferError> {
        Ok(Executor {
            catalog: self.catalog,
            journal: self.journal,
            control: self.control,
            events: self.events,
            root: self.root.try_clone().map_err(|error| {
                refused(&format!(
                    "The destination handle could not be duplicated: {error}"
                ))
            })?,
            source_root: self
                .source_root
                .as_ref()
                .map(Root::try_clone)
                .transpose()
                .map_err(|error| {
                    refused(&format!(
                        "The source handle could not be duplicated: {error}"
                    ))
                })?,
            source_root_path: self.source_root_path.clone(),
            destination_root: self.destination_root.clone(),
            blocked_directories: self.blocked_directories.clone(),
            verified_folders: self.verified_folders.clone(),
            staging_counter: 0,
            protected_names: Arc::clone(&self.protected_names),
            total_items: self.total_items,
            total_bytes: self.total_bytes,
            progress: Arc::clone(&self.progress),
            outcomes: Vec::new(),
        })
    }

    fn parallel_files(
        &mut self,
        items: &[ItemValidation],
        workers: usize,
    ) -> Result<bool, TransferError> {
        let files = items
            .iter()
            .filter(|item| item.effect != ItemEffect::CreateDirectory)
            .collect::<Vec<_>>();
        let mut forks = Vec::new();
        for _ in 0..workers.min(files.len()) {
            forks.push(self.fork()?);
        }
        let next = AtomicUsize::new(0);
        let stop = AtomicBool::new(false);
        let (results, spawn_error) = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            let mut spawn_error = None;
            for (index, mut executor) in forks.into_iter().enumerate() {
                let files = &files;
                let next = &next;
                let stop = &stop;
                match std::thread::Builder::new()
                    .name(format!("vfx-copy-{}-{index}", self.journal.run_id()))
                    .spawn_scoped(scope, move || {
                        let _panic_guard = StopOnPanic(stop);
                        let mut cancelled = false;
                        loop {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            if executor.control.checkpoint() == Checkpoint::Cancelled {
                                cancelled = true;
                                stop.store(true, Ordering::Release);
                                break;
                            }
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(item) = files.get(index) else {
                                break;
                            };
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            match executor.process_one(item) {
                                Ok(true) => {
                                    cancelled = true;
                                    stop.store(true, Ordering::Release);
                                    break;
                                }
                                Ok(false) => {}
                                Err(error) => {
                                    stop.store(true, Ordering::Release);
                                    return Err(error);
                                }
                            }
                        }
                        Ok((executor.outcomes, cancelled))
                    }) {
                    Ok(handle) => handles.push(handle),
                    Err(error) => {
                        stop.store(true, Ordering::Release);
                        spawn_error =
                            Some(refused(&format!("A copy worker could not start: {error}")));
                        break;
                    }
                }
            }
            let results = handles.into_iter().map(|handle| handle.join().unwrap_or_else(|_| Err(refused("A copy worker stopped unexpectedly; inspect the journal before retrying.")))).collect::<Vec<_>>();
            (results, spawn_error)
        });
        let mut error = spawn_error;
        let mut cancelled = false;
        for result in results {
            match result {
                Ok((outcomes, was_cancelled)) => {
                    self.outcomes.extend(outcomes);
                    cancelled |= was_cancelled;
                }
                Err(problem) => {
                    if error.is_none() {
                        error = Some(problem);
                    }
                }
            }
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(cancelled)
    }

    fn process_one(&mut self, item: &ItemValidation) -> Result<bool, TransferError> {
        match self.run(item) {
            Ok(Done::Succeeded { note, recovery }) => {
                let state = success_state(item.effect);
                let bytes = item.bytes.unwrap_or(0);
                self.record(item, state, bytes, note, recovery)?;
            }
            Ok(Done::SourceRetained { message, recovery }) => {
                self.record(
                    item,
                    ItemState::CopiedSourceRetained,
                    item.bytes.unwrap_or(0),
                    Some(message),
                    recovery,
                )?;
            }
            Err(Flow::Problem(message, recovery)) => {
                self.note_failure(item);
                self.events.diagnostic(
                    JobPhase::Copying,
                    format!("item {} failed: {message}", item.index + 1),
                );
                self.record(item, ItemState::Failed, 0, Some(message), recovery)?;
            }
            Err(Flow::PostInstall { message, recovery }) => {
                self.note_failure(item);
                self.events.diagnostic(
                    JobPhase::Copying,
                    format!(
                        "item {} installed but was not verified: {message}",
                        item.index + 1
                    ),
                );
                // The reviewed bytes are already committed to the destination;
                // the journal accepts them only after an install checkpoint.
                self.record(
                    item,
                    ItemState::Failed,
                    item.bytes.unwrap_or(0),
                    Some(message),
                    recovery,
                )?;
            }
            Err(Flow::Cancelled(recovery)) => {
                self.record(
                    item,
                    ItemState::Cancelled,
                    0,
                    Some("cancelled before the destination was installed".to_string()),
                    recovery,
                )?;
                return Ok(true);
            }
            Err(Flow::Abort(error)) => return Err(error),
        }
        Ok(false)
    }

    // -- journal -------------------------------------------------------------

    /// Appends one durable terminal outcome. Failures here abort the run and
    /// leave it inspectable as interrupted; they are never swallowed.
    fn record(
        &mut self,
        item: &ItemValidation,
        state: ItemState,
        committed_bytes: u64,
        message: Option<String>,
        recovery: Vec<PathBuf>,
    ) -> Result<(), TransferError> {
        let ordinal = item.index as i64;
        awaited(update_item_outcome(
            self.catalog,
            self.journal,
            ordinal,
            state,
            committed_bytes,
            message.clone(),
            &recovery,
        ))?;
        self.outcomes.push(ItemOutcome {
            ordinal,
            state,
            committed_bytes,
            message,
            recovery,
        });
        {
            let mut progress = self
                .progress
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            progress.completed += 1;
            progress.committed = progress.committed.saturating_add(committed_bytes);
            if let Some(destination) = &item.destination {
                progress.staging.remove(destination.path());
            }
        }
        self.emit_progress(None);
        Ok(())
    }

    fn checkpoint(
        &self,
        item: &ItemValidation,
        step: ItemCheckpoint,
        message: Option<String>,
        recovery: &[PathBuf],
    ) -> Result<(), TransferError> {
        awaited(append_item_checkpoint(
            self.catalog,
            self.journal,
            item.index as i64,
            step,
            message,
            recovery,
        ))
    }

    fn emit_progress(&self, current: Option<PathBuf>) {
        let progress = self
            .progress
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.events.progress(
            JobPhase::Copying,
            JobProgress {
                completed: progress.completed,
                total: Some(self.total_items),
                bytes_done: progress
                    .staging
                    .values()
                    .fold(progress.committed, |sum, bytes| sum.saturating_add(*bytes)),
                bytes_total: self.total_bytes,
                current,
            },
        );
    }

    fn note_failure(&mut self, item: &ItemValidation) {
        if item.effect != ItemEffect::CreateDirectory {
            return;
        }
        if let Some(destination) = item.destination.as_ref().map(StoredPath::path) {
            if let Ok(relative) = destination.strip_prefix(&self.destination_root) {
                Arc::make_mut(&mut self.blocked_directories).insert(relative.to_path_buf());
            }
        }
    }

    // -- evidence ------------------------------------------------------------

    /// Revalidates the reviewed destination root and every reviewed ancestor
    /// against the identity snapshot captured while validating. Missing or
    /// incomparable evidence refuses the run; it is never treated as agreement.
    /// Each proof is kept for the per-item revalidation `open_parent` performs.
    fn revalidate_destination_evidence(
        &mut self,
        evidence: &DestinationEvidence,
    ) -> Result<(), String> {
        let root_identity = identity_of_handle(&self.root)
            .map_err(|error| format!("the destination root can no longer be inspected: {error}"))?;
        require_same_identity(
            evidence.root_identity,
            Some(root_identity),
            "the destination root",
        )?;
        Arc::make_mut(&mut self.verified_folders).insert(PathBuf::new(), root_identity);
        for ancestor in &evidence.ancestors {
            let path = ancestor.path.path();
            let relative = path.strip_prefix(&self.destination_root).map_err(|_| {
                format!(
                    "reviewed destination folder {} is outside the reviewed root",
                    path.display()
                )
            })?;
            let handle = if relative.as_os_str().is_empty() {
                self.root.duplicate()
            } else {
                self.root.open_dir(relative.as_os_str())
            }
            .map_err(|error| {
                format!(
                    "reviewed destination folder {} cannot be reopened: {error}",
                    path.display()
                )
            })?;
            let fresh = identity_of_handle(&handle).map_err(|error| {
                format!(
                    "reviewed destination folder {} cannot be inspected: {error}",
                    path.display()
                )
            })?;
            require_same_identity(
                ancestor.identity,
                Some(fresh),
                &format!("reviewed destination folder {}", path.display()),
            )?;
            Arc::make_mut(&mut self.verified_folders).insert(relative.to_path_buf(), fresh);
        }
        Ok(())
    }

    // -- items ---------------------------------------------------------------

    fn run(&mut self, item: &ItemValidation) -> Result<Done, Flow> {
        if let Some(message) = self.blocked_parent(item)? {
            return Err(problem(message));
        }
        match item.effect {
            ItemEffect::Skip => Ok(Done::Succeeded {
                note: None,
                recovery: Vec::new(),
            }),
            ItemEffect::CreateDirectory => self.create_directory(item),
            ItemEffect::CreateFile => self.create_file(item),
            ItemEffect::Copy | ItemEffect::Move => self.transfer_file(item),
        }
    }

    fn destination<'a>(&self, item: &'a ItemValidation) -> Result<&'a Path, Flow> {
        item.destination
            .as_ref()
            .map(StoredPath::path)
            .ok_or_else(|| problem("reviewed item has no destination"))
    }

    fn relative<'a>(&self, destination: &'a Path) -> Result<&'a Path, Flow> {
        destination
            .strip_prefix(&self.destination_root)
            .map_err(|_| {
                problem(format!(
                    "destination {} is outside the reviewed root",
                    destination.display()
                ))
            })
    }

    fn blocked_parent(&self, item: &ItemValidation) -> Result<Option<String>, Flow> {
        let destination = self.destination(item)?;
        let relative = self.relative(destination)?;
        let mut prefix = PathBuf::new();
        let mut components = relative.components().peekable();
        while let Some(component) = components.next() {
            if components.peek().is_none() {
                break;
            }
            prefix.push(component.as_os_str());
            if self.blocked_directories.contains(&prefix) {
                return Ok(Some(format!(
                    "folder {} could not be created, so this item was not attempted",
                    self.destination_root.join(&prefix).display()
                )));
            }
        }
        Ok(None)
    }

    /// Opens one destination folder and proves it is the very folder this run
    /// verified, so an item never mutates a folder swapped in after that check.
    /// A folder this run has not proven is refused instead of trusted by name.
    fn open_parent(&self, parent_relative: &Path) -> io::Result<OwnedFd> {
        let mut handle = self.verified_folder(Path::new(""))?;
        let mut prefix = PathBuf::new();
        for component in parent_relative.components() {
            if !matches!(component, Component::Normal(_)) {
                return Err(io::Error::new(
                    ErrorKind::InvalidInput,
                    "destination parent must be relative",
                ));
            }
            prefix.push(component.as_os_str());
            handle = self.verified_folder(&prefix)?;
        }
        Ok(handle)
    }

    fn verified_folder(&self, parent_relative: &Path) -> io::Result<OwnedFd> {
        let handle = if parent_relative.as_os_str().is_empty() {
            self.root.duplicate()
        } else {
            self.root.open_dir(parent_relative.as_os_str())
        }?;
        let expected = self.verified_folders.get(parent_relative).copied();
        let fresh = identity_of_handle(&handle);
        let proven = match (expected, fresh) {
            (Some(expected), Ok(fresh)) if expected.same(&fresh) == Some(true) => {
                return Ok(handle);
            }
            (Some(_), Ok(_)) => "is no longer the folder this run verified".to_string(),
            (Some(_), Err(error)) => return Err(error),
            (None, _) => "was never verified for this run, so it is not written into".to_string(),
        };
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!(
                "{} {proven}",
                self.destination_root.join(parent_relative).display()
            ),
        ))
    }

    /// Resolves one reviewed source beneath the source root held for this run.
    /// The root identity proved at run start is what makes this spelling
    /// trustworthy, so a source is never reopened by its absolute path; every
    /// folder is reached through the held handle with the native no-follow,
    /// beneath-root, no-cross-device resolver.
    fn source_entry(&self, source: &Path) -> Result<(OwnedFd, OsString), Flow> {
        let root_path = self.source_root_path.as_deref().ok_or_else(|| {
            problem(format!(
                "source {} has no reviewed source root",
                source.display()
            ))
        })?;
        let root = self.source_root.as_ref().ok_or_else(|| {
            problem(format!(
                "source {} has no source root held for this run",
                source.display()
            ))
        })?;
        let relative = source.strip_prefix(root_path).map_err(|_| {
            problem(format!(
                "source {} is outside the reviewed source root {}",
                source.display(),
                root_path.display()
            ))
        })?;
        if relative.as_os_str().is_empty() {
            return Err(problem(format!(
                "source {} is the reviewed source root itself",
                source.display()
            )));
        }
        if !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        {
            return Err(problem(format!(
                "source {} is not a plain descendant of the reviewed source root {}",
                source.display(),
                root_path.display()
            )));
        }
        let name = relative
            .file_name()
            .ok_or_else(|| problem(format!("source {} has no file name", source.display())))?
            .to_os_string();
        let parent_relative = relative.parent().unwrap_or(Path::new(""));
        let parent = if parent_relative.as_os_str().is_empty() {
            root.duplicate()
        } else {
            root.open_dir(parent_relative.as_os_str())
        }
        .map_err(|error| {
            io_problem("the source folder", &root_path.join(parent_relative), error)
        })?;
        Ok((parent, name))
    }

    /// Removes the one staging file this item created, closing the held handle
    /// first. The only identity ever used is the one captured when that handle
    /// was created: a name that now points elsewhere is a mismatch to report, not
    /// an entry to adopt. A published stage name is never touched again.
    ///
    /// Returns a visible note when the file cannot be shown to be gone: a failed
    /// item must not hide junk it left.
    fn discard_stage(&self, parent: &OwnedFd, stage: Stage) -> Option<String> {
        let Stage {
            file,
            name,
            path,
            mut identity,
            published,
        } = stage;
        if published {
            return None;
        }
        // Permission preservation changes mode, not ownership. Read mode only
        // from our still-held creating handle, never from the cleanup pathname.
        match rustix::fs::fstat(&file) {
            Ok(stat) if same_entry(EntryIdentity::from(stat), identity) => {
                identity.mode = stat.st_mode
            }
            _ => {
                return Some(format!(
                    "The staging file {} was retained because its creating handle could not be verified.",
                    path.display()
                ));
            }
        }
        drop(file);
        match self.root.remove_owned(parent, &name, identity) {
            Ok(()) => None,
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => Some(format!(
                "the staging file {} was retained: {error}",
                path.display()
            )),
        }
    }

    /// Discards the staging file of a step that failed, keeping the reason
    /// visible: appended to a diagnostic outcome, or emitted when the journal
    /// cannot carry it. A journal failure mutates nothing and names the exact
    /// path that was left behind.
    fn abandon_stage(&self, parent: &OwnedFd, stage: Stage, flow: Flow) -> Flow {
        if let Flow::Abort(error) = flow {
            if !stage.published {
                self.events.diagnostic(
                    JobPhase::Copying,
                    format!(
                        "the journal failed, so the staging file {} was retained",
                        stage.path.display()
                    ),
                );
            }
            return Flow::Abort(error);
        }
        let stage_path = stage.path.clone();
        let note = match self.discard_stage(parent, stage) {
            Some(note) => note,
            None => return flow,
        };
        let retain = |recovery: Vec<PathBuf>| {
            let mut recovery = recovery;
            if !recovery.contains(&stage_path) {
                recovery.push(stage_path.clone());
            }
            recovery
        };
        match flow {
            Flow::Problem(message, recovery) => {
                Flow::Problem(format!("{message}; {note}"), retain(recovery))
            }
            Flow::PostInstall { message, recovery } => Flow::PostInstall {
                message: format!("{message}; {note}"),
                recovery: retain(recovery),
            },
            Flow::Cancelled(recovery) => {
                self.events.diagnostic(JobPhase::Copying, note);
                Flow::Cancelled(retain(recovery))
            }
            Flow::Abort(error) => Flow::Abort(error),
        }
    }

    // -- folders -------------------------------------------------------------

    fn create_directory(&mut self, item: &ItemValidation) -> Result<Done, Flow> {
        let destination = self.destination(item)?.to_path_buf();
        let relative = self.relative(&destination)?.to_path_buf();
        let parent_relative = relative.parent().unwrap_or(Path::new("")).to_path_buf();
        let name = relative
            .file_name()
            .ok_or_else(|| problem("folder destination has no name"))?
            .to_os_string();
        let parent = self
            .open_parent(&parent_relative)
            .map_err(|error| io_problem("the destination folder", &destination, error))?;
        match item.expectation.as_ref() {
            Some(DestinationExpectation::Absent) => match self.root.mkdir(&parent, &name) {
                Ok(created) => {
                    let created_error = |error: io::Error| {
                        Flow::Problem(
                            format!(
                                "folder {} was created but could not be verified or synced: {error}",
                                destination.display()
                            ),
                            vec![destination.clone()],
                        )
                    };
                    let identity = identity_of_handle(&created).map_err(&created_error)?;
                    // The new folder's own entry set becomes durable only when it is
                    // synced, so its children cannot outlive it after a crash.
                    self.root.sync_dir(&created).map_err(&created_error)?;
                    self.root.sync_dir(&parent).map_err(created_error)?;
                    Arc::make_mut(&mut self.verified_folders).insert(relative, identity);
                    Ok(Done::Succeeded {
                        note: None,
                        recovery: Vec::new(),
                    })
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => Err(problem(format!(
                    "folder {} appeared after review; it was left untouched",
                    destination.display()
                ))),
                Err(error) => Err(io_problem("the folder to create", &destination, error)),
            },
            Some(DestinationExpectation::Directory { identity }) => {
                // Add-to-existing: the reviewed folder is accepted only while it is
                // still the same directory.
                let handle = self
                    .root
                    .open_dir(relative.as_os_str())
                    .map_err(|error| io_problem("the reviewed folder", &destination, error))?;
                let fresh = identity_of_handle(&handle)
                    .map_err(|error| io_problem("the reviewed folder", &destination, error))?;
                require_same_identity(
                    *identity,
                    Some(fresh),
                    &format!("folder {}", destination.display()),
                )
                .map_err(|message| Flow::Problem(message, Vec::new()))?;
                Arc::make_mut(&mut self.verified_folders).insert(relative, fresh);
                Ok(Done::Succeeded {
                    note: None,
                    recovery: Vec::new(),
                })
            }
            _ => Err(problem(format!(
                "reviewed folder expectation for {} is unusable",
                destination.display()
            ))),
        }
    }

    // -- files ---------------------------------------------------------------

    /// Creates a generated file from the reviewed contents.
    fn create_file(&mut self, item: &ItemValidation) -> Result<Done, Flow> {
        let contents = match self.journal.validated().plan().actions.get(item.index) {
            Some(PlannedAction::CreateFile { contents, .. }) => contents.clone(),
            _ => return Err(problem("reviewed generated file has no contents")),
        };
        let destination = self.destination(item)?.to_path_buf();
        let relative = self.relative(&destination)?.to_path_buf();
        let parent_relative = relative.parent().unwrap_or(Path::new("")).to_path_buf();
        let name = relative
            .file_name()
            .ok_or_else(|| problem("file destination has no name"))?
            .to_os_string();
        let parent = self
            .open_parent(&parent_relative)
            .map_err(|error| io_problem("the destination folder", &destination, error))?;

        let mut stage = self.begin_staging(item, &parent, &name, &destination)?;
        if let Err(error) = stage.file.write_all(&contents) {
            let flow = io_problem("the staging file", &stage.path, error);
            return Err(self.abandon_stage(&parent, stage, flow));
        }
        if let Err(error) = stage.file.sync_all() {
            let flow = io_problem("the staging file", &stage.path, error);
            return Err(self.abandon_stage(&parent, stage, flow));
        }
        if let Err(error) =
            self.checkpoint(item, ItemCheckpoint::Staged, None, &[stage.path.clone()])
        {
            return Err(self.abandon_stage(&parent, stage, Flow::Abort(error)));
        }

        match self.install(item, &parent, &name, &mut stage, &destination) {
            Ok(installed) => Ok(Done::Succeeded {
                note: installed.note,
                recovery: installed.recovery,
            }),
            Err(flow) => Err(self.abandon_stage(&parent, stage, flow)),
        }
    }

    /// Copies or moves one source file into the destination root.
    fn transfer_file(&mut self, item: &ItemValidation) -> Result<Done, Flow> {
        let source = match item.source.as_ref() {
            Some(source) => source.path().to_path_buf(),
            None => return Err(problem("reviewed transfer has no source")),
        };
        let reviewed = match item.source_stamp.as_ref() {
            Some(stamp) => stamp.clone(),
            None => return Err(problem("reviewed transfer has no source stamp")),
        };
        let destination = self.destination(item)?.to_path_buf();
        let relative = self.relative(&destination)?.to_path_buf();
        let parent_relative = relative.parent().unwrap_or(Path::new("")).to_path_buf();
        let name = relative
            .file_name()
            .ok_or_else(|| problem("file destination has no name"))?
            .to_os_string();
        let parent = self
            .open_parent(&parent_relative)
            .map_err(|error| io_problem("the destination folder", &destination, error))?;

        let (source_parent, source_name) = self.source_entry(&source)?;
        let (mut source_file, source_identity) = self
            .root
            .open_regular(&source_parent, &source_name, false)
            .map_err(|error| io_problem("the source file", &source, error))?;
        let source_meta = source_file
            .metadata()
            .map_err(|error| io_problem("the source file", &source, error))?;
        verify_stamp(&source_meta, &reviewed, &source)
            .map_err(|message| Flow::Problem(message, Vec::new()))?;

        let mut stage = self.begin_staging(item, &parent, &name, &destination)?;
        let metadata_note = match self.fill_staging(
            &mut stage,
            &mut source_file,
            &source_meta,
            &reviewed,
            &source,
            &destination,
        ) {
            Ok(note) => note,
            Err(flow) => {
                return Err(self.abandon_stage(&parent, stage, flow));
            }
        };
        if let Err(error) =
            self.checkpoint(item, ItemCheckpoint::Staged, None, &[stage.path.clone()])
        {
            return Err(self.abandon_stage(&parent, stage, Flow::Abort(error)));
        }

        let installed = match self.install(item, &parent, &name, &mut stage, &destination) {
            Ok(installed) => installed,
            Err(flow) => {
                return Err(self.abandon_stage(&parent, stage, flow));
            }
        };
        let note = combine_notes(metadata_note, installed.note);
        let recovery = installed.recovery;

        if item.effect == ItemEffect::Move {
            let mut move_recovery = recovery.clone();
            for path in [&source, &destination] {
                if !move_recovery.contains(path) {
                    move_recovery.push(path.clone());
                }
            }
            let with_note = |message: String| match &note {
                Some(note) => format!("{note}; {message}"),
                None => message,
            };
            self.checkpoint(
                item,
                ItemCheckpoint::RemovingSource,
                note.clone(),
                &move_recovery,
            )?;
            let current = match self.recheck_source(
                &source_parent,
                &source_name,
                &source,
                &reviewed,
                &source_file,
                source_identity,
            ) {
                SourceCheck::Unchanged(identity) => identity,
                SourceCheck::Changed(message) => {
                    return Ok(Done::SourceRetained {
                        message: with_note(format!(
                            "{message}; the copy is installed and the source was kept instead of deleted"
                        )),
                        recovery: move_recovery,
                    });
                }
                SourceCheck::Unknown(message) => {
                    return Err(Flow::PostInstall {
                        message: with_note(format!(
                            "{message}; the installed copy at {} was kept and the source {} was left in place",
                            destination.display(),
                            source.display()
                        )),
                        recovery: move_recovery,
                    });
                }
            };
            if let Err(error) = self
                .root
                .remove_owned(&source_parent, &source_name, current)
            {
                return Ok(Done::SourceRetained {
                    message: with_note(format!(
                        "the copy is installed, but the source {} could not be removed: {error}",
                        source.display()
                    )),
                    recovery: move_recovery,
                });
            }
            if let Err(message) = self.sync_move_directories(&source_parent, &parent) {
                return Err(Flow::PostInstall {
                    message: with_note(format!(
                        "the destination is installed and the source was removed, but {message}"
                    )),
                    recovery: move_recovery,
                });
            }
        }
        Ok(Done::Succeeded { note, recovery })
    }

    /// Re-proves the source between the acknowledged `RemovingSource` checkpoint
    /// and the unlink. A no-follow stat of the directory entry and the metadata of
    /// the very handle that was copied must both still be the reviewed regular
    /// file, and must be the same file; a Move deletes nothing it cannot re-prove.
    fn recheck_source(
        &self,
        source_parent: &OwnedFd,
        source_name: &OsStr,
        source: &Path,
        reviewed: &SourceStamp,
        copied: &File,
        opened: EntryIdentity,
    ) -> SourceCheck {
        let entry = match self.root.entry_stat(source_parent, source_name) {
            Ok(entry) => entry,
            Err(error) => {
                return SourceCheck::Unknown(format!(
                    "the source entry {} could not be inspected after the install: {error}",
                    source.display()
                ));
            }
        };
        if let Err(message) = verify_stamp_stat(&entry, reviewed, source) {
            return SourceCheck::Changed(message);
        }
        let meta = match copied.metadata() {
            Ok(meta) => meta,
            Err(error) => {
                return SourceCheck::Unknown(format!(
                    "the copied source {} could no longer be inspected after the install: {error}",
                    source.display()
                ));
            }
        };
        if let Err(message) = verify_stamp(&meta, reviewed, source) {
            return SourceCheck::Changed(message);
        }
        let entry_identity = EntryIdentity::from(entry);
        if entry_identity.device != opened.device || entry_identity.inode != opened.inode {
            return SourceCheck::Changed(format!(
                "the source {} is no longer the file that was copied",
                source.display()
            ));
        }
        SourceCheck::Unchanged(entry_identity)
    }

    /// Copies the opened source into the staging file, preserves its metadata,
    /// flushes it, and revalidates the source afterwards.
    fn fill_staging(
        &mut self,
        stage: &mut Stage,
        source_file: &mut File,
        source_meta: &fs::Metadata,
        reviewed: &SourceStamp,
        source: &Path,
        destination: &Path,
    ) -> Result<Option<String>, Flow> {
        let mut buffer = vec![0u8; COPY_CHUNK];
        let mut written = 0u64;
        loop {
            if self.control.checkpoint() == Checkpoint::Cancelled {
                return Err(Flow::Cancelled(Vec::new()));
            }
            let read = source_file
                .read(&mut buffer)
                .map_err(|error| io_problem("the source file", source, error))?;
            if read == 0 {
                break;
            }
            stage
                .file
                .write_all(&buffer[..read])
                .map_err(|error| io_problem("the staging file", &stage.path, error))?;
            written += read as u64;
            if written > reviewed.size {
                return Err(problem(format!(
                    "source {} grew while it was copied; the staged file was discarded",
                    source.display()
                )));
            }
            self.progress
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .staging
                .insert(destination.to_path_buf(), written);
            self.emit_progress(Some(destination.to_path_buf()));
        }
        if written != reviewed.size {
            return Err(problem(format!(
                "source {} was read as {written} of {} reviewed bytes; the staged file was discarded",
                source.display(),
                reviewed.size
            )));
        }

        // Metadata preservation is best effort but never silent: the outcome
        // carries the reason when it fails.
        let mut note = None;
        if let Err(error) = rustix::fs::fchmod(
            &stage.file,
            Mode::from_raw_mode(source_meta_mode(source_meta)),
        ) {
            note = Some(format!(
                "copied, but the source permissions could not be preserved: {error}"
            ));
        }
        if let Err(error) = rustix::fs::futimens(&stage.file, &source_times(source_meta)) {
            let message =
                format!("copied, but the source modification time could not be preserved: {error}");
            note = Some(match note {
                Some(previous) => format!("{previous}; {message}"),
                None => message,
            });
        }
        stage
            .file
            .sync_all()
            .map_err(|error| io_problem("the staging file", &stage.path, error))?;

        let after = source_file
            .metadata()
            .map_err(|error| io_problem("the source file", source, error))?;
        verify_stamp(&after, reviewed, source)
            .map_err(|message| Flow::Problem(message, Vec::new()))?;

        Ok(note)
    }

    /// Commits the staged file without clobbering, displacing the reviewed
    /// existing destination first when the item asks for it.
    fn install(
        &mut self,
        item: &ItemValidation,
        parent: &OwnedFd,
        name: &OsStr,
        stage: &mut Stage,
        destination: &Path,
    ) -> Result<InstallOutcome, Flow> {
        match item.expectation.as_ref() {
            Some(DestinationExpectation::Absent) => {
                match self.root.publish_noreplace(parent, &stage.name, name) {
                    Ok(()) => {
                        stage.published = true;
                        if let Err(message) =
                            self.verify_installed(parent, name, stage, destination)
                        {
                            self.checkpoint(
                                item,
                                ItemCheckpoint::Installed,
                                Some(message.clone()),
                                &[destination.to_path_buf()],
                            )?;
                            return Err(Flow::PostInstall {
                                message,
                                recovery: vec![destination.to_path_buf()],
                            });
                        }
                        // Journal evidence follows the namespace sync, so the record
                        // never claims a durability the filesystem refused.
                        match self.namespace_sync(parent) {
                            Ok(()) => {
                                self.checkpoint(item, ItemCheckpoint::Installed, None, &[])?;
                                Ok(InstallOutcome {
                                    note: None,
                                    recovery: Vec::new(),
                                })
                            }
                            Err(reason) => {
                                self.checkpoint(
                                    item,
                                    ItemCheckpoint::Installed,
                                    Some(format!(
                                        "the reviewed bytes are installed at {}, but {reason}",
                                        destination.display()
                                    )),
                                    &[destination.to_path_buf()],
                                )?;
                                Err(Flow::PostInstall {
                                    message: format!(
                                        "the reviewed bytes were installed at {}, but {reason}, so the install is not confirmed durable",
                                        destination.display()
                                    ),
                                    recovery: vec![destination.to_path_buf()],
                                })
                            }
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                        Err(problem(format!(
                            "destination {} appeared after review; it was left untouched",
                            destination.display()
                        )))
                    }
                    Err(error) if no_clobber_unavailable(&error) => Err(problem(format!(
                        "this filesystem does not support a no-clobber commit ({error}); refusing to install {}",
                        destination.display()
                    ))),
                    Err(error) => Err(io_problem("the staging file", destination, error)),
                }
            }
            Some(DestinationExpectation::RegularFile { stamp }) => {
                let current = self
                    .root
                    .entry_stat(parent, name)
                    .map_err(|error| io_problem("the existing destination", destination, error))?;
                verify_existing_destination(&current, stamp, destination)
                    .map_err(|message| Flow::Problem(message, Vec::new()))?;
                // Held for every later proof: only the entry this item read is ever
                // restored or deleted, never whatever the name means afterwards.
                let old_identity = EntryIdentity::from(current);

                let (backup, keep) = match item.displaced.as_ref() {
                    Some(displaced) => {
                        let path = displaced.path();
                        if path.parent() != destination.parent() {
                            return Err(problem(format!(
                                "reviewed preservation target {} is not beside the destination",
                                path.display()
                            )));
                        }
                        let backup = path
                            .file_name()
                            .ok_or_else(|| problem("preservation target has no name"))?
                            .to_os_string();
                        (backup, true)
                    }
                    None => (self.unique_backup_name(parent, name, destination)?, false),
                };

                let backup_path = destination.with_file_name(&backup);
                let retained =
                    |backup: &Path| vec![destination.to_path_buf(), backup.to_path_buf()];

                // The exact displacement window is journaled before the first
                // rename: the staged file, the destination and the name the
                // reviewed original will occupy.
                self.checkpoint(
                    item,
                    ItemCheckpoint::Displacing,
                    None,
                    &[
                        stage.path.clone(),
                        destination.to_path_buf(),
                        backup_path.clone(),
                    ],
                )?;

                // The checkpoint is awaited, so the old file is proved again in
                // full - kind, size, mtime and identity - before it is moved. A
                // change after review or since the intent was recorded refuses the
                // item with both names untouched.
                let fresh = self
                    .root
                    .entry_stat(parent, name)
                    .map_err(|error| io_problem("the existing destination", destination, error))?;
                verify_existing_destination(&fresh, stamp, destination)
                    .map_err(|message| Flow::Problem(message, Vec::new()))?;
                if !same_entry(EntryIdentity::from(fresh), old_identity) {
                    return Err(problem(format!(
                        "destination {} is no longer the file this item journaled its displacement for; it was left untouched",
                        destination.display()
                    )));
                }

                if let Err(error) = self.root.rename_between(parent, name, parent, &backup) {
                    return Err(if error.kind() == ErrorKind::AlreadyExists {
                        problem(format!(
                            "the reviewed preservation target for {} appeared after review; the destination was left untouched",
                            destination.display()
                        ))
                    } else {
                        io_problem("the existing destination", destination, error)
                    });
                }
                // Displacement succeeded. From here the original is handled only
                // through the held identity, so a name that stopped meaning it is
                // reported instead of being restored or deleted.
                match self.root.entry_stat(parent, &backup) {
                    Ok(entry) if same_entry(EntryIdentity::from(entry), old_identity) => {}
                    Ok(_) => {
                        return Err(Flow::Problem(
                            format!(
                                "the reviewed destination was moved to {}, but that name no longer holds the file this item displaced; nothing was restored or deleted",
                                backup_path.display()
                            ),
                            retained(&backup_path),
                        ));
                    }
                    Err(error) => {
                        return Err(Flow::Problem(
                            format!(
                                "the reviewed destination was moved to {}, but it cannot be inspected: {error}; nothing was restored or deleted",
                                backup_path.display()
                            ),
                            retained(&backup_path),
                        ));
                    }
                }
                // A displacement that is not durable could lose both names in a
                // power loss, so the install waits for this sync.
                if let Err(reason) = self.namespace_sync(parent) {
                    return Err(Flow::Problem(
                        format!(
                            "the reviewed destination was moved to {}, but {reason}; nothing was restored or deleted",
                            backup_path.display()
                        ),
                        retained(&backup_path),
                    ));
                }
                match self.root.publish_noreplace(parent, &stage.name, name) {
                    Ok(()) => {
                        stage.published = true;
                        if let Err(message) =
                            self.verify_installed(parent, name, stage, destination)
                        {
                            self.checkpoint(
                                item,
                                ItemCheckpoint::Installed,
                                Some(message.clone()),
                                &retained(&backup_path),
                            )?;
                            return Err(Flow::PostInstall {
                                message,
                                recovery: retained(&backup_path),
                            });
                        }
                        // Evidence follows the folder sync while the displaced
                        // original is still beside the destination. A refused sync
                        // is recorded as content installed, not as durability.
                        match self.namespace_sync(parent) {
                            Ok(()) => {
                                self.checkpoint(item, ItemCheckpoint::Installed, None, &[])?
                            }
                            Err(reason) => {
                                self.checkpoint(
                                    item,
                                    ItemCheckpoint::Installed,
                                    Some(format!(
                                        "the reviewed bytes are installed at {}, but {reason}",
                                        destination.display()
                                    )),
                                    &retained(&backup_path),
                                )?;
                                return Err(Flow::PostInstall {
                                    message: format!(
                                        "the reviewed bytes were installed at {}, but {reason}, so the install is not confirmed durable; the previous destination is retained at {}",
                                        destination.display(),
                                        backup_path.display()
                                    ),
                                    recovery: retained(&backup_path),
                                });
                            }
                        }
                        if keep {
                            // PreserveExisting keeps the reviewed original at its
                            // numbered name and reports exactly where it stayed.
                            return Ok(InstallOutcome {
                                note: Some(format!(
                                    "installed; the previous destination was kept at {}",
                                    backup_path.display()
                                )),
                                recovery: vec![backup_path.clone()],
                            });
                        }
                        // Cleanup is an additive journaled step naming the exact
                        // file, appended after the install evidence and before the
                        // deletion.
                        self.checkpoint(
                            item,
                            ItemCheckpoint::RemovingBackup,
                            None,
                            &[backup_path.clone()],
                        )?;
                        if let Err(error) = self.root.remove_owned(parent, &backup, old_identity) {
                            if error.kind() != ErrorKind::NotFound {
                                return Err(Flow::PostInstall {
                                    message: format!(
                                        "the reviewed bytes were installed at {}, but the replaced file {} could not be removed or shown to be the file this item displaced: {error}",
                                        destination.display(),
                                        backup_path.display()
                                    ),
                                    recovery: retained(&backup_path),
                                });
                            }
                        }
                        if let Err(reason) = self.namespace_sync(parent) {
                            return Err(Flow::PostInstall {
                                message: format!(
                                    "the reviewed bytes were installed at {}, and the replaced file {} was removed, but {reason}",
                                    destination.display(),
                                    backup_path.display()
                                ),
                                recovery: retained(&backup_path),
                            });
                        }
                        Ok(InstallOutcome {
                            note: None,
                            recovery: Vec::new(),
                        })
                    }
                    Err(error) => {
                        // A failed commit restores the original only while the held
                        // identity still proves the backup name means it; the
                        // restore is itself a no-clobber rename, synced afterwards.
                        let proven = self.root.entry_stat(parent, &backup).is_ok_and(|entry| {
                            same_entry(EntryIdentity::from(entry), old_identity)
                        });
                        if !proven {
                            return Err(Flow::Problem(
                                format!(
                                    "the staged file could not be installed at {} ({error}), and {} cannot be shown to be the file this item displaced, so nothing was restored or deleted",
                                    destination.display(),
                                    backup_path.display()
                                ),
                                retained(&backup_path),
                            ));
                        }
                        match self.root.rename_between(parent, &backup, parent, name) {
                            Ok(()) => match self.namespace_sync(parent) {
                                Ok(()) => Err(problem(format!(
                                    "the staged file could not be installed at {} ({error}); the previous destination was restored",
                                    destination.display()
                                ))),
                                Err(reason) => Err(Flow::Problem(
                                    format!(
                                        "the staged file could not be installed at {} ({error}); the previous destination was restored to that name, but {reason}",
                                        destination.display()
                                    ),
                                    retained(&backup_path),
                                )),
                            },
                            Err(restore_error) => Err(Flow::Problem(
                                format!(
                                    "the staged file could not be installed at {} ({error}), and the previous destination could not be restored ({restore_error}); it remains at {}",
                                    destination.display(),
                                    backup_path.display()
                                ),
                                vec![backup_path],
                            )),
                        }
                    }
                }
            }
            _ => Err(problem(format!(
                "reviewed destination expectation for {} cannot be installed",
                destination.display()
            ))),
        }
    }

    /// Smallest unused `<stem>.vfxlauncher-replace-<n>` beside the destination.
    /// The rename that uses it reserves the name with no-clobber semantics, so a
    /// raced file is never overwritten.
    fn unique_backup_name(
        &self,
        parent: &OwnedFd,
        name: &OsStr,
        destination: &Path,
    ) -> Result<OsString, Flow> {
        for counter in 1..=u64::from(MAX_BACKUP_ATTEMPTS) {
            let candidate = suffixed_name(name, REPLACE_SUFFIX, counter);
            if self
                .protected_names
                .contains(&destination.with_file_name(&candidate))
            {
                continue;
            }
            match self.root.entry_identity(parent, &candidate) {
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(candidate),
                Ok(_) => continue,
                Err(error) => {
                    return Err(problem(format!(
                        "the replacement target could not be inspected: {error}"
                    )));
                }
            }
        }
        Err(problem(format!(
            "no free replacement name within {MAX_BACKUP_ATTEMPTS} attempts"
        )))
    }

    fn staging_name(&mut self, name: &OsStr) -> OsString {
        self.staging_counter += 1;
        suffixed_name(name, STAGING_SUFFIX, self.staging_counter)
    }

    /// Proof that the destination name now means the staged file this item
    /// created, using the identity held from the creating handle.
    fn verify_installed(
        &self,
        parent: &OwnedFd,
        name: &OsStr,
        stage: &Stage,
        destination: &Path,
    ) -> Result<(), String> {
        let entry = self.root.entry_stat(parent, name).map_err(|error| {
            format!(
                "the installed destination {} could not be inspected after the commit: {error}",
                destination.display()
            )
        })?;
        if !same_entry(EntryIdentity::from(entry), stage.identity) {
            return Err(format!(
                "the destination {} no longer holds the staged file this item created; nothing was restored or deleted",
                destination.display()
            ));
        }
        Ok(())
    }

    /// Syncs the folder that now holds a committed file. Some network and FUSE
    /// filesystems refuse a directory fsync; that is reported, never assumed.
    fn namespace_sync(&self, parent: &OwnedFd) -> Result<(), String> {
        self.root
            .sync_dir(parent)
            .map_err(|error| format!("the destination folder could not be synced: {error}"))
    }

    fn sync_move_directories(
        &self,
        source_parent: &OwnedFd,
        destination_parent: &OwnedFd,
    ) -> Result<(), String> {
        match (
            self.root.sync_dir(source_parent),
            self.root.sync_dir(destination_parent),
        ) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) | (_, Err(error)) => Err(format!(
                "moved, but the affected folders could not be synced: {error}"
            )),
        }
    }

    /// Reserves one concrete staging basename beside the destination, journals
    /// that exact absolute path and the destination as the intent, and only then
    /// creates the file exclusively. A name taken between the intent and the
    /// create fails the item instead of retrying: a second checkpoint would
    /// describe a file this run never made, and the entry that won the race is
    /// not this run's to adopt or delete.
    fn begin_staging(
        &mut self,
        item: &ItemValidation,
        parent: &OwnedFd,
        name: &OsStr,
        destination: &Path,
    ) -> Result<Stage, Flow> {
        let stage_name = self.free_staging_name(parent, name, destination)?;
        let stage_path = destination.with_file_name(&stage_name);
        self.checkpoint(
            item,
            ItemCheckpoint::Staging,
            None,
            &[stage_path.clone(), destination.to_path_buf()],
        )
        .map_err(Flow::Abort)?;
        let file = match self.root.create_staging(parent, &stage_name) {
            Ok(staging) => staging.into_file(),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                return Err(problem(format!(
                    "{} appeared beside the destination after this item recorded its staging intent; it was left untouched",
                    stage_path.display()
                )));
            }
            Err(error) => return Err(io_problem("the staging file", &stage_path, error)),
        };
        // The identity of the file this handle created is the only one any later
        // cleanup may use; a name that came to mean something else is reported.
        let identity = match rustix::fs::fstat(&file) {
            Ok(stat) => EntryIdentity::from(stat),
            Err(error) => {
                drop(file);
                return Err(Flow::Problem(
                    format!(
                        "the staging file {} was created but its identity could not be read, so it is retained: {error}",
                        stage_path.display()
                    ),
                    vec![stage_path],
                ));
            }
        };
        Ok(Stage {
            file,
            name: stage_name,
            path: stage_path,
            identity,
            published: false,
        })
    }

    /// Smallest unused app-owned staging basename beside the destination. A
    /// leftover from an interrupted run is never reused; the counter moves past
    /// it.
    fn free_staging_name(
        &mut self,
        parent: &OwnedFd,
        name: &OsStr,
        destination: &Path,
    ) -> Result<OsString, Flow> {
        for _ in 0..MAX_BACKUP_ATTEMPTS {
            let candidate = self.staging_name(name);
            if self
                .protected_names
                .contains(&destination.with_file_name(&candidate))
            {
                continue;
            }
            match self.root.entry_identity(parent, &candidate) {
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(candidate),
                Ok(_) => continue,
                Err(error) => {
                    return Err(problem(format!(
                        "the staging name beside {} could not be inspected: {error}",
                        destination.display()
                    )));
                }
            }
        }
        Err(problem(format!(
            "no free staging name beside {} within {MAX_BACKUP_ATTEMPTS} attempts",
            destination.display()
        )))
    }
}

fn suffixed_name(name: &OsStr, suffix: &str, counter: u64) -> OsString {
    let mut candidate = name.to_os_string();
    candidate.push(suffix);
    candidate.push("-");
    candidate.push(counter.to_string());
    candidate
}

fn combine_notes(first: Option<String>, second: Option<String>) -> Option<String> {
    match (first, second) {
        (Some(first), Some(second)) => Some(format!("{first}; {second}")),
        (Some(note), None) | (None, Some(note)) => Some(note),
        (None, None) => None,
    }
}

fn source_meta_mode(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::MetadataExt;
    meta.mode()
}

fn source_times(meta: &fs::Metadata) -> Timestamps {
    use std::os::unix::fs::MetadataExt;
    Timestamps {
        last_access: Timespec {
            tv_sec: meta.atime(),
            tv_nsec: meta.atime_nsec(),
        },
        last_modification: Timespec {
            tv_sec: meta.mtime(),
            tv_nsec: meta.mtime_nsec(),
        },
    }
}

/// Opens the declared source root once per run and refuses unless its recorded
/// identity provably agrees with the held handle. Every source is then resolved
/// beneath that handle, so a later path swap cannot redirect a read.
fn hold_source_root(validated: &ValidatedPlan) -> Result<(Root, PathBuf), String> {
    let plan = validated.plan();
    let path = plan
        .source_root_path()
        .ok_or_else(|| {
            "the reviewed plan copies or moves files but declares no source root".to_string()
        })?
        .to_path_buf();
    let evidence = plan.source_evidence.as_ref().ok_or_else(|| {
        format!(
            "the reviewed plan records no source evidence for {}, so its sources cannot be confined",
            path.display()
        )
    })?;
    let root = Root::open(path.as_os_str()).map_err(|error| {
        format!(
            "the reviewed source root {} cannot be opened: {error}",
            path.display()
        )
    })?;
    let fresh = identity_of_handle(&root).map_err(|error| {
        format!(
            "the reviewed source root {} can no longer be inspected: {error}",
            path.display()
        )
    })?;
    require_same_identity(
        evidence.root_identity,
        Some(fresh),
        "the reviewed source root",
    )?;
    Ok((root, path))
}

fn identity_of_handle(handle: &impl AsFd) -> io::Result<NativeIdentity> {
    let stat = rustix::fs::fstat(handle)?;
    Ok(native_identity(EntryIdentity::from(stat)))
}

fn native_identity(identity: EntryIdentity) -> NativeIdentity {
    NativeIdentity {
        unix: Some((identity.device, identity.inode)),
        windows: None,
    }
}

/// Fails closed unless the recorded and freshly observed identities provably
/// agree. Missing evidence on either side is unknown, never agreement.
fn require_same_identity(
    recorded: Option<NativeIdentity>,
    fresh: Option<NativeIdentity>,
    what: &str,
) -> Result<(), String> {
    match (recorded, fresh) {
        (Some(recorded), Some(fresh)) => match recorded.same(&fresh) {
            Some(true) => Ok(()),
            Some(false) => Err(format!("{what} changed after review")),
            None => Err(format!(
                "{what} shares no comparable identity evidence with the review"
            )),
        },
        (Some(_), None) => Err(format!("{what} identity can no longer be read")),
        (None, _) => Err(format!(
            "the review recorded no identity evidence for {what}"
        )),
    }
}

/// The source must still be exactly the file the review recorded.
fn verify_stamp(meta: &fs::Metadata, reviewed: &SourceStamp, path: &Path) -> Result<(), String> {
    if meta.len() != reviewed.size {
        return Err(format!(
            "source {} is {} bytes; the review recorded {}",
            path.display(),
            meta.len(),
            reviewed.size
        ));
    }
    if let Some(expected) = reviewed.modified_ns {
        let observed = meta
            .modified()
            .map_err(|error| {
                format!(
                    "source {} modification time is unavailable: {error}",
                    path.display()
                )
            })
            .and_then(nanos_from_time)?;
        if observed != expected {
            return Err(format!(
                "source {} changed after review: its modification time moved",
                path.display()
            ));
        }
    }
    match (
        reviewed.identity,
        crate::paths::FileIdentity::from_metadata(meta).map(super::super::native_identity_of),
    ) {
        (Some(before), Some(now)) => match before.same(&now) {
            Some(true) => Ok(()),
            Some(false) => Err(format!(
                "source {} is no longer the file the review recorded",
                path.display()
            )),
            None => Err(format!(
                "source {} shares no comparable identity evidence with the review",
                path.display()
            )),
        },
        (Some(_), None) => Err(format!(
            "source {} identity can no longer be read",
            path.display()
        )),
        (None, _) => Err(format!(
            "the review recorded no identity evidence for source {}",
            path.display()
        )),
    }
}

/// The reviewed existing destination must still be the same regular file with the
/// same size and modification time before Replace or PreserveExisting displaces
/// it. Only that displaced entry is ever mutated.
fn verify_existing_destination(
    stat: &rustix::fs::Stat,
    reviewed: &SourceStamp,
    path: &Path,
) -> Result<(), String> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(format!(
            "destination {} is no longer a regular file; it was left untouched",
            path.display()
        ));
    }
    let size = u64::try_from(stat.st_size)
        .map_err(|_| format!("destination {} reports an unusable size", path.display()))?;
    if size != reviewed.size {
        return Err(format!(
            "destination {} changed size after review; it was left untouched",
            path.display()
        ));
    }
    if let Some(expected) = reviewed.modified_ns {
        let observed = i128::from(stat.st_mtime) * 1_000_000_000 + i128::from(stat.st_mtime_nsec);
        if observed != i128::from(expected) {
            return Err(format!(
                "destination {} changed after review: its modification time moved",
                path.display()
            ));
        }
    }
    require_same_identity(
        reviewed.identity,
        Some(native_identity(EntryIdentity::from(*stat))),
        &format!("the existing destination {}", path.display()),
    )
}

/// The directory entry a Move is about to unlink must still be the reviewed
/// regular file: type, size, modification time and identity all agree.
fn verify_stamp_stat(
    stat: &rustix::fs::Stat,
    reviewed: &SourceStamp,
    path: &Path,
) -> Result<(), String> {
    if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
        return Err(format!(
            "source {} is no longer a regular file",
            path.display()
        ));
    }
    let size = u64::try_from(stat.st_size)
        .map_err(|_| format!("source {} reports an unusable size", path.display()))?;
    if size != reviewed.size {
        return Err(format!(
            "source {} is {size} bytes; the review recorded {}",
            path.display(),
            reviewed.size
        ));
    }
    if let Some(expected) = reviewed.modified_ns {
        let observed = i128::from(stat.st_mtime) * 1_000_000_000 + i128::from(stat.st_mtime_nsec);
        if observed != i128::from(expected) {
            return Err(format!(
                "source {} changed after review: its modification time moved",
                path.display()
            ));
        }
    }
    require_same_identity(
        reviewed.identity,
        Some(native_identity(EntryIdentity::from(*stat))),
        &format!("source {}", path.display()),
    )
}

/// `renameat2(RENAME_NOREPLACE)` is the no-clobber commit primitive. When the
/// kernel or filesystem does not provide it, the item is refused instead of
/// falling back to a replace-by-rename.
fn no_clobber_unavailable(error: &io::Error) -> bool {
    matches!(
        Errno::from_io_error(error),
        Some(Errno::NOSYS | Errno::INVAL | Errno::OPNOTSUPP)
    )
}
