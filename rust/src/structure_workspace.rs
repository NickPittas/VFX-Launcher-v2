//! Shots & Sequences workspace and the New project dialog.
//!
//! Both surfaces are the launcher's own UI over the S1 structure planners
//! (`structure.rs`) and the S2 Nuke generator (`nuke.rs`); no planning, naming or
//! script text lives here. What this module owns is the desktop interaction:
//!
//! - the checkable tree of *existing project folders*
//!   (`add_to_existing_project_tab.py` `populate_existing_tree`), where every
//!   folder is an independently checkable, arbitrary target;
//! - the editable sequence/shot editor (`widgets/shot_sequence_widgets.py`):
//!   typable sequence names, a per-sequence Add shot, typable and removable shot
//!   names, and standalone shots — never synthetic numbered names;
//! - Nuke options (frame rate, resolution preset or custom size, proxy, ACES) and
//!   "parent folder as sequence", which only `plan_add_to_targets` consumes;
//! - one preview, one explicit review, then one tracked run:
//!   `Regenerate preview` → `Check additions` → `Add to project`.
//!
//! Every execution reuses the shared engine: a planner builds an
//! [`OperationPlan`], `transfers::validate_plan` produces the reviewed
//! `ValidatedPlan` on a tracked worker, and `transfers::execute` runs it after
//! `ValidatedPlan::attribute_to` binds the acting user. Nothing in this module
//! writes to the filesystem or the catalog.
//!
//! The New project dialog is the same flow for a plan whose first action is the
//! new project root; after a successful run the root is registered in the catalog
//! through [`ProjectWorkspace::register_created_project`], which also selects and
//! scans it.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;

use crate::jobs::{
    JobContext, JobControl, JobEvents, JobId, JobOutcome, JobPhase, JobProgress, Jobs,
};
use crate::nuke::{RESOLUTION_PRESETS, ScriptGenerator};
use crate::project_workspace::ProjectWorkspace;
use crate::storage::{CatalogClient, ProjectId, UserId};
use crate::structure::{
    self, CreateProject, NamingPreset, NukeSettings, ShotGroup, StructureTemplate, TemplateMarkers,
};
use crate::templates::{LibrarySnapshot, TemplateLibrary, TemplateSummary};
use crate::transfers::{
    self, ActionKind, ItemValidation, OperationPlan, PlannedAction, ReviewState, RunReport,
    RunState, TransferError, ValidatedPlan, ValidationReport,
};

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2f, 0x6b, 0xa8);
const WARNING: egui::Color32 = egui::Color32::from_rgb(0xe0, 0x9a, 0x6a);
/// Frame rates the source widgets offered.
const FPS_PRESETS: [f64; 9] = [23.976, 24.0, 25.0, 29.97, 30.0, 50.0, 59.94, 60.0, 120.0];
const NO_PROJECT: &str = "Select a project in Projects first.";
const BUSY: &str = "Wait for the current operation to finish.";
const NO_CATALOG: &str = "The project catalog is not ready yet.";
const NO_USER: &str = "Choose who is using the app first.";
const NO_TEMPLATE: &str = "Create or import a folder template first.";

// ---------------------------------------------------------------------------
// Small UI helpers
// ---------------------------------------------------------------------------

fn accent_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let text = egui::RichText::new(label).color(egui::Color32::WHITE);
    ui.add_enabled(enabled, egui::Button::new(text).fill(ACCENT))
}

fn explain(response: &egui::Response, reason: &str) {
    let _ = response.clone().on_hover_text(reason);
}

fn notice(ui: &mut egui::Ui, message: &str, error: bool) {
    let text = egui::RichText::new(message).small();
    if error {
        ui.label(text.color(WARNING));
    } else {
        ui.label(text.weak());
    }
}

/// Every line of a list the user must be able to read in full: load warnings and
/// run reports scroll inside their own box instead of being truncated.
fn notices(ui: &mut egui::Ui, title: &str, id: &str, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    egui::CollapsingHeader::new(format!("{title} ({})", lines.len()))
        .id_salt(id)
        .default_open(lines.len() <= 6)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt((id, "lines"))
                .max_height(160.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for line in lines {
                        ui.label(egui::RichText::new(line).small().weak());
                    }
                });
        });
}

fn empty_state(ui: &mut egui::Ui, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(10.0);
        ui.label(title);
        ui.add_space(2.0);
        ui.label(egui::RichText::new(hint).color(egui::Color32::from_gray(180)));
    });
}

fn file_label(relative: &Path) -> String {
    relative
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| relative.display().to_string())
}

fn describe(error: &TransferError) -> String {
    format!("{}: {}", error.category().label(), error.message())
}

// ---------------------------------------------------------------------------
// Editable sequence/shot tree
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct RowId(u64);

#[derive(Clone, Debug, PartialEq, Eq)]
struct ShotDraft {
    id: RowId,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SequenceDraft {
    id: RowId,
    name: String,
    shots: Vec<ShotDraft>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct TreeDraft {
    sequences: Vec<SequenceDraft>,
    independent: Vec<ShotDraft>,
}

/// The source widgets as one value: sequences the user typed, a shot row list per
/// sequence, and independent ("standalone") shots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShotTreeEditor {
    draft: TreeDraft,
    next_row: u64,
    /// Row that receives focus on the next frame (a newly added row).
    focus: Option<RowId>,
}

impl ShotTreeEditor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn clear(&mut self) {
        let next_row = self.next_row;
        *self = Self {
            next_row,
            ..Self::default()
        };
    }

    pub fn is_empty(&self) -> bool {
        self.draft.sequences.is_empty() && self.draft.independent.is_empty()
    }

    /// Rows that would be planned. A blank sequence name with no shots is an empty
    /// draft, not an addition, so it is dropped instead of failing validation.
    fn groups(&self) -> Vec<ShotGroup> {
        let shots = |rows: &[ShotDraft]| -> Vec<String> {
            rows.iter()
                .map(|row| row.name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect()
        };
        let mut groups = Vec::new();
        for sequence in &self.draft.sequences {
            let names = shots(&sequence.shots);
            if sequence.name.trim().is_empty() && names.is_empty() {
                continue;
            }
            groups.push(ShotGroup {
                sequence: Some(sequence.name.clone()),
                shots: names,
            });
        }
        let independent = shots(&self.draft.independent);
        if !independent.is_empty() {
            groups.push(ShotGroup {
                sequence: None,
                shots: independent,
            });
        }
        groups
    }

    fn typed_counts(&self) -> (usize, usize) {
        let sequences = self.draft.sequences.len();
        let shots = self
            .draft
            .sequences
            .iter()
            .map(|sequence| {
                sequence
                    .shots
                    .iter()
                    .filter(|shot| !shot.name.trim().is_empty())
                    .count()
            })
            .sum::<usize>()
            + self
                .draft
                .independent
                .iter()
                .filter(|shot| !shot.name.trim().is_empty())
                .count();
        (sequences, shots)
    }

    /// Draws the editor. Returns `true` when a name or row changed.
    pub fn ui(&mut self, ui: &mut egui::Ui, enabled: bool) -> bool {
        let mut changed = false;
        let mut next_row = self.next_row;
        let mut focus = self.focus.take();
        let mut remove_sequence: Option<RowId> = None;
        let mut remove_shot: Option<(Option<RowId>, RowId)> = None;
        let mut add_shot: Option<Option<RowId>> = None;
        let limit = structure::MAX_SEQUENCE_OR_SHOT_CHARS;
        let (sequences, shots) = self.typed_counts();

        ui.horizontal(|ui| {
            if ui
                .add_enabled(enabled, egui::Button::new("Add sequence"))
                .clicked()
            {
                let sequence_id = RowId(next_row);
                next_row += 1;
                let shot_id = RowId(next_row);
                next_row += 1;
                self.draft.sequences.push(SequenceDraft {
                    id: sequence_id,
                    name: String::new(),
                    shots: vec![ShotDraft {
                        id: shot_id,
                        name: String::new(),
                    }],
                });
                focus = Some(sequence_id);
                changed = true;
            }
            ui.label(
                egui::RichText::new(format!("{sequences} sequence(s) · {shots} shot(s)"))
                    .small()
                    .weak(),
            );
        });

        let mut row_changed = false;
        let mut sequence_removed = None;
        let mut sequence_add_shot = None;
        for sequence_index in 0..self.draft.sequences.len() {
            let sequence_id = self.draft.sequences[sequence_index].id;
            ui.push_id(("sequence", sequence_id.0), |ui| {
                ui.horizontal(|ui| {
                    let edit = ui.add_enabled(
                        enabled,
                        egui::TextEdit::singleline(&mut self.draft.sequences[sequence_index].name)
                            .hint_text("Sequence name")
                            .char_limit(limit)
                            .desired_width(170.0),
                    );
                    if edit.changed() {
                        row_changed = true;
                    }
                    if focus == Some(sequence_id) && enabled {
                        edit.request_focus();
                        focus = None;
                    }
                    if ui
                        .add_enabled(enabled, egui::Button::new("Add shot"))
                        .clicked()
                    {
                        sequence_add_shot = Some(sequence_id);
                    }
                    if ui
                        .add_enabled(enabled, egui::Button::new("Remove"))
                        .clicked()
                    {
                        sequence_removed = Some(sequence_id);
                    }
                });
                let mut shot_removed = None;
                let count = self.draft.sequences[sequence_index].shots.len();
                for shot_index in 0..count {
                    let shot_id = self.draft.sequences[sequence_index].shots[shot_index].id;
                    ui.push_id(("shot", shot_id.0), |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(16.0);
                            let edit = ui.add_enabled(
                                enabled,
                                egui::TextEdit::singleline(
                                    &mut self.draft.sequences[sequence_index].shots[shot_index]
                                        .name,
                                )
                                .hint_text("Shot name")
                                .char_limit(limit)
                                .desired_width(170.0),
                            );
                            if edit.changed() {
                                row_changed = true;
                            }
                            if focus == Some(shot_id) && enabled {
                                edit.request_focus();
                                focus = None;
                            }
                            if ui
                                .add_enabled(enabled, egui::Button::new("Remove"))
                                .clicked()
                            {
                                shot_removed = Some(shot_id);
                            }
                        });
                    });
                }
                if let Some(shot) = shot_removed {
                    remove_shot = Some((Some(sequence_id), shot));
                }
            });
            ui.separator();
        }
        if let Some(sequence) = sequence_removed {
            remove_sequence = Some(sequence);
        }

        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Standalone shots").small().weak());
            if ui
                .add_enabled(enabled, egui::Button::new("Add shot"))
                .clicked()
            {
                add_shot = Some(None);
            }
        });
        let mut independent_removed = None;
        for index in 0..self.draft.independent.len() {
            let shot_id = self.draft.independent[index].id;
            ui.push_id(("independent", shot_id.0), |ui| {
                ui.horizontal(|ui| {
                    let edit = ui.add_enabled(
                        enabled,
                        egui::TextEdit::singleline(&mut self.draft.independent[index].name)
                            .hint_text("Shot name")
                            .char_limit(limit)
                            .desired_width(170.0),
                    );
                    if edit.changed() {
                        row_changed = true;
                    }
                    if focus == Some(shot_id) && enabled {
                        edit.request_focus();
                        focus = None;
                    }
                    if ui
                        .add_enabled(enabled, egui::Button::new("Remove"))
                        .clicked()
                    {
                        independent_removed = Some(shot_id);
                    }
                });
            });
        }
        if let Some(shot) = independent_removed {
            remove_shot = Some((None, shot));
        }

        if let Some(sequence) = remove_sequence {
            self.draft.sequences.retain(|row| row.id != sequence);
            changed = true;
        }
        if let Some((sequence, shot)) = remove_shot {
            match sequence {
                Some(sequence) => {
                    if let Some(row) = self
                        .draft
                        .sequences
                        .iter_mut()
                        .find(|row| row.id == sequence)
                    {
                        row.shots.retain(|row| row.id != shot);
                    }
                }
                None => self.draft.independent.retain(|row| row.id != shot),
            }
            changed = true;
        }
        if let Some(sequence) = add_shot.or(sequence_add_shot.map(Some)) {
            let id = RowId(next_row);
            next_row += 1;
            match sequence {
                Some(sequence) => {
                    if let Some(row) = self
                        .draft
                        .sequences
                        .iter_mut()
                        .find(|row| row.id == sequence)
                    {
                        row.shots.push(ShotDraft {
                            id,
                            name: String::new(),
                        });
                    }
                }
                None => self.draft.independent.push(ShotDraft {
                    id,
                    name: String::new(),
                }),
            }
            focus = Some(id);
            changed = true;
        }
        self.next_row = next_row;
        self.focus = focus;
        changed || row_changed
    }
}

// ---------------------------------------------------------------------------
// Nuke options
// ---------------------------------------------------------------------------

/// The options the source collected, in the order the dock shows them.
#[derive(Clone, Debug, PartialEq)]
struct NukeEditor {
    enabled: bool,
    fps: f64,
    width: u32,
    height: u32,
    proxy: bool,
    aces: bool,
}

impl Default for NukeEditor {
    fn default() -> Self {
        Self {
            enabled: false,
            fps: crate::nuke::DEFAULT_FPS,
            width: crate::nuke::DEFAULT_WIDTH,
            height: crate::nuke::DEFAULT_HEIGHT,
            proxy: false,
            aces: false,
        }
    }
}

impl NukeEditor {
    fn settings(&self) -> Option<NukeSettings> {
        self.enabled.then(|| NukeSettings {
            fps: self.fps,
            width: self.width,
            height: self.height,
            proxy: self.proxy,
            aces: self.aces,
        })
    }

    /// Preset matching the current size, `None` for a custom one.
    fn preset(&self) -> Option<usize> {
        RESOLUTION_PRESETS
            .iter()
            .position(|preset| preset.width == self.width && preset.height == self.height)
    }

    /// Returns `true` when an option changed. `parent_as_sequence` is drawn when
    /// the caller offers it, because only the add-to-existing planner consumes it.
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        enabled: bool,
        parent_as_sequence: Option<&mut bool>,
    ) -> bool {
        let mut changed = false;
        let options_enabled = enabled && self.enabled;
        ui.horizontal_wrapped(|ui| {
            changed |= ui
                .add_enabled(
                    enabled,
                    egui::Checkbox::new(&mut self.enabled, "Nuke scripts"),
                )
                .changed();

            ui.label(egui::RichText::new("Frame rate").small().weak());
            let mut fps_changed = false;
            ui.add_enabled_ui(options_enabled, |ui| {
                egui::ComboBox::from_id_salt("nuke_fps")
                    .selected_text(trim_number(self.fps))
                    .width(74.0)
                    .show_ui(ui, |ui| {
                        for preset in FPS_PRESETS {
                            if ui
                                .selectable_label(self.fps == preset, trim_number(preset))
                                .clicked()
                            {
                                self.fps = preset;
                                fps_changed = true;
                            }
                        }
                    });
            });
            changed |= fps_changed;

            ui.label(egui::RichText::new("Resolution").small().weak());
            let selected = self
                .preset()
                .map(|index| RESOLUTION_PRESETS[index].label)
                .unwrap_or("Custom");
            let mut resolution_changed = false;
            ui.add_enabled_ui(options_enabled, |ui| {
                egui::ComboBox::from_id_salt("nuke_resolution")
                    .selected_text(selected)
                    .width(150.0)
                    .show_ui(ui, |ui| {
                        for preset in RESOLUTION_PRESETS.iter() {
                            let active = self.width == preset.width && self.height == preset.height;
                            if ui.selectable_label(active, preset.label).clicked() {
                                self.width = preset.width;
                                self.height = preset.height;
                                resolution_changed = true;
                            }
                        }
                        if ui
                            .selectable_label(self.preset().is_none(), "Custom")
                            .clicked()
                        {
                            resolution_changed = true;
                        }
                    });
                let mut dimensions = ui
                    .add(egui::DragValue::new(&mut self.width).range(1..=20_000))
                    .changed();
                ui.label("×");
                dimensions |= ui
                    .add(egui::DragValue::new(&mut self.height).range(1..=20_000))
                    .changed();
                resolution_changed |= dimensions;
            });
            changed |= resolution_changed;

            ui.add_enabled_ui(options_enabled, |ui| {
                changed |= ui.checkbox(&mut self.proxy, "Proxy").changed();
                changed |= ui.checkbox(&mut self.aces, "ACES").changed();
                if let Some(parent_as_sequence) = parent_as_sequence {
                    changed |= ui
                        .checkbox(parent_as_sequence, "Parent folder as sequence")
                        .on_hover_text("Names a standalone shot's script after the checked folder.")
                        .changed();
                }
            });
        });
        changed
    }
}

