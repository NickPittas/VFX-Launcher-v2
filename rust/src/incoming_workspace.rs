//! I5: Organize Incoming — the source-to-destination workspace.
//!
//! Ported from `CleanIncomings`, whose pipeline is the reference: scan a messy
//! incoming folder, group its frames into sequences
//! (`python/mapping_utils/group_image_sequences.py`), extract shot/task/asset/
//! stage/version/resolution tags from filenames only
//! (`python/mapping_utils/*_extractor.py`), route each item through the profile
//! rules (`python/mapping_utils/generate_simple_target_path.py`) and copy or
//! move the files into the resulting folders.
//!
//! The legacy tool wrote wherever its last computation pointed. This port keeps
//! the reviewed path: discovery produces one snapshot, [`crate::incoming`] turns
//! the selected items into concrete proposals,
//! [`crate::incoming::plan_incoming`] expands them into one plan of exact member
//! files, and only a plan validated against the exact roots, selection, tags,
//! profile, action and conflict policy may execute through
//! [`crate::transfers`]. Any edit invalidates the review until the current
//! selection is validated again.
//!
//! Nothing here touches the filesystem or the catalog on the UI thread:
//! scanning, matching, validating, previewing the destination tree and
//! executing are each one tracked [`Jobs`] worker that reports through the job
//! transport. The frame renders state and drains channels only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;

use crate::discovery::{
    self, Coverage, FileEntry, ScanFilters, ScanPurpose, ScanRequest, ScanSnapshot, SuffixMode,
};
use crate::incoming::{
    self, IncomingAnalysis, IncomingItem, IncomingPlanRequest, ItemEdit, MappingProposal,
    SingleKind,
};
use crate::jobs::{
    Checkpoint, JobContext, JobControl, JobId, JobOutcome, JobPhase, JobProgress, JobStatus, Jobs,
};
use crate::patterns::{
    CompiledPatterns, IncomingProfile, IncomingRule, MappingState, PatternRule, PatternSet,
    PatternStore, ProfileSet, RuleMode, TagEdits, TagKind,
};
use crate::storage::{CatalogClient, UserId};
use crate::transfers::{
    self, ActionKind, ConflictPolicy, FileAction, ItemInspection, ItemState, ItemValidation,
    ReviewState, RunCounts, RunInspection, RunReport, RunState, TransferError, ValidatedPlan,
    ValidationReport,
};

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2f, 0x6b, 0xa8);
const WARNING: egui::Color32 = egui::Color32::from_rgb(0xe0, 0x9a, 0x6a);
const DANGER: egui::Color32 = egui::Color32::from_rgb(0xd8, 0x5f, 0x55);
const NEW_ENTRY: egui::Color32 = egui::Color32::from_rgb(0x7d, 0xb8, 0x7a);
const MUTED: egui::Color32 = egui::Color32::from_gray(150);
const NO_CATALOG: &str = "The catalog is not ready yet.";
const NO_USER: &str = "Choose who is using the app first.";
const POLICIES: [ConflictPolicy; 4] = [
    ConflictPolicy::Skip,
    ConflictPolicy::Replace,
    ConflictPolicy::KeepBoth,
    ConflictPolicy::PreserveExisting,
];
/// Destination preview limits: the tree is evidence for the user, not an
/// inventory, so a huge root cannot stall the page.
const PREVIEW_MAX_DEPTH: usize = 8;
const PREVIEW_MAX_ENTRIES: usize = 4000;
/// How many reviewed items or preview entries pass between two cancellation
/// checkpoints in a UI worker.
const CHECKPOINT_STRIDE: usize = 64;
/// Reply of a validation the user cancelled before it finished.
const CANCELLED_VALIDATION: &str =
    "The validation was cancelled before it finished; validate the selection again.";
/// Longest list of missing frame numbers spelled out in one row.
const MISSING_FRAMES_SHOWN: usize = 6;

/// Smallest folder field the toolbar still lays out on one row.
const PATH_MIN_WIDTH: f32 = 150.0;
/// Working list and run report row heights.
const ITEM_ROW_HEIGHT: f32 = 22.0;
const REPORT_ROW_HEIGHT: f32 = 18.0;
/// Height of the member list inside one expanded row.
const MEMBER_SCROLL_HEIGHT: f32 = 120.0;

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
        ConflictPolicy::Replace => "Replace an existing destination file with the incoming one.",
        ConflictPolicy::KeepBoth => "Keep both by numbering the incoming file.",
        ConflictPolicy::PreserveExisting => {
            "Keep the existing destination file and set it aside under a numbered name."
        }
    }
}

/// Which folder a browse request fills.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
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

/// A configuration file the editor reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfigFile {
    Patterns,
    Profiles,
}

impl ConfigFile {
    fn title(self) -> &'static str {
        match self {
            Self::Patterns => "pattern file",
            Self::Profiles => "profile file",
        }
    }

    fn default_name(self) -> &'static str {
        match self {
            Self::Patterns => "patterns.json",
            Self::Profiles => "profiles.json",
        }
    }
}

/// What one browse worker does.
#[derive(Clone)]
enum PickTask {
    Folder(Field),
    Import(ConfigFile),
    Export(ConfigFile, String),
}

/// What a browse worker produced.
enum PickOutcome {
    Folder(PathBuf),
    Imported(ConfigFile, String),
    Exported(ConfigFile, PathBuf),
}

struct PickRun {
    job: JobId,
    task: PickTask,
    replies: Receiver<Result<Option<PickOutcome>, String>>,
}

/// What a configuration worker produced.
enum ConfigOutcome {
    Loaded {
        patterns: PatternSet,
        profiles: ProfileSet,
        seeded: bool,
    },
    Saved(ConfigFile),
}

struct ConfigRun {
    job: JobId,
    title: &'static str,
    replies: Receiver<Result<ConfigOutcome, String>>,
}

/// One scan + analysis + match worker.
struct ScanRun {
    job: JobId,
    replies: Receiver<Result<Analyzed, String>>,
    /// Pattern revision and profile the worker matched with, so a reply taken
    /// with rules the user has since edited is matched again instead of
    /// installed as if it were current.
    patterns_revision: u64,
    profile: String,
}

/// Everything a completed scan produced for the configuration it was taken with.
struct Analyzed {
    snapshot: ScanSnapshot,
    analysis: IncomingAnalysis,
    proposals: Vec<MappingProposal>,
}

/// A completed scan, bound to the source folder it came from.
struct ScanResult {
    snapshot: ScanSnapshot,
    analysis: IncomingAnalysis,
}

struct ValidateRun {
    job: JobId,
    draft: Draft,
    replies: Receiver<Result<ValidationReport, String>>,
}

/// Where one reviewed item writes, so a run report can name it after the review
/// is gone. One label per reviewed ordinal, directories included: the journal
/// numbers every plan item, so dropping an ordinal here would shift every later
/// row onto the wrong source and destination.
struct ItemLabel {
    source: PathBuf,
    destination: String,
    directory: bool,
}

struct ExecuteRun {
    job: JobId,
    replies: Receiver<Result<RunReport, ExecuteFailure>>,
    labels: Vec<ItemLabel>,
    draft: Draft,
}

struct ExecuteFailure {
    message: String,
    run_id: Option<i64>,
    inspection: Option<RunInspection>,
    progress: JobProgress,
}

/// One accepted validation. `ready` is the only handle that can execute and it
/// is consumed by the run that uses it.
struct Review {
    draft: Draft,
    items: Vec<ItemValidation>,
    folders: usize,
    files_ready: usize,
    files_skip: usize,
    files_blocked: usize,
    warnings: Vec<String>,
    refusal: Option<String>,
    space_unknown: bool,
    ready: Option<ValidatedPlan>,
}

/// One destination-tree preview.
struct PreviewRun {
    replies: Receiver<Result<PreviewOutcome, String>>,
    destination: String,
}

enum PreviewOutcome {
    Tree(PreviewTree),
    /// The worker was cancelled; nothing is installed.
    Cancelled,
}

#[derive(Default)]
struct PreviewTree {
    /// Destination-relative display spellings and whether each is a directory.
    entries: BTreeMap<String, bool>,
    truncated: bool,
    /// Entries read_dir could not resolve, or whose name is not UTF-8 text and
    /// so cannot be spelled here. Counted, never invented or dropped silently.
    unlisted: usize,
}

/// One node of the merged existing/proposed destination tree.
#[derive(Default)]
struct PreviewNode {
    name: String,
    is_dir: bool,
    exists: bool,
    proposed: bool,
    children: Vec<PreviewNode>,
}

struct ReportRow {
    directory: bool,
    outcome: Option<ItemState>,
    source: PathBuf,
    destination: String,
    state: &'static str,
    failed: bool,
    message: Option<String>,
    recovery: Vec<String>,
}

struct RunDone {
    draft: Draft,
    run_id: Option<i64>,
    error: Option<String>,
    progress: JobProgress,
    state: RunState,
    counts: RunCounts,
    /// Reviewed files, excluding the directory actions the plan also carries.
    files: usize,
    rows: Vec<ReportRow>,
    warnings: Vec<String>,
    recovery: Vec<String>,
}

/// Everything a validation is bound to.
#[derive(Clone, PartialEq, Eq)]
struct Draft {
    source: String,
    destination: String,
    policy: ConflictPolicy,
    action: FileAction,
    profile: String,
    /// Bumped by any pattern or profile change.
    patterns_revision: u64,
    scan_generation: u64,
    selection_generation: u64,
    edit_generation: u64,
}

/// The contextual editor buffers for one focused item.
#[derive(Default)]
struct ItemEditor {
    item: Option<usize>,
    tags: [String; 6],
    folder: String,
    filename: String,
}

/// One cached working-list row.
struct Row {
    item: usize,
    source_dir: PathBuf,
    label: String,
    kind: String,
    /// True for a grouped frame sequence, false for one single file.
    sequence: bool,
    frames: usize,
    bytes: u64,
    folder: String,
    state: MappingState,
    reason: String,
    /// Frames absent from a sequence; zero when there is no gap.
    gaps: usize,
    /// The exact missing frame numbers, shown on demand beside the count.
    missing: Option<String>,
}

/// Deferred structural edit of the pattern or profile lists.
enum ListAction {
    RemovePatternRule(TagKind, usize),
    MovePatternRule(TagKind, usize, isize),
    /// Profile index and rule index.
    RemoveRule(usize, usize),
    /// Profile index, rule index and direction.
    MoveRule(usize, usize, isize),
    /// Profile index, rule index and keyword index.
    DropKeyword(usize, usize, usize),
    MoveProfile(usize, isize),
    DeleteProfile(String),
}

/// The Organize Incoming workspace: draft inputs, tracked workers and the
/// review and run evidence the page renders.
struct AiPrepared {
    draft: Draft,
    request: crate::ai::AiRequest,
    config: crate::ai::HarnessConfig,
    context: String,
}
struct AiRun {
    job: JobId,
    draft: Draft,
    request: crate::ai::AiRequest,
    replies: Receiver<Result<Vec<crate::ai::AiSuggestion>, String>>,
}
struct AiChoice {
    selected: bool,
    suggestion: crate::ai::AiSuggestion,
}
struct AiResponse {
    draft: Draft,
    choices: Vec<AiChoice>,
}

pub struct OrganizeIncomingState {
    ai_open: bool,
    ai_config: crate::ai::HarnessConfig,
    ai_instruction: String,
    ai_prepared: Option<AiPrepared>,
    ai_run: Option<AiRun>,
    ai_response: Option<AiResponse>,
    ai_previous: Option<(Draft, Vec<crate::ai::AiSuggestion>)>,
    ai_error: Option<String>,
    ai_counter: u64,
    ctx: egui::Context,
    store: PatternStore,
    catalog: Option<CatalogClient>,
    user: Option<UserId>,

    source: String,
    destination: String,
    policy: ConflictPolicy,
    move_files: bool,
    profile_name: String,
    last_pick: Option<PathBuf>,
    rules_open: bool,

    patterns: Option<PatternSet>,
    profiles: Option<ProfileSet>,
    compiled: Option<CompiledPatterns>,
    config_run: Option<ConfigRun>,
    config_message: Option<String>,
    config_error: Option<String>,
    load_attempted: bool,
    new_pattern: [String; 6],
    new_pattern_mode: RuleMode,
    new_pattern_task: String,
    new_rule_path: String,
    new_rule_keywords: String,
    profile_entry: String,

    pick: Option<PickRun>,
    scan_run: Option<ScanRun>,
    scanned: Option<ScanResult>,
    scan_error: Option<String>,
    scan_generation: u64,

    proposals: Vec<MappingProposal>,
    rows: Vec<Row>,
    selected: HashSet<usize>,
    selection_generation: u64,
    edits: HashMap<usize, ItemEdit>,
    edit_generation: u64,
    patterns_revision: u64,
    filter: String,
    folder_scope: Option<PathBuf>,
    /// Item whose exact member files are open in the frames dialog.
    frames_open: Option<usize>,
    focus: Option<usize>,
    editor: ItemEditor,

    plan_counter: u64,
    validate_run: Option<ValidateRun>,
    review: Option<Review>,
    execute_run: Option<ExecuteRun>,
    report: Option<RunDone>,

    preview_run: Option<PreviewRun>,
    preview: Option<PreviewTree>,
    preview_for: Option<String>,
    preview_requested: bool,
    tree: Option<PreviewNode>,
    tree_key: String,

    message: Option<String>,
}

impl OrganizeIncomingState {
    pub fn new(ctx: &egui::Context, data_dir: PathBuf) -> Self {
        let profiles = ProfileSet::default();
        let profile_name = profiles
            .profiles
            .first()
            .map(|profile| profile.name.clone())
            .unwrap_or_default();
        Self {
            ai_open: false,
            ai_config: Default::default(),
            ai_instruction: "Suggest folders for these files using the selected folder layout."
                .into(),
            ai_prepared: None,
            ai_run: None,
            ai_response: None,
            ai_previous: None,
            ai_error: None,
            ai_counter: 0,
            ctx: ctx.clone(),
            store: PatternStore::new(&data_dir),
            catalog: None,
            user: None,
            source: String::new(),
            destination: String::new(),
            policy: ConflictPolicy::Skip,
            move_files: false,
            profile_name,
            last_pick: None,
            rules_open: false,
            patterns: None,
            profiles: Some(profiles),
            compiled: None,
            config_run: None,
            config_message: None,
            config_error: None,
            load_attempted: false,
            new_pattern: Default::default(),
            new_pattern_mode: RuleMode::Regex,
            new_pattern_task: String::new(),
            new_rule_path: String::new(),
            new_rule_keywords: String::new(),
            profile_entry: String::new(),
            pick: None,
            scan_run: None,
            scanned: None,
            scan_error: None,
            scan_generation: 0,
            proposals: Vec::new(),
            rows: Vec::new(),
            selected: HashSet::new(),
            selection_generation: 0,
            edits: HashMap::new(),
            edit_generation: 0,
            patterns_revision: 0,
            filter: String::new(),
            folder_scope: None,
            frames_open: None,
            focus: None,
            editor: ItemEditor::default(),
            plan_counter: 0,
            validate_run: None,
            review: None,
            execute_run: None,
            report: None,
            preview_run: None,
            preview: None,
            preview_for: None,
            preview_requested: false,
            tree: None,
            tree_key: String::new(),
            message: None,
        }
    }

    pub fn move_files(&self) -> bool {
        self.move_files
    }

    pub fn set_move_files(&mut self, move_files: bool) {
        if self.move_files == move_files {
            return;
        }
        self.move_files = move_files;
        self.invalidate_review();
    }

    fn action(&self) -> FileAction {
        if self.move_files {
            FileAction::Move
        } else {
            FileAction::Copy
        }
    }

