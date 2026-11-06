//! Application shell: approved desktop composition, six working views and the
//! pinned action/status strip.
//!
//! The layout follows `DESIGN-APPROVAL.md` and `USER-UI-REQUIREMENTS.md`: menu
//! bar, workspace strip, docked resizable panes with internal scrolling, and a
//! persistent bottom status/execution strip.
//!
//! Feature workspaces own their background work; the shell coordinates their
//! catalog admission, close lifecycle and shared execution strip.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui;

use crate::copy_workspace::CopyByFileTypeState;
use crate::incoming_workspace::OrganizeIncomingState;
use crate::jobs::{JobId, JobStatus, Jobs, LIVE_POLL_INTERVAL};
use crate::launch::Tool;
use crate::project_workspace::{ProjectWorkspace, RegistrationStatus, SettingsPath};
use crate::projects::{FileGroup, FileKind, ProjectFile, ProjectList};
use crate::storage::{FileId, ProjectId};
use crate::structure_workspace::StructureWorkspace;

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2f, 0x6b, 0xa8);
const PANEL_SURFACE: egui::Color32 = egui::Color32::from_rgb(0x24, 0x25, 0x28);
const EDITOR_SURFACE: egui::Color32 = egui::Color32::from_rgb(0x1a, 0x1b, 0x1d);
const CONTROL_SURFACE: egui::Color32 = egui::Color32::from_rgb(0x2d, 0x2f, 0x33);
const UNAVAILABLE: &str = "Not available in this build.";
/// Missing metadata is displayed without fabricating a value.
const UNKNOWN: &str = "—";

/// Starts the native desktop application against one local data area.
pub fn run_gui(data_dir: PathBuf) -> eframe::Result {
    let workspace_dir = crate::config::resolve_workspace(&data_dir)
        .map_err(|error| eframe::Error::AppCreation(Box::new(error)))?;
    let options = eframe::NativeOptions {
        persistence_path: Some(data_dir.join("window.ron")),
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 880.0])
            .with_min_inner_size([1080.0, 680.0]),
        ..Default::default()
    };

    eframe::run_native(
        "vfx-launcher",
        options,
        Box::new(move |cc| Ok(Box::new(VfxLauncherApp::new(cc, data_dir, workspace_dir)))),
    )
}

fn apply_desktop_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.visuals = egui::Visuals::dark();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(7.0, 3.0);
        style.spacing.interact_size.y = 20.0;
        style.spacing.indent = 14.0;

        style.visuals.panel_fill = PANEL_SURFACE;
        style.visuals.window_fill = PANEL_SURFACE;
        style.visuals.extreme_bg_color = EDITOR_SURFACE;
        style.visuals.faint_bg_color = CONTROL_SURFACE;
        style.visuals.selection.bg_fill = ACCENT;
        style.visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
        style.visuals.widgets.inactive.weak_bg_fill = CONTROL_SURFACE;
        style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(0x3a, 0x3d, 0x42);
        style.visuals.widgets.active.weak_bg_fill = ACCENT;

        style.text_styles = [
            (
                egui::TextStyle::Heading,
                egui::FontId::new(15.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Body,
                egui::FontId::new(13.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Button,
                egui::FontId::new(13.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Small,
                egui::FontId::new(11.0, egui::FontFamily::Proportional),
            ),
            (
                egui::TextStyle::Monospace,
                egui::FontId::new(12.0, egui::FontFamily::Monospace),
            ),
        ]
        .into();
    });
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum View {
    Projects,
    ShotsAndSequences,
    OrganizeIncoming,
    CopyByFileType,
    BackUpProjects,
    Settings,
}

impl View {
    const WORKSPACE: [Self; 5] = [
        Self::Projects,
        Self::ShotsAndSequences,
        Self::OrganizeIncoming,
        Self::CopyByFileType,
        Self::BackUpProjects,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Projects => "Projects",
            Self::ShotsAndSequences => "Shots & Sequences",
            Self::OrganizeIncoming => "Organize Incoming",
            Self::CopyByFileType => "Copy by File Type",
            Self::BackUpProjects => "Back Up Projects",
            Self::Settings => "Settings",
        }
    }
}

/// What a pane asks the shell to do once the frame's borrows are released.
enum Action {
    SetTab(ProjectList),
    SelectProject(ProjectId),
    ToggleFavorite(ProjectId),
    ProjectInfo(ProjectId),
    UnregisterProject(ProjectId),
    Launch(ProjectFile),
    Reveal(ProjectFile),
    FileInfo(ProjectFile),
    CopyText(String),
    ChooseUser(crate::storage::UserId),
}

pub struct VfxLauncherApp {
    ctx: egui::Context,
    view: View,
    jobs: Jobs,
    workspace: ProjectWorkspace,
    project_filter: String,
    /// Approved Projects layout: the selected project's files are shown under a
    /// Nuke / After Effects tab and the file filter beside it.
    file_tab: FileKind,
    file_filter: String,
    /// Version picked per file group, so one row can stand for every version of a
    /// group. A pick that the current view no longer offers falls back to latest.
    version_choice: HashMap<GroupKey, FileId>,
    incoming: OrganizeIncomingState,
    copy_by_type: CopyByFileTypeState,
    backup: crate::backup_workspace::BackupWorkspace,
    legacy_import: crate::import_workspace::ImportWorkspace,
    about_open: bool,
    history: crate::history::HistoryWindow,
    info_dialog: Option<(String, Vec<String>)>,
    user_edit: Option<crate::storage::UserId>,
    user_name: String,
    user_is_admin: bool,
    user_delete_confirmation: Option<crate::storage::UserId>,
    /// Shots & Sequences editor, template administration and the New project dialog.
    structure: StructureWorkspace,
    /// A close request is pending until the workers and the catalog have stopped.
    closing: bool,
    /// The close command was already sent; no further coordination is wanted.
    close_sent: bool,
}

impl VfxLauncherApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        data_dir: PathBuf,
        workspace_dir: PathBuf,
    ) -> Self {
        apply_desktop_style(&cc.egui_ctx);
        Self {
            ctx: cc.egui_ctx.clone(),
            view: cc
                .storage
                .and_then(|storage| storage.get_string("workspace_view"))
                .and_then(|label| {
                    View::WORKSPACE
                        .into_iter()
                        .chain([View::Settings])
                        .find(|view| view.label() == label)
                })
                .unwrap_or(View::Projects),
            jobs: Jobs::new(),
            workspace: ProjectWorkspace::new(&cc.egui_ctx, workspace_dir),
            project_filter: String::new(),
            file_tab: FileKind::Nuke,
            file_filter: String::new(),
            version_choice: HashMap::new(),
            incoming: OrganizeIncomingState::new(&cc.egui_ctx, data_dir.clone()),
            copy_by_type: CopyByFileTypeState::new(&cc.egui_ctx),
            backup: crate::backup_workspace::BackupWorkspace::new(&cc.egui_ctx, &data_dir),
            legacy_import: crate::import_workspace::ImportWorkspace::new(data_dir.clone()),
            about_open: false,
            history: crate::history::HistoryWindow::default(),
            info_dialog: None,
            user_edit: None,
            user_name: String::new(),
            user_is_admin: false,
            user_delete_confirmation: None,
            structure: StructureWorkspace::new(&cc.egui_ctx, &data_dir),
            closing: false,
            close_sent: false,
        }
    }
}