fn trim_number(value: f64) -> String {
    if (value.fract()).abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

// ---------------------------------------------------------------------------
// Drafts
// ---------------------------------------------------------------------------

/// Which planner a draft feeds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlanMode {
    NewProject,
    AddToProject,
}

/// How an add-to-existing draft chooses its destinations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Destinations {
    /// Every checked folder in the existing-project tree (`plan_add_to_targets`).
    #[default]
    Checked,
    /// The chosen template's marker folders inside the project (`plan_add_shots`).
    Template,
}

/// Everything one plan depends on. Comparing a stored draft with the live one is
/// the whole staleness rule: any edit invalidates the previous preview and review.
#[derive(Clone, Debug, PartialEq)]
struct StructureDraft {
    mode: PlanMode,
    /// Existing project root (add mode).
    root: Option<PathBuf>,
    /// Destination parent of the new project (new-project mode).
    parent: Option<PathBuf>,
    naming: NamingPreset,
    number: String,
    client: String,
    name: String,
    date: String,
    template: String,
    destinations: Destinations,
    /// Absolute checked folders, sorted; empty in template mode.
    targets: Vec<PathBuf>,
    groups: Vec<ShotGroup>,
    nuke: Option<NukeSettings>,
    parent_as_sequence: bool,
    /// Template-library content revision this draft was built from. Any library
    /// mutation bumps it, which invalidates this draft's preview and review the
    /// same way an edited field does.
    library: u64,
}

impl StructureDraft {
    fn template_destinations(&self) -> bool {
        matches!(self.destinations, Destinations::Template)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum PreviewKind {
    Folder,
    Script,
}

#[derive(Clone, Debug)]
struct PreviewRow {
    depth: usize,
    label: String,
    path: PathBuf,
    kind: PreviewKind,
    /// Index into `plan.actions`; review rows carry the same index.
    action: usize,
}

/// The plan the user is looking at, plus its flattened tree.
#[derive(Clone, Debug)]
struct Preview {
    draft: StructureDraft,
    revision: u64,
    plan: OperationPlan,
    /// Existing project root (additions) or the folder the new project creates.
    project_root: PathBuf,
    rows: Vec<PreviewRow>,
    folders: usize,
    scripts: usize,
    warnings: Vec<String>,
}

impl Preview {
    fn build(draft: StructureDraft, plan: OperationPlan) -> Self {
        let mut entries: Vec<(PathBuf, PreviewKind, usize)> = Vec::new();
        let mut folders = 0usize;
        let mut scripts = 0usize;
        for (index, action) in plan.actions.iter().enumerate() {
            match action {
                PlannedAction::CreateDirectory { relative } => {
                    folders += 1;
                    entries.push((relative.path().to_path_buf(), PreviewKind::Folder, index));
                }
                PlannedAction::CreateFile { relative, .. }
                | PlannedAction::File { relative, .. } => {
                    scripts += 1;
                    entries.push((relative.path().to_path_buf(), PreviewKind::Script, index));
                }
            }
        }
        // A new project's first action is its own root; additions run in an
        // existing root, so the plan root is already the project folder.
        let plan_root = plan.destination_root_path().to_path_buf();
        let project_root = if draft.mode == PlanMode::NewProject {
            plan.actions
                .first()
                .map(|action| plan_root.join(action.relative().path()))
                .unwrap_or_else(|| plan_root.clone())
        } else {
            plan_root
        };
        entries.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let rows = entries
            .into_iter()
            .map(|(path, kind, action)| PreviewRow {
                depth: path.components().count().saturating_sub(1),
                label: file_label(&path),
                path,
                kind,
                action,
            })
            .collect();
        Self {
            revision: plan.revision,
            warnings: plan.warnings.clone(),
            draft,
            plan,
            project_root,
            rows,
            folders,
            scripts,
        }
    }
}

/// One reviewed row.
#[derive(Clone, Debug)]
struct ItemRow {
    label: String,
    kind: &'static str,
    state: &'static str,
    message: Option<String>,
    blocked: bool,
}

#[derive(Clone, Debug, Default)]
struct ReviewCounts {
    ready: usize,
    skipped: usize,
    blocked: usize,
}

/// The reviewed result of `Check additions`.
struct ReviewView {
    draft: StructureDraft,
    revision: u64,
    ready: Option<ValidatedPlan>,
    project_root: PathBuf,
    rows: Vec<ItemRow>,
    counts: ReviewCounts,
    warnings: Vec<String>,
    refusal: Option<String>,
}

impl ReviewView {
    fn from_report(
        draft: StructureDraft,
        revision: u64,
        report: ValidationReport,
        root: &Path,
        project_root: PathBuf,
    ) -> Self {
        let rows = report
            .items
            .iter()
            .map(|item| item_row(item, root))
            .collect::<Vec<_>>();
        let mut counts = ReviewCounts::default();
        for item in &report.items {
            match item.state {
                ReviewState::Ready => counts.ready += 1,
                ReviewState::Skipped => counts.skipped += 1,
                ReviewState::Blocked => counts.blocked += 1,
            }
        }
        Self {
            draft,
            revision,
            ready: report.ready,
            project_root,
            rows,
            counts,
            warnings: report.warnings,
            refusal: report.refusal,
        }
    }

    fn summary(&self) -> String {
        format!(
            "Checked preview {} · {} ready · {} skipped · {} blocked",
            self.revision, self.counts.ready, self.counts.skipped, self.counts.blocked
        )
    }
}

fn item_row(item: &ItemValidation, root: &Path) -> ItemRow {
    let label = item
        .destination
        .as_ref()
        .map(|path| {
            path.path()
                .strip_prefix(root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| path.path().to_path_buf())
                .display()
                .to_string()
        })
        .unwrap_or_else(|| "—".to_string());
    ItemRow {
        label,
        kind: match item.kind {
            ActionKind::Directory => "folder",
            ActionKind::CreateFile => "script",
            ActionKind::File => "file",
        },
        state: match item.state {
            ReviewState::Ready => "ready",
            ReviewState::Skipped => "skip",
            ReviewState::Blocked => "blocked",
        },
        message: item.message.clone(),
        blocked: item.state == ReviewState::Blocked,
    }
}

// ---------------------------------------------------------------------------
// Workers
// ---------------------------------------------------------------------------

struct ValidateRun {
    draft: StructureDraft,
    revision: u64,
    /// Plan destination root, used to shorten reviewed item paths.
    root: PathBuf,
    /// Folder this plan creates or adds into; what a run must refresh or register.
    project_root: PathBuf,
    replies: Receiver<Result<ValidationReport, String>>,
}

struct RunFailure {
    message: String,
    inspection: Vec<String>,
    progress: JobProgress,
}

struct RunJob {
    root: PathBuf,
    job: JobId,
    labels: Vec<String>,
    replies: Receiver<Result<RunReport, RunFailure>>,
}

/// Outcome of one execution, for the owner that must refresh or register.
#[derive(Clone, Debug)]
struct CompletedRun {
    root: PathBuf,
    succeeded: bool,
    created: usize,
    failed: usize,
}

#[derive(Clone, Debug)]
struct RunDone {
    headline: String,
    error: Option<String>,
    lines: Vec<String>,
    inspection: Vec<String>,
}

/// The plan → review → run plumbing shared by both surfaces.
#[derive(Default)]
struct PlanSession {
    counter: u64,
    planning: Option<Receiver<Result<Preview, String>>>,
    preview: Option<Preview>,
    validate: Option<ValidateRun>,
    review: Option<ReviewView>,
    run: Option<RunJob>,
    report: Option<RunDone>,
    message: Option<String>,
    completion: Option<CompletedRun>,
}

impl PlanSession {
    fn next_revision(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    fn busy(&self) -> bool {
        self.planning.is_some() || self.validate.is_some() || self.run.is_some()
    }

    /// Drops results that no longer describe `draft`, as every edit must.
    fn invalidate(&mut self, draft: &StructureDraft) {
        if self.busy() {
            return;
        }
        let stale_preview = self
            .preview
            .as_ref()
            .is_some_and(|preview| &preview.draft != draft);
        let stale_review = self
            .review
            .as_ref()
            .is_some_and(|review| &review.draft != draft);
        if stale_preview {
            self.preview = None;
        }
        if stale_review {
            self.review = None;
        }
        if stale_preview || stale_review {
            self.message = Some("The additions changed; regenerate the preview.".to_string());
        }
    }

    fn clear(&mut self) {
        if self.busy() {
            return;
        }
        *self = Self {
            counter: self.counter,
            ..Self::default()
        };
    }

    fn take_completion(&mut self) -> Option<CompletedRun> {
        self.completion.take()
    }

    fn poll(&mut self) {
        if let Some(replies) = self.planning.take() {
            match replies.try_recv() {
                Ok(Ok(preview)) => {
                    self.preview = Some(preview);
                    self.message = None;
                }
                Ok(Err(message)) => self.message = Some(message),
                Err(TryRecvError::Empty) => self.planning = Some(replies),
                Err(TryRecvError::Disconnected) => {
                    self.message =
                        Some("Preparing the additions stopped without a result.".to_string())
                }
            }
        }
        self.poll_validate();
        self.poll_run();
    }

    fn begin_preview(
        &mut self,
        ctx: &egui::Context,
        jobs: &mut Jobs,
        draft: StructureDraft,
        build: impl FnOnce(&StructureDraft, u64) -> Result<OperationPlan, String> + Send + 'static,
    ) -> Result<(), String> {
        if self.busy() {
            return Err(BUSY.to_string());
        }
        let revision = self.next_revision();
        let (tx, rx) = channel();
        jobs.start(ctx, "Preparing folders and scripts", move |context| {
            let result = if context.control.checkpoint() == crate::jobs::Checkpoint::Cancelled {
                Err("Preparing additions was cancelled.".to_string())
            } else {
                build(&draft, revision).map(|plan| Preview::build(draft, plan))
            };
            let cancelled = context.control.checkpoint() == crate::jobs::Checkpoint::Cancelled;
            let outcome = if cancelled {
                JobOutcome::Cancelled
            } else if result.is_ok() {
                JobOutcome::Succeeded
            } else {
                JobOutcome::Failed
            };
            let result = if cancelled {
                Err("Preparing additions was cancelled.".to_string())
            } else {
                result
            };
            let _ = tx.send(result);
            context.events.finish(outcome, JobProgress::default());
        })
        .map_err(|error| format!("Preparing additions could not start: {error}"))?;
        self.preview = None;
        self.review = None;
        self.report = None;
        self.message = Some("Preparing the destination preview…".to_string());
        self.planning = Some(rx);
        Ok(())
    }

    fn poll_validate(&mut self) {
        let Some(run) = self.validate.take() else {
            return;
        };
        match run.replies.try_recv() {
            Ok(result) => match result {
                Ok(report) => {
                    self.review = Some(ReviewView::from_report(
                        run.draft,
                        run.revision,
                        report,
                        &run.root,
                        run.project_root,
                    ));
                    self.message = None;
                }
                Err(message) => {
                    self.review = None;
                    self.message = Some(message);
                }
            },
            Err(TryRecvError::Empty) => self.validate = Some(run),
            Err(TryRecvError::Disconnected) => {
                self.message = Some("Checking the additions stopped without a report.".to_string());
            }
        }
    }

    fn poll_run(&mut self) {
        let Some(run) = self.run.take() else {
            return;
        };
        match run.replies.try_recv() {
            Ok(result) => {
                match result {
                    Ok(report) => {
                        let counts = &report.summary.counts;
                        let lines = report
                            .items
                            .iter()
                            .map(|item| {
                                let label = run
                                    .labels
                                    .get(usize::try_from(item.ordinal).unwrap_or(usize::MAX))
                                    .cloned()
                                    .unwrap_or_else(|| format!("#{}", item.ordinal));
                                match &item.message {
                                    Some(message) => {
                                        format!("{} {label} — {message}", item.state.as_str())
                                    }
                                    None => format!("{} {label}", item.state.as_str()),
                                }
                            })
                            .collect::<Vec<_>>();
                        self.report = Some(RunDone {
                            headline: format!(
                                "{} · {} created · {} skipped · {} failed",
                                report.summary.state.as_str(),
                                counts.created,
                                counts.skipped,
                                counts.failed
                            ),
                            error: None,
                            lines,
                            inspection: Vec::new(),
                        });
                        self.completion = Some(CompletedRun {
                            root: run.root,
                            succeeded: report.summary.state == RunState::Succeeded,
                            created: counts.created,
                            failed: counts.failed,
                        });
                    }
                    Err(failure) => {
                        self.report = Some(RunDone {
                            headline: "The run could not be completed".to_string(),
                            error: Some(failure.message.clone()),
                            lines: Vec::new(),
                            inspection: failure.inspection,
                        });
                        self.completion = Some(CompletedRun {
                            root: run.root,
                            succeeded: false,
                            created: 0,
                            failed: 0,
                        });
                        self.message = Some(failure.message);
                    }
                }
                // A run changes the destination, so the reviewed plan can never be
                // reused: the next execution needs a fresh check.
                self.review = None;
                self.preview = None;
            }
            Err(TryRecvError::Empty) => self.run = Some(run),
            Err(TryRecvError::Disconnected) => {
                self.review = None;
                self.preview = None;
                self.message = Some(format!(
                    "The run stopped without a report (worker {worker}). Inspect the destination before retrying.",
                    worker = run.job
                ));
            }
        }
    }

    fn begin_validate(
        &mut self,
        ctx: &egui::Context,
        jobs: &mut Jobs,
        draft: StructureDraft,
        plan: OperationPlan,
        revision: u64,
        project_root: PathBuf,
    ) -> Result<(), String> {
        if jobs.is_shutting_down() {
            return Err("The application is closing.".to_string());
        }
        let root = plan.destination_root_path().to_path_buf();
        let total = plan.actions.len() as u64;
        let (tx, rx) = channel();
        let started = jobs.start(ctx, "Checking additions", move |context| {
            let JobContext { events, .. } = context;
            events.progress(
                JobPhase::Validating,
                JobProgress {
                    completed: 0,
                    total: Some(total),
                    ..JobProgress::default()
                },
            );
            let report = transfers::validate_plan(&plan);
            let outcome = if report.counts.blocked > 0 {
                JobOutcome::Partial
            } else {
                JobOutcome::Succeeded
            };
            let progress = JobProgress {
                completed: report.counts.total as u64,
                total: Some(report.counts.total as u64),
                ..JobProgress::default()
            };
            let _ = tx.send(Ok(report));
            events.finish(outcome, progress);
        });
        match started {
            Ok(_) => {
                self.validate = Some(ValidateRun {
                    draft,
                    revision,
                    root,
                    project_root,
                    replies: rx,
                });
                self.review = None;
                self.report = None;
                self.message = None;
                Ok(())
            }
            Err(error) => Err(format!(
                "Checking the additions could not be started: {error}"
            )),
        }
    }

    fn begin_execute(
        &mut self,
        ctx: &egui::Context,
        jobs: &mut Jobs,
        catalog: CatalogClient,
        user: UserId,
        title: &str,
    ) -> Result<(), String> {
        if jobs.is_shutting_down() {
            return Err("The application is closing.".to_string());
        }
        let Some(mut review) = self.review.take() else {
            return Err("Check the additions first.".to_string());
        };
        let Some(validated) = review.ready.take() else {
            self.review = Some(review);
            return Err("The checked plan has blocked items.".to_string());
        };
        let root = review.project_root.clone();
        let labels: Vec<String> = review.rows.iter().map(|row| row.label.clone()).collect();
        self.review = Some(review);
        // The acting identity is bound to the immutable reviewed plan before the
        // worker starts, so the journal records who asked for every mutation.
        let validated = validated.attribute_to(user);
        let (tx, rx) = channel();
        let title = title.to_string();
        let started = jobs.start(ctx, title, move |context| {
            let JobContext {
                control, events, ..
            } = context;
            let result =
                transfers::execute(&catalog, validated, &control, &events).map_err(|error| {
                    let mut message = describe(&error);
                    let inspection = error
                        .run_id()
                        .and_then(|run_id| {
                            match transfers::read_run(&catalog, run_id)
                                .and_then(|reply| reply.wait())
                            {
                                Ok(inspection) => inspection,
                                Err(read_error) => {
                                    message.push_str(&format!(
                                        " The journal record could not be read: {read_error}"
                                    ));
                                    None
                                }
                            }
                        })
                        .map(inspection_lines)
                        .unwrap_or_default();
                    RunFailure {
                        message,
                        inspection,
                        progress: events.last_progress(),
                    }
                });
            let progress = match &result {
                Ok(report) => JobProgress {
                    completed: report.summary.counts.total as u64,
                    total: Some(report.summary.counts.total as u64),
                    ..JobProgress::default()
                },
                Err(failure) => failure.progress.clone(),
            };
            let outcome = match &result {
                Ok(report) => match report.summary.state {
                    RunState::Succeeded => JobOutcome::Succeeded,
                    RunState::Partial => JobOutcome::Partial,
                    RunState::Cancelled => JobOutcome::Cancelled,
                    _ => JobOutcome::Failed,
                },
                Err(_) => JobOutcome::Failed,
            };
            if let Err(failure) = &result {
                events.diagnostic(JobPhase::Copying, failure.message.clone());
            }
            let _ = tx.send(result);
            events.finish(outcome, progress);
        });
        match started {
            Ok(job) => {
                self.run = Some(RunJob {
                    root,
                    job,
                    labels,
                    replies: rx,
                });
                self.report = None;
                self.message = None;
                Ok(())
            }
            Err(error) => Err(format!("The run could not be started: {error}")),
        }
    }
}

fn inspection_lines(inspection: transfers::RunInspection) -> Vec<String> {
    let mut lines = vec![format!(
        "Run {} · {} · {}",
        inspection.run_id,
        inspection.kind,
        inspection.state.as_str()
    )];
    if let Some(actor) = &inspection.actor {
        lines.push(format!("Requested by {}", actor.username));
    }
    for item in &inspection.items {
        let destination = item
            .destination
            .as_ref()
            .map(|path| path.display().to_string())
            .or_else(|| item.destination_display.clone())
            .unwrap_or_else(|| "—".to_string());
        let mut line = format!("{} {destination}", item.state.as_str());
        if let Some(message) = &item.message_error {
            line.push_str(&format!(" — {message}"));
        }
        lines.push(line);
        for event in &item.events {
            for recovery in event.recovery_paths() {
                lines.push(format!("  recovery: {}", recovery.display()));
            }
        }
    }
    for warning in &inspection.warnings {
        lines.push(warning.clone());
    }
    lines
}

// ---------------------------------------------------------------------------
// Existing project folder tree
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct FolderRow {
    relative: PathBuf,
    depth: usize,
}

#[derive(Clone, Debug, Default)]
struct FolderTree {
    rows: Vec<FolderRow>,
    checked: Vec<PathBuf>,
    expanded: Vec<PathBuf>,
    note: Option<String>,
}

impl FolderTree {
    fn clear(&mut self) {
        self.rows.clear();
        self.checked.clear();
        self.expanded.clear();
        self.note = None;
    }

    fn is_checked(&self, relative: &Path) -> bool {
        self.checked.iter().any(|path| path == relative)
    }

    fn toggle(&mut self, relative: &Path, checked: bool) {
        if checked {
            if !self.is_checked(relative) {
                self.checked.push(relative.to_path_buf());
            }
        } else {
            self.checked.retain(|path| path != relative);
        }
    }

    fn is_expanded(&self, relative: &Path) -> bool {
        self.expanded.iter().any(|path| path == relative)
    }

    fn toggle_expanded(&mut self, relative: &Path) {
        if self.is_expanded(relative) {
            self.expanded.retain(|path| path != relative);
        } else {
            self.expanded.push(relative.to_path_buf());
        }
    }

    /// Checked folders as absolute paths inside `root`, sorted and deduplicated,
    /// so the plan never depends on the order rows were checked.
    fn targets(&self, root: &Path) -> Vec<PathBuf> {
        let mut targets: Vec<PathBuf> = self
            .checked
            .iter()
            .map(|relative| root.join(relative))
            .collect();
        targets.sort();
        targets.dedup();
        targets
    }

    /// Replaces the listed folders. Checks and expansion state survive a reload
    /// for folders that still exist, so adding shots does not lose the selection.
    fn load(&mut self, rows: Vec<FolderRow>, note: Option<String>) {
        let exists = |relative: &PathBuf| rows.iter().any(|row| &row.relative == relative);
        let checked: Vec<PathBuf> = self
            .checked
            .iter()
            .filter(|relative| exists(relative))
            .cloned()
            .collect();
        let expanded: Vec<PathBuf> = self
            .expanded
            .iter()
            .filter(|relative| exists(relative))
            .cloned()
            .collect();
        let defaults: Vec<PathBuf> = rows
            .iter()
            .filter(|row| row.depth == 0)
            .map(|row| row.relative.clone())
            .collect();
        self.rows = rows;
        self.note = note;
        self.checked = checked;
        self.expanded = expanded;
        for relative in defaults {
            if !self.expanded.contains(&relative) {
                self.expanded.push(relative);
            }
        }
    }
}

fn walk_project(
    root: &Path,
    control: &JobControl,
    events: &JobEvents,
) -> Result<(Vec<FolderRow>, Option<String>), String> {
    let mut rows = Vec::new();
    let mut note = None;
    let mut pending = vec![(PathBuf::new(), 0)];
    while let Some((relative, depth)) = pending.pop() {
        if control.checkpoint() == crate::jobs::Checkpoint::Cancelled {
            return Err("Reading project folders was cancelled.".to_string());
        }
        let dir = root.join(&relative);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if relative.as_os_str().is_empty() => {
                return Err(format!("{} cannot be listed: {error}", dir.display()));
            }
            Err(error) => {
                note.get_or_insert_with(|| {
                    format!("Some folders could not be read: {}: {error}", dir.display())
                });
                continue;
            }
        };
        let mut children = Vec::new();
        for entry in entries {
            if control.checkpoint() == crate::jobs::Checkpoint::Cancelled {
                return Err("Reading project folders was cancelled.".to_string());
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    note.get_or_insert_with(|| {
                        format!("Some folder entries could not be read: {error}")
                    });
                    continue;
                }
            };
            match fs::symlink_metadata(entry.path()) {
                Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                    children.push(relative.join(entry.file_name()))
                }
                Ok(_) => {}
                Err(error) => {
                    note.get_or_insert_with(|| {
                        format!(
                            "Some folders could not be inspected: {}: {error}",
                            entry.path().display()
                        )
                    });
                }
            }
        }
        children.sort();
        for child in &children {
            rows.push(FolderRow {
                relative: child.clone(),
                depth,
            });
        }
        pending.extend(children.into_iter().rev().map(|child| (child, depth + 1)));
        events.progress(
            JobPhase::Scanning,
            JobProgress {
                completed: rows.len() as u64,
                current: Some(dir),
                ..JobProgress::default()
            },
        );
    }
    rows.sort_by(|a, b| a.relative.cmp(&b.relative));
    Ok((rows, note))
}