    // ------------------------------------------------------------ coordination

    /// Drains every worker reply and keeps the review bound to the current
    /// draft. Called once per frame from the application logic.
    pub fn poll(&mut self, catalog: Option<&CatalogClient>, user: Option<UserId>, jobs: &Jobs) {
        let _ = jobs;
        self.catalog = catalog.cloned();
        self.user = user;
        self.poll_config();
        self.poll_pick();
        self.poll_scan();
        self.poll_validation();
        self.poll_execute(jobs);
        self.poll_preview();
        self.poll_ai();
        self.sync_draft();
        self.invalidate_review();
    }

    pub fn is_picker_job(&self, id: JobId) -> bool {
        self.pick.as_ref().is_some_and(|run| run.job == id)
            || self.config_run.as_ref().is_some_and(|run| run.job == id)
    }

    pub fn picker_pending(&self) -> bool {
        self.pick.is_some() || self.config_run.is_some()
    }

    pub fn is_ai_job(&self, id: JobId) -> bool {
        self.ai_run.as_ref().is_some_and(|run| run.job == id)
    }

    fn busy(&self) -> bool {
        self.ai_run.is_some()
            || self.pick.is_some()
            || self.config_run.is_some()
            || self.scan_run.is_some()
            || self.validate_run.is_some()
            || self.execute_run.is_some()
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

    fn draft(&self) -> Draft {
        Draft {
            source: self.source.trim().to_string(),
            destination: self.destination.trim().to_string(),
            policy: self.policy,
            action: self.action(),
            profile: self.profile_name.clone(),
            patterns_revision: self.patterns_revision,
            scan_generation: self.scan_generation,
            selection_generation: self.selection_generation,
            edit_generation: self.edit_generation,
        }
    }

    /// Drops the scan when the source folder it was taken from changed.
    fn sync_draft(&mut self) {
        let Some(scanned) = &self.scanned else {
            return;
        };
        if scanned.snapshot.root == Path::new(self.source.trim()) {
            return;
        }
        self.forget_scan();
        self.message = Some("The source folder changed; scan it again.".to_string());
    }

    fn forget_scan(&mut self) {
        self.scanned = None;
        self.proposals.clear();
        self.rows.clear();
        self.selected.clear();
        self.edits.clear();
        self.frames_open = None;
        self.focus = None;
        self.editor = ItemEditor::default();
        self.review = None;
        self.report = None;
        self.preview = None;
        self.preview_for = None;
        self.tree = None;
        self.selection_generation += 1;
    }

    fn invalidate_review(&mut self) {
        if self.execute_run.is_some() {
            return;
        }
        if self
            .review
            .as_ref()
            .is_some_and(|review| review.draft != self.draft())
        {
            self.review = None;
            self.message = Some("The selection or settings changed; validate again.".to_string());
        }
    }

    fn active_profile(&self) -> Option<&IncomingProfile> {
        self.profiles.as_ref().and_then(|set| {
            set.find(&self.profile_name)
                .or_else(|| set.profiles.first())
        })
    }

    fn selected_proposals(&self) -> Vec<MappingProposal> {
        self.proposals
            .iter()
            .filter(|proposal| self.selected.contains(&proposal.item))
            .cloned()
            .collect()
    }

    fn selected_counts(&self) -> (usize, usize, u64) {
        let mut items = 0;
        let mut files = 0;
        let mut bytes = 0;
        for proposal in &self.proposals {
            if !self.selected.contains(&proposal.item) {
                continue;
            }
            items += 1;
            files += proposal.frames();
            bytes += proposal.bytes();
        }
        (items, files, bytes)
    }

    // ------------------------------------------------------------------ workers

    fn begin_load_patterns(&mut self, jobs: &mut Jobs) {
        if self.config_run.is_some() || jobs.is_shutting_down() {
            return;
        }
        self.load_attempted = true;
        let store = self.store.clone();
        let (tx, rx) = channel();
        let started = jobs.start(&self.ctx, "Opening incoming rules", move |context| {
            let result = match store.load() {
                Ok(stored) => Ok(ConfigOutcome::Loaded {
                    patterns: stored.patterns,
                    profiles: stored.profiles,
                    seeded: stored.seeded,
                }),
                Err(error) => Err(error.to_string()),
            };
            let (outcome, progress) = match &result {
                Ok(_) => (JobOutcome::Succeeded, JobProgress::default()),
                Err(message) => {
                    context
                        .events
                        .diagnostic(JobPhase::Validating, message.clone());
                    (JobOutcome::Failed, JobProgress::default())
                }
            };
            let _ = tx.send(result);
            context.events.finish(outcome, progress);
        });
        match started {
            Ok(job) => {
                self.config_run = Some(ConfigRun {
                    job,
                    title: "Opening incoming rules",
                    replies: rx,
                })
            }
            Err(error) => {
                self.config_error = Some(format!("The rule files could not be opened: {error}"))
            }
        }
    }

    fn begin_save(&mut self, jobs: &mut Jobs, file: ConfigFile) {
        if self.config_run.is_some() || jobs.is_shutting_down() {
            return;
        }
        let store = self.store.clone();
        let patterns = self.patterns.clone();
        let profiles = self.profiles.clone();
        if patterns.is_none() || profiles.is_none() {
            self.config_error =
                Some("The rules are still opening, so there is nothing to save yet.".to_string());
            return;
        }
        let (tx, rx) = channel();
        let title = format!("Saving the {}", file.title());
        let started = jobs.start(&self.ctx, title.clone(), move |context| {
            let result = (|| match file {
                ConfigFile::Patterns => store
                    .save_patterns(patterns.as_ref().expect("checked above"))
                    .map_err(|error| error.to_string()),
                ConfigFile::Profiles => store
                    .save_profiles(profiles.as_ref().expect("checked above"))
                    .map_err(|error| error.to_string()),
            })()
            .map(|_| ConfigOutcome::Saved(file));
            let (outcome, progress) = match &result {
                Ok(_) => (JobOutcome::Succeeded, JobProgress::default()),
                Err(message) => {
                    context
                        .events
                        .diagnostic(JobPhase::Validating, message.clone());
                    (JobOutcome::Failed, JobProgress::default())
                }
            };
            let _ = tx.send(result);
            context.events.finish(outcome, progress);
        });
        match started {
            Ok(job) => {
                self.config_run = Some(ConfigRun {
                    job,
                    title: title_name(file),
                    replies: rx,
                })
            }
            Err(error) => self.config_error = Some(format!("The save could not start: {error}")),
        }
    }

    fn poll_config(&mut self) {
        let Some(run) = self.config_run.take() else {
            return;
        };
        let result = match run.replies.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.config_run = Some(run);
                return;
            }
            Err(TryRecvError::Disconnected) => {
                Err(format!("{} ended without a result.", run.title))
            }
        };
        match result {
            Ok(ConfigOutcome::Loaded {
                patterns,
                profiles,
                seeded,
            }) => {
                self.compiled = Some(patterns.compile());
                self.patterns = Some(patterns);
                if profiles.find(&self.profile_name).is_none() {
                    self.profile_name = profiles
                        .profiles
                        .first()
                        .map(|profile| profile.name.clone())
                        .unwrap_or_default();
                }
                self.profiles = Some(profiles);
                self.patterns_revision += 1;
                self.config_error = None;
                self.config_message = Some(if seeded {
                    format!(
                        "Created default rules in {} — edit or import your own.",
                        self.store.directory().display()
                    )
                } else {
                    format!("Rules loaded from {}.", self.store.directory().display())
                });
                self.rematch();
            }
            Ok(ConfigOutcome::Saved(file)) => {
                self.config_message = Some(format!("Saved the {}.", file.title()));
                self.config_error = None;
            }
            Err(message) => self.config_error = Some(message),
        }
    }

    fn poll_pick(&mut self) {
        let Some(run) = self.pick.take() else {
            return;
        };
        let result = match run.replies.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.pick = Some(run);
                return;
            }
            Err(TryRecvError::Disconnected) => {
                Err("The chooser ended without a result.".to_string())
            }
        };
        match result {
            Ok(Some(PickOutcome::Folder(folder))) => {
                if let PickTask::Folder(field) = run.task {
                    self.apply_folder(field, folder);
                }
            }
            Ok(Some(PickOutcome::Imported(file, text))) => self.apply_import(file, &text),
            Ok(Some(PickOutcome::Exported(file, path))) => {
                self.config_message =
                    Some(format!("{} exported to {}.", file.title(), path.display()));
                self.config_error = None;
            }
            Ok(None) => {}
            Err(message) => self.message = Some(message),
        }
    }

    fn apply_folder(&mut self, field: Field, folder: PathBuf) {
        self.last_pick = Some(folder.clone());
        let Some(text) = folder.to_str().map(str::to_string) else {
            self.message = Some(
                "This folder name cannot be shown in the path field. Choose a folder with a Unicode name.".to_string(),
            );
            return;
        };
        match field {
            Field::Source => {
                if self.source != text {
                    self.source = text;
                    self.folder_scope = None;
                }
            }
            Field::Destination => {
                if self.destination != text {
                    self.destination = text;
                    self.preview = None;
                    self.preview_for = None;
                    self.tree = None;
                }
            }
        }
    }

    fn apply_import(&mut self, file: ConfigFile, text: &str) {
        match file {
            ConfigFile::Patterns => match PatternSet::from_json(text) {
                Ok(set) => {
                    let issues = set.dialect_issues();
                    self.config_error = None;
                    self.config_message = Some(format!(
                        "Imported {} patterns{}.",
                        set.total_rules(),
                        if issues.is_empty() {
                            String::new()
                        } else {
                            format!(" with {} dialect issue(s) listed below", issues.len())
                        }
                    ));
                    self.set_patterns(set);
                }
                Err(error) => self.config_error = Some(error.to_string()),
            },
            ConfigFile::Profiles => match ProfileSet::from_json(text) {
                Ok(set) => {
                    self.config_error = None;
                    self.config_message =
                        Some(format!("Imported {} profile(s).", set.profiles.len()));
                    self.set_profiles(set);
                }
                Err(error) => self.config_error = Some(error.to_string()),
            },
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
        self.report = None;
        self.focus = None;
        self.editor = ItemEditor::default();
        match result {
            Ok(analyzed) => {
                self.scan_generation += 1;
                self.selection_generation += 1;
                self.edits.clear();
                self.frames_open = None;
                self.selected = (0..analyzed.analysis.items.len()).collect();
                self.scanned = Some(ScanResult {
                    snapshot: analyzed.snapshot,
                    analysis: analyzed.analysis,
                });
                self.folder_scope = None;
                self.scan_error = None;
                self.message = None;
                let current_profile = self.active_profile().map(|profile| profile.name.clone());
                let fresh = run.patterns_revision == self.patterns_revision
                    && current_profile.as_deref() == Some(run.profile.as_str());
                if fresh {
                    self.proposals = analyzed.proposals;
                    self.rebuild_rows();
                } else {
                    // The rules or the chosen layout changed while the scan ran.
                    // Match the accepted analysis with the rules in force now
                    // instead of installing routing the user has already edited.
                    self.rematch();
                }
                self.preview_requested = true;
            }
            Err(message) => {
                self.scanned = None;
                self.proposals.clear();
                self.rows.clear();
                self.selected.clear();
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
            Ok(report) => {
                let ValidationReport {
                    ready,
                    items,
                    space_unknown,
                    warnings,
                    refusal,
                    ..
                } = report;
                let mut folders = 0;
                let mut files_ready = 0;
                let mut files_skip = 0;
                let mut files_blocked = 0;
                for item in &items {
                    if item.kind == ActionKind::Directory {
                        folders += 1;
                        continue;
                    }
                    match item.state {
                        ReviewState::Ready => files_ready += 1,
                        ReviewState::Skipped => files_skip += 1,
                        ReviewState::Blocked => files_blocked += 1,
                    }
                }
                self.review = Some(Review {
                    draft: run.draft,
                    items,
                    folders,
                    files_ready,
                    files_skip,
                    files_blocked,
                    warnings,
                    refusal,
                    space_unknown,
                    ready,
                });
                self.message = None;
                self.preview_requested = true;
            }
            Err(message) => {
                self.review = None;
                self.message = Some(message);
            }
        }
    }

    fn poll_execute(&mut self, jobs: &Jobs) {
        let Some(run) = self.execute_run.take() else {
            return;
        };
        let result = match run.replies.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.execute_run = Some(run);
                return;
            }
            Err(TryRecvError::Disconnected) => Err(ExecuteFailure {
                message: "The transfer worker stopped without a report. Some files may already be at the destination; inspect it before retrying.".to_string(),
                run_id: None,
                inspection: None,
                progress: job_status(jobs, Some(run.job))
                    .map(|job| job.progress.clone())
                    .unwrap_or_default(),
            }),
        };
        let warnings = self
            .review
            .as_ref()
            .map(|review| review.warnings.clone())
            .unwrap_or_default();
        match result {
            Ok(report) => {
                let (rows, recovery) = report_rows(&report, &run.labels);
                self.report = Some(RunDone {
                    draft: run.draft,
                    run_id: Some(report.run_id),
                    error: None,
                    progress: report_progress(&report),
                    state: report.summary.state,
                    counts: report.summary.counts,
                    files: file_labels(&run.labels),
                    rows,
                    warnings,
                    recovery,
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
        self.preview_requested = true;
    }

    fn poll_preview(&mut self) {
        let Some(run) = self.preview_run.take() else {
            return;
        };
        let result = match run.replies.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => {
                self.preview_run = Some(run);
                return;
            }
            Err(TryRecvError::Disconnected) => {
                Err("The destination preview ended without a result.".to_string())
            }
        };
        match result {
            Ok(PreviewOutcome::Cancelled) => {
                // Cancelled: install nothing and keep whatever tree is already
                // shown for this destination instead of claiming a failure.
            }
            Ok(PreviewOutcome::Tree(tree)) => {
                if run.destination == self.destination.trim() {
                    self.preview = Some(tree);
                    self.preview_for = Some(run.destination);
                    self.tree = None;
                }
            }
            Err(message) => {
                self.preview = None;
                self.preview_for = None;
                self.tree = None;
                self.message = Some(message);
            }
        }
    }

    // ------------------------------------------------------------- configuration

    fn set_patterns(&mut self, set: PatternSet) {
        if self.patterns.as_ref() == Some(&set) {
            return;
        }
        self.compiled = Some(set.compile());
        self.patterns = Some(set);
        self.patterns_revision += 1;
        self.rematch();
    }

    fn set_profiles(&mut self, set: ProfileSet) {
        if self.profiles.as_ref() == Some(&set) {
            return;
        }
        if set.find(&self.profile_name).is_none() {
            self.profile_name = set
                .profiles
                .first()
                .map(|profile| profile.name.clone())
                .unwrap_or_default();
        }
        self.profiles = Some(set);
        self.patterns_revision += 1;
        self.rematch();
    }

    /// Recomputes every proposal from the current patterns, profile and edits.
    /// Pure matching over the analyzed items: no filesystem work.
    fn rematch(&mut self) {
        let compiled = self.compiled.clone();
        let profile = self.active_profile().cloned();
        let (Some(analysis), Some(compiled), Some(profile)) = (
            self.scanned.as_ref().map(|scanned| &scanned.analysis),
            compiled,
            profile,
        ) else {
            self.proposals.clear();
            self.rows.clear();
            return;
        };
        let edits: Vec<ItemEdit> = (0..analysis.items.len())
            .map(|index| self.edits.get(&index).cloned().unwrap_or_default())
            .collect();
        self.proposals =
            incoming::propose_incoming(&analysis.items, &compiled, &profile, &edits, |_| {});
        self.review = None;
        self.rebuild_rows();
        self.sync_editor();
    }

    fn rebuild_rows(&mut self) {
        let Some(scanned) = &self.scanned else {
            self.rows.clear();
            return;
        };
        let mut rows = Vec::with_capacity(self.proposals.len());
        for proposal in &self.proposals {
            let Some(item) = scanned.analysis.items.get(proposal.item) else {
                continue;
            };
            let source_dir = match item {
                IncomingItem::Single(single) => single
                    .file
                    .path
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default(),
                IncomingItem::Sequence(sequence) => sequence.directory.clone(),
            };
            let (kind, sequence_item, gaps, missing) = match item {
                IncomingItem::Single(single) => {
                    let name = single.file.path.file_name();
                    let extension = name
                        .and_then(|name| name.to_str())
                        .and_then(|name| name.rsplit_once('.'))
                        .map(|(_, extension)| extension.to_uppercase())
                        .unwrap_or_default();
                    let label = match (extension.is_empty(), single.kind) {
                        (false, _) => extension,
                        (true, SingleKind::Project) => "Project".to_string(),
                        (true, SingleKind::Cache) => "Cache".to_string(),
                        (true, _) => "File".to_string(),
                    };
                    (label, false, 0usize, None)
                }
                IncomingItem::Sequence(sequence) => {
                    let extension = sequence.suffix.trim_start_matches('.').to_uppercase();
                    let mut missing = Vec::new();
                    let mut gap_frames = 0usize;
                    for gap in &sequence.gaps {
                        gap_frames += gap.missing_frames as usize;
                        let mut frame = gap.after_frame + 1;
                        while frame < gap.before_frame && missing.len() < MISSING_FRAMES_SHOWN {
                            missing.push(frame);
                            frame += 1;
                        }
                    }
                    let text = if gap_frames == 0 {
                        None
                    } else if missing.len() >= MISSING_FRAMES_SHOWN && gap_frames > missing.len() {
                        Some(format!(
                            "missing {}: {} …",
                            gap_frames,
                            missing
                                .iter()
                                .map(|frame| frame.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                    } else {
                        Some(format!(
                            "missing {}: {}",
                            gap_frames,
                            missing
                                .iter()
                                .map(|frame| frame.to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                    };
                    (format!("{extension} sequence"), true, gap_frames, text)
                }
            };
            rows.push(Row {
                item: proposal.item,
                source_dir,
                label: proposal.label.clone(),
                kind,
                sequence: sequence_item,
                frames: proposal.frames(),
                bytes: proposal.bytes(),
                folder: proposal.folder_display(),
                state: proposal.state,
                reason: proposal.reason.clone(),
                gaps,
                missing,
            });
        }
        self.rows = rows;
    }

    /// Refreshes the contextual editor from the focused item.
    fn sync_editor(&mut self) {
        let Some(item) = self.focus else {
            self.editor.item = None;
            return;
        };
        let Some(proposal) = self.proposals.iter().find(|p| p.item == item) else {
            self.focus = None;
            self.editor.item = None;
            return;
        };
        self.editor.item = Some(item);
        for kind in TagKind::ALL {
            let slot = &mut self.editor.tags[tag_index(kind)];
            slot.clear();
            if let Some(value) = proposal.tags.get(kind) {
                slot.push_str(value);
            }
        }
        self.editor.folder = proposal.folder_display();
        self.editor.filename = proposal.rename.clone().unwrap_or_default();
    }

    // ------------------------------------------------------------------ commands

    fn begin_pick(&mut self, jobs: &mut Jobs, task: PickTask) {
        if self.pick.is_some() || jobs.is_shutting_down() {
            return;
        }
        let start = self.last_pick.clone();
        let store = self.store.clone();
        let title = match &task {
            PickTask::Folder(field) => field.title().to_string(),
            PickTask::Import(file) => format!("Import {}", file.title()),
            PickTask::Export(file, _) => format!("Export {}", file.title()),
        };
        let worker_task = task.clone();
        let dialog_title = title.clone();
        let (tx, rx) = channel();
        let started = jobs.start(&self.ctx, title, move |context| {
            let mut dialog = rfd::FileDialog::new().set_title(dialog_title);
            if let Some(start) = start {
                dialog = dialog.set_directory(start);
            }
            let outcome: Result<Option<PickOutcome>, String> = match worker_task {
                PickTask::Folder(field) => {
                    let _ = field;
                    Ok(dialog.pick_folder().map(PickOutcome::Folder))
                }
                PickTask::Import(file) => match dialog.pick_file() {
                    Some(path) => std::fs::read_to_string(&path)
                        .map(|text| Some(PickOutcome::Imported(file, text)))
                        .map_err(|error| format!("{} could not be read: {error}", path.display())),
                    None => Ok(None),
                },
                // The write happens on the worker, never in the frame.
                PickTask::Export(file, text) => {
                    match dialog.set_file_name(file.default_name()).save_file() {
                        Some(path) => store
                            .write_export(&path, &text)
                            .map(|_| Some(PickOutcome::Exported(file, path)))
                            .map_err(|error| error.to_string()),
                        None => Ok(None),
                    }
                }
            };
            let (outcome_label, progress) = match &outcome {
                Ok(_) => (JobOutcome::Succeeded, JobProgress::default()),
                Err(message) => {
                    context
                        .events
                        .diagnostic(JobPhase::Validating, message.clone());
                    (JobOutcome::Failed, JobProgress::default())
                }
            };
            let _ = tx.send(outcome);
            context.events.finish(outcome_label, progress);
        });
        match started {
            Ok(job) => {
                self.pick = Some(PickRun {
                    job,
                    task,
                    replies: rx,
                })
            }
            Err(error) => self.message = Some(format!("The chooser could not be opened: {error}")),
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
        let Some(compiled) = self.compiled.clone() else {
            self.message = Some("The incoming rules are still opening.".to_string());
            return;
        };
        let Some(profile) = self.active_profile().cloned() else {
            self.message = Some("Choose a folder layout first.".to_string());
            return;
        };
        let root = PathBuf::from(self.source.trim());
        // The reference scanner listed every non-skipped entry and classified it
        // afterwards, so extensionless files and unknown types stay in the scan.
        // `^\\.` is the scanner's own hidden-name skip (`skip_patterns`); the rest
        // of its component skips come from the analysis module. Both apply to
        // Incoming only, never to the project or copy scans.
        let mut excludes = incoming::scan_excludes();
        excludes.push(".*".to_string());
        let request = ScanRequest {
            root: root.clone(),
            purpose: ScanPurpose::Incoming,
            recursive: true,
            filters: ScanFilters {
                suffixes: Vec::new(),
                mode: SuffixMode::MatchAll,
                excluded_name_suffixes: Vec::new(),
                excludes,
            },
        };
        let start_title = format!(
            "Scanning {}",
            root.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.display().to_string())
        );
        let (tx, rx) = channel();
        let patterns_revision = self.patterns_revision;
        let profile_name = profile.name.clone();
        let started = jobs.start(&self.ctx, start_title, move |context| {
            let JobContext {
                control, events, ..
            } = context;
            let result = discovery::walk(&request, &control, &events).map(|snapshot| {
                let analysis = incoming::analyze_snapshot(&snapshot);
                let total = analysis.items.len().max(1) as u64;
                let edits = vec![ItemEdit::default(); analysis.items.len()];
                let proposals = incoming::propose_incoming(
                    &analysis.items,
                    &compiled,
                    &profile,
                    &edits,
                    |matched| {
                        events.progress(
                            JobPhase::Validating,
                            JobProgress {
                                completed: matched as u64,
                                total: Some(total),
                                bytes_done: 0,
                                bytes_total: None,
                                current: None,
                            },
                        );
                    },
                );
                Analyzed {
                    snapshot,
                    analysis,
                    proposals,
                }
            });
            let (payload, outcome, progress) = match result {
                Ok(analyzed) => {
                    let outcome = match analyzed.snapshot.coverage {
                        Coverage::Complete => JobOutcome::Succeeded,
                        Coverage::Partial => JobOutcome::Partial,
                        Coverage::Cancelled => JobOutcome::Cancelled,
                        Coverage::RootUnavailable => JobOutcome::Failed,
                    };
                    let files = analyzed.snapshot.entries.len() as u64;
                    (
                        Ok(analyzed),
                        outcome,
                        JobProgress {
                            completed: files,
                            total: Some(files),
                            bytes_done: 0,
                            bytes_total: None,
                            current: None,
                        },
                    )
                }
                Err(error) => {
                    let message = error.message().to_string();
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
                    job,
                    replies: rx,
                    patterns_revision,
                    profile: profile_name,
                });
                self.scanned = None;
                self.proposals.clear();
                self.rows.clear();
                self.review = None;
                self.report = None;
                self.scan_error = None;
                self.message = None;
            }
            Err(error) => self.message = Some(format!("The scan could not be started: {error}")),
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
        let selected: HashSet<PathBuf> = self
            .selected_proposals()
            .iter()
            .flat_map(|proposal| proposal.sources.iter().map(|entry| entry.path.clone()))
            .collect();
        let entries: Vec<FileEntry> = scanned
            .snapshot
            .entries
            .iter()
            .filter(|entry| selected.contains(&entry.path))
            .cloned()
            .collect();
        let snapshot = ScanSnapshot {
            entries,
            ..scanned.snapshot.clone()
        };
        let proposals = self.selected_proposals();
        let destination = PathBuf::from(self.destination.trim());
        let policy = self.policy;
        let action = self.action();
        let draft = self.draft();
        self.plan_counter += 1;
        let revision = self.plan_counter;
        let id = format!("incoming-{revision}");
        let (tx, rx) = channel();
        let started = jobs.start(&self.ctx, "Validating selection", move |context| {
            let JobContext {
                control, events, ..
            } = context;
            let mut files: u64 = 0;
            let mut cancelled = false;
            // Checkpoint while walking the reviewed selection: a cancelled job
            // never reaches the planner at entry, and stops early if the user
            // cancels mid-walk.
            for (index, proposal) in proposals.iter().enumerate() {
                if index % CHECKPOINT_STRIDE == 0 && control.checkpoint() == Checkpoint::Cancelled {
                    cancelled = true;
                    break;
                }
                files += proposal.frames() as u64;
            }
            events.progress(
                JobPhase::Validating,
                JobProgress {
                    completed: 0,
                    total: Some(files.max(1)),
                    bytes_done: 0,
                    bytes_total: None,
                    current: None,
                },
            );
            let result = if cancelled {
                Err(CANCELLED_VALIDATION.to_string())
            } else {
                incoming::plan_incoming(IncomingPlanRequest {
                    id: &id,
                    revision,
                    snapshot: &snapshot,
                    destination_root: &destination,
                    policy,
                    file_action: action,
                    proposals: &proposals,
                })
                .map_err(|error| describe_transfer(&error))
                .map(|plan| transfers::validate_plan(&plan))
            };
            // Checkpoint after validation as well: a plan validated while the
            // user was cancelling is dropped instead of being offered for
            // execution.
            let result = match result {
                Ok(report) if control.checkpoint() == Checkpoint::Cancelled => {
                    drop(report);
                    Err(CANCELLED_VALIDATION.to_string())
                }
                other => other,
            };
            let (outcome, progress) = match &result {
                Ok(report) => {
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
                    let outcome = if *message == CANCELLED_VALIDATION {
                        JobOutcome::Cancelled
                    } else {
                        JobOutcome::Failed
                    };
                    (outcome, JobProgress::default())
                }
            };
            let _ = tx.send(result);
            events.finish(outcome, progress);
        });
        match started {
            Ok(job) => {
                self.validate_run = Some(ValidateRun {
                    job,
                    draft,
                    replies: rx,
                });
                self.message = None;
            }
            Err(error) => self.message = Some(format!("Validation could not be started: {error}")),
        }
    }

    fn begin_execution(&mut self, jobs: &mut Jobs) {
        if jobs.is_shutting_down() {
            return;
        }
        if let Some(reason) = self.execute_blocked() {
            self.message = Some(reason);
            return;
        }
        let Some(catalog) = self.catalog.clone() else {
            self.message = Some(NO_CATALOG.to_string());
            return;
        };
        // The run records who accepted it, so the identity is read here, on the
        // UI thread, and travels with the plan instead of being re-derived (or
        // lost) inside the worker.
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
        let labels: Vec<ItemLabel> = review
            .items
            .iter()
            .map(|item| ItemLabel {
                source: item
                    .source
                    .as_ref()
                    .map(|path| path.path().to_path_buf())
                    .unwrap_or_default(),
                destination: item
                    .destination
                    .as_ref()
                    .map(|path| path.display().unwrap_or("—").to_string())
                    .unwrap_or_else(|| "—".to_string()),
                directory: item.kind == ActionKind::Directory,
            })
            .collect();
        let draft = review.draft.clone();
        let action = if self.move_files { "Moving" } else { "Copying" };
        self.review = Some(review);
        let (tx, rx) = channel();
        let title = format!("{action} incoming files");
        let started = jobs.start(&self.ctx, title, move |context| {
            let JobContext {
                control, events, ..
            } = context;
            let result =
                transfers::execute(&catalog, validated.attribute_to(user), &control, &events)
                    .map_err(|error| {
                        let mut message = describe_transfer(&error);
                        let inspection =
                            error.run_id().and_then(|run_id| {
                                match transfers::read_run(&catalog, run_id)
                                    .and_then(|reply| reply.wait())
                                {
                                    Ok(inspection) => inspection,
                                    Err(read_error) => {
                                        message.push_str(&format!(
                                            " Journal inspection failed: {read_error}"
                                        ));
                                        None
                                    }
                                }
                            });
                        ExecuteFailure {
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
                self.execute_run = Some(ExecuteRun {
                    job,
                    replies: rx,
                    labels,
                    draft,
                });
                self.report = None;
                self.message = None;
            }
            Err(error) => {
                self.review = None;
                self.message = Some(format!(
                    "The transfer could not be started: {error}. Validate the selection again before retrying."
                ));
            }
        }
    }

    fn begin_preview(&mut self, jobs: &mut Jobs) {
        let destination = self.destination.trim().to_string();
        if destination.is_empty() || !Path::new(&destination).is_absolute() {
            return;
        }
        if self.preview_run.is_some() || jobs.is_shutting_down() {
            return;
        }
        if self.preview_for.as_deref() == Some(destination.as_str()) && self.preview.is_some() {
            return;
        }
        let root = PathBuf::from(&destination);
        let (tx, rx) = channel();
        let started = jobs.start(&self.ctx, "Reading the destination", move |context| {
            let JobContext {
                control, events, ..
            } = context;
            let result = match read_preview(&root, &control) {
                Ok(tree) => Ok(PreviewOutcome::Tree(tree)),
                Err(PreviewRead::Cancelled) => Ok(PreviewOutcome::Cancelled),
                Err(PreviewRead::Failed(message)) => Err(message),
            };
            let (outcome, progress) = match &result {
                Ok(PreviewOutcome::Tree(_)) => (JobOutcome::Succeeded, JobProgress::default()),
                Ok(PreviewOutcome::Cancelled) => (JobOutcome::Cancelled, JobProgress::default()),
                Err(message) => {
                    events.diagnostic(JobPhase::Scanning, message.clone());
                    (JobOutcome::Failed, JobProgress::default())
                }
            };
            let _ = tx.send(result);
            events.finish(outcome, progress);
        });
        match started {
            Ok(_job) => {
                self.preview_run = Some(PreviewRun {
                    replies: rx,
                    destination: destination.clone(),
                })
            }
            Err(error) => self.message = Some(format!("The preview could not start: {error}")),
        }
    }

    // ------------------------------------------------------------------ blockers

    fn scan_blocked(&self) -> Option<String> {
        if let Some(reason) = self.live_reason() {
            return Some(reason);
        }
        if let Some(reason) = self.busy_reason() {
            return Some(reason);
        }
        if self.patterns.is_none() {
            return Some(match &self.config_error {
                Some(error) => format!("The incoming rules could not be opened: {error}"),
                None => "The incoming rules are still opening.".to_string(),
            });
        }
        if self.source.trim().is_empty() {
            return Some("Choose a source folder first.".to_string());
        }
        if !Path::new(self.source.trim()).is_absolute() {
            return Some("Use an absolute source folder path.".to_string());
        }
        if self.active_profile().is_none() {
            return Some("Choose a folder layout first.".to_string());
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
            return Some("Select at least one item.".to_string());
        }
        if self.destination.trim().is_empty() {
            return Some("Choose a destination folder first.".to_string());
        }
        if !Path::new(self.destination.trim()).is_absolute() {
            return Some("Use an absolute destination folder path.".to_string());
        }
        let mut blocked = 0usize;
        let mut first: Option<&MappingProposal> = None;
        for proposal in &self.proposals {
            if !self.selected.contains(&proposal.item) || proposal.is_executable() {
                continue;
            }
            blocked += 1;
            if first.is_none() {
                first = Some(proposal);
            }
        }
        if let Some(first) = first {
            return Some(format!(
                "{} selected item(s) have no executable destination; first: {} — {}.",
                blocked, first.label, first.reason
            ));
        }
        None
    }

    fn execute_blocked(&self) -> Option<String> {
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
                "The review has blocked items; fix them and validate again.".to_string()
            }));
        }
        None
    }

    // ------------------------------------------------------------- list editing

    fn set_selected(&mut self, item: usize, selected: bool) {
        let changed = if selected {
            self.selected.insert(item)
        } else {
            self.selected.remove(&item)
        };
        if changed {
            self.selection_generation += 1;
        }
    }

    fn select_all(&mut self) {
        if self.selected.len() == self.proposals.len() {
            return;
        }
        self.selected = self
            .proposals
            .iter()
            .map(|proposal| proposal.item)
            .collect();
        self.selection_generation += 1;
    }

    fn clear_selection(&mut self) {
        if self.selected.is_empty() {
            return;
        }
        self.selected.clear();
        self.selection_generation += 1;
    }

    /// Applies the editor buffers to the focused item only.
    fn apply_editor_to_item(&mut self) {
        let Some(item) = self.focus else {
            return;
        };
        let Some(proposal) = self.proposals.iter().find(|p| p.item == item).cloned() else {
            return;
        };
        let edit = self.edit_from_editor(&proposal);
        if edit.is_empty() {
            // Applying an empty editor clears the edit this item still carries;
            // leaving it in place would keep a destination the UI no longer shows.
            self.edits.remove(&item);
        } else {
            self.edits.insert(item, edit);
        }
        self.edit_generation += 1;
        self.rematch();
    }

    /// Applies the editor tags (and the folder when `with_folder`) to every
    /// selected item, each computed against its own extracted tags.
    fn apply_editor_to_selection(&mut self, with_folder: bool) {
        let selected: Vec<MappingProposal> = self
            .proposals
            .iter()
            .filter(|proposal| self.selected.contains(&proposal.item))
            .cloned()
            .collect();
        if selected.is_empty() {
            return;
        }
        let mut changed = false;
        for proposal in selected {
            let mut edit = self.edit_from_editor(&proposal);
            let existing = self.edits.get(&proposal.item);
            // The name field belongs to the focused item only. A selection-wide
            // apply therefore never renames anything: every item keeps its own
            // filename (or the edit it already had).
            edit.filename = existing
                .map(|existing| existing.filename.clone())
                .unwrap_or_default();
            if with_folder {
                edit.tags = existing
                    .map(|existing| existing.tags.clone())
                    .unwrap_or_default();
            }
            if !with_folder {
                edit.folder = existing
                    .map(|existing| existing.folder.clone())
                    .unwrap_or_default();
            }
            if edit.is_empty() {
                self.edits.remove(&proposal.item);
            } else {
                self.edits.insert(proposal.item, edit);
            }
            changed = true;
        }
        if changed {
            self.edit_generation += 1;
            self.rematch();
        }
    }

    fn edit_from_editor(&self, proposal: &MappingProposal) -> ItemEdit {
        let mut tags = TagEdits::default();
        for kind in TagKind::ALL {
            let text = self.editor.tags[tag_index(kind)].trim().to_string();
            let extracted = proposal.extracted.get(kind).unwrap_or_default();
            if text != extracted {
                tags.set(kind, Some(text));
            }
        }
        let folder = self.editor.folder.trim().to_string();
        let filename = self.editor.filename.trim().to_string();
        let rename = proposal.rename.clone().unwrap_or_default();
        ItemEdit {
            tags,
            folder: if folder == proposal.folder_display() {
                String::new()
            } else {
                folder
            },
            filename: if filename == rename {
                String::new()
            } else {
                filename
            },
        }
    }

    fn reset_item(&mut self) {
        if let Some(item) = self.focus {
            if self.edits.remove(&item).is_some() {
                self.edit_generation += 1;
            }
        }
        self.rematch();
    }

    fn reset_selection(&mut self) {
        let selected: Vec<usize> = self.selected.iter().copied().collect();
        let mut changed = false;
        for item in selected {
            changed |= self.edits.remove(&item).is_some();
        }
        if changed {
            self.edit_generation += 1;
        }
        self.rematch();
    }

    /// Applies a deferred structural edit to the pattern and profile lists.
    fn apply_list_actions(&mut self, actions: Vec<ListAction>) {
        if actions.is_empty() {
            return;
        }
        let (mut patterns, mut profiles) = match (self.patterns.clone(), self.profiles.clone()) {
            (Some(patterns), Some(profiles)) => (patterns, profiles),
            (patterns, profiles) => {
                if let Some(patterns) = patterns {
                    self.set_patterns(patterns);
                }
                if let Some(profiles) = profiles {
                    self.set_profiles(profiles);
                }
                return;
            }
        };
        let mut profile_changed = false;
        for action in actions {
            match action {
                ListAction::RemovePatternRule(kind, index) => {
                    patterns.remove(kind, index);
                }
                ListAction::MovePatternRule(kind, index, delta) => {
                    patterns.move_rule(kind, index, delta);
                }
                ListAction::RemoveRule(profile, index) => {
                    if let Some(profile) = profiles.profiles.get_mut(profile) {
                        if index < profile.rules.len() {
                            profile.rules.remove(index);
                            profile_changed = true;
                        }
                    }
                }
                ListAction::MoveProfile(index, delta) => {
                    let length = profiles.profiles.len();
                    if let Some(target) = offset(index, delta, length) {
                        profiles.profiles.swap(index, target);
                        profile_changed = true;
                    }
                }
                ListAction::MoveRule(profile, index, delta) => {
                    if let Some(profile) = profiles.profiles.get_mut(profile) {
                        let length = profile.rules.len();
                        if let Some(target) = offset(index, delta, length) {
                            profile.rules.swap(index, target);
                            profile_changed = true;
                        }
                    }
                }
                ListAction::DropKeyword(profile, rule, keyword) => {
                    if let Some(profile) = profiles.profiles.get_mut(profile) {
                        if let Some(rule) = profile.rules.get_mut(rule) {
                            if keyword < rule.keywords.len() {
                                rule.keywords.remove(keyword);
                                profile_changed = true;
                            }
                        }
                    }
                }
                ListAction::DeleteProfile(name) => {
                    if profiles.profiles.len() > 1 {
                        profiles.profiles.retain(|profile| profile.name != name);
                        profile_changed = true;
                    }
                }
            }
        }
        if profile_changed {
            if profiles.find(&self.profile_name).is_none() {
                self.profile_name = profiles
                    .profiles
                    .first()
                    .map(|profile| profile.name.clone())
                    .unwrap_or_default();
            }
            self.set_profiles(profiles);
        }
        self.set_patterns(patterns);
    }

    fn export_text(&self, file: ConfigFile) -> Option<String> {
        match file {
            ConfigFile::Patterns => self.patterns.as_ref().map(PatternSet::to_json),
            ConfigFile::Profiles => self.profiles.as_ref().map(ProfileSet::to_json),
        }
    }

    // -------------------------------------------------------------------- UI

    /// The working page. Everything stays on this page: the source tree, the
    /// files with their destinations, the destination preview and the progress
    /// and report dock.
    pub fn view(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        if self.patterns.is_none() && !self.load_attempted {
            self.begin_load_patterns(jobs);
        }
        let profile_before = self.profile_name.clone();
        let mut scan = false;
        let mut pick: Option<PickTask> = None;
        let mut refresh_preview = false;
        let mut rules_open = false;

        egui::Panel::top("incoming_toolbar").show(ui, |ui| {
            // Both folder fields share the width the Scan control leaves, so a
            // wide window does not strand two 240px inputs at the left. The
            // measurements decide whether one row fits; a narrow window stacks
            // the fields instead of squeezing a path below its useful minimum.
            let gap = ui.spacing().item_spacing.x;
            let scan_width = button_width(ui, "Scan folders");
            let browse_width = button_width(ui, "Browse…");
            let label_width = small_width(ui, "Destination");
            let fixed = scan_width + 2.0 * browse_width + 2.0 * label_width + 8.0 * gap + 24.0;
            let wide = ui.available_width() - fixed >= 2.0 * PATH_MIN_WIDTH;
            ui.horizontal(|ui| {
                let blocked = self.scan_blocked();
                let response = accent_button(ui, "Scan folders", blocked.is_none());
                if response.clicked() {
                    scan = true;
                }
                if let Some(reason) = &blocked {
                    explain(&response, reason);
                }
                ui.separator();
                ui.label(egui::RichText::new("Source").small().weak());
                let room = ui.available_width();
                let field = if wide {
                    let tail = 2.0 * browse_width + label_width + 4.0 * gap + 20.0;
                    ((room - tail) / 2.0).max(PATH_MIN_WIDTH)
                } else {
                    (room - browse_width - gap).max(PATH_MIN_WIDTH)
                };
                let source = ui.add(
                    egui::TextEdit::singleline(&mut self.source)
                        .hint_text("Incoming folder")
                        .desired_width(field),
                );
                let source_path = self.source.trim().to_owned();
                if !source_path.is_empty() {
                    source.clone().on_hover_text(source_path);
                }
                if source.changed() {
                    self.folder_scope = None;
                }
                if ui.button("Browse…").clicked() {
                    pick = Some(PickTask::Folder(Field::Source));
                }
                if wide {
                    ui.separator();
                    ui.label(egui::RichText::new("Destination").small().weak());
                    let width = (ui.available_width() - browse_width - gap).max(PATH_MIN_WIDTH);
                    let destination = ui.add(
                        egui::TextEdit::singleline(&mut self.destination)
                            .hint_text("Destination folder")
                            .desired_width(width),
                    );
                    let destination_path = self.destination.trim().to_owned();
                    if !destination_path.is_empty() {
                        destination.clone().on_hover_text(destination_path);
                    }
                    if destination.changed() {
                        self.preview = None;
                        self.preview_for = None;
                        self.tree = None;
                    }
                    if ui.button("Browse…").clicked() {
                        pick = Some(PickTask::Folder(Field::Destination));
                    }
                }
            });
            if !wide {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Destination").small().weak());
                    let width = (ui.available_width() - browse_width - gap).max(PATH_MIN_WIDTH);
                    let destination = ui.add(
                        egui::TextEdit::singleline(&mut self.destination)
                            .hint_text("Destination folder")
                            .desired_width(width),
                    );
                    let destination_path = self.destination.trim().to_owned();
                    if !destination_path.is_empty() {
                        destination.clone().on_hover_text(destination_path);
                    }
                    if destination.changed() {
                        self.preview = None;
                        self.preview_for = None;
                        self.tree = None;
                    }
                    if ui.button("Browse…").clicked() {
                        pick = Some(PickTask::Folder(Field::Destination));
                    }
                });
            }
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Folder layout").small().weak());
                profile_combo(ui, &mut self.profile_name, self.profiles.as_ref());
                let rules = accent_text_button(ui, "Rules…");
                if rules.clicked() {
                    rules_open = true;
                }
                ui.separator();
                ui.label(egui::RichText::new("Existing files").small().weak());
                egui::ComboBox::from_id_salt("incoming_policy")
                    .selected_text(policy_label(self.policy))
                    .show_ui(ui, |ui| {
                        for policy in POLICIES {
                            if ui
                                .selectable_label(self.policy == policy, policy_label(policy))
                                .on_hover_text(policy_hint(policy))
                                .clicked()
                            {
                                self.policy = policy;
                            }
                        }
                    });
                ui.separator();
                if ui.button("Refresh preview").clicked() {
                    refresh_preview = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Suggest folders…").clicked() {
                        self.ai_open = true;
                    }
                    ui.separator();
                    ui.label(egui::RichText::new(self.list_summary()).small().weak());
                });
            });
        });

        let progress_max = (ui.available_height() - 420.0).clamp(150.0, 560.0);
        egui::Panel::bottom("incoming_progress_dock")
            .resizable(true)
            .default_size(230.0)
            .min_size(150.0)
            .max_size(progress_max)
            .show(ui, |ui| self.progress_dock(ui, jobs));

        egui::CentralPanel::default().show(ui, |ui| {
            let destination_max = (ui.available_height() - 260.0).clamp(120.0, 520.0);
            egui::Panel::bottom("incoming_destination_dock")
                .resizable(true)
                .default_size(250.0)
                .min_size(120.0)
                .max_size(destination_max)
                .show(ui, |ui| self.destination_dock(ui));

            egui::Panel::left("incoming_source_pane")
                .resizable(true)
                .default_size(300.0)
                .min_size(200.0)
                .max_size(560.0)
                .show(ui, |ui| self.source_pane(ui));

            egui::CentralPanel::default().show(ui, |ui| {
                let editor_max = (ui.available_height() - 150.0).clamp(80.0, 360.0);
                egui::Panel::bottom("incoming_item_editor")
                    .resizable(true)
                    .default_size(170.0)
                    .min_size(80.0)
                    .max_size(editor_max)
                    .show(ui, |ui| self.item_editor_pane(ui));
                self.items_pane(ui);
            });
        });

        self.rules_window(ui.ctx(), jobs);
        self.frames_window(ui.ctx());

        if scan {
            self.begin_scan(jobs);
        }
        if let Some(task) = pick {
            self.begin_pick(jobs, task);
        }
        if refresh_preview {
            self.preview_for = None;
            self.preview_requested = true;
        }
        if rules_open {
            self.rules_open = true;
        }
        if self.profile_name != profile_before {
            self.rematch();
        }
        if std::mem::take(&mut self.preview_requested) {
            self.begin_preview(jobs);
        }
        self.sync_draft();
        self.invalidate_review();
        let context = ui.ctx().clone();
        self.ai_window(&context, jobs);
    }

    /// The bottom strip's validation control.
    pub fn validate_control(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        let blocked = self.validate_blocked();
        let response = ui.add_enabled(blocked.is_none(), egui::Button::new("Validate selection"));
        if response.clicked() {
            self.begin_validation(jobs);
        }
        if let Some(reason) = &blocked {
            explain(&response, reason);
        }
        if let Some(state) = self.state_text() {
            ui.label(egui::RichText::new(state).small().weak());
        }
    }

    /// The bottom strip's single execution control. The label follows the
    /// Copy/Move choice, and only the validated plan matching the current draft
    /// may run.
    pub fn execution_control(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        let blocked = self.execute_blocked();
        let label = if self.move_files {
            "Move selected"
        } else {
            "Copy selected"
        };
        let response = accent_button(ui, label, blocked.is_none());
        if response.clicked() {
            self.begin_execution(jobs);
        }
        if let Some(reason) = &blocked {
            explain(&response, reason);
        }
    }

    fn state_text(&self) -> Option<String> {
        if self.execute_run.is_some() {
            return Some(if self.move_files {
                "Moving files.".to_string()
            } else {
                "Copying files.".to_string()
            });
        }
        if self.validate_run.is_some() {
            return Some("Validating the current selection.".to_string());
        }
        let review = self.review.as_ref()?;
        let mut parts = vec![format!(
            "{} folders · {} ready · {} skip · {} blocked",
            review.folders, review.files_ready, review.files_skip, review.files_blocked
        )];
        if review.space_unknown {
            parts.push("free space unknown".to_string());
        }
        Some(parts.join(" · "))
    }

    fn list_summary(&self) -> String {
        if self.proposals.is_empty() {
            return match &self.scan_error {
                Some(error) => format!("Scan failed: {error}"),
                None => "Nothing scanned yet".to_string(),
            };
        }
        let (items, files, _) = self.selected_counts();
        let folders = self.source_folders();
        format!(
            "{} folders · {} items · {items} selected ({files} files)",
            folders.len(),
            self.proposals.len()
        )
    }

    fn source_folders(&self) -> Vec<PathBuf> {
        let Some(scanned) = &self.scanned else {
            return Vec::new();
        };
        let root = &scanned.snapshot.root;
        let mut folders: Vec<PathBuf> = Vec::new();
        for entry in &scanned.snapshot.entries {
            let Some(parent) = entry.path.parent() else {
                continue;
            };
            let mut current = parent.to_path_buf();
            while current.starts_with(root) && current != *root {
                if !folders.iter().any(|known| known == &current) {
                    folders.push(current.clone());
                }
                match current.parent() {
                    Some(parent) => current = parent.to_path_buf(),
                    None => break,
                }
            }
        }
        folders.sort();
        folders
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn title_name(file: ConfigFile) -> &'static str {
    match file {
        ConfigFile::Patterns => "Saving the pattern file",
        ConfigFile::Profiles => "Saving the profile file",
    }
}

fn offset(index: usize, delta: isize, length: usize) -> Option<usize> {
    if delta == 0 || length == 0 {
        return None;
    }
    let target = index as isize + delta;
    if target < 0 || target >= length as isize {
        return None;
    }
    Some(target as usize)
}

fn tag_index(kind: TagKind) -> usize {
    match kind {
        TagKind::Shot => 0,
        TagKind::Stage => 1,
        TagKind::Task => 2,
        TagKind::Asset => 3,
        TagKind::Resolution => 4,
        TagKind::Version => 5,
    }
}

fn describe_transfer(error: &TransferError) -> String {
    let mut text = error.message().to_string();
    if let Some(path) = error.path() {
        text.push_str(&format!(" ({})", path.display()));
    }
    text
}

fn job_status(jobs: &Jobs, id: Option<JobId>) -> Option<&JobStatus> {
    let id = id?;
    jobs.statuses().find(|status| status.id == id)
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

/// Reviewed files, excluding the directory actions every plan also carries.
fn file_labels(labels: &[ItemLabel]) -> usize {
    labels.iter().filter(|label| !label.directory).count()
}

fn report_rows(report: &RunReport, labels: &[ItemLabel]) -> (Vec<ReportRow>, Vec<String>) {
    let mut rows = Vec::with_capacity(report.items.len());
    let mut recovery = Vec::new();
    for item in &report.items {
        let label = usize::try_from(item.ordinal)
            .ok()
            .and_then(|index| labels.get(index));
        // A directory action is evidence, not a file row; it still occupies its
        // ordinal in the journal, so the label list keeps it and only the row is
        // left out. A folder action that did not simply succeed keeps its row:
        // nothing else would report it.
        if label.is_some_and(|label| label.directory)
            && matches!(item.state, ItemState::Created | ItemState::Skipped)
        {
            for path in &item.recovery {
                let text = path.display().to_string();
                if !recovery.contains(&text) {
                    recovery.push(text);
                }
            }
            continue;
        }
        let mut paths = Vec::new();
        for path in &item.recovery {
            let text = path.display().to_string();
            if !recovery.contains(&text) {
                recovery.push(text.clone());
            }
            paths.push(text);
        }
        rows.push(ReportRow {
            directory: label.is_some_and(|label| label.directory),
            outcome: Some(item.state),
            source: label
                .map(|label| label.source.clone())
                .unwrap_or_else(PathBuf::new),
            destination: label
                .map(|label| label.destination.clone())
                .unwrap_or_else(|| "—".to_string()),
            state: item_state_label(item.state),
            failed: matches!(
                item.state,
                ItemState::Failed | ItemState::CopiedSourceRetained | ItemState::Cancelled
            ),
            message: item.message.clone(),
            recovery: paths,
        });
    }
    (rows, recovery)
}

fn failed_run(
    failure: ExecuteFailure,
    labels: &[ItemLabel],
    draft: Draft,
    mut warnings: Vec<String>,
) -> RunDone {
    let mut counts = RunCounts {
        total: file_labels(labels),
        ..RunCounts::default()
    };
    let mut state = RunState::Interrupted;
    let mut inspected: HashMap<i64, &ItemInspection> = HashMap::new();
    let mut trustworthy = false;
    let mut recovery = Vec::new();
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
    let rows = labels
        .iter()
        .enumerate()
        .filter_map(|(ordinal, label)| {
            let item = inspected.get(&(ordinal as i64)).copied();
            let confirmed = item.filter(|item| {
                trustworthy
                    && item.message_error.is_none()
                    && !matches!(item.state, ItemState::Pending | ItemState::Running)
            });
            // Failed or unresolved folder actions keep their row; a folder that
            // simply was created stays out of the file list.
            if label.directory
                && confirmed.is_some_and(|item| {
                    matches!(item.state, ItemState::Created | ItemState::Skipped)
                })
            {
                return None;
            }
            let mut messages = Vec::new();
            let mut paths = Vec::new();
            if let Some(item) = item {
                if let Some(error) = &item.message_error {
                    messages.push(format!("Journal inspection: {error}"));
                }
                for event in &item.events {
                    if let Some(message) = &event.message {
                        messages.push(message.clone());
                    }
                    for path in event.recovery_paths() {
                        let text = path.display().to_string();
                        if !recovery.contains(&text) {
                            recovery.push(text.clone());
                        }
                        paths.push(text);
                    }
                }
            }
            if confirmed.is_none() {
                messages.push(
                    "No confirmed terminal outcome. The destination may have changed; inspect it before retrying."
                        .to_string(),
                );
            }
            Some(ReportRow {
                directory: label.directory,
                outcome: confirmed.map(|item| item.state),
                source: label.source.clone(),
                destination: label.destination.clone(),
                state: confirmed
                    .map(|item| item_state_label(item.state))
                    .unwrap_or("Outcome unknown"),
                failed: confirmed.is_none_or(|item| {
                    matches!(
                        item.state,
                        ItemState::Failed | ItemState::Cancelled | ItemState::CopiedSourceRetained
                    )
                }),
                message: Some(messages.join("\n")).filter(|text| !text.is_empty()),
                recovery: paths,
            })
        })
        .collect();
    RunDone {
        draft,
        run_id: failure.run_id,
        error: Some(failure.message),
        progress: failure.progress,
        state,
        counts,
        files: file_labels(labels),
        rows,
        warnings,
        recovery,
    }
}

/// Read-only bounded destination walk for the preview: display spellings and
/// directory flags only, never written anywhere. Entries that cannot be read or
/// spelled are counted in `unlisted` instead of vanishing from the tree.
fn read_preview(root: &Path, control: &JobControl) -> Result<PreviewTree, PreviewRead> {
    if control.checkpoint() == Checkpoint::Cancelled {
        return Err(PreviewRead::Cancelled);
    }
    let mut tree = PreviewTree::default();
    if !root.is_dir() {
        return Err(PreviewRead::Failed(format!(
            "{} is not a folder, so it has nothing to preview",
            root.display()
        )));
    }
    let mut pending = vec![(root.to_path_buf(), 0usize)];
    let mut walked = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        if control.checkpoint() == Checkpoint::Cancelled {
            return Err(PreviewRead::Cancelled);
        }
        if depth > PREVIEW_MAX_DEPTH {
            tree.truncated = true;
            continue;
        }
        let entries = match std::fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => {
                tree.truncated = true;
                tree.unlisted += 1;
                continue;
            }
        };
        for entry in entries {
            if tree.entries.len() >= PREVIEW_MAX_ENTRIES {
                tree.truncated = true;
                break;
            }
            if walked % CHECKPOINT_STRIDE == 0 && control.checkpoint() == Checkpoint::Cancelled {
                return Err(PreviewRead::Cancelled);
            }
            walked += 1;
            let Ok(entry) = entry else {
                tree.unlisted += 1;
                continue;
            };
            let path = entry.path();
            let Ok(relative) = path.strip_prefix(root) else {
                tree.unlisted += 1;
                continue;
            };
            let Some(spelling) = relative.to_str() else {
                // The native name is untouched; only its display spelling is
                // impossible here, so it is reported rather than hidden.
                tree.unlisted += 1;
                continue;
            };
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            tree.entries.insert(spelling.to_string(), is_dir);
            if is_dir {
                pending.push((path, depth + 1));
            }
        }
        if tree.entries.len() >= PREVIEW_MAX_ENTRIES {
            tree.truncated = true;
            break;
        }
    }
    if control.checkpoint() == Checkpoint::Cancelled {
        return Err(PreviewRead::Cancelled);
    }
    Ok(tree)
}

/// Why a preview walk produced no tree.
enum PreviewRead {
    Cancelled,
    Failed(String),
}
// ---------------------------------------------------------------------------
// Panes
// ---------------------------------------------------------------------------

impl OrganizeIncomingState {
    /// The source pane: a text filter over the scanned folders, the folder
    /// scope, and the scanned folders themselves. Selecting a folder here filters
    /// the list; it never changes which items are selected.
    fn source_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Source").small().weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{} folders", self.source_folders().len()))
                        .small()
                        .weak(),
                );
            });
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.filter)
                .hint_text("Filter files by name or destination")
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.folder_scope.is_none(), "All folders")
                .clicked()
            {
                self.folder_scope = None;
            }
            if let Some(scope) = self.folder_scope.clone() {
                let name = scope
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| scope.display().to_string());
                if ui
                    .selectable_label(true, format!("{name}  ✕"))
                    .on_hover_text(scope.display().to_string())
                    .clicked()
                {
                    self.folder_scope = None;
                }
            }
        });
        ui.separator();
        if self.scanned.is_none() {
            empty_state(
                ui,
                "No source folder scanned.",
                "Choose the incoming folder, then Scan folders.",
            );
            return;
        }
        let root = self
            .scanned
            .as_ref()
            .map(|scanned| scanned.snapshot.root.clone())
            .unwrap_or_default();
        let folders = self.source_folders();
        if folders.is_empty() {
            empty_state(
                ui,
                "No subfolders found.",
                "Every scanned file sits directly in the source folder.",
            );
            return;
        }
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for folder in folders {
                    let depth = folder
                        .strip_prefix(&root)
                        .map(|relative| relative.components().count())
                        .unwrap_or(0);
                    let name = folder
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| folder.display().to_string());
                    ui.horizontal(|ui| {
                        ui.add_space(depth as f32 * 12.0);
                        let selected = self.folder_scope.as_deref() == Some(folder.as_path());
                        if ui
                            .selectable_label(selected, name)
                            .on_hover_text(folder.display().to_string())
                            .clicked()
                        {
                            self.folder_scope = if selected { None } else { Some(folder.clone()) };
                        }
                    });
                }
            });
    }

    /// The working list: one row per analyzed item, with its exact destination
    /// folder, type and frame count and, only when a gap exists, an orange
    /// missing-frame warning.
    fn items_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Files and destinations").small().weak());
            if ui.small_button("Select all").clicked() {
                self.select_all();
            }
            if ui.small_button("Select none").clicked() {
                self.clear_selection();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (items, files, bytes) = self.selected_counts();
                ui.label(
                    egui::RichText::new(format!(
                        "{items} items · {files} files · {}",
                        size_text(bytes)
                    ))
                    .small()
                    .weak(),
                );
            });
        });
        ui.separator();
        if self.rows.is_empty() {
            empty_state(
                ui,
                "No files analyzed yet.",
                "Scan the source folder to group its frames and route its files.",
            );
            return;
        }
        let mut actions: Vec<RowAction> = Vec::new();
        let rows = std::mem::take(&mut self.rows);
        let visible: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| self.row_visible(row))
            .map(|(index, _)| index)
            .collect();
        if visible.is_empty() {
            self.rows = rows;
            empty_state(
                ui,
                "No item matches the current filter.",
                "Clear the filter or choose another source folder.",
            );
            return;
        }
        let height = ui.available_height().max(ITEM_ROW_HEIGHT * 4.0);
        egui::ScrollArea::horizontal()
            .id_salt("incoming_items_horizontal")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(960.0);
                egui_extras::TableBuilder::new(ui)
                    .id_salt("incoming_items_table")
                    .resizable(true)
                    .striped(true)
                    .min_scrolled_height(ITEM_ROW_HEIGHT * 4.0)
                    .max_scroll_height(height)
                    .auto_shrink([false, false])
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(egui_extras::Column::auto())
                    .column(
                        egui_extras::Column::initial(260.0)
                            .at_least(120.0)
                            .clip(true),
                    )
                    .column(
                        egui_extras::Column::initial(110.0)
                            .at_least(64.0)
                            .clip(true),
                    )
                    .column(egui_extras::Column::initial(70.0).at_least(48.0))
                    .column(
                        egui_extras::Column::initial(240.0)
                            .at_least(110.0)
                            .clip(true),
                    )
                    .column(egui_extras::Column::remainder().at_least(180.0))
                    .header(20.0, |mut header| {
                        for title in ["", "File", "Type", "Frames", "Destination", "State"] {
                            header.col(|ui| {
                                ui.label(egui::RichText::new(title).small().weak());
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(ITEM_ROW_HEIGHT, visible.len(), |mut table_row| {
                            let Some(row) = visible
                                .get(table_row.index())
                                .and_then(|index| rows.get(*index))
                            else {
                                return;
                            };
                            if let Some(action) = self.row_cells(&mut table_row, row) {
                                actions.push(action);
                            }
                        });
                    });
            });
        self.rows = rows;
        for action in actions {
            match action {
                RowAction::Select(item, selected) => self.set_selected(item, selected),
                RowAction::Focus(item) => {
                    self.focus = Some(item);
                    self.sync_editor();
                }
                RowAction::Frames(item) => self.frames_open = Some(item),
            }
        }
    }

    fn row_visible(&self, row: &Row) -> bool {
        if let Some(scope) = &self.folder_scope {
            if !row.source_dir.starts_with(scope) {
                return false;
            }
        }
        let filter = self.filter.trim().to_lowercase();
        if filter.is_empty() {
            return true;
        }
        row.label.to_lowercase().contains(&filter)
            || row.folder.to_lowercase().contains(&filter)
            || row.kind.to_lowercase().contains(&filter)
    }

    /// One working-list row. Columns are the table's, not a stretched line: the
    /// file name stays next to its type and frames, the planned folder is its
    /// own column, and details wait on hover or in the frames dialog.
    fn row_cells(
        &self,
        table_row: &mut egui_extras::TableRow<'_, '_>,
        row: &Row,
    ) -> Option<RowAction> {
        let mut action = None;
        let mut checked = self.selected.contains(&row.item);
        table_row.col(|ui| {
            if ui.checkbox(&mut checked, "").changed() {
                action = Some(RowAction::Select(row.item, checked));
            }
        });
        let focused = self.focus == Some(row.item);
        table_row.col(|ui| {
            let mut label = egui::RichText::new(&row.label);
            if focused {
                label = label.strong();
            }
            let response = ui.selectable_label(focused, label);
            if response.clicked() {
                action = Some(RowAction::Focus(row.item));
            }
            response.on_hover_text(format!(
                "{}\n{}",
                row.source_dir.display(),
                size_text(row.bytes)
            ));
        });
        table_row.col(|ui| {
            ui.label(egui::RichText::new(&row.kind).small().color(MUTED));
        });
        table_row.col(|ui| {
            if row.sequence {
                ui.label(
                    egui::RichText::new(row.frames.to_string())
                        .small()
                        .color(MUTED),
                );
            }
        });
        table_row.col(|ui| {
            let folder = if row.folder.is_empty() {
                "—".to_string()
            } else {
                row.folder.clone()
            };
            let response = ui.label(egui::RichText::new(folder).small().color(MUTED));
            let root = self.destination.trim();
            if !root.is_empty() && !row.folder.is_empty() {
                response.on_hover_text(Path::new(root).join(&row.folder).display().to_string());
            }
        });
        table_row.col(|ui| {
            ui.horizontal(|ui| {
                // The footage rule is expected routing, so it stays quiet; a
                // conflict or a broken destination is the only loud state.
                if row.state != MappingState::Matched {
                    let color = match row.state {
                        MappingState::Ambiguous | MappingState::Invalid => DANGER,
                        _ => MUTED,
                    };
                    ui.label(egui::RichText::new(row.state.label()).small().color(color))
                        .on_hover_text(&row.reason);
                }
                if row.gaps > 0 {
                    let response = ui.label(
                        egui::RichText::new(format!("missing {}", row.gaps))
                            .small()
                            .color(WARNING),
                    );
                    if let Some(detail) = &row.missing {
                        response.on_hover_text(detail);
                    }
                }
                let label = if row.sequence { "Frames" } else { "Files" };
                if ui.small_button(label).clicked() {
                    action = Some(RowAction::Frames(row.item));
                }
            });
        });
        action
    }

    /// The exact frames dialog: every member file of one item, never a range.
    fn frames_window(&mut self, ctx: &egui::Context) {
        let Some(item) = self.frames_open else {
            return;
        };
        let Some(proposal) = self.proposals.iter().find(|proposal| proposal.item == item) else {
            self.frames_open = None;
            return;
        };
        let mut open = true;
        egui::Window::new(format!("{} · exact files", proposal.label))
            .open(&mut open)
            .resizable(true)
            .default_size([420.0, 300.0])
            .show(ctx, |ui| self.member_list(ui, item));
        if !open {
            self.frames_open = None;
        }
    }

    /// The exact member files of one item. A sequence lists its actual frames,
    /// never a range, inside its own bounded scroll area.
    fn member_list(&self, ui: &mut egui::Ui, item: usize) {
        let Some(scanned) = &self.scanned else {
            return;
        };
        let Some(analyzed) = scanned.analysis.items.get(item) else {
            return;
        };
        let files = analyzed.files();
        ui.label(
            egui::RichText::new(format!("{} exact files", files.len()))
                .small()
                .weak(),
        );
        egui::ScrollArea::vertical()
            .id_salt("incoming_frames_list")
            .max_height(MEMBER_SCROLL_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for file in files {
                    let name = file
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| file.path.display().to_string());
                    ui.label(egui::RichText::new(name).small().monospace().color(MUTED));
                }
            });
        if let IncomingItem::Sequence(sequence) = analyzed {
            if !sequence.duplicate_frames.is_empty() {
                let frames = sequence
                    .duplicate_frames
                    .iter()
                    .map(|duplicate| duplicate.frame.to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                ui.label(
                    egui::RichText::new(format!("duplicate frames: {frames}"))
                        .small()
                        .color(WARNING),
                );
            }
        }
    }

    /// The contextual editor for the focused item, plus the batch controls that
    /// apply the same tags to every selected item.
    fn item_editor_pane(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Selected item").small().weak());
            match self
                .focus
                .and_then(|item| self.proposals.iter().find(|p| p.item == item))
            {
                Some(proposal) => {
                    ui.label(egui::RichText::new(&proposal.label).small().strong());
                    if let Some(value) = proposal.extracted.get(TagKind::Task) {
                        ui.label(
                            egui::RichText::new(format!("task {value}"))
                                .small()
                                .color(MUTED),
                        );
                    }
                }
                None => {
                    ui.label(
                        egui::RichText::new("Choose a row to edit its tags and destination.")
                            .small()
                            .color(MUTED),
                    );
                }
            }
        });
        if self.focus.is_none() {
            return;
        }
        let mut changed = false;
        for chunk in [0usize, 3usize] {
            ui.horizontal(|ui| {
                for offset in 0..3 {
                    let index = chunk + offset;
                    let kind = TagKind::ALL[index];
                    ui.label(egui::RichText::new(kind.label()).small().weak());
                    changed |= ui
                        .add(
                            egui::TextEdit::singleline(&mut self.editor.tags[tag_index(kind)])
                                .hint_text("—")
                                .desired_width(110.0),
                        )
                        .changed();
                }
            });
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Destination").small().weak());
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.editor.folder)
                        .hint_text("folder inside the destination")
                        .desired_width(320.0),
                )
                .changed();
            ui.label(egui::RichText::new("Name").small().weak());
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.editor.filename)
                        .hint_text("keep")
                        .desired_width(170.0),
                )
                .changed();
            if changed {
                // The buffers only describe edits; nothing is applied until the
                // user says so, and nothing reaches the filesystem at all.
                ui.label(egui::RichText::new("not applied").small().color(WARNING));
            }
        });
        ui.horizontal(|ui| {
            let selected = self.selected.len();
            if accent_text_button(ui, "Apply to item").clicked() {
                self.apply_editor_to_item();
            }
            if ui
                .add_enabled(
                    selected > 0,
                    egui::Button::new(format!("Apply tags to {selected} selected")),
                )
                .clicked()
            {
                self.apply_editor_to_selection(false);
            }
            if ui
                .add_enabled(
                    selected > 0,
                    egui::Button::new(format!("Apply folder to {selected} selected")),
                )
                .clicked()
            {
                self.apply_editor_to_selection(true);
            }
            if ui.button("Reset item").clicked() {
                self.reset_item();
            }
            if ui.button("Reset selected").clicked() {
                self.reset_selection();
            }
        });
        if let Some(reason) = self
            .focus
            .and_then(|item| self.proposals.iter().find(|p| p.item == item))
            .map(|proposal| proposal.reason.clone())
        {
            ui.label(egui::RichText::new(reason).small().color(MUTED));
        }
    }

    /// The lower dock: the real destination tree with the planned additions in
    /// it, so a conflict is visible before anything runs.
    fn destination_dock(&mut self, ui: &mut egui::Ui) {
        let proposed = self.proposed_paths();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Destination preview").small().weak());
            let existing = self
                .preview
                .as_ref()
                .map(|tree| tree.entries.len())
                .unwrap_or(0);
            ui.label(
                egui::RichText::new(format!("{existing} existing · {} planned", proposed.len()))
                    .small()
                    .color(MUTED),
            );
            if self.preview.as_ref().is_some_and(|tree| tree.truncated) {
                ui.label(
                    egui::RichText::new("preview truncated")
                        .small()
                        .color(WARNING),
                )
                .on_hover_text("Only the first part of the destination tree is shown.");
            }
            if let Some(unlisted) = self
                .preview
                .as_ref()
                .map(|tree| tree.unlisted)
                .filter(|unlisted| *unlisted > 0)
            {
                ui.label(
                    egui::RichText::new(format!("{unlisted} entries not shown"))
                        .small()
                        .color(WARNING),
                )
                .on_hover_text(
                    "The destination holds entries that could not be listed here: an unreadable folder, or a name that is not UTF-8 text. Nothing was changed; check the folder itself.",
                );
            }
            if self.preview_for.is_none() && self.destination.trim().is_empty() {
                ui.label(
                    egui::RichText::new("choose a destination folder")
                        .small()
                        .color(MUTED),
                );
            }
        });
        let key = self.preview_key();
        if self.tree.is_none() || self.tree_key != key {
            let existing = self
                .preview
                .as_ref()
                .map(|tree| tree.entries.clone())
                .unwrap_or_default();
            self.tree = Some(build_preview(&existing, &proposed));
            self.tree_key = key;
        }
        ui.separator();
        let Some(tree) = &self.tree else {
            empty_state(
                ui,
                "No destination preview yet.",
                "Choose a destination folder to see its tree beside the planned files.",
            );
            return;
        };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                preview_node_ui(ui, tree, 0);
            });
    }

    fn preview_key(&self) -> String {
        // The proposed half of the tree is derived from the current rules and
        // edits, so a rule or profile edit must rebuild it even when the
        // selection and the edit set are unchanged.
        format!(
            "{}|{}|{}|{}|{}|{}",
            self.preview_for.as_deref().unwrap_or_default(),
            self.review.is_some(),
            self.selection_generation,
            self.edit_generation,
            self.patterns_revision,
            self.scan_generation
        )
    }

    /// Every planned destination spelling, relative to the destination root.
    fn proposed_paths(&self) -> Vec<String> {
        let mut paths = Vec::new();
        if let Some(review) = &self.review {
            for item in &review.items {
                if item.kind == ActionKind::Directory {
                    continue;
                }
                let Some(relative) = &item.destination else {
                    continue;
                };
                // The destination evidence is absolute; the preview is relative.
                let text = relative.path().display().to_string();
                let root = self.destination.trim();
                let spelling = text
                    .strip_prefix(root)
                    .map(|rest| rest.trim_start_matches(['/', '\\']).to_string())
                    .unwrap_or(text);
                if !paths.contains(&spelling) {
                    paths.push(spelling);
                }
            }
            return paths;
        }
        for proposal in &self.proposals {
            if !self.selected.contains(&proposal.item) || !proposal.is_executable() {
                continue;
            }
            for (_, relative) in proposal.destinations() {
                let spelling = relative
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                if !paths.contains(&spelling) {
                    paths.push(spelling);
                }
            }
        }
        paths
    }

    /// Scan, match and transfer progress with the real counts behind them, then
    /// the run report and every error and recovery path.
    fn progress_dock(&mut self, ui: &mut egui::Ui, jobs: &Jobs) {
        let scan_job = self.scan_run.as_ref().map(|run| run.job);
        let validate_job = self.validate_run.as_ref().map(|run| run.job);
        let execute_job = self.execute_run.as_ref().map(|run| run.job);
        let moving = self.move_files;

        let scan = job_status(jobs, scan_job);
        let validate = job_status(jobs, validate_job);
        let execute = job_status(jobs, execute_job);

        let files = self
            .scanned
            .as_ref()
            .map(|scanned| scanned.snapshot.entries.len())
            .unwrap_or(0);
        let items = self.proposals.len();
        let scan_detail = match scan {
            Some(job) => format!("{} files so far", job.progress.completed),
            None if files > 0 => format!("{files} files"),
            None => "waiting".to_string(),
        };
        let scan_fraction = match scan {
            Some(job) => job.progress.fraction(),
            None if files > 0 => 1.0,
            None => 0.0,
        };
        let match_detail = match (scan, validate) {
            (Some(job), _) if job.phase == JobPhase::Validating => {
                format!("{} items matched", job.progress.completed)
            }
            (_, Some(job)) => format!(
                "{} of {} reviewed",
                job.progress.completed,
                job.progress.total.unwrap_or(0)
            ),
            _ if items > 0 => format!("{items} items matched"),
            _ => "waiting".to_string(),
        };
        // Reviewing is validation work; the bar is labelled for the work it shows.
        let review_label = if validate.is_some() {
            "Review"
        } else {
            "Match"
        };
        let match_fraction = match (scan, validate) {
            (Some(job), _) if job.phase == JobPhase::Validating => job.progress.fraction(),
            (_, Some(job)) => job.progress.fraction(),
            _ if items > 0 => 1.0,
            _ => 0.0,
        };
        let transfer_detail = match execute {
            Some(job) => {
                let mut detail = format!(
                    "{} of {} operations",
                    job.progress.completed,
                    job.progress.total.unwrap_or(0)
                );
                if let Some(total) = job.progress.bytes_total.filter(|total| *total > 0) {
                    detail.push_str(&format!(
                        " · {} of {}",
                        size_text(job.progress.bytes_done),
                        size_text(total)
                    ));
                }
                detail
            }
            None => self
                .report
                .as_ref()
                .map(|report| {
                    format!(
                        "{} of {} files copied or moved · {} skipped",
                        report.counts.copied + report.counts.moved,
                        report.files,
                        report_file_counts(report).1
                    )
                })
                .unwrap_or_else(|| "waiting".to_string()),
        };
        let transfer_fraction = match execute {
            Some(job) => job.progress.fraction(),
            None => self
                .report
                .as_ref()
                .map(|report| report.progress.fraction())
                .unwrap_or(0.0),
        };

        for (phase, fraction, detail) in [
            ("Scan", scan_fraction, scan_detail),
            (review_label, match_fraction, match_detail),
            (
                if moving { "Move" } else { "Copy" },
                transfer_fraction,
                transfer_detail,
            ),
        ] {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(phase).small().weak());
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(200.0)
                        .desired_height(9.0),
                );
                ui.label(egui::RichText::new(detail).small().color(MUTED));
            });
        }
        if let Some(current) = execute.and_then(|job| job.progress.current.as_ref()) {
            ui.label(
                egui::RichText::new(current.display().to_string())
                    .small()
                    .monospace()
                    .color(MUTED),
            );
        }
        if let Some(message) = &self.message {
            ui.label(egui::RichText::new(message).small().color(WARNING));
        }
        let warnings = self
            .review
            .as_ref()
            .map(|review| review.warnings.clone())
            .unwrap_or_default();
        if !warnings.is_empty() {
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("{} review warnings", warnings.len()))
                    .small()
                    .color(WARNING),
            )
            .id_salt("incoming_review_warnings")
            .show(ui, |ui| {
                for warning in &warnings {
                    ui.label(egui::RichText::new(warning).small().color(WARNING));
                }
            });
        }
        ui.separator();
        if self.report.is_none() {
            ui.label(
                egui::RichText::new(
                    "The run report appears here: one row per file, with its destination, outcome, error and any recovery path.",
                )
                .small()
                .color(MUTED),
            );
            return;
        }
        let Some(report) = &self.report else {
            return;
        };
        let headline = report_headline(report);
        let stale = report.draft != self.draft();
        let failed = report.error.is_some() || report.rows.iter().any(|row| row.failed);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(headline).small().color(if failed {
                WARNING
            } else {
                NEW_ENTRY
            }));
            if let Some(run_id) = report.run_id {
                ui.label(
                    egui::RichText::new(format!("run {run_id}"))
                        .small()
                        .color(MUTED),
                );
            }
            if stale {
                ui.label(
                    egui::RichText::new("the selection changed since this run")
                        .small()
                        .color(WARNING),
                );
            }
        });
        if let Some(error) = &report.error {
            ui.label(egui::RichText::new(error).small().color(DANGER));
        }
        if !report.warnings.is_empty() {
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("{} run warnings", report.warnings.len()))
                    .small()
                    .color(WARNING),
            )
            .id_salt("incoming_report_warnings")
            .show(ui, |ui| {
                for warning in &report.warnings {
                    ui.label(egui::RichText::new(warning).small().color(WARNING));
                }
            });
        }
        if !report.recovery.is_empty() {
            egui::CollapsingHeader::new(
                egui::RichText::new(format!("{} kept-aside paths", report.recovery.len())).small(),
            )
            .id_salt("incoming_report_recovery")
            .show(ui, |ui| {
                for path in &report.recovery {
                    ui.label(
                        egui::RichText::new(format!("kept aside: {path}"))
                            .small()
                            .monospace()
                            .color(MUTED),
                    );
                }
            });
        }
        // A copy leaves the source untouched, so it needs no rescan warning; a
        // move does change what the scan read.
        if report.draft.action == FileAction::Move
            && report
                .rows
                .iter()
                .any(|row| row.outcome == Some(ItemState::Moved))
        {
            ui.label(
                egui::RichText::new(
                    "The moved source files are gone, so scan the source folder again before planning another run.",
                )
                .small()
                .color(MUTED),
            );
        }
        let root = report.draft.destination.trim().to_string();
        let rows = &report.rows;
        let height = ui.available_height().max(REPORT_ROW_HEIGHT * 4.0);
        egui_extras::TableBuilder::new(ui)
            .id_salt("incoming_report_table")
            .resizable(true)
            .striped(true)
            .min_scrolled_height(REPORT_ROW_HEIGHT * 3.0)
            .max_scroll_height(height)
            .auto_shrink([false, false])
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(
                egui_extras::Column::initial(220.0)
                    .at_least(110.0)
                    .clip(true),
            )
            .column(
                egui_extras::Column::initial(360.0)
                    .at_least(140.0)
                    .clip(true),
            )
            .column(egui_extras::Column::remainder().at_least(170.0))
            .header(18.0, |mut header| {
                for title in ["File", "Destination", "Result"] {
                    header.col(|ui| {
                        ui.label(egui::RichText::new(title).small().weak());
                    });
                }
            })
            .body(|body| {
                body.rows(REPORT_ROW_HEIGHT, rows.len(), |mut table_row| {
                    let Some(row) = rows.get(table_row.index()) else {
                        return;
                    };
                    let color = if row.failed { DANGER } else { MUTED };
                    table_row.col(|ui| {
                        let name = row
                            .source
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let text = if name.is_empty() {
                            "—".to_string()
                        } else {
                            name
                        };
                        let response = ui.label(egui::RichText::new(text).small().color(color));
                        if !row.source.as_os_str().is_empty() {
                            response.on_hover_text(row.source.display().to_string());
                        }
                    });
                    table_row.col(|ui| {
                        let relative = run_relative(&root, &row.destination);
                        ui.label(
                            egui::RichText::new(relative)
                                .small()
                                .monospace()
                                .color(color),
                        )
                        .on_hover_text(row.destination.clone());
                    });
                    table_row.col(|ui| {
                        let mut hover: Vec<String> = Vec::new();
                        if let Some(message) = &row.message {
                            hover.push(message.clone());
                        }
                        for path in &row.recovery {
                            hover.push(format!("kept aside: {path}"));
                        }
                        let text = if row.recovery.is_empty() {
                            row.state.to_string()
                        } else {
                            format!("{} · kept aside", row.state)
                        };
                        let response =
                            ui.label(egui::RichText::new(text).small().color(if row.failed {
                                DANGER
                            } else {
                                NEW_ENTRY
                            }));
                        if !hover.is_empty() {
                            response.on_hover_text(hover.join("\n"));
                        }
                    });
                });
            });
    }

    // ---------------------------------------------------------------- rules

    fn rules_window(&mut self, ctx: &egui::Context, jobs: &mut Jobs) {
        if !self.rules_open {
            return;
        }
        let mut open = self.rules_open;
        let mut save: Option<ConfigFile> = None;
        let mut pick: Option<PickTask> = None;
        let mut actions: Vec<ListAction> = Vec::new();
        egui::Window::new("Folder layout and filename rules")
            .open(&mut open)
            .default_width(660.0)
            .default_height(520.0)
            .vscroll(true)
            .show(ctx, |ui| {
                self.rules_body(ui, &mut actions, &mut save, &mut pick);
            });
        self.rules_open = open;
        if let Some(file) = save {
            self.begin_save(jobs, file);
        }
        if let Some(task) = pick {
            self.begin_pick(jobs, task);
        }
        self.apply_list_actions(actions);
    }

    fn rules_body(
        &mut self,
        ui: &mut egui::Ui,
        actions: &mut Vec<ListAction>,
        save: &mut Option<ConfigFile>,
        pick: &mut Option<PickTask>,
    ) {
        let Some(mut patterns) = self.patterns.clone() else {
            match &self.config_error {
                Some(error) => {
                    ui.label(egui::RichText::new(error).small().color(DANGER));
                    if ui.button("Try again").clicked() {
                        self.load_attempted = false;
                        self.config_error = None;
                    }
                }
                None => {
                    ui.label("The pattern files are still opening.");
                }
            }
            return;
        };
        let mut profiles = self.profiles.clone().unwrap_or_default();

        ui.label(
            egui::RichText::new(format!(
                "Rules live in {}",
                self.store.directory().display()
            ))
            .small()
            .color(MUTED),
        );
        if let Some(message) = &self.config_message {
            ui.label(egui::RichText::new(message).small().color(NEW_ENTRY));
        }
        if let Some(error) = &self.config_error {
            ui.label(egui::RichText::new(error).small().color(DANGER));
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Folder layouts").small().weak());
            if ui.button("Save").clicked() {
                *save = Some(ConfigFile::Profiles);
            }
            if ui.button("Import…").clicked() {
                *pick = Some(PickTask::Import(ConfigFile::Profiles));
            }
            if ui.button("Export…").clicked() {
                if let Some(text) = self.export_text(ConfigFile::Profiles) {
                    *pick = Some(PickTask::Export(ConfigFile::Profiles, text));
                }
            }
        });

        // One profile's ordered rules.
        let selected = profiles
            .position(&self.profile_name)
            .unwrap_or(0)
            .min(profiles.profiles.len().saturating_sub(1));
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("incoming_profile_editor")
                .selected_text(self.profile_name.clone())
                .show_ui(ui, |ui| {
                    for (index, profile) in profiles.profiles.iter().enumerate() {
                        if ui
                            .selectable_label(index == selected, &profile.name)
                            .clicked()
                        {
                            self.profile_name = profile.name.clone();
                        }
                    }
                });
            if ui
                .small_button("↑")
                .on_hover_text("Move layout up")
                .clicked()
            {
                actions.push(ListAction::MoveProfile(selected, -1));
            }
            if ui
                .small_button("↓")
                .on_hover_text("Move layout down")
                .clicked()
            {
                actions.push(ListAction::MoveProfile(selected, 1));
            }
            if ui
                .add_enabled(
                    profiles.profiles.len() > 1,
                    egui::Button::new("Delete layout").small(),
                )
                .on_hover_text("Remove this layout from the file")
                .clicked()
            {
                actions.push(ListAction::DeleteProfile(self.profile_name.clone()));
            }
            ui.label(egui::RichText::new("Layout name").small().weak());
            ui.add(
                egui::TextEdit::singleline(&mut self.profile_entry)
                    .hint_text("name")
                    .desired_width(140.0),
            );
            if ui.small_button("Add").clicked() {
                let name = self.profile_entry.trim().to_string();
                if !name.is_empty() && profiles.find(&name).is_none() {
                    profiles.upsert(IncomingProfile::new(name, Vec::new()));
                    self.profile_entry.clear();
                }
            }
            let name = self.profile_entry.trim().to_string();
            let available = !name.is_empty() && profiles.find(&name).is_none();
            if ui
                .add_enabled(
                    available && !profiles.profiles.is_empty(),
                    egui::Button::new("Rename").small(),
                )
                .clicked()
            {
                profiles.profiles[selected].name = name.clone();
                self.profile_name = name.clone();
                self.profile_entry.clear();
            }
            if ui
                .add_enabled(
                    available && !profiles.profiles.is_empty(),
                    egui::Button::new("Duplicate").small(),
                )
                .clicked()
            {
                let mut profile = profiles.profiles[selected].clone();
                profile.name = name.clone();
                profiles.profiles.push(profile);
                self.profile_name = name;
                self.profile_entry.clear();
            }
        });

        if let Some(profile) = profiles.profiles.get_mut(selected) {
            ui.label(
                egui::RichText::new(
                    "Rules are tried in this order: an exact task match first, then an exact asset match, then the footage rule.",
                )
                .small()
                .color(MUTED),
            );
            let mut remove: Option<usize> = None;
            let mut drops: Vec<(usize, usize)> = Vec::new();
            for (index, rule) in profile.rules.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut rule.path)
                            .hint_text("folder inside the destination")
                            .desired_width(220.0),
                    );
                    if ui.small_button("↑").clicked() {
                        actions.push(ListAction::MoveRule(selected, index, -1));
                    }
                    if ui.small_button("↓").clicked() {
                        actions.push(ListAction::MoveRule(selected, index, 1));
                    }
                    if ui.small_button("✕").clicked() {
                        remove = Some(index);
                    }
                    ui.label(egui::RichText::new("keywords").small().weak());
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(14.0);
                    for (keyword_index, keyword) in rule.keywords.iter_mut().enumerate() {
                        ui.add(
                            egui::TextEdit::singleline(keyword)
                                .desired_width(110.0)
                                .font(egui::TextStyle::Small),
                        );
                        if keyword.trim().is_empty() && ui.small_button("remove").clicked() {
                            drops.push((index, keyword_index));
                        }
                    }
                    if ui.small_button("＋ keyword").clicked() {
                        rule.keywords.push(String::new());
                    }
                });
            }
            if let Some(index) = remove {
                actions.push(ListAction::RemoveRule(selected, index));
            }
            for (rule, keyword) in drops {
                actions.push(ListAction::DropKeyword(selected, rule, keyword));
            }
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_rule_path)
                        .hint_text("folder inside the destination")
                        .desired_width(220.0),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut self.new_rule_keywords)
                        .hint_text("keywords, comma separated")
                        .desired_width(240.0),
                );
                if ui.button("Add rule").clicked() {
                    let path = self.new_rule_path.trim().to_string();
                    if !path.is_empty() {
                        let keywords = self
                            .new_rule_keywords
                            .split(',')
                            .map(|word| word.trim().to_string())
                            .filter(|word| !word.is_empty())
                            .collect();
                        profile.rules.push(IncomingRule { path, keywords });
                        self.new_rule_path.clear();
                        self.new_rule_keywords.clear();
                    }
                }
            });
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Filename patterns").small().weak());
            if ui.button("Save").clicked() {
                *save = Some(ConfigFile::Patterns);
            }
            if ui.button("Reload").clicked() {
                self.patterns = None;
                self.load_attempted = false;
            }
            if ui.button("Import…").clicked() {
                *pick = Some(PickTask::Import(ConfigFile::Patterns));
            }
            if ui.button("Export…").clicked() {
                if let Some(text) = self.export_text(ConfigFile::Patterns) {
                    *pick = Some(PickTask::Export(ConfigFile::Patterns, text));
                }
            }
        });
        for kind in TagKind::ALL {
            let id = format!("patterns_{}", kind.tag_key());
            egui::CollapsingHeader::new(format!("{} ({})", kind.label(), patterns.count(kind)))
                .id_salt(id)
                .default_open(kind == TagKind::Task)
                .show(ui, |ui| {
                    let is_task = kind == TagKind::Task;
                    let mut remove: Option<usize> = None;
                    for (index, rule) in patterns.rules_mut(kind).iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            if is_task {
                                let mut task = rule.task.clone().unwrap_or_default();
                                if ui
                                    .add(
                                        egui::TextEdit::singleline(&mut task)
                                            .hint_text("task key")
                                            .desired_width(150.0),
                                    )
                                    .changed()
                                {
                                    rule.task = Some(task);
                                }
                            }
                            ui.add(
                                egui::TextEdit::singleline(&mut rule.text)
                                    .hint_text("expression")
                                    .desired_width(300.0)
                                    .font(egui::TextStyle::Small),
                            );
                            let mut mode = rule.mode;
                            egui::ComboBox::from_id_salt((kind.tag_key(), index, "mode"))
                                .width(74.0)
                                .selected_text(mode.label())
                                .show_ui(ui, |ui| {
                                    for candidate in [RuleMode::Regex, RuleMode::Literal] {
                                        ui.selectable_value(
                                            &mut mode,
                                            candidate,
                                            candidate.label(),
                                        );
                                    }
                                });
                            rule.mode = mode;
                            if ui.small_button("↑").clicked() {
                                actions.push(ListAction::MovePatternRule(kind, index, -1));
                            }
                            if ui.small_button("↓").clicked() {
                                actions.push(ListAction::MovePatternRule(kind, index, 1));
                            }
                            if ui.small_button("✕").clicked() {
                                remove = Some(index);
                            }
                        });
                    }
                    if let Some(index) = remove {
                        actions.push(ListAction::RemovePatternRule(kind, index));
                    }
                    ui.horizontal(|ui| {
                        let index = tag_index(kind);
                        if is_task
                            && ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.new_pattern_task)
                                        .hint_text("task key")
                                        .desired_width(150.0),
                                )
                                .changed()
                        {
                            // The buffer is read when the pattern is added.
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut self.new_pattern[index])
                                .hint_text("new expression")
                                .desired_width(300.0)
                                .font(egui::TextStyle::Small),
                        );
                        if ui.button("Add pattern").clicked() {
                            let text = self.new_pattern[index].trim().to_string();
                            if !text.is_empty() {
                                let task = if is_task {
                                    Some(self.new_pattern_task.trim().to_string())
                                        .filter(|task| !task.is_empty())
                                } else {
                                    None
                                };
                                patterns.rules_mut(kind).push(PatternRule {
                                    task,
                                    text,
                                    mode: self.new_pattern_mode,
                                });
                                self.new_pattern[index].clear();
                            }
                        }
                        let mut mode = self.new_pattern_mode;
                        egui::ComboBox::from_id_salt((kind.tag_key(), "new_mode"))
                            .width(74.0)
                            .selected_text(mode.label())
                            .show_ui(ui, |ui| {
                                for candidate in [RuleMode::Regex, RuleMode::Literal] {
                                    ui.selectable_value(&mut mode, candidate, candidate.label());
                                }
                            });
                        self.new_pattern_mode = mode;
                    });
                });
        }

        let issues = patterns.dialect_issues();
        if !issues.is_empty() {
            ui.separator();
            ui.label(
                egui::RichText::new(format!(
                    "{} pattern(s) use syntax this dialect cannot run; they match as plain text instead",
                    issues.len()
                ))
                .small()
                .color(WARNING),
            );
            for issue in &issues {
                ui.label(egui::RichText::new(issue.describe()).small().color(DANGER));
            }
        }

        // Commit whatever the widgets changed; the setters compare first, so a
        // frame that changed nothing does no work.
        self.set_profiles(profiles);
        self.set_patterns(patterns);
    }
}