impl eframe::App for VfxLauncherApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        storage.set_string("workspace_view", self.view.label().to_string());
    }
    /// Runs before `ui` and also while the window is hidden, so job polling and
    /// close coordination never depend on a painted frame.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.workspace.poll(&mut self.jobs);
        self.legacy_import.poll();
        self.structure.poll(&mut self.workspace, &mut self.jobs);
        self.history.poll();
        self.backup.poll(
            self.workspace.catalog_client().as_ref(),
            self.workspace.user().map(|user| user.id),
            &mut self.jobs,
        );
        self.incoming
            .set_media_applications(self.workspace.settings().applications.clone());
        self.incoming.poll(
            self.workspace.catalog_client().as_ref(),
            self.workspace.user().map(|user| user.id),
            &self.jobs,
        );
        self.copy_by_type.poll(
            self.workspace.catalog_client().as_ref(),
            self.workspace.user().map(|user| user.id),
            &self.jobs,
        );
        if self.closing {
            self.jobs.begin_shutdown();
            // An admitted execution run keeps the request queue open until its
            // own journal writes are answered.
            self.workspace.set_workers_busy(self.jobs.is_busy());
            let catalog_pending = self.workspace.shutdown_pending();
            if !self.jobs.is_busy() && !catalog_pending {
                self.closing = false;
                self.close_sent = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else {
                ctx.request_repaint_after(LIVE_POLL_INTERVAL);
                if ctx.input(|input| input.viewport().close_requested()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
            }
            return;
        }

        self.jobs.poll();

        if self.close_sent {
            return;
        }
        if ctx.input(|input| input.viewport().close_requested()) {
            // Keep the window open for the length of the coordinated stop instead
            // of killing queued catalog work with the process.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.closing = true;
            self.backup.prepare_close(&mut self.jobs);
            self.jobs.begin_shutdown();
            self.workspace.set_workers_busy(self.jobs.is_busy());
            self.workspace.begin_shutdown();
            ctx.request_repaint_after(LIVE_POLL_INTERVAL);
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.navigation(ui);
        self.action_status_bar(ui);

        egui::CentralPanel::default().show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            match self.view {
                View::Projects => self.projects_view(ui),
                View::ShotsAndSequences => self.shots_view(ui),
                View::OrganizeIncoming => self.incoming_view(ui),
                View::CopyByFileType => self.copy_by_file_type_view(ui),
                View::BackUpProjects => self.backup_view(ui),
                View::Settings => self.settings_view(ui),
            }
        });

        let ctx = ui.ctx().clone();
        self.new_project_dialog(&ctx);
        self.info_dialog(&ctx);
        self.about_dialog(&ctx);
        self.history
            .show(&ctx, self.workspace.catalog_client().as_ref());
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Non-blocking: signal cancellation and start the catalog's coordinated
        // shutdown. Joining a live worker here could hang the process.
        self.jobs.begin_shutdown();
        self.workspace.begin_shutdown();
    }
}

impl VfxLauncherApp {
    /// The approved workspace uses two compact horizontal bars.
    fn navigation(&mut self, ui: &mut egui::Ui) {
        egui::Panel::top("app_navigation").show(ui, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.label("VFX Launcher");
                ui.separator();
                ui.menu_button("File", |ui| {
                    if ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("View", |ui| {
                    if ui.button("Logs and file-operation history…").clicked() {
                        self.history.open = true;
                        ui.close();
                    }
                    for view in View::WORKSPACE {
                        if ui.button(view.label()).clicked() {
                            self.view = view;
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(View::Settings.label()).clicked() {
                        self.view = View::Settings;
                        ui.close();
                    }
                });
                ui.menu_button("Help", |ui| {
                    if ui.button("About VFX Launcher").clicked() {
                        self.about_open = true;
                        ui.close();
                    }
                });
            });
            ui.separator();

            ui.horizontal(|ui| {
                for view in View::WORKSPACE {
                    self.navigation_button(ui, view);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    self.navigation_button(ui, View::Settings);
                });
            });
        });
    }

    fn navigation_button(&mut self, ui: &mut egui::Ui, view: View) {
        let selected = self.view == view;
        if ui
            .add(egui::Button::new(view.label()).selected(selected))
            .clicked()
        {
            self.view = view;
        }
    }

    fn action_status_bar(&mut self, ui: &mut egui::Ui) {
        let job = self.jobs.current().cloned();
        let closing = self.closing;
        let live_jobs = self.jobs.live_job_count();
        let mut cancel: Option<JobId> = None;
        let mut pause: Option<JobId> = None;
        let mut resume: Option<JobId> = None;
        let mut dismiss = false;

        egui::Panel::bottom("action_status_strip").show(ui, |ui| {
            ui.add_space(3.0);
            if let Some(job) = &job {
                ui.add(
                    egui::ProgressBar::new(job.progress.fraction())
                        .text(progress_label(job))
                        .desired_height(13.0),
                );
                if let Some(current) = &job.progress.current {
                    ui.label(
                        egui::RichText::new(current.display().to_string())
                            .small()
                            .weak(),
                    );
                }
            }

            ui.horizontal(|ui| {
                // Reserve execution controls before status text can consume the strip.
                match self.view {
                    View::CopyByFileType => self.copy_by_type.execution_control(ui, &mut self.jobs),
                    View::BackUpProjects => self.backup.execution_control(ui, &mut self.jobs),
                    View::ShotsAndSequences => self.structure.execution_control(ui, &mut self.jobs),
                    View::OrganizeIncoming => self.incoming.execution_control(ui, &mut self.jobs),
                    _ => {}
                }
                if self.view == View::OrganizeIncoming {
                    let mut move_files = self.incoming.move_files();
                    ui.radio_value(&mut move_files, false, "Copy");
                    ui.radio_value(&mut move_files, true, "Move");
                    if move_files != self.incoming.move_files() {
                        self.incoming.set_move_files(move_files);
                    }
                    ui.separator();
                    self.incoming.validate_control(ui, &mut self.jobs);
                }
                if self.view == View::CopyByFileType {
                    self.copy_by_type.validate_control(ui, &mut self.jobs);
                }
                if self.view == View::BackUpProjects {
                    self.backup.validate_control(ui, &mut self.jobs);
                }
                if self.view == View::ShotsAndSequences {
                    self.structure.validate_control(ui, &mut self.jobs);
                }
                if closing
                    && (self.backup.picker_pending()
                        || self.copy_by_type.picker_pending()
                        || self.structure.picker_pending()
                        || self.incoming.picker_pending()
                        || self.legacy_import.picker_pending()
                        || self.workspace.registration_picker_pending()
                        || self.workspace.settings_browse_pending())
                {
                    ui.label("Close the file chooser to finish quitting.");
                } else if self.view == View::ShotsAndSequences {
                    ui.label(egui::RichText::new(self.structure.status_line()).small());
                } else {
                    ui.label(
                        egui::RichText::new(status_text(job.as_ref(), closing, live_jobs)).small(),
                    );
                }
                if let Some(job) = &job {
                    if job.omitted_diagnostics > 0 {
                        ui.label(
                            egui::RichText::new(format!(
                                "{} diagnostics omitted",
                                job.omitted_diagnostics
                            ))
                            .small()
                            .weak(),
                        );
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(job) = &job {
                        if job.is_terminal() && job.is_reaped() {
                            if ui.small_button("Dismiss").clicked() {
                                dismiss = true;
                            }
                        } else if job.is_terminal() {
                            // Reported, still unwinding its own cleanup: the row
                            // stays visible and cannot be dismissed yet.
                            ui.label(egui::RichText::new("Finishing…").small().weak());
                        } else if self.copy_by_type.is_picker_job(job.id)
                            || self.backup.is_picker_job(job.id)
                            || self.structure.is_picker_job(job.id)
                            || self.incoming.is_picker_job(job.id)
                            || self.legacy_import.is_picker_job(job.id)
                        {
                            ui.label("Waiting for file chooser");
                        } else {
                            if ui.button("Cancel").clicked() {
                                cancel = Some(job.id);
                            }
                            if self.incoming.is_ai_job(job.id) {
                                ui.label("Waiting for Pi");
                            } else if self.incoming.is_nuke_job(job.id) {
                                ui.label("Waiting for Nuke");
                            } else if job.paused || job.pause_requested {
                                if ui.button("Resume").clicked() {
                                    resume = Some(job.id);
                                }
                            } else if ui.button("Pause").clicked() {
                                pause = Some(job.id);
                            }
                        }
                    }
                });
            });
            let blocked = match self.view {
                View::CopyByFileType => self.copy_by_type.execution_blocked_reason(),
                View::BackUpProjects => self.backup.execution_blocked_reason(),
                _ => None,
            };
            if let Some(reason) = blocked {
                ui.label(reason);
            }
            ui.add_space(3.0);
        });

        if let Some(id) = cancel {
            self.jobs.cancel(id);
        }
        if let Some(id) = pause {
            self.jobs.pause(id);
        }
        if let Some(id) = resume {
            self.jobs.resume(id);
        }
        if dismiss {
            self.jobs.clear_finished();
        }
    }

