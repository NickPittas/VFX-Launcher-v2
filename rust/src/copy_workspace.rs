//! Copy by File Type: the UI and worker coordination for the extension copy.
//!
//! Ported from `gitclones/File-Extention-Copier/file-copier-ui.py`: a source and
//! destination folder, a configurable case-insensitive suffix list defaulting to
//! `.nk` and `.autosave`, a recursive walk that reports every matching file, and
//! progress over the files that are actually copied. The relative folder
//! structure and filenames are preserved because each file's destination is its
//! exact source-relative path beneath the chosen destination root.
//!
//! The legacy tool copied whatever its second walk returned. This port keeps the
//! reviewed path instead: discovery produces one snapshot, the selected subset
//! becomes one explicit manifest, and only a plan validated against the exact
//! roots, suffixes, selection and conflict policy may execute. Any edit to those
//! inputs invalidates the review and the execution control until the current
//! selection is validated again.
//!
//! Nothing here touches the filesystem or the catalog on the UI thread.
//! Scanning, validation and execution each run in one tracked [`Jobs`] worker
//! and report through the job transport; the frame only drains their channels.
//! The native folder chooser is a tracked job too, so closing the window never
//! detaches a request thread.

use std::{
    collections::{HashMap, HashSet},
    path::Path,
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError, channel},
};

use eframe::egui;

use crate::{
    discovery::{
        self, Coverage, FileEntry, ScanFilters, ScanPurpose, ScanRequest, ScanSnapshot, SuffixMode,
    },
    jobs::{JobContext, JobId, JobOutcome, JobPhase, JobProgress, Jobs},
    storage::{CatalogClient, UserId},
    transfers::{
        self, ActionKind, ConflictPolicy, ItemEffect, ItemState, ItemValidation, ReviewState,
        RunCounts, RunInspection, RunReport, RunState, TransferError, ValidatedPlan,
        ValidationReport,
    },
};

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2f, 0x6b, 0xa8);
const WARNING: egui::Color32 = egui::Color32::from_rgb(0xe0, 0x9a, 0x6a);
const DEFAULT_EXTENSIONS: [&str; 2] = [".autosave", ".nk"];
const POLICIES: [ConflictPolicy; 4] = [
    ConflictPolicy::Skip,
    ConflictPolicy::Replace,
    ConflictPolicy::KeepBoth,
    ConflictPolicy::PreserveExisting,
];
const NO_CATALOG: &str = "The catalog is not ready yet.";
const NO_USER: &str = "Choose who is using the app first.";

/// Which folder field a browse request fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Source,
    Destination,
}

impl Field {
    fn title(self) -> &'static str {
        match self {
            Self::Source => "Source folder",
            Self::Destination => "Destination folder",
        }
    }
}

fn policy_label(policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::Skip => "Skip",
        ConflictPolicy::Replace => "Replace",
        ConflictPolicy::KeepBoth => "Keep both",
        ConflictPolicy::PreserveExisting => "Preserve existing",
    }
}

fn policy_hint(policy: ConflictPolicy) -> &'static str {
    match policy {
        ConflictPolicy::Skip => "Leave a file that already exists at the destination untouched.",
        ConflictPolicy::Replace => "Replace an existing destination file with the source copy.",
        ConflictPolicy::KeepBoth => "Copy beside the existing file under a numbered name.",
        ConflictPolicy::PreserveExisting => {
            "Keep the existing destination file and place it aside under a numbered name."
        }
    }
}

/// Everything a validation is bound to. Editing any field, changing the file
/// selection or rescanning makes the reviewed plan stale.
#[derive(Clone, PartialEq, Eq)]
struct Draft {
    backup: Option<crate::backup::BackupJob>,
    source: String,
    destination: String,
    extensions: Vec<String>,
    policy: ConflictPolicy,
    scan_generation: u64,
    selection_generation: u64,
}

/// One discovered file, with the source-relative spelling its destination keeps.
struct TableRow {
    path: PathBuf,
    relative: String,
    size: u64,
}

/// What the file table says about one row for the current review or run.
#[derive(Clone)]
struct RowState {
    label: String,
    detail: Option<String>,
    blocked: bool,
}

/// One matching file list produced by a completed scan, bound to the exact
/// source and suffix set it was found with.
struct ScanResult {
    summary: Option<String>,
    backup: Option<crate::backup::BackupJob>,
    snapshot: ScanSnapshot,
    source: String,
    extensions: Vec<String>,
}

struct PickRun {
    job: JobId,
    field: Field,
    replies: Receiver<Option<PathBuf>>,
}

struct ScanRun {
    backup: Option<crate::backup::BackupJob>,
    job: JobId,
    replies: Receiver<Result<(ScanSnapshot, Option<String>), String>>,
    source: String,
    extensions: Vec<String>,
}

struct ValidateRun {
    draft: Draft,
    replies: Receiver<Result<ValidationDone, String>>,
}

struct ValidationDone {
    report: ValidationReport,
    original_sources: Vec<Option<PathBuf>>,
}

/// Where one reviewed item writes, so a run report can name its destination
/// after the review itself is gone.
struct ItemLabel {
    source: Option<PathBuf>,
    destination: String,
    kind: &'static str,
}

struct CopyRun {
    job: JobId,
    replies: Receiver<Result<RunReport, CopyFailure>>,
    labels: Vec<ItemLabel>,
    draft: Draft,
}

struct CopyFailure {
    message: String,
    run_id: Option<i64>,
    inspection: Option<RunInspection>,
    progress: JobProgress,
}

/// One accepted validation. `ready` is the only handle that can execute, and it
/// is consumed by the run that uses it.
struct Review {
    draft: Draft,
    items: Vec<ItemValidation>,
    original_sources: Vec<Option<PathBuf>>,
    folders: usize,
    files_copy: usize,
    files_skip: usize,
    files_blocked: usize,
    warnings: Vec<String>,
    refusal: Option<String>,
    space_unknown: bool,
    ready: Option<ValidatedPlan>,
}

/// One finished run, kept beside the progress so results never need a tab.
struct ReportRow {
    source: Option<PathBuf>,
    kind: &'static str,
    destination: String,
    state: &'static str,
    failed: bool,
    message: Option<String>,
    recovery: Vec<String>,
    bytes: u64,
}

struct RunDone {
    draft: Draft,
    run_id: Option<i64>,
    error: Option<String>,
    progress: JobProgress,
    state: RunState,
    counts: RunCounts,
    rows: Vec<ReportRow>,
    warnings: Vec<String>,
}

#[derive(Default)]
struct ReviewCounts {
    folders: usize,
    copy: usize,
    skip: usize,
    blocked: usize,
}

/// The Copy by File Type workspace: draft inputs, the tracked workers and the
/// review/run evidence the page and the bottom strip render.
pub struct CopyByFileTypeState {
    automatic_phase: Option<u8>,
    backup: Option<crate::backup::BackupJob>,
    ctx: egui::Context,
    catalog: Option<CatalogClient>,
    user: Option<UserId>,
    source: String,
    destination: String,
    extensions: Vec<String>,
    extension_entry: String,
    extension_error: Option<String>,
    policy: ConflictPolicy,
    pick: Option<PickRun>,
    scan_run: Option<ScanRun>,
    scanned: Option<ScanResult>,
    rows: Vec<TableRow>,
    selected: HashSet<PathBuf>,
    row_states: HashMap<PathBuf, RowState>,
    selection_generation: u64,
    scan_generation: u64,
    plan_counter: u64,
    validate_run: Option<ValidateRun>,
    review: Option<Review>,
    copy_run: Option<CopyRun>,
    report: Option<RunDone>,
    scan_error: Option<String>,
    message: Option<String>,
    /// Last picked folder, so the chooser opens where the user last worked.
    last_pick: Option<PathBuf>,
}