// ---------------------------------------------------------------------------
// Folder picker jobs
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickTarget {
    NewProjectParent,
    TemplateFolder,
    ClientsFile,
}

struct PickRun {
    job: JobId,
    target: PickTarget,
    replies: Receiver<Option<PathBuf>>,
}

// ---------------------------------------------------------------------------
// Template library workers
// ---------------------------------------------------------------------------

/// One unit of work against the template data area. The workspace itself never
/// reads or writes that area from a UI frame: every request runs as a tracked
/// job and answers once with exactly the data the frame needs.
#[derive(Clone, Debug)]
enum LibraryRequest {
    Load,
    Snapshot {
        id: String,
        name: String,
    },
    Create {
        name: String,
    },
    Rename {
        id: String,
        name: String,
    },
    Delete {
        id: String,
    },
    Reorder {
        id: String,
        delta: i8,
    },
    AddFolder {
        id: String,
        parent: PathBuf,
        name: String,
    },
    RenameFolder {
        id: String,
        relative: PathBuf,
        name: String,
    },
    RemoveFolder {
        id: String,
        relative: PathBuf,
    },
    SetMarkers {
        id: String,
        relative: PathBuf,
        markers: TemplateMarkers,
    },
    SaveClients {
        clients: Vec<String>,
    },
    ImportClients {
        path: PathBuf,
    },
    ImportFolder {
        path: PathBuf,
    },
}

impl LibraryRequest {
    fn title(&self) -> &'static str {
        match self {
            Self::Load => "Reading templates",
            Self::Snapshot { .. } => "Reading the template",
            Self::Create { .. } => "Creating the template",
            Self::Rename { .. } => "Renaming the template",
            Self::Delete { .. } => "Deleting the template",
            Self::Reorder { .. } => "Reordering templates",
            Self::AddFolder { .. } => "Adding a template folder",
            Self::RenameFolder { .. } => "Renaming a template folder",
            Self::RemoveFolder { .. } => "Removing a template folder",
            Self::SetMarkers { .. } => "Updating marker files",
            Self::SaveClients { .. } => "Saving the client list",
            Self::ImportClients { .. } => "Importing clients",
            Self::ImportFolder { .. } => "Importing a template folder",
        }
    }

    /// Requests whose success changes template content, so every open plan goes
    /// stale. Client-only and plain reads leave the plans alone.
    fn mutates_templates(&self) -> bool {
        !matches!(
            self,
            Self::Snapshot { .. } | Self::SaveClients { .. } | Self::ImportClients { .. }
        )
    }
}

/// Everything one request hands back. Only the fields the frame needs are set.
#[derive(Default)]
struct LibraryOutcome {
    snapshot: Option<LibrarySnapshot>,
    clients: Option<Vec<String>>,
    template: Option<StructureTemplate>,
    /// Template created or imported by this request, for the dialog's selection.
    selected: Option<String>,
    feedback: Option<String>,
}

struct LibraryJob {
    request: LibraryRequest,
    replies: Receiver<Result<LibraryOutcome, String>>,
}

/// The next free project number, keyed by the parent and preset it was read for.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NumberKey {
    parent: PathBuf,
    naming: NamingPreset,
}

struct NumberJob {
    job: JobId,
    key: NumberKey,
    replies: Receiver<Result<String, String>>,
}

fn run_library(
    library: &TemplateLibrary,
    request: LibraryRequest,
) -> Result<LibraryOutcome, String> {
    let fail = |error| describe(&error);
    let mut outcome = LibraryOutcome::default();
    match request {
        LibraryRequest::Load => {
            outcome.snapshot = Some(library.load().map_err(fail)?);
        }
        LibraryRequest::Snapshot { id, name } => {
            let summary = TemplateSummary { id, name };
            outcome.template = Some(library.snapshot(&summary).map_err(fail)?);
        }
        LibraryRequest::Create { name } => {
            library.create(&name).map_err(fail)?;
            let snapshot = library.load().map_err(fail)?;
            outcome.selected = snapshot.templates.last().map(|entry| entry.id.clone());
            outcome.feedback = Some(format!("Created template {name}."));
            outcome.snapshot = Some(snapshot);
        }
        LibraryRequest::Rename { id, name } => {
            library.rename(&id, &name).map_err(fail)?;
            let snapshot = library.load().map_err(fail)?;
            outcome.selected = Some(id);
            outcome.feedback = Some("Template renamed.".to_string());
            outcome.snapshot = Some(snapshot);
        }
        LibraryRequest::Delete { id } => {
            library.delete(&id).map_err(fail)?;
            outcome.feedback = Some("Template deleted.".to_string());
            outcome.snapshot = Some(library.load().map_err(fail)?);
        }
        LibraryRequest::Reorder { id, delta } => {
            library.reorder(&id, delta).map_err(fail)?;
            outcome.selected = Some(id);
            outcome.snapshot = Some(library.load().map_err(fail)?);
        }
        LibraryRequest::AddFolder { id, parent, name } => {
            library.add_folder(&id, &parent, &name).map_err(fail)?;
            outcome.feedback = Some(format!("Added folder {name}."));
        }
        LibraryRequest::RenameFolder { id, relative, name } => {
            library.rename_folder(&id, &relative, &name).map_err(fail)?;
            outcome.feedback = Some("Folder renamed.".to_string());
        }
        LibraryRequest::RemoveFolder { id, relative } => {
            library.remove_folder(&id, &relative).map_err(fail)?;
            outcome.feedback = Some(format!("Removed {}.", relative.display()));
        }
        LibraryRequest::SetMarkers {
            id,
            relative,
            markers,
        } => {
            library.set_markers(&id, &relative, markers).map_err(fail)?;
            outcome.feedback = Some("Marker files updated.".to_string());
        }
        LibraryRequest::SaveClients { clients } => {
            outcome.clients = Some(library.save_clients(&clients).map_err(fail)?);
            outcome.feedback = Some("Client list saved.".to_string());
        }
        LibraryRequest::ImportClients { path } => {
            outcome.clients = Some(library.import_clients(&path).map_err(fail)?);
            outcome.feedback = Some("Clients imported.".to_string());
        }
        LibraryRequest::ImportFolder { path } => {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Template".to_string());
            library.import(&path, &name).map_err(fail)?;
            let snapshot = library.load().map_err(fail)?;
            outcome.selected = snapshot.templates.last().map(|entry| entry.id.clone());
            outcome.feedback = Some("Template imported.".to_string());
            outcome.snapshot = Some(snapshot);
        }
    }
    Ok(outcome)
}

