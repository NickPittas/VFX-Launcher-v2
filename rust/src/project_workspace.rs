//! U1 project workspace: the asynchronous catalog state behind the Projects view.
//!
//! Everything expensive happens off the egui frame: settings load and save run on
//! their own worker thread, registration planning runs on a worker thread, folder
//! selection runs in the native dialog on a worker thread, launching and revealing
//! run on worker threads, scanning runs as a [`crate::jobs`] job with real progress
//! and cancellation, and every catalog call returns a [`Pending`] the frame only
//! polls. No SQL, `stat`, dialog, spawn or walk runs inside a frame.
//!
//! Stale-async safety: a reply is applied only while the request id it was issued
//! under is still current, so switching tab, filter, user or selected project can
//! never show another selection's projects or files.
//!
//! Cancellation is a result: a cancelled scan reconciles nothing
//! (`reconcile_scan` rolls back), and the workspace says so instead of reporting a
//! generic failure.
//!
//! Each accepted launch keeps its own reply slot together with the identity and
//! file it was accepted for, so two launches never replace each other and a later
//! user change cannot re-attribute or drop the access record. Closing stops new
//! requests, drains the accepted launch and reveal replies and the access records
//! they produce, and only then consumes the catalog: no queued write is lost and
//! no frame waits on a thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use eframe::egui;

use crate::config::{ConfigError, Settings, SettingsStore};
use crate::discovery::{self, Coverage, ScanPurpose, ScanRequest, ScanSnapshot};
use crate::jobs::{JobContext, JobControl, JobId, JobOutcome, JobPhase, JobProgress, Jobs};
use crate::launch::{self, LaunchError, Tool};
use crate::paths::PathError;
use crate::projects::{
    FileGroup, Project, ProjectFile, ProjectList, ReconcileSummary, RegistrationPlan,
    RegistrationSummary,
};
use crate::storage::{
    Catalog, CatalogInfo, CatalogShutdown, FileId, Pending, ProjectId, ReplyState, StorageError,
    UserId, UserRecord,
};

/// How long the project filter waits after the last keystroke before it is
/// queried, so typing does not queue one catalog read per character.
const FILTER_DEBOUNCE: f64 = 0.25;

/// Settings file I/O on its own thread, so startup and identity changes never
/// read or write the config on the UI thread. A malformed file is reported and
/// never replaced with defaults.
struct SettingsWorker {
    commands: Option<Sender<SettingsCommand>>,
    events: Receiver<SettingsEvent>,
    thread: Option<std::thread::JoinHandle<()>>,
}

enum SettingsCommand {
    Load,
    Save(Settings),
}

enum SettingsEvent {
    Loaded(Result<Settings, ConfigError>),
    Saved(Result<(), ConfigError>, Settings),
}

impl SettingsWorker {
    fn start(path: PathBuf, ctx: egui::Context) -> Self {
        let (commands, command_rx) = channel::<SettingsCommand>();
        let (event_tx, events) = channel::<SettingsEvent>();
        let thread = std::thread::Builder::new()
            .name("vfx-settings".to_string())
            .spawn(move || {
                let store = SettingsStore::new(path);
                while let Ok(command) = command_rx.recv() {
                    let event = match command {
                        SettingsCommand::Load => SettingsEvent::Loaded(store.load()),
                        SettingsCommand::Save(settings) => {
                            let result = store.save(&settings);
                            SettingsEvent::Saved(result, settings)
                        }
                    };
                    if event_tx.send(event).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .ok();
        Self {
            commands: Some(commands),
            events,
            thread,
        }
    }

    fn load(&self) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(SettingsCommand::Load);
        }
    }

    fn save(&self, settings: Settings) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(SettingsCommand::Save(settings));
        }
    }

    fn begin_shutdown(&mut self) {
        self.commands.take();
    }

    fn shutdown_pending(&mut self) -> bool {
        if self
            .thread
            .as_ref()
            .is_some_and(|thread| thread.is_finished())
        {
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
        self.thread.is_some()
    }
}

/// Catalog startup, one non-blocking step at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogStage {
    /// Waiting for the settings worker and the data directory.
    Settings,
    /// `Catalog::open` was called; waiting for the open to be answered.
    Opening,
    /// Reading the identity list.
    ReadingUsers,
    /// Creating the first `admin` identity in a catalog with no identities.
    CreatingAdmin,
    Ready,
    Failed,
}

enum Registration {
    Idle,
    Choosing(Receiver<Option<Vec<PathBuf>>>),
    Planning(Receiver<RegistrationPlan>),
    Registering(Pending<RegistrationSummary>),
    Summary(RegistrationSummary),
    Failed(String),
}

/// One scope is walked, then recorded, before the next scope starts.
enum ScanStep {
    Walking {
        project: ProjectId,
        label: String,
        job: JobId,
        files: Receiver<(Result<ScanSnapshot, PathError>, JobControl)>,
    },
    Reconciling(Pending<ReconcileSummary>),
}

/// Serial scan queue over the registered scan folders of every project.
#[derive(Default)]
struct Scan {
    cancelled: bool,
    control: Option<JobControl>,
    projects: Option<Pending<Vec<Project>>>,
    queue: Vec<(ProjectId, PathBuf)>,
    step: Option<ScanStep>,
    lines: Vec<String>,
    running: bool,
}

/// One launch, reveal or registration result shown in the Projects pane.
pub struct Notice {
    pub title: String,
    pub detail: Option<String>,
    pub error: bool,
}

/// How many notices are retained before the oldest is dropped, so a burst of
/// launches cannot grow this list without bound.
const NOTICE_LIMIT: usize = 6;

/// One accepted launch. Tool, path, file and identity are captured when the
/// command is accepted, so a later view, project or identity change cannot
/// re-attribute the access record or lose it. The worker handle is reaped only
/// after it finished, never joined while it runs, and it only covers the spawn:
/// the launched application itself is never awaited.
struct LaunchRequest {
    tool: Tool,
    path: PathBuf,
    file: FileId,
    user: UserId,
    replies: Receiver<Result<(), LaunchError>>,
    worker: Option<JoinHandle<()>>,
}

