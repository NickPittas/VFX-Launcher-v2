//! Worker job identity, control and event plumbing.
//!
//! This is the F0 foundation half of `CONTRACTS.md` §6: a process-local
//! monotonically increasing [`JobId`], cooperative [`JobControl`], bounded
//! [`JobEvents`] transport and a [`Jobs`] registry the UI polls. Feature packets
//! supply the worker body; this module performs no file I/O and no persistence.
//!
//! Deliberate limits of this packet: unfinished runs are not recorded on disk
//! yet, and [`Jobs::begin_shutdown`] only signals cancellation and reaps threads
//! that have already finished. A worker blocked in OS I/O still delays its own
//! exit, and that limit is surfaced in the UI rather than masked by a timeout.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use eframe::egui;

/// Monotonically increasing, process-local job identifier.
pub type JobId = u64;

/// Minimum spacing between two repaint requests caused by progress updates.
/// Progress values themselves are never dropped: the newest one always replaces
/// the previous value in the job's single latest-progress slot.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Repaint heartbeat while a worker has not been reaped yet. Covers the final
/// wakeup race so a terminal report or a finished thread cannot be stranded.
pub const LIVE_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Upper bound on diagnostics queued by one worker and not yet drained.
const MAX_QUEUED_DIAGNOSTICS: usize = 64;

/// Upper bound on diagnostics retained for display per job.
const MAX_DIAGNOSTICS: usize = 64;

/// Diagnostics one job may deliver per frame, so a noisy worker cannot starve rendering.
const MAX_DIAGNOSTICS_PER_POLL: usize = 32;

/// Coarse stage reported by a worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobPhase {
    FindingFolders,
    MatchingFolders,
    Counting,
    Scanning,
    Validating,
    Copying,
    Finishing,
}

impl JobPhase {
    pub fn label(self) -> &'static str {
        match self {
            Self::FindingFolders => "Finding project folders",
            Self::MatchingFolders => "Matching project folders",
            Self::Counting => "Counting files",
            Self::Scanning => "Scanning",
            Self::Validating => "Validating",
            Self::Copying => "Copying",
            Self::Finishing => "Finishing",
        }
    }
}

/// Terminal batch outcome. Counts live in [`JobProgress`] on the terminal event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobOutcome {
    Succeeded,
    Partial,
    Failed,
    Cancelled,
}

impl JobOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Succeeded => "Finished",
            Self::Partial => "Finished with failures",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// Counters reported with a job event. Byte totals stay optional because they are
/// unknown until an operation is planned.
#[derive(Clone, Debug, Default)]
pub struct JobProgress {
    pub completed: u64,
    pub total: Option<u64>,
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
    pub current: Option<PathBuf>,
}

impl JobProgress {
    /// Completed fraction for a determinate total, `0.0` when the total is unknown.
    pub fn fraction(&self) -> f32 {
        match self.total {
            Some(total) if total > 0 => (self.completed as f32 / total as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }
}

/// Cooperative checkpoint result returned to a worker between items or chunks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Checkpoint {
    Continue,
    Cancelled,
}

#[derive(Default)]
struct ControlShared {
    cancel: AtomicBool,
    /// Pause was requested by the user; the worker may still be mid-item.
    pause_requested: AtomicBool,
    /// The worker acknowledged the request by reaching a checkpoint.
    paused: AtomicBool,
    lock: Mutex<()>,
    wake: Condvar,
    /// Set once at spawn so acknowledgements can wake the UI without polling.
    ui: Mutex<Option<egui::Context>>,
}

/// Clonable handle a worker checks between items. Pause blocks inside
/// [`JobControl::checkpoint`]; cancel wakes a paused worker.
#[derive(Clone, Default)]
pub struct JobControl {
    shared: Arc<ControlShared>,
}

impl JobControl {
    /// Signals the pause request. Blocks at the worker's next checkpoint; until
    /// then [`JobControl::is_pausing`] is true and [`JobControl::is_paused`] is not.
    pub fn pause(&self) {
        let guard = lock(&self.shared.lock);
        self.shared.pause_requested.store(true, Ordering::SeqCst);
        drop(guard);
        self.wake_ui();
    }