impl CopyByFileTypeState {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            backup: None,
            automatic_phase: None,
            ctx: ctx.clone(),
            catalog: None,
            user: None,
            source: String::new(),
            destination: String::new(),
            extensions: DEFAULT_EXTENSIONS
                .iter()
                .map(|ext| ext.to_string())
                .collect(),
            extension_entry: String::new(),
            extension_error: None,
            policy: ConflictPolicy::Skip,
            pick: None,
            scan_run: None,
            scanned: None,
            rows: Vec::new(),
            selected: HashSet::new(),
            row_states: HashMap::new(),
            selection_generation: 0,
            scan_generation: 0,
            plan_counter: 0,
            validate_run: None,
            review: None,
            copy_run: None,
            report: None,
            scan_error: None,
            message: None,
            last_pick: None,
        }
    }

    pub fn new_backup(ctx: &egui::Context, job: crate::backup::BackupJob) -> Self {
        let mut state = Self::new(ctx);
        state.source = job.source.clone();
        state.destination = job.destination.clone();
        state.extensions = vec![".aep".into(), ".nk".into()];
        state.policy = ConflictPolicy::PreserveExisting;
        state.backup = Some(job);
        state
    }

    pub fn backup_job(&self) -> Option<crate::backup::BackupJob> {
        self.backup.clone().map(|mut job| {
            job.source = self.source.trim().into();
            job.destination = self.destination.trim().into();
            job
        })
    }

    pub fn backup_job_mut(&mut self) -> Option<&mut crate::backup::BackupJob> {
        self.backup.as_mut()
    }
    pub fn operation_pending(&self) -> bool {
        self.busy()
    }

    pub fn automatic_active(&self) -> bool {
        self.automatic_phase.is_some()
    }
    pub fn begin_automatic(&mut self, saved: &crate::backup::BackupJob, jobs: &mut Jobs) {
        if self.busy() || self.backup_job().as_ref() != Some(saved) {
            return;
        }
        self.begin_scan(jobs);
        if self.scan_run.is_some() {
            self.automatic_phase = Some(1);
        }
    }
    pub fn advance_automatic(&mut self, jobs: &mut Jobs) {
        if self.busy() || jobs.is_busy() {
            return;
        }
        match self.automatic_phase {
            Some(1) => {
                if self
                    .scanned
                    .as_ref()
                    .is_some_and(|scan| scan.snapshot.coverage == Coverage::Complete)
                    && !self.selected.is_empty()
                {
                    self.begin_validation(jobs);
                    self.automatic_phase = self.validate_run.as_ref().map(|_| 2);
                } else {
                    self.automatic_phase = None;
                }
            }
            Some(2) => {
                if self
                    .review
                    .as_ref()
                    .is_some_and(|review| review.ready.is_some())
                {
                    self.begin_copy(jobs);
                    self.automatic_phase = self.copy_run.as_ref().map(|_| 3);
                } else {
                    self.automatic_phase = None;
                }
            }
            Some(3) => self.automatic_phase = None,
            _ => {}
        }
    }

    // ------------------------------------------------------------ coordination

    /// Drains every worker reply and keeps the review bound to the current
    /// draft. Called once per frame from the application logic.
    pub fn poll(&mut self, catalog: Option<&CatalogClient>, user: Option<UserId>, jobs: &Jobs) {
        self.catalog = catalog.cloned();
        self.user = user;
        self.poll_pick();
        self.poll_scan();
        self.poll_validation();
        self.poll_copy(jobs);
        self.sync_draft();
        self.invalidate_review();
    }

    fn poll_pick(&mut self) {
        let Some(run) = &self.pick else {
            return;
        };
        let picked = match run.replies.try_recv() {
            Ok(picked) => picked,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => None,
        };
        let field = run.field;
        self.pick = None;
        let Some(folder) = picked else {
            return;
        };
        self.last_pick = Some(folder.clone());
        let Some(text) = folder.to_str().map(str::to_owned) else {
            self.message = Some("This folder name cannot be shown in the path field. Choose a folder with a Unicode name.".to_string());
            return;
        };
        if let Some(job) = &mut self.backup {
            job.last_browse_dir = text.clone();
        }
        match field {
            Field::Source => self.source = text,
            Field::Destination => self.destination = text,
        }
    }

    fn poll_scan(&mut self) {
        let Some(run) = self.scan_run.take() else {
            return;
        };
        let result = match run.replies.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.scan_run = Some(run);
                return;
            }
            Err(TryRecvError::Disconnected) => Err("The scan ended without a result.".to_string()),
        };
        self.review = None;
        self.row_states.clear();
        self.report = None;
        match result {
            Ok((snapshot, summary)) => {
                self.scan_generation += 1;
                self.rows = table_rows(&snapshot);
                self.selected = snapshot
                    .entries
                    .iter()
                    .map(|entry| entry.path.clone())
                    .collect();
                self.selection_generation += 1;
                self.scan_error = None;
                self.message = summary.clone();
                self.scanned = Some(ScanResult {
                    summary,
                    snapshot,
                    source: run.source,
                    extensions: run.extensions,
                    backup: run.backup,
                });
            }
            Err(message) => {
                self.scanned = None;
                self.rows.clear();
                self.selected.clear();
                self.selection_generation += 1;
                self.scan_error = Some(message.clone());
                self.message = Some(message);
            }
        }
    }

    fn poll_validation(&mut self) {
        let Some(run) = self.validate_run.take() else {
            return;
        };
        let result = match run.replies.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.validate_run = Some(run);
                return;
            }
            Err(TryRecvError::Disconnected) => {
                Err("Validation ended without a report.".to_string())
            }
        };
        if run.draft != self.draft() {
            self.message = Some(
                "The selection or settings changed during validation; validate again.".to_string(),
            );
            return;
        }
        match result {
            Ok(ValidationDone {
                report,
                original_sources,
            }) => {
                let ValidationReport {
                    ready,
                    items,
                    space_unknown,
                    warnings,
                    refusal,
                    ..
                } = report;
                let counted = count_review(&items);
                self.review = Some(Review {
                    draft: run.draft,
                    items,
                    original_sources,
                    folders: counted.folders,
                    files_copy: counted.copy,
                    files_skip: counted.skip,
                    files_blocked: counted.blocked,
                    warnings,
                    refusal,
                    space_unknown,
                    ready,
                });
                self.message = None;
                self.rebuild_row_states();
            }
            Err(message) => {
                self.review = None;
                self.row_states.clear();
                self.message = Some(message);
            }
        }
    }

    fn poll_copy(&mut self, jobs: &Jobs) {
        let Some(run) = self.copy_run.take() else {
            return;
        };
        let result = match run.replies.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.copy_run = Some(run);
                return;
            }
            Err(TryRecvError::Disconnected) => {
                Err(CopyFailure {
                    message: "The copy worker stopped without a report. Some files may have been installed; inspect the destination before retrying.".to_string(),
                    run_id: None,
                    inspection: None,
                    progress: job_status(jobs, Some(run.job))
                        .map(|job| job.progress.clone()).unwrap_or_default(),
                })
            }
        };
        let warnings = self
            .review
            .as_ref()
            .map(|review| review.warnings.clone())
            .unwrap_or_default();
        match result {
            Ok(report) => {
                self.report = Some(RunDone {
                    draft: run.draft,
                    run_id: Some(report.run_id),
                    error: None,
                    progress: report_progress(&report),
                    state: report.summary.state,
                    counts: report.summary.counts,
                    rows: report_rows(&report, &run.labels),
                    warnings,
                });
                self.message = None;
            }
            Err(failure) => {
                self.message = Some(failure.message.clone());
                self.report = Some(failed_run(failure, &run.labels, run.draft, warnings));
            }
        }
        // A run changes the destination, so the reviewed plan can never be
        // reused: the next execution needs a fresh validation.
        self.review = None;
        self.rebuild_row_states();
    }

    pub fn is_picker_job(&self, id: JobId) -> bool {
        self.pick.as_ref().is_some_and(|run| run.job == id)
    }

    pub fn picker_pending(&self) -> bool {
        self.pick.is_some()
    }

    /// Drops the scan when the source or the suffix set it was taken with no
    /// longer matches the draft.
    fn sync_draft(&mut self) {
        let Some(scanned) = &self.scanned else {
            return;
        };
        if scanned.source == self.source.trim()
            && scanned.extensions == self.extensions
            && scanned.backup == self.backup_job()
        {
            return;
        }
        self.scanned = None;
        self.rows.clear();
        self.selected.clear();
        self.row_states.clear();
        self.review = None;
        self.selection_generation += 1;
        self.message = Some("The source folder or file types changed; scan again.".to_string());
    }

    fn invalidate_review(&mut self) {
        if self.copy_run.is_some() {
            return;
        }
        let stale = self
            .review
            .as_ref()
            .is_some_and(|review| review.draft != self.draft());
        if stale {
            self.review = None;
            self.message = Some(
                "The selection or settings changed; validate the selection again.".to_string(),
            );
            self.rebuild_row_states();
        }
        if self.review.is_none()
            && self
                .report
                .as_ref()
                .is_some_and(|report| report.draft != self.draft())
        {
            self.row_states.clear();
        }
    }

    // ----------------------------------------------------------------- drafts

    fn draft(&self) -> Draft {
        Draft {
            backup: self.backup_job(),
            source: self.source.trim().to_string(),
            destination: self.destination.trim().to_string(),
            extensions: self.extensions.clone(),
            policy: self.policy,
            scan_generation: self.scan_generation,
            selection_generation: self.selection_generation,
        }
    }

    fn busy(&self) -> bool {
        self.pick.is_some()
            || self.scan_run.is_some()
            || self.validate_run.is_some()
            || self.copy_run.is_some()
    }

    fn live_reason(&self) -> Option<String> {
        if self.catalog.is_none() {
            return Some(NO_CATALOG.to_string());
        }
        if self.user.is_none() {
            return Some(NO_USER.to_string());
        }
        None
    }

    fn busy_reason(&self) -> Option<String> {
        self.busy()
            .then(|| "Wait for the current operation to finish.".to_string())
    }

    fn scan_blocked(&self) -> Option<String> {
        if let Some(reason) = self.live_reason() {
            return Some(reason);
        }
        if let Some(reason) = self.busy_reason() {
            return Some(reason);
        }
        if self.source.trim().is_empty() {
            return Some("Choose a source folder first.".to_string());
        }
        if !Path::new(self.source.trim()).is_absolute() {
            return Some("Use an absolute source folder path.".to_string());
        }
        if self.extensions.is_empty() {
            return Some("Choose at least one file type.".to_string());
        }
        None
    }

    fn validate_blocked(&self) -> Option<String> {
        if let Some(reason) = self.live_reason() {
            return Some(reason);
        }
        if let Some(reason) = self.busy_reason() {
            return Some(reason);
        }
        let Some(scanned) = &self.scanned else {
            return Some("Scan the source folder first.".to_string());
        };
        match scanned.snapshot.coverage {
            Coverage::Complete => {}
            Coverage::Partial => {
                return Some("Part of the source could not be read; scan again.".to_string());
            }
            Coverage::Cancelled => {
                return Some("The scan was cancelled; scan again.".to_string());
            }
            Coverage::RootUnavailable => {
                return Some("The source folder was unavailable during the scan.".to_string());
            }
        }
        if self.selected.is_empty() {
            return Some("Select at least one file.".to_string());
        }
        if self.destination.trim().is_empty() {
            return Some("Choose a destination folder first.".to_string());
        }
        if !Path::new(self.destination.trim()).is_absolute() {
            return Some("Use an absolute destination folder path.".to_string());
        }
        None
    }

    fn copy_blocked(&self) -> Option<String> {
        if let Some(reason) = self.live_reason() {
            return Some(reason);
        }
        if let Some(reason) = self.busy_reason() {
            return Some(reason);
        }
        let Some(review) = &self.review else {
            return Some("Validate the current selection first.".to_string());
        };
        if review.draft != self.draft() {
            return Some("Validate the current selection again.".to_string());
        }
        if review.ready.is_none() {
            return Some(review.refusal.clone().unwrap_or_else(|| {
                review
                    .items
                    .iter()
                    .find(|item| item.state == ReviewState::Blocked)
                    .and_then(|item| item.message.clone())
                    .unwrap_or_else(|| {
                        "The review has blocked items; fix them and validate again.".to_string()
                    })
            }));
        }
        None
    }

    // --------------------------------------------------------------- extensions

    fn add_extension(&mut self) {
        let raw = self.extension_entry.trim();
        if raw.is_empty() {
            return;
        }
        let mut text = raw.to_string();
        if !text.starts_with('.') {
            text.insert(0, '.');
        }
        // The legacy tool accepted `.` followed by letters and digits only.
        let valid = text.len() > 1 && text[1..].chars().all(|c| c.is_ascii_alphanumeric());
        if !valid {
            self.extension_error =
                Some("Use letters and digits only, for example .mov".to_string());
            return;
        }
        let extension = text.to_ascii_lowercase();
        self.extension_entry.clear();
        self.extension_error = None;
        if self.extensions.contains(&extension) {
            return;
        }
        self.extensions.push(extension);
        self.extensions.sort();
        self.sync_draft();
    }

    // ---------------------------------------------------------------- selection

    fn set_selected(&mut self, path: &Path, selected: bool) {
        let changed = if selected {
            self.selected.insert(path.to_path_buf())
        } else {
            self.selected.remove(path)
        };
        if changed {
            self.selection_generation += 1;
        }
    }

    fn select_all(&mut self) {
        if self.selected.len() == self.rows.len() {
            return;
        }
        self.selected = self.rows.iter().map(|row| row.path.clone()).collect();
        self.selection_generation += 1;
    }

    fn clear_selection(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        self.selected.clear();
        self.selection_generation += 1;
    }

    fn rebuild_row_states(&mut self) {
        self.row_states.clear();
        if let Some(review) = &self.review {
            for item in &review.items {
                if item.kind == ActionKind::Directory {
                    continue;
                }
                let Some(source) = review
                    .original_sources
                    .get(item.index)
                    .and_then(Option::as_ref)
                else {
                    continue;
                };
                self.row_states
                    .insert(source.clone(), review_row_state(item));
            }
            return;
        }
        if let Some(report) = &self.report {
            if report.draft != self.draft() {
                return;
            }
            for row in &report.rows {
                let Some(source) = &row.source else {
                    continue;
                };
                self.row_states.insert(
                    source.clone(),
                    RowState {
                        label: row.state.to_string(),
                        detail: row.message.clone(),
                        blocked: row.failed,
                    },
                );
            }
        }
    }

    // ------------------------------------------------------------------ workers

    fn begin_pick(&mut self, jobs: &mut Jobs, field: Field) {
        if self.pick.is_some() || jobs.is_shutting_down() {
            return;
        }
        let current = match field {
            Field::Source => self.source.trim(),
            Field::Destination => self.destination.trim(),
        };
        let start = if let Some(last) = self
            .backup
            .as_ref()
            .map(|job| job.last_browse_dir.trim())
            .filter(|last| !last.is_empty())
        {
            Some(PathBuf::from(last))
        } else if current.is_empty() {
            self.last_pick.clone()
        } else {
            Some(PathBuf::from(current))
        };
        let title = field.title().to_string();
        let (tx, rx) = channel();
        // The dialog runs on the tracked worker, like the existing folder picker:
        // the toolkits initialize on their own thread and the frame never blocks.
        let started = jobs.start(&self.ctx, title.clone(), move |context| {
            let mut dialog = rfd::FileDialog::new().set_title(title);
            if let Some(start) = start {
                dialog = dialog.set_directory(start);
            }
            let picked = dialog.pick_folder();
            let _ = tx.send(picked);
            context
                .events
                .finish(JobOutcome::Succeeded, JobProgress::default());
        });
        match started {
            Ok(job) => {
                self.pick = Some(PickRun {
                    job,
                    field,
                    replies: rx,
                })
            }
            Err(error) => {
                self.message = Some(format!("The folder chooser could not be opened: {error}"));
            }
        }
    }

    fn begin_scan(&mut self, jobs: &mut Jobs) {
        if jobs.is_shutting_down() {
            return;
        }
        if let Some(reason) = self.scan_blocked() {
            self.message = Some(reason);
            return;
        }
        let backup = self.backup_job();
        let worker_backup = backup.clone();
        let root = PathBuf::from(self.source.trim());
        let request = ScanRequest {
            root: root.clone(),
            purpose: ScanPurpose::ExtensionCopy,
            recursive: true,
            filters: ScanFilters {
                suffixes: self.extensions.clone(),
                mode: SuffixMode::NameSuffix,
                excluded_name_suffixes: Vec::new(),
                excludes: Vec::new(),
            },
        };
        let source = self.source.trim().to_string();
        let extensions = self.extensions.clone();
        let start_title = format!(
            "Scanning {}",
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string())
        );
        let (tx, rx) = channel();
        let started = jobs.start(&self.ctx, start_title, move |context| {
            let JobContext {
                control, events, ..
            } = context;
            let result = if let Some(job) = worker_backup {
                crate::backup::scan(&job, &control, &events).map(|scan| {
                    for issue in &scan.comparison_issues {
                        events.diagnostic(JobPhase::Scanning, issue.clone());
                    }
                    events.diagnostic(
                        JobPhase::Scanning,
                        format!(
                            "{} files compared; {} already up to date.",
                            scan.compared,
                            scan.up_to_date.len()
                        ),
                    );
                    let summary = format!(
                        "{} project files compared · {} up to date · {} need copying",
                        scan.compared,
                        scan.up_to_date.len(),
                        scan.snapshot.entries.len()
                    );
                    (scan.snapshot, Some(summary))
                })
            } else {
                discovery::walk(&request, &control, &events)
                    .map(|snapshot| (snapshot, None))
                    .map_err(|error| error.message().to_string())
            };
            let (payload, outcome, progress) = match result {
                Ok((snapshot, summary)) => {
                    let outcome = match snapshot.coverage {
                        Coverage::Complete => JobOutcome::Succeeded,
                        Coverage::Partial => JobOutcome::Partial,
                        Coverage::Cancelled => JobOutcome::Cancelled,
                        Coverage::RootUnavailable => JobOutcome::Failed,
                    };
                    let progress = JobProgress {
                        completed: snapshot.entries.len() as u64,
                        total: Some(snapshot.entries.len() as u64),
                        bytes_done: 0,
                        bytes_total: None,
                        current: None,
                    };
                    (Ok((snapshot, summary)), outcome, progress)
                }
                Err(error) => {
                    let message = error.to_string();
                    events.diagnostic(JobPhase::Scanning, message.clone());
                    (Err(message), JobOutcome::Failed, JobProgress::default())
                }
            };
            let _ = tx.send(payload);
            events.finish(outcome, progress);
        });
        match started {
            Ok(job) => {
                self.scan_run = Some(ScanRun {
                    backup,
                    job,
                    replies: rx,
                    source,
                    extensions,
                });
                self.scanned = None;
                self.rows.clear();
                self.selected.clear();
                self.row_states.clear();
                self.review = None;
                self.report = None;
                self.scan_error = None;
                self.message = None;
            }
            Err(error) => {
                self.message = Some(format!("The scan could not be started: {error}"));
            }
        }
    }

    fn begin_validation(&mut self, jobs: &mut Jobs) {
        if jobs.is_shutting_down() {
            return;
        }
        if let Some(reason) = self.validate_blocked() {
            self.message = Some(reason);
            return;
        }
        let Some(scanned) = &self.scanned else {
            return;
        };
        // The selected subset keeps the scan's own coverage and evidence: the
        // manifest is explicit, and an incomplete scan can never be trimmed into
        // a plan that looks complete.
        let entries: Vec<FileEntry> = scanned
            .snapshot
            .entries
            .iter()
            .filter(|entry| self.selected.contains(&entry.path))
            .cloned()
            .collect();
        let manifest = ScanSnapshot {
            entries,
            ..scanned.snapshot.clone()
        };
        let destination = PathBuf::from(self.destination.trim());
        let policy = self.policy;
        let draft = self.draft();
        self.plan_counter += 1;
        let revision = self.plan_counter;
        let total = manifest.entries.len() as u64;
        let (tx, rx) = channel();
        let started = jobs.start(&self.ctx, "Validating selection", move |context| {
            let JobContext {
                events, control, ..
            } = context;
            events.progress(
                JobPhase::Validating,
                JobProgress {
                    completed: 0,
                    total: Some(total),
                    bytes_done: 0,
                    bytes_total: None,
                    current: None,
                },
            );
            let planned = if manifest.purpose == ScanPurpose::Backup {
                transfers::plan_backup_copy("backup", revision, &manifest, &destination)
            } else {
                transfers::plan_extension_copy(
                    "copy-by-file-type",
                    revision,
                    &manifest,
                    &destination,
                    policy,
                )
            };
            let result = planned.map_err(|error| describe(&error)).map(|plan| {
                let original_sources = plan
                    .actions
                    .iter()
                    .map(|action| {
                        action
                            .source()
                            .map(|_| manifest.root.join(action.relative().path()))
                    })
                    .collect();
                ValidationDone {
                    report: transfers::validate_plan(&plan),
                    original_sources,
                }
            });
            let result = if control.checkpoint() == crate::jobs::Checkpoint::Cancelled {
                Err("Validation was cancelled; no copy was started.".into())
            } else {
                result
            };
            let (outcome, progress) = match &result {
                Ok(ValidationDone { report, .. }) => {
                    let outcome = if report.counts.blocked > 0 {
                        JobOutcome::Partial
                    } else {
                        JobOutcome::Succeeded
                    };
                    let reviewed = report.counts.total as u64;
                    (
                        outcome,
                        JobProgress {
                            completed: reviewed,
                            total: Some(reviewed),
                            bytes_done: 0,
                            bytes_total: None,
                            current: None,
                        },
                    )
                }
                Err(message) => {
                    events.diagnostic(JobPhase::Validating, message.clone());
                    (JobOutcome::Failed, JobProgress::default())
                }
            };
            let _ = tx.send(result);
            events.finish(outcome, progress);
        });
        match started {
            Ok(_) => {
                self.validate_run = Some(ValidateRun { draft, replies: rx });
                self.message = None;
            }
            Err(error) => {
                self.message = Some(format!("Validation could not be started: {error}"));
            }
        }
    }

    fn begin_copy(&mut self, jobs: &mut Jobs) {
        if jobs.is_shutting_down() {
            return;
        }
        if let Some(reason) = self.copy_blocked() {
            self.message = Some(reason);
            return;
        }
        let Some(catalog) = self.catalog.clone() else {
            self.message = Some(NO_CATALOG.to_string());
            return;
        };
        let Some(user) = self.user else {
            self.message = Some(NO_USER.to_string());
            return;
        };
        let Some(mut review) = self.review.take() else {
            return;
        };
        let Some(validated) = review.ready.take() else {
            self.review = Some(review);
            return;
        };
        let validated = validated.attribute_to(user);
        let labels = review
            .items
            .iter()
            .map(|item| ItemLabel {
                source: review.original_sources.get(item.index).cloned().flatten(),
                destination: item
                    .destination
                    .as_ref()
                    .map(|path| path_display(path.path()))
                    .unwrap_or_else(|| "—".to_string()),
                kind: item_kind_label(item),
            })
            .collect();
        let draft = review.draft.clone();
        let workers = draft.backup.as_ref().map(|job| job.workers).unwrap_or(1);
        self.review = Some(review);
        let (tx, rx) = channel();
        let started =
            jobs.start(&self.ctx, "Copying files", move |context| {
                let JobContext {
                    control, events, ..
                } = context;
                let result = transfers::execute_with_workers(
                    &catalog, validated, &control, &events, workers,
                )
                .map_err(|error| {
                    let mut message = describe(&error);
                    let inspection = error.run_id().and_then(|run_id| {
                        match transfers::read_run(&catalog, run_id).and_then(|reply| reply.wait()) {
                            Ok(inspection) => inspection,
                            Err(read_error) => {
                                message
                                    .push_str(&format!(" Journal inspection failed: {read_error}"));
                                None
                            }
                        }
                    });
                    CopyFailure {
                        message,
                        run_id: error.run_id(),
                        inspection,
                        progress: events.last_progress(),
                    }
                });
                let (outcome, progress) = match &result {
                    Ok(report) => {
                        let progress = report_progress(report);
                        (job_outcome(report.summary.state), progress)
                    }
                    Err(failure) => {
                        events.diagnostic(JobPhase::Copying, failure.message.clone());
                        (JobOutcome::Failed, failure.progress.clone())
                    }
                };
                let _ = tx.send(result);
                events.finish(outcome, progress);
            });
        match started {
            Ok(job) => {
                self.copy_run = Some(CopyRun {
                    job,
                    replies: rx,
                    labels,
                    draft,
                });
                self.report = None;
                self.message = None;
                self.rebuild_row_states();
            }
            Err(error) => {
                self.review = None;
                self.report = None;
                self.rebuild_row_states();
                self.message = Some(format!(
                    "The copy could not be started: {error}. Validate the selection again before retrying."
                ));
            }
        }
    }

    // ---------------------------------------------------------------------- UI

    /// The working page: scan toolbar, setup fields, file table and the progress
    /// and results dock. Everything stays on this page; no tab hides the work.
    pub fn view(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        let mut scan = false;
        let mut pick: Option<Field> = None;

        egui::Panel::top("copy_toolbar").show(ui, |ui| {
            ui.horizontal(|ui| {
                let blocked = self.scan_blocked();
                let response = accent_button(ui, "Scan folders", blocked.is_none());
                if response.clicked() {
                    scan = true;
                }
                if let Some(reason) = blocked {
                    explain(&response, &reason);
                }
                ui.label(
                    egui::RichText::new(if self.backup.is_some() { "Back up newer Nuke and After Effects files; preserve older destination versions." } else { "Copy selected file types and keep folders and filenames." })
                        .small()
                        .weak(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(self.discovery_summary()).small().weak());
                });
            });
        });

        egui::Panel::bottom("copy_progress_dock")
            .resizable(true)
            .default_size(240.0)
            .min_size(180.0)
            .max_size(600.0)
            .show(ui, |ui| self.progress_dock(ui, jobs));

        egui::CentralPanel::default().show(ui, |ui| {
            let editable = !self.busy();
            ui.add_enabled_ui(editable, |ui| {
                self.setup(ui, &mut pick);
            });
            ui.separator();
            if let Some(message) = &self.message {
                ui.label(egui::RichText::new(message).small().color(WARNING));
            }
            self.file_table(ui);
        });

        if scan {
            self.begin_scan(jobs);
        }
        if let Some(field) = pick {
            self.begin_pick(jobs, field);
        }
        self.sync_draft();
        self.invalidate_review();
    }

    /// The bottom strip's validation control and the review state beside it.
    pub fn validate_control(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        let blocked = self.validate_blocked();
        let response = ui.add_enabled(blocked.is_none(), egui::Button::new("Validate selection"));
        if response.clicked() {
            self.begin_validation(jobs);
        }
        if let Some(reason) = blocked {
            explain(&response, &reason);
        }
        if let Some(state) = self.state_text() {
            ui.label(egui::RichText::new(state).small().weak());
        }
    }

    /// The bottom strip's single execution control. A copy is admitted only for
    /// the validated plan that matches the current draft, and only one run at a
    /// time.
    pub fn execution_control(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        let blocked = self.copy_blocked();
        let response = accent_button(
            ui,
            if self.backup.is_some() {
                "Run backup"
            } else {
                "Copy selected"
            },
            blocked.is_none(),
        );
        if response.clicked() {
            self.begin_copy(jobs);
        }
        if let Some(reason) = blocked {
            explain(&response, &reason);
        }
    }

    pub fn execution_blocked_reason(&self) -> Option<String> {
        self.copy_blocked()
    }

    fn state_text(&self) -> Option<String> {
        let base = if self.copy_run.is_some() {
            "Copying files.".to_string()
        } else if self.validate_run.is_some() {
            "Validating the current selection.".to_string()
        } else {
            match &self.review {
                None => "Draft · validate the current file list before copying.".to_string(),
                Some(review) if review.draft != self.draft() => {
                    "The selection or settings changed; validate again.".to_string()
                }
                Some(review) => match &review.refusal {
                    Some(refusal) => refusal.clone(),
                    None if review.ready.is_some() => format!(
                        "Validated · {} file(s) will copy, {} will skip",
                        review.files_copy, review.files_skip
                    ),
                    None => review
                        .items
                        .iter()
                        .find(|item| item.state == ReviewState::Blocked)
                        .and_then(|item| item.message.clone())
                        .map(|reason| format!("Cannot copy: {reason}"))
                        .unwrap_or_else(|| {
                            "The review has blocked items; fix them and validate again.".to_string()
                        }),
                },
            }
        };
        Some(format!(
            "{base} · Existing files: {}",
            policy_label(self.policy)
        ))
    }

    fn discovery_summary(&self) -> String {
        if self.scan_run.is_some() {
            return "Scanning…".to_string();
        }
        if self.scan_error.is_some() {
            return "Scan failed".to_string();
        }
        let Some(scanned) = &self.scanned else {
            return "Not scanned".to_string();
        };
        if let Some(summary) = &scanned.summary {
            return summary.clone();
        }
        let mut text = format!(
            "{} matching file(s) · {} selected",
            self.rows.len(),
            self.selected.len()
        );
        match scanned.snapshot.coverage {
            Coverage::Complete => {}
            Coverage::Partial => text.push_str(" · partial scan"),
            Coverage::Cancelled => text.push_str(" · cancelled"),
            Coverage::RootUnavailable => text.push_str(" · source unavailable"),
        }
        text
    }

    fn setup(&mut self, ui: &mut egui::Ui, pick: &mut Option<Field>) {
        egui::Grid::new("copy_setup")
            .num_columns(3)
            .spacing(egui::vec2(8.0, 6.0))
            .show(ui, |ui| {
                for (field, value) in [
                    (Field::Source, &mut self.source),
                    (Field::Destination, &mut self.destination),
                ] {
                    ui.label(egui::RichText::new(field.title()).small().weak());
                    ui.add_sized(
                        [540.0, ui.spacing().interact_size.y],
                        egui::TextEdit::singleline(value)
                            .hint_text(match field {
                                Field::Source => "Source folder",
                                Field::Destination => "Destination folder",
                            })
                            .font(egui::TextStyle::Monospace)
                            .desired_width(420.0),
                    );
                    let browse = ui.button("Browse…");
                    if browse.clicked() {
                        *pick = Some(field);
                    }
                    ui.end_row();
                }
            });

        if let Some(job) = &mut self.backup {
            ui.horizontal(|ui| {
                ui.label("Exclude");
                ui.add(
                    egui::TextEdit::singleline(&mut job.excludes)
                        .desired_width(540.0)
                        .hint_text("Comma-separated names or patterns, e.g. #Backup, *.tmp"),
                );
                ui.label("Copy workers");
                ui.add(egui::DragValue::new(&mut job.workers).range(1..=16));
            });
            ui.label(egui::RichText::new(".nk and .aep · only missing or newer files · older destinations are kept under numbered names").small().weak());
            return;
        }

        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("File types").small().weak());
            let mut removed: Option<usize> = None;
            for (index, extension) in self.extensions.iter().enumerate() {
                if ui.small_button(format!("{extension} ✕")).clicked() {
                    removed = Some(index);
                }
            }
            if let Some(index) = removed {
                self.extensions.remove(index);
                self.sync_draft();
            }
            ui.separator();
            ui.add(
                egui::TextEdit::singleline(&mut self.extension_entry)
                    .hint_text("Other, e.g. .mov")
                    .desired_width(130.0),
            );
            if ui.button("Add").clicked() {
                self.add_extension();
            }
            if let Some(error) = &self.extension_error {
                ui.label(egui::RichText::new(error).small().color(WARNING));
            }
        });

        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Existing files").small().weak());
            let picked = egui::ComboBox::from_id_salt("copy_policy")
                .width(150.0)
                .selected_text(policy_label(self.policy))
                .show_ui(ui, |ui| {
                    for policy in POLICIES {
                        ui.selectable_value(&mut self.policy, policy, policy_label(policy));
                    }
                });
            picked.response.on_hover_text(policy_hint(self.policy));
        });
    }

    fn file_table(&mut self, ui: &mut egui::Ui) {
        let list_summary = self.list_summary();
        let editable = !self.busy();
        let mut toggles: Vec<(PathBuf, bool)> = Vec::new();
        let mut select_all = false;
        let mut clear = false;

        egui::CollapsingHeader::new(format!("Files to copy  ·  {list_summary}"))
            .id_salt("copy_files_header")
            .default_open(true)
            .show(ui, |ui| {
                if self.rows.is_empty() {
                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(if self.backup.is_some() && self.scanned.as_ref().is_some_and(|scan| scan.snapshot.coverage == Coverage::Complete) {
                            "No backup copies are needed. Matching project files are up to date."
                        } else if self.scanned.is_some() {
                            "No files of the selected types were found."
                        } else {
                            "Scan the source folder to list the matching files."
                        })
                        .weak(),
                    );
                    return;
                }
                egui_extras::TableBuilder::new(ui)
                    .id_salt("copy_files_table")
                    .resizable(true)
                    .striped(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(egui_extras::Column::auto())
                    .column(
                        egui_extras::Column::initial(360.0)
                            .at_least(160.0)
                            .clip(true),
                    )
                    .column(
                        egui_extras::Column::initial(300.0)
                            .at_least(140.0)
                            .clip(true),
                    )
                    .column(egui_extras::Column::initial(84.0).at_least(64.0))
                    .column(egui_extras::Column::remainder().at_least(104.0))
                    .header(20.0, |mut header| {
                        for title in ["", "File and folder", "Destination", "Size", "State"] {
                            header.col(|ui| {
                                ui.label(title);
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(20.0, self.rows.len(), |mut row| {
                            let index = row.index();
                            let Some(entry) = self.rows.get(index) else {
                                return;
                            };
                            let mut selected = self.selected.contains(&entry.path);
                            row.col(|ui| {
                                if ui
                                    .add_enabled(editable, egui::Checkbox::new(&mut selected, ""))
                                    .changed()
                                {
                                    toggles.push((entry.path.clone(), selected));
                                }
                            });
                            row.col(|ui| {
                                ui.label(&entry.relative)
                                    .on_hover_text(path_display(&entry.path));
                            });
                            row.col(|ui| {
                                let root = self.destination.trim();
                                if root.is_empty() {
                                    ui.label(&entry.relative)
                                        .on_hover_text("Choose a destination folder");
                                    return;
                                }
                                let full = Path::new(root).join(&entry.relative);
                                let label = match Path::new(root).file_name() {
                                    Some(name) => {
                                        format!("{}/{}", name.to_string_lossy(), entry.relative)
                                    }
                                    None => path_display(&full),
                                };
                                ui.label(label).on_hover_text(path_display(&full));
                            });
                            row.col(|ui| {
                                ui.label(egui::RichText::new(size_text(entry.size)).small());
                            });
                            row.col(|ui| {
                                match self.row_states.get(&entry.path).filter(|_| {
                                    self.review.is_some()
                                        || self
                                            .report
                                            .as_ref()
                                            .is_none_or(|report| report.draft == self.draft())
                                }) {
                                    Some(state) => {
                                        let text = egui::RichText::new(&state.label).small();
                                        let text = if state.blocked {
                                            text.color(WARNING)
                                        } else {
                                            text
                                        };
                                        let response = ui.label(text);
                                        if let Some(detail) = &state.detail {
                                            response.on_hover_text(detail);
                                        }
                                    }
                                    None => {
                                        if selected {
                                            ui.label(
                                                egui::RichText::new("Will copy").small().weak(),
                                            );
                                        } else {
                                            ui.label(
                                                egui::RichText::new("Not selected").small().weak(),
                                            );
                                        }
                                    }
                                }
                            });
                        });
                    });
            });

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Destination paths keep their source folders")
                    .small()
                    .weak(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(editable, egui::Button::new("Clear").small())
                    .clicked()
                {
                    clear = true;
                }
                if ui
                    .add_enabled(editable, egui::Button::new("Select all").small())
                    .clicked()
                {
                    select_all = true;
                }
                if let Some(error) = &self.scan_error {
                    ui.label(egui::RichText::new(error).small().color(WARNING));
                }
            });
        });

        for (path, selected) in toggles {
            self.set_selected(&path, selected);
        }
        if select_all {
            self.select_all();
        }
        if clear {
            self.clear_selection();
        }
    }

    fn list_summary(&self) -> String {
        let selected = self.selected.len();
        let Some(review) = &self.review else {
            return format!("{selected} selected");
        };
        let mut text = format!(
            "{selected} selected · {} will copy · {} will skip",
            review.files_copy, review.files_skip
        );
        if review.folders > 0 {
            text.push_str(&format!(" · {} folder(s)", review.folders));
        }
        if review.files_blocked > 0 {
            text.push_str(&format!(" · {} blocked", review.files_blocked));
        }
        text
    }

    fn progress_dock(&mut self, ui: &mut egui::Ui, jobs: &Jobs) {
        egui::ScrollArea::vertical()
            .id_salt("copy_progress_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let scan_job = job_status(jobs, self.scan_run.as_ref().map(|run| run.job));
                let scan_label = match scan_job {
                    Some(job) => format!("{} found", job.progress.completed),
                    None => match (&self.scanned, &self.scan_error) {
                        (Some(scanned), _) => match scanned.snapshot.coverage {
                            Coverage::Complete => format!("{} files", self.rows.len()),
                            Coverage::Partial => "Incomplete".to_string(),
                            Coverage::Cancelled => "Cancelled".to_string(),
                            Coverage::RootUnavailable => "Source unavailable".to_string(),
                        },
                        (None, Some(_)) => "Failed".to_string(),
                        (None, None) => "Ready".to_string(),
                    },
                };
                let scan_fraction = match scan_job {
                    Some(job) if !job.is_terminal() && job.progress.total.is_none() => None,
                    Some(job) => Some(job.progress.fraction()),
                    None => {
                        if self.scanned.is_some() {
                            Some(1.0)
                        } else {
                            Some(0.0)
                        }
                    }
                };
                progress_block(
                    ui,
                    "Scan folders",
                    &scan_label,
                    scan_fraction,
                    scan_job
                        .and_then(|job| job.progress.current.as_ref())
                        .map(|path| path_display(path))
                        .unwrap_or_else(|| {
                            if self.scanned.is_some() {
                                "Folders scanned.".to_string()
                            } else {
                                "Waiting to scan source folders".to_string()
                            }
                        }),
                );

                let copy_job = job_status(jobs, self.copy_run.as_ref().map(|run| run.job));
                let copy_label = match copy_job {
                    Some(job) => match job.progress.total {
                        Some(total) => format!("{} of {total} items", job.progress.completed),
                        None => format!("{} items", job.progress.completed),
                    },
                    None => match &self.report {
                        Some(report) if report.error.is_some() => "Stopped — inspect results".to_string(),
                        Some(report) => format!("{} item(s) reported", report.counts.total),
                        None => "Waiting for a validated file list".to_string(),
                    },
                };
                let copy_fraction = match copy_job {
                    Some(job) if !job.is_terminal() && job.progress.total.is_none() => None,
                    Some(job) => Some(job.progress.fraction()),
                    None => {
                        Some(self.report.as_ref().map(|report| report.progress.fraction()).unwrap_or(0.0))
                    }
                };
                progress_block(
                    ui,
                    "Copy files",
                    &copy_label,
                    copy_fraction,
                    copy_job
                        .and_then(|job| job.progress.current.as_ref())
                        .map(|path| path_display(path))
                        .unwrap_or_else(|| {
                            if let Some(report) = &self.report {
                                if report.error.is_some() {
                                    "Copy stopped. Some destinations may have changed; inspect the item outcomes before retrying.".to_string()
                                } else {
                                    run_headline(report)
                                }
                            } else {
                                "Waiting to copy".to_string()
                            }
                        }),
                );

                if let Some(review) = &self.review {
                    if !review.warnings.is_empty() {
                        ui.collapsing("Review details (does not block copying)", |ui| {
                            for warning in &review.warnings {
                                ui.label(egui::RichText::new(warning).small());
                            }
                        });
                    }
                    if review.space_unknown {
                        ui.label(
                            egui::RichText::new(
                                "Free space at the destination is not estimated by this build.",
                            )
                            .small()
                            .weak(),
                        );
                    }
                }

                if let Some(report) = &self.report {
                    let headline = if report.draft != self.draft() {
                        format!("Previous run · {}", run_headline(report))
                    } else {
                        run_headline(report)
                    };
                    egui::CollapsingHeader::new(headline)
                        .id_salt("copy_results")
                        .default_open(report.error.is_some() || report.state != RunState::Succeeded)
                        .show(ui, |ui| {
                            if report.draft != self.draft() {
                                ui.label("Previous run — the current selection or settings have changed.");
                            }
                            ui.label(format!("Destination: {}", report.draft.destination));
                            if let Some(run_id) = report.run_id {
                                ui.label(format!("Journal run {run_id}"));
                            }
                            if let Some(error) = &report.error {
                                ui.label(egui::RichText::new(error).color(WARNING));
                            }
                            ui.label(egui::RichText::new(run_counts_text(&report.counts)).small());
                            for warning in &report.warnings {
                                ui.label(egui::RichText::new(warning).small().color(WARNING));
                            }
                            for row in &report.rows {
                                ui.horizontal(|ui| {
                                    let text = egui::RichText::new(row.state).small();
                                    let text = if row.failed {
                                        text.color(WARNING)
                                    } else {
                                        text
                                    };
                                    ui.label(text);
                                    ui.label(egui::RichText::new(row.kind).small().weak());
                                    ui.label(
                                        egui::RichText::new(&row.destination).small().monospace(),
                                    )
                                    .on_hover_text(&row.destination);
                                    if row.bytes > 0 {
                                        ui.label(
                                            egui::RichText::new(size_text(row.bytes))
                                                .small()
                                                .weak(),
                                        );
                                    }
                                });
                                if let Some(message) = &row.message {
                                    ui.label(egui::RichText::new(message).small().weak());
                                }
                                for recovery in &row.recovery {
                                    ui.label(
                                        egui::RichText::new(format!("Recovery: {recovery}"))
                                            .small()
                                            .color(WARNING),
                                    );
                                }
                            }
                        });
                }
            });
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn accent_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let text = egui::RichText::new(label).color(egui::Color32::WHITE);
    ui.add_enabled(enabled, egui::Button::new(text).fill(ACCENT))
}

