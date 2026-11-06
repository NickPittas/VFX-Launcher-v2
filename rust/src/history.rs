//! Read-only activity and operation history, available from View → Logs.

use crate::storage::{CatalogClient, Pending, ReplyState};
use crate::transfers::{self, RunInspection};
use eframe::egui;

struct Activity {
    id: i64,
    at: String,
    user: ActivityUser,
    action: String,
    description: String,
}

/// The identity an action was recorded for. The id is retained because a
/// username can be removed and then recreated as a different identity, so the
/// name alone no longer says who acted.
struct ActivityUser {
    id: Option<i64>,
    /// The name stored with the action, exactly as it was recorded then.
    recorded: String,
    /// The name that identity has now, when its row is still in the user list.
    current: Option<String>,
    removed: bool,
}

impl ActivityUser {
    fn label(&self) -> String {
        let Some(id) = self.id else {
            return format!(
                "{} (user unresolved: this legacy entry recorded no user id)",
                self.recorded
            );
        };
        let mut label = format!("{} (user #{id}", self.recorded);
        match &self.current {
            Some(current) if *current != self.recorded => {
                label.push_str(&format!(", now named {current}"))
            }
            Some(_) => {}
            None => label.push_str(", no longer in the user list"),
        }
        if self.removed {
            label.push_str(", removed");
        }
        label.push(')');
        label
    }
}

struct RunHead {
    id: i64,
    at: String,
    kind: String,
    state: String,
}
struct Snapshot {
    activity: Vec<Activity>,
    runs: Vec<RunHead>,
}

#[derive(Default)]
pub struct HistoryWindow {
    pub open: bool,
    pending: Option<Pending<Snapshot>>,
    inspection_pending: Option<Pending<Option<RunInspection>>>,
    snapshot: Option<Snapshot>,
    inspection: Option<RunInspection>,
    error: Option<String>,
    filter: String,
    failures_only: bool,
    cleared_through: i64,
    cleared_runs_through: i64,
    selected_item: Option<i64>,
}

impl HistoryWindow {
    pub fn poll(&mut self) {
        if let Some(mut pending) = self.pending.take() {
            match pending.try_poll() {
                ReplyState::Waiting => self.pending = Some(pending),
                ReplyState::Ready(snapshot) => {
                    self.snapshot = Some(snapshot);
                    self.error = None;
                }
                ReplyState::Failed(error) => self.error = Some(error.to_string()),
                ReplyState::Finished => {}
            }
        }
        if let Some(mut pending) = self.inspection_pending.take() {
            match pending.try_poll() {
                ReplyState::Waiting => self.inspection_pending = Some(pending),
                ReplyState::Ready(inspection) => {
                    self.inspection = inspection;
                    self.selected_item = None;
                }
                ReplyState::Failed(error) => self.error = Some(error.to_string()),
                ReplyState::Finished => {}
            }
        }
    }