/// One accepted reveal in the platform file manager.
struct RevealRequest {
    replies: Receiver<Result<(), LaunchError>>,
    worker: Option<JoinHandle<()>>,
}

/// One accepted favorite write. The reply is kept, not discarded, so a failure
/// is reported and the affected user's list is re-read afterwards.
struct FavoriteWrite {
    user: UserId,
    pending: Pending<bool>,
}

/// Async state behind the Projects view.
pub struct ProjectWorkspace {
    ctx: egui::Context,
    data_dir: PathBuf,
    settings_worker: Option<SettingsWorker>,
    settings: Settings,
    settings_loaded: bool,
    settings_error: Option<String>,
    settings_save_error: Option<String>,
    catalog: Option<Catalog>,
    shutdown: Option<CatalogShutdown>,
    catalog_stage: CatalogStage,
    catalog_error: Option<String>,
    info: Option<Pending<CatalogInfo>>,
    users: Option<Pending<Vec<UserRecord>>>,
    creating_admin: Option<Pending<UserRecord>>,
    users_list: Vec<UserRecord>,
    user: Option<UserRecord>,
    tab: ProjectList,
    filter_edit: String,
    filter_applied: String,
    filter_changed_at: f64,
    projects: Vec<Project>,
    projects_pending: Option<(u64, Pending<Vec<Project>>)>,
    projects_request: u64,
    projects_error: Option<String>,
    selected: Option<ProjectId>,
    files: Vec<FileGroup>,
    files_pending: Option<(u64, ProjectId, Pending<Vec<FileGroup>>)>,
    files_request: u64,
    files_error: Option<String>,
    recent: Option<Pending<String>>,
    registration: Registration,
    scan: Scan,
    launches: Vec<LaunchRequest>,
    reveals: Vec<RevealRequest>,
    access: Vec<Pending<()>>,
    favorites: Vec<FavoriteWrite>,
    /// A coordinated stop started: no new request is accepted any more.
    closing: bool,
    notices: Vec<Notice>,
}

impl ProjectWorkspace {
    pub fn new(ctx: &egui::Context, data_dir: PathBuf) -> Self {
        let mut workspace = Self {
            ctx: ctx.clone(),
            data_dir: data_dir.clone(),
            settings_worker: None,
            settings: Settings::default(),
            settings_loaded: false,
            settings_error: None,
            settings_save_error: None,
            catalog: None,
            shutdown: None,
            catalog_stage: CatalogStage::Settings,
            catalog_error: None,
            info: None,
            users: None,
            creating_admin: None,
            users_list: Vec::new(),
            user: None,
            tab: ProjectList::All,
            filter_edit: String::new(),
            filter_applied: String::new(),
            filter_changed_at: 0.0,
            projects: Vec::new(),
            projects_pending: None,
            projects_request: 0,
            projects_error: None,
            selected: None,
            files: Vec::new(),
            files_pending: None,
            files_request: 0,
            files_error: None,
            recent: None,
            registration: Registration::Idle,
            scan: Scan::default(),
            launches: Vec::new(),
            reveals: Vec::new(),
            access: Vec::new(),
            favorites: Vec::new(),
            closing: false,
            notices: Vec::new(),
        };
        workspace.start_settings();
        workspace
    }

    /// Drives every asynchronous stage. Called once per frame from `logic`, so it
    /// also runs while the window is hidden.
    pub fn poll(&mut self, jobs: &mut Jobs) {
        let now = self.ctx.input(|input| input.time);
        self.poll_settings();
        self.poll_catalog();
        self.poll_filter(now);
        self.poll_projects();
        self.poll_files();
        self.poll_registration();
        self.poll_scan(jobs);
        self.poll_launch();
        self.poll_reveal();
        self.poll_access();
        self.poll_favorites();
        self.poll_recent();
        // Only after the accepted launch, reveal and access replies drained is the
        // catalog queue closed; until then its queued writes must stay accepted.
        self.try_close_catalog();
    }

    /// Appends one result, keeping the newest few instead of silently overwriting
    /// the outcome of a concurrent launch or reveal.
    fn push_notice(&mut self, notice: Notice) {
        self.notices.push(notice);
        while self.notices.len() > NOTICE_LIMIT {
            self.notices.remove(0);
        }
    }

    // ---------------------------------------------------------------- settings

    fn start_settings(&mut self) {
        let worker = SettingsWorker::start(self.data_dir.join("settings.json"), self.ctx.clone());
        worker.load();
        self.settings_worker = Some(worker);
    }

    fn poll_settings(&mut self) {
        let Some(worker) = &self.settings_worker else {
            return;
        };
        let mut loaded = None;
        let mut saved = None;
        let mut stopped = false;
        loop {
            match worker.events.try_recv() {
                Ok(SettingsEvent::Loaded(result)) => loaded = Some(result),
                Ok(SettingsEvent::Saved(result, settings)) => saved = Some((result, settings)),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    stopped = true;
                    break;
                }
            }
        }
        if stopped && !self.settings_loaded && self.settings_error.is_none() {
            // The settings worker ended without answering: report it instead of
            // leaving the app waiting for a reply that will never arrive.
            self.settings_error = Some(
                "The settings file could not be read: the settings worker stopped.".to_string(),
            );
            if self.catalog_stage == CatalogStage::Settings {
                self.catalog_stage = CatalogStage::Opening;
            }
        }

        if let Some((result, settings)) = saved {
            match result {
                Ok(()) => {
                    self.settings = settings;
                    self.settings_save_error = None;
                }
                Err(error) => {
                    self.settings_save_error = Some(format!(
                        "The settings could not be saved: {}",
                        error.message()
                    ));
                }
            }
        }