    pub fn resume(&self) {
        // Update the predicate under the wait mutex to prevent a lost wakeup.
        let guard = lock(&self.shared.lock);
        self.shared.pause_requested.store(false, Ordering::SeqCst);
        self.shared.wake.notify_all();
        drop(guard);
        self.wake_ui();
    }

    pub fn cancel(&self) {
        let guard = lock(&self.shared.lock);
        self.shared.cancel.store(true, Ordering::SeqCst);
        self.shared.pause_requested.store(false, Ordering::SeqCst);
        self.shared.wake.notify_all();
        drop(guard);
        self.wake_ui();
    }

    /// A pause was requested and the worker has not acknowledged it yet.
    pub fn is_pausing(&self) -> bool {
        self.shared.pause_requested.load(Ordering::SeqCst)
            && !self.shared.paused.load(Ordering::SeqCst)
    }

    /// The worker acknowledged the request and is idle at a safe boundary.
    pub fn is_paused(&self) -> bool {
        self.shared.paused.load(Ordering::SeqCst)
    }

    pub fn is_pause_requested(&self) -> bool {
        self.shared.pause_requested.load(Ordering::SeqCst)
    }

    pub fn is_cancelled(&self) -> bool {
        self.shared.cancel.load(Ordering::SeqCst)
    }

    /// Cooperative between-item checkpoint. Blocks while paused and returns as
    /// soon as the job is cancelled.
    pub fn checkpoint(&self) -> Checkpoint {
        if self.is_cancelled() {
            return Checkpoint::Cancelled;
        }
        if !self.is_pause_requested() {
            return Checkpoint::Continue;
        }

        let mut guard = lock(&self.shared.lock);
        if self.is_pause_requested() && !self.is_cancelled() {
            // Acknowledge before sleeping: from here on the worker is idle at a
            // safe boundary, which is what the UI reports as Paused.
            self.shared.paused.store(true, Ordering::SeqCst);
            self.wake_ui();
            while self.is_pause_requested() && !self.is_cancelled() {
                guard = self
                    .shared
                    .wake
                    .wait(guard)
                    .unwrap_or_else(|poison| poison.into_inner());
            }
            self.shared.paused.store(false, Ordering::SeqCst);
        }
        drop(guard);
        // Reflect the resume/acknowledgement transition without waiting for the
        // next progress update.
        self.wake_ui();

        if self.is_cancelled() {
            Checkpoint::Cancelled
        } else {
            Checkpoint::Continue
        }
    }

    fn attach_ui(&self, ctx: &egui::Context) {
        *lock(&self.shared.ui) = Some(ctx.clone());
    }

    fn wake_ui(&self) {
        let ctx = lock(&self.shared.ui).clone();
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    }
}

/// Queued diagnostic: the stage that produced it and its text.
type Diagnostic = (JobPhase, String);

#[derive(Default)]
struct DiagnosticQueue {
    pending: VecDeque<Diagnostic>,
    omitted: u64,
}

#[derive(Clone, Debug)]
struct TerminalReport {
    phase: JobPhase,
    progress: JobProgress,
    outcome: JobOutcome,
}

/// Transport between one worker and the UI. Bounded by construction: one
/// replaceable latest-progress slot, one bounded diagnostic queue with drop
/// accounting, and one terminal slot that no other traffic can displace.
#[derive(Default)]
struct EventShared {
    progress: Mutex<Option<(JobPhase, JobProgress)>>,
    diagnostics: Mutex<DiagnosticQueue>,
    terminal: Mutex<Option<TerminalReport>>,
    /// Set when the producer side is gone, whatever the reason.
    sink_gone: AtomicBool,
}

/// Producer side of one job's event transport. Cloned state is shared with the
/// registry; dropping this sink is the worker-scope exit guard.
pub struct JobEvents {
    shared: Arc<EventShared>,
    ctx: egui::Context,
    last_repaint: Mutex<Option<Instant>>,
    last_progress: Mutex<JobProgress>,
}

impl JobEvents {
    fn new(shared: Arc<EventShared>, ctx: egui::Context) -> Self {
        Self {
            shared,
            ctx,
            last_repaint: Mutex::new(None),
            last_progress: Mutex::new(JobProgress::default()),
        }
    }