    // ------------------------------------------------------------------ projects

    fn projects_view(&mut self, ui: &mut egui::Ui) {
        let mut actions = Vec::new();
        self.projects_toolbar(ui, &mut actions);
        self.projects_notices(ui);
        ui.separator();

        egui::Panel::left("project_list_pane")
            .resizable(true)
            .default_size(320.0)
            .min_size(200.0)
            .max_size(620.0)
            .show(ui, |ui| {
                project_list_pane(&self.workspace, ui, &mut actions)
            });

        egui::CentralPanel::default().show(ui, |ui| {
            project_files_pane(
                &self.workspace,
                ui,
                &mut actions,
                &mut self.file_tab,
                &mut self.file_filter,
                &mut self.version_choice,
            )
        });

        for action in actions {
            self.apply(action);
        }
    }

    fn projects_toolbar(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            let scan_running = self.workspace.scan_running();
            let scan_ready = self.workspace.actions_enabled() && !scan_running;
            let scan = primary_button(
                ui,
                if scan_running {
                    "Scanning…"
                } else {
                    "Scan folders"
                },
                scan_ready,
            );
            if scan.clicked() {
                self.workspace.begin_scan();
            }
            if scan_running && ui.button("Cancel scan").clicked() {
                self.workspace.cancel_scan(&mut self.jobs);
            }
            if !scan_ready {
                explain(
                    &scan,
                    &if scan_running {
                        "A scan is already running.".to_string()
                    } else {
                        self.action_block_reason()
                    },
                );
            }

            let add = ui.add_enabled(
                self.workspace.actions_enabled() && !self.workspace.registration_busy(),
                egui::Button::new("Add existing folder…"),
            );
            if add.clicked() {
                self.workspace.begin_add_folders();
            }
            if !self.workspace.actions_enabled() {
                explain(&add, &self.action_block_reason());
            }

            ui.separator();
            let filter_enabled = self.workspace.user().is_some();
            let filter = ui.add_enabled(
                filter_enabled,
                egui::TextEdit::singleline(&mut self.project_filter)
                    .hint_text("Filter projects")
                    .desired_width(200.0),
            );
            if filter.changed() {
                self.workspace.set_filter(self.project_filter.clone());
            }

            ui.separator();
            for (tab, label) in [
                (ProjectList::All, "All"),
                (ProjectList::Favorites, "Favorites"),
                (ProjectList::Recents, "Recent"),
            ] {
                let selected = self.workspace.tab() == tab;
                let button = ui.add_enabled(
                    self.workspace.user().is_some(),
                    egui::Button::new(label).selected(selected),
                );
                if button.clicked() {
                    actions.push(Action::SetTab(tab));
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("New project…").clicked() {
                    self.structure.begin_new_project(&self.workspace);
                }
                ui.separator();
                self.user_chooser(ui, actions);
            });
        });
    }

    /// Who the recorded actions belong to. A catalog with several identities and
    /// no recorded choice shows the picker instead of guessing one.
    fn user_chooser(&mut self, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
        let current = self
            .workspace
            .user()
            .map(|user| user.username.clone())
            .unwrap_or_else(|| "Choose user".to_string());
        let users: Vec<(crate::storage::UserId, String)> = self
            .workspace
            .users()
            .iter()
            .map(|user| (user.id, user.username.clone()))
            .collect();
        egui::ComboBox::from_id_salt("active_user")
            .selected_text(egui::RichText::new(current).small())
            .width(150.0)
            .show_ui(ui, |ui| {
                if users.is_empty() {
                    ui.label(egui::RichText::new("No users yet").small().weak());
                }
                for (id, name) in users {
                    let selected = self.workspace.user().map(|user| user.id) == Some(id);
                    if ui.selectable_label(selected, name).clicked() {
                        actions.push(Action::ChooseUser(id));
                    }
                }
            })
            .response
            .on_hover_text(
                "Recorded actions (recent projects, favorites, file access) use this name.",
            );
    }

    fn action_block_reason(&self) -> String {
        self.workspace
            .actions_blocked_reason()
            .unwrap_or_else(|| UNAVAILABLE.to_string())
    }

    fn projects_notices(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.workspace.catalog_error() {
            notice_row(ui, "The project catalog is unavailable.", Some(error), true);
        }
        if let Some(error) = self.workspace.settings_error() {
            notice_row(
                ui,
                "The settings file could not be read.",
                Some(error),
                true,
            );
        }
        if let Some(error) = self.workspace.settings_save_error() {
            notice_row(ui, "Settings save notice", Some(error), true);
        }
        if self.workspace.user().is_none()
            && self.workspace.catalog_error().is_none()
            && !self.workspace.users().is_empty()
        {
            ui.label(
                egui::RichText::new("Choose the name that recorded actions belong to.")
                    .small()
                    .weak(),
            );
        }
        if let Some(error) = self.workspace.projects_error() {
            notice_row(ui, "The project list could not be read.", Some(error), true);
        }

        let mut dismiss_registration = false;
        match self.workspace.registration_status() {
            Some(status) => {
                let line = status.summary_line();
                let error = matches!(status, RegistrationStatus::Failed(_));
                let details = status.detail_lines();
                ui.horizontal(|ui| {
                    if matches!(status, RegistrationStatus::Busy(_)) {
                        ui.spinner();
                    }
                    notice_row(ui, &line, None, error);
                    if !matches!(status, RegistrationStatus::Busy(_))
                        && ui.small_button("Dismiss").clicked()
                    {
                        dismiss_registration = true;
                    }
                });
                if !details.is_empty() {
                    ui.collapsing("Folder details", |ui| {
                        for line in details {
                            ui.label(egui::RichText::new(line).small());
                        }
                    });
                }
            }
            None => {}
        }
        if dismiss_registration {
            self.workspace.dismiss_registration_summary();
        }

        if !self.workspace.scan_lines().is_empty() {
            let summary = format!(
                "{} folder{} scanned",
                self.workspace.scan_lines().len(),
                if self.workspace.scan_lines().len() == 1 {
                    ""
                } else {
                    "s"
                }
            );
            let mut dismiss_scan = false;
            ui.horizontal(|ui| {
                ui.collapsing(summary, |ui| {
                    for line in self.workspace.scan_lines() {
                        ui.label(egui::RichText::new(line).small());
                    }
                });
                if !self.workspace.scan_running() && ui.small_button("Dismiss").clicked() {
                    dismiss_scan = true;
                }
            });
            if dismiss_scan {
                self.workspace.dismiss_scan_summary();
            }
        }

        let notices: Vec<(String, Option<String>, bool)> = self
            .workspace
            .notices()
            .iter()
            .map(|notice| (notice.title.clone(), notice.detail.clone(), notice.error))
            .collect();
        let mut dismiss: Option<usize> = None;
        for (index, (title, detail, error)) in notices.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    notice_row(ui, title, detail.as_deref(), *error);
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    if ui.small_button("Dismiss").clicked() {
                        dismiss = Some(index);
                    }
                });
            });
        }
        if let Some(index) = dismiss {
            self.workspace.dismiss_notice(index);
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::SetTab(tab) => self.workspace.set_tab(tab),
            Action::SelectProject(id) => self.workspace.select_project(id),
            Action::ToggleFavorite(id) => self.workspace.toggle_favorite(id),
            Action::Launch(file) => self.workspace.begin_launch(&file),
            Action::Reveal(file) => self.workspace.reveal(&file),
            Action::CopyText(text) => self.ctx.copy_text(text),
            Action::ChooseUser(id) => self.workspace.choose_user(id),
            Action::UnregisterProject(id) => self.workspace.unregister_project(id),
            Action::ProjectInfo(id) => {
                if let Some(project) = self
                    .workspace
                    .projects()
                    .iter()
                    .find(|project| project.id == id)
                {
                    let mut lines = Vec::new();
                    lines.push(format!("Name: {}", project.name));
                    lines.push(format!(
                        "Folder: {}",
                        project
                            .root
                            .as_ref()
                            .map(|root| root.display().to_string())
                            .or_else(|| project.root_display.clone())
                            .unwrap_or_else(|| UNKNOWN.to_string())
                    ));
                    for folder in &project.scan_folders {
                        lines.push(format!(
                            "Scanned folder: {}",
                            folder
                                .path
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .or_else(|| folder.display.clone())
                                .unwrap_or_else(|| UNKNOWN.to_string())
                        ));
                    }
                    lines.push(format!("Added: {}", project.added_at));
                    lines.push(format!("Favorite: {}", yes_no(project.favorite)));
                    if let Some(accessed) = &project.accessed_at {
                        lines.push(format!("Last opened: {accessed}"));
                    }
                    self.info_dialog = Some((project.name.clone(), lines));
                }
            }
            Action::FileInfo(file) => {
                let mut lines = Vec::new();
                lines.push(format!(
                    "Path: {}",
                    file.path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .or_else(|| file.display.clone())
                        .unwrap_or_else(|| UNKNOWN.to_string())
                ));
                lines.push(format!(
                    "Version: {}",
                    file.version
                        .as_ref()
                        .map(|version| version.text.clone())
                        .unwrap_or_else(|| UNKNOWN.to_string())
                ));
                lines.push(format!(
                    "Type: {}",
                    file.filetype.clone().unwrap_or_else(|| UNKNOWN.to_string())
                ));
                lines.push(format!(
                    "Size: {}",
                    file.size_bytes
                        .map(format_size)
                        .unwrap_or_else(|| UNKNOWN.to_string())
                ));
                lines.push(format!(
                    "Modified: {}",
                    file.modified_unix_ns
                        .map(|nanos| crate::storage::utc_timestamp_signed(
                            nanos.div_euclid(1_000_000_000)
                        ))
                        .unwrap_or_else(|| UNKNOWN.to_string())
                ));
                lines.push(format!(
                    "Last opened: {}",
                    file.last_opened_at.as_deref().unwrap_or(UNKNOWN)
                ));
                lines.push(format!(
                    "Opened by: {}",
                    file.opened_by.as_deref().unwrap_or(UNKNOWN)
                ));
                lines.push(format!(
                    "Last scanned: {}",
                    file.last_seen_at
                        .clone()
                        .unwrap_or_else(|| UNKNOWN.to_string())
                ));
                self.info_dialog = Some(("File information".to_string(), lines));
            }
        }
    }

    // ------------------------------------------------------------------ views

    fn shots_view(&mut self, ui: &mut egui::Ui) {
        self.structure.view(ui, &mut self.jobs);
    }

    fn incoming_view(&mut self, ui: &mut egui::Ui) {
        self.incoming.view(ui, &mut self.jobs);
    }

    fn copy_by_file_type_view(&mut self, ui: &mut egui::Ui) {
        self.copy_by_type.view(ui, &mut self.jobs);
    }

    fn backup_view(&mut self, ui: &mut egui::Ui) {
        self.backup.view(ui, &mut self.jobs);
    }

    fn settings_view(&mut self, ui: &mut egui::Ui) {
        self.settings_action_strip(ui);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.settings_form(ui);
                ui.add_space(6.0);
                self.settings_users_section(ui);
                ui.collapsing("Import previous Launcher data", |ui| {
                    self.legacy_import.ui(ui, &mut self.jobs);
                });
                ui.collapsing("Logs", |ui| {
                    if ui.button("Open logs and file-operation history…").clicked() {
                        self.history.open = true;
                    }
                    if let Some(job) = self.jobs.current() {
                        ui.label(&job.title);
                        for diagnostic in &job.diagnostics {
                            ui.label(diagnostic);
                        }
                    }
                });
                ui.collapsing("Data location", |ui| {
                    ui.label(
                        egui::RichText::new(self.workspace.data_dir().display().to_string())
                            .monospace()
                            .small(),
                    );
                    if let Some(error) = self.workspace.settings_error() {
                        ui.label(egui::RichText::new(error).small());
                    }
                });
            });
    }

    /// Save and Cancel stay visible under the form. Both act only on the draft: the
    /// acknowledged settings change when the worker reports the atomic write, never
    /// when a button is pressed.
    fn settings_action_strip(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("settings_actions").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let loaded = self.workspace.settings_loaded();
                let dirty = self.workspace.settings_dirty();
                let saving = self.workspace.settings_saving();

                let save = primary_button(
                    ui,
                    if saving { "Saving…" } else { "Save" },
                    loaded && dirty && !saving,
                );
                if save.clicked() {
                    self.workspace.save_settings_draft();
                }
                if !loaded {
                    explain(
                        &save,
                        "The settings file could not be read; it is left untouched.",
                    );
                } else if !dirty {
                    explain(&save, "No unsaved changes.");
                }

                let cancel = ui.add_enabled(dirty && !saving, egui::Button::new("Cancel"));
                if cancel.clicked() {
                    self.workspace.cancel_settings_draft();
                }
                explain(&cancel, "Discard the edits and keep the saved settings.");

                if dirty && !saving {
                    ui.label(egui::RichText::new("Unsaved changes").small());
                }
                if let Some(error) = self.workspace.settings_edit_error() {
                    ui.label(egui::RichText::new(error).small().color(PROBLEM));
                }
                if let Some(error) = self.workspace.settings_save_error() {
                    ui.label(egui::RichText::new(error).small().color(PROBLEM));
                }
            });
            ui.add_space(4.0);
        });
    }

    fn settings_form(&mut self, ui: &mut egui::Ui) {
        let loaded = self.workspace.settings_loaded();
        let saving = self.workspace.settings_saving();
        let browse_open = self.workspace.settings_browse_pending();
        let mut browse: Option<SettingsPath> = None;
        let mut edited = false;

        // Locked while a write is in flight: the draft then always holds exactly the
        // values that write carries, so a close-time flush cannot persist an edit
        // the user has not saved.
        ui.add_enabled_ui(loaded && !saving, |ui| {
            section_label(ui, "Applications");
            egui::Grid::new("settings_applications")
                .num_columns(2)
                .spacing(egui::vec2(14.0, 6.0))
                .show(ui, |ui| {
                    let Some(draft) = self.workspace.settings_draft_mut() else {
                        return;
                    };
                    let (nuke_browse, nuke_edited) = settings_path_row(
                        ui,
                        "Nuke application",
                        &mut draft.nuke_application,
                        "/opt/Nuke17.0v1/Nuke17.0 or nuke",
                        !browse_open && !saving,
                    );
                    if nuke_browse {
                        browse = Some(SettingsPath::Nuke);
                    }
                    edited |= nuke_edited;
                    ui.end_row();
                    let (effects_browse, effects_edited) = settings_path_row(
                        ui,
                        "After Effects application",
                        &mut draft.after_effects_application,
                        "/opt/AfterFX or AfterFX",
                        !browse_open && !saving,
                    );
                    if effects_browse {
                        browse = Some(SettingsPath::AfterEffects);
                    }
                    edited |= effects_edited;
                    ui.end_row();
                    for (label, field, target) in [
                        (
                            "DJV application",
                            &mut draft.djv_application,
                            SettingsPath::Djv,
                        ),
                        (
                            "MRV2 application",
                            &mut draft.mrv2_application,
                            SettingsPath::Mrv2,
                        ),
                    ] {
                        let (picked, changed) = settings_path_row(
                            ui,
                            label,
                            field,
                            "Application path or program name",
                            !browse_open && !saving,
                        );
                        if picked {
                            browse = Some(target);
                        }
                        edited |= changed;
                        ui.end_row();
                    }
                    ui.label("Nuke launch command (Linux)");
                    edited |= ui
                        .add(
                            egui::TextEdit::singleline(&mut draft.nuke_launch_command)
                                .hint_text(
                                    "ghostty -e env LC_NUMERIC=C /opt/Nuke17.0v1/Nuke17.0 --nukex",
                                )
                                .desired_width(SETTINGS_FIELD_WIDTH)
                                .font(egui::TextStyle::Monospace),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("");
                    ui.label(
                        egui::RichText::new("Optional command used to start Nuke on Linux.")
                            .small()
                            .weak(),
                    );
                    ui.end_row();
                });

            ui.add_space(8.0);
            section_label(ui, "Project folders");
            egui::Grid::new("settings_projects")
                .num_columns(2)
                .spacing(egui::vec2(14.0, 6.0))
                .show(ui, |ui| {
                    let Some(draft) = self.workspace.settings_draft_mut() else {
                        return;
                    };
                    ui.label("Folder names");
                    edited |= ui
                        .add(
                            egui::TextEdit::singleline(&mut draft.project_folder_names)
                                .hint_text("project, projects")
                                .desired_width(SETTINGS_FIELD_WIDTH),
                        )
                        .changed();
                    ui.end_row();
                    ui.label("");
                    ui.label(
                        egui::RichText::new(
                            "Separate folder names with commas, for example: project, projects.",
                        )
                        .small()
                        .weak(),
                    );
                    ui.end_row();
                });
        });

        if let Some(field) = browse {
            self.workspace.begin_settings_browse(field);
        }
        if edited {
            self.workspace.clear_settings_edit_error();
        }
        if !loaded {
            ui.label(
                egui::RichText::new(
                    "Editing is disabled because the settings file could not be read; it is never overwritten.",
                )
                .small()
                .color(PROBLEM),
            );
        }
    }

    fn settings_users_section(&mut self, ui: &mut egui::Ui) {
        let mut actions = Vec::new();
        let mut save = None;
        let mut remove = None;
        ui.collapsing("Users", |ui| {
            if self.workspace.users().is_empty() {
                ui.label(egui::RichText::new("No users yet.").small().weak());
            }
            let editable = self.workspace.actions_enabled() && !self.workspace.users_busy();
            for user in self.workspace.users() {
                let current = self.workspace.user().map(|current| current.id) == Some(user.id);
                let mut label = user.username.clone();
                if user.is_admin {
                    label.push_str(" (admin)");
                }
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(self.user_edit == Some(user.id), label)
                        .clicked()
                    {
                        self.user_edit = Some(user.id);
                        self.user_name = user.username.clone();
                        self.user_is_admin = user.is_admin;
                        self.user_delete_confirmation = None;
                    }
                    if current {
                        ui.label("Current user");
                    } else if ui
                        .add_enabled(editable, egui::Button::new("Use for history"))
                        .clicked()
                    {
                        actions.push(Action::ChooseUser(user.id));
                    }
                    if !current && self.workspace.users().len() > 1 {
                        if self.user_delete_confirmation == Some(user.id) {
                            if ui
                                .add_enabled(editable, egui::Button::new("Confirm remove"))
                                .clicked()
                            {
                                remove = Some(user.id);
                            }
                            if ui.button("Cancel").clicked() {
                                self.user_delete_confirmation = None;
                            }
                        } else if ui
                            .add_enabled(editable, egui::Button::new("Remove"))
                            .clicked()
                        {
                            self.user_delete_confirmation = Some(user.id);
                        }
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.add_enabled(
                    editable,
                    egui::TextEdit::singleline(&mut self.user_name).desired_width(240.0),
                );
                ui.add_enabled(
                    editable,
                    egui::Checkbox::new(&mut self.user_is_admin, "Admin"),
                );
                if ui
                    .add_enabled(
                        editable && !self.user_name.trim().is_empty(),
                        egui::Button::new("Add user"),
                    )
                    .clicked()
                {
                    save = Some(None);
                }
                if ui
                    .add_enabled(
                        editable && self.user_edit.is_some() && !self.user_name.trim().is_empty(),
                        egui::Button::new("Save name / role"),
                    )
                    .clicked()
                {
                    save = Some(self.user_edit);
                }
            });
            ui.label("Removing a name keeps its earlier file history.");
            if let Some(notice) = self
                .workspace
                .notices()
                .last()
                .filter(|notice| notice.error && notice.title.starts_with("User"))
            {
                ui.label(egui::RichText::new(&notice.title).color(PROBLEM));
                if let Some(detail) = &notice.detail {
                    ui.label(egui::RichText::new(detail).color(PROBLEM));
                }
            }
            if self.workspace.users_busy() {
                ui.label("Saving user…");
            }
        });
        for action in actions {
            self.apply(action);
        }
        if let Some(id) = save {
            self.workspace
                .save_user(id, self.user_name.clone(), self.user_is_admin);
        }
        if let Some(id) = remove {
            self.workspace.remove_user(id);
            self.user_delete_confirmation = None;
        }
    }

    fn new_project_dialog(&mut self, ctx: &egui::Context) {
        self.structure
            .new_project_dialog(ctx, &mut self.workspace, &mut self.jobs);
        self.structure.templates_dialog(ctx, &mut self.jobs);
    }

    fn info_dialog(&mut self, ctx: &egui::Context) {
        let Some((title, lines)) = self.info_dialog.take() else {
            return;
        };
        let mut open = true;
        egui::Window::new(title.clone())
            .collapsible(false)
            .resizable(true)
            .open(&mut open)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for line in &lines {
                            ui.label(egui::RichText::new(line).small().monospace());
                        }
                    });
                if ui.button("Copy").clicked() {
                    ctx.copy_text(lines.join("\n"));
                }
            });
        if open {
            self.info_dialog = Some((title, lines));
        }
    }

    fn about_dialog(&mut self, ctx: &egui::Context) {
        if !self.about_open {
            return;
        }

        let mut open = true;
        egui::Window::new("About VFX Launcher")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.label(
                    egui::RichText::new(
                        "Project catalog, file launch, Shots & Sequences additions, new-project creation, incoming organization, file-type copy, backup and settings editing are wired to the shared transfer engine.",
                    )
                    .small()
                    .weak(),
                );
            });
        self.about_open = open;
    }
}

