//! Explicit prepare, inspect, and adopt flow; source data is never modified.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use eframe::egui;

use crate::jobs::{JobId, JobOutcome, JobProgress, Jobs};
use crate::legacy_import::{LegacySources, PreparedImport};

#[derive(Default)]
struct PathField {
    text: String,
    native: Option<PathBuf>,
}

impl PathField {
    fn selected(&mut self, path: PathBuf) {
        self.text = path.display().to_string();
        self.native = Some(path);
    }

    fn path(&self) -> Option<PathBuf> {
        if self.text.trim().is_empty() {
            return None;
        }
        Some(
            self.native
                .clone()
                .unwrap_or_else(|| PathBuf::from(self.text.trim())),
        )
    }
}

#[derive(Clone, Copy)]
enum Pick {
    Database,
    Ini,
    Favorites,
}

enum Reply {
    Picked(Pick, Vec<PathBuf>),
    Prepared(Result<PreparedImport, String>),
    Adopted(Result<(), String>),
}

pub struct ImportWorkspace {
    data_root: PathBuf,
    database: PathField,
    ini: PathField,
    favorites: Vec<PathBuf>,
    worker: Option<(JobId, bool, Receiver<Reply>)>,
    prepared: Option<PreparedImport>,
    reviewed: bool,
    adopted: bool,
    message: Option<String>,
}

impl ImportWorkspace {
    pub fn new(data_root: PathBuf) -> Self {
        Self {
            data_root,
            database: PathField::default(),
            ini: PathField::default(),
            favorites: Vec::new(),
            worker: None,
            prepared: None,
            reviewed: false,
            adopted: false,
            message: None,
        }
    }