    /// Replaces the job's latest progress value. The newest value always wins and
    /// the wakeup is throttled to `PROGRESS_INTERVAL`, but a throttled wakeup is
    /// deferred rather than dropped, so the newest value is always painted.
    pub fn progress(&self, phase: JobPhase, progress: JobProgress) {
        *lock(&self.last_progress) = progress.clone();
        *lock(&self.shared.progress) = Some((phase, progress));
        match self.progress_repaint_delay() {
            Some(delay) => self.ctx.request_repaint_after(delay),
            None => self.ctx.request_repaint(),
        }
    }

    /// Last producer progress, including values already drained by the UI.
    pub fn last_progress(&self) -> JobProgress {
        lock(&self.last_progress).clone()
    }

    /// Queues a diagnostic. Beyond the queue bound the message is dropped and
    /// counted so the UI can report omissions instead of growing without limit.
    pub fn diagnostic(&self, phase: JobPhase, message: impl Into<String>) {
        let message = message.into();
        {
            let mut queue = lock(&self.shared.diagnostics);
            if queue.pending.len() >= MAX_QUEUED_DIAGNOSTICS {
                queue.omitted += 1;
            } else {
                queue.pending.push_back((phase, message));
            }
        }
        self.ctx.request_repaint();
    }

    /// Emits the single terminal report. Consuming the sink means a worker cannot
    /// emit a second one, and the terminal slot is independent of the bounded
    /// progress/diagnostic traffic.
    pub fn finish(self, outcome: JobOutcome, progress: JobProgress) {
        *lock(&self.shared.terminal) = Some(TerminalReport {
            phase: JobPhase::Finishing,
            progress,
            outcome,
        });
        self.ctx.request_repaint();
    }

    /// `None` means repaint now; `Some(delay)` schedules the trailing repaint that
    /// the throttle window still owes the newest progress value.
    fn progress_repaint_delay(&self) -> Option<Duration> {
        let mut last = lock(&self.last_repaint);
        if let Some(previous) = *last {
            let elapsed = previous.elapsed();
            if elapsed < PROGRESS_INTERVAL {
                return Some(PROGRESS_INTERVAL - elapsed);
            }
        }
        *last = Some(Instant::now());
        None
    }
}

impl Drop for JobEvents {
    fn drop(&mut self) {
        // Worker-scope exit guard. `Drop` runs on a normal return and while a
        // panic unwinds, and it runs after any event the worker sent, so the
        // disconnection is already stored when the repaint request wakes the UI.
        // A live worker is still covered by the registry's heartbeat until its
        // thread handle is reaped.
        self.shared.sink_gone.store(true, Ordering::SeqCst);
        self.ctx.request_repaint();
    }
}

/// What a worker receives: its identity, control handle and event sink.
pub struct JobContext {
    pub id: JobId,
    pub control: JobControl,
    pub events: JobEvents,
}

/// Snapshot the UI renders. Owned so the status bar never holds a borrow of [`Jobs`].
#[derive(Clone, Debug)]
pub struct JobStatus {
    pub id: JobId,
    pub title: String,
    pub phase: JobPhase,
    pub progress: JobProgress,
    /// Pause was requested but the worker has not acknowledged it.
    pub pause_requested: bool,
    /// The worker acknowledged the request and sits at a safe boundary.
    pub paused: bool,
    pub cancelling: bool,
    pub diagnostics: Vec<String>,
    pub omitted_diagnostics: u64,
    /// Terminal report, kept visible while the worker is still being reaped.
    pub outcome: Option<JobOutcome>,
    /// The worker thread finished and its handle was joined.
    pub reaped: bool,
}

impl JobStatus {
    /// A terminal report has been received.
    pub fn is_terminal(&self) -> bool {
        self.outcome.is_some()
    }