    fn refresh(&mut self, catalog: &CatalogClient) {
        if self.pending.is_some() {
            return;
        }
        match catalog.submit(|connection| {
            let mut activity_query = connection.prepare("SELECT a.id, a.at, a.user_id, a.username, u.username, u.deleted_at, a.action_type, a.description FROM user_activity_log a LEFT JOIN users u ON u.id = a.user_id ORDER BY a.id DESC LIMIT 1000").map_err(error)?;
            let activity = activity_query.query_map([], |row| Ok(Activity { id: row.get(0)?, at: row.get(1)?, user: ActivityUser { id: row.get(2)?, recorded: row.get(3)?, current: row.get(4)?, removed: row.get::<_, Option<String>>(5)?.is_some() }, action: row.get(6)?, description: row.get(7)? })).map_err(error)?.collect::<Result<Vec<_>,_>>().map_err(error)?;
            let mut runs_query = connection.prepare("SELECT id,started_at,kind,state FROM operation_runs ORDER BY id DESC LIMIT 500").map_err(error)?;
            let runs = runs_query.query_map([], |row| Ok(RunHead { id: row.get(0)?, at: row.get(1)?, kind: row.get(2)?, state: row.get(3)? })).map_err(error)?.collect::<Result<Vec<_>,_>>().map_err(error)?;
            Ok(Snapshot { activity, runs })
        }) {
            Ok(pending) => self.pending = Some(pending),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, catalog: Option<&CatalogClient>) {
        if !self.open {
            return;
        }
        if self.snapshot.is_none() && self.pending.is_none() {
            if let Some(catalog) = catalog {
                self.refresh(catalog);
            }
        }
        let mut open = self.open;
        let mut selected_run = None;
        egui::Window::new("Logs and file-operation history").open(&mut open).default_size(egui::vec2(1060., 720.)).resizable(true).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter history"));
                ui.checkbox(&mut self.failures_only, "Only problem runs");
                if ui.add_enabled(catalog.is_some() && self.pending.is_none(), egui::Button::new("Refresh")).clicked() {
                    if let Some(catalog) = catalog { self.refresh(catalog); }
                }
                if ui.button("Clear this view").clicked() {
                    self.cleared_through = self.snapshot.as_ref().and_then(|snapshot| snapshot.activity.first()).map(|entry| entry.id).unwrap_or(0);
                    self.cleared_runs_through = self.snapshot.as_ref().and_then(|snapshot| snapshot.runs.first()).map(|run| run.id).unwrap_or(0);
                    self.inspection = None;
                    self.inspection_pending = None;
                }
            });
            if self.pending.is_some() { ui.spinner(); }
            if let Some(error) = &self.error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
            if catalog.is_none() { ui.label("The local catalog is not available."); }
            let needle = self.filter.to_lowercase();
            egui::ScrollArea::vertical().id_salt("history_scroll").show(ui, |ui| {
                if let Some(snapshot) = &self.snapshot {
                    ui.collapsing("Recorded actions", |ui| {
                        let rows: Vec<_> = snapshot.activity.iter().filter(|entry| entry.id > self.cleared_through).map(|entry| (entry, entry.user.label())).filter(|(entry, user)| format!("{user} {} {}", entry.action, entry.description).to_lowercase().contains(&needle)).collect();
                        if rows.is_empty() { ui.label("No matching recorded actions."); }
                        for (entry, user) in rows {
                            ui.horizontal_wrapped(|ui| { ui.label(&entry.at); ui.label(user); ui.label(&entry.description); });
                        }
                    });
                    ui.collapsing("File operations", |ui| {
                        for run in &snapshot.runs {
                            if run.id <= self.cleared_runs_through { continue; }
                            if self.failures_only && !matches!(run.state.as_str(), "failed" | "partial" | "interrupted" | "running" | "pending") { continue; }
                            let label = format!("Run {} · {} · {} · {}", run.id, run.at, run.kind.replace('_', " "), run.state);
                            if !label.to_lowercase().contains(&needle) { continue; }
                            if ui.selectable_label(self.inspection.as_ref().is_some_and(|selected| selected.run_id == run.id), label).clicked() { selected_run = Some(run.id); }
                        }
                        if snapshot.runs.is_empty() { ui.label("No file operations recorded."); }
                    });
                }
                if self.inspection_pending.is_some() { ui.label("Reading operation details…"); }
                if let Some(run) = &self.inspection {
                    ui.separator();
                    ui.label(format!("Run {} · {}", run.run_id, run.state.as_str()));
                    ui.label(run.actor.as_ref().map(|actor| format!("Requested by {}", actor.username)).unwrap_or_else(|| "No user identity was recorded for this run.".into()));
                    if let Some(error) = &run.detail_error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
                    if let Some(counts) = run.confirmed_counts() { ui.label(format!("{} copied · {} created · {} skipped · {} failed · {} unfinished", counts.copied, counts.created, counts.skipped, counts.failed, counts.unfinished())); }
                    ui.label("History is read-only. Retained paths are never deleted or replayed automatically.");
                    egui::ScrollArea::vertical().id_salt("operation_items").max_height(220.).show_rows(ui, 22., run.items.len(), |ui, range| {
                        for item in &run.items[range] {
                            let path = item.destination.as_ref().map(|path| path.display().to_string()).or_else(|| item.destination_display.clone()).unwrap_or_else(|| "Unresolved destination".into());
                            if ui.selectable_label(self.selected_item == Some(item.ordinal), format!("{} · {} · {}", item.ordinal + 1, format!("{:?}", item.state), path)).clicked() { self.selected_item = Some(item.ordinal); }
                        }
                    });
                    if let Some(item) = run.items.iter().find(|item| Some(item.ordinal) == self.selected_item) {
                        if let Some(error) = &item.message_error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
                        for event in &item.events {
                            if let Some(message) = &event.message { ui.label(message); }
                            for path in event.recovery_paths() {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(format!("Retained path: {}", path.display()));
                                    if let Some(text) = path.to_str() {
                                        if ui.small_button("Copy path").clicked() { ui.ctx().copy_text(text.to_string()); }
                                    }
                                });
                            }
                        }
                    }
                }
            });
        });
        self.open = open;
        if let (Some(catalog), Some(run_id)) = (catalog, selected_run) {
            self.inspection = None;
            match transfers::read_run(catalog, run_id) {
                Ok(pending) => self.inspection_pending = Some(pending),
                Err(error) => self.error = Some(error.to_string()),
            }
        }
    }
}

fn error(error: rusqlite::Error) -> crate::storage::StorageError {
    crate::storage::StorageError::new(
        crate::storage::StorageCategory::Database,
        None,
        format!("Could not read history: {error}"),
    )
}
