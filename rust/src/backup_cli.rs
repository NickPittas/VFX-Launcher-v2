//! Headless backup uses the same scan, validation and journaled executor as the UI.
use crate::backup::BackupJob;
use crate::jobs::{JobOutcome, Jobs};
use crate::storage::Catalog;
use crate::transfers::{self, RunState};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

pub fn run(arguments: Vec<OsString>, mut data_dir: Option<PathBuf>) -> Result<u8, String> {
    let mut args = arguments.into_iter();
    let mut source = None;
    let mut destination = None;
    let mut job_path = None;
    let mut dry_run = false;
    while let Some(argument) = args.next() {
        if argument == "--dry-run" {
            if dry_run {
                return Err("--dry-run was given more than once.".into());
            }
            dry_run = true;
        } else if argument == "--job"
            || argument == "--source"
            || argument == "--destination"
            || argument == "--data-dir"
        {
            let value = args
                .next()
                .ok_or_else(|| format!("{} needs a path.", argument.to_string_lossy()))?;
            let slot = if argument == "--job" {
                &mut job_path
            } else if argument == "--source" {
                &mut source
            } else if argument == "--destination" {
                &mut destination
            } else {
                &mut data_dir
            };
            if slot.is_some() {
                return Err(format!(
                    "{} was given more than once.",
                    argument.to_string_lossy()
                ));
            }
            *slot = Some(PathBuf::from(value));
        } else {
            return Err(format!(
                "Unknown backup option: {}",
                argument.to_string_lossy()
            ));
        }
    }
    let job = if let Some(path) = job_path {
        if source.is_some() || destination.is_some() {
            return Err("Use either --job or --source with --destination.".into());
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
        if bytes.len() > 1024 * 1024 {
            return Err("The backup job exceeds 1 MiB.".into());
        }
        BackupJob::import(&bytes)?
    } else {
        if !dry_run {
            return Err("A backup that writes files requires --job; direct roots are available with --dry-run.".into());
        }
        let source = source.ok_or("Choose --job or provide both --source and --destination.")?;
        let destination = destination.ok_or("--destination is required.")?;
        BackupJob {
            source: source
                .to_str()
                .ok_or("The source cannot be represented in a backup JSON job.")?
                .into(),
            destination: destination
                .to_str()
                .ok_or("The destination cannot be represented in a backup JSON job.")?
                .into(),
            ..Default::default()
        }
    };
    job.roots()?;
    let mut catalog = if dry_run {
        None
    } else {
        let directory = match data_dir {
            Some(directory) => directory,
            None => crate::config::default_data_dir().map_err(|error| error.to_string())?,
        };
        if !directory.is_absolute() {
            return Err("--data-dir must be absolute.".into());
        }
        let catalog =
            Catalog::open(directory.join("catalog.sqlite3")).map_err(|error| error.to_string())?;
        catalog
            .info()
            .map_err(|error| error.to_string())?
            .wait()
            .map_err(|error| error.to_string())?;
        Some(catalog)
    };
    let client = catalog.as_ref().map(Catalog::client);
    let context = eframe::egui::Context::default();
    let mut jobs = Jobs::new();
    let (sender, receiver) = mpsc::channel();
    jobs.start(&context, "Backup", move |context| {
        let result = (|| -> Result<u8, String> {
            let scan = crate::backup::scan(&job, &context.control, &context.events)?;
            println!(
                "Scanned: {} | Up to date: {} | To copy: {}",
                scan.compared,
                scan.up_to_date.len(),
                scan.snapshot.entries.len()
            );
            let plan = crate::backup::plan(
                "backup-cli",
                1,
                &scan,
                std::path::Path::new(job.destination.trim()),
            )
            .map_err(|error| error.to_string())?;
            let review = transfers::validate_plan(&plan);
            for warning in &review.warnings {
                eprintln!("Warning: {warning}");
            }
            if let Some(refusal) = &review.refusal {
                return Err(refusal.clone());
            }
            if review.counts.blocked > 0 {
                for item in &review.items {
                    if let Some(reason) = &item.message {
                        eprintln!(
                            "{}: {reason}",
                            item.destination
                                .as_ref()
                                .map(|path| path.path().display().to_string())
                                .unwrap_or_else(|| "Unknown destination".into())
                        );
                    }
                }
                return Err(format!(
                    "{} operation(s) could not be validated; nothing was copied.",
                    review.counts.blocked
                ));
            }
            let validated = review
                .ready
                .ok_or("The backup could not produce a validated plan.")?;
            if dry_run {
                println!(
                    "Dry run: {} reviewed operations; no files or catalog were changed.",
                    review.counts.total
                );
                return Ok(0);
            }
            if scan.snapshot.entries.is_empty() {
                println!("Backup complete: no changed files.");
                return Ok(0);
            }
            let client = client.ok_or("The backup catalog is unavailable.")?;
            let report = transfers::execute(&client, validated, &context.control, &context.events)
                .map_err(|error| error.to_string())?;
            let counts = report.summary.counts;
            println!(
                "Run {}: {} | Copied: {} | Skipped: {} | Failed: {} | Cancelled: {} | Bytes: {}",
                report.summary.run_id,
                report.summary.state.as_str(),
                counts.copied,
                counts.skipped,
                counts.failed,
                counts.cancelled,
                counts
                    .committed_bytes
                    .map(|bytes| bytes.to_string())
                    .unwrap_or_else(|| "unknown".into())
            );
            Ok(if report.summary.state == RunState::Succeeded {
                0
            } else {
                1
            })
        })();
        let outcome = if matches!(result, Ok(0)) {
            JobOutcome::Succeeded
        } else {
            JobOutcome::Failed
        };
        let progress = context.events.last_progress();
        let _ = sender.send(result);
        context.events.finish(outcome, progress);
    })
    .map_err(|error| format!("The backup worker could not start: {error}"))?;
    while jobs.is_busy() {
        jobs.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    let result = receiver
        .recv()
        .map_err(|_| "The backup worker ended without a report.".to_string())?;
    if let Some(catalog) = catalog.take() {
        catalog.shutdown().join();
    }
    result
}