fn explain(response: &egui::Response, reason: &str) {
    let reason = reason.to_string();
    let _ = response
        .clone()
        .on_disabled_hover_text(reason.clone())
        .on_hover_text(reason);
}

fn progress_block(
    ui: &mut egui::Ui,
    label: &str,
    count: &str,
    fraction: Option<f32>,
    current: String,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(count).small().weak());
        });
    });
    if let Some(fraction) = fraction {
        ui.add(
            egui::ProgressBar::new(fraction.clamp(0.0, 1.0))
                .desired_height(8.0)
                .show_percentage(),
        );
    } else {
        ui.add(
            egui::ProgressBar::new(0.0)
                .text("Counting matching files…")
                .desired_height(16.0),
        );
    }
    // Changing paths must not add rows to the resizable progress dock.
    ui.add(egui::Label::new(egui::RichText::new(&current).small().weak()).truncate())
        .on_hover_text(&current);
}

fn job_status(jobs: &Jobs, id: Option<JobId>) -> Option<&crate::jobs::JobStatus> {
    let id = id?;
    jobs.statuses().find(|status| status.id == id)
}

fn table_rows(snapshot: &ScanSnapshot) -> Vec<TableRow> {
    let mut rows: Vec<TableRow> = snapshot
        .entries
        .iter()
        .map(|entry| TableRow {
            path: entry.path.clone(),
            relative: entry
                .path
                .strip_prefix(&snapshot.root)
                .map(path_display)
                .unwrap_or_else(|_| path_display(&entry.path)),
            size: entry.stamp.size,
        })
        .collect();
    rows.sort_by(|a, b| a.relative.cmp(&b.relative));
    rows
}

