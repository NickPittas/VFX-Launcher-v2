//! Application shell: approved desktop composition, six working views and the
//! pinned action/status strip.
//!
//! The layout follows `DESIGN-APPROVAL.md` and `USER-UI-REQUIREMENTS.md`: menu
//! bar, workspace strip, docked resizable panes with internal scrolling, and a
//! persistent bottom status/execution strip. No view holds data yet, so every
//! list renders its explicit empty state instead of sample content.

use eframe::egui;

use crate::jobs::{JobId, JobStatus, Jobs};

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2f, 0x6b, 0xa8);
const PANEL_SURFACE: egui::Color32 = egui::Color32::from_rgb(0x24, 0x25, 0x28);
const EDITOR_SURFACE: egui::Color32 = egui::Color32::from_rgb(0x1a, 0x1b, 0x1d);
const CONTROL_SURFACE: egui::Color32 = egui::Color32::from_rgb(0x2d, 0x2f, 0x33);
const UNAVAILABLE: &str = "Not available in this build.";

/// Starts the native desktop application.
pub fn run_gui() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 880.0])
            .with_min_inner_size([1080.0, 680.0]),
        ..Default::default()
    };

    eframe::run_native(
        "vfx-launcher",
        options,
        Box::new(|cc| Ok(Box::new(VfxLauncherApp::new(cc)))),
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

    /// Execution control pinned in the bottom strip. `None` for views whose
    /// execution lives on a row (Project file launch) or that execute nothing.
    fn primary_action(self) -> Option<&'static str> {
        match self {
            Self::ShotsAndSequences => Some("Add to project"),
            Self::OrganizeIncoming => Some("Copy selected"),
            Self::CopyByFileType => Some("Copy selected"),
            Self::BackUpProjects => Some("Back up now"),
            Self::Projects | Self::Settings => None,
        }
    }
}

// Presentation rows for the empty catalog state. P1 replaces these with catalog types.
#[derive(Clone, Debug, Default)]
struct ProjectRow {
    name: String,
    files: u64,
}

#[derive(Clone, Debug, Default)]
struct FileRow {
    file: String,
    version: String,
    modified: String,
    last_opened: String,
    by: String,
}

#[derive(Clone, Debug, Default)]
struct NewProjectDraft {
    name: String,
}

pub struct VfxLauncherApp {
    view: View,
    jobs: Jobs,
    projects: Vec<ProjectRow>,
    selected_project: Option<usize>,
    files: Vec<FileRow>,
    project_filter: String,
    incoming_source: String,
    incoming_destination: String,
    incoming_move: bool,
    copy_source: String,
    copy_destination: String,
    copy_types: String,
    backup_source: String,
    backup_destination: String,
    backup_exclusions: String,
    about_open: bool,
    new_project: Option<NewProjectDraft>,
    /// A close request is pending until every worker thread has been reaped.
    closing: bool,
}

impl Default for VfxLauncherApp {
    fn default() -> Self {
        Self {
            view: View::Projects,
            jobs: Jobs::new(),
            projects: Vec::new(),
            selected_project: None,
            files: Vec::new(),
            project_filter: String::new(),
            incoming_source: String::new(),
            incoming_destination: String::new(),
            incoming_move: false,
            copy_source: String::new(),
            copy_destination: String::new(),
            copy_types: String::new(),
            backup_source: String::new(),
            backup_destination: String::new(),
            backup_exclusions: String::new(),
            about_open: false,
            new_project: None,
            closing: false,
        }
    }
}

impl VfxLauncherApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        apply_desktop_style(&cc.egui_ctx);
        Self::default()
    }
}

impl eframe::App for VfxLauncherApp {
    /// Runs before `ui` and also while the window is hidden, so job polling and
    /// close coordination never depend on a painted frame.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.closing {
            // Re-signal every frame so work started while the window is closing is
            // cancelled too, and keep reaping on the same non-blocking poll.
            self.jobs.begin_shutdown();
        } else {
            self.jobs.poll();
        }
        self.coordinate_close(ctx);
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
        self.about_dialog(&ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Non-blocking: signal cancellation and reap only threads that have
        // already finished. Joining a live worker here could hang the process.
        self.jobs.begin_shutdown();
    }
}