/// Case-insensitive substring first, then every typed character in order
/// (`UI/assets/app.js` `filterClients`).
fn client_matches(query: &str, client: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }
    let client = client.to_lowercase();
    if client.contains(&query) {
        return true;
    }
    let mut typed = query.chars();
    let mut wanted = typed.next();
    for found in client.chars() {
        match wanted {
            Some(character) if character == found => wanted = typed.next(),
            _ => {}
        }
        if wanted.is_none() {
            return true;
        }
    }
    wanted.is_none()
}

// ---------------------------------------------------------------------------
// Templates & clients administration
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Confirm {
    Template(String),
    Folder(PathBuf),
}

#[derive(Default)]
struct TemplateDialog {
    open: bool,
    selected: String,
    new_name: String,
    rename: String,
    folder: PathBuf,
    folder_name: String,
    folder_rename: String,
    markers_sequences: bool,
    markers_shots: bool,
    client_entry: String,
    client_rename: Option<(String, String)>,
    confirm: Option<Confirm>,
    feedback: Option<String>,
    error: Option<String>,
}

enum DialogAction {
    Close,
    CreateTemplate,
    ImportTemplate,
    ImportClients,
    RenameTemplate,
    MoveUp,
    MoveDown,
    AskDeleteTemplate,
    AskDeleteFolder,
    ConfirmDelete,
    CancelDelete,
    AddFolder,
    RenameFolder,
    SetMarkers,
    AddClient,
    RemoveClient(String),
    RenameClient(String, String),
    CancelClientRename,
}

// ---------------------------------------------------------------------------
// New project dialog
// ---------------------------------------------------------------------------

struct NewProjectDialog {
    parent: String,
    picked_parent: Option<(PathBuf, String)>,
    number: String,
    suggested_number: String,
    client: String,
    name: String,
    date: String,
    naming: NamingPreset,
    template: String,
    tree: ShotTreeEditor,
    nuke: NukeEditor,
    session: PlanSession,
    /// The partly or fully created root was handed to the catalog already.
    registered: bool,
    feedback: Option<String>,
}

impl NewProjectDialog {
    fn new(library: &LibrarySnapshot, parent: Option<PathBuf>, template: &str) -> Self {
        let date = chrono::Local::now().format("%Y-%m-%d").to_string();
        let template = if library.template(template).is_some() {
            template.to_string()
        } else {
            library
                .templates
                .first()
                .map(|entry| entry.id.clone())
                .unwrap_or_default()
        };
        Self {
            picked_parent: parent
                .as_ref()
                .map(|path| (path.clone(), path.display().to_string())),
            parent: parent
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            number: String::new(),
            suggested_number: String::new(),
            client: String::new(),
            name: String::new(),
            date,
            naming: NamingPreset::LauncherDated,
            template,
            tree: ShotTreeEditor::new(),
            nuke: NukeEditor::default(),
            session: PlanSession::default(),
            registered: false,
            feedback: None,
        }
    }

    fn parent_path(&self) -> Option<PathBuf> {
        if let Some((path, spelling)) = &self.picked_parent {
            if &self.parent == spelling {
                return Some(path.clone());
            }
        }
        let text = self.parent.trim();
        (!text.is_empty()).then(|| PathBuf::from(text))
    }

    fn draft(&self, library: u64) -> StructureDraft {
        StructureDraft {
            mode: PlanMode::NewProject,
            root: None,
            parent: self.parent_path(),
            naming: self.naming,
            number: self.number.trim().to_string(),
            client: self.client.trim().to_string(),
            name: self.name.trim().to_string(),
            date: self.date.trim().to_string(),
            template: self.template.clone(),
            destinations: Destinations::Checked,
            targets: Vec::new(),
            groups: self.tree.groups(),
            nuke: self.nuke.settings(),
            parent_as_sequence: false,
            library,
        }
    }

    fn busy(&self) -> bool {
        self.session.busy()
    }

    /// Everything the New project dialog needs before a preview is possible.
    fn plan_reason(&self, library: &LibrarySnapshot) -> Option<String> {
        if self.busy() {
            return Some(BUSY.to_string());
        }
        if self.parent.trim().is_empty() {
            return Some("Choose where the project goes first.".to_string());
        }
        if !Path::new(self.parent.trim()).is_absolute() {
            return Some("Use an absolute parent folder path.".to_string());
        }
        if self.name.trim().is_empty() {
            return Some("Give the project a name.".to_string());
        }
        if library.template(&self.template).is_none() {
            return Some(NO_TEMPLATE.to_string());
        }
        None
    }

    /// The folder the plan will create. A blank number is shown as the suggested
    /// one, because `plan_structure` auto-numbers in that case.
    fn root_name(&self) -> String {
        let name = self.name.trim();
        if name.is_empty() {
            return "—".to_string();
        }
        let number = if self.number.trim().is_empty() {
            self.suggested_number.clone()
        } else {
            self.number.trim().to_string()
        };
        match structure::project_root_name_with_date(
            self.naming,
            Some(number.as_str()).filter(|number| !number.is_empty()),
            Some(self.client.trim()).filter(|client| !client.is_empty()),
            name,
            self.date.trim(),
        ) {
            Ok(root) => root,
            Err(error) => error.message().to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// The workspace
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Scope {
    id: ProjectId,
    name: String,
    root: Option<PathBuf>,
}

/// Shots & Sequences plus the New project dialog: one instance per application.
pub struct StructureWorkspace {
    ctx: egui::Context,
    library: TemplateLibrary,
    snapshot: LibrarySnapshot,
    library_error: Option<String>,
    catalog: Option<CatalogClient>,
    user: Option<UserId>,
    scope: Option<Scope>,
    tree: FolderTree,
    walk: Option<(
        JobId,
        Receiver<Result<(Vec<FolderRow>, Option<String>), String>>,
    )>,
    template: String,
    /// Template id and library revision the destination snapshot was read for.
    loaded_template: Option<(String, u64)>,
    template_snapshot: Option<StructureTemplate>,
    template_error: Option<String>,
    /// The template the administration dialog is showing, loaded independently of
    /// the destination template the plan uses.
    dialog_template: Option<StructureTemplate>,
    dialog_loaded: Option<(String, u64)>,
    /// Bumped by every successful library request that changes template content.
    library_revision: u64,
    /// First snapshot read: the constructor never touches the data area.
    reload_requested: bool,
    /// Mutations and the full snapshot read; one at a time.
    library_job: Option<LibraryJob>,
    /// One template folder read, separate so a mutation never waits on it.
    template_job: Option<LibraryJob>,
    number_job: Option<NumberJob>,
    /// Parent and preset the user committed, so typing a path never reads a
    /// directory per keystroke.
    number_request: Option<NumberKey>,
    destinations: Destinations,
    tree_editor: ShotTreeEditor,
    nuke: NukeEditor,
    parent_as_sequence: bool,
    session: PlanSession,
    pick: Option<PickRun>,
    last_pick: Option<PathBuf>,
    dialog: TemplateDialog,
    new_project: Option<NewProjectDialog>,
}

impl StructureWorkspace {
    /// Cheap by construction: nothing here touches the data area. The first
    /// snapshot read is a tracked job started by the first `poll`. A missing
    /// registry is seeded with the built-in templates by that read itself
    /// (`TemplateLibrary::load`).
    pub fn new(ctx: &egui::Context, data_dir: &Path) -> Self {
        Self {
            ctx: ctx.clone(),
            library: TemplateLibrary::new(data_dir),
            snapshot: LibrarySnapshot::default(),
            library_error: None,
            catalog: None,
            user: None,
            scope: None,
            tree: FolderTree::default(),
            walk: None,
            template: String::new(),
            loaded_template: None,
            template_snapshot: None,
            template_error: None,
            dialog_template: None,
            dialog_loaded: None,
            library_revision: 0,
            reload_requested: true,
            library_job: None,
            template_job: None,
            number_job: None,
            number_request: None,
            destinations: Destinations::default(),
            tree_editor: ShotTreeEditor::new(),
            nuke: NukeEditor::default(),
            parent_as_sequence: false,
            session: PlanSession::default(),
            pick: None,
            last_pick: None,
            dialog: TemplateDialog::default(),
            new_project: None,
        }
    }

    /// Keeps both template selections inside a snapshot the library just returned.
    fn settle_template_selection(&mut self) {
        if self.snapshot.template(&self.template).is_none() {
            self.template = self
                .snapshot
                .templates
                .first()
                .map(|entry| entry.id.clone())
                .unwrap_or_default();
            self.loaded_template = None;
        }
        if self.snapshot.template(&self.dialog.selected).is_none() {
            self.dialog.selected = self.template.clone();
            self.dialog.folder = PathBuf::new();
            self.dialog_loaded = None;
        }
        if let Some(dialog) = self.new_project.as_mut() {
            if self.snapshot.template(&dialog.template).is_none() && !dialog.busy() {
                dialog.template = self
                    .snapshot
                    .templates
                    .first()
                    .map(|entry| entry.id.clone())
                    .unwrap_or_default();
                dialog.session.clear();
            }
        }
    }

    // ------------------------------------------------------------ coordination

    /// Drains workers and keeps both surfaces bound to the current selection.
    pub fn poll(&mut self, workspace: &mut ProjectWorkspace, jobs: &mut Jobs) {
        self.catalog = workspace.catalog_client();
        self.user = workspace.user().map(|user| user.id);
        if self.reload_requested && self.library_job.is_none() && !jobs.is_shutting_down() {
            self.reload_requested = false;
            match self.start_library_job(jobs, LibraryRequest::Load) {
                Ok(job) => self.library_job = Some(job),
                Err(error) => {
                    self.library_error = Some(error);
                    self.reload_requested = true;
                }
            }
        }
        self.poll_library();

        let scope = workspace.selected_project().map(|project| Scope {
            id: project.id,
            name: project.name.clone(),
            root: project.root.clone(),
        });
        if scope != self.scope {
            self.scope = scope;
            self.tree.clear();
            self.walk = None;
            self.tree_editor.clear();
            // A running plan keeps its own state; its review is invalidated by the
            // draft comparison below as soon as the new selection is settled.
            if !self.session.busy() {
                self.session.clear();
            }
            self.begin_walk(jobs);
        }
        self.poll_pick(jobs);
        self.poll_walk();
        self.ensure_template(jobs);
        self.ensure_dialog_template(jobs);
        self.ensure_number(jobs);
        self.poll_number();
        self.session.poll();
        if let Some(dialog) = self.new_project.as_mut() {
            dialog.session.poll();
        }
        if let Some(completion) = self.session.take_completion() {
            if completion.created > 0 || completion.failed > 0 {
                self.begin_walk(jobs);
            }
        }
        let completion = self
            .new_project
            .as_mut()
            .and_then(|dialog| dialog.session.take_completion());
        if let Some(completion) = completion {
            let finished = completion.succeeded && completion.failed == 0 && completion.created > 0;
            if let Some(dialog) = self.new_project.as_mut() {
                // A partial run still created the root, so the catalog gets it and
                // the dialog keeps the report instead of claiming success.
                if completion.created > 0 && !dialog.registered {
                    dialog.registered = true;
                    workspace.register_created_project(completion.root.clone());
                }
                if !finished {
                    dialog.feedback = Some(if completion.created > 0 {
                        "Only part of the project was created. The report lists every failure, \
                         and the folder is registered so it can be repaired from Projects."
                            .to_string()
                    } else {
                        "The project was not created; inspect the report before retrying."
                            .to_string()
                    });
                }
            }
            if finished {
                self.new_project = None;
            }
        }

        let draft = self.add_draft();
        self.session.invalidate(&draft);
        let revision = self.library_revision;
        if let Some(dialog) = self.new_project.as_mut() {
            dialog.session.invalidate(&dialog.draft(revision));
        }
    }

    // ------------------------------------------------------- template library

    fn library_busy(&self) -> bool {
        self.library_job.is_some() || self.template_job.is_some()
    }

    /// Why the library controls are inert, if they are.
    fn library_reason(&self) -> &'static str {
        if self.library_busy() {
            "Wait for the template library to finish working."
        } else if self.pick.is_some() {
            "Close the folder chooser first."
        } else if self.session.busy()
            || self
                .new_project
                .as_ref()
                .is_some_and(NewProjectDialog::busy)
        {
            "Wait for the current plan to finish."
        } else {
            ""
        }
    }

    /// Starts one library request as a tracked job. The worker owns every read and
    /// write; the reply carries exactly what the frame needs.
    fn start_library_job(
        &mut self,
        jobs: &mut Jobs,
        request: LibraryRequest,
    ) -> Result<LibraryJob, String> {
        if jobs.is_shutting_down() {
            return Err("The application is closing.".to_string());
        }
        let library = self.library.clone();
        let title = request.title();
        let work = request.clone();
        let (tx, rx) = channel();
        jobs.start(&self.ctx, title, move |context| {
            let JobContext {
                control, events, ..
            } = context;
            let result = if control.checkpoint() == crate::jobs::Checkpoint::Cancelled {
                Err("The template library request was cancelled.".to_string())
            } else {
                run_library(&library, work)
            };
            let outcome = if control.is_cancelled() {
                JobOutcome::Cancelled
            } else if result.is_ok() {
                JobOutcome::Succeeded
            } else {
                JobOutcome::Failed
            };
            if let Err(message) = &result {
                events.diagnostic(JobPhase::Scanning, message.clone());
            }
            let _ = tx.send(result);
            events.finish(outcome, JobProgress::default());
        })
        .map_err(|error| format!("The template request could not be started: {error}"))?;
        Ok(LibraryJob {
            request,
            replies: rx,
        })
    }

    fn request_library(&mut self, jobs: &mut Jobs, request: LibraryRequest) {
        if self.library_busy() {
            self.dialog.error = Some(BUSY.to_string());
            return;
        }
        match self.start_library_job(jobs, request) {
            Ok(job) => self.library_job = Some(job),
            Err(error) => self.dialog.error = Some(error),
        }
    }

    fn poll_library(&mut self) {
        if let Some(job) = self.library_job.take() {
            match job.replies.try_recv() {
                Ok(result) => {
                    let request = job.request.clone();
                    self.apply_library(request, result);
                }
                Err(TryRecvError::Empty) => self.library_job = Some(job),
                Err(TryRecvError::Disconnected) => {
                    self.dialog.error =
                        Some("The template library stopped without a result.".to_string());
                    self.loaded_template = None;
                    self.dialog_loaded = None;
                }
            }
        }
        if let Some(job) = self.template_job.take() {
            match job.replies.try_recv() {
                Ok(result) => self.apply_template_read(&job.request, result),
                Err(TryRecvError::Empty) => self.template_job = Some(job),
                Err(TryRecvError::Disconnected) => {
                    self.template_error =
                        Some("Reading the template stopped without a result.".to_string());
                }
            }
        }
    }

    fn apply_library(&mut self, request: LibraryRequest, result: Result<LibraryOutcome, String>) {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(message) => {
                match &request {
                    LibraryRequest::Load => self.library_error = Some(message),
                    _ => self.dialog.error = Some(message),
                }
                return;
            }
        };
        self.library_error = None;
        self.dialog.error = None;
        if let Some(snapshot) = outcome.snapshot {
            self.snapshot = snapshot;
            self.settle_template_selection();
        }
        if let Some(clients) = outcome.clients {
            self.snapshot.clients = clients;
        }
        if let Some(selected) = outcome.selected {
            self.dialog.selected = selected;
            self.dialog.confirm = None;
            self.dialog_loaded = None;
        }
        if let Some(feedback) = outcome.feedback {
            self.dialog.feedback = Some(feedback);
        }
        match &request {
            LibraryRequest::Create { .. } => self.dialog.new_name.clear(),
            LibraryRequest::Rename { .. } => self.dialog.rename.clear(),
            LibraryRequest::AddFolder { .. } => self.dialog.folder_name.clear(),
            LibraryRequest::SaveClients { .. } => {
                self.dialog.client_entry.clear();
                self.dialog.client_rename = None;
            }
            LibraryRequest::Delete { .. } => {
                self.dialog.folder = PathBuf::new();
                self.dialog.confirm = None;
                self.settle_template_selection();
            }
            LibraryRequest::RenameFolder { relative, name, .. } => {
                self.dialog.folder = match relative.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => parent.join(name),
                    _ => PathBuf::from(name),
                };
                self.dialog.folder_rename.clear();
            }
            LibraryRequest::RemoveFolder { .. } => {
                self.dialog.folder = PathBuf::new();
                self.dialog.confirm = None;
            }
            _ => {}
        }
        // Any content change makes every template snapshot and open plan stale.
        if request.mutates_templates() {
            self.library_revision += 1;
            self.loaded_template = None;
            self.dialog_loaded = None;
        }
    }