/// The deferred per-row result so a panel can render rows without holding a
/// mutable borrow of the row list.
enum RowAction {
    Select(usize, bool),
    Focus(usize),
    Frames(usize),
}

fn size_text(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
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

fn report_headline(report: &RunDone) -> String {
    let (completed, skipped, unfinished) = report_file_counts(report);
    let state = match report.state {
        RunState::Succeeded => "Finished",
        RunState::Partial => "Finished with items left out",
        RunState::Failed => "Failed",
        RunState::Cancelled => "Cancelled",
        RunState::Running => "Running",
        RunState::Pending => "Not started",
        RunState::Interrupted => "Interrupted",
    };
    format!(
        "{state}: {} of {} files copied or moved · {} skipped · {} unfinished",
        completed, report.files, skipped, unfinished
    )
}

fn report_file_counts(report: &RunDone) -> (usize, usize, usize) {
    let mut counts = (0, 0, 0);
    for row in report.rows.iter().filter(|row| !row.directory) {
        match row.outcome {
            Some(ItemState::Copied | ItemState::Moved) => counts.0 += 1,
            Some(ItemState::Skipped) => counts.1 += 1,
            Some(ItemState::Pending | ItemState::Running) | None => counts.2 += 1,
            _ => {}
        }
    }
    counts
}

fn accent_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let text = egui::RichText::new(label).color(egui::Color32::WHITE);
    ui.add_enabled(enabled, egui::Button::new(text).fill(ACCENT))
}