// ---------------------------------------------------------------------- panes

fn project_list_pane(workspace: &ProjectWorkspace, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    if workspace.catalog_error().is_some() || workspace.user().is_none() {
        empty_state(
            ui,
            "Projects are unavailable.",
            "The catalog or the active user must be available first.",
        );
        return;
    }
    if workspace.projects().is_empty() {
        empty_state(
            ui,
            "No projects yet.",
            "Add an existing project folder to get started.",
        );
        return;
    }

    let projects = workspace.projects();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for project in projects {
            let selected = workspace.selected() == Some(project.id);
            let mut label = project.name.clone();
            if project.favorite {
                label.push_str("  *");
            }
            let response = ui.selectable_label(selected, label);
            let response = response.on_hover_text(project_path(project));
            if response.clicked() {
                actions.push(Action::SelectProject(project.id));
            }
            row_menu(ui, &response, |ui| {
                if ui
                    .button(if project.favorite {
                        "Remove from favorites"
                    } else {
                        "Add to favorites"
                    })
                    .clicked()
                {
                    actions.push(Action::ToggleFavorite(project.id));
                    ui.close();
                }
                if ui.button("Copy folder path").clicked() {
                    actions.push(Action::CopyText(project_path(project)));
                    ui.close();
                }
                if ui.button("Project information").clicked() {
                    actions.push(Action::ProjectInfo(project.id));
                    ui.close();
                }
                if ui.button("Unregister project (keep files)").clicked() {
                    actions.push(Action::UnregisterProject(project.id));
                    ui.close();
                }
            });
        }
    });
}