        if let Some(result) = loaded {
            match result {
                Ok(settings) => {
                    self.settings = settings;
                    self.settings_loaded = true;
                }
                Err(error) => {
                    // The in-memory defaults keep the app readable; the file stays
                    // exactly as it is, and no save is attempted while it is broken.
                    self.settings_error = Some(format!(
                        "The settings file could not be read ({}) and will not be overwritten: {}",
                        error
                            .path()
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "unknown path".to_string()),
                        error.message()
                    ));
                }
            }
            if self.catalog_stage == CatalogStage::Settings {
                self.catalog_stage = CatalogStage::Opening;
            }
        }
    }

    fn save_settings(&mut self) {
        if !self.settings_loaded {
            return;
        }
        if let Some(worker) = &self.settings_worker {
            worker.save(self.settings.clone());
        }
    }

    pub fn settings_save_error(&self) -> Option<&str> {
        self.settings_save_error.as_deref()
    }

    // ----------------------------------------------------------------- catalog

    fn poll_catalog(&mut self) {
        // A closing or closed catalog is never reopened: the coordinated stop owns
        // the queue from here on.
        if self.closing || self.shutdown.is_some() {
            return;
        }
        if matches!(
            self.catalog_stage,
            CatalogStage::Settings | CatalogStage::Failed | CatalogStage::Ready
        ) {
            return;
        }

        if self.catalog_stage == CatalogStage::Opening && self.catalog.is_none() {
            match Catalog::open(self.data_dir.join("catalog.sqlite3")) {
                Ok(catalog) => {
                    let ctx = self.ctx.clone();
                    catalog.set_completion_notifier(Some(Arc::new(move || {
                        ctx.request_repaint();
                    })));
                    match catalog.info() {
                        Ok(pending) => {
                            self.info = Some(pending);
                            self.catalog = Some(catalog);
                        }
                        Err(error) => self.fail_catalog(&error),
                    }
                }
                Err(error) => self.fail_catalog(&error),
            }
        }

        if let Some(pending) = self.info.as_mut() {
            match pending.try_poll() {
                ReplyState::Waiting => {}
                ReplyState::Ready(_info) => {
                    self.info = None;
                    self.catalog_stage = CatalogStage::ReadingUsers;
                    self.request_users();
                }
                ReplyState::Failed(error) => {
                    self.info = None;
                    self.fail_catalog(&error);
                }
                ReplyState::Finished => self.info = None,
            }
        }

        if let Some(pending) = self.users.as_mut() {
            match pending.try_poll() {
                ReplyState::Waiting => {}
                ReplyState::Ready(users) => {
                    self.users = None;
                    self.users_list = users;
                    self.after_users();
                }
                ReplyState::Failed(error) => {
                    self.users = None;
                    self.fail_catalog(&error);
                }
                ReplyState::Finished => self.users = None,
            }
        }

        if let Some(pending) = self.creating_admin.as_mut() {
            match pending.try_poll() {
                ReplyState::Waiting => {}
                ReplyState::Ready(user) => {
                    self.creating_admin = None;
                    self.catalog_stage = CatalogStage::Ready;
                    self.select_user(user);
                    if self.settings_loaded {
                        self.settings.selected_username = Some("admin".to_string());
                        self.save_settings();
                    }
                }
                ReplyState::Failed(error) => {
                    self.creating_admin = None;
                    self.fail_catalog(&error);
                }
                ReplyState::Finished => self.creating_admin = None,
            }
        }
    }

    fn request_users(&mut self) {
        let Some(catalog) = &self.catalog else {
            return;
        };
        match catalog.list_users() {
            Ok(pending) => self.users = Some(pending),
            Err(error) => self.fail_catalog(&error),
        }
    }

    /// Restores the recorded identity by name. `admin` is created only in a
    /// catalog that has no identities at all — never by assuming id 1, never by
    /// rewriting an existing name.
    fn after_users(&mut self) {
        self.catalog_stage = CatalogStage::Ready;
        if let Some(username) = self.settings.selected_username.clone() {
            if let Some(user) = self
                .users_list
                .iter()
                .find(|user| user.username == username && user.deleted_at.is_none())
            {
                let user = user.clone();
                self.select_user(user);
                return;
            }
        }
        if self.users_list.is_empty() {
            let Some(catalog) = &self.catalog else {
                return;
            };
            match catalog.create_user("admin", true) {
                Ok(pending) => {
                    self.catalog_stage = CatalogStage::CreatingAdmin;
                    self.creating_admin = Some(pending);
                }
                Err(error) => self.fail_catalog(&error),
            }
        }
        // Otherwise the toolbar chooser is the only way in: no guessed identity.
    }

    fn fail_catalog(&mut self, error: &StorageError) {
        self.catalog_stage = CatalogStage::Failed;
        self.catalog_error = Some(format!(
            "The catalog could not be opened: {}{}",
            error.message(),
            error
                .path()
                .map(|path| format!(" ({})", path.display()))
                .unwrap_or_default()
        ));
    }

    fn select_user(&mut self, user: UserRecord) {
        let changed = self.user.as_ref().map(|current| current.id) != Some(user.id);
        self.user = Some(user);
        if changed {
            self.projects.clear();
            self.projects_error = None;
            self.selected = None;
            self.files.clear();
            self.files_error = None;
        }
        self.refresh_projects();
    }

    /// Chooses who the recorded actions are attributed to.
    pub fn choose_user(&mut self, id: UserId) {
        if self.closing {
            return;
        }
        let Some(user) = self.users_list.iter().find(|user| user.id == id) else {
            return;
        };
        if self.user.as_ref().map(|user| user.id) == Some(id) {
            return;
        }
        let user = user.clone();
        if self.settings_loaded {
            self.settings.selected_username = Some(user.username.clone());
            self.save_settings();
        }
        self.select_user(user);
    }

    // ---------------------------------------------------------------- projects

    fn poll_filter(&mut self, now: f64) {
        if self.filter_edit == self.filter_applied {
            return;
        }
        if now - self.filter_changed_at < FILTER_DEBOUNCE {
            // Keep a short repaint scheduled so the debounce actually expires.
            self.ctx.request_repaint_after(Duration::from_millis(60));
            return;
        }
        self.filter_applied = self.filter_edit.clone();
        self.refresh_projects();
    }

    pub fn set_filter(&mut self, text: String) {
        if self.closing || self.filter_edit == text {
            return;
        }
        self.filter_edit = text;
        self.filter_changed_at = self.ctx.input(|input| input.time);
    }

    pub fn set_tab(&mut self, tab: ProjectList) {
        if self.closing || self.tab == tab {
            return;
        }
        self.tab = tab;
        self.refresh_projects();
    }

    /// Re-reads the project list for the current user, tab and filter.
    pub fn refresh_projects(&mut self) {
        if self.closing {
            return;
        }
        let (Some(catalog), Some(user)) = (&self.catalog, &self.user) else {
            return;
        };
        let filter = {
            let trimmed = self.filter_applied.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        };
        match crate::projects::list_projects(catalog, user.id, self.tab, filter) {
            Ok(pending) => {
                self.projects_request += 1;
                self.projects_error = None;
                self.projects_pending = Some((self.projects_request, pending));
            }
            Err(error) => self.projects_error = Some(error.message().to_string()),
        }
    }

    fn poll_projects(&mut self) {
        let Some((request, pending)) = self.projects_pending.as_mut() else {
            return;
        };
        let request = *request;
        let reply = pending.try_poll();
        match reply {
            ReplyState::Waiting => {}
            ReplyState::Ready(projects) if request == self.projects_request => {
                self.projects_pending = None;
                self.projects = projects;
                self.projects_error = None;
                if let Some(selected) = self.selected {
                    if !self.projects.iter().any(|project| project.id == selected) {
                        self.selected = None;
                        self.files.clear();
                        self.files_error = None;
                    }
                }
            }
            ReplyState::Failed(error) if request == self.projects_request => {
                self.projects_pending = None;
                self.projects_error = Some(error.message().to_string());
            }
            // A newer query (tab, filter or user change) owns the view now, so this
            // answer describes a list the user is no longer looking at.
            ReplyState::Ready(_) | ReplyState::Failed(_) | ReplyState::Finished => {
                self.projects_pending = None;
            }
        }
    }

    /// Selects a project: loads its files and records the access for this user.
    pub fn select_project(&mut self, id: ProjectId) {
        if self.closing || self.selected == Some(id) {
            return;
        }
        self.selected = Some(id);
        self.files.clear();
        self.files_error = None;
        self.request_files(id);
        self.mark_recent(id);
    }

    fn request_files(&mut self, id: ProjectId) {
        let Some(catalog) = &self.catalog else {
            return;
        };
        self.files_request += 1;
        match crate::projects::list_project_files(catalog, id, None) {
            Ok(pending) => self.files_pending = Some((self.files_request, id, pending)),
            Err(error) => self.files_error = Some(error.message().to_string()),
        }
    }

    fn poll_files(&mut self) {
        let Some((request, project, pending)) = self.files_pending.as_mut() else {
            return;
        };
        let request = *request;
        let project = *project;
        let reply = pending.try_poll();
        let current = request == self.files_request && self.selected == Some(project);
        match reply {
            ReplyState::Waiting => {}
            ReplyState::Ready(groups) if current => {
                self.files_pending = None;
                self.files = groups;
                self.files_error = None;
            }
            ReplyState::Failed(error) if current => {
                self.files_pending = None;
                self.files_error = Some(error.message().to_string());
            }
            // Stale: the selection changed while this reply was in flight.
            ReplyState::Ready(_) | ReplyState::Failed(_) | ReplyState::Finished => {
                self.files_pending = None;
            }
        }
    }

    fn mark_recent(&mut self, id: ProjectId) {
        let (Some(catalog), Some(user)) = (&self.catalog, &self.user) else {
            return;
        };
        match crate::projects::mark_recent(catalog, user.id, id) {
            Ok(pending) => self.recent = Some(pending),
            Err(error) => self.push_notice(notice_from_error("Recent projects", &error)),
        }
    }

    fn poll_recent(&mut self) {
        let Some(pending) = self.recent.as_mut() else {
            return;
        };
        match pending.try_poll() {
            ReplyState::Waiting => {}
            ReplyState::Ready(_) => {
                self.recent = None;
                if self.tab == ProjectList::Recents {
                    self.refresh_projects();
                }
            }
            ReplyState::Failed(error) => {
                self.recent = None;
                self.push_notice(notice_from_error("Recent projects", &error));
            }
            ReplyState::Finished => self.recent = None,
        }
    }

    /// Adds or removes the current user's favorite for one project. The reply is
    /// retained: a failed write is reported and the affected user's list is re-read
    /// instead of the outcome being discarded.
    pub fn toggle_favorite(&mut self, id: ProjectId) {
        if self.closing {
            return;
        }
        let (Some(catalog), Some(user)) = (&self.catalog, &self.user) else {
            return;
        };
        let favorite = !self
            .projects
            .iter()
            .find(|project| project.id == id)
            .map(|project| project.favorite)
            .unwrap_or(false);
        match crate::projects::set_favorite(catalog, user.id, id, favorite) {
            Ok(pending) => self.favorites.push(FavoriteWrite {
                user: user.id,
                pending,
            }),
            Err(error) => self.push_notice(notice_from_error("Favorites", &error)),
        }
    }

    /// Every accepted favorite write is polled on its own, so a later write never
    /// replaces an earlier one. The list is re-read only while the write's own user
    /// is still the one on screen, so a stale reply never replaces another
    /// identity's list.
    fn poll_favorites(&mut self) {
        let mut index = 0;
        while index < self.favorites.len() {
            let reply = self.favorites[index].pending.try_poll();
            match reply {
                ReplyState::Waiting => index += 1,
                ReplyState::Ready(_) => {
                    let user = self.favorites.remove(index).user;
                    if self.user.as_ref().map(|current| current.id) == Some(user) {
                        self.refresh_projects();
                    }
                }
                ReplyState::Failed(error) => {
                    self.favorites.remove(index);
                    self.push_notice(notice_from_error("Favorites", &error));
                }
                ReplyState::Finished => {
                    self.favorites.remove(index);
                }
            }
        }
    }

    // ------------------------------------------------------------ registration

    /// Opens the native folder picker on a worker thread.
    pub fn begin_add_folders(&mut self) {
        if !self.actions_enabled() || self.registration_busy() {
            return;
        }
        let (tx, rx) = channel();
        let ctx = self.ctx.clone();
        let start = self
            .selected_project()
            .and_then(|project| project.root.clone());
        let _ = std::thread::Builder::new()
            .name("vfx-folder-dialog".to_string())
            .spawn(move || {
                let mut dialog = rfd::FileDialog::new().set_title("Add existing project folders");
                if let Some(start) = start {
                    dialog = dialog.set_directory(start);
                }
                let picked = dialog.pick_folders();
                let _ = tx.send(picked);
                ctx.request_repaint();
            });
        self.registration = Registration::Choosing(rx);
    }

    fn poll_registration(&mut self) {
        let mut next = None;
        match &mut self.registration {
            Registration::Choosing(rx) => match rx.try_recv() {
                Ok(Some(roots)) if !roots.is_empty() => {
                    next = Some(Registration::Planning(start_planning(
                        roots,
                        self.ctx.clone(),
                    )));
                }
                Ok(_) => next = Some(Registration::Idle),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => next = Some(Registration::Idle),
            },
            Registration::Planning(rx) => match rx.try_recv() {
                Ok(plan) => {
                    next = match (&self.catalog, &self.user) {
                        (Some(catalog), Some(user)) => Some(
                            match crate::projects::register_projects(catalog, user.id, plan) {
                                Ok(pending) => Registration::Registering(pending),
                                Err(error) => Registration::Failed(error.message().to_string()),
                            },
                        ),
                        _ => Some(Registration::Idle),
                    };
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    next = Some(Registration::Failed(
                        "Adding the folders stopped without a result.".to_string(),
                    ));
                }
            },
            Registration::Registering(pending) => match pending.try_poll() {
                ReplyState::Waiting => {}
                ReplyState::Ready(summary) => {
                    self.refresh_projects();
                    next = Some(Registration::Summary(summary));
                }
                ReplyState::Failed(error) => {
                    next = Some(Registration::Failed(error.message().to_string()));
                }
                ReplyState::Finished => next = Some(Registration::Idle),
            },
            Registration::Idle | Registration::Summary(_) | Registration::Failed(_) => {}
        }
        if let Some(registration) = next {
            self.registration = registration;
        }
    }

    /// What the Projects pane should say about registration, if anything.
    pub fn registration_status(&self) -> Option<RegistrationStatus<'_>> {
        match &self.registration {
            Registration::Idle => None,
            Registration::Choosing(_) => Some(RegistrationStatus::Busy("Choosing folders…")),
            Registration::Planning(_) => Some(RegistrationStatus::Busy("Checking folders…")),
            Registration::Registering(_) => Some(RegistrationStatus::Busy("Adding projects…")),
            Registration::Summary(summary) => Some(RegistrationStatus::Summary(summary)),
            Registration::Failed(message) => Some(RegistrationStatus::Failed(message)),
        }
    }

    // -------------------------------------------------------------------- scan

    /// Queues every registered scan folder of every project, one scope at a time.
    pub fn begin_scan(&mut self) {
        if !self.actions_enabled() || self.scan.running {
            return;
        }
        let (Some(catalog), Some(user)) = (&self.catalog, &self.user) else {
            return;
        };
        match crate::projects::list_projects(catalog, user.id, ProjectList::All, None) {
            Ok(pending) => {
                self.scan.projects = Some(pending);
                self.scan.running = true;
                self.scan.cancelled = false;
                self.scan.control = None;
                self.scan.queue.clear();
                self.scan.lines.clear();
            }
            Err(error) => self.push_notice(notice_from_error("Scan", &error)),
        }
    }

    /// Cancels the running scope and drops the queued ones. The current scope
    /// still finishes cooperatively, and its reconciliation writes nothing.
    pub fn cancel_scan(&mut self, jobs: &mut Jobs) {
        self.scan.cancelled = true;
        self.scan.projects = None;
        if let Some(control) = &self.scan.control {
            control.cancel();
        }
        if let Some(ScanStep::Walking { job, .. }) = &self.scan.step {
            jobs.cancel(*job);
        }
        self.scan.queue.clear();
        if self.scan.step.is_none() {
            self.scan.running = false;
        }
    }

    fn poll_scan(&mut self, jobs: &mut Jobs) {
        if !self.scan.running {
            return;
        }

        if let Some(pending) = self.scan.projects.as_mut() {
            match pending.try_poll() {
                ReplyState::Waiting => {}
                ReplyState::Ready(projects) => {
                    self.scan.projects = None;
                    self.scan.queue = projects
                        .into_iter()
                        .flat_map(|project| {
                            project.scan_folders.into_iter().filter_map(move |folder| {
                                folder.path.map(|path| (project.id, path))
                            })
                        })
                        .collect();
                    if self.scan.queue.is_empty() {
                        self.scan.running = false;
                        self.push_notice(Notice {
                            title: "No folders to scan.".to_string(),
                            detail: Some(
                                "Scans only cover folders of registered projects. Add a project folder first."
                                    .to_string(),
                            ),
                            error: false,
                        });
                    }
                }
                ReplyState::Failed(error) => {
                    self.scan.projects = None;
                    self.scan.running = false;
                    self.push_notice(notice_from_error("Scan", &error));
                }
                ReplyState::Finished => self.scan.projects = None,
            }
        }

        if let Some(step) = self.scan.step.take() {
            match step {
                ScanStep::Walking {
                    project,
                    label,
                    job: _job,
                    files,
                } => match files.try_recv() {
                    Ok((result, control)) => match result {
                        Ok(snapshot) => {
                            if self.scan.cancelled {
                                control.cancel();
                            }
                            self.scan.control = Some(control.clone());
                            let cancelled = snapshot.coverage == Coverage::Cancelled;
                            if let Some(catalog) = &self.catalog {
                                let scope = snapshot.root.clone();
                                match crate::projects::reconcile_scan(
                                    catalog, project, scope, snapshot, control,
                                ) {
                                    Ok(pending) => {
                                        self.scan.step = Some(ScanStep::Reconciling(pending));
                                    }
                                    Err(error) => self.scan.lines.push(format!(
                                        "{label}: could not be recorded ({})",
                                        error.message()
                                    )),
                                }
                            }
                            if cancelled {
                                self.scan.queue.clear();
                            }
                        }
                        Err(error) => self
                            .scan
                            .lines
                            .push(format!("{label}: {}", error.message())),
                    },
                    // The walk is still running: keep the step and keep polling.
                    Err(TryRecvError::Empty) => {
                        self.scan.step = Some(ScanStep::Walking {
                            project,
                            label,
                            job: _job,
                            files,
                        });
                    }
                    Err(TryRecvError::Disconnected) => {
                        self.scan
                            .lines
                            .push(format!("{label}: the scan ended without a result"));
                    }
                },
                ScanStep::Reconciling(mut pending) => match pending.try_poll() {
                    ReplyState::Waiting => {
                        self.scan.step = Some(ScanStep::Reconciling(pending));
                    }
                    ReplyState::Ready(summary) => {
                        self.scan.lines.push(describe_reconcile(&summary));
                        if summary.cancelled {
                            self.scan.running = false;
                            self.scan.queue.clear();
                            self.push_notice(Notice {
                                title: "Scan cancelled. Nothing was recorded.".to_string(),
                                detail: summary.prune_suppressed.clone(),
                                error: false,
                            });
                            self.refresh_projects();
                            return;
                        }
                        if self.selected == Some(summary.project) {
                            self.request_files(summary.project);
                        }
                    }
                    ReplyState::Failed(error) => {
                        self.scan.lines.push(format!(
                            "A folder could not be recorded: {}",
                            error.message()
                        ));
                    }
                    ReplyState::Finished => {}
                },
            }
        }

        if self.scan.step.is_none() {
            if self.scan.queue.is_empty() {
                self.scan.running = false;
                self.refresh_projects();
            } else {
                let (project, scope) = self.scan.queue.remove(0);
                // One scan row at a time in the status strip: finished scopes are
                // already summarized in this pane's scan details.
                jobs.clear_finished();
                let label = folder_label(&scope);
                match start_walk(jobs, &self.ctx, format!("Scanning {label}"), scope) {
                    Ok((job, files)) => {
                        self.scan.step = Some(ScanStep::Walking {
                            project,
                            label,
                            job,
                            files,
                        });
                    }
                    Err(error) => self
                        .scan
                        .lines
                        .push(format!("{label}: the scan could not be started ({error})")),
                }
            }
        }
    }

    pub fn scan_running(&self) -> bool {
        self.scan.running
    }

    pub fn scan_lines(&self) -> &[String] {
        &self.scan.lines
    }

    // ------------------------------------------------------------------ launch

    pub fn begin_launch(&mut self, file: &ProjectFile) {
        if !self.actions_enabled() {
            return;
        }
        // Captured with the request: the access record keeps this identity even if
        // the user, view or selected project changes before the reply arrives.
        let Some(user) = self.user.as_ref().map(|user| user.id) else {
            return;
        };
        let Some(path) = file.path.clone() else {
            self.push_notice(Notice {
                title: "This file has no usable path on this machine.".to_string(),
                detail: file.display.clone(),
                error: true,
            });
            return;
        };
        let Some(tool) = file.filetype.as_deref().and_then(Tool::from_filetype) else {
            self.push_notice(Notice {
                title: "This file type has no configured application.".to_string(),
                detail: file.display.clone(),
                error: true,
            });
            return;
        };
        let applications = self.settings.applications.clone();
        let (tx, rx) = channel();
        let ctx = self.ctx.clone();
        let worker_path = path.clone();
        // The worker only covers planning and the spawn; the launched application
        // itself is never awaited, so closing never waits for the DCC to exit.
        let worker = std::thread::Builder::new()
            .name("vfx-launch".to_string())
            .spawn(move || {
                let result = launch::start(tool, &worker_path, &applications);
                let _ = tx.send(result);
                ctx.request_repaint();
            })
            .ok();
        self.launches.push(LaunchRequest {
            tool,
            path,
            file: file.file,
            user,
            replies: rx,
            worker,
        });
        // A worker that never sends (dropped sender) is noticed on a later frame.
        self.ctx.request_repaint_after(Duration::from_millis(40));
    }

    /// Drains every accepted launch independently, so a second launch never
    /// replaces the first one's reply, identity or access record.
    fn poll_launch(&mut self) {
        let mut index = 0;
        while index < self.launches.len() {
            let reply = self.launches[index].replies.try_recv();
            match reply {
                Err(TryRecvError::Empty) => {
                    reap_finished(&mut self.launches[index].worker);
                    index += 1;
                }
                Err(TryRecvError::Disconnected) => {
                    let mut request = self.launches.remove(index);
                    reap_finished(&mut request.worker);
                    self.push_notice(Notice {
                        title: format!("{} launch stopped without a result.", request.tool.label()),
                        detail: Some(request.path.display().to_string()),
                        error: true,
                    });
                }
                Ok(Ok(())) => {
                    let mut request = self.launches.remove(index);
                    reap_finished(&mut request.worker);
                    // Only now is the access real: the OS accepted the spawn, and it
                    // is recorded for the identity that was captured on acceptance.
                    self.log_access(request.user, request.file);
                    self.push_notice(Notice {
                        title: format!("{} launch command started.", request.tool.label()),
                        detail: Some(format!(
                            "{}\n\nThis build cannot confirm that {} finished loading the file.",
                            request.path.display(),
                            request.tool.label()
                        )),
                        error: false,
                    });
                }
                Ok(Err(error)) => {
                    let mut request = self.launches.remove(index);
                    reap_finished(&mut request.worker);
                    self.push_notice(Notice {
                        title: format!("{} could not be started.", request.tool.label()),
                        detail: Some(format!("{}\n\n{}", error.message(), request.path.display())),
                        error: true,
                    });
                }
            }
        }
    }

    /// Queues the access record for the identity captured when the launch was
    /// accepted, so a later user change cannot re-attribute it.
    fn log_access(&mut self, user: UserId, file: FileId) {
        let Some(catalog) = &self.catalog else {
            return;
        };
        match crate::projects::log_file_access(catalog, user, file) {
            Ok(pending) => {
                self.access.push(pending);
                // Kept until answered: dropping a reply would lose a write failure.
                self.ctx.request_repaint_after(Duration::from_millis(40));
            }
            Err(error) => self.push_notice(notice_from_error("File access", &error)),
        }
    }

    /// Every accepted access record is answered on its own, so a burst of launches
    /// never replaces an earlier reply.
    fn poll_access(&mut self) {
        let mut index = 0;
        while index < self.access.len() {
            let reply = self.access[index].try_poll();
            match reply {
                ReplyState::Waiting => index += 1,
                ReplyState::Ready(()) => {
                    self.access.remove(index);
                }
                ReplyState::Failed(error) => {
                    self.access.remove(index);
                    self.push_notice(notice_from_error("File access", &error));
                }
                ReplyState::Finished => {
                    self.access.remove(index);
                }
            }
        }
    }

    /// Reveals one file in the native file manager, off the UI thread.
    pub fn reveal(&mut self, file: &ProjectFile) {
        if self.closing {
            return;
        }
        let Some(path) = file.path.clone() else {
            self.push_notice(Notice {
                title: "This file has no usable path on this machine.".to_string(),
                detail: file.display.clone(),
                error: true,
            });
            return;
        };
        let (tx, rx) = channel();
        let ctx = self.ctx.clone();
        let worker = std::thread::Builder::new()
            .name("vfx-reveal".to_string())
            .spawn(move || {
                let result = launch::reveal(&path);
                let _ = tx.send(result);
                ctx.request_repaint();
            })
            .ok();
        self.reveals.push(RevealRequest {
            replies: rx,
            worker,
        });
    }

    /// Every accepted reveal is drained independently; a running worker is never
    /// joined, so no frame waits for the file manager.
    fn poll_reveal(&mut self) {
        let mut index = 0;
        while index < self.reveals.len() {
            let reply = self.reveals[index].replies.try_recv();
            match reply {
                Err(TryRecvError::Empty) => {
                    reap_finished(&mut self.reveals[index].worker);
                    index += 1;
                }
                Err(TryRecvError::Disconnected) => {
                    let mut request = self.reveals.remove(index);
                    reap_finished(&mut request.worker);
                }
                Ok(Ok(())) => {
                    let mut request = self.reveals.remove(index);
                    reap_finished(&mut request.worker);
                }
                Ok(Err(error)) => {
                    let mut request = self.reveals.remove(index);
                    reap_finished(&mut request.worker);
                    self.push_notice(Notice {
                        title: "The file manager could not be opened.".to_string(),
                        detail: Some(error.message().to_string()),
                        error: true,
                    });
                }
            }
        }
    }

    // ----------------------------------------------------------------- closing

    /// Starts the coordinated stop. No new request is accepted, and the catalog
    /// queue is closed only after every accepted launch, reveal and access record
    /// drained, so no queued write is lost and no reply is dropped. Nothing here
    /// blocks: a live worker thread is never joined.
    pub fn begin_shutdown(&mut self) {
        self.closing = true;
        self.scan.cancelled = true;
        self.scan.queue.clear();
        self.scan.projects = None;
        if let Some(control) = &self.scan.control {
            control.cancel();
        }
        if let Some(worker) = &mut self.settings_worker {
            worker.begin_shutdown();
        }
        self.try_close_catalog();
    }

    /// True while an accepted launch, reveal or access record is still unanswered.
    fn actions_pending(&self) -> bool {
        !self.launches.is_empty() || !self.reveals.is_empty() || !self.access.is_empty()
    }

    /// Closes the catalog request queue once the accepted actions drained. Called
    /// from `poll`, so the drain continues during closing while the catalog still
    /// accepts the access records those launches produced.
    fn try_close_catalog(&mut self) {
        if !self.closing || self.shutdown.is_some() || self.actions_pending() {
            return;
        }
        if let Some(catalog) = self.catalog.take() {
            self.shutdown = Some(catalog.shutdown());
        }
    }

    /// True while the coordinated stop is unfinished. Keeps repainting so pending
    /// replies are observed without a frame ever blocking on a thread.
    pub fn shutdown_pending(&mut self) -> bool {
        self.try_close_catalog();
        let settings_pending = self
            .settings_worker
            .as_mut()
            .is_some_and(SettingsWorker::shutdown_pending);
        let mut catalog_pending = false;
        if let Some(shutdown) = &mut self.shutdown {
            if shutdown.try_finish() {
                self.shutdown = None;
            } else {
                catalog_pending = true;
            }
        }
        if !settings_pending && !catalog_pending && !self.actions_pending() {
            return false;
        }
        self.ctx.request_repaint_after(Duration::from_millis(40));
        true
    }

    // ------------------------------------------------------------------- status

    pub fn catalog_error(&self) -> Option<&str> {
        self.catalog_error.as_deref()
    }

    pub fn settings_error(&self) -> Option<&str> {
        self.settings_error.as_deref()
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    pub fn user(&self) -> Option<&UserRecord> {
        self.user.as_ref()
    }

    pub fn users(&self) -> &[UserRecord] {
        &self.users_list
    }

    pub fn tab(&self) -> ProjectList {
        self.tab
    }

    pub fn filter(&self) -> &str {
        &self.filter_edit
    }

    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    pub fn projects_error(&self) -> Option<&str> {
        self.projects_error.as_deref()
    }

    pub fn selected(&self) -> Option<ProjectId> {
        self.selected
    }

    pub fn selected_project(&self) -> Option<&Project> {
        let selected = self.selected?;
        self.projects.iter().find(|project| project.id == selected)
    }

    pub fn files(&self) -> &[FileGroup] {
        &self.files
    }

    pub fn files_error(&self) -> Option<&str> {
        self.files_error.as_deref()
    }

    /// Every result shown in the Projects pane, newest last.
    pub fn notices(&self) -> &[Notice] {
        &self.notices
    }

    pub fn dismiss_notice(&mut self, index: usize) {
        if index < self.notices.len() {
            self.notices.remove(index);
        }
    }

    /// Catalog open, no startup failure, an identity chosen and no coordinated stop
    /// in progress: the gate for every action that writes or starts a process.
    pub fn actions_enabled(&self) -> bool {
        !self.closing
            && self.catalog.is_some()
            && (self.catalog_stage == CatalogStage::Ready
                || self.catalog_stage == CatalogStage::CreatingAdmin)
            && self.catalog_error.is_none()
            && self.settings_error.is_none()
            && self.user.is_some()
    }

    /// Why actions are unavailable, for the disabled control's tooltip.
    pub fn actions_blocked_reason(&self) -> Option<String> {
        if self.closing {
            return Some("The application is closing.".to_string());
        }
        if let Some(error) = &self.catalog_error {
            return Some(error.clone());
        }
        if let Some(error) = &self.settings_error {
            return Some(error.clone());
        }
        if self.catalog.is_none() || self.catalog_stage != CatalogStage::Ready {
            return Some("The catalog is still opening.".to_string());
        }
        if self.user.is_none() {
            return Some(
                "Choose who is using the app first; recorded actions are attributed to that user."
                    .to_string(),
            );
        }
        None
    }

    pub fn registration_busy(&self) -> bool {
        matches!(
            self.registration,
            Registration::Choosing(_) | Registration::Planning(_) | Registration::Registering(_)
        )
    }
}