fn count_review(items: &[ItemValidation]) -> ReviewCounts {
    let mut counts = ReviewCounts::default();
    for item in items {
        if item.kind == ActionKind::Directory {
            counts.folders += 1;
            continue;
        }
        match item.state {
            ReviewState::Ready => counts.copy += 1,
            ReviewState::Skipped => counts.skip += 1,
            ReviewState::Blocked => counts.blocked += 1,
        }
    }
    counts
}

fn review_row_state(item: &ItemValidation) -> RowState {
    let (label, blocked) = match (item.state, item.effect) {
        (ReviewState::Blocked, _) => ("Blocked", true),
        (ReviewState::Skipped, _) | (_, ItemEffect::Skip) => ("Will skip", false),
        (ReviewState::Ready, ItemEffect::Copy) => ("Will copy", false),
        (ReviewState::Ready, _) => ("Will create", false),
    };
    RowState {
        label: label.to_string(),
        detail: item.message.clone(),
        blocked,
    }
}

fn report_rows(report: &RunReport, labels: &[ItemLabel]) -> Vec<ReportRow> {
    report
        .items
        .iter()
        .map(|item| {
            let label = usize::try_from(item.ordinal)
                .ok()
                .and_then(|index| labels.get(index));
            ReportRow {
                source: label.and_then(|label| label.source.clone()),
                kind: label.map(|label| label.kind).unwrap_or("Item"),
                destination: label
                    .map(|label| label.destination.clone())
                    .unwrap_or_else(|| "—".to_string()),
                state: item_state_label(item.state),
                failed: matches!(
                    item.state,
                    ItemState::Failed | ItemState::CopiedSourceRetained | ItemState::Cancelled
                ),
                message: item.message.clone(),
                recovery: item
                    .recovery
                    .iter()
                    .map(|path| path_display(path))
                    .collect(),
                bytes: item.committed_bytes,
            }
        })
        .collect()
}