fn project_files_pane(
    workspace: &ProjectWorkspace,
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
    tab: &mut FileKind,
    filter: &mut String,
    choices: &mut HashMap<GroupKey, FileId>,
) {
    let Some(project) = workspace.selected_project() else {
        empty_state(
            ui,
            "No project selected.",
            "Select a project to see its Nuke and After Effects files.",
        );
        return;
    };

    // File tabs and the file filter sit above the files pane; both narrow the
    // grouped rows without merging or re-grouping a bucket.
    ui.horizontal(|ui| {
        for kind in [FileKind::Nuke, FileKind::AfterEffects] {
            let selected = *tab == kind;
            if ui
                .add(egui::Button::new(file_kind_label(kind)).selected(selected))
                .clicked()
            {
                *tab = kind;
            }
        }
        ui.separator();
        ui.add(
            egui::TextEdit::singleline(filter)
                .hint_text("Filter files")
                .desired_width(200.0),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if let Some(error) = workspace.files_error() {
                ui.label(egui::RichText::new(error).small());
            }
            ui.label(egui::RichText::new(project.name.clone()).small().weak());
        });
    });
    ui.separator();

    if workspace.files().is_empty() {
        empty_state(
            ui,
            "No files in this project yet.",
            "Scan folders to record its Nuke and After Effects files.",
        );
        return;
    }

    let groups = visible_groups(workspace.files(), *tab, filter);
    if groups.is_empty() {
        empty_state(
            ui,
            "No matching files.",
            &format!(
                "This project has no {} files under the current filter. Clear the filter or switch tabs.",
                file_kind_label(*tab)
            ),
        );
        return;
    }

    let actions_enabled = workspace.actions_enabled();
    let project_id = project.id;
    let extension = file_kind_extension(*tab);
    let mut previous_bucket: Option<(Option<String>, Option<String>)> = None;

    egui_extras::TableBuilder::new(ui)
        .id_salt("project_files_table")
        .resizable(true)
        .column(
            egui_extras::Column::initial(360.0)
                .at_least(180.0)
                .clip(true),
        )
        .column(egui_extras::Column::initial(92.0).at_least(72.0).clip(true))
        .column(egui_extras::Column::initial(78.0).at_least(66.0))
        .column(
            egui_extras::Column::initial(120.0)
                .at_least(90.0)
                .clip(true),
        )
        .column(
            egui_extras::Column::initial(110.0)
                .at_least(80.0)
                .clip(true),
        )
        .column(egui_extras::Column::initial(90.0).at_least(60.0).clip(true))
        // The file column keeps a fixed starting width so file, version and Launch
        // stay together on wide windows; the free space trails the columns instead.
        .column(egui_extras::Column::remainder())
        .header(20.0, |mut header| {
            for title in [
                "File",
                "Version",
                "Launch",
                "Modified",
                "Last opened",
                "By",
                "",
            ] {
                header.col(|ui| {
                    ui.label(title);
                });
            }
        })
        .body(|mut body| {
            for (group, files) in &groups {
                let bucket = (group.timeline.clone(), group.shot.clone());
                if previous_bucket.as_ref() != Some(&bucket) {
                    previous_bucket = Some(bucket.clone());
                    let label = bucket_label(&bucket);
                    body.row(18.0, |mut row| {
                        row.col(|ui| {
                            ui.label(egui::RichText::new(label).small().weak());
                        });
                        for _ in 0..6 {
                            row.col(|_ui| {});
                        }
                    });
                }
                let key = GroupKey::of(project_id, group, extension);
                let Some(file) = chosen_file(choices.get(&key).copied(), group.latest, files)
                else {
                    continue;
                };
                let mut picked: Option<FileId> = None;
                body.row(22.0, |mut row| {
                    row.col(|ui| {
                        let name = group.base_name.clone().unwrap_or_else(|| file_name(file));
                        let response = ui.selectable_label(false, name);
                        let response = response.on_hover_text(file_path(file));
                        file_menu(ui, &response, workspace, file, actions);
                    });
                    row.col(|ui| {
                        if files.len() > 1 || file.version.is_some() {
                            egui::ComboBox::from_id_salt(ui.make_persistent_id(&key))
                                .selected_text(version_text(file))
                                .width(70.0)
                                .show_ui(ui, |ui| {
                                    for candidate in files.iter().copied() {
                                        let selected = candidate.file == file.file;
                                        if ui
                                            .selectable_label(selected, version_text(candidate))
                                            .clicked()
                                        {
                                            picked = Some(candidate.file);
                                        }
                                    }
                                });
                        } else {
                            ui.label(version_text(file));
                        }
                    });
                    row.col(|ui| {
                        let tool = file.filetype.as_deref().and_then(Tool::from_filetype);
                        let ready = actions_enabled && tool.is_some() && file.path.is_some();
                        let button = ui.add_enabled(ready, egui::Button::new("Launch").small());
                        if button.clicked() {
                            actions.push(Action::Launch(file.clone()));
                        }
                        if !ready {
                            let reason = if tool.is_none() {
                                "This file type has no configured application."
                            } else if file.path.is_none() {
                                "This file has no usable path on this machine."
                            } else {
                                "The catalog is not ready yet."
                            };
                            button.on_hover_text(reason);
                        }
                    });
                    row.col(|ui| {
                        ui.label(modified_text(file));
                    });
                    row.col(|ui| {
                        ui.label(file.last_opened_at.as_deref().unwrap_or(UNKNOWN));
                    });
                    row.col(|ui| {
                        ui.label(file.opened_by.as_deref().unwrap_or(UNKNOWN));
                    });
                    row.col(|_ui| {});
                });
                if let Some(picked) = picked {
                    choices.insert(key, picked);
                }
            }
        });
}

