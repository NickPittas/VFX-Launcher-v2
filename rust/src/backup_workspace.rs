//! Backup settings, job import/export and the GUI-only scheduler.
use crate::backup::{self, BackupJob};
use crate::copy_workspace::CopyByFileTypeState;
use crate::jobs::{JobId, JobOutcome, JobProgress, Jobs};
use crate::storage::{CatalogClient, UserId};
use eframe::egui;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

enum Reply {
    Loaded(Result<BackupJob, String>),
    Saved(BackupJob, Result<(), String>),
    Imported(Result<Option<BackupJob>, String>),
    Exported(Result<bool, String>),
}
struct Request {
    id: JobId,
    dialog: bool,
    receiver: Receiver<Reply>,
}

pub struct BackupWorkspace {
    ctx: egui::Context,
    copy: CopyByFileTypeState,
    path: PathBuf,
    loaded: bool,
    attempted_load: bool,
    saved: Option<BackupJob>,
    changed_at: Option<Instant>,
    request: Option<Request>,
    error: Option<String>,
    message: Option<String>,
    schedule: crate::schedule::Schedule,
    was_running: bool,
}

impl BackupWorkspace {
    pub fn new(ctx: &egui::Context, data_dir: &std::path::Path) -> Self {
        Self {
            ctx: ctx.clone(),
            copy: CopyByFileTypeState::new_backup(ctx, BackupJob::default()),
            path: data_dir.join("backup.json"),
            loaded: false,
            attempted_load: false,
            saved: None,
            changed_at: None,
            request: None,
            error: None,
            message: None,
            schedule: Default::default(),
            was_running: false,
        }
    }

    pub fn poll(&mut self, catalog: Option<&CatalogClient>, user: Option<UserId>, jobs: &mut Jobs) {
        self.copy.poll(catalog, user, jobs);
        if let Some(request) = self.request.take() {
            match request.receiver.try_recv() {
                Err(TryRecvError::Empty) => self.request = Some(request),
                Err(TryRecvError::Disconnected) => {
                    self.error = Some(
                        "The backup settings worker stopped without a result. Retry the operation."
                            .into(),
                    )
                }
                Ok(reply) => match reply {
                    Reply::Loaded(Ok(job)) => {
                        self.copy = CopyByFileTypeState::new_backup(&self.ctx, job.clone());
                        self.saved = Some(job.clone());
                        self.loaded = true;
                        self.error = self.schedule.update(&job).err();
                    }
                    Reply::Loaded(Err(error)) => self.error = Some(error),
                    Reply::Saved(job, result) => match result {
                        Ok(()) => {
                            self.saved = Some(job.clone());
                            self.error = self.schedule.update(&job).err();
                            self.message = Some("Backup settings saved.".into());
                        }
                        Err(error) => self.error = Some(error),
                    },
                    Reply::Imported(Ok(Some(mut job))) => {
                        job.schedule_enabled = false;
                        self.copy = CopyByFileTypeState::new_backup(&self.ctx, job);
                        self.message = Some(
                            "Job imported. Review both folders before enabling its schedule."
                                .into(),
                        );
                        self.error = None;
                    }
                    Reply::Imported(Ok(None)) | Reply::Exported(Ok(false)) => {}
                    Reply::Imported(Err(error)) | Reply::Exported(Err(error)) => {
                        self.error = Some(error)
                    }
                    Reply::Exported(Ok(true)) => self.message = Some("Backup job exported.".into()),
                },
            }
        }
        if !self.attempted_load && !jobs.is_shutting_down() {
            self.attempted_load = true;
            let path = self.path.clone();
            self.start(jobs, "Loading backup settings", false, move || {
                Reply::Loaded(backup::load_job(&path))
            });
        }
        if !self.loaded {
            return;
        }
        let Some(job) = self.copy.backup_job() else {
            return;
        };
        if self.saved.as_ref() != Some(&job) {
            let changed = self.changed_at.get_or_insert_with(Instant::now);
            if changed.elapsed() >= Duration::from_millis(400)
                && self.request.is_none()
                && self.error.is_none()
                && !jobs.is_shutting_down()
            {
                let path = self.path.clone();
                self.start(jobs, "Saving backup settings", false, move || {
                    let result = backup::save_job(&path, &job);
                    Reply::Saved(job, result)
                });
            }
        } else {
            self.changed_at = None;
        }
        self.copy.advance_automatic(jobs);
        let running = self.copy.operation_pending() || self.copy.automatic_active();
        if self.was_running && !running {
            if let Err(error) = self.schedule.terminal() {
                self.error = Some(error);
            }
        }
        self.was_running = running;
        if self.schedule.due(
            jobs.is_busy()
                || running
                || self.request.is_some()
                || !self.loaded
                || catalog.is_none()
                || user.is_none(),
        ) && !jobs.is_shutting_down()
        {
            self.copy.begin_automatic(jobs);
            self.was_running = self.copy.automatic_active();
            if !self.was_running {
                self.error = self.schedule.terminal().err();
            }
        }
        self.ctx.request_repaint_after(Duration::from_secs(1));
    }