fn report_progress(report: &RunReport) -> JobProgress {
    let counts = report.summary.counts;
    JobProgress {
        completed: (counts.total - counts.unfinished()) as u64,
        total: Some(counts.total as u64),
        bytes_done: counts.committed_bytes.unwrap_or(0),
        bytes_total: None,
        current: None,
    }
}

fn failed_run(
    failure: CopyFailure,
    labels: &[ItemLabel],
    draft: Draft,
    mut warnings: Vec<String>,
) -> RunDone {
    let mut counts = RunCounts {
        total: labels.len(),
        ..RunCounts::default()
    };
    let mut state = RunState::Interrupted;
    let mut inspected = HashMap::new();
    let mut trustworthy = false;
    if let Some(inspection) = &failure.inspection {
        for warning in &inspection.warnings {
            if !warnings.contains(warning) {
                warnings.push(warning.clone());
            }
        }
        if let Some(error) = &inspection.detail_error {
            warnings.push(format!("Journal inspection: {error}"));
        } else {
            trustworthy = true;
            state = inspection.state;
            if let Some(confirmed) = inspection.confirmed_counts() {
                counts = confirmed;
            }
        }
        inspected.extend(inspection.items.iter().map(|item| (item.ordinal, item)));
    }
    let rows = labels.iter().enumerate().map(|(ordinal, label)| {
        let item = inspected.get(&(ordinal as i64)).copied();
        let confirmed = item.filter(|item| trustworthy && item.message_error.is_none()
            && !matches!(item.state, ItemState::Pending | ItemState::Running));
        let mut messages = Vec::new();
        let mut recovery = Vec::new();
        if let Some(item) = item {
            if let Some(error) = &item.message_error {
                messages.push(format!("Journal inspection: {error}"));
            }
            for event in &item.events {
                if let Some(message) = &event.message {
                    messages.push(message.clone());
                }
                for path in event.recovery_paths() {
                    let path = path_display(path);
                    if !recovery.contains(&path) { recovery.push(path); }
                }
            }
        }
        if confirmed.is_none() {
            messages.push("No confirmed terminal outcome. The destination may have changed; inspect it before retrying.".to_string());
        }
        ReportRow {
            source: label.source.clone(),
            kind: label.kind,
            destination: label.destination.clone(),
            state: confirmed.map(|item| item_state_label(item.state)).unwrap_or("Outcome unknown"),
            failed: confirmed.is_none_or(|item| matches!(item.state, ItemState::Failed | ItemState::Cancelled | ItemState::CopiedSourceRetained)),
            message: Some(messages.join("\n")),
            recovery,
            bytes: confirmed.and_then(|item| item.events.last()).and_then(|event| event.committed_bytes).unwrap_or(0),
        }
    }).collect();
    RunDone {
        draft,
        run_id: failure.run_id,
        error: Some(failure.message),
        progress: failure.progress,
        state,
        counts,
        rows,
        warnings,
    }
}

