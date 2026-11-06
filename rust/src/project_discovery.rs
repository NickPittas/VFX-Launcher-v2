//! Launcher directory matching, counting and file processing, ported from
//! FolderMatchWorker/FileSearchWorker in ui_slick/scanner.py. Native traversal
//! replaces shell find/dir while preserving configured target-folder matching.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::discovery::{self, Coverage, RootContinuity, ScanPurpose, ScanRequest, ScanSnapshot};
use crate::jobs::{Checkpoint, JobControl, JobEvents, JobPhase, JobProgress};
use crate::paths::{
    PathCategory, PathError, PathIssue, PathResult, fold_native_text, inspect_root,
};

pub fn walk_project(
    request: &ScanRequest,
    names: &[String],
    control: &JobControl,
    events: &JobEvents,
) -> PathResult<ScanSnapshot> {
    if request.purpose != ScanPurpose::Projects || !request.root.is_absolute() {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            Some(&request.root),
            "A project scan needs an absolute project root.",
        ));
    }
    let names: HashSet<_> = names
        .iter()
        .map(|name| fold_native_text(name.as_bytes()))
        .collect();
    if names.is_empty() {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            Some(&request.root),
            "Choose at least one project-folder name in Settings.",
        ));
    }
    let matches = |path: &Path| {
        path.file_name()
            .is_some_and(|name| names.contains(&fold_native_text(name.as_encoded_bytes())))
    };
    // A previously selected Projects folder already is a target: the original
    // quick-scan path scans that scope directly rather than widening to its parent.
    if matches(&request.root) {
        return discovery::walk(request, control, events);
    }
    let root_identity = match inspect_root(&request.root) {
        Ok(identity) => identity,
        Err(_) => return discovery::walk(request, control, events),
    };
    let mut result = ScanSnapshot {
        root: request.root.clone(),
        purpose: request.purpose,
        entries: Vec::new(),
        coverage: Coverage::Complete,
        continuity: RootContinuity::Uncertain,
        errors: Vec::new(),
        root_identity: Some(root_identity),
    };
    let mut stack = vec![request.root.clone()];
    let mut directories = Vec::new();
    let mut visited = HashSet::new();
    while let Some(directory) = stack.pop() {
        if control.checkpoint() == Checkpoint::Cancelled {
            result.coverage = Coverage::Cancelled;
            return Ok(result);
        }
        match inspect_root(&directory) {
            Ok(identity) if visited.insert(identity) => {}
            Ok(_) => {
                issue(
                    &mut result,
                    events,
                    &directory,
                    "Repeated directory identity was skipped.",
                );
                continue;
            }
            Err(error) => {
                issue(&mut result, events, &directory, error.message());
                continue;
            }
        }
        directories.push(directory.clone());
        events.progress(
            JobPhase::FindingFolders,
            JobProgress {
                completed: directories.len() as u64,
                current: Some(directory.clone()),
                ..JobProgress::default()
            },
        );
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                issue(
                    &mut result,
                    events,
                    &directory,
                    &format!("Could not list folders: {error}"),
                );
                continue;
            }
        };
        for entry in entries {
            if control.checkpoint() == Checkpoint::Cancelled {
                result.coverage = Coverage::Cancelled;
                return Ok(result);
            }
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    issue(
                        &mut result,
                        events,
                        &directory,
                        &format!("Could not read a folder entry: {error}"),
                    );
                    continue;
                }
            };
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => {
                    if !discovery::is_project_autosave(&entry.path()) {
                        stack.push(entry.path());
                    }
                }
                Ok(kind) if kind.is_symlink() => {
                    issue(
                        &mut result,
                        events,
                        &entry.path(),
                        "A link was skipped while finding project folders.",
                    );
                }
                Ok(_) => {}
                Err(error) => issue(
                    &mut result,
                    events,
                    &entry.path(),
                    &format!("Could not inspect an entry: {error}"),
                ),
            }
        }
    }
    directories.sort();
    let total_directories = directories.len() as u64;
    let mut targets: Vec<PathBuf> = Vec::new();
    for (index, directory) in directories.into_iter().enumerate() {
        if control.checkpoint() == Checkpoint::Cancelled {
            result.coverage = Coverage::Cancelled;
            return Ok(result);
        }
        events.progress(
            JobPhase::MatchingFolders,
            JobProgress {
                completed: index as u64 + 1,
                total: Some(total_directories),
                current: Some(directory.clone()),
                ..JobProgress::default()
            },
        );
        if matches(&directory) && !targets.iter().any(|parent| directory.starts_with(parent)) {
            targets.push(directory);
        }
    }
    let mut total_files = 0;
    for target in &targets {
        let mut target_request = request.clone();
        target_request.root = target.clone();
        let counted = discovery::walk_pass(&target_request, control, events, None, total_files)?;
        total_files += counted.entries.len() as u64;
        let coverage = counted.coverage;
        result.errors.extend(counted.errors);
        if coverage != Coverage::Complete {
            result.coverage = if coverage == Coverage::Cancelled {
                Coverage::Cancelled
            } else {
                Coverage::Partial
            };
            result.entries.extend(counted.entries);
            // Do not turn an incomplete counting pass into a complete processing pass.
            return Ok(result);
        }
    }
    let mut offset = 0;
    for target in targets {
        let mut target_request = request.clone();
        target_request.root = target;
        let processed =
            discovery::walk_pass(&target_request, control, events, Some(total_files), offset)?;
        offset += processed.entries.len() as u64;
        result.entries.extend(processed.entries);
        result.errors.extend(processed.errors);
        if processed.coverage == Coverage::Cancelled {
            result.coverage = Coverage::Cancelled;
            return Ok(result);
        }
        if processed.coverage != Coverage::Complete {
            result.coverage = Coverage::Partial;
        }
    }
    result.entries.sort_by(|a, b| a.path.cmp(&b.path));
    result.entries.dedup_by(|a, b| a.path == b.path);
    if inspect_root(&request.root).ok().and_then(|fresh| {
        result
            .root_identity
            .as_ref()
            .and_then(|before| before.compare_identity(&fresh))
    }) != Some(true)
    {
        issue(
            &mut result,
            events,
            &request.root,
            "The project root changed or became unavailable during scanning.",
        );
    }
    Ok(result)
}

fn issue(snapshot: &mut ScanSnapshot, events: &JobEvents, path: &Path, message: &str) {
    snapshot.coverage = Coverage::Partial;
    snapshot
        .errors
        .push(PathIssue::new(path, PathCategory::Unavailable, message));
    events.diagnostic(
        JobPhase::FindingFolders,
        format!("{}: {message}", path.display()),
    );
}