fn file_menu(
    ui: &mut egui::Ui,
    response: &egui::Response,
    workspace: &ProjectWorkspace,
    file: &ProjectFile,
    actions: &mut Vec<Action>,
) {
    let path = file_path(file);
    let ready = workspace.actions_enabled();
    row_menu(ui, response, |ui| {
        let launch = ui.add_enabled(
            ready && file.path.is_some() && file.filetype.is_some(),
            egui::Button::new("Launch"),
        );
        if launch.clicked() {
            actions.push(Action::Launch(file.clone()));
            ui.close();
        }
        let reveal = ui.add_enabled(
            file.path.is_some(),
            egui::Button::new("Reveal in file manager"),
        );
        if reveal.clicked() {
            actions.push(Action::Reveal(file.clone()));
            ui.close();
        }
        if ui.button("Copy path").clicked() {
            actions.push(Action::CopyText(path));
            ui.close();
        }
        if ui.button("Information").clicked() {
            actions.push(Action::FileInfo(file.clone()));
            ui.close();
        }
    });
}

/// Right-click context menu on a row, plus `Shift+F10` when that row has focus,
/// so the same actions are reachable without a mouse.
fn row_menu(ui: &mut egui::Ui, response: &egui::Response, contents: impl FnOnce(&mut egui::Ui)) {
    let keyboard = response.has_focus()
        && ui.input(|input| {
            input.key_pressed(egui::Key::Menu)
                || (input.key_pressed(egui::Key::F10) && input.modifiers.shift)
        });
    let open = if response.secondary_clicked() || keyboard {
        Some(egui::SetOpenCommand::Bool(true))
    } else if response.clicked() {
        Some(egui::SetOpenCommand::Bool(false))
    } else {
        None
    };
    let mut popup = egui::Popup::menu(response)
        .open_memory(open)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClick);
    if !keyboard {
        popup = popup.at_pointer_fixed();
    }
    popup.show(|ui| {
        ui.set_min_width(180.0);
        contents(ui);
    });
}