    fn start(
        &mut self,
        jobs: &mut Jobs,
        title: &str,
        dialog: bool,
        work: impl FnOnce() -> Reply + Send + 'static,
    ) {
        let (sender, receiver) = mpsc::channel();
        match jobs.start(&self.ctx, title, move |context| {
            let reply = work();
            let failed = matches!(
                &reply,
                Reply::Loaded(Err(_))
                    | Reply::Saved(_, Err(_))
                    | Reply::Imported(Err(_))
                    | Reply::Exported(Err(_))
            );
            let _ = sender.send(reply);
            context.events.finish(
                if failed {
                    JobOutcome::Failed
                } else {
                    JobOutcome::Succeeded
                },
                JobProgress::default(),
            );
        }) {
            Ok(id) => {
                self.request = Some(Request {
                    id,
                    dialog,
                    receiver,
                })
            }
            Err(error) => self.error = Some(format!("The backup worker could not start: {error}")),
        }
    }

    pub fn view(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        egui::Panel::top("backup_job_controls").show(ui, |ui| {
            ui.horizontal(|ui| {
                let enabled =
                    self.loaded && self.request.is_none() && !self.copy.operation_pending();
                ui.add_enabled_ui(enabled, |ui| {
                    if ui.button("Import job…").clicked() {
                        self.start(jobs, "Importing backup job", true, || {
                            let result = rfd::FileDialog::new()
                                .set_title("Import backup job")
                                .add_filter("Backup job", &["json"])
                                .pick_file()
                                .map(|path| {
                                    let bytes = std::fs::read(&path).map_err(|error| {
                                        format!("The job could not be read: {error}")
                                    })?;
                                    if bytes.len() > 1024 * 1024 {
                                        return Err("The backup job exceeds 1 MiB.".into());
                                    }
                                    BackupJob::import(&bytes)
                                })
                                .transpose();
                            Reply::Imported(result)
                        });
                    }
                    if ui.button("Export job…").clicked() {
                        if let Some(job) = self.copy.backup_job() {
                            self.start(jobs, "Exporting backup job", true, move || {
                                let result = rfd::FileDialog::new()
                                    .set_title("Export backup job")
                                    .set_file_name("backup-job.json")
                                    .save_file()
                                    .map(|path| {
                                        job.validate()?;
                                        crate::config::write_versioned(
                                            &path,
                                            "exported backup job",
                                            &job,
                                        )
                                        .map_err(|error| error.to_string())
                                    })
                                    .transpose()
                                    .map(|result| result.is_some());
                                Reply::Exported(result)
                            });
                        }
                    }
                    ui.separator();
                    if let Some(job) = self.copy.backup_job_mut() {
                        ui.checkbox(&mut job.schedule_enabled, "Schedule while app is open");
                        egui::ComboBox::from_id_salt("backup_schedule_mode")
                            .selected_text(if job.schedule_mode == "daily" {
                                "Daily at"
                            } else {
                                "Every"
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut job.schedule_mode,
                                    "interval".into(),
                                    "Every",
                                );
                                ui.selectable_value(
                                    &mut job.schedule_mode,
                                    "daily".into(),
                                    "Daily at",
                                );
                            });
                        if job.schedule_mode == "daily" {
                            ui.add(
                                egui::TextEdit::singleline(&mut job.schedule_time)
                                    .desired_width(55.0),
                            );
                        } else {
                            ui.add(
                                egui::DragValue::new(&mut job.schedule_interval_minutes)
                                    .range(1..=525600),
                            );
                            ui.label("min");
                        }
                    }
                });
                ui.label(self.schedule.description());
            });
            if let Some(error) = self.error.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(230, 155, 100), error);
                    if ui.button("Retry").clicked() {
                        self.error = None;
                        if !self.loaded {
                            self.attempted_load = false;
                        }
                    }
                });
            } else if let Some(message) = &self.message {
                ui.label(message);
            }
        });
        ui.add_enabled_ui(self.loaded && self.request.is_none(), |ui| {
            ui.push_id("backup", |ui| self.copy.view(ui, jobs));
        });
    }

    pub fn validate_control(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        ui.add_enabled_ui(self.loaded && self.request.is_none(), |ui| {
            self.copy.validate_control(ui, jobs)
        });
    }
    pub fn execution_control(&mut self, ui: &mut egui::Ui, jobs: &mut Jobs) {
        ui.add_enabled_ui(self.loaded && self.request.is_none(), |ui| {
            self.copy.execution_control(ui, jobs)
        });
    }
    pub fn picker_pending(&self) -> bool {
        self.copy.picker_pending() || self.request.as_ref().is_some_and(|request| request.dialog)
    }
    pub fn is_picker_job(&self, id: JobId) -> bool {
        self.copy.is_picker_job(id)
            || self
                .request
                .as_ref()
                .is_some_and(|request| request.id == id && request.dialog)
    }
}