fn accent_text_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(egui::Button::new(egui::RichText::new(label).color(egui::Color32::WHITE)).fill(ACCENT))
}

fn explain(response: &egui::Response, reason: &str) {
    let _ = response.clone().on_hover_text(reason);
}

/// Width a button with this text takes, measured the way egui measures it, so a
/// toolbar can reserve the space before laying out a field beside it.
fn button_width(ui: &egui::Ui, label: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let text = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, egui::Color32::WHITE)
        .size()
        .x;
    text + 2.0 * ui.spacing().button_padding.x
}

fn small_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = egui::TextStyle::Small.resolve(ui.style());
    ui.painter()
        .layout_no_wrap(text.to_owned(), font, MUTED)
        .size()
        .x
}

/// Destination spelling relative to the run's own destination root, so a report
/// row stays readable after the user edits the destination field.
fn run_relative(root: &str, path: &str) -> String {
    if root.is_empty() {
        return path.to_string();
    }
    match Path::new(path).strip_prefix(Path::new(root)) {
        Ok(relative) if !relative.as_os_str().is_empty() => relative.display().to_string(),
        _ => path.to_string(),
    }
}

fn empty_state(ui: &mut egui::Ui, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(12.0);
        ui.label(title);
        ui.add_space(2.0);
        ui.label(egui::RichText::new(hint).color(MUTED));
    });
}