// -------------------------------------------------------------------- helpers

fn project_path(project: &crate::projects::Project) -> String {
    project
        .root
        .as_ref()
        .map(|root| root.display().to_string())
        .or_else(|| project.root_display.clone())
        .unwrap_or_else(|| "Unresolved folder on this machine".to_string())
}

fn file_path(file: &ProjectFile) -> String {
    file.path
        .as_ref()
        .map(|path| path.display().to_string())
        .or_else(|| file.display.clone())
        .unwrap_or_else(|| "Unresolved path on this machine".to_string())
}

fn file_name(file: &ProjectFile) -> String {
    file.filename
        .clone()
        .or_else(|| file.display.clone())
        .unwrap_or_else(|| UNKNOWN.to_string())
}

fn bucket_label(bucket: &(Option<String>, Option<String>)) -> String {
    match (&bucket.0, &bucket.1) {
        (Some(timeline), Some(shot)) if !timeline.is_empty() => format!("{timeline} / {shot}"),
        (_, Some(shot)) => shot.clone(),
        (Some(timeline), None) if !timeline.is_empty() => timeline.clone(),
        _ => "Unsorted".to_string(),
    }
}

fn modified_text(file: &ProjectFile) -> String {
    match file.modified_unix_ns {
        Some(nanos) => chrono::DateTime::from_timestamp(
            nanos.div_euclid(1_000_000_000),
            nanos.rem_euclid(1_000_000_000) as u32,
        )
        .map(|stamp| {
            stamp
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_else(|| UNKNOWN.to_string()),
        None => UNKNOWN.to_string(),
    }
}

fn format_size(bytes: u64) -> String {
    let bytes = bytes as f64;
    if bytes >= 1024.0 * 1024.0 {
        format!("{:.1} MB", bytes / (1024.0 * 1024.0))
    } else if bytes >= 1024.0 {
        format!("{:.1} KB", bytes / 1024.0)
    } else {
        format!("{bytes:.0} B")
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let text = egui::RichText::new(label).color(egui::Color32::WHITE);
    ui.add_enabled(enabled, egui::Button::new(text).fill(ACCENT))
}

/// Attaches the one explanation a disabled control gets.
fn explain(response: &egui::Response, reason: &str) {
    let _ = response.clone().on_hover_text(reason);
}

fn notice_row(ui: &mut egui::Ui, title: &str, detail: Option<&str>, error: bool) {
    ui.horizontal(|ui| {
        let text = egui::RichText::new(title).small();
        if error {
            ui.label(text.color(egui::Color32::from_rgb(0xe0, 0x9a, 0x6a)));
        } else {
            ui.label(text);
        }
        if let Some(detail) = detail {
            ui.label(
                egui::RichText::new(detail.lines().next().unwrap_or(""))
                    .small()
                    .weak(),
            );
        }
    });
    if let Some(detail) = detail {
        if detail.lines().count() > 1 || detail.len() > 80 {
            ui.collapsing("Details", |ui| {
                for line in detail.lines() {
                    ui.label(egui::RichText::new(line).small().monospace());
                }
            });
        }
    }
}

/// Width shared by the editable Settings fields.
const SETTINGS_FIELD_WIDTH: f32 = 360.0;
/// Error copy colour used by the settings strip.
const PROBLEM: egui::Color32 = egui::Color32::from_rgb(0xe0, 0x9a, 0x6a);

fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(text).small().weak());
    ui.add_space(2.0);
}

/// One application path with its native browse button. Returns `(browse_clicked,
/// edited)`; `browse_enabled` is false while a chooser is already open.
fn settings_path_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    hint: &str,
    browse_enabled: bool,
) -> (bool, bool) {
    ui.label(label);
    ui.horizontal(|ui| {
        let edited = ui
            .add(
                egui::TextEdit::singleline(value)
                    .hint_text(hint)
                    .desired_width(SETTINGS_FIELD_WIDTH)
                    .font(egui::TextStyle::Monospace),
            )
            .changed();
        let browse = ui
            .add_enabled(browse_enabled, egui::Button::new("Browse…"))
            .clicked();
        (browse, edited)
    })
    .inner
}