    fn apply_template_read(
        &mut self,
        request: &LibraryRequest,
        result: Result<LibraryOutcome, String>,
    ) {
        let LibraryRequest::Snapshot { id, .. } = request else {
            return;
        };
        match result {
            Ok(outcome) => {
                let Some(template) = outcome.template else {
                    return;
                };
                if self.template == *id {
                    self.template_snapshot = Some(template.clone());
                    self.template_error = None;
                }
                if self.dialog.open && self.dialog.selected == *id {
                    self.dialog_template = Some(template);
                }
            }
            Err(message) => {
                if self.template == *id {
                    self.template_snapshot = None;
                    self.template_error = Some(message.clone());
                }
                if self.dialog.open && self.dialog.selected == *id {
                    self.dialog_template = None;
                    self.dialog.error = Some(message);
                }
            }
        }
    }

    // ----------------------------------------------------- next project number

    fn number_key(&self) -> Option<NumberKey> {
        let dialog = self.new_project.as_ref()?;
        Some(NumberKey {
            parent: dialog.parent_path()?,
            naming: dialog.naming,
        })
    }

    /// Reads the next free number on a worker for the parent and preset the user
    /// committed. A reply is only acknowledged for that same pair.
    fn ensure_number(&mut self, jobs: &mut Jobs) {
        let Some(requested) = self.number_request.clone() else {
            return;
        };
        if self.number_key().as_ref() != Some(&requested) {
            if let Some(job) = self.number_job.take() {
                jobs.cancel(job.job);
            }
            self.number_request = None;
            return;
        }
        if self
            .number_job
            .as_ref()
            .is_some_and(|job| job.key == requested)
        {
            return;
        }
        if let Some(job) = self.number_job.take() {
            jobs.cancel(job.job);
        }
        if jobs.is_shutting_down() {
            return;
        }
        let start = requested.parent.clone();
        let naming = requested.naming;
        let (tx, rx) = channel();
        let started = jobs.start(
            &self.ctx,
            "Reading the next project number",
            move |context| {
                let result = match naming {
                    NamingPreset::LauncherDated => structure::next_launcher_code(&start),
                    NamingPreset::FolderStructurer => {
                        structure::next_folder_structurer_number(&start)
                    }
                }
                .map_err(|error| describe(&error));
                let _ = tx.send(result);
                context
                    .events
                    .finish(JobOutcome::Succeeded, JobProgress::default());
            },
        );
        if let Ok(job) = started {
            self.number_job = Some(NumberJob {
                job,
                key: requested,
                replies: rx,
            });
        }
    }

    fn poll_number(&mut self) {
        let Some(job) = self.number_job.take() else {
            return;
        };
        match job.replies.try_recv() {
            Ok(result) => {
                if self.number_key().as_ref() != Some(&job.key) {
                    return;
                }
                if self.number_request.as_ref() == Some(&job.key) {
                    self.number_request = None;
                }
                let Some(dialog) = self.new_project.as_mut() else {
                    return;
                };
                match result {
                    Ok(number) => {
                        if dialog.number.trim().is_empty()
                            || dialog.number == dialog.suggested_number
                        {
                            dialog.number = number.clone();
                        }
                        dialog.suggested_number = number;
                    }
                    Err(message) => {
                        dialog.suggested_number.clear();
                        dialog.feedback = Some(message);
                    }
                }
            }
            Err(TryRecvError::Empty) => self.number_job = Some(job),
            Err(TryRecvError::Disconnected) => {
                if self.number_request.as_ref() == Some(&job.key) {
                    self.number_request = None;
                }
            }
        }
    }

    fn poll_pick(&mut self, jobs: &mut Jobs) {
        let Some(run) = self.pick.take() else {
            return;
        };
        let reply = match run.replies.try_recv() {
            Ok(reply) => reply,
            Err(TryRecvError::Empty) => {
                self.pick = Some(run);
                return;
            }
            Err(TryRecvError::Disconnected) => {
                self.dialog.error =
                    Some("The folder chooser stopped without a result.".to_string());
                return;
            }
        };
        let Some(path) = reply else {
            return;
        };
        // Remember a starting folder, never a chosen file.
        self.last_pick = match (run.target, &path) {
            (PickTarget::ClientsFile, path) => path.parent().map(Path::to_path_buf),
            (_, path) => Some(path.clone()),
        };
        match run.target {
            PickTarget::NewProjectParent => {
                if let Some(dialog) = self.new_project.as_mut() {
                    dialog.parent = path.display().to_string();
                    dialog.picked_parent = Some((path, dialog.parent.clone()));
                }
                self.number_request = self.number_key();
            }
            // The library does the filesystem work, not the chooser worker.
            PickTarget::TemplateFolder => {
                self.request_library(jobs, LibraryRequest::ImportFolder { path })
            }
            PickTarget::ClientsFile => {
                self.request_library(jobs, LibraryRequest::ImportClients { path })
            }
        }
    }

    /// Opens the native chooser on a tracked job. The picker only returns the
    /// chosen path; the workspace decides what to do with it.
    fn begin_pick(&mut self, jobs: &mut Jobs, target: PickTarget) {
        if self.pick.is_some() || jobs.is_shutting_down() {
            return;
        }
        let title = match target {
            PickTarget::NewProjectParent => "Choose where the project goes",
            PickTarget::TemplateFolder => "Choose the folder to import",
            PickTarget::ClientsFile => "Choose a clients file",
        };
        let start = self.last_pick.clone();
        let (tx, rx) = channel();
        let started = jobs
            .start(&self.ctx, title, move |context| {
                let mut dialog = rfd::FileDialog::new().set_title(title);
                if let Some(start) = start {
                    dialog = dialog.set_directory(start);
                }
                let picked = match target {
                    PickTarget::ClientsFile => {
                        dialog.add_filter("Clients file", &["json"]).pick_file()
                    }
                    _ => dialog.pick_folder(),
                };
                let _ = tx.send(picked);
                context
                    .events
                    .finish(JobOutcome::Succeeded, JobProgress::default());
            })
            .map_err(|error| format!("The chooser could not be opened: {error}"));
        match started {
            Ok(job) => {
                self.pick = Some(PickRun {
                    job,
                    target,
                    replies: rx,
                })
            }
            Err(error) => self.dialog.error = Some(error),
        }
    }

    fn begin_walk(&mut self, jobs: &mut Jobs) {
        let Some(root) = self.scope.as_ref().and_then(|scope| scope.root.clone()) else {
            self.tree.clear();
            return;
        };
        if jobs.is_shutting_down() || self.walk.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let started = jobs.start(&self.ctx, "Reading project folders", move |context| {
            let result = walk_project(&root, &context.control, &context.events);
            let outcome = match &result {
                _ if context.control.is_cancelled() => JobOutcome::Cancelled,
                Ok((_, Some(_))) => JobOutcome::Partial,
                Ok(_) => JobOutcome::Succeeded,
                Err(_) => JobOutcome::Failed,
            };
            let _ = tx.send(result);
            context.events.finish(outcome, JobProgress::default());
        });
        match started {
            Ok(job) => self.walk = Some((job, rx)),
            Err(error) => {
                self.tree.note = Some(format!("The project folders could not be read: {error}"))
            }
        }
    }

