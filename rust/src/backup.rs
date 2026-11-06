//! NAS Backup's collect → compare → preserve-existing-copy workflow.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};

use crate::discovery::{self, Coverage, FileEntry, ScanPurpose, ScanRequest, ScanSnapshot};
use crate::jobs::{Checkpoint, JobControl, JobEvents, JobPhase, JobProgress};
use crate::transfers::{self, OperationPlan, TransferError};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackupJob {
    #[serde(flatten)]
    pub import_metadata: std::collections::BTreeMap<String, serde_json::Value>,
    pub source: String,
    pub destination: String,
    pub last_browse_dir: String,
    pub excludes: String,
    pub workers: usize,
    pub schedule_enabled: bool,
    pub schedule_mode: String,
    pub schedule_interval_minutes: u64,
    pub schedule_time: String,
}

impl Default for BackupJob {
    fn default() -> Self {
        Self {
            import_metadata: Default::default(),
            source: String::new(),
            destination: String::new(),
            last_browse_dir: String::new(),
            excludes: String::new(),
            workers: 4,
            schedule_enabled: false,
            schedule_mode: "interval".into(),
            schedule_interval_minutes: 240,
            schedule_time: "02:00".into(),
        }
    }
}

impl BackupJob {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=16).contains(&self.workers) {
            return Err("Choose between 1 and 16 copy workers.".into());
        }
        if self.schedule_mode != "interval" && self.schedule_mode != "daily" {
            return Err("Choose an interval or daily schedule.".into());
        }
        if self.schedule_interval_minutes == 0
            || self.schedule_interval_minutes.checked_mul(60).is_none()
        {
            return Err(
                "The backup interval must be a positive number of minutes within range.".into(),
            );
        }
        self.daily_time()?;
        if self.excludes.len() > 64 * 1024 {
            return Err("The exclusion list is too long.".into());
        }
        Ok(())
    }

    pub fn roots(&self) -> Result<(PathBuf, PathBuf), String> {
        self.validate()?;
        let source = PathBuf::from(self.source.trim());
        let destination = PathBuf::from(self.destination.trim());
        if !source.is_absolute() || !destination.is_absolute() {
            return Err("Choose absolute source and destination folders.".into());
        }
        if !source.is_dir() || !destination.is_dir() {
            return Err("Both backup folders must exist and be available.".into());
        }
        Ok((source, destination))
    }

    pub fn daily_time(&self) -> Result<(u32, u32), String> {
        let (hour, minute) = self
            .schedule_time
            .split_once(':')
            .ok_or("Enter the daily time as HH:MM.")?;
        if hour.len() != 2 || minute.len() != 2 {
            return Err("Enter the daily time as HH:MM.".into());
        }
        let hour: u32 = hour
            .parse()
            .map_err(|_| "The hour must be between 00 and 23.")?;
        let minute: u32 = minute
            .parse()
            .map_err(|_| "The minute must be between 00 and 59.")?;
        if hour > 23 || minute > 59 {
            return Err("The daily backup time is outside 00:00–23:59.".into());
        }
        Ok((hour, minute))
    }

    pub fn import(bytes: &[u8]) -> Result<Self, String> {
        let job: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("The backup job cannot be read: {error}"))?;
        if let Some(version) = job.import_metadata.get("schema_version") {
            if version.as_u64() != Some(1) {
                return Err("This backup job version is unsupported; nothing was imported.".into());
            }
        }
        job.validate()?;
        Ok(job)
    }

    pub fn export(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        serde_json::to_vec_pretty(self).map_err(|error| error.to_string())
    }
}

#[derive(Serialize, Deserialize)]
struct SavedJob {
    schema_version: u32,
    job: BackupJob,
}

pub fn save_job(path: &Path, job: &BackupJob) -> Result<(), String> {
    job.validate()?;
    crate::config::write_versioned(
        path,
        "backup settings",
        &SavedJob {
            schema_version: 1,
            job: job.clone(),
        },
    )
    .map_err(|error| error.to_string())
}

