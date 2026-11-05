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
//! displacement/restore critical section used by Replace and PreserveExisting:
//! the old file is renamed to a reserved sibling, the staged file is committed,
//! and a failed commit renames the old file back. Nothing else moves, replaces
//! or deletes a destination, and no-clobber rename is the commit primitive on
//! both sides of that window.
//!
//! Deliberate limits of this packet:
//! - Linux only; other platforms keep the unsupported-operation boundary.
//! - Items run one at a time, in reviewed order. The contract's transfer
//!   concurrency setting is not implemented here, so there is no per-run worker
//!   pool and no shared mutable executor state.
//! - Pause/cancel is cooperative between items and copy chunks. A blocked OS
//!   read/write is not interrupted.
//! - Identity comparison around a name-based rename or unlink is a fresh check,
//!   not an atomic one: correctness assumes cooperating writers in the selected
//!   directories and makes no hostile-race claim.
//! - Source removal for Move re-checks the source identity, but a crash between
//!   install and removal leaves both files; the run is then interrupted and
//!   inspectable, never replayed automatically.

use super::super::native::{EntryIdentity, Root};
use super::super::{
    DestinationEvidence, DestinationExpectation, ItemCheckpoint, ItemEffect, ItemState,
    ItemValidation, JournaledPlan, NativeIdentity, PlannedAction, RunSummary, RunTerminal,
    SourceStamp, StoredPath, TransferCategory, TransferError, ValidatedPlan,
    append_item_checkpoint, finalize_run, identity_of, mark_run_running, nanos_from_time,
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
    collections::HashSet,
    ffi::{OsStr, OsString},
    fs::{self, File},
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
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
pub fn execute(
    catalog: &CatalogClient,
    validated: ValidatedPlan,
    control: &JobControl,
    events: &JobEvents,
) -> Result<RunReport, TransferError> {
    let total_items = validated.items().len() as u64;
    let total_bytes = plan_bytes(validated.items());
    let evidence = validated.destination_evidence().clone();
    let destination_root = validated.destination_root().to_path_buf();

    let journal = awaited(persist_validated_intent(catalog, validated))?;
    let run_id = journal.run_id();
    awaited(mark_run_running(catalog, run_id))?;

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
        destination_root,
        blocked_directories: HashSet::new(),
        staging_counter: 0,
        total_items,
        total_bytes,
        completed: 0,
        bytes_done: 0,
        outcomes: Vec::with_capacity(journal.validated().items().len()),
    };

    if let Err(reason) = executor.revalidate_destination_evidence(&evidence) {
        settle_refused(catalog, &journal, &reason)?;
        return Err(refused(&reason));
    }

    let items = journal.validated().items().to_vec();
    let mut cancelled = false;
    for item in &items {
        if control.checkpoint() == Checkpoint::Cancelled {
            cancelled = true;
            break;
        }
        match executor.run(item) {
            Ok(Done::Succeeded(message)) => {
                let state = success_state(item.effect);
                let bytes = item.bytes.unwrap_or(0);
                executor.record(item, state, bytes, message, Vec::new())?;
            }
            Ok(Done::SourceRetained { message, source }) => {
                executor.record(
                    item,
                    ItemState::CopiedSourceRetained,
                    item.bytes.unwrap_or(0),
                    Some(message),
                    vec![source],
                )?;
            }
            Err(Flow::Problem(message, recovery)) => {
                executor.note_failure(item);
                events.diagnostic(
                    JobPhase::Copying,
                    format!("item {} failed: {message}", item.index + 1),
                );
                executor.record(item, ItemState::Failed, 0, Some(message), recovery)?;
            }
            Err(Flow::Cancelled(recovery)) => {
                executor.record(
                    item,
                    ItemState::Cancelled,
                    0,
                    Some("cancelled before the destination was installed".to_string()),
                    recovery,
                )?;
                cancelled = true;
                break;
            }
            Err(Flow::Abort(error)) => return Err(error),
        }
    }

    let terminal = if cancelled {
        RunTerminal::Cancelled
    } else {
        terminal_for(&executor.outcomes)
    };
    let summary = awaited(finalize_run(catalog, run_id, terminal))?;
    Ok(RunReport {
        run_id,
        summary,
        items: executor.outcomes,
    })
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

/// How one item finished before its terminal state is chosen.
enum Done {
    Succeeded(Option<String>),
    SourceRetained { message: String, source: PathBuf },
}

/// Per-item flow. A `Problem` or `Cancelled` becomes that item's terminal
/// outcome; `Abort` stops the run because the journal itself is unusable.
enum Flow {
    Problem(String, Vec<PathBuf>),
    Cancelled(Vec<PathBuf>),
    Abort(TransferError),
}

impl From<TransferError> for Flow {
    fn from(error: TransferError) -> Self {
        Self::Abort(error)
    }
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

struct Executor<'a> {
    catalog: &'a CatalogClient,
    journal: &'a JournaledPlan,
    control: &'a JobControl,
    events: &'a JobEvents,
    root: Root,
    destination_root: PathBuf,
    /// Destination-relative folders whose creation failed; children of these are
    /// failed without being attempted, so a blocked prerequisite stays visible.
    blocked_directories: HashSet<PathBuf>,
    staging_counter: u64,
    total_items: u64,
    total_bytes: Option<u64>,
    completed: u64,
    bytes_done: u64,
    outcomes: Vec<ItemOutcome>,
}

impl Executor<'_> {
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
        self.completed += 1;
        self.bytes_done += committed_bytes;
        self.emit_progress(None);
        Ok(())
    }

    fn checkpoint(
        &self,
        item: &ItemValidation,
        step: ItemCheckpoint,
        message: Option<String>,
    ) -> Result<(), TransferError> {
        awaited(append_item_checkpoint(
            self.catalog,
            self.journal,
            item.index as i64,
            step,
            message,
            &[],
        ))
    }

    fn emit_progress(&self, current: Option<PathBuf>) {
        self.events.progress(
            JobPhase::Copying,
            JobProgress {
                completed: self.completed,
                total: Some(self.total_items),
                bytes_done: self.bytes_done,
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
                self.blocked_directories.insert(relative.to_path_buf());
            }
        }
    }

    // -- evidence ------------------------------------------------------------

    /// Revalidates the reviewed destination root and every existing ancestor
    /// folder against the identity snapshot captured while validating. Missing or
    /// incomparable evidence refuses the run; it is never treated as agreement.
    fn revalidate_destination_evidence(
        &self,
        evidence: &DestinationEvidence,
    ) -> Result<(), String> {
        let root_identity = identity_of_handle(&self.root)
            .map_err(|error| format!("the destination root can no longer be inspected: {error}"))?;
        require_same_identity(
            evidence.root_identity,
            Some(root_identity),
            "the destination root",
        )?;
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
        }
        Ok(())
    }

    // -- items ---------------------------------------------------------------

    fn run(&mut self, item: &ItemValidation) -> Result<Done, Flow> {
        if let Some(message) = self.blocked_parent(item)? {
            return Err(problem(message));
        }
        match item.effect {
            ItemEffect::Skip => Ok(Done::Succeeded(None)),
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

    fn open_parent(&self, parent_relative: &Path) -> io::Result<OwnedFd> {
        if parent_relative.as_os_str().is_empty() {
            self.root.duplicate()
        } else {
            self.root.open_dir(parent_relative.as_os_str())
        }
    }

    fn discard_staging(&self, parent: &OwnedFd, stage_name: &OsStr) {
        if let Ok(identity) = self.root.entry_identity(parent, stage_name) {
            let _ = self.root.remove_owned(parent, stage_name, identity);
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
                Ok(_created) => {
                    self.root.sync_dir(&parent).map_err(|error| {
                        io_problem("the destination folder", &destination, error)
                    })?;
                    Ok(Done::Succeeded(None))
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
                Ok(Done::Succeeded(None))
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

        self.checkpoint(item, ItemCheckpoint::Staging, None)?;
        let (mut staged, stage_name) = self.create_staging(&parent, &name, &destination)?;
        if let Err(error) = staged.write_all(&contents) {
            self.discard_staging(&parent, &stage_name);
            return Err(io_problem("the staging file", &destination, error));
        }
        if let Err(error) = staged.sync_all() {
            self.discard_staging(&parent, &stage_name);
            return Err(io_problem("the staging file", &destination, error));
        }
        self.checkpoint(item, ItemCheckpoint::Staged, None)?;

        Ok(Done::Succeeded(self.install(
            item,
            &parent,
            &name,
            &stage_name,
            &destination,
        )?))
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

        let source_parent_path = source
            .parent()
            .ok_or_else(|| problem(format!("source {} has no folder", source.display())))?
            .to_path_buf();
        let source_name = source
            .file_name()
            .ok_or_else(|| problem(format!("source {} has no file name", source.display())))?
            .to_os_string();
        let source_parent = Root::open(source_parent_path.as_os_str())
            .map_err(|error| io_problem("the source folder", &source_parent_path, error))?;
        let (mut source_file, source_identity) = self
            .root
            .open_regular(&source_parent, &source_name, false)
            .map_err(|error| io_problem("the source file", &source, error))?;
        let source_meta = source_file
            .metadata()
            .map_err(|error| io_problem("the source file", &source, error))?;
        verify_stamp(&source_meta, &reviewed, &source)
            .map_err(|message| Flow::Problem(message, Vec::new()))?;

        // Same-filesystem Move: one no-clobber rename installs the destination and
        // removes the source atomically. Cross-device moves fall through to the
        // staged copy below; unavailable no-clobber rename refuses the item.
        if item.effect == ItemEffect::Move
            && matches!(item.expectation, Some(DestinationExpectation::Absent))
        {
            match self
                .root
                .rename_between(&source_parent, &source_name, &parent, &name)
            {
                Ok(()) => {
                    let note = self.sync_move_directories(&source_parent, &parent);
                    self.checkpoint(item, ItemCheckpoint::Installed, None)?;
                    return Ok(Done::Succeeded(note));
                }
                Err(error) if is_cross_device(&error) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                    return Err(problem(format!(
                        "destination {} appeared after review; the source was left in place",
                        destination.display()
                    )));
                }
                Err(error) if no_clobber_unavailable(&error) => {
                    return Err(problem(format!(
                        "this filesystem does not support a no-clobber rename ({error}); refusing to move {} without it",
                        source.display()
                    )));
                }
                Err(error) => return Err(io_problem("the source file", &source, error)),
            }
        }

        self.checkpoint(item, ItemCheckpoint::Staging, None)?;
        let (mut staged, stage_name) = self.create_staging(&parent, &name, &destination)?;
        let metadata_note = match self.fill_staging(
            &mut staged,
            &mut source_file,
            &source_meta,
            &reviewed,
            &source,
            &destination,
        ) {
            Ok(note) => note,
            Err(flow) => {
                self.discard_staging(&parent, &stage_name);
                return Err(flow);
            }
        };
        self.checkpoint(item, ItemCheckpoint::Staged, None)?;

        let installed = self.install(item, &parent, &name, &stage_name, &destination);
        let install_note = match installed {
            Ok(note) => note,
            Err(flow) => {
                self.discard_staging(&parent, &stage_name);
                return Err(flow);
            }
        };
        let note = combine_notes(metadata_note, install_note);

        if item.effect == ItemEffect::Move {
            self.checkpoint(item, ItemCheckpoint::RemovingSource, None)?;
            let current = self
                .root
                .entry_identity(&source_parent, &source_name)
                .map_err(|error| {
                    Flow::Problem(
                        format!(
                            "the copy is installed, but the source {} could not be rechecked before removal: {error}",
                            source.display()
                        ),
                        vec![source.clone()],
                    )
                })?;
            if current != source_identity {
                return Ok(Done::SourceRetained {
                    message: format!(
                        "the copy is installed, but the source {} changed after it was read; it was kept",
                        source.display()
                    ),
                    source,
                });
            }
            if let Err(error) = self
                .root
                .remove_owned(&source_parent, &source_name, current)
            {
                return Ok(Done::SourceRetained {
                    message: format!(
                        "the copy is installed, but the source {} could not be removed: {error}",
                        source.display()
                    ),
                    source,
                });
            }
            self.sync_move_directories(&source_parent, &parent);
        }
        Ok(Done::Succeeded(note))
    }

    /// Copies the opened source into the staging file, preserves its metadata,
    /// flushes it, and revalidates the source afterwards.
    fn fill_staging(
        &mut self,
        staged: &mut File,
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
            staged
                .write_all(&buffer[..read])
                .map_err(|error| io_problem("the staging file", destination, error))?;
            written += read as u64;
            if written > reviewed.size {
                return Err(problem(format!(
                    "source {} grew while it was copied; the staged file was discarded",
                    source.display()
                )));
            }
            self.events.progress(
                JobPhase::Copying,
                JobProgress {
                    completed: self.completed,
                    total: Some(self.total_items),
                    bytes_done: self.bytes_done + written,
                    bytes_total: self.total_bytes,
                    current: Some(destination.to_path_buf()),
                },
            );
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
        if let Err(error) =
            rustix::fs::fchmod(&*staged, Mode::from_raw_mode(source_meta_mode(source_meta)))
        {
            note = Some(format!(
                "copied, but the source permissions could not be preserved: {error}"
            ));
        }
        if let Err(error) = rustix::fs::futimens(&*staged, &source_times(source_meta)) {
            let message =
                format!("copied, but the source modification time could not be preserved: {error}");
            note = Some(match note {
                Some(previous) => format!("{previous}; {message}"),
                None => message,
            });
        }
        staged
            .sync_all()
            .map_err(|error| io_problem("the staging file", destination, error))?;

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
        stage_name: &OsStr,
        destination: &Path,
    ) -> Result<Option<String>, Flow> {
        match item.expectation.as_ref() {
            Some(DestinationExpectation::Absent) => {
                match self.root.publish_noreplace(parent, stage_name, name) {
                    Ok(()) => {
                        self.checkpoint(item, ItemCheckpoint::Installed, None)?;
                        self.root.sync_dir(parent).map_err(|error| {
                            io_problem("the destination folder", destination, error)
                        })?;
                        Ok(None)
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
                    None => (self.unique_backup_name(parent, name)?, false),
                };

                // Displacement/restore critical section: the reviewed destination
                // is renamed aside without clobbering, the staged file is committed
                // without clobbering, and a failed commit renames it straight back.
                self.checkpoint(item, ItemCheckpoint::Displacing, None)?;
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
                match self.root.publish_noreplace(parent, stage_name, name) {
                    Ok(()) => {
                        let mut note = None;
                        if !keep {
                            if let Err(error) = self.remove_owned(parent, &backup) {
                                note = Some(format!(
                                    "installed, but the replaced file {} could not be removed: {error}",
                                    destination.with_file_name(&backup).display()
                                ));
                            }
                        }
                        self.checkpoint(item, ItemCheckpoint::Installed, None)?;
                        self.root.sync_dir(parent).map_err(|error| {
                            io_problem("the destination folder", destination, error)
                        })?;
                        Ok(note)
                    }
                    Err(error) => {
                        let backup_path = destination.with_file_name(&backup);
                        match self.root.rename_between(parent, &backup, parent, name) {
                            Ok(()) => Err(problem(format!(
                                "the staged file could not be installed at {} ({error}); the previous destination was restored",
                                destination.display()
                            ))),
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

    fn remove_owned(&self, parent: &OwnedFd, name: &OsStr) -> io::Result<()> {
        let identity = self.root.entry_identity(parent, name)?;
        self.root.remove_owned(parent, name, identity)
    }

    /// Smallest unused `<stem>.vfxlauncher-replace-<n>` beside the destination.
    /// The rename that uses it reserves the name with no-clobber semantics, so a
    /// raced file is never overwritten.
    fn unique_backup_name(&self, parent: &OwnedFd, name: &OsStr) -> Result<OsString, Flow> {
        for counter in 1..=u64::from(MAX_BACKUP_ATTEMPTS) {
            let candidate = suffixed_name(name, REPLACE_SUFFIX, counter);
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

    fn sync_move_directories(
        &self,
        source_parent: &Root,
        destination_parent: &OwnedFd,
    ) -> Option<String> {
        match (
            self.root.sync_dir(source_parent),
            self.root.sync_dir(destination_parent),
        ) {
            (Ok(()), Ok(())) => None,
            (Err(error), _) | (_, Err(error)) => Some(format!(
                "moved, but the affected folders could not be synced: {error}"
            )),
        }
    }

    /// Exclusive app-owned staging file beside the destination. A leftover from an
    /// interrupted run is never reused; the counter moves past it.
    fn create_staging(
        &mut self,
        parent: &OwnedFd,
        name: &OsStr,
        destination: &Path,
    ) -> Result<(File, OsString), Flow> {
        for _ in 0..MAX_BACKUP_ATTEMPTS {
            let stage_name = self.staging_name(name);
            match self.root.create_staging(parent, &stage_name) {
                Ok(staging) => return Ok((staging.into_file(), stage_name)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(io_problem("the staging file", destination, error)),
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
    match (reviewed.identity, identity_of(meta)) {
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

fn is_cross_device(error: &io::Error) -> bool {
    matches!(Errno::from_io_error(error), Some(Errno::XDEV))
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