    fn poll_walk(&mut self) {
        let Some((_, replies)) = self.walk.as_ref() else {
            return;
        };
        let result = match replies.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.walk = None;
                self.tree.note = Some("Reading the project folders stopped.".to_string());
                return;
            }
        };
        self.walk = None;
        match result {
            Ok((rows, note)) => self.tree.load(rows, note),
            Err(message) => {
                self.tree.clear();
                self.tree.note = Some(message);
            }
        }
    }

    /// Loads the selected template's snapshot once per selection and revision.
    fn ensure_template(&mut self, jobs: &mut Jobs) {
        let key = (self.template.clone(), self.library_revision);
        if self.loaded_template.as_ref() == Some(&key) || self.template_job.is_some() {
            return;
        }
        let Some(summary) = self.snapshot.template(&self.template).cloned() else {
            self.loaded_template = Some(key);
            self.template_snapshot = None;
            self.template_error = None;
            return;
        };
        if self.loaded_template.as_ref().map(|(id, _)| id) != Some(&summary.id) {
            self.template_snapshot = None;
            self.template_error = None;
        }
        self.loaded_template = Some(key);
        let request = LibraryRequest::Snapshot {
            id: summary.id,
            name: summary.name,
        };
        match self.start_library_job(jobs, request) {
            Ok(job) => self.template_job = Some(job),
            Err(error) => self.template_error = Some(error),
        }
    }

    /// Loads the template the administration dialog is editing, once per selection
    /// and revision. Reuses the destination read when both point at one template.
    fn ensure_dialog_template(&mut self, jobs: &mut Jobs) {
        if !self.dialog.open || self.template_job.is_some() {
            return;
        }
        let key = (self.dialog.selected.clone(), self.library_revision);
        if self.dialog_loaded.as_ref() == Some(&key) {
            return;
        }
        if self.template == self.dialog.selected
            && self.loaded_template.as_ref() == Some(&key)
            && self.template_snapshot.is_some()
        {
            self.dialog_template = self.template_snapshot.clone();
            self.dialog_loaded = Some(key);
            return;
        }
        let Some(summary) = self.snapshot.template(&self.dialog.selected).cloned() else {
            self.dialog_loaded = Some(key);
            self.dialog_template = None;
            return;
        };
        if self.dialog_loaded.as_ref().map(|(id, _)| id) != Some(&summary.id) {
            self.dialog_template = None;
        }
        self.dialog_loaded = Some(key);
        let request = LibraryRequest::Snapshot {
            id: summary.id,
            name: summary.name,
        };
        match self.start_library_job(jobs, request) {
            Ok(job) => self.template_job = Some(job),
            Err(error) => self.dialog.error = Some(error),
        }
    }

    // ----------------------------------------------------------------- drafts

    fn add_draft(&self) -> StructureDraft {
        let targets = match self.destinations {
            Destinations::Checked => self
                .scope
                .as_ref()
                .and_then(|scope| scope.root.as_deref())
                .map(|root| self.tree.targets(root))
                .unwrap_or_default(),
            Destinations::Template => Vec::new(),
        };
        StructureDraft {
            mode: PlanMode::AddToProject,
            root: self.scope.as_ref().and_then(|scope| scope.root.clone()),
            parent: None,
            naming: NamingPreset::FolderStructurer,
            number: String::new(),
            client: String::new(),
            name: String::new(),
            date: String::new(),
            template: self.template.clone(),
            destinations: self.destinations.clone(),
            targets,
            groups: self.tree_editor.groups(),
            nuke: self.nuke.settings(),
            parent_as_sequence: self.parent_as_sequence,
            library: self.library_revision,
        }
    }

    fn busy(&self) -> bool {
        self.pick.is_some()
            || self.library_busy()
            || self.session.busy()
            || self
                .new_project
                .as_ref()
                .is_some_and(NewProjectDialog::busy)
    }

    pub fn is_picker_job(&self, id: JobId) -> bool {
        self.pick.as_ref().is_some_and(|run| run.job == id)
    }

    pub fn picker_pending(&self) -> bool {
        self.pick.is_some()
    }

    // ------------------------------------------------------------ shared state

    fn live_reason(&self) -> Option<String> {
        if self.catalog.is_none() {
            return Some(NO_CATALOG.to_string());
        }
        if self.user.is_none() {
            return Some(NO_USER.to_string());
        }
        None
    }

    fn plan_reason(&self) -> Option<String> {
        if let Some(reason) = self.live_reason() {
            return Some(reason);
        }
        if self.busy() {
            return Some(BUSY.to_string());
        }
        let Some(scope) = &self.scope else {
            return Some(NO_PROJECT.to_string());
        };
        if scope.root.is_none() {
            return Some("This project record has no readable folder.".to_string());
        }
        if self.destinations == Destinations::Template && self.template_snapshot.is_none() {
            return Some(match &self.template_error {
                Some(error) => error.clone(),
                None => NO_TEMPLATE.to_string(),
            });
        }
        if self.tree_editor.is_empty() {
            return Some("Add a sequence or a standalone shot first.".to_string());
        }
        if self.destinations == Destinations::Checked && self.tree.checked.is_empty() {
            return Some("Check at least one existing folder first.".to_string());
        }
        None
    }

    fn build_preview(&mut self, jobs: &mut Jobs) -> Result<(), String> {
        if let Some(reason) = self.plan_reason() {
            return Err(reason);
        }
        // Checked-folder plans never read the template, so an unreadable template
        // must not block the flow the user is actually using.
        let template = match (&self.template_snapshot, &self.destinations) {
            (Some(template), _) => template.clone(),
            (None, Destinations::Checked) => StructureTemplate::new(String::new(), String::new()),
            (None, Destinations::Template) => {
                return Err(match &self.template_error {
                    Some(error) => error.clone(),
                    None => NO_TEMPLATE.to_string(),
                });
            }
        };
        let draft = self.add_draft();
        self.session
            .begin_preview(&self.ctx, jobs, draft, move |draft, revision| {
                plan_add(draft, &template, revision)
            })
    }

    fn validate_blocked(&self) -> Option<String> {
        if let Some(reason) = self.live_reason() {
            return Some(reason);
        }
        if self.busy() {
            return Some(BUSY.to_string());
        }
        let Some(preview) = &self.session.preview else {
            return Some("Regenerate the preview first.".to_string());
        };
        if preview.draft != self.add_draft() {
            return Some("The additions changed; regenerate the preview.".to_string());
        }
        None
    }

    fn execute_blocked(&self) -> Option<String> {
        if let Some(reason) = self.live_reason() {
            return Some(reason);
        }
        if self.busy() {
            return Some(BUSY.to_string());
        }
        let Some(review) = &self.session.review else {
            return Some("Check the additions first.".to_string());
        };
        if review.draft != self.add_draft() {
            return Some("The additions changed; check them again.".to_string());
        }
        if review.ready.is_none() {
            return Some(
                review
                    .refusal
                    .clone()
                    .unwrap_or_else(|| "Resolve the blocked items first.".to_string()),
            );
        }
        None
    }

    fn begin_validate(&mut self, jobs: &mut Jobs) {
        if let Some(reason) = self.validate_blocked() {
            self.session.message = Some(reason);
            return;
        }
        let Some(preview) = self.session.preview.clone() else {
            return;
        };
        if let Err(message) = self.session.begin_validate(
            &self.ctx,
            jobs,
            preview.draft.clone(),
            preview.plan.clone(),
            preview.revision,
            preview.project_root.clone(),
        ) {
            self.session.message = Some(message);
        }
    }

    fn begin_execute(&mut self, jobs: &mut Jobs) {
        if let Some(reason) = self.execute_blocked() {
            self.session.message = Some(reason);
            return;
        }
        let (Some(catalog), Some(user)) = (self.catalog.clone(), self.user) else {
            self.session.message = Some(NO_CATALOG.to_string());
            return;
        };
        if let Err(message) =
            self.session
                .begin_execute(&self.ctx, jobs, catalog, user, "Adding folders and scripts")
        {
            self.session.message = Some(message);
        }
    }

    pub fn status_line(&self) -> String {
        if let Some(reason) = self.live_reason() {
            return reason;
        }
        if let Some(job) = &self.library_job {
            return format!("{}…", job.request.title());
        }
        if self.template_job.is_some() {
            return "Reading the template…".to_string();
        }
        if self.walk.is_some() {
            return "Reading project folders…".to_string();
        }
        if self.session.validate.is_some() {
            return "Checking the additions…".to_string();
        }
        if self.session.run.is_some() {
            return "Adding folders and scripts…".to_string();
        }
        if let Some(review) = &self.session.review {
            return review.summary();
        }
        if let Some(preview) = &self.session.preview {
            let targets = match self.destinations {
                Destinations::Checked => format!("{} checked folder(s)", self.tree.checked.len()),
                Destinations::Template => "template folders".to_string(),
            };
            return format!(
                "Preview {} · {} folder(s), {} script(s) · {targets}",
                preview.revision, preview.folders, preview.scripts
            );
        }
        match self.scope {
            Some(_) => "Draft · regenerate the preview after editing".to_string(),
            None => NO_PROJECT.to_string(),
        }
    }

    // ------------------------------------------------------------------ view

    /// Draws the whole Shots & Sequences page.
    pub fn view(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        let enabled = !self.busy() && self.scope.is_some();
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Project").small().weak());
            let name = self
                .scope
                .as_ref()
                .map(|scope| scope.name.clone())
                .unwrap_or_else(|| "No project selected".to_string());
            ui.label(name);
            if let Some(note) = &self.tree.note {
                ui.separator();
                ui.label(egui::RichText::new(note).small().color(WARNING));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!self.busy(), egui::Button::new("Templates & clients…"))
                    .on_hover_text("Folder templates and client names for both flows.")
                    .clicked()
                {
                    self.dialog.open = true;
                }
            });
        });
        if !self.snapshot.warnings.is_empty() {
            notices(
                ui,
                "Library notices",
                "structure_library",
                &self.snapshot.warnings,
            );
        }
        ui.separator();

        egui::Panel::bottom("structure_preview_dock")
            .resizable(true)
            .default_size(240.0)
            .min_size(150.0)
            .max_size(460.0)
            .show(ui, |ui| self.preview_dock(ui, jobs));
        egui::Panel::left("structure_destination_tree")
            .resizable(true)
            .default_size(300.0)
            .min_size(200.0)
            .max_size(600.0)
            .show(ui, |ui| self.folder_panel(ui, enabled, jobs));
        egui::CentralPanel::default().show(ui, |ui| self.editor_panel(ui, enabled));
    }

    fn folder_panel(&mut self, ui: &mut egui::Ui, enabled: bool, jobs: &mut Jobs) {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Existing project folders")
                    .small()
                    .weak(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(enabled && self.walk.is_none(), egui::Button::new("Reload"))
                    .clicked()
                {
                    self.begin_walk(jobs);
                }
            });
        });
        ui.separator();
        if self
            .scope
            .as_ref()
            .and_then(|scope| scope.root.as_ref())
            .is_none()
        {
            empty_state(
                ui,
                "No project folder.",
                "Choose a project in Projects, then edit its structure.",
            );
            return;
        }
        let template_mode = self.destinations == Destinations::Template;
        if template_mode {
            ui.label(
                egui::RichText::new("Additions go into the template's marker folders.")
                    .small()
                    .weak(),
            );
        }
        ui.label(
            egui::RichText::new(format!("{} checked", self.tree.checked.len()))
                .small()
                .weak(),
        );
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.tree.rows.is_empty() {
                    empty_state(
                        ui,
                        "No folders.",
                        "This project folder has no subfolders to add into.",
                    );
                    return;
                }
                let mut covered_below: Option<usize> = None;
                for index in 0..self.tree.rows.len() {
                    let row = self.tree.rows[index].clone();
                    if let Some(depth) = covered_below {
                        if row.depth > depth {
                            continue;
                        }
                        covered_below = None;
                    }
                    ui.push_id(("folder", &row.relative), |ui| {
                        ui.horizontal(|ui| {
                            ui.add_space(row.depth as f32 * 12.0);
                            let expanded = self.tree.is_expanded(&row.relative);
                            let has_children = self
                                .tree
                                .rows
                                .get(index + 1)
                                .is_some_and(|next| next.depth > row.depth);
                            if has_children {
                                if ui
                                    .small_button(if expanded { "▾" } else { "▸" })
                                    .on_hover_text("Show or hide the folders inside")
                                    .clicked()
                                {
                                    self.tree.toggle_expanded(&row.relative);
                                }
                            } else {
                                ui.add_space(16.0);
                            }
                            let mut checked = self.tree.is_checked(&row.relative);
                            let response = ui.add_enabled(
                                enabled && !template_mode,
                                egui::Checkbox::new(
                                    &mut checked,
                                    format!("{}/", file_label(&row.relative)),
                                ),
                            );
                            if response.changed() {
                                self.tree.toggle(&row.relative, checked);
                            }
                        });
                    });
                    if !self.tree.is_expanded(&row.relative) {
                        covered_below = Some(row.depth);
                    }
                }
            });
    }

    fn editor_panel(&mut self, ui: &mut egui::Ui, enabled: bool) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Sequences and shots").small().weak());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(enabled, egui::Button::new("Clear"))
                    .on_hover_text("Remove every sequence and shot row.")
                    .clicked()
                {
                    self.tree_editor.clear();
                    self.session.invalidate(&self.add_draft());
                }
            });
        });
        ui.separator();
        let before = self.add_draft();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.tree_editor.ui(ui, enabled);
            });
        if self.add_draft() != before {
            self.session.invalidate(&self.add_draft());
        }
    }

    fn preview_dock(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Destination preview").small().weak());
            if let Some(preview) = &self.session.preview {
                ui.label(
                    egui::RichText::new(format!(
                        "{} folder(s) · {} script(s)",
                        preview.folders, preview.scripts
                    ))
                    .small(),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let blocked = self.plan_reason();
                let button = accent_button(ui, "Regenerate preview", blocked.is_none());
                if button.clicked() {
                    if let Err(message) = self.build_preview(jobs) {
                        self.session.message = Some(message);
                    }
                }
                if let Some(reason) = &blocked {
                    explain(&button, reason);
                }
            });
        });
        ui.separator();

        let enabled = !self.busy();
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("Destination").small().weak());
            let selected = match self.destinations {
                Destinations::Checked => "Checked folders".to_string(),
                Destinations::Template => self
                    .snapshot
                    .name_of(&self.template)
                    .map(str::to_string)
                    .unwrap_or_else(|| "Template folders".to_string()),
            };
            let mut chosen = self.destinations.clone();
            egui::ComboBox::from_id_salt("structure_destination")
                .selected_text(selected)
                .width(190.0)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(
                            matches!(chosen, Destinations::Checked),
                            "Checked folders",
                        )
                        .clicked()
                    {
                        chosen = Destinations::Checked;
                    }
                    for entry in self.snapshot.templates.clone() {
                        let active =
                            matches!(chosen, Destinations::Template) && self.template == entry.id;
                        if ui.selectable_label(active, &entry.name).clicked() {
                            chosen = Destinations::Template;
                            self.template = entry.id.clone();
                            self.loaded_template = None;
                        }
                    }
                });
            if chosen != self.destinations {
                self.destinations = chosen;
            }
            if self.destinations == Destinations::Template {
                ui.label(
                    egui::RichText::new("template marker folders")
                        .small()
                        .weak(),
                );
            }
        });

        ui.separator();
        if self
            .nuke
            .ui(ui, enabled, Some(&mut self.parent_as_sequence))
        {
            self.session.invalidate(&self.add_draft());
        }
        ui.separator();

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(message) = &self.session.message {
                    notice(ui, message, false);
                }
                if let Some(report) = &self.session.report {
                    ui.label(egui::RichText::new(&report.headline).small());
                    if let Some(error) = &report.error {
                        notice(ui, error, true);
                    }
                    notices(ui, "Files and failures", "structure_run", &report.lines);
                    if !report.inspection.is_empty() {
                        ui.collapsing("Journal record", |ui| {
                            for line in &report.inspection {
                                ui.label(egui::RichText::new(line).small().monospace());
                            }
                        });
                    }
                }
                if let Some(review) = &self.session.review {
                    ui.label(egui::RichText::new(review.summary()).small());
                    if let Some(refusal) = &review.refusal {
                        notice(ui, refusal, true);
                    }
                    for warning in &review.warnings {
                        notice(ui, warning, false);
                    }
                }
                match (&self.session.review, &self.session.preview) {
                    (Some(review), _) => review_rows(ui, &review.rows),
                    (None, Some(preview)) => {
                        for warning in &preview.warnings {
                            notice(ui, warning, false);
                        }
                        preview_rows(ui, preview, None);
                    }
                    (None, None) => empty_state(
                        ui,
                        "No preview yet.",
                        "Check the folders and add sequences or shots, then regenerate.",
                    ),
                }
            });
    }

    /// The bottom strip's review control.
    pub fn validate_control(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        let blocked = self.validate_blocked();
        let button = ui.add_enabled(blocked.is_none(), egui::Button::new("Check additions"));
        if button.clicked() {
            self.begin_validate(jobs);
        }
        if let Some(reason) = &blocked {
            explain(&button, reason);
        }
    }

    /// The bottom strip's execution control.
    pub fn execution_control(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        let blocked = self.execute_blocked();
        let button = accent_button(ui, "Add to project", blocked.is_none());
        if button.clicked() {
            self.begin_execute(jobs);
        }
        if let Some(reason) = &blocked {
            explain(&button, reason);
        }
    }

    // ------------------------------------------------------------ new project

    pub fn begin_new_project(&mut self, workspace: &ProjectWorkspace) {
        if self.library_error.is_some() {
            // Retried by the next poll; nothing here touches the data area.
            self.reload_requested = true;
        }
        let parent = workspace
            .selected_project()
            .and_then(|project| project.root.as_ref())
            .and_then(|root| root.parent().map(Path::to_path_buf));
        let dialog = NewProjectDialog::new(&self.snapshot, parent, &self.template);
        self.new_project = Some(dialog);
        self.number_request = self.number_key();
    }

    pub fn new_project_dialog(
        &mut self,
        ctx: &egui::Context,
        _workspace: &mut ProjectWorkspace,
        jobs: &mut Jobs,
    ) {
        let Some(mut dialog) = self.new_project.take() else {
            return;
        };
        // Escape closes the dialog unless a field is capturing input, so the key
        // never eats a keystroke the user meant for a name.
        if ctx.input(|input| input.key_pressed(egui::Key::Escape))
            && ctx.memory(|memory| memory.focused().is_none())
            && !dialog.busy()
        {
            return;
        }
        let templates = self.snapshot.templates.clone();
        let clients = self.snapshot.clients.clone();
        let library_error = self.library_error.clone();
        let warnings = self.snapshot.warnings.clone();
        // Any library worker, the folder chooser, or the page's own plan keeps the
        // whole form inert: templates cannot change under a running plan.
        let library_held = !self.library_reason().is_empty();
        let number_pending = self.number_request.is_some();
        let revision = self.library_revision;
        let mut cancel = false;
        let mut open = true;
        let mut ask_number = false;
        egui::Window::new("New project")
            .collapsible(false)
            .resizable(true)
            .default_size([860.0, 720.0])
            .min_width(640.0)
            .open(&mut open)
            .show(ctx, |ui| {
                if let Some(error) = &library_error {
                    notice(ui, error, true);
                }
                notices(ui, "Library notices", "new_project_library", &warnings);
                if library_held {
                    notice(ui, self.library_reason(), false);
                }
                let editing = !dialog.busy() && !library_held;
                let library_blocked = library_held.then(|| self.library_reason().to_string());
                let number_blocked = number_pending
                    .then(|| "Reading the next project number in that folder…".to_string());

                // Pinned actions first, then a resizable review dock, so the form
                // above keeps a usable height and the buttons never scroll away.
                egui::Panel::bottom("new_project_actions").show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let plan_reason = dialog
                            .plan_reason(&self.snapshot)
                            .or_else(|| library_blocked.clone())
                            .or_else(|| number_blocked.clone());
                        let preview = ui.add_enabled(
                            plan_reason.is_none(),
                            egui::Button::new("Preview"),
                        );
                        if preview.clicked() {
                            dialog.session.clear();
                            if let Some(summary) = self.snapshot.template(&dialog.template).cloned()
                            {
                                let library = self.library.clone();
                                let draft = dialog.draft(revision);
                                if let Err(message) = dialog.session.begin_preview(
                                    ctx,
                                    jobs,
                                    draft,
                                    move |draft, revision| {
                                        let template = library
                                            .snapshot(&summary)
                                            .map_err(|error| describe(&error))?;
                                        plan_new_project(draft, &template, revision)
                                    },
                                ) {
                                    dialog.session.message = Some(message);
                                }
                            } else {
                                dialog.session.message = Some(NO_TEMPLATE.to_string());
                            }
                        }
                        if let Some(reason) = &plan_reason {
                            explain(&preview, reason);
                        }

                        let check_reason = match &dialog.session.preview {
                            None => Some("Preview the project first.".to_string()),
                            Some(preview) if preview.draft != dialog.draft(revision) => {
                                Some("The form changed; preview again.".to_string())
                            }
                            Some(_) => None,
                        }
                        .or_else(|| library_blocked.clone());
                        let check = ui.add_enabled(
                            !dialog.busy() && check_reason.is_none(),
                            egui::Button::new("Check"),
                        );
                        if check.clicked() {
                            if let Some(preview) = dialog.session.preview.clone() {
                                if let Err(message) = dialog.session.begin_validate(
                                    ctx,
                                    jobs,
                                    preview.draft.clone(),
                                    preview.plan.clone(),
                                    preview.revision,
                                    preview.project_root.clone(),
                                ) {
                                    dialog.session.message = Some(message);
                                }
                            }
                        }
                        if let Some(reason) = &check_reason {
                            explain(&check, reason);
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled(!dialog.busy(), egui::Button::new("Cancel"))
                                .clicked()
                            {
                                cancel = true;
                            }
                            let execute_reason = if dialog.busy() {
                                Some(BUSY.to_string())
                            } else if let Some(reason) = library_blocked.clone() {
                                Some(reason)
                            } else if dialog
                                .session
                                .review
                                .as_ref()
                                .is_some_and(|review| review.draft != dialog.draft(revision))
                            {
                                Some("The form changed; check again.".to_string())
                            } else if dialog
                                .session
                                .review
                                .as_ref()
                                .and_then(|review| review.ready.as_ref())
                                .is_none()
                            {
                                Some("Check the preview first.".to_string())
                            } else {
                                None
                            };
                            let add = accent_button(ui, "Add project", execute_reason.is_none());
                            if add.clicked() {
                                match (self.catalog.clone(), self.user) {
                                    (Some(catalog), Some(user)) => {
                                        if let Err(message) = dialog.session.begin_execute(
                                            ctx,
                                            jobs,
                                            catalog,
                                            user,
                                            "Creating the project",
                                        ) {
                                            dialog.session.message = Some(message);
                                        }
                                    }
                                    _ => dialog.session.message = Some(NO_CATALOG.to_string()),
                                }
                            }
                            if let Some(reason) = &execute_reason {
                                explain(&add, reason);
                            }
                        });
                    });
                });

                egui::Panel::bottom("new_project_review")
                    .resizable(true)
                    .default_size(300.0)
                    .min_size(140.0)
                    .max_size(560.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Review and report").small().weak());
                            if let Some(preview) = &dialog.session.preview {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} folder(s) · {} script(s)",
                                        preview.folders, preview.scripts
                                    ))
                                    .small()
                                    .weak(),
                                );
                            }
                        });
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .id_salt("new_project_review_lines")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                if let Some(feedback) = &dialog.feedback {
                                    notice(ui, feedback, false);
                                }
                                if let Some(message) = &dialog.session.message {
                                    notice(ui, message, false);
                                }
                                if let Some(report) = &dialog.session.report {
                                    ui.label(egui::RichText::new(&report.headline).small());
                                    if let Some(error) = &report.error {
                                        notice(ui, error, true);
                                    }
                                    notices(ui, "Created and failed items", "new_project_run", &report.lines);
                                    if !report.inspection.is_empty() {
                                        ui.collapsing("Journal record", |ui| {
                                            for line in &report.inspection {
                                                ui.label(
                                                    egui::RichText::new(line).small().monospace(),
                                                );
                                            }
                                        });
                                    }
                                }
                                if let Some(review) = &dialog.session.review {
                                    ui.label(egui::RichText::new(review.summary()).small());
                                    if let Some(refusal) = &review.refusal {
                                        notice(ui, refusal, true);
                                    }
                                    for warning in &review.warnings {
                                        notice(ui, warning, false);
                                    }
                                    review_rows(ui, &review.rows);
                                } else if let Some(preview) = &dialog.session.preview {
                                    for warning in &preview.warnings {
                                        notice(ui, warning, false);
                                    }
                                    preview_rows(ui, preview, None);
                                } else {
                                    empty_state(
                                        ui,
                                        "No preview yet.",
                                        "Fill in the form and preview the project to review it here.",
                                    );
                                }
                            });
                    });

                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("new_project_form")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Grid::new("new_project_fields")
                                .num_columns(2)
                                .spacing([12.0, 6.0])
                                .show(ui, |ui| {
                                    ui.label("Create inside");
                                    ui.horizontal(|ui| {
                                        let edit = ui.add_enabled(
                                            editing,
                                            egui::TextEdit::singleline(&mut dialog.parent)
                                                .desired_width(400.0)
                                                .hint_text("Absolute parent folder"),
                                        );
                                        if edit.changed() {
                                            // No folder read per keystroke: the next free
                                            // number is looked up when the field is committed.
                                            if dialog.number == dialog.suggested_number {
                                                dialog.number.clear();
                                            }
                                            dialog.suggested_number.clear();
                                        }
                                        if edit.lost_focus() {
                                            ask_number = true;
                                        }
                                        if ui
                                            .add_enabled(editing, egui::Button::new("Browse…"))
                                            .clicked()
                                        {
                                            self.begin_pick(jobs, PickTarget::NewProjectParent);
                                        }
                                    });
                                    ui.end_row();

                                    ui.label("Naming");
                                    let mut naming = dialog.naming;
                                    ui.add_enabled_ui(editing, |ui| {
                                        egui::ComboBox::from_id_salt("new_project_naming")
                                            .selected_text(naming.label())
                                            .width(220.0)
                                            .show_ui(ui, |ui| {
                                                for preset in [
                                                    NamingPreset::LauncherDated,
                                                    NamingPreset::FolderStructurer,
                                                ] {
                                                    ui.selectable_value(
                                                        &mut naming,
                                                        preset,
                                                        preset.label(),
                                                    );
                                                }
                                            });
                                    });
                                    if naming != dialog.naming {
                                        let old =
                                            std::mem::replace(&mut dialog.suggested_number, String::new());
                                        if dialog.number == old {
                                            dialog.number.clear();
                                        }
                                        dialog.naming = naming;
                                        ask_number = true;
                                    }
                                    ui.end_row();

                                    ui.label("Number");
                                    ui.horizontal(|ui| {
                                        ui.add_enabled(
                                            editing,
                                            egui::TextEdit::singleline(&mut dialog.number)
                                                .desired_width(90.0)
                                                .hint_text("auto"),
                                        );
                                        if ui
                                            .add_enabled(editing, egui::Button::new("Next"))
                                            .on_hover_text("Use the next free number in that folder.")
                                            .clicked()
                                        {
                                            dialog.number = dialog.suggested_number.clone();
                                        }
                                        ui.label(
                                            egui::RichText::new(match dialog.naming {
                                                NamingPreset::LauncherDated => {
                                                    "required by the dated preset"
                                                }
                                                NamingPreset::FolderStructurer => "optional",
                                            })
                                            .small()
                                            .weak(),
                                        );
                                    });
                                    ui.end_row();

                                    ui.label("Client");
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.add_enabled(
                                                editing,
                                                egui::TextEdit::singleline(&mut dialog.client)
                                                    .desired_width(200.0)
                                                    .hint_text("Client name"),
                                            );
                                            if ui
                                                .add_enabled(
                                                    editing,
                                                    egui::Button::new("Import clients…"),
                                                )
                                                .on_hover_text(
                                                    "Merge an existing clients.json into the library.",
                                                )
                                                .clicked()
                                            {
                                                self.begin_pick(jobs, PickTarget::ClientsFile);
                                            }
                                        });
                                        let matches: Vec<String> = clients
                                            .iter()
                                            .filter(|client| client_matches(&dialog.client, client))
                                            .cloned()
                                            .collect();
                                        if !matches.is_empty() {
                                            ui.add_enabled_ui(editing, |ui| {
                                                egui::ScrollArea::vertical()
                                                    .id_salt("new_project_clients")
                                                    .max_height(96.0)
                                                    .auto_shrink([false, false])
                                                    .show(ui, |ui| {
                                                        ui.horizontal_wrapped(|ui| {
                                                            for client in &matches {
                                                                let selected =
                                                                    &dialog.client == client;
                                                                if ui
                                                                    .add(
                                                                        egui::Button::selectable(
                                                                            selected, client,
                                                                        ),
                                                                    )
                                                                    .clicked()
                                                                {
                                                                    dialog.client =
                                                                        client.clone();
                                                                }
                                                            }
                                                        });
                                                    });
                                            });
                                        }
                                    });
                                    ui.end_row();

                                    ui.label("Project name");
                                    ui.add_enabled(
                                        editing,
                                        egui::TextEdit::singleline(&mut dialog.name)
                                            .desired_width(260.0)
                                            .hint_text("Working title"),
                                    );
                                    ui.end_row();

                                    ui.label("Date");
                                    ui.horizontal(|ui| {
                                        ui.add_enabled(
                                            editing,
                                            egui::TextEdit::singleline(&mut dialog.date)
                                                .desired_width(110.0)
                                                .hint_text("YYYY-MM-DD"),
                                        );
                                        ui.label(
                                            egui::RichText::new(
                                                "captured once for preview and creation",
                                            )
                                            .small()
                                            .weak(),
                                        );
                                    });
                                    ui.end_row();

                                    ui.label("Folder template");
                                    ui.horizontal(|ui| {
                                        let mut template = dialog.template.clone();
                                        ui.add_enabled_ui(editing, |ui| {
                                            egui::ComboBox::from_id_salt("new_project_template")
                                                .selected_text(
                                                    self.snapshot
                                                        .name_of(&template)
                                                        .unwrap_or("No template"),
                                                )
                                                .width(200.0)
                                                .show_ui(ui, |ui| {
                                                    for entry in &templates {
                                                        ui.selectable_value(
                                                            &mut template,
                                                            entry.id.clone(),
                                                            &entry.name,
                                                        );
                                                    }
                                                });
                                            if ui
                                                .button("Manage…")
                                                .on_hover_text("Templates & clients")
                                                .clicked()
                                            {
                                                self.dialog.open = true;
                                            }
                                        });
                                        if template != dialog.template {
                                            dialog.template = template;
                                            dialog.session.clear();
                                        }
                                    });
                                    ui.end_row();

                                    ui.label("Project folder");
                                    ui.label(egui::RichText::new(dialog.root_name()).monospace());
                                    ui.end_row();
                                });

                            ui.add_space(4.0);
                            ui.separator();
                            ui.label(egui::RichText::new("Sequences and shots").small().weak());
                            if dialog.tree.ui(ui, editing) {
                                dialog.session.invalidate(&dialog.draft(revision));
                            }
                            ui.add_space(4.0);
                            ui.separator();
                            dialog.nuke.ui(ui, editing, None);
                        });
                });
            });

        if dialog.busy() || (!cancel && open) {
            self.new_project = Some(dialog);
            if ask_number {
                self.number_request = self.number_key();
            }
        }
    }

    // ------------------------------------------------------- templates dialog

    pub fn templates_dialog(&mut self, ctx: &egui::Context, jobs: &mut Jobs) {
        if !self.dialog.open {
            return;
        }
        if ctx.input(|input| input.key_pressed(egui::Key::Escape))
            && ctx.memory(|memory| memory.focused().is_none())
        {
            self.dialog.open = false;
            return;
        }
        // The window stays visible and honest while a worker runs; only its
        // controls go inert, so nothing the user typed disappears.
        let editable = !self.busy();
        let blocked = self.library_reason();
        let warnings = self.snapshot.warnings.clone();
        let mut actions: Vec<DialogAction> = Vec::new();
        let mut open = self.dialog.open;
        const PANES: f32 = 250.0;
        egui::Window::new("Templates & clients")
            .collapsible(false)
            .resizable(true)
            .default_size([880.0, 660.0])
            .min_width(720.0)
            .open(&mut open)
            .show(ctx, |ui| {
                if let Some(error) = &self.library_error {
                    notice(ui, error, true);
                }
                if let Some(error) = &self.dialog.error {
                    notice(ui, error, true);
                }
                if let Some(feedback) = &self.dialog.feedback {
                    notice(ui, feedback, false);
                }
                notices(ui, "Library notices", "manager_library", &warnings);

                egui::Panel::bottom("manager_footer").show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new("Changes are written to disk immediately.")
                                .small()
                                .weak(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Close").clicked() {
                                actions.push(DialogAction::Close);
                            }
                        });
                    });
                });

                egui::CentralPanel::default().show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("manager_body")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.horizontal_top(|ui| {
                                ui.vertical(|ui| {
                                    ui.set_width(300.0);
                                    ui.set_min_height(PANES);
                                    ui.label(egui::RichText::new("Templates").small().weak());
                                    egui::ScrollArea::vertical()
                                        .id_salt("template_list")
                                        .max_height(PANES)
                                        .min_scrolled_height(PANES)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            for entry in self.snapshot.templates.clone() {
                                                let selected = self.dialog.selected == entry.id;
                                                if ui
                                                    .add_enabled(
                                                        editable,
                                                        egui::Button::selectable(
                                                            selected,
                                                            &entry.name,
                                                        ),
                                                    )
                                                    .clicked()
                                                {
                                                    self.dialog.selected = entry.id.clone();
                                                    self.dialog.folder = PathBuf::new();
                                                    self.dialog.confirm = None;
                                                }
                                            }
                                            if self.snapshot.templates.is_empty() {
                                                ui.label(
                                                    egui::RichText::new("No templates yet.")
                                                        .small()
                                                        .weak(),
                                                );
                                            }
                                        });
                                    ui.horizontal(|ui| {
                                        ui.add_enabled(
                                            editable,
                                            egui::TextEdit::singleline(&mut self.dialog.new_name)
                                                .hint_text("New template name")
                                                .desired_width(150.0),
                                        );
                                        if ui
                                            .add_enabled(editable, egui::Button::new("Create"))
                                            .clicked()
                                        {
                                            actions.push(DialogAction::CreateTemplate);
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add_enabled(
                                                editable,
                                                egui::Button::new("Import folder…"),
                                            )
                                            .clicked()
                                        {
                                            actions.push(DialogAction::ImportTemplate);
                                        }
                                        if ui
                                            .add_enabled(
                                                editable && !self.dialog.selected.is_empty(),
                                                egui::Button::new("Delete template"),
                                            )
                                            .on_hover_text("Delete the template and its folder.")
                                            .clicked()
                                        {
                                            actions.push(DialogAction::AskDeleteTemplate);
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        ui.add_enabled(
                                            editable,
                                            egui::TextEdit::singleline(&mut self.dialog.rename)
                                                .hint_text("Rename to")
                                                .desired_width(150.0),
                                        );
                                        if ui
                                            .add_enabled(
                                                editable && !self.dialog.selected.is_empty(),
                                                egui::Button::new("Rename"),
                                            )
                                            .clicked()
                                        {
                                            actions.push(DialogAction::RenameTemplate);
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add_enabled(
                                                editable && !self.dialog.selected.is_empty(),
                                                egui::Button::new("Move up"),
                                            )
                                            .clicked()
                                        {
                                            actions.push(DialogAction::MoveUp);
                                        }
                                        if ui
                                            .add_enabled(
                                                editable && !self.dialog.selected.is_empty(),
                                                egui::Button::new("Move down"),
                                            )
                                            .clicked()
                                        {
                                            actions.push(DialogAction::MoveDown);
                                        }
                                    });
                                });
                                ui.separator();
                                ui.vertical(|ui| {
                                    ui.set_min_height(PANES);
                                    ui.label(egui::RichText::new("Folders").small().weak());
                                    let rows = self.selected_template_rows();
                                    egui::ScrollArea::vertical()
                                        .id_salt("template_folders")
                                        .max_height(PANES)
                                        .min_scrolled_height(PANES)
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| {
                                            for (relative, depth, markers) in &rows {
                                                ui.push_id(("template_folder", relative), |ui| {
                                                    ui.horizontal(|ui| {
                                                        ui.add_space(*depth as f32 * 12.0);
                                                        let label =
                                                            format!("{}/", file_label(relative));
                                                        if ui
                                                            .add_enabled(
                                                                editable,
                                                                egui::Button::selectable(
                                                                    self.dialog.folder == *relative,
                                                                    label,
                                                                ),
                                                            )
                                                            .clicked()
                                                        {
                                                            self.dialog.folder = relative.clone();
                                                            self.dialog.markers_sequences =
                                                                markers.sequences;
                                                            self.dialog.markers_shots =
                                                                markers.shots;
                                                        }
                                                        let mut flags = Vec::new();
                                                        if markers.sequences {
                                                            flags.push("sequences");
                                                        }
                                                        if markers.shots {
                                                            flags.push("shots");
                                                        }
                                                        if !flags.is_empty() {
                                                            ui.label(
                                                                egui::RichText::new(
                                                                    flags.join(" + "),
                                                                )
                                                                .small()
                                                                .weak(),
                                                            );
                                                        }
                                                    });
                                                });
                                            }
                                            if rows.is_empty() {
                                                ui.label(
                                                    egui::RichText::new(
                                                        "No folders in this template.",
                                                    )
                                                    .small()
                                                    .weak(),
                                                );
                                            }
                                        });
                                    ui.horizontal(|ui| {
                                        ui.add_enabled(
                                            editable,
                                            egui::TextEdit::singleline(
                                                &mut self.dialog.folder_name,
                                            )
                                            .hint_text("New folder name")
                                            .desired_width(150.0),
                                        );
                                        if ui
                                            .add_enabled(editable, egui::Button::new("Add folder"))
                                            .on_hover_text(
                                                "Add inside the selected folder, or at the top.",
                                            )
                                            .clicked()
                                        {
                                            actions.push(DialogAction::AddFolder);
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        ui.add_enabled(
                                            editable,
                                            egui::TextEdit::singleline(
                                                &mut self.dialog.folder_rename,
                                            )
                                            .hint_text("Rename folder to")
                                            .desired_width(150.0),
                                        );
                                        if ui
                                            .add_enabled(
                                                editable
                                                    && !self.dialog.folder.as_os_str().is_empty(),
                                                egui::Button::new("Rename folder"),
                                            )
                                            .clicked()
                                        {
                                            actions.push(DialogAction::RenameFolder);
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        let mut markers_changed = false;
                                        markers_changed |= ui
                                            .add_enabled(
                                                editable
                                                    && !self.dialog.folder.as_os_str().is_empty(),
                                                egui::Checkbox::new(
                                                    &mut self.dialog.markers_sequences,
                                                    "Sequence folders",
                                                ),
                                            )
                                            .on_hover_text(
                                                "sequences.txt: one folder per named sequence",
                                            )
                                            .changed();
                                        markers_changed |= ui
                                            .add_enabled(
                                                editable
                                                    && !self.dialog.folder.as_os_str().is_empty(),
                                                egui::Checkbox::new(
                                                    &mut self.dialog.markers_shots,
                                                    "Shot folders",
                                                ),
                                            )
                                            .on_hover_text("shots.txt: shots inside this folder")
                                            .changed();
                                        if markers_changed {
                                            actions.push(DialogAction::SetMarkers);
                                        }
                                        if ui
                                            .add_enabled(
                                                editable
                                                    && !self.dialog.folder.as_os_str().is_empty(),
                                                egui::Button::new("Delete folder"),
                                            )
                                            .on_hover_text("Delete this folder and its contents.")
                                            .clicked()
                                        {
                                            actions.push(DialogAction::AskDeleteFolder);
                                        }
                                    });
                                    if let Some(confirm) = self.dialog.confirm.clone() {
                                        ui.horizontal(|ui| {
                                            let what = match &confirm {
                                                Confirm::Template(_) => {
                                                    "this template and its folder"
                                                }
                                                Confirm::Folder(_) => {
                                                    "this template folder and its contents"
                                                }
                                            };
                                            ui.label(
                                                egui::RichText::new(format!("Delete {what}?"))
                                                    .small()
                                                    .color(WARNING),
                                            );
                                            if ui
                                                .add_enabled(
                                                    editable,
                                                    egui::Button::new("Delete").small(),
                                                )
                                                .clicked()
                                            {
                                                actions.push(DialogAction::ConfirmDelete);
                                            }
                                            if ui
                                                .add_enabled(
                                                    editable,
                                                    egui::Button::new("Keep").small(),
                                                )
                                                .clicked()
                                            {
                                                actions.push(DialogAction::CancelDelete);
                                            }
                                        });
                                    }
                                    if !blocked.is_empty() {
                                        ui.label(
                                            egui::RichText::new(blocked).small().color(WARNING),
                                        );
                                    }
                                });
                            });

                            ui.separator();
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Client names").small().weak());
                                if ui
                                    .add_enabled(editable, egui::Button::new("Import clients…"))
                                    .on_hover_text("Merge a clients.json file into the library.")
                                    .clicked()
                                {
                                    actions.push(DialogAction::ImportClients);
                                }
                            });
                            egui::ScrollArea::vertical()
                                .id_salt("manager_clients")
                                .max_height(150.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        for client in self.snapshot.clients.clone() {
                                            ui.push_id(("client", &client), |ui| {
                                                ui.group(|ui| {
                                                    ui.horizontal(|ui| {
                                                        ui.label(&client);
                                                        if ui
                                                            .add_enabled(
                                                                editable,
                                                                egui::Button::new("Rename").small(),
                                                            )
                                                            .clicked()
                                                        {
                                                            self.dialog.client_rename = Some((
                                                                client.clone(),
                                                                client.clone(),
                                                            ));
                                                        }
                                                        if ui
                                                            .add_enabled(
                                                                editable,
                                                                egui::Button::new("Remove").small(),
                                                            )
                                                            .clicked()
                                                        {
                                                            actions.push(
                                                                DialogAction::RemoveClient(
                                                                    client.clone(),
                                                                ),
                                                            );
                                                        }
                                                    });
                                                });
                                            });
                                        }
                                        if self.snapshot.clients.is_empty() {
                                            ui.label(
                                                egui::RichText::new("No clients yet.")
                                                    .small()
                                                    .weak(),
                                            );
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        ui.add_enabled(
                                            editable,
                                            egui::TextEdit::singleline(
                                                &mut self.dialog.client_entry,
                                            )
                                            .hint_text("New client name")
                                            .desired_width(180.0),
                                        );
                                        if ui
                                            .add_enabled(editable, egui::Button::new("Add client"))
                                            .clicked()
                                        {
                                            actions.push(DialogAction::AddClient);
                                        }
                                    });
                                    if let Some((from, to)) = self.dialog.client_rename.clone() {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                egui::RichText::new(format!("Rename {from} to"))
                                                    .small(),
                                            );
                                            let mut value = to.clone();
                                            if ui
                                                .add_enabled(
                                                    editable,
                                                    egui::TextEdit::singleline(&mut value)
                                                        .desired_width(160.0),
                                                )
                                                .changed()
                                            {
                                                self.dialog.client_rename =
                                                    Some((from.clone(), value.clone()));
                                            }
                                            if ui
                                                .add_enabled(
                                                    editable,
                                                    egui::Button::new("Apply").small(),
                                                )
                                                .clicked()
                                            {
                                                actions.push(DialogAction::RenameClient(
                                                    from.clone(),
                                                    value,
                                                ));
                                            }
                                            if ui
                                                .add_enabled(
                                                    editable,
                                                    egui::Button::new("Cancel").small(),
                                                )
                                                .clicked()
                                            {
                                                actions.push(DialogAction::CancelClientRename);
                                            }
                                        });
                                    }
                                });
                        });
                });
            });

        if !open {
            self.dialog.open = false;
        }
        for action in actions {
            self.apply_dialog_action(action, jobs);
        }
    }

    fn selected_template_rows(&self) -> Vec<(PathBuf, usize, TemplateMarkers)> {
        let Some(template) = &self.dialog_template else {
            return Vec::new();
        };
        template
            .entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                structure::TemplateEntryKind::Directory { markers } => Some((
                    entry.relative.clone(),
                    entry.relative.components().count().saturating_sub(1),
                    *markers,
                )),
                _ => None,
            })
            .collect()
    }

    fn apply_dialog_action(&mut self, action: DialogAction, jobs: &mut Jobs) {
        if self.library_busy() {
            self.dialog.error = Some(BUSY.to_string());
            return;
        }
        // Template contents are part of both plans, even when their ID is unchanged.
        self.session.clear();
        if let Some(dialog) = self.new_project.as_mut() {
            dialog.session.clear();
        }
        self.dialog.error = None;
        match action {
            DialogAction::Close => self.dialog.open = false,
            DialogAction::CreateTemplate => {
                let name = self.dialog.new_name.clone();
                self.request_library(jobs, LibraryRequest::Create { name });
            }
            DialogAction::ImportTemplate => self.begin_pick(jobs, PickTarget::TemplateFolder),
            DialogAction::ImportClients => self.begin_pick(jobs, PickTarget::ClientsFile),
            DialogAction::AskDeleteTemplate => {
                self.dialog.confirm = Some(Confirm::Template(self.dialog.selected.clone()));
            }
            DialogAction::AskDeleteFolder => {
                self.dialog.confirm = Some(Confirm::Folder(self.dialog.folder.clone()));
            }
            DialogAction::CancelDelete => self.dialog.confirm = None,
            DialogAction::ConfirmDelete => match self.dialog.confirm.take() {
                Some(Confirm::Template(id)) => {
                    self.dialog.selected.clear();
                    self.dialog.folder = PathBuf::new();
                    self.request_library(jobs, LibraryRequest::Delete { id });
                }
                Some(Confirm::Folder(relative)) => {
                    let id = self.dialog.selected.clone();
                    self.request_library(jobs, LibraryRequest::RemoveFolder { id, relative });
                }
                None => {}
            },
            DialogAction::RenameTemplate => {
                let id = self.dialog.selected.clone();
                let name = self.dialog.rename.clone();
                self.request_library(jobs, LibraryRequest::Rename { id, name });
            }
            DialogAction::MoveUp | DialogAction::MoveDown => {
                let delta = if matches!(action, DialogAction::MoveUp) {
                    -1
                } else {
                    1
                };
                let id = self.dialog.selected.clone();
                self.request_library(jobs, LibraryRequest::Reorder { id, delta });
            }
            DialogAction::AddFolder => {
                let id = self.dialog.selected.clone();
                let parent = self.dialog.folder.clone();
                let name = self.dialog.folder_name.clone();
                self.request_library(jobs, LibraryRequest::AddFolder { id, parent, name });
            }
            DialogAction::RenameFolder => {
                let id = self.dialog.selected.clone();
                let relative = self.dialog.folder.clone();
                let name = self.dialog.folder_rename.clone();
                self.request_library(jobs, LibraryRequest::RenameFolder { id, relative, name });
            }
            DialogAction::SetMarkers => {
                let id = self.dialog.selected.clone();
                let relative = self.dialog.folder.clone();
                let markers = TemplateMarkers {
                    sequences: self.dialog.markers_sequences,
                    shots: self.dialog.markers_shots,
                };
                self.request_library(
                    jobs,
                    LibraryRequest::SetMarkers {
                        id,
                        relative,
                        markers,
                    },
                );
            }
            DialogAction::AddClient => {
                let mut clients = self.snapshot.clients.clone();
                match structure::add_client(&mut clients, &self.dialog.client_entry) {
                    Ok(true) => {
                        self.request_library(jobs, LibraryRequest::SaveClients { clients });
                    }
                    Ok(false) => {
                        self.dialog.feedback = Some("That client is already listed.".to_string());
                    }
                    Err(error) => self.dialog.error = Some(describe(&error)),
                }
            }
            DialogAction::RemoveClient(name) => {
                let clients: Vec<String> = self
                    .snapshot
                    .clients
                    .iter()
                    .filter(|client| **client != name)
                    .cloned()
                    .collect();
                self.request_library(jobs, LibraryRequest::SaveClients { clients });
            }
            DialogAction::RenameClient(from, to) => {
                let renamed = match structure::normalize_client(&to) {
                    Ok(renamed) => renamed,
                    Err(error) => {
                        self.dialog.error = Some(describe(&error));
                        return;
                    }
                };
                let clients: Vec<String> = self
                    .snapshot
                    .clients
                    .iter()
                    .map(|client| {
                        if *client == from {
                            renamed.clone()
                        } else {
                            client.clone()
                        }
                    })
                    .collect();
                self.request_library(jobs, LibraryRequest::SaveClients { clients });
            }
            DialogAction::CancelClientRename => self.dialog.client_rename = None,
        }
    }
}