fn item_kind_label(item: &ItemValidation) -> &'static str {
    match item.kind {
        ActionKind::Directory => "Folder",
        ActionKind::File => "File",
        ActionKind::CreateFile => "New file",
    }
}

fn item_state_label(state: ItemState) -> &'static str {
    match state {
        ItemState::Pending => "Not started",
        ItemState::Running => "Copying",
        ItemState::Copied => "Copied",
        ItemState::Moved => "Moved",
        ItemState::Created => "Created",
        ItemState::Skipped => "Skipped",
        ItemState::CopiedSourceRetained => "Copied, source kept",
        ItemState::Failed => "Failed",
        ItemState::Cancelled => "Cancelled",
    }
}

fn job_outcome(state: RunState) -> JobOutcome {
    match state {
        RunState::Succeeded => JobOutcome::Succeeded,
        RunState::Partial => JobOutcome::Partial,
        RunState::Failed => JobOutcome::Failed,
        RunState::Cancelled => JobOutcome::Cancelled,
        RunState::Pending | RunState::Running | RunState::Interrupted => JobOutcome::Failed,
    }
}

fn run_headline(report: &RunDone) -> String {
    if report.error.is_some() {
        return "Copy stopped — inspect the recorded outcomes".to_string();
    }
    let counts = &report.counts;
    let mut text = match report.state {
        RunState::Succeeded => "Copy finished".to_string(),
        RunState::Partial => "Copy finished with failures".to_string(),
        RunState::Failed => "Copy failed".to_string(),
        RunState::Cancelled => {
            "Copy cancelled · completed files remain at the destination".to_string()
        }
        RunState::Pending | RunState::Running | RunState::Interrupted => {
            "Copy result is incomplete".to_string()
        }
    };
    let details = run_counts_text(counts);
    if !details.is_empty() {
        text.push_str(" · ");
        text.push_str(&details);
    }
    text
}