fn profile_combo(ui: &mut egui::Ui, name: &mut String, profiles: Option<&ProfileSet>) {
    let names = profiles.map(ProfileSet::names).unwrap_or_default();
    egui::ComboBox::from_id_salt("incoming_profile")
        .selected_text(if name.is_empty() {
            "no layout".to_string()
        } else {
            name.clone()
        })
        .show_ui(ui, |ui| {
            for candidate in &names {
                if ui.selectable_label(name == candidate, candidate).clicked() {
                    *name = candidate.clone();
                }
            }
        });
}

fn insert_preview(root: &mut PreviewNode, path: &str, is_dir: bool, proposed: bool, exists: bool) {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return;
    }
    let mut node = root;
    for (index, part) in parts.iter().enumerate() {
        let last = index + 1 == parts.len();
        let position = match node.children.iter().position(|child| child.name == *part) {
            Some(position) => position,
            None => {
                node.children.push(PreviewNode {
                    name: (*part).to_string(),
                    is_dir: !last || is_dir,
                    ..Default::default()
                });
                node.children.len() - 1
            }
        };
        node = &mut node.children[position];
        if last {
            node.is_dir = is_dir;
            node.proposed |= proposed;
            node.exists |= exists;
        } else {
            node.is_dir = true;
        }
    }
}