// ---------------------------------------------------------------------------
// Planning and rendering helpers
// ---------------------------------------------------------------------------

fn plan_add(
    draft: &StructureDraft,
    template: &StructureTemplate,
    revision: u64,
) -> Result<OperationPlan, String> {
    let generator = ScriptGenerator;
    let Some(root) = draft.root.clone() else {
        return Err(NO_PROJECT.to_string());
    };
    let plan = if draft.template_destinations() {
        structure::plan_add_shots(
            "add-shots",
            revision,
            &root,
            &draft.groups,
            template,
            draft.nuke.as_ref(),
            Some(&generator),
        )
    } else {
        structure::plan_add_to_targets(
            "add-shots",
            revision,
            &root,
            &draft.targets,
            &draft.groups,
            draft.nuke.as_ref(),
            draft.parent_as_sequence,
            Some(&generator),
        )
    };
    plan.map_err(|error| describe(&error))
}

fn plan_new_project(
    draft: &StructureDraft,
    template: &StructureTemplate,
    revision: u64,
) -> Result<OperationPlan, String> {
    let generator = ScriptGenerator;
    let Some(parent) = draft.parent.clone() else {
        return Err("Choose where the project goes first.".to_string());
    };
    let input = CreateProject {
        parent,
        number: Some(draft.number.clone()).filter(|number| !number.trim().is_empty()),
        client: Some(draft.client.clone()).filter(|client| !client.trim().is_empty()),
        name: draft.name.clone(),
        date: Some(draft.date.clone()),
        naming: draft.naming,
        shots: draft.groups.clone(),
        nuke: draft.nuke.clone(),
    };
    structure::plan_structure("new-project", revision, &input, template, Some(&generator))
        .map_err(|error| describe(&error))
}