    pub fn is_picker_job(&self, id: JobId) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|(job, picker, _)| *job == id && *picker)
    }

    pub fn picker_pending(&self) -> bool {
        self.worker.as_ref().is_some_and(|(_, picker, _)| *picker)
    }

    pub fn poll(&mut self) {
        let Some((_, _, replies)) = &self.worker else {
            return;
        };
        let reply = match replies.try_recv() {
            Ok(reply) => reply,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.worker = None;
                self.message = Some("Import stopped without a result. The current workspace remains open; inspect any prepared workspace before retrying.".into());
                return;
            }
        };
        self.worker = None;
        match reply {
            Reply::Picked(kind, paths) => {
                match kind {
                    Pick::Database => {
                        if let Some(path) = paths.into_iter().next() {
                            self.database.selected(path);
                        }
                    }
                    Pick::Ini => {
                        if let Some(path) = paths.into_iter().next() {
                            self.ini.selected(path);
                        }
                    }
                    Pick::Favorites => {
                        for path in paths {
                            if !self.favorites.contains(&path) {
                                self.favorites.push(path);
                            }
                        }
                    }
                }
                self.prepared = None;
                self.reviewed = false;
            }
            Reply::Prepared(result) => match result {
                Ok(prepared) => {
                    self.message =
                        Some("Import prepared. Review the report below before using it.".into());
                    self.prepared = Some(prepared);
                    self.reviewed = false;
                }
                Err(message) => self.message = Some(message),
            },
            Reply::Adopted(result) => match result {
                Ok(()) => {
                    self.adopted = true;
                    self.message = Some("The imported workspace is selected for the next launch. Close and reopen VFX Launcher to use it. This session continues using its current catalog.".into());
                }
                Err(message) => self.message = Some(message),
            },
        }
    }

    fn browse(&mut self, ui: &egui::Ui, jobs: &mut Jobs, kind: Pick) {
        let (tx, rx) = channel();
        let result = jobs.start(ui.ctx(), "Choose previous Launcher data", move |context| {
            let dialog = rfd::FileDialog::new().set_title("Choose previous Launcher data");
            let paths = match kind {
                Pick::Favorites => dialog
                    .add_filter("Favorites", &["json"])
                    .pick_files()
                    .unwrap_or_default(),
                Pick::Ini => dialog
                    .add_filter("Settings", &["ini"])
                    .pick_file()
                    .into_iter()
                    .collect(),
                Pick::Database => dialog
                    .add_filter("SQLite catalog", &["db", "sqlite", "sqlite3"])
                    .pick_file()
                    .into_iter()
                    .collect(),
            };
            let _ = tx.send(Reply::Picked(kind, paths));
            context
                .events
                .finish(JobOutcome::Succeeded, JobProgress::default());
        });
        match result {
            Ok(id) => self.worker = Some((id, true, rx)),
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn prepare(&mut self, ui: &egui::Ui, jobs: &mut Jobs) {
        let Some(database) = self.database.path() else {
            self.message = Some("Choose the previous Launcher database first.".into());
            return;
        };
        let sources = LegacySources {
            database,
            ini: self.ini.path(),
            favorites: self.favorites.clone(),
        };
        if std::iter::once(&sources.database)
            .chain(sources.ini.iter())
            .chain(sources.favorites.iter())
            .any(|path| !path.is_absolute())
        {
            self.message =
                Some("Choose absolute paths for the database, settings, and favorites.".into());
            return;
        }
        let root = self.data_root.clone();
        let (tx, rx) = channel();
        match jobs.start(ui.ctx(), "Import previous Launcher data", move |context| {
            let result = crate::legacy_import::prepare_import(
                &sources,
                &root,
                &context.control,
                &context.events,
            );
            let outcome = if context.control.is_cancelled() {
                JobOutcome::Cancelled
            } else if result.is_ok() {
                JobOutcome::Succeeded
            } else {
                JobOutcome::Failed
            };
            let _ = tx.send(Reply::Prepared(result));
            context.events.finish(outcome, JobProgress::default());
        }) {
            Ok(id) => {
                self.worker = Some((id, false, rx));
                self.prepared = None;
                self.reviewed = false;
                self.message = Some("Preparing a separate imported workspace…".into());
            }
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    fn adopt(&mut self, ui: &egui::Ui, jobs: &mut Jobs) {
        let Some(prepared) = &self.prepared else {
            return;
        };
        let directory = prepared.directory.clone();
        let root = self.data_root.clone();
        let (tx, rx) = channel();
        match jobs.start(ui.ctx(), "Select imported workspace", move |context| {
            let result = crate::config::adopt_workspace(&root, &directory).map_err(|error| format!("The workspace selection could not be confirmed: {error}. Keep the prepared folder; a restart will read the saved selection."));
            let outcome = if result.is_ok() { JobOutcome::Succeeded } else { JobOutcome::Failed };
            let _ = tx.send(Reply::Adopted(result));
            context.events.finish(outcome, JobProgress::default());
        }) {
            Ok(id) => self.worker = Some((id, false, rx)),
            Err(error) => self.message = Some(error.to_string()),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        ui.label("Choose the original database and any settings or favorites files. Import creates a separate workspace and keeps the originals intact.");
        let editable = self.worker.is_none() && !self.adopted && !jobs.is_shutting_down();
        let mut pick = None;
        let mut changed = false;
        ui.add_enabled_ui(editable, |ui| {
            egui::Grid::new("legacy_import_sources")
                .num_columns(3)
                .show(ui, |ui| {
                    for (label, field, kind) in [
                        ("Database", &mut self.database, Pick::Database),
                        ("Settings INI (optional)", &mut self.ini, Pick::Ini),
                    ] {
                        ui.label(label);
                        if ui
                            .add(egui::TextEdit::singleline(&mut field.text).desired_width(420.0))
                            .changed()
                        {
                            field.native = None;
                            changed = true;
                        }
                        if ui.button("Browse…").clicked() {
                            pick = Some(kind);
                        }
                        ui.end_row();
                    }
                });
            ui.horizontal(|ui| {
                if ui.button("Add favorites files…").clicked() {
                    pick = Some(Pick::Favorites);
                }
                if ui
                    .add_enabled(
                        !self.favorites.is_empty(),
                        egui::Button::new("Clear favorites files"),
                    )
                    .clicked()
                {
                    self.favorites.clear();
                    changed = true;
                }
            });
            for path in &self.favorites {
                ui.label(path.display().to_string());
            }
        });
        if changed {
            self.prepared = None;
            self.reviewed = false;
        }
        if let Some(kind) = pick {
            self.browse(ui, jobs, kind);
        }
        if let Some(message) = &self.message {
            ui.label(message);
        }
        if let Some(prepared) = &self.prepared {
            ui.label(format!(
                "Prepared workspace: {}",
                prepared.directory.display()
            ));
            ui.collapsing("Source records", |ui| {
                for (table, count) in &prepared.report.source_counts {
                    ui.label(format!("{table}: {count}"));
                }
            });
            for issue in &prepared.report.issues {
                ui.label(egui::RichText::new(issue).color(egui::Color32::from_rgb(224, 154, 106)));
            }
            ui.collapsing("Record-by-record import report", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        for row in &prepared.report.rows {
                            ui.label(format!(
                                "{} {}: {}",
                                row.table,
                                row.legacy_id.map(|id| id.to_string()).unwrap_or_default(),
                                row.message
                            ));
                        }
                    });
            });
            ui.add_enabled_ui(editable, |ui| {
                ui.checkbox(&mut self.reviewed, "I reviewed the import report");
            });
        }
        ui.horizontal(|ui| {
            if ui
                .add_enabled(editable, egui::Button::new("Prepare import"))
                .clicked()
            {
                self.prepare(ui, jobs);
            }
            let can_adopt = editable && self.prepared.is_some() && self.reviewed && !jobs.is_busy();
            if ui
                .add_enabled(
                    can_adopt,
                    egui::Button::new("Use imported workspace on next launch"),
                )
                .clicked()
            {
                self.adopt(ui, jobs);
            }
        });
    }
}