fn empty_state(ui: &mut egui::Ui, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(14.0);
        ui.label(title);
        ui.add_space(2.0);
        ui.label(egui::RichText::new(hint).color(egui::Color32::from_gray(180)));
    });
}

fn file_kind_label(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Nuke => "Nuke",
        FileKind::AfterEffects => "After Effects",
    }
}

/// The stored `filetype` a tab shows, matching the catalog's own file-kind filter.
fn file_kind_extension(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Nuke => "nk",
        FileKind::AfterEffects => "aep",
    }
}

/// Identity of one version list: a project's file group inside one file-type tab.
/// Timeline and shot are part of it so a version picked in one bucket never
/// shows up in another, and the project keeps picks per project.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct GroupKey {
    project: ProjectId,
    timeline: Option<String>,
    shot: Option<String>,
    base: Option<String>,
    extension: &'static str,
}

impl GroupKey {
    fn of(project: ProjectId, group: &FileGroup, extension: &'static str) -> Self {
        Self {
            project,
            timeline: group.timeline.clone(),
            shot: group.shot.clone(),
            base: group.base_name.clone(),
            extension,
        }
    }
}

/// The version a group row shows: the stored pick while the current tab and file
/// filter still offer it, otherwise the group's latest visible file. A pick that a
/// rescan or refresh removed therefore falls back instead of blocking the row.
fn chosen_file<'a>(
    chosen: Option<FileId>,
    latest: Option<FileId>,
    files: &[&'a ProjectFile],
) -> Option<&'a ProjectFile> {
    if let Some(chosen) = chosen {
        if let Some(file) = files.iter().copied().find(|file| file.file == chosen) {
            return Some(file);
        }
    }
    files
        .iter()
        .copied()
        .find(|file| Some(file.file) == latest)
        .or_else(|| files.first().copied())
}

fn version_text(file: &ProjectFile) -> String {
    file.version
        .as_ref()
        .map(|version| version.text.clone())
        .unwrap_or_else(|| UNKNOWN.to_string())
}

/// The rows one tab and the file filter keep. Buckets are filtered, never merged
/// or re-grouped: each surviving group keeps its own timeline/shot label, its row
/// order and its context menu, and an emptied group is dropped. The returned files
/// are the versions the group's version selector offers.
fn visible_groups<'a>(
    groups: &'a [FileGroup],
    kind: FileKind,
    filter: &str,
) -> Vec<(&'a FileGroup, Vec<&'a ProjectFile>)> {
    let needle = crate::paths::fold_native_text(filter.trim().as_bytes());
    let extension = file_kind_extension(kind);
    let mut visible = Vec::new();
    for group in groups {
        let files: Vec<&ProjectFile> = group
            .files
            .iter()
            .filter(|file| {
                file.filetype
                    .as_deref()
                    .is_some_and(|filetype| filetype.eq_ignore_ascii_case(extension))
            })
            .filter(|file| needle.is_empty() || file_matches(file, &needle))
            .collect();
        if !files.is_empty() {
            visible.push((group, files));
        }
    }
    visible
}

fn file_matches(file: &ProjectFile, needle: &[u8]) -> bool {
    let name = file_name(file);
    let version = file
        .version
        .as_ref()
        .map(|version| version.text.clone())
        .unwrap_or_default();
    [name.as_str(), version.as_str()]
        .iter()
        .any(|text| folded_contains(text, needle))
}

fn folded_contains(text: &str, needle: &[u8]) -> bool {
    crate::paths::fold_native_text(text.as_bytes())
        .windows(needle.len())
        .any(|window| window == needle)
}

fn status_text(job: Option<&JobStatus>, closing: bool, live_jobs: usize) -> String {
    if closing {
        return match live_jobs {
            0 => "Closing".to_owned(),
            1 => "Closing — waiting for 1 worker to stop".to_owned(),
            count => format!("Closing — waiting for {count} workers to stop"),
        };
    }

    let Some(job) = job else {
        return "Ready".to_owned();
    };

    match job.outcome {
        Some(outcome) if !job.is_reaped() => {
            format!("{} — {} (finishing)", job.title, outcome.label())
        }
        Some(outcome) => format!("{} — {}", job.title, outcome.label()),
        None if job.cancelling => format!("{} — cancelling", job.title),
        None if job.paused => format!("{} — paused", job.title),
        None if job.is_pausing() => format!("{} — pausing", job.title),
        None => format!("{} — {}", job.title, job.phase.label()),
    }
}

fn progress_label(job: &JobStatus) -> String {
    if let Some(outcome) = job.outcome {
        return outcome.label().to_string();
    }
    match job.progress.total {
        Some(total) => format!("{} {}/{}", job.phase.label(), job.progress.completed, total),
        None if job.progress.completed > 0 => {
            format!("{} {}", job.phase.label(), job.progress.completed)
        }
        None => job.phase.label().to_string(),
    }
}