fn preview_rows(ui: &mut egui::Ui, preview: &Preview, review: Option<&ReviewView>) {
    for row in &preview.rows {
        ui.push_id(("preview", &row.path), |ui| {
            ui.horizontal(|ui| {
                ui.add_space(row.depth as f32 * 12.0);
                let suffix = match row.kind {
                    PreviewKind::Folder => "/",
                    PreviewKind::Script => "",
                };
                let mut text = egui::RichText::new(format!("{}{suffix}", row.label)).small();
                if row.kind == PreviewKind::Script {
                    text = text.monospace();
                }
                ui.label(text);
                match review.and_then(|review| review.rows.get(row.action)) {
                    Some(item) => {
                        let colour = if item.blocked {
                            WARNING
                        } else {
                            egui::Color32::GRAY
                        };
                        ui.label(egui::RichText::new(item.state).small().color(colour))
                            .on_hover_text(item.message.clone().unwrap_or_default());
                    }
                    None => {
                        ui.label(egui::RichText::new("new").small().weak());
                    }
                }
            });
        });
    }
}

fn review_rows(ui: &mut egui::Ui, rows: &[ItemRow]) {
    for (index, row) in rows.iter().enumerate() {
        ui.push_id(("review", index), |ui| {
            ui.horizontal(|ui| {
                let colour = if row.blocked {
                    WARNING
                } else {
                    egui::Color32::GRAY
                };
                ui.label(egui::RichText::new(row.state).small().color(colour));
                ui.label(egui::RichText::new(row.kind).small().weak());
                ui.label(egui::RichText::new(&row.label).small().monospace());
                if let Some(message) = &row.message {
                    ui.label(egui::RichText::new(message).small().weak());
                }
            });
        });
    }
}