pub fn load_job(path: &Path) -> Result<BackupJob, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BackupJob::default());
        }
        Err(error) => return Err(format!("Backup settings could not be read: {error}")),
    };
    let stored: SavedJob = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Backup settings are invalid: {error}"))?;
    if stored.schema_version != 1 {
        return Err(
            "This backup settings version is unsupported; the file was left unchanged.".into(),
        );
    }
    stored.job.validate()?;
    Ok(stored.job)
}

#[derive(Clone, Debug)]
pub struct BackupScan {
    pub snapshot: ScanSnapshot,
    pub up_to_date: Vec<FileEntry>,
    pub compared: usize,
    pub comparison_issues: Vec<String>,
}

/// Collect the legacy .nk/.aep set, then compare destination mtimes with bounded workers.
pub fn scan(
    job: &BackupJob,
    control: &JobControl,
    events: &JobEvents,
) -> Result<BackupScan, String> {
    let (source, destination) = job.roots()?;
    let mut filters =
        discovery::default_filters(ScanPurpose::Backup).ok_or("Backup filters are unavailable.")?;
    filters.excludes = job
        .excludes
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    let mut snapshot = discovery::walk(
        &ScanRequest {
            root: source.clone(),
            purpose: ScanPurpose::Backup,
            recursive: true,
            filters,
        },
        control,
        events,
    )
    .map_err(|error| error.to_string())?;
    let entries = std::mem::take(&mut snapshot.entries);
    let total = entries.len();
    let next = AtomicUsize::new(0);
    let completed = AtomicUsize::new(0);
    let progress = Mutex::new(());
    let (sender, receiver) = mpsc::channel();
    std::thread::scope(|scope| {
        for _ in 0..job.workers.min(total) {
            let sender = sender.clone();
            let entries = &entries;
            let next = &next;
            let completed = &completed;
            let source = &source;
            let destination = &destination;
            let progress = &progress;
            scope.spawn(move || loop {
                if control.checkpoint() == Checkpoint::Cancelled { break; }
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(entry) = entries.get(index) else { break; };
                let relative = match entry.path.strip_prefix(source) { Ok(relative) => relative, Err(_) => break };
                let target = destination.join(relative);
                let (changed, issue) = match fs::metadata(&target) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => (true, None),
                    Err(error) => (true, Some(format!("{} could not be compared: {error}", target.display()))),
                    Ok(metadata) => match (entry.stamp.modified, metadata.modified()) {
                        (Some(source_time), Ok(destination_time)) => (source_time > destination_time, None),
                        _ => (true, Some(format!("{} has no comparable modification time; validation is required.", target.display()))),
                    },
                };
                if sender.send((index, changed, issue)).is_err() { break; }
                // Serialize the count and event together so parallel completions cannot move progress backwards.
                if let Ok(_guard) = progress.lock() {
                    let done = completed.fetch_add(1, Ordering::Relaxed) + 1;
                    events.progress(JobPhase::Scanning, JobProgress { completed: done as u64, total: Some(total as u64), current: Some(entry.path.clone()), ..Default::default() });
                }
            });
        }
        drop(sender);
    });
    let mut results = receiver.into_iter().collect::<Vec<_>>();
    results.sort_by_key(|(index, _, _)| *index);
    let compared = results.len();
    let mut up_to_date = Vec::new();
    let mut comparison_issues = Vec::new();
    for (index, changed, issue) in results {
        if let Some(issue) = issue {
            comparison_issues.push(issue);
        }
        if changed {
            snapshot.entries.push(entries[index].clone());
        } else {
            up_to_date.push(entries[index].clone());
        }
    }
    if control.is_cancelled() {
        snapshot.coverage = Coverage::Cancelled;
    }
    if total == 0 {
        events.progress(
            JobPhase::Scanning,
            JobProgress {
                total: Some(0),
                ..Default::default()
            },
        );
    }
    Ok(BackupScan {
        snapshot,
        up_to_date,
        compared,
        comparison_issues,
    })
}

pub fn plan(
    id: impl Into<String>,
    revision: u64,
    scan: &BackupScan,
    destination: &Path,
) -> Result<OperationPlan, TransferError> {
    let mut plan = transfers::plan_backup_copy(id, revision, &scan.snapshot, destination)?;
    plan.warnings.extend(scan.comparison_issues.iter().cloned());
    Ok(plan)
}
