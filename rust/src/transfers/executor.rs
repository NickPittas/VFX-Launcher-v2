//! X1b: the transfer executor.
//!
//! Linux is currently the only supported transfer backend; the other-platform
//! module keeps the public API and refuses execution.

#[cfg(target_os = "linux")]
#[path = "executor/linux.rs"]
mod platform;

#[cfg(not(target_os = "linux"))]
#[path = "executor/unsupported.rs"]
mod platform;

pub use platform::{ItemOutcome, RunReport};

use super::{TransferCategory, TransferError, ValidatedPlan};
use crate::jobs::{JobControl, JobEvents};
use crate::paths::{self, PathKey, PathRelation};
use crate::storage::CatalogClient;
use std::sync::{
    Mutex, OnceLock,
    atomic::{AtomicU64, Ordering},
};

struct Access {
    id: u64,
    destination: PathKey,
    source: Option<PathKey>,
}
static ACTIVE: OnceLock<Mutex<Vec<Access>>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Admission(u64);

impl Drop for Admission {
    fn drop(&mut self) {
        if let Some(active) = ACTIVE.get() {
            active
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|access| access.id != self.0);
        }
    }
}

fn resolved_key(path: &std::path::Path) -> Result<PathKey, TransferError> {
    let resolved = std::fs::canonicalize(path).map_err(|error| {
        TransferError::new(
            TransferCategory::Unavailable,
            Some(path),
            format!("The operation folder could not be resolved: {error}"),
        )
    })?;
    Ok(paths::path_key(&resolved, &resolved)?)
}

fn overlaps(left: &PathKey, right: &PathKey) -> bool {
    paths::relation(left, right) != PathRelation::Disjoint
}

/// Separate workspaces may read the same source, but intersecting writes and
/// writes into another active run's source are refused until that run finishes.
pub fn execute(
    catalog: &CatalogClient,
    validated: ValidatedPlan,
    control: &JobControl,
    events: &JobEvents,
) -> Result<RunReport, TransferError> {
    execute_with_workers(catalog, validated, control, events, 1)
}

pub fn execute_with_workers(
    catalog: &CatalogClient,
    validated: ValidatedPlan,
    control: &JobControl,
    events: &JobEvents,
    workers: usize,
) -> Result<RunReport, TransferError> {
    if !(1..=16).contains(&workers) {
        return Err(TransferError::new(
            TransferCategory::InvalidInput,
            None,
            "Choose between 1 and 16 copy workers.",
        ));
    }
    let destination = resolved_key(validated.destination_root())?;
    let source = validated
        .plan()
        .source_root_path()
        .map(resolved_key)
        .transpose()?;
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let mut active = ACTIVE
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if active.iter().any(|other| {
        overlaps(&destination, &other.destination)
            || other
                .source
                .as_ref()
                .is_some_and(|root| overlaps(&destination, root))
            || source
                .as_ref()
                .is_some_and(|root| overlaps(root, &other.destination))
    }) {
        return Err(TransferError::new(
            TransferCategory::Refused,
            Some(validated.destination_root()),
            "Another file operation is using an overlapping source or destination. Wait for it to finish, then validate again.",
        ));
    }
    active.push(Access {
        id,
        destination,
        source,
    });
    let admission = Admission(id);
    drop(active);
    let result = platform::execute_with_workers(catalog, validated, control, events, workers);
    drop(admission);
    result
}