    /// The worker thread has finished and been joined.
    pub fn is_reaped(&self) -> bool {
        self.reaped
    }

    /// The worker still holds a thread handle: it stays controllable, visible and
    /// non-clearable even after reporting its outcome.
    pub fn is_busy(&self) -> bool {
        !self.reaped
    }

    /// Pause requested, safe boundary not reached yet.
    pub fn is_pausing(&self) -> bool {
        self.pause_requested && !self.paused
    }
}

struct TrackedJob {
    control: JobControl,
    shared: Arc<EventShared>,
    worker: Option<JoinHandle<()>>,
    status: JobStatus,
}

impl TrackedJob {
    fn drain(&mut self) {
        // Terminal report first: frame budgets and diagnostics can never hide it.
        let terminal = lock(&self.shared.terminal).take();
        if let Some(report) = terminal {
            self.status.phase = report.phase;
            self.status.progress = report.progress;
            self.status.outcome = Some(report.outcome);
        }

        // At most one progress value exists, so this is a constant-time read.
        let latest = lock(&self.shared.progress).take();
        if let Some((phase, progress)) = latest {
            if self.status.outcome.is_none() {
                self.status.phase = phase;
                self.status.progress = progress;
            }
        }

        let (drained, omitted) = {
            let mut queue = lock(&self.shared.diagnostics);
            let take = queue.pending.len().min(MAX_DIAGNOSTICS_PER_POLL);
            let drained: Vec<String> = queue
                .pending
                .drain(..take)
                .map(|(_phase, message)| message)
                .collect();
            let omitted = core::mem::take(&mut queue.omitted);
            (drained, omitted)
        };
        self.status.omitted_diagnostics = self.status.omitted_diagnostics.saturating_add(omitted);
        for message in drained {
            if self.status.diagnostics.len() < MAX_DIAGNOSTICS {
                self.status.diagnostics.push(message);
            } else {
                self.status.omitted_diagnostics = self.status.omitted_diagnostics.saturating_add(1);
            }
        }

        // Reap only handles whose thread has already ended: never block the UI.
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
        {
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
            self.status.reaped = true;
        }

        if self.status.outcome.is_none()
            && (self.status.reaped || self.shared.sink_gone.load(Ordering::SeqCst))
        {
            // Finish may race the first read this frame. Read again after
            // observing producer exit before synthesizing a missing report.
            let terminal = lock(&self.shared.terminal).take();
            if let Some(report) = terminal {
                self.status.phase = report.phase;
                self.status.progress = report.progress;
                self.status.outcome = Some(report.outcome);
            } else {
                self.report_failure("The worker stopped without a terminal report.");
            }
        }

        // Snapshot control state last so the strip never renders an older state.
        self.status.pause_requested = self.control.is_pause_requested();
        self.status.paused = self.control.is_paused();
        self.status.cancelling = self.control.is_cancelled();
    }