/// Registration state the Projects pane renders.
pub enum RegistrationStatus<'a> {
    Busy(&'static str),
    Summary(&'a RegistrationSummary),
    Failed(&'a str),
}

impl RegistrationStatus<'_> {
    /// One line for the pane, with the counts the summary actually reports.
    pub fn summary_line(&self) -> String {
        match self {
            Self::Busy(label) => (*label).to_string(),
            Self::Failed(message) => format!("Folders were not added: {message}"),
            Self::Summary(summary) => {
                let mut parts = Vec::new();
                if !summary.added.is_empty() {
                    parts.push(format!("{} added", summary.added.len()));
                }
                if !summary.skipped.is_empty() {
                    parts.push(format!("{} already registered", summary.skipped.len()));
                }
                if !summary.failed.is_empty() {
                    parts.push(format!("{} failed", summary.failed.len()));
                }
                if parts.is_empty() {
                    "No folders were added.".to_string()
                } else {
                    parts.join(", ")
                }
            }
        }
    }

    /// Per-folder detail lines for the collapsible details.
    pub fn detail_lines(&self) -> Vec<String> {
        match self {
            Self::Summary(summary) => registration_details(summary),
            Self::Failed(message) => vec![message.to_string()],
            Self::Busy(_) => Vec::new(),
        }
    }
}

fn notice_from_error(context: &str, error: &StorageError) -> Notice {
    Notice {
        title: format!("{context} could not be updated."),
        detail: Some(error.message().to_string()),
        error: true,
    }
}

/// Joins a worker that already finished and keeps a live one for a later frame, so
/// closing never waits on a running thread; a dropped handle only detaches it.
fn reap_finished(worker: &mut Option<JoinHandle<()>>) {
    if worker.as_ref().is_some_and(JoinHandle::is_finished) {
        if let Some(handle) = worker.take() {
            let _ = handle.join();
        }
    }
}

fn start_planning(roots: Vec<PathBuf>, ctx: egui::Context) -> Receiver<RegistrationPlan> {
    let (tx, rx) = channel();
    let _ = std::thread::Builder::new()
        .name("vfx-registration-plan".to_string())
        .spawn(move || {
            let plan = crate::projects::plan_registration(&roots);
            let _ = tx.send(plan);
            ctx.request_repaint();
        });
    rx
}

fn start_walk(
    jobs: &mut Jobs,
    ctx: &egui::Context,
    title: String,
    scope: PathBuf,
) -> Result<
    (
        JobId,
        Receiver<(Result<ScanSnapshot, PathError>, JobControl)>,
    ),
    std::io::Error,
> {
    let filters = discovery::default_filters(ScanPurpose::Projects)
        .ok_or_else(|| std::io::Error::other("the project scan filter preset is unavailable"))?;
    let (tx, rx) = channel();
    let job = jobs.start(ctx, title, move |context: JobContext| {
        let JobContext {
            control, events, ..
        } = context;
        let request = ScanRequest {
            root: scope,
            purpose: ScanPurpose::Projects,
            recursive: true,
            filters,
        };
        let result = discovery::walk(&request, &control, &events);
        let progress = JobProgress {
            completed: result
                .as_ref()
                .map(|snapshot| snapshot.entries.len() as u64)
                .unwrap_or(0),
            total: None,
            bytes_done: 0,
            bytes_total: None,
            current: None,
        };
        let outcome = match &result {
            Ok(snapshot) => match snapshot.coverage {
                Coverage::Cancelled => JobOutcome::Cancelled,
                Coverage::RootUnavailable => JobOutcome::Failed,
                Coverage::Complete => JobOutcome::Succeeded,
                Coverage::Partial => JobOutcome::Partial,
            },
            Err(error) => {
                events.diagnostic(JobPhase::Scanning, error.message().to_string());
                JobOutcome::Failed
            }
        };
        let _ = tx.send((result, control.clone()));
        events.finish(outcome, progress);
    })?;
    Ok((job, rx))
}

/// Folder name for progress copy, falling back to the whole path.
fn folder_label(scope: &Path) -> String {
    scope
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| scope.display().to_string())
}

fn describe_reconcile(summary: &ReconcileSummary) -> String {
    if !summary.applied {
        return format!(
            "{}: {} seen, nothing recorded",
            folder_label(&summary.scope),
            summary.observed
        );
    }
    let mut line = format!(
        "{}: {} seen, {} new, {} updated",
        folder_label(&summary.scope),
        summary.observed,
        summary.files_created,
        summary.files_updated
    );
    if summary.prune_suppressed.is_some() {
        line.push_str(", unseen files kept");
    }
    if !summary.issues.is_empty() {
        line.push_str(&format!(", {} issues", summary.issues.len()));
    }
    line
}

fn registration_details(summary: &RegistrationSummary) -> Vec<String> {
    let mut lines = Vec::new();
    for added in &summary.added {
        lines.push(format!(
            "Added {} ({})",
            added.root.display(),
            if added.revived {
                "restored"
            } else {
                "new project"
            }
        ));
    }
    for skipped in &summary.skipped {
        lines.push(format!(
            "Skipped {} ({})",
            skipped.root.display(),
            skipped.reason.label()
        ));
    }
    for issue in &summary.failed {
        lines.push(format!(
            "Failed {} ({})",
            issue.path.display(),
            issue.message
        ));
    }
    lines
}
