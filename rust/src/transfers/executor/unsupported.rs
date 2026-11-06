//! Transfer executor boundary for platforms without a native backend.
//!
//! The public shape matches the Linux executor so callers compile everywhere,
//! but execution is refused before any journal or filesystem work happens.

use super::super::{ItemState, RunSummary, TransferCategory, TransferError, ValidatedPlan};
use crate::jobs::{JobControl, JobEvents};
use crate::storage::CatalogClient;
use std::path::PathBuf;

/// One item's durable terminal outcome.
#[derive(Clone, Debug)]
pub struct ItemOutcome {
    pub ordinal: i64,
    pub state: ItemState,
    pub committed_bytes: u64,
    pub message: Option<String>,
    pub recovery: Vec<PathBuf>,
}

/// Result of one executed run.
#[derive(Clone, Debug)]
pub struct RunReport {
    pub run_id: i64,
    pub summary: RunSummary,
    pub items: Vec<ItemOutcome>,
}

/// Refuses execution: no platform-safe staging/commit primitives exist here yet.
pub fn execute_with_workers(
    _catalog: &CatalogClient,
    _validated: ValidatedPlan,
    _control: &JobControl,
    _events: &JobEvents,
    _workers: usize,
) -> Result<RunReport, TransferError> {
    Err(TransferError::new(
        TransferCategory::Unsupported,
        None,
        "the transfer executor is implemented for Linux only",
    ))
}