impl VfxLauncherApp {
    /// Delays the window close until every worker thread has been reaped.
    ///
    /// The first close request is cancelled, cancellation is signalled and the
    /// status strip reports the wait. Only already-finished threads are joined,
    /// on this same non-blocking poll, and the close is re-issued once no worker
    /// remains. A worker blocked in OS I/O cannot be interrupted: the window stays
    /// open and says so instead of claiming a timeout guarantee.
    fn coordinate_close(&mut self, ctx: &egui::Context) {
        if self.closing {
            if !self.jobs.is_busy() {
                self.closing = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if ctx.input(|input| input.viewport().close_requested()) {
                // A repeated close request must not force the window shut while
                // workers are still running.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            return;
        }

        if ctx.input(|input| input.viewport().close_requested()) && self.jobs.is_busy() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.closing = true;
            self.jobs.begin_shutdown();
        }
    }

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
                if self.view == View::OrganizeIncoming {
                    ui.radio_value(&mut self.incoming_move, false, "Copy");
                    ui.radio_value(&mut self.incoming_move, true, "Move");
                    ui.separator();
                }
                if matches!(self.view, View::OrganizeIncoming | View::CopyByFileType) {
                    let _ = pending_button(ui, "Validate selection");
                }
                ui.label(
                    egui::RichText::new(status_text(job.as_ref(), closing, live_jobs)).small(),
                );
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
                let action = if self.view == View::OrganizeIncoming && self.incoming_move {
                    Some("Move selected")
                } else {
                    self.view.primary_action()
                };
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(action) = action {
                        let _ = primary_button(ui, action, false);
                    }
                    if let Some(job) = &job {
                        if job.is_terminal() && job.is_reaped() {
                            if ui.small_button("Dismiss").clicked() {
                                dismiss = true;
                            }
                        } else if job.is_terminal() {
                            // Reported, still unwinding its own cleanup: the row
                            // stays visible and cannot be dismissed yet.
                            ui.label(egui::RichText::new("Finishing…").small().weak());
                        } else {
                            if ui.button("Cancel").clicked() {
                                cancel = Some(job.id);
                            }
                            if job.paused || job.pause_requested {
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

    fn projects_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let _ = primary_button(ui, "Scan folders", false);
            ui.separator();
            ui.add_enabled(
                false,
                egui::TextEdit::singleline(&mut self.project_filter)
                    .hint_text("Filter projects")
                    .desired_width(200.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("New project").clicked() {
                    self.new_project = Some(NewProjectDraft::default());
                }
            });
        });
        ui.separator();

        egui::Panel::left("project_list_pane")
            .resizable(true)
            .default_size(340.0)
            .min_size(220.0)
            .max_size(620.0)
            .show(ui, |ui| self.project_list_pane(ui));

        egui::CentralPanel::default().show(ui, |ui| self.project_files_pane(ui));
    }

    fn project_list_pane(&mut self, ui: &mut egui::Ui) {
        if self.projects.is_empty() {
            empty_state(
                ui,
                "No projects yet.",
                "Adding projects is not available in this build.",
            );
            return;
        }

        let mut selected = self.selected_project;
        let projects = &self.projects;
        egui_extras::TableBuilder::new(ui)
            .id_salt("project_table")
            .resizable(true)
            .column(egui_extras::Column::remainder().at_least(120.0).clip(true))
            .column(egui_extras::Column::initial(64.0).at_least(48.0))
            .header(20.0, |mut header| {
                header.col(|ui| {
                    ui.label("Project");
                });
                header.col(|ui| {
                    ui.label("Files");
                });
            })
            .body(|mut body| {
                for (index, project) in projects.iter().enumerate() {
                    body.row(22.0, |mut row| {
                        row.col(|ui| {
                            ui.selectable_value(&mut selected, Some(index), project.name.as_str());
                        });
                        row.col(|ui| {
                            ui.label(project.files.to_string());
                        });
                    });
                }
            });
        self.selected_project = selected;
    }

    fn project_files_pane(&mut self, ui: &mut egui::Ui) {
        let Some(index) = self.selected_project else {
            empty_state(
                ui,
                "No project selected.",
                "Select a project to see its Nuke and After Effects files.",
            );
            return;
        };

        let name = self
            .projects
            .get(index)
            .map(|project| project.name.as_str())
            .unwrap_or("Selected project");
        ui.label(egui::RichText::new(name).small().weak());

        if self.files.is_empty() {
            empty_state(
                ui,
                "No files in this project yet.",
                "Scan folders to list versions.",
            );
            return;
        }

        let files = &self.files;
        egui_extras::TableBuilder::new(ui)
            .id_salt("project_files_table")
            .resizable(true)
            .column(egui_extras::Column::remainder().at_least(160.0).clip(true))
            .column(egui_extras::Column::initial(64.0).at_least(48.0))
            .column(egui_extras::Column::initial(76.0).at_least(64.0))
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
            .header(20.0, |mut header| {
                for title in ["File", "Version", "Launch", "Modified", "Last opened", "By"] {
                    header.col(|ui| {
                        ui.label(title);
                    });
                }
            })
            .body(|mut body| {
                for file in files {
                    body.row(22.0, |mut row| {
                        row.col(|ui| {
                            ui.label(file.file.as_str());
                        });
                        row.col(|ui| {
                            ui.label(file.version.as_str());
                        });
                        row.col(|ui| {
                            let _ = ui.add_enabled(false, egui::Button::new("Launch").small());
                        });
                        row.col(|ui| {
                            ui.label(file.modified.as_str());
                        });
                        row.col(|ui| {
                            ui.label(file.last_opened.as_str());
                        });
                        row.col(|ui| {
                            ui.label(file.by.as_str());
                        });
                    });
                }
            });
    }

    fn shots_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Project").small().weak());
            let current = self
                .selected_project_name()
                .unwrap_or("No project selected");
            ui.add_enabled(false, egui::Button::new(current));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let _ = pending_button(ui, "Templates…");
            });
        });
        ui.separator();

        egui::Panel::bottom("structure_preview_dock")
            .resizable(true)
            .default_size(170.0)
            .min_size(110.0)
            .max_size(340.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Preview").small().weak());
                empty_state(
                    ui,
                    "No preview yet.",
                    "Proposed folders and Nuke scripts appear here.",
                );
            });

        egui::Panel::left("structure_destination_tree")
            .resizable(true)
            .default_size(300.0)
            .min_size(200.0)
            .max_size(560.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Destination folders").small().weak());
                empty_state(
                    ui,
                    "No project folders.",
                    "Choose a project to edit its sequence and shot structure.",
                );
            });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                let _ = pending_button(ui, "Add sequence");
                let _ = pending_button(ui, "Add shot");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let _ = pending_button(ui, "Remove");
                });
            });
            ui.separator();
            empty_state(
                ui,
                "No sequences yet.",
                "Sequences, shots and Nuke settings appear here once a project is selected.",
            );
        });
    }

    fn incoming_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let _ = primary_button(ui, "Scan folders", false);
            ui.separator();
            folder_field(
                ui,
                "Source",
                &mut self.incoming_source,
                "No source folder chosen",
            );
            ui.separator();
            folder_field(
                ui,
                "Destination",
                &mut self.incoming_destination,
                "No destination folder chosen",
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let _ = pending_button(ui, "Suggest folders");
            });
        });
        ui.separator();

        egui::Panel::bottom("incoming_preview_dock")
            .resizable(true)
            .default_size(160.0)
            .min_size(110.0)
            .max_size(340.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Selected file").small().weak());
                empty_state(
                    ui,
                    "No file selected.",
                    "The planned destination and file changes appear here.",
                );
            });

        egui::Panel::left("incoming_source_tree")
            .resizable(true)
            .default_size(300.0)
            .min_size(200.0)
            .max_size(560.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Source folders").small().weak());
                empty_state(
                    ui,
                    "No source folder scanned.",
                    "Choose a source folder, then scan.",
                );
            });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Planned files").small().weak());
            });
            ui.separator();
            empty_state(
                ui,
                "No planned files yet.",
                "Copy and move plans appear here after a scan.",
            );
        });
    }

    fn copy_by_file_type_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let _ = primary_button(ui, "Scan folders", false);
            ui.separator();
            folder_field(
                ui,
                "Source",
                &mut self.copy_source,
                "No source folder chosen",
            );
            ui.separator();
            folder_field(
                ui,
                "Destination",
                &mut self.copy_destination,
                "No destination folder chosen",
            );
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("File types").small().weak());
            ui.add_enabled(
                false,
                egui::TextEdit::singleline(&mut self.copy_types)
                    .hint_text("Scan a source folder to list its file types")
                    .desired_width(360.0),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(
                        "Copies matching file types and keeps their folder structure and filenames.",
                    )
                    .small()
                    .weak(),
                );
            });
        });
        ui.separator();

        ui.horizontal(|ui| {
            for phase in ["Scanning", "Copying"] {
                ui.label(egui::RichText::new(phase).small().weak());
                ui.add(
                    egui::ProgressBar::new(0.0)
                        .desired_width(170.0)
                        .desired_height(8.0),
                );
            }
        });
        ui.separator();

        empty_state(
            ui,
            "No files to copy yet.",
            "Scan a source folder to preview the matching files.",
        );
    }

    fn backup_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let _ = primary_button(ui, "Scan folders", false);
            ui.separator();
            folder_field(
                ui,
                "Source",
                &mut self.backup_source,
                "No source folder chosen",
            );
            ui.separator();
            folder_field(
                ui,
                "Destination",
                &mut self.backup_destination,
                "No destination folder chosen",
            );
        });
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Exclusions").small().weak());
            ui.add_enabled(
                false,
                egui::TextEdit::singleline(&mut self.backup_exclusions)
                    .hint_text("Nothing excluded")
                    .desired_width(260.0),
            );
            ui.separator();
            ui.label(egui::RichText::new("Schedule").small().weak());
            let _ = pending_button(ui, "Manual only");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let _ = pending_button(ui, "Advanced…");
            });
        });
        ui.separator();

        for (phase, detail) in [
            ("Scanning", "0 folders"),
            ("Finding", "0 files"),
            ("Copying", "0 of 0 files"),
        ] {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(phase).small().weak());
                ui.add(
                    egui::ProgressBar::new(0.0)
                        .desired_width(190.0)
                        .desired_height(8.0),
                );
                ui.label(egui::RichText::new(detail).small().weak());
            });
        }
        ui.separator();

        empty_state(
            ui,
            "No backup run yet.",
            "Source, destination and backup results appear here.",
        );
    }

    fn settings_view(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.collapsing("Applications", |ui| {
                empty_state(
                    ui,
                    "No applications configured.",
                    "Nuke and After Effects paths appear here.",
                );
            });
            ui.collapsing("Project discovery", |ui| {
                empty_state(
                    ui,
                    "No project roots added.",
                    "Folders searched for existing projects appear here.",
                );
            });
            ui.collapsing("Users", |ui| {
                empty_state(
                    ui,
                    "No users yet.",
                    "Names used for file attribution appear here.",
                );
            });
            ui.collapsing("Logs", |ui| {
                empty_state(ui, "No log entries yet.", "Diagnostics appear here.");
            });
        });
    }

    fn new_project_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut draft) = self.new_project.take() else {
            return;
        };

        let mut open = true;
        let mut cancel = false;
        egui::Window::new("New project")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(240.0));
                });
                ui.horizontal(|ui| {
                    ui.label("Client");
                    let _ = pending_button(ui, "Default");
                });
                ui.horizontal(|ui| {
                    ui.label("Template");
                    let _ = pending_button(ui, "Default");
                });
                ui.separator();
                ui.horizontal(|ui| {
                    let _ = pending_button(ui, "Create");
                    if ui.button("Cancel").clicked() {
                        cancel = true;
                    }
                });
                ui.label(
                    egui::RichText::new(
                        "Project creation needs the catalog, which is not in this build.",
                    )
                    .small()
                    .weak(),
                );
            });

        if open && !cancel {
            self.new_project = Some(draft);
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
                        "Desktop foundation build. Project catalog, folder scanning and file operations arrive in later builds.",
                    )
                    .small()
                    .weak(),
                );
            });
        self.about_open = open;
    }

    fn selected_project_name(&self) -> Option<&str> {
        self.selected_project
            .and_then(|index| self.projects.get(index))
            .map(|project| project.name.as_str())
    }
}

fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    let text = egui::RichText::new(label).color(egui::Color32::WHITE);
    let response = ui.add_enabled(enabled, egui::Button::new(text).fill(ACCENT));
    if enabled {
        response
    } else {
        response.on_hover_text(UNAVAILABLE)
    }
}

fn pending_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add_enabled(false, egui::Button::new(label))
        .on_hover_text(UNAVAILABLE)
}

fn empty_state(ui: &mut egui::Ui, title: &str, hint: &str) {
    ui.vertical_centered(|ui| {
        ui.add_space(14.0);
        ui.label(title);
        ui.add_space(2.0);
        ui.label(egui::RichText::new(hint).small().weak());
    });
}

fn folder_field(ui: &mut egui::Ui, label: &str, value: &mut String, hint: &'static str) {
    ui.label(egui::RichText::new(label).small().weak());
    ui.add_enabled(
        false,
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(210.0),
    );
    let _ = pending_button(ui, "Browse…");
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
    match job.progress.total {
        Some(total) => format!("{} {}/{}", job.phase.label(), job.progress.completed, total),
        None => format!("{} {}", job.phase.label(), job.progress.completed),
    }
}