    fn report_failure(&mut self, message: &str) {
        self.status.outcome = Some(JobOutcome::Failed);
        if self.status.diagnostics.len() < MAX_DIAGNOSTICS {
            self.status.diagnostics.push(message.to_owned());
        } else {
            self.status.omitted_diagnostics = self.status.omitted_diagnostics.saturating_add(1);
        }
    }
}

/// Registry of running and recently finished jobs.
pub struct Jobs {
    next_id: JobId,
    jobs: Vec<TrackedJob>,
    ctx: Option<egui::Context>,
    shutting_down: bool,
}

impl Default for Jobs {
    fn default() -> Self {
        Self::new()
    }
}

impl Jobs {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            jobs: Vec::new(),
            ctx: None,
            shutting_down: false,
        }
    }

    /// Starts a worker thread. Returns an I/O error when the thread cannot be spawned.
    pub fn start<F>(
        &mut self,
        ctx: &egui::Context,
        title: impl Into<String>,
        work: F,
    ) -> std::io::Result<JobId>
    where
        F: FnOnce(JobContext) + Send + 'static,
    {
        if self.shutting_down {
            return Err(std::io::Error::other("The application is closing."));
        }
        let id = self.next_id;
        self.next_id += 1;
        let title = title.into();

        let control = JobControl::default();
        control.attach_ui(ctx);
        let worker_control = control.clone();
        let shared = Arc::new(EventShared::default());
        let worker_events = JobEvents::new(Arc::clone(&shared), ctx.clone());

        let worker = thread::Builder::new()
            .name(format!("vfx-job-{id}"))
            .spawn(move || {
                work(JobContext {
                    id,
                    control: worker_control,
                    events: worker_events,
                });
            })?;

        self.ctx = Some(ctx.clone());
        self.jobs.push(TrackedJob {
            control,
            shared,
            worker: Some(worker),
            status: JobStatus {
                id,
                title,
                phase: JobPhase::Scanning,
                progress: JobProgress::default(),
                pause_requested: false,
                paused: false,
                cancelling: false,
                diagnostics: Vec::new(),
                omitted_diagnostics: 0,
                outcome: None,
                reaped: false,
            },
        });

        Ok(id)
    }

    /// Drains pending events. Call once per frame before rendering job state.
    pub fn poll(&mut self) {
        for job in &mut self.jobs {
            job.drain();
        }

        // Heartbeat while a worker is unreaped: covers the race between a worker's
        // exit wakeup and its thread becoming joinable.
        if self
            .jobs
            .iter()
            .any(|job| job.status.is_busy() || !lock(&job.shared.diagnostics).pending.is_empty())
        {
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(LIVE_POLL_INTERVAL);
            }
        }
    }

    pub fn statuses(&self) -> impl Iterator<Item = &JobStatus> {
        self.jobs.iter().map(|job| &job.status)
    }

    /// Most recently started job, running or finished.
    pub fn current(&self) -> Option<&JobStatus> {
        self.jobs.last().map(|job| &job.status)
    }

    /// True while any worker thread has not been reaped, including one that has
    /// already reported a terminal outcome.
    pub fn is_busy(&self) -> bool {
        self.jobs.iter().any(|job| job.status.is_busy())
    }

    /// Number of workers that have not been reaped yet.
    pub fn live_job_count(&self) -> usize {
        self.jobs.iter().filter(|job| job.status.is_busy()).count()
    }

    pub fn cancel(&mut self, id: JobId) -> bool {
        match self.control(id) {
            Some(control) => {
                control.cancel();
                true
            }
            None => false,
        }
    }

    pub fn pause(&mut self, id: JobId) -> bool {
        match self.control(id) {
            Some(control) => {
                control.pause();
                true
            }
            None => false,
        }
    }

    pub fn resume(&mut self, id: JobId) -> bool {
        match self.control(id) {
            Some(control) => {
                control.resume();
                true
            }
            None => false,
        }
    }

    /// Drops only jobs whose terminal report was received *and* whose worker was
    /// joined. A worker that reported but is still running stays registered, so
    /// clearing never detaches a live thread.
    pub fn clear_finished(&mut self) {
        self.jobs
            .retain(|job| !(job.status.is_terminal() && job.status.is_reaped()));
    }

    /// Starts a coordinated, non-blocking shutdown: signals cancellation to every
    /// worker, then polls once to reap threads that have already finished. It
    /// never joins a live worker and never waits, so it is safe to call on the UI
    /// thread and from `on_exit`. The caller keeps polling and closes the window
    /// only once [`Jobs::is_busy`] is false.
    pub fn begin_shutdown(&mut self) {
        self.shutting_down = true;
        for job in &self.jobs {
            job.control.cancel();
        }
        self.poll();
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down
    }

    fn control(&self, id: JobId) -> Option<&JobControl> {
        self.jobs
            .iter()
            .find(|job| job.status.id == id)
            .map(|job| &job.control)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}