fn build_preview(existing: &BTreeMap<String, bool>, proposed: &[String]) -> PreviewNode {
    let mut root = PreviewNode {
        name: String::new(),
        is_dir: true,
        ..Default::default()
    };
    for (path, is_dir) in existing {
        insert_preview(&mut root, &path.replace('\\', "/"), *is_dir, false, true);
    }
    for path in proposed {
        insert_preview(&mut root, path, false, true, false);
    }
    sort_preview(&mut root);
    root
}

impl OrganizeIncomingState {
    fn prepare_ai(&mut self) -> Result<(), String> {
        let scanned = self
            .scanned
            .as_ref()
            .ok_or("Scan the incoming folder first.")?;
        let mut indices = self.selected.iter().copied().collect::<Vec<_>>();
        indices.sort_unstable();
        let mut selected = Vec::new();
        for index in indices {
            let item = scanned
                .analysis
                .items
                .get(index)
                .ok_or("The selected file list changed; scan again.")?;
            let files = item.files();
            let filename = files
                .first()
                .and_then(|file| file.path.file_name())
                .and_then(|name| name.to_str())
                .ok_or("A selected filename cannot be sent as Unicode text.")?
                .to_owned();
            selected.push(crate::ai::AiItem {
                item_id: index as u64,
                filename,
                kind: if matches!(item, IncomingItem::Sequence(_)) {
                    "frame sequence"
                } else {
                    "single file"
                }
                .into(),
                actual_frames: if matches!(item, IncomingItem::Sequence(_)) {
                    files.len()
                } else {
                    0
                },
                size_bytes: files
                    .iter()
                    .try_fold(0u64, |sum, file| sum.checked_add(file.stamp.size)),
            });
        }
        let draft = self.draft();
        let previous_proposals = self
            .ai_previous
            .as_ref()
            .filter(|(old, _)| {
                old.source == draft.source
                    && old.destination == draft.destination
                    && old.profile == draft.profile
                    && old.scan_generation == draft.scan_generation
                    && old.patterns_revision == draft.patterns_revision
            })
            .map(|(_, suggestions)| {
                suggestions
                    .iter()
                    .filter(|suggestion| {
                        selected
                            .iter()
                            .any(|item| item.item_id == suggestion.item_id)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let request_id = self
            .ai_counter
            .checked_add(1)
            .ok_or("The assistant request counter is exhausted; restart the app.")?;
        let request = crate::ai::AiRequest {
            request_id,
            selected,
            structure: self
                .active_profile()
                .ok_or("Choose a folder layout first.")?
                .rules
                .iter()
                .map(|rule| rule.path.clone())
                .collect(),
            instructions: self.ai_instruction.clone(),
            previous_proposals,
        };
        let context = crate::ai::request_context(&request)?;
        self.ai_counter = request_id;
        self.ai_prepared = Some(AiPrepared {
            draft,
            request,
            context,
            config: self.ai_config.clone(),
        });
        self.ai_response = None;
        self.ai_error = None;
        Ok(())
    }

    fn begin_ai(&mut self, jobs: &mut Jobs) {
        if self.busy() || jobs.is_shutting_down() {
            return;
        }
        let Some(prepared) = &self.ai_prepared else {
            return;
        };
        if prepared.draft != self.draft() {
            self.ai_error =
                Some("The file selection or layout changed. Prepare a new request.".into());
            return;
        }
        let request = prepared.request.clone();
        let config = prepared.config.clone();
        let draft = prepared.draft.clone();
        let worker_request = request.clone();
        let (sender, replies) = channel();
        match jobs.start(&self.ctx, "Suggesting incoming folders", move |context| {
            context
                .events
                .progress(JobPhase::Suggesting, JobProgress::default());
            let result = crate::ai::suggest(&config, &worker_request, &context.control);
            let outcome = if context.control.is_cancelled() {
                JobOutcome::Cancelled
            } else if result.is_ok() {
                JobOutcome::Succeeded
            } else {
                JobOutcome::Failed
            };
            let count = result.as_ref().map(Vec::len).unwrap_or(0) as u64;
            let _ = sender.send(result);
            context.events.finish(
                outcome,
                JobProgress {
                    completed: count,
                    total: Some(worker_request.selected.len() as u64),
                    ..Default::default()
                },
            );
        }) {
            Ok(job) => {
                self.ai_run = Some(AiRun {
                    job,
                    draft,
                    request,
                    replies,
                });
                self.ai_error = None;
                self.ai_response = None;
            }
            Err(error) => {
                self.ai_error = Some(format!("The assistant worker could not start: {error}"))
            }
        }
    }

    fn poll_ai(&mut self) {
        let Some(run) = self.ai_run.take() else {
            return;
        };
        let result = match run.replies.try_recv() {
            Err(TryRecvError::Empty) => {
                self.ai_run = Some(run);
                return;
            }
            Err(TryRecvError::Disconnected) => {
                Err("The assistant stopped without a result. Nothing was applied.".into())
            }
            Ok(result) => result,
        };
        if run.draft != self.draft() {
            self.ai_error = Some("The files or layout changed while the assistant was working. Its reply was discarded.".into());
            return;
        }
        match result {
            Err(error) => self.ai_error = Some(error),
            Ok(suggestions) => {
                // Decode again against the captured IDs if the bridge ever changes.
                let response = serde_json::json!({"suggestions": suggestions});
                match crate::ai::decode_suggestions(&response.to_string(), &run.request) {
                    Err(error) => self.ai_error = Some(error),
                    Ok(suggestions) => {
                        self.ai_previous = Some((run.draft.clone(), suggestions.clone()));
                        self.ai_response = Some(AiResponse {
                            draft: run.draft,
                            choices: suggestions
                                .into_iter()
                                .map(|suggestion| AiChoice {
                                    selected: true,
                                    suggestion,
                                })
                                .collect(),
                        });
                        self.ai_error = None;
                    }
                }
            }
        }
    }

    fn apply_ai(&mut self) -> Result<(), String> {
        if self.busy() {
            return Err("Wait for the current operation to finish.".into());
        }
        let response = self
            .ai_response
            .as_ref()
            .ok_or("There are no suggestions to apply.")?;
        if response.draft != self.draft() {
            return Err("The selection or layout changed. Request fresh suggestions.".into());
        }
        let edits = response
            .choices
            .iter()
            .filter(|choice| choice.selected)
            .map(|choice| {
                let folder =
                    crate::patterns::split_relative(&choice.suggestion.destination_relative)?
                        .join("/");
                let index = usize::try_from(choice.suggestion.item_id)
                    .map_err(|_| "A suggestion has an invalid item ID.")?;
                if !self.selected.contains(&index) {
                    return Err("A suggested item is no longer selected.".into());
                }
                Ok((index, folder))
            })
            .collect::<Result<Vec<_>, String>>()?;
        if edits.is_empty() {
            return Err("Select a suggestion to apply.".into());
        }
        for (index, folder) in edits {
            self.edits.entry(index).or_default().folder = folder;
        }
        self.edit_generation += 1;
        self.ai_response = None;
        self.ai_prepared = None;
        self.rematch();
        self.tree = None;
        self.message = Some("Suggested folders applied to the draft. Review the destinations, then validate the selection.".into());
        Ok(())
    }

    fn ai_window(&mut self, ctx: &egui::Context, jobs: &mut Jobs) {
        if !self.ai_open {
            return;
        }
        let mut open = self.ai_open;
        let mut prepare = false;
        let mut send = false;
        let mut apply = false;
        egui::Window::new("Suggest folders")
            .open(&mut open)
            .default_width(760.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(
                    "Pi handles sign-in and model setup. Only the reviewed context below is sent.",
                );
                let mut changed = ui
                    .add_enabled(
                        self.ai_run.is_none(),
                        egui::TextEdit::multiline(&mut self.ai_instruction)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY)
                            .hint_text("Describe how these files should be organized"),
                    )
                    .changed();
                ui.collapsing("Pi settings", |ui| {
                    ui.add_enabled_ui(self.ai_run.is_none(), |ui| {
                        let mut executable =
                            self.ai_config.executable.to_string_lossy().into_owned();
                        ui.horizontal(|ui| {
                            ui.label("Command");
                            if ui.text_edit_singleline(&mut executable).changed() {
                                self.ai_config.executable = executable.clone().into();
                                changed = true;
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label("Model");
                            changed |= ui
                                .add(
                                    egui::TextEdit::singleline(&mut self.ai_config.model)
                                        .hint_text("Use Pi's selected model"),
                                )
                                .changed();
                        });
                        ui.horizontal(|ui| {
                            ui.label("Timeout");
                            changed |= ui
                                .add(
                                    egui::DragValue::new(&mut self.ai_config.timeout_seconds)
                                        .range(1..=600),
                                )
                                .changed();
                            ui.label("seconds");
                        });
                    });
                });
                if changed {
                    self.ai_prepared = None;
                }
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.busy(), egui::Button::new("Prepare request"))
                        .clicked()
                    {
                        prepare = true;
                    }
                    let enabled = !self.busy()
                        && self
                            .ai_prepared
                            .as_ref()
                            .is_some_and(|prepared| prepared.draft == self.draft());
                    if ui
                        .add_enabled(enabled, egui::Button::new("Send to Pi").fill(ACCENT))
                        .clicked()
                    {
                        send = true;
                    }
                    if self.ai_run.is_some() {
                        ui.label("Waiting for suggestions…");
                    }
                });
                if let Some(error) = &self.ai_error {
                    ui.colored_label(WARNING, error);
                }
                if let Some(prepared) = &mut self.ai_prepared {
                    ui.collapsing("Context sent to Pi", |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("ai_request_context")
                            .max_height(180.0)
                            .show(ui, |ui| {
                                ui.add(
                                    egui::TextEdit::multiline(&mut prepared.context)
                                        .font(egui::TextStyle::Monospace)
                                        .interactive(false)
                                        .desired_width(f32::INFINITY),
                                );
                            });
                    });
                }
                if let Some(response) = &mut self.ai_response {
                    egui::ScrollArea::vertical()
                        .id_salt("ai_folder_choices")
                        .max_height(340.0)
                        .show(ui, |ui| {
                            for choice in &mut response.choices {
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut choice.selected, "");
                                    let index = choice.suggestion.item_id as usize;
                                    ui.label(
                                        self.proposals
                                            .iter()
                                            .find(|proposal| proposal.item == index)
                                            .map(|proposal| proposal.label.as_str())
                                            .unwrap_or("Unknown item"),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(
                                            &mut choice.suggestion.destination_relative,
                                        )
                                        .desired_width(280.0),
                                    );
                                });
                                ui.label(
                                    egui::RichText::new(&choice.suggestion.reason)
                                        .small()
                                        .weak(),
                                );
                                ui.separator();
                            }
                        });
                    if ui.button("Apply selected folders to draft").clicked() {
                        apply = true;
                    }
                }
            });
        self.ai_open = open;
        if prepare {
            if let Err(error) = self.prepare_ai() {
                self.ai_error = Some(error);
            }
        }
        if send {
            self.begin_ai(jobs);
        }
        if apply {
            if let Err(error) = self.apply_ai() {
                self.ai_error = Some(error);
            }
        }
    }
}