/// Files and folders stay distinct: `copied` counts files, `created` counts the
/// destination folders the plan actually created.
fn run_counts_text(counts: &RunCounts) -> String {
    let mut parts = Vec::new();
    if counts.copied > 0 {
        parts.push(format!("{} file(s) copied", counts.copied));
    }
    if counts.moved > 0 {
        parts.push(format!("{} file(s) moved", counts.moved));
    }
    if counts.created > 0 {
        parts.push(format!("{} folder(s) created", counts.created));
    }
    if counts.skipped > 0 {
        parts.push(format!("{} skipped", counts.skipped));
    }
    if counts.failed > 0 {
        parts.push(format!("{} failed", counts.failed));
    }
    if counts.copied_source_retained > 0 {
        parts.push(format!(
            "{} kept their source",
            counts.copied_source_retained
        ));
    }
    if counts.cancelled > 0 {
        parts.push(format!("{} not started", counts.cancelled));
    }
    if let Some(bytes) = counts.committed_bytes {
        if bytes > 0 {
            parts.push(format!("{} written", size_text(bytes)));
        }
    }
    parts.join(" · ")
}

fn describe(error: &TransferError) -> String {
    match error.path() {
        Some(path) => format!("{} ({})", error.message(), path_display(path)),
        None => error.message().to_string(),
    }
}

fn path_display(path: &Path) -> String {
    path.display().to_string()
}

fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