fn sort_preview(node: &mut PreviewNode) {
    node.children.sort_by(|left, right| {
        right
            .is_dir
            .cmp(&left.is_dir)
            .then_with(|| left.name.cmp(&right.name))
    });
    for child in &mut node.children {
        sort_preview(child);
    }
}

fn preview_node_ui(ui: &mut egui::Ui, node: &PreviewNode, depth: usize) {
    for child in &node.children {
        let planned_new = child.proposed && !child.exists;
        let conflict = child.proposed && child.exists && !child.is_dir;
        let label = if child.is_dir {
            child.name.clone()
        } else {
            child.name.clone()
        };
        let colour = if conflict {
            WARNING
        } else if planned_new {
            NEW_ENTRY
        } else {
            MUTED
        };
        if child.is_dir {
            let title = if planned_new {
                format!("{}  +", child.name)
            } else {
                label
            };
            egui::CollapsingHeader::new(egui::RichText::new(title).small().color(colour))
                .id_salt((depth, &child.name, child.proposed, child.exists))
                .default_open(depth < 2)
                .show(ui, |ui| preview_node_ui(ui, child, depth + 1));
        } else {
            let title = if conflict {
                format!("{}  (exists)", child.name)
            } else if planned_new {
                format!("{}  +", child.name)
            } else {
                label
            };
            ui.horizontal(|ui| {
                ui.add_space(14.0);
                ui.label(egui::RichText::new(title).small().monospace().color(colour));
            });
        }
    }
}
