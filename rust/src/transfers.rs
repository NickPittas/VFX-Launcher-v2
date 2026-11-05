//! X1a: concrete transfer plans, read-only snapshot validation and the durable
//! operation journal.
//!
//! Nothing here mutates the filesystem. Validation stats sources and
//! destinations once, records what it saw as reviewable evidence, and X1b must
//! revalidate every reviewed item immediately before its no-clobber write.
//! Plan and item envelopes are versioned JSON stored in the existing
//! `operation_runs.detail` and `operation_items.message` columns; paths inside
//! them are tagged native bytes (POSIX raw, Windows UTF-16LE) so non-UTF-8
//! spellings survive the round trip.
use crate::{
    discovery::{Coverage, FileEntry, RootContinuity, ScanPurpose, ScanSnapshot},
    paths::{self, PathCategory, PathError, PathIssue, PathKey, PathRelation},
    storage::{
        CURRENT_PLATFORM, Catalog, PathColumns, Pending, StorageCategory, StorageError,
        StorageResult, utc_timestamp_now,
    },
};
use rusqlite::{OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fmt, fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const RUN_ENVELOPE_VERSION: u32 = 1;
const ITEM_ENVELOPE_VERSION: u32 = 1;
const MAX_NUMBERED_ATTEMPTS: u32 = 9_999;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Stable transfer failure categories, distinct from catalog storage errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferCategory {
    /// The caller supplied a plan or path this module cannot use.
    InvalidInput,
    /// The request was understood but refused because it cannot be proven safe.
    Refused,
    /// A path could not be inspected; absence is never inferred from this.
    Unavailable,
    /// Inspection was refused by permissions.
    Permission,
    /// The source scan is incomplete or its evidence disagrees with the plan.
    StaleEvidence,
    /// The path is a link, special file or foreign-platform spelling.
    Unsupported,
}

impl TransferCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid transfer request",
            Self::Refused => "transfer refused",
            Self::Unavailable => "path unavailable",
            Self::Permission => "permission denied",
            Self::StaleEvidence => "stale scan evidence",
            Self::Unsupported => "unsupported transfer input",
        }
    }
}

/// Module-owned transfer error: category, affected path and a readable cause.
#[derive(Clone, Debug)]
pub struct TransferError {
    category: TransferCategory,
    path: Option<PathBuf>,
    message: String,
}

impl TransferError {
    pub fn new(
        category: TransferCategory,
        path: Option<&Path>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            category,
            path: path.map(Path::to_path_buf),
            message: message.into(),
        }
    }

    pub fn category(&self) -> TransferCategory {
        self.category
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(path) => write!(
                f,
                "{} ({}): {}",
                self.message,
                self.category.label(),
                path.display()
            ),
            None => write!(f, "{} ({})", self.message, self.category.label()),
        }
    }
}

impl std::error::Error for TransferError {}

impl From<PathError> for TransferError {
    fn from(error: PathError) -> Self {
        Self {
            category: match error.category() {
                PathCategory::InvalidInput => TransferCategory::InvalidInput,
                PathCategory::ForeignPlatform | PathCategory::Unsupported => {
                    TransferCategory::Unsupported
                }
                PathCategory::MissingRoot | PathCategory::Escape => TransferCategory::Refused,
                PathCategory::Unavailable => TransferCategory::Unavailable,
                PathCategory::Permission => TransferCategory::Permission,
            },
            path: error.path().map(Path::to_path_buf),
            message: error.message().to_string(),
        }
    }
}

fn invalid(message: impl Into<String>) -> StorageError {
    StorageError::new(StorageCategory::InvalidInput, None, message)
}

fn db_error(message: impl Into<String>, error: impl fmt::Display) -> StorageError {
    StorageError::new(
        StorageCategory::Database,
        None,
        format!("{}: {error}", message.into()),
    )
}

// ---------------------------------------------------------------------------
// Lossless native path envelope
// ---------------------------------------------------------------------------

/// A native path whose native spelling is preserved losslessly through serde.
///
/// Encoding uses [`PathColumns::native`] (POSIX raw bytes, Windows UTF-16LE) and
/// decoding uses [`PathColumns::decode`], so a spelling that belongs to another
/// platform is refused instead of being guessed. `PathBuf` is never serialized
/// directly: serde's implementation fails on non-UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPath {
    native: PathBuf,
    spelling: Vec<u8>,
    display: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct EncodedPath {
    platform: String,
    spelling: Vec<u8>,
    display: Option<String>,
}

impl StoredPath {
    /// Encodes `path` with the explicit `base` used for relative spellings.
    pub fn from_native(path: &Path, base: &Path) -> Result<Self, TransferError> {
        let key = paths::path_key(path, base)?;
        Self::from_columns(PathColumns::native(path, &key))
    }

    /// Encodes an absolute path; recovery paths and roots must be absolute.
    pub fn absolute(path: &Path) -> Result<Self, TransferError> {
        if !path.is_absolute() {
            return Err(TransferError::new(
                TransferCategory::InvalidInput,
                Some(path),
                "path must be absolute to be recorded",
            ));
        }
        Self::from_native(path, path)
    }

    fn from_columns(columns: PathColumns) -> Result<Self, TransferError> {
        let native = columns.decode().ok_or_else(|| {
            TransferError::new(
                TransferCategory::Unsupported,
                None,
                "the stored path spelling cannot be decoded on this platform",
            )
        })?;
        Ok(Self {
            native,
            spelling: columns.spelling,
            display: columns.display,
        })
    }

    pub fn path(&self) -> &Path {
        &self.native
    }

    pub fn display(&self) -> Option<&str> {
        self.display.as_deref()
    }

    /// Native bytes as stored in the catalog spelling columns.
    pub fn spelling(&self) -> &[u8] {
        &self.spelling
    }
}

impl Serialize for StoredPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        EncodedPath {
            platform: CURRENT_PLATFORM.to_string(),
            spelling: self.spelling.clone(),
            display: self.display.clone(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for StoredPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let encoded = EncodedPath::deserialize(deserializer)?;
        if encoded.platform != CURRENT_PLATFORM {
            return Err(serde::de::Error::custom(format!(
                "stored path belongs to the {} platform; this build cannot resolve it",
                encoded.platform
            )));
        }
        let columns = PathColumns {
            storage: None,
            platform: CURRENT_PLATFORM,
            spelling: encoded.spelling,
            display: encoded.display,
        };
        Self::from_columns(columns).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Observable metadata evidence
// ---------------------------------------------------------------------------

/// Observable identity of a file or directory from one stat snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeIdentity {
    unix: Option<(u64, u64)>,
    windows: Option<(u64, u64)>,
}

impl NativeIdentity {
    /// `Some(agreement)` when an evidence kind is present on both sides.
    pub fn same(&self, other: &Self) -> Option<bool> {
        let mut compared = false;
        let mut agree = true;
        if let (Some(a), Some(b)) = (self.unix, other.unix) {
            compared = true;
            agree &= a == b;
        }
        if let (Some(a), Some(b)) = (self.windows, other.windows) {
            compared = true;
            agree &= a == b;
        }
        compared.then_some(agree)
    }
}

#[cfg(unix)]
fn identity_of(meta: &fs::Metadata) -> Option<NativeIdentity> {
    use std::os::unix::fs::MetadataExt;
    Some(NativeIdentity {
        unix: Some((meta.dev(), meta.ino())),
        windows: None,
    })
}

#[cfg(windows)]
fn identity_of(meta: &fs::Metadata) -> Option<NativeIdentity> {
    use std::os::windows::fs::MetadataExt;
    meta.volume_serial_number()
        .zip(meta.file_index())
        .map(|(volume, index)| NativeIdentity {
            unix: None,
            windows: Some((u64::from(volume), index)),
        })
}

#[cfg(not(any(unix, windows)))]
fn identity_of(_meta: &fs::Metadata) -> Option<NativeIdentity> {
    None
}

/// Expected source metadata recorded at review time. `modified_ns` is `None`
/// only for scan-sourced expectations where discovery could not represent the
/// time; validation re-stats the file and fails closed when it cannot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceStamp {
    pub size: u64,
    pub modified_ns: Option<u64>,
    pub identity: Option<NativeIdentity>,
}

/// What the destination looked like at review time. A boolean "exists" is not
/// enough to revalidate or to detect a replaced file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DestinationExpectation {
    /// Must still be absent when execution writes it.
    Absent,
    /// Must be an existing directory (add-to-existing).
    Directory,
    /// Reviewed existing regular file; Replace may overwrite it.
    RegularFile { stamp: SourceStamp },
    /// Expected skip: the reviewed destination must still match the stamp.
    SkipExisting { stamp: SourceStamp },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueSummary {
    pub path: Option<StoredPath>,
    pub path_display: String,
    pub category: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceCoverage {
    Complete,
    Partial,
    Cancelled,
    RootUnavailable,
}

impl EvidenceCoverage {
    fn of(coverage: Coverage) -> Self {
        match coverage {
            Coverage::Complete => Self::Complete,
            Coverage::Partial => Self::Partial,
            Coverage::Cancelled => Self::Cancelled,
            Coverage::RootUnavailable => Self::RootUnavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceContinuity {
    Proven,
    Uncertain,
}

/// Source root evidence preserved with the plan: what was scanned, when, with
/// which coverage and observable root identity, plus every discovery issue.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceEvidence {
    pub observed_at: String,
    pub coverage: EvidenceCoverage,
    pub continuity: EvidenceContinuity,
    pub root_identity: Option<NativeIdentity>,
    pub root_modified_ns: Option<u64>,
    pub issues: Vec<IssueSummary>,
}

// ---------------------------------------------------------------------------
// Plan
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    Skip,
    Replace,
    KeepBoth,
    PreserveExisting,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileAction {
    Copy,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanOrigin {
    Structure,
    Incoming,
    ExtensionCopy,
    Backup,
    Ai,
}

impl PlanOrigin {
    fn kind(self) -> &'static str {
        match self {
            Self::Structure => "structure",
            Self::Incoming => "incoming",
            Self::ExtensionCopy => "extension_copy",
            Self::Backup => "backup",
            Self::Ai => "ai",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    Directory,
    File,
    CreateFile,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlannedAction {
    CreateDirectory {
        relative: StoredPath,
    },
    CreateFile {
        relative: StoredPath,
        contents: Vec<u8>,
    },
    File {
        source: StoredPath,
        relative: StoredPath,
        action: FileAction,
        /// Stamp observed by the scan that produced this plan, when it had one.
        expected: Option<SourceStamp>,
    },
}

impl PlannedAction {
    pub fn kind(&self) -> ActionKind {
        match self {
            Self::CreateDirectory { .. } => ActionKind::Directory,
            Self::CreateFile { .. } => ActionKind::CreateFile,
            Self::File { .. } => ActionKind::File,
        }
    }

    pub fn relative(&self) -> &StoredPath {
        match self {
            Self::CreateDirectory { relative }
            | Self::CreateFile { relative, .. }
            | Self::File { relative, .. } => relative,
        }
    }

    pub fn source(&self) -> Option<&StoredPath> {
        match self {
            Self::File { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// The concrete plan a user reviews. Every action is relative to
/// `destination_root`; paths are tagged native spellings, never display text.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationPlan {
    pub id: String,
    pub revision: u64,
    pub origin: PlanOrigin,
    pub destination_root: StoredPath,
    /// Add-to-existing plans accept existing directories; new roots are exclusive.
    pub add_to_existing: bool,
    pub policy: ConflictPolicy,
    pub source_root: Option<StoredPath>,
    pub source_evidence: Option<SourceEvidence>,
    /// Planning-time warnings (for example uncertain mount continuity) that are
    /// carried into review and the durable intent envelope.
    #[serde(default)]
    pub warnings: Vec<String>,
    pub actions: Vec<PlannedAction>,
}

impl OperationPlan {
    pub fn source_root_path(&self) -> Option<&Path> {
        self.source_root.as_ref().map(StoredPath::path)
    }

    pub fn destination_root_path(&self) -> &Path {
        self.destination_root.path()
    }
}

// ---------------------------------------------------------------------------
// Extension-copy planning
// ---------------------------------------------------------------------------

/// Plans an extension copy from an explicit reviewed scan.
///
/// `Complete` coverage is required: `Partial`, `Cancelled` and
/// `RootUnavailable` scans refuse the whole plan and name the reason instead of
/// silently copying a subset. Uncertain mount continuity is allowed because the
/// copy list is explicit and reviewed; it is recorded as a visible warning and
/// keeps catalog pruning suppressed. No subset API exists yet, so a partial
/// scan must be rescanned rather than trimmed by index.
pub fn plan_extension_copy(
    id: impl Into<String>,
    revision: u64,
    snapshot: &ScanSnapshot,
    destination_root: &Path,
    policy: ConflictPolicy,
) -> Result<OperationPlan, TransferError> {
    if snapshot.purpose != ScanPurpose::ExtensionCopy {
        return Err(TransferError::new(
            TransferCategory::InvalidInput,
            Some(&snapshot.root),
            "only an ExtensionCopy scan can produce an extension-copy plan",
        ));
    }
    if !snapshot.root.is_absolute() || !destination_root.is_absolute() {
        return Err(TransferError::new(
            TransferCategory::InvalidInput,
            Some(destination_root),
            "source and destination roots must be absolute",
        ));
    }
    match snapshot.coverage {
        Coverage::Complete => {}
        Coverage::Partial => {
            return Err(TransferError::new(
                TransferCategory::StaleEvidence,
                Some(&snapshot.root),
                format!(
                    "the scan is partial: {} path issue(s) left part of the source unread; rescan before copying",
                    snapshot.errors.len()
                ),
            ));
        }
        Coverage::Cancelled => {
            return Err(TransferError::new(
                TransferCategory::StaleEvidence,
                Some(&snapshot.root),
                "the scan was cancelled before enumeration finished; rescan before copying",
            ));
        }
        Coverage::RootUnavailable => {
            return Err(TransferError::new(
                TransferCategory::Unavailable,
                Some(&snapshot.root),
                "the source root was unavailable during the scan",
            ));
        }
    }

    let mut warnings = Vec::new();
    if snapshot.continuity == RootContinuity::Uncertain {
        warnings.push(
            "mount continuity could not be proven; the explicit reviewed file list may be copied, but catalog pruning stays suppressed"
                .to_string(),
        );
    }

    paths::inspect_root(destination_root)?;
    let source_key = paths::path_key(&snapshot.root, &snapshot.root)?;
    let destination_key = paths::path_key(destination_root, destination_root)?;
    match paths::relation(&source_key, &destination_key) {
        PathRelation::Disjoint => {}
        PathRelation::Same => {
            return Err(TransferError::new(
                TransferCategory::Refused,
                Some(destination_root),
                "source and destination roots are the same directory",
            ));
        }
        _ => {
            return Err(TransferError::new(
                TransferCategory::Refused,
                Some(destination_root),
                "source and destination roots are nested; a recursive copy would chase its own output",
            ));
        }
    }
    if paths::evidence_same_file(&snapshot.root, destination_root) == Some(true) {
        return Err(TransferError::new(
            TransferCategory::Refused,
            Some(destination_root),
            "source and destination resolve to the same directory through a link or mount alias",
        ));
    }

    let scanned_identity = paths::inspect_root(&snapshot.root)?;
    match snapshot.root_identity {
        Some(recorded) => match recorded.compare_identity(&scanned_identity) {
            Some(false) => {
                return Err(TransferError::new(
                    TransferCategory::StaleEvidence,
                    Some(&snapshot.root),
                    "the source root identity changed after the scan; the entry list is no longer trusted",
                ));
            }
            Some(true) => {}
            None => warnings.push(
                "the scan and the current root share no comparable identity evidence; continuity stays unproven"
                    .to_string(),
            ),
        },
        None => warnings.push(
            "the scan recorded no root identity evidence; continuity stays unproven".to_string(),
        ),
    }

    let root_metadata = fs::metadata(&snapshot.root).map_err(|error| {
        TransferError::new(
            TransferCategory::Unavailable,
            Some(&snapshot.root),
            format!("source root metadata could not be read: {error}"),
        )
    })?;
    let root_modified_ns = match root_metadata.modified() {
        Ok(modified) => match nanos_from_time(modified) {
            Ok(nanos) => Some(nanos),
            Err(message) => {
                warnings.push(format!("source root {message}"));
                None
            }
        },
        Err(error) => {
            warnings.push(format!(
                "source root modification time is unavailable ({error}); continuity stays unproven"
            ));
            None
        }
    };

    let normalized_root = paths::normalize_lexical(&snapshot.root, &snapshot.root)?;
    let mut seen = HashSet::new();
    let mut missing_mtime = 0usize;
    let mut actions = Vec::with_capacity(snapshot.entries.len());
    for entry in &snapshot.entries {
        let relative = entry.path.strip_prefix(&normalized_root).map_err(|_| {
            TransferError::new(
                TransferCategory::InvalidInput,
                Some(&entry.path),
                "scan entry is outside its own root",
            )
        })?;
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(TransferError::new(
                TransferCategory::InvalidInput,
                Some(&entry.path),
                "scan entry does not have a plain relative spelling",
            ));
        }
        let key = paths::path_key(&entry.path, &snapshot.root)?;
        if !seen.insert(key) {
            warnings.push(format!(
                "duplicate scan entry ignored: {}",
                entry.path.display()
            ));
            continue;
        }
        let expected = expected_stamp(entry)?;
        if expected.modified_ns.is_none() {
            missing_mtime += 1;
        }
        actions.push(PlannedAction::File {
            source: StoredPath::from_native(&entry.path, &snapshot.root)?,
            relative: StoredPath::from_native(relative, &snapshot.root)?,
            action: FileAction::Copy,
            expected: Some(expected),
        });
    }
    if missing_mtime > 0 {
        warnings.push(format!(
            "{missing_mtime} scan entr(ies) carried no modification time; validation re-stats them and refuses any it cannot record"
        ));
    }

    let evidence = SourceEvidence {
        observed_at: utc_timestamp_now(),
        coverage: EvidenceCoverage::of(snapshot.coverage),
        continuity: match snapshot.continuity {
            RootContinuity::Proven => EvidenceContinuity::Proven,
            RootContinuity::Uncertain => EvidenceContinuity::Uncertain,
        },
        root_identity: identity_of(&root_metadata),
        root_modified_ns,
        issues: snapshot.errors.iter().map(issue_summary).collect(),
    };

    Ok(OperationPlan {
        id: id.into(),
        revision,
        origin: PlanOrigin::ExtensionCopy,
        destination_root: StoredPath::from_native(destination_root, destination_root)?,
        add_to_existing: false,
        policy,
        source_root: Some(StoredPath::from_native(&snapshot.root, &snapshot.root)?),
        source_evidence: Some(evidence),
        warnings,
        actions,
    })
}

fn expected_stamp(entry: &FileEntry) -> Result<SourceStamp, TransferError> {
    let modified_ns = match entry.stamp.modified {
        Some(modified) => Some(nanos_from_time(modified).map_err(|message| {
            TransferError::new(
                TransferCategory::Unsupported,
                Some(&entry.path),
                format!("scan stamp {message}"),
            )
        })?),
        None => None,
    };
    Ok(SourceStamp {
        size: entry.stamp.size,
        modified_ns,
        // Discovery's identity value has no readable fields; validation records
        // the fresh identity it stats instead of inventing one here.
        identity: None,
    })
}

fn issue_summary(issue: &PathIssue) -> IssueSummary {
    IssueSummary {
        path: StoredPath::from_native(&issue.path, &issue.path).ok(),
        path_display: issue.path.display().to_string(),
        category: issue.category.label().to_string(),
        message: issue.message.clone(),
    }
}

fn nanos_from_time(time: SystemTime) -> Result<u64, String> {
    let elapsed = time
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "modification time is before the UNIX epoch".to_string())?;
    u64::try_from(elapsed.as_nanos())
        .map_err(|_| "modification time is outside the recordable range".to_string())
}

fn stamp_of(path: &Path) -> Result<SourceStamp, TransferError> {
    let meta = fs::metadata(path).map_err(|error| {
        TransferError::new(
            TransferCategory::Unavailable,
            Some(path),
            format!("source metadata could not be read: {error}"),
        )
    })?;
    stamp_from_metadata(&meta, path)
}

fn stamp_from_metadata(meta: &fs::Metadata, path: &Path) -> Result<SourceStamp, TransferError> {
    let modified = meta.modified().map_err(|error| {
        TransferError::new(
            TransferCategory::Unsupported,
            Some(path),
            format!("modification time is unavailable: {error}"),
        )
    })?;
    let modified_ns = nanos_from_time(modified).map_err(|message| {
        TransferError::new(TransferCategory::Unsupported, Some(path), message)
    })?;
    Ok(SourceStamp {
        size: meta.len(),
        modified_ns: Some(modified_ns),
        identity: identity_of(meta),
    })
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewState {
    Ready,
    Skipped,
    Blocked,
}

/// Execution semantics of one reviewed item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemEffect {
    Copy,
    Move,
    CreateFile,
    CreateDirectory,
    /// Reviewed no-op under the Skip policy.
    Skip,
}

/// One reviewed action with its concrete destination and expected state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemValidation {
    pub index: usize,
    pub state: ReviewState,
    pub kind: ActionKind,
    pub effect: ItemEffect,
    pub source: Option<StoredPath>,
    pub destination: Option<StoredPath>,
    /// PreserveExisting proposal: the numbered old-target path the destination
    /// is moved aside to. A proposal only; X1b reserves it without clobbering.
    pub displaced: Option<StoredPath>,
    pub source_stamp: Option<SourceStamp>,
    pub expectation: Option<DestinationExpectation>,
    pub message: Option<String>,
    pub bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationCounts {
    pub total: usize,
    pub ready: usize,
    pub skipped: usize,
    pub blocked: usize,
}

/// Read-only validation outcome. `ready` exists only when nothing is blocked.
pub struct ValidationReport {
    pub ready: Option<ValidatedPlan>,
    pub items: Vec<ItemValidation>,
    pub counts: ValidationCounts,
    /// Free-space estimation is not implemented; this is always reported.
    pub space_unknown: bool,
    pub warnings: Vec<String>,
    /// Plan-level refusal cause; every action is then reported blocked.
    pub refusal: Option<String>,
}

/// Validation-bound plan. Only [`validate_plan`] constructs it, and its fields
/// are private so a caller cannot inject unvalidated destinations.
pub struct ValidatedPlan {
    plan: OperationPlan,
    items: Vec<ItemValidation>,
    warnings: Vec<String>,
}

impl ValidatedPlan {
    pub fn plan(&self) -> &OperationPlan {
        &self.plan
    }

    /// Reviewed items with concrete destinations, policies and stamps.
    pub fn items(&self) -> &[ItemValidation] {
        &self.items
    }

    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    pub fn plan_id(&self) -> &str {
        &self.plan.id
    }

    pub fn revision(&self) -> u64 {
        self.plan.revision
    }

    pub fn policy(&self) -> ConflictPolicy {
        self.plan.policy
    }

    pub fn destination_root(&self) -> &Path {
        self.plan.destination_root_path()
    }
}

struct Resolution {
    destination: PathBuf,
    key: PathKey,
    expectation: DestinationExpectation,
    effect: ItemEffect,
    displaced: Option<PathBuf>,
}

struct Validator<'a> {
    plan: &'a OperationPlan,
    destination_root: &'a Path,
    source_root_key: Option<PathKey>,
    planned_directories: HashMap<PathKey, usize>,
    /// Sources of items that will actually execute.
    executing_sources: HashSet<PathKey>,
    destinations: HashSet<PathKey>,
    executing_destinations: Vec<(usize, PathKey)>,
    warnings: Vec<String>,
}

/// Validates one plan against the filesystem as it is right now.
///
/// Extension-copy plans must carry `Complete` source evidence; `Partial`,
/// `Cancelled` and `RootUnavailable` coverage are refused. Uncertain continuity
/// is surfaced as a warning rather than refused, because the entry list is
/// explicit. Every conflict policy resolves to concrete proposed destinations:
/// Replace reuses the reviewed path, KeepBoth numbers the incoming name,
/// PreserveExisting numbers the displaced old-target path, Skip records the
/// reviewed destination and writes nothing. Nothing is reserved in the
/// filesystem; X1b must revalidate and use no-clobber writes.
pub fn validate_plan(plan: &OperationPlan) -> ValidationReport {
    let mut warnings = plan.warnings.clone();
    let destination_root = plan.destination_root_path();
    if !destination_root.is_absolute() {
        return refused_report(plan, "destination root must be an absolute path");
    }
    if let Err(error) = paths::inspect_root(destination_root) {
        return refused_report(plan, format!("destination root cannot be used: {error}"));
    }

    let source_root_key = match plan.source_root.as_ref() {
        Some(source_root) => {
            let root = source_root.path();
            if !root.is_absolute() {
                return refused_report(plan, "source root must be an absolute path");
            }
            if let Err(error) = paths::inspect_root(root) {
                return refused_report(plan, format!("source root cannot be inspected: {error}"));
            }
            let key = match paths::path_key(root, root) {
                Ok(key) => key,
                Err(error) => {
                    return refused_report(plan, format!("source root is unusable: {error}"));
                }
            };
            let destination_key = match paths::path_key(destination_root, destination_root) {
                Ok(key) => key,
                Err(error) => {
                    return refused_report(plan, format!("destination root is unusable: {error}"));
                }
            };
            match paths::relation(&key, &destination_key) {
                PathRelation::Disjoint => {}
                PathRelation::Same => {
                    return refused_report(plan, "source and destination roots are the same");
                }
                _ => {
                    return refused_report(
                        plan,
                        "source and destination roots are nested; the copy could chase its own output",
                    );
                }
            }
            if paths::evidence_same_file(root, destination_root) == Some(true) {
                return refused_report(
                    plan,
                    "source and destination roots are the same directory through a link or mount alias",
                );
            }
            Some(key)
        }
        None => None,
    };

    if plan.origin == PlanOrigin::ExtensionCopy {
        if plan.source_root.is_none() || plan.source_evidence.is_none() {
            return refused_report(
                plan,
                "an extension-copy plan must record its source root and scan evidence",
            );
        }
    }

    if let Some(evidence) = &plan.source_evidence {
        if evidence.coverage != EvidenceCoverage::Complete {
            return refused_report(
                plan,
                format!(
                    "source evidence covers a {:?} scan; an incomplete scan is never copied silently",
                    evidence.coverage
                ),
            );
        }
        if evidence.continuity == EvidenceContinuity::Uncertain {
            warnings.push(
                "mount continuity was not proven at planning time; verification stays snapshot-only"
                    .to_string(),
            );
        }
        if let (Some(recorded), Some(source_root)) = (evidence.root_identity, &plan.source_root) {
            let fresh = fs::metadata(source_root.path())
                .ok()
                .and_then(|meta| identity_of(&meta));
            match fresh {
                Some(fresh) => match recorded.same(&fresh) {
                    Some(true) => {}
                    Some(false) => {
                        return refused_report(
                            plan,
                            "the source root identity changed after planning",
                        );
                    }
                    None => warnings.push(
                        "no shared root identity evidence remains; continuity unproven".into(),
                    ),
                },
                None => {
                    return refused_report(
                        plan,
                        "the source root identity can no longer be inspected",
                    );
                }
            }
        }
    }

    let mut validator = Validator {
        plan,
        destination_root,
        source_root_key,
        planned_directories: HashMap::new(),
        executing_sources: HashSet::new(),
        destinations: HashSet::new(),
        executing_destinations: Vec::new(),
        warnings,
    };
    validator.index_directories();
    let mut items = Vec::with_capacity(plan.actions.len());
    for index in 0..plan.actions.len() {
        items.push(validator.resolve_action(index));
    }
    validator.block_destination_source_chains(&mut items);

    let mut counts = ValidationCounts {
        total: items.len(),
        ..ValidationCounts::default()
    };
    for item in &items {
        match item.state {
            ReviewState::Ready => counts.ready += 1,
            ReviewState::Skipped => counts.skipped += 1,
            ReviewState::Blocked => counts.blocked += 1,
        }
    }
    let warnings = validator.warnings;
    let ready = if counts.blocked == 0 {
        Some(ValidatedPlan {
            plan: plan.clone(),
            items: items.clone(),
            warnings: warnings.clone(),
        })
    } else {
        None
    };
    ValidationReport {
        ready,
        items,
        counts,
        space_unknown: true,
        warnings,
        refusal: None,
    }
}

fn refused_report(plan: &OperationPlan, reason: impl Into<String>) -> ValidationReport {
    let reason = reason.into();
    let items: Vec<ItemValidation> = plan
        .actions
        .iter()
        .enumerate()
        .map(|(index, action)| ItemValidation {
            index,
            state: ReviewState::Blocked,
            kind: action.kind(),
            effect: ItemEffect::Skip,
            source: action.source().cloned(),
            destination: None,
            displaced: None,
            source_stamp: None,
            expectation: None,
            message: Some(reason.clone()),
            bytes: None,
        })
        .collect();
    ValidationReport {
        ready: None,
        counts: ValidationCounts {
            total: items.len(),
            ready: 0,
            skipped: 0,
            blocked: items.len(),
        },
        items,
        space_unknown: true,
        warnings: plan.warnings.clone(),
        refusal: Some(reason),
    }
}

impl<'a> Validator<'a> {
    fn index_directories(&mut self) {
        for (index, action) in self.plan.actions.iter().enumerate() {
            if !matches!(action, PlannedAction::CreateDirectory { .. }) {
                continue;
            }
            if let Ok(destination) =
                paths::validate_destination(self.destination_root, action.relative().path())
            {
                if let Ok(key) = paths::path_key(&destination, self.destination_root) {
                    self.planned_directories.entry(key).or_insert(index);
                }
            }
        }
    }

    fn resolve_action(&mut self, index: usize) -> ItemValidation {
        let plan: &'a OperationPlan = self.plan;
        let action = &plan.actions[index];
        let relative = action.relative().path().to_path_buf();
        let destination = match paths::validate_destination(self.destination_root, &relative) {
            Ok(destination) => destination,
            Err(error) => {
                return self.blocked(index, action, format!("destination refused: {error}"));
            }
        };
        let destination_key = match paths::path_key(&destination, self.destination_root) {
            Ok(key) => key,
            Err(error) => {
                return self.blocked(index, action, format!("destination unusable: {error}"));
            }
        };
        if let Err(reason) = self.check_parent_chain(&relative, index) {
            return self.blocked(index, action, reason);
        }
        if plan.origin == PlanOrigin::ExtensionCopy
            && !matches!(
                action,
                PlannedAction::File {
                    action: FileAction::Copy,
                    ..
                }
            )
        {
            return self.blocked(
                index,
                action,
                "extension-copy plans may only copy files; other actions need their own plan origin",
            );
        }
        match action {
            PlannedAction::CreateDirectory { .. } => {
                self.resolve_directory(index, action, destination, destination_key)
            }
            PlannedAction::CreateFile { contents, .. } => self.resolve_create_file(
                index,
                action,
                destination,
                destination_key,
                contents.len() as u64,
            ),
            PlannedAction::File {
                source,
                action: file_action,
                expected,
                ..
            } => self.resolve_file(
                index,
                action,
                source,
                *file_action,
                expected.as_ref(),
                destination,
                destination_key,
            ),
        }
    }

    fn resolve_directory(
        &mut self,
        index: usize,
        action: &PlannedAction,
        destination: PathBuf,
        destination_key: PathKey,
    ) -> ItemValidation {
        let existing = match inspect_destination(&destination) {
            Ok(existing) => existing,
            Err(reason) => return self.blocked(index, action, reason),
        };
        match existing {
            None => self.ready(
                index,
                action,
                Resolution {
                    destination,
                    key: destination_key,
                    expectation: DestinationExpectation::Absent,
                    effect: ItemEffect::CreateDirectory,
                    displaced: None,
                },
                None,
                Some(0),
                None,
            ),
            Some(meta) if meta.file_type().is_symlink() => self.blocked(
                index,
                action,
                "destination is a symbolic link; creating through links is refused",
            ),
            Some(meta) if meta.is_dir() => {
                if self.plan.add_to_existing {
                    self.ready(
                        index,
                        action,
                        Resolution {
                            destination,
                            key: destination_key,
                            expectation: DestinationExpectation::Directory,
                            effect: ItemEffect::CreateDirectory,
                            displaced: None,
                        },
                        None,
                        Some(0),
                        Some("existing directory accepted for add-to-existing".to_string()),
                    )
                } else {
                    self.blocked(
                        index,
                        action,
                        "destination directory already exists; a new root is created exclusively",
                    )
                }
            }
            Some(_) => self.blocked(index, action, "destination exists and is not a directory"),
        }
    }

    fn resolve_create_file(
        &mut self,
        index: usize,
        action: &PlannedAction,
        destination: PathBuf,
        destination_key: PathKey,
        bytes: u64,
    ) -> ItemValidation {
        self.resolve_write(
            index,
            action,
            None,
            None,
            destination,
            destination_key,
            ItemEffect::CreateFile,
            bytes,
        )
    }

    // The transfer path needs the destination kind and policy outcome in one
    // place so File and CreateFile share identical no-clobber semantics.
    fn resolve_file(
        &mut self,
        index: usize,
        action: &PlannedAction,
        source: &StoredPath,
        file_action: FileAction,
        expected: Option<&SourceStamp>,
        destination: PathBuf,
        destination_key: PathKey,
    ) -> ItemValidation {
        let source_path = source.path();
        if !source_path.is_absolute() {
            return self.blocked(index, action, "source path must be absolute");
        }
        let link_meta = match fs::symlink_metadata(source_path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                return self.blocked(index, action, "source no longer exists");
            }
            Err(error) => {
                return self.blocked(
                    index,
                    action,
                    format!("source cannot be inspected: {error}"),
                );
            }
        };
        if link_meta.file_type().is_symlink() {
            return self.blocked(
                index,
                action,
                "source is a symbolic link; transfers need an explicit link policy and this plan has none",
            );
        }
        if !link_meta.is_file() {
            return self.blocked(index, action, "source is not a regular file");
        }
        let stamp = match stamp_of(source_path) {
            Ok(stamp) => stamp,
            Err(error) => return self.blocked(index, action, error.message().to_string()),
        };
        if let Some(expected) = expected {
            if expected.size != stamp.size {
                return self.blocked(
                    index,
                    action,
                    "source size changed since the scan that produced this plan",
                );
            }
            if let (Some(before), Some(now)) = (expected.modified_ns, stamp.modified_ns) {
                if before != now {
                    return self.blocked(
                        index,
                        action,
                        "source modification time changed since the scan that produced this plan",
                    );
                }
            }
        }
        let source_key = match paths::path_key(source_path, source_path) {
            Ok(key) => key,
            Err(error) => return self.blocked(index, action, format!("source unusable: {error}")),
        };
        if let Some(root_key) = &self.source_root_key {
            if paths::relation(&source_key, root_key) != PathRelation::FirstInsideSecond {
                return self.blocked(
                    index,
                    action,
                    "source is not inside the declared source root",
                );
            }
        }
        if paths::relation(&source_key, &destination_key) != PathRelation::Disjoint {
            return self.blocked(
                index,
                action,
                "source and destination are the same path or one contains the other",
            );
        }
        if paths::evidence_same_file(source_path, &destination) == Some(true) {
            return self.blocked(
                index,
                action,
                "source and destination are the same file through a hard link or alias",
            );
        }
        let effect = match file_action {
            FileAction::Copy => ItemEffect::Copy,
            FileAction::Move => ItemEffect::Move,
        };
        let item = self.resolve_write(
            index,
            action,
            Some(source),
            Some(stamp.clone()),
            destination,
            destination_key,
            effect,
            stamp.size,
        );
        if item.state == ReviewState::Ready && item.effect != ItemEffect::Skip {
            self.executing_sources.insert(source_key);
        }
        item
    }

    fn resolve_write(
        &mut self,
        index: usize,
        action: &PlannedAction,
        source: Option<&StoredPath>,
        source_stamp: Option<SourceStamp>,
        destination: PathBuf,
        destination_key: PathKey,
        effect: ItemEffect,
        bytes: u64,
    ) -> ItemValidation {
        let existing = match inspect_destination(&destination) {
            Ok(existing) => existing,
            Err(reason) => return self.blocked(index, action, reason),
        };
        let requested = destination.clone();
        let resolution = match self.resolve_conflict(destination, destination_key, existing, effect)
        {
            Ok(resolution) => resolution,
            Err(reason) => return self.blocked(index, action, reason),
        };
        let message = if resolution.effect == ItemEffect::Skip {
            Some("destination exists; Skip policy leaves it untouched".to_string())
        } else if resolution.displaced.is_some() {
            Some("existing destination moves aside to the reviewed numbered path".to_string())
        } else if matches!(
            resolution.expectation,
            DestinationExpectation::RegularFile { .. }
        ) {
            Some("replaces the reviewed destination after revalidation".to_string())
        } else if resolution.destination != requested {
            Some("existing destination stays; the incoming file uses a free numbered name".into())
        } else {
            None
        };
        self.ready_with_source(
            index,
            action,
            source,
            source_stamp,
            resolution,
            Some(bytes),
            message,
        )
    }

    fn resolve_conflict(
        &mut self,
        destination: PathBuf,
        destination_key: PathKey,
        existing: Option<fs::Metadata>,
        effect: ItemEffect,
    ) -> Result<Resolution, String> {
        let Some(meta) = existing else {
            return Ok(Resolution {
                destination,
                key: destination_key,
                expectation: DestinationExpectation::Absent,
                effect,
                displaced: None,
            });
        };
        if meta.file_type().is_symlink() {
            return Err("destination is a symbolic link; replacing links is refused".into());
        }
        if !meta.is_file() {
            return Err("destination exists and is not a regular file".into());
        }
        let stamp = stamp_from_metadata(&meta, &destination)
            .map_err(|error| error.message().to_string())?;
        match self.plan.policy {
            ConflictPolicy::Skip => Ok(Resolution {
                destination,
                key: destination_key,
                expectation: DestinationExpectation::SkipExisting {
                    stamp: stamp.clone(),
                },
                effect: ItemEffect::Skip,
                displaced: None,
            }),
            ConflictPolicy::Replace => Ok(Resolution {
                destination,
                key: destination_key,
                expectation: DestinationExpectation::RegularFile { stamp },
                effect,
                displaced: None,
            }),
            ConflictPolicy::KeepBoth => {
                let candidate = self.free_numbered_path(&destination)?;
                let key = paths::path_key(&candidate, self.destination_root)
                    .map_err(|error| error.message().to_string())?;
                Ok(Resolution {
                    destination: candidate,
                    key,
                    expectation: DestinationExpectation::Absent,
                    effect,
                    displaced: None,
                })
            }
            ConflictPolicy::PreserveExisting => {
                let displaced = self.free_numbered_path(&destination)?;
                Ok(Resolution {
                    destination,
                    key: destination_key,
                    expectation: DestinationExpectation::RegularFile { stamp },
                    effect,
                    displaced: Some(displaced),
                })
            }
        }
    }

    /// Smallest free `n_<stem><suffix>` beside `target`, checked against the
    /// filesystem and every destination this plan already proposes. This is a
    /// proposal, never an OS reservation: X1b must commit without clobbering.
    fn free_numbered_path(&self, target: &Path) -> Result<PathBuf, String> {
        let parent = target
            .parent()
            .ok_or_else(|| "numbered destination has no parent directory".to_string())?;
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                "destination file name is not valid UTF-8; automatic numbering is unsupported"
                    .to_string()
            })?;
        let spelling = Path::new(name);
        let stem = spelling
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or(name);
        let suffix = match spelling.extension().and_then(|ext| ext.to_str()) {
            Some(extension) => format!(".{extension}"),
            None => String::new(),
        };
        for counter in 1..=MAX_NUMBERED_ATTEMPTS {
            let candidate = parent.join(format!("{counter}_{stem}{suffix}"));
            let key = paths::path_key(&candidate, self.destination_root)
                .map_err(|error| error.message().to_string())?;
            if self.destinations.contains(&key) {
                continue;
            }
            match fs::symlink_metadata(&candidate) {
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(candidate),
                Ok(_) => continue,
                Err(error) => {
                    return Err(format!(
                        "numbered destination {} cannot be inspected: {error}",
                        candidate.display()
                    ));
                }
            }
        }
        Err(format!(
            "no free numbered name for {} within {MAX_NUMBERED_ATTEMPTS} attempts",
            target.display()
        ))
    }

    fn check_parent_chain(&self, relative: &Path, index: usize) -> Result<(), String> {
        let mut components = relative.components().peekable();
        let mut prefix = PathBuf::new();
        while let Some(component) = components.next() {
            if components.peek().is_none() {
                break;
            }
            prefix.push(component.as_os_str());
            let parent = self.destination_root.join(&prefix);
            let key = paths::path_key(&parent, self.destination_root)
                .map_err(|error| error.message().to_string())?;
            if let Some(directory_index) = self.planned_directories.get(&key) {
                if *directory_index > index {
                    return Err(format!(
                        "parent directory action for {} comes later in the plan",
                        prefix.display()
                    ));
                }
                continue;
            }
            match fs::symlink_metadata(&parent) {
                Ok(meta) if meta.is_dir() => continue,
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(format!(
                        "parent {} is a symbolic link; containment cannot be proven",
                        prefix.display()
                    ));
                }
                Ok(_) => {
                    return Err(format!(
                        "parent {} exists and is not a directory",
                        prefix.display()
                    ));
                }
                Err(error) if error.kind() == ErrorKind::NotFound => {
                    return Err(format!(
                        "parent directory {} does not exist and is not created by this plan",
                        prefix.display()
                    ));
                }
                Err(error) => {
                    return Err(format!(
                        "parent {} cannot be inspected: {error}",
                        prefix.display()
                    ));
                }
            }
        }
        Ok(())
    }

    fn ready(
        &mut self,
        index: usize,
        action: &PlannedAction,
        resolution: Resolution,
        source_stamp: Option<SourceStamp>,
        bytes: Option<u64>,
        message: Option<String>,
    ) -> ItemValidation {
        self.ready_with_source(
            index,
            action,
            action.source(),
            source_stamp,
            resolution,
            bytes,
            message,
        )
    }

    fn ready_with_source(
        &mut self,
        index: usize,
        action: &PlannedAction,
        source: Option<&StoredPath>,
        source_stamp: Option<SourceStamp>,
        resolution: Resolution,
        bytes: Option<u64>,
        message: Option<String>,
    ) -> ItemValidation {
        if resolution.effect != ItemEffect::Skip {
            if !self.destinations.insert(resolution.key.clone()) {
                return self.blocked(
                    index,
                    action,
                    format!(
                        "duplicate destination: {}",
                        resolution.destination.display()
                    ),
                );
            }
            self.executing_destinations.push((index, resolution.key));
        }
        let mut displaced = None;
        if let Some(path) = &resolution.displaced {
            match paths::path_key(path, self.destination_root) {
                Ok(key) => {
                    if !self.destinations.insert(key.clone()) {
                        return self.blocked(
                            index,
                            action,
                            format!(
                                "displaced destination {} collides with another planned destination",
                                path.display()
                            ),
                        );
                    }
                    self.executing_destinations.push((index, key));
                    displaced = Some(path);
                }
                Err(error) => {
                    return self.blocked(
                        index,
                        action,
                        format!("displaced destination unusable: {error}"),
                    );
                }
            }
        }
        let destination =
            match StoredPath::from_native(&resolution.destination, self.destination_root) {
                Ok(path) => path,
                Err(error) => return self.blocked(index, action, error.message().to_string()),
            };
        let displaced = match displaced {
            Some(path) => match StoredPath::from_native(path, self.destination_root) {
                Ok(path) => Some(path),
                Err(error) => return self.blocked(index, action, error.message().to_string()),
            },
            None => None,
        };
        let state = if resolution.effect == ItemEffect::Skip {
            ReviewState::Skipped
        } else {
            ReviewState::Ready
        };
        ItemValidation {
            index,
            state,
            kind: action.kind(),
            effect: resolution.effect,
            source: source.cloned(),
            destination: Some(destination),
            displaced,
            source_stamp,
            expectation: Some(resolution.expectation),
            message,
            bytes: Some(if state == ReviewState::Skipped {
                0
            } else {
                bytes.unwrap_or(0)
            }),
        }
    }

    fn blocked(
        &self,
        index: usize,
        action: &PlannedAction,
        message: impl Into<String>,
    ) -> ItemValidation {
        ItemValidation {
            index,
            state: ReviewState::Blocked,
            kind: action.kind(),
            effect: ItemEffect::Skip,
            source: action.source().cloned(),
            destination: None,
            displaced: None,
            source_stamp: None,
            expectation: None,
            message: Some(message.into()),
            bytes: None,
        }
    }

    /// A destination that is also the source of an executing item would read a
    /// file another item already overwrote, or overwrite a pending source.
    fn block_destination_source_chains(&self, items: &mut [ItemValidation]) {
        for (index, key) in &self.executing_destinations {
            if self.executing_sources.contains(key) {
                if let Some(item) = items.get_mut(*index) {
                    if item.state == ReviewState::Ready {
                        item.state = ReviewState::Blocked;
                        item.effect = ItemEffect::Skip;
                        item.message = Some(
                            "destination is the source of another transferring item in this plan"
                                .to_string(),
                        );
                    }
                }
            }
        }
    }
}

fn inspect_destination(destination: &Path) -> Result<Option<fs::Metadata>, String> {
    match fs::symlink_metadata(destination) {
        Ok(meta) => Ok(Some(meta)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "destination {} cannot be inspected: {error}",
            destination.display()
        )),
    }
}

// ---------------------------------------------------------------------------
// Journal state
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Pending,
    Running,
    Succeeded,
    Partial,
    Failed,
    Cancelled,
    Interrupted,
}

impl RunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Partial => "partial",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "partial" => Self::Partial,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Partial | Self::Failed | Self::Cancelled
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemState {
    Pending,
    Running,
    Copied,
    Moved,
    Created,
    Skipped,
    CopiedSourceRetained,
    Failed,
    Cancelled,
}

impl ItemState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Copied => "copied",
            Self::Moved => "moved",
            Self::Created => "created",
            Self::Skipped => "skipped",
            Self::CopiedSourceRetained => "copied_source_retained",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "copied" => Self::Copied,
            "moved" => Self::Moved,
            "created" => Self::Created,
            "skipped" => Self::Skipped,
            "copied_source_retained" => Self::CopiedSourceRetained,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }

    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }
}

/// Terminal batch outcome requested by the caller. Item counts are always
/// derived from the persisted items, never taken from the caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunTerminal {
    Succeeded,
    Partial,
    Failed,
    Cancelled,
}

impl RunTerminal {
    pub fn state(self) -> RunState {
        match self {
            Self::Succeeded => RunState::Succeeded,
            Self::Partial => RunState::Partial,
            Self::Failed => RunState::Failed,
            Self::Cancelled => RunState::Cancelled,
        }
    }

    fn check(&self, counts: &RunCounts) -> Result<(), String> {
        let mutated = counts.copied + counts.moved + counts.created;
        let unsuccessful = counts.failed + counts.cancelled + counts.copied_source_retained;
        match self {
            Self::Succeeded => {
                if unsuccessful > 0 {
                    return Err(format!(
                        "{} item(s) failed, were cancelled or kept their source; finalize as partial",
                        unsuccessful
                    ));
                }
                Ok(())
            }
            Self::Partial => {
                if unsuccessful == 0 {
                    return Err(
                        "no item failed, was cancelled or kept its source; finalize as succeeded"
                            .to_string(),
                    );
                }
                if mutated + counts.skipped == 0 {
                    return Err("no item completed; finalize as failed".to_string());
                }
                Ok(())
            }
            Self::Failed => {
                if counts.failed == 0 && counts.total > 0 {
                    return Err(
                        "no item failed; finalize as succeeded, partial or cancelled".to_string(),
                    );
                }
                if mutated > 0 {
                    return Err(format!("{mutated} item(s) completed; finalize as partial"));
                }
                Ok(())
            }
            Self::Cancelled => {
                if counts.cancelled == 0 && counts.total > 0 {
                    return Err(
                        "no item was cancelled; finalize as succeeded, partial or failed"
                            .to_string(),
                    );
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunCounts {
    pub total: usize,
    pub pending: usize,
    pub running: usize,
    pub copied: usize,
    pub moved: usize,
    pub created: usize,
    pub skipped: usize,
    pub copied_source_retained: usize,
    pub failed: usize,
    pub cancelled: usize,
}

impl RunCounts {
    fn add(&mut self, state: ItemState, count: usize) {
        self.total += count;
        match state {
            ItemState::Pending => self.pending += count,
            ItemState::Running => self.running += count,
            ItemState::Copied => self.copied += count,
            ItemState::Moved => self.moved += count,
            ItemState::Created => self.created += count,
            ItemState::Skipped => self.skipped += count,
            ItemState::CopiedSourceRetained => self.copied_source_retained += count,
            ItemState::Failed => self.failed += count,
            ItemState::Cancelled => self.cancelled += count,
        }
    }

    pub fn unfinished(&self) -> usize {
        self.pending + self.running
    }

    pub fn completed(&self) -> usize {
        self.copied + self.moved + self.created + self.skipped
    }
}

impl RunInspection {
    /// Every recovery path the items reported, in event order.
    pub fn recovery_paths(&self) -> Vec<&Path> {
        self.items
            .iter()
            .flat_map(|item| item.events.iter())
            .flat_map(ItemEvent::recovery_paths)
            .collect()
    }
}

/// One appended journal event for an item. Earlier events are never dropped, so
/// recovery paths reported mid-run survive later terminal updates.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemEvent {
    pub recorded_at: String,
    pub state: ItemState,
    pub message: Option<String>,
    pub recovery: Vec<StoredPath>,
}

impl ItemEvent {
    pub fn recovery_paths(&self) -> impl Iterator<Item = &Path> {
        self.recovery.iter().map(StoredPath::path)
    }
}

#[derive(Serialize, Deserialize)]
struct ItemEnvelope {
    schema_version: u32,
    events: Vec<ItemEvent>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunResultSummary {
    pub recorded_at: String,
    pub state: RunState,
    pub counts: RunCounts,
}

#[derive(Serialize, Deserialize)]
struct RunEnvelope {
    schema_version: u32,
    intent: OperationPlan,
    items: Vec<ItemValidation>,
    warnings: Vec<String>,
    #[serde(default)]
    result: Option<RunResultSummary>,
}

/// Summary returned after finalizing a run.
#[derive(Clone, Debug)]
pub struct RunSummary {
    pub run_id: i64,
    pub state: RunState,
    pub started_at: String,
    pub finished_at: String,
    pub counts: RunCounts,
}

/// One item read back from the journal.
#[derive(Clone, Debug)]
pub struct ItemInspection {
    pub ordinal: i64,
    pub state: ItemState,
    pub source: Option<PathBuf>,
    pub source_display: Option<String>,
    pub destination: Option<PathBuf>,
    pub destination_display: Option<String>,
    pub source_undecodable: bool,
    pub destination_undecodable: bool,
    pub events: Vec<ItemEvent>,
    pub message_error: Option<String>,
}

/// Read-only recovery view: the reviewed intent and evidence, every item state
/// and every recovery path reported so far. Nothing here deletes or replays.
#[derive(Clone, Debug)]
pub struct RunInspection {
    pub run_id: i64,
    pub kind: String,
    pub state: RunState,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub intent: Option<OperationPlan>,
    pub reviewed_items: Vec<ItemValidation>,
    pub warnings: Vec<String>,
    pub result: Option<RunResultSummary>,
    pub detail_error: Option<String>,
    pub items: Vec<ItemInspection>,
}

/// Run ids marked interrupted by [`mark_unfinished_interrupted`].
#[derive(Clone, Debug)]
pub struct InterruptReport {
    pub run_ids: Vec<i64>,
    pub marked_at: String,
}

// ---------------------------------------------------------------------------
// Journal persistence
// ---------------------------------------------------------------------------

/// Persists a validated plan as one pending run plus its pending items in a
/// single catalog transaction. Callers must await the reply before X1b may
/// touch the filesystem.
pub fn persist_validated_intent(
    catalog: &Catalog,
    validated: ValidatedPlan,
) -> StorageResult<Pending<i64>> {
    catalog.transaction(move |tx| insert_intent(tx, &validated))
}

fn insert_intent(tx: &mut Transaction<'_>, validated: &ValidatedPlan) -> StorageResult<i64> {
    let envelope = RunEnvelope {
        schema_version: RUN_ENVELOPE_VERSION,
        intent: validated.plan.clone(),
        items: validated.items.clone(),
        warnings: validated.warnings.clone(),
        result: None,
    };
    let detail = serde_json::to_string(&envelope)
        .map_err(|error| db_error("could not encode the operation intent envelope", error))?;
    let kind = validated.plan.origin.kind();
    if validated.items.len() != validated.plan.actions.len() {
        return Err(invalid(format!(
            "validated plan {} reviews {} item(s) for {} action(s)",
            validated.plan.id,
            validated.items.len(),
            validated.plan.actions.len()
        )));
    }
    let affected = tx
        .execute(
            "INSERT INTO operation_runs(kind,state,started_at,detail) VALUES(?1,'pending',?2,?3)",
            params![kind, utc_timestamp_now(), detail],
        )
        .map_err(|error| db_error("could not insert the operation run", error))?;
    if affected != 1 {
        return Err(db_error(
            "could not insert the operation run",
            "no row was written",
        ));
    }
    let run_id = tx.last_insert_rowid();
    for (ordinal, item) in validated.items.iter().enumerate() {
        let source = item.source.as_ref();
        let destination = item.destination.as_ref();
        let affected = tx
            .execute(
                "INSERT INTO operation_items(run_id,ordinal,state,source_spelling,source_display,destination_spelling,destination_display,message) \
                 VALUES(?1,?2,'pending',?3,?4,?5,?6,NULL)",
                params![
                    run_id,
                    ordinal as i64,
                    source.map(|path| path.spelling()),
                    source.and_then(StoredPath::display),
                    destination.map(|path| path.spelling()),
                    destination.and_then(StoredPath::display),
                ],
            )
            .map_err(|error| {
                db_error(
                    format!("could not insert item {ordinal} of run {run_id}"),
                    error,
                )
            })?;
        if affected != 1 {
            return Err(db_error(
                format!("could not insert item {ordinal} of run {run_id}"),
                "no row was written",
            ));
        }
    }
    Ok(run_id)
}

/// Marks a pending run running. Only `pending -> running` is accepted; every
/// other state is a state-machine violation with a readable cause.
pub fn mark_run_running(catalog: &Catalog, run_id: i64) -> StorageResult<Pending<RunCounts>> {
    catalog.transaction(move |tx| {
        let affected = tx
            .execute(
                "UPDATE operation_runs SET state='running' WHERE id=?1 AND state='pending'",
                [run_id],
            )
            .map_err(|error| db_error("could not start the operation run", error))?;
        if affected == 0 {
            let state = read_run_state(tx, run_id)?;
            return Err(invalid(format!(
                "operation run {run_id} is {}; only a pending run may start",
                state.as_str()
            )));
        }
        if affected != 1 {
            return Err(db_error(
                "could not start the operation run",
                "more than one row was updated",
            ));
        }
        count_items(tx, run_id)
    })
}

/// Records one item outcome after its filesystem step. Terminal items are never
/// overwritten; a recorded event is appended so prior recovery paths survive.
pub fn update_item_outcome(
    catalog: &Catalog,
    run_id: i64,
    ordinal: i64,
    state: ItemState,
    message: Option<String>,
    recovery: &[PathBuf],
) -> StorageResult<Pending<RunCounts>> {
    let recovery = recovery
        .iter()
        .map(|path| StoredPath::absolute(path))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| invalid(format!("recovery path refused: {error}")))?;
    catalog.transaction(move |tx| {
        let run_state = read_run_state(tx, run_id)?;
        if run_state != RunState::Running {
            return Err(invalid(format!(
                "operation run {run_id} is {}; item outcomes are only recorded while it is running",
                run_state.as_str()
            )));
        }
        let row: Option<(String, Option<String>)> = tx
            .query_row(
                "SELECT state, message FROM operation_items WHERE run_id=?1 AND ordinal=?2",
                params![run_id, ordinal],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| db_error("could not read the operation item", error))?;
        let Some((current, existing_message)) = row else {
            return Err(invalid(format!(
                "operation run {run_id} has no item at ordinal {ordinal}"
            )));
        };
        let current = ItemState::parse(&current).ok_or_else(|| {
            invalid(format!(
                "item {ordinal} of run {run_id} has an unrecognized persisted state"
            ))
        })?;
        item_transition(current, state).map_err(invalid)?;
        write_item_event(
            tx,
            run_id,
            ordinal,
            existing_message,
            state,
            message,
            &recovery,
        )?;
        count_items(tx, run_id)
    })
}

fn item_transition(current: ItemState, next: ItemState) -> Result<(), String> {
    if next == ItemState::Pending {
        return Err("an item cannot move back to pending".to_string());
    }
    match current {
        ItemState::Pending => Ok(()),
        ItemState::Running => {
            if next == ItemState::Running {
                Err("the item is already running".to_string())
            } else {
                Ok(())
            }
        }
        terminal => Err(format!(
            "the item is already {}; terminal item outcomes are never overwritten",
            terminal.as_str()
        )),
    }
}

fn write_item_event(
    tx: &Transaction<'_>,
    run_id: i64,
    ordinal: i64,
    existing_message: Option<String>,
    state: ItemState,
    message: Option<String>,
    recovery: &[StoredPath],
) -> StorageResult<()> {
    let mut envelope = parse_item_envelope(existing_message, run_id, ordinal)?;
    envelope.events.push(ItemEvent {
        recorded_at: utc_timestamp_now(),
        state,
        message,
        recovery: recovery.to_vec(),
    });
    let text = serde_json::to_string(&envelope)
        .map_err(|error| db_error("could not encode the item envelope", error))?;
    let affected = tx
        .execute(
            "UPDATE operation_items SET state=?1, message=?2 WHERE run_id=?3 AND ordinal=?4",
            params![state.as_str(), text, run_id, ordinal],
        )
        .map_err(|error| db_error("could not record the item outcome", error))?;
    if affected != 1 {
        return Err(db_error(
            "could not record the item outcome",
            format!("item {ordinal} of run {run_id} was not the expected single row"),
        ));
    }
    Ok(())
}

fn parse_item_envelope(
    message: Option<String>,
    run_id: i64,
    ordinal: i64,
) -> StorageResult<ItemEnvelope> {
    match message {
        None => Ok(ItemEnvelope {
            schema_version: ITEM_ENVELOPE_VERSION,
            events: Vec::new(),
        }),
        Some(text) => {
            let envelope: ItemEnvelope = serde_json::from_str(&text).map_err(|error| {
                invalid(format!(
                    "the message of item {ordinal} of run {run_id} is not a recognized item envelope; refusing to overwrite it: {error}"
                ))
            })?;
            if envelope.schema_version != ITEM_ENVELOPE_VERSION {
                return Err(invalid(format!(
                    "the message of item {ordinal} of run {run_id} uses envelope version {}; this build writes version {ITEM_ENVELOPE_VERSION} and refuses to overwrite unknown data",
                    envelope.schema_version
                )));
            }
            Ok(envelope)
        }
    }
}

/// Finalizes a run. Terminal counts are derived from the persisted items; a
/// cancellation first records every unfinished item as cancelled, so completed
/// items stay completed. The intent envelope is preserved and only the result
/// section is added.
pub fn finalize_run(
    catalog: &Catalog,
    run_id: i64,
    terminal: RunTerminal,
) -> StorageResult<Pending<RunSummary>> {
    catalog.transaction(move |tx| {
        let (state, started_at) = read_run_head(tx, run_id)?;
        if !matches!(state, RunState::Pending | RunState::Running) {
            return Err(invalid(format!(
                "operation run {run_id} is already {}",
                state.as_str()
            )));
        }
        if terminal == RunTerminal::Cancelled {
            cancel_unfinished_items(tx, run_id)?;
        }
        let counts = count_items(tx, run_id)?;
        if counts.unfinished() > 0 {
            return Err(invalid(format!(
                "cannot finalize operation run {run_id}: {} item(s) are still pending or running",
                counts.unfinished()
            )));
        }
        terminal.check(&counts).map_err(invalid)?;
        let detail: Option<String> = tx
            .query_row(
                "SELECT detail FROM operation_runs WHERE id=?1",
                [run_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()
            .map_err(|error| db_error("could not read the operation intent envelope", error))?
            .flatten();
        let mut envelope = parse_run_envelope(detail, run_id)?;
        if envelope.result.is_some() {
            return Err(invalid(format!(
                "operation run {run_id} already recorded a terminal result"
            )));
        }
        let result = RunResultSummary {
            recorded_at: utc_timestamp_now(),
            state: terminal.state(),
            counts: counts.clone(),
        };
        envelope.result = Some(result);
        let text = serde_json::to_string(&envelope)
            .map_err(|error| db_error("could not encode the operation result", error))?;
        let finished_at = utc_timestamp_now();
        let affected = tx
            .execute(
                "UPDATE operation_runs SET state=?1, finished_at=?2, detail=?3 WHERE id=?4 AND state IN ('pending','running')",
                params![terminal.state().as_str(), finished_at, text, run_id],
            )
            .map_err(|error| db_error("could not finalize the operation run", error))?;
        if affected != 1 {
            return Err(db_error(
                "could not finalize the operation run",
                format!("run {run_id} was not in a finalizable state"),
            ));
        }
        Ok(RunSummary {
            run_id,
            state: terminal.state(),
            started_at,
            finished_at,
            counts,
        })
    })
}

fn cancel_unfinished_items(tx: &Transaction<'_>, run_id: i64) -> StorageResult<()> {
    let ordinals: Vec<i64> = {
        let mut statement = tx
            .prepare(
                "SELECT ordinal FROM operation_items WHERE run_id=?1 AND state IN ('pending','running') ORDER BY ordinal",
            )
            .map_err(|error| db_error("could not read unfinished items", error))?;
        let rows = statement
            .query_map([run_id], |row| row.get(0))
            .map_err(|error| db_error("could not read unfinished items", error))?;
        let mut ordinals = Vec::new();
        for row in rows {
            ordinals.push(row.map_err(|error| db_error("could not read unfinished items", error))?);
        }
        ordinals
    };
    for ordinal in ordinals {
        let message: Option<String> = tx
            .query_row(
                "SELECT message FROM operation_items WHERE run_id=?1 AND ordinal=?2",
                params![run_id, ordinal],
                |row| row.get(0),
            )
            .map_err(|error| db_error("could not read the item message", error))?;
        write_item_event(
            tx,
            run_id,
            ordinal,
            message,
            ItemState::Cancelled,
            Some("run cancelled before this item completed".to_string()),
            &[],
        )?;
    }
    Ok(())
}

/// Marks every pending/running run interrupted, for startup recovery. Item
/// states are left untouched for inspection. Idempotent, and it never deletes
/// or replays anything.
pub fn mark_unfinished_interrupted(catalog: &Catalog) -> StorageResult<Pending<InterruptReport>> {
    catalog.transaction(|tx| {
        let run_ids: Vec<i64> = {
            let mut statement = tx
                .prepare("SELECT id FROM operation_runs WHERE state IN ('pending','running') ORDER BY id")
                .map_err(|error| db_error("could not read unfinished runs", error))?;
            let rows = statement
                .query_map([], |row| row.get(0))
                .map_err(|error| db_error("could not read unfinished runs", error))?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row.map_err(|error| db_error("could not read unfinished runs", error))?);
            }
            ids
        };
        let marked_at = utc_timestamp_now();
        let affected = tx
            .execute(
                "UPDATE operation_runs SET state='interrupted', finished_at=?1 WHERE state IN ('pending','running')",
                params![marked_at],
            )
            .map_err(|error| db_error("could not mark unfinished runs interrupted", error))?;
        if affected != run_ids.len() {
            return Err(db_error(
                "could not mark unfinished runs interrupted",
                format!(
                    "updated {affected} row(s) but read {} unfinished run(s)",
                    run_ids.len()
                ),
            ));
        }
        Ok(InterruptReport { run_ids, marked_at })
    })
}

/// Reads every unfinished or interrupted run with its intent, evidence, item
/// states and recovery paths. Inspection only.
pub fn read_incomplete_runs(catalog: &Catalog) -> StorageResult<Pending<Vec<RunInspection>>> {
    catalog.submit(|connection| {
        let run_ids: Vec<i64> = {
            let mut statement = connection
                .prepare(
                    "SELECT id FROM operation_runs WHERE state IN ('pending','running','interrupted') ORDER BY id",
                )
                .map_err(|error| db_error("could not read unfinished runs", error))?;
            let rows = statement
                .query_map([], |row| row.get(0))
                .map_err(|error| db_error("could not read unfinished runs", error))?;
            let mut ids = Vec::new();
            for row in rows {
                ids.push(row.map_err(|error| db_error("could not read unfinished runs", error))?);
            }
            ids
        };
        let mut runs = Vec::with_capacity(run_ids.len());
        for run_id in run_ids {
            if let Some(run) = load_run(connection, run_id)? {
                runs.push(run);
            }
        }
        Ok(runs)
    })
}

/// Reads one run whatever its state, for recovery inspection.
pub fn read_run(catalog: &Catalog, run_id: i64) -> StorageResult<Pending<Option<RunInspection>>> {
    catalog.submit(move |connection| load_run(connection, run_id))
}

fn load_run(
    connection: &rusqlite::Connection,
    run_id: i64,
) -> StorageResult<Option<RunInspection>> {
    let row: Option<(String, String, String, Option<String>, Option<String>)> = connection
        .query_row(
            "SELECT kind, state, started_at, finished_at, detail FROM operation_runs WHERE id=?1",
            [run_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )
        .optional()
        .map_err(|error| db_error("could not read the operation run", error))?;
    let Some((kind, state, started_at, finished_at, detail)) = row else {
        return Ok(None);
    };
    let state = RunState::parse(&state).ok_or_else(|| {
        invalid(format!(
            "operation run {run_id} has an unrecognized persisted state"
        ))
    })?;
    let mut intent = None;
    let mut reviewed_items = Vec::new();
    let mut warnings = Vec::new();
    let mut result = None;
    let mut detail_error = None;
    match detail {
        None => {
            detail_error = Some(format!(
                "operation run {run_id} has no intent envelope; intent, evidence and review detail are unavailable"
            ));
        }
        Some(text) => match serde_json::from_str::<RunEnvelope>(&text) {
            Ok(envelope) if envelope.schema_version == RUN_ENVELOPE_VERSION => {
                intent = Some(envelope.intent);
                reviewed_items = envelope.items;
                warnings = envelope.warnings;
                result = envelope.result;
            }
            Ok(envelope) => {
                detail_error = Some(format!(
                    "operation run {run_id} detail uses envelope version {}; this build reads version {RUN_ENVELOPE_VERSION} and left it untouched",
                    envelope.schema_version
                ));
            }
            Err(error) => {
                detail_error = Some(format!(
                    "operation run {run_id} detail is not a recognized envelope and was left untouched: {error}"
                ));
            }
        },
    }
    Ok(Some(RunInspection {
        run_id,
        kind,
        state,
        started_at,
        finished_at,
        intent,
        reviewed_items,
        warnings,
        result,
        detail_error,
        items: load_items(connection, run_id)?,
    }))
}

fn load_items(
    connection: &rusqlite::Connection,
    run_id: i64,
) -> StorageResult<Vec<ItemInspection>> {
    let mut statement = connection
        .prepare(
            "SELECT ordinal, state, source_spelling, source_display, destination_spelling, destination_display, message \
             FROM operation_items WHERE run_id=?1 ORDER BY ordinal",
        )
        .map_err(|error| db_error("could not read operation items", error))?;
    let rows = statement
        .query_map([run_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
            ))
        })
        .map_err(|error| db_error("could not read operation items", error))?;
    let mut items = Vec::new();
    for row in rows {
        let (ordinal, state, source, source_display, destination, destination_display, message) =
            row.map_err(|error| db_error("could not read an operation item", error))?;
        let state = ItemState::parse(&state).ok_or_else(|| {
            invalid(format!(
                "item {ordinal} of run {run_id} has an unrecognized persisted state"
            ))
        })?;
        let (source, source_undecodable) = decode_spelling(source);
        let (destination, destination_undecodable) = decode_spelling(destination);
        let mut events = Vec::new();
        let mut message_error = None;
        if let Some(text) = message {
            match serde_json::from_str::<ItemEnvelope>(&text) {
                Ok(envelope) if envelope.schema_version == ITEM_ENVELOPE_VERSION => {
                    events = envelope.events;
                }
                Ok(envelope) => {
                    message_error = Some(format!(
                        "item {ordinal} message uses envelope version {}; this build reads version {ITEM_ENVELOPE_VERSION} and left it untouched",
                        envelope.schema_version
                    ));
                }
                Err(error) => {
                    message_error = Some(format!(
                        "item {ordinal} message is not a recognized envelope and was left untouched: {error}"
                    ));
                }
            }
        }
        items.push(ItemInspection {
            ordinal,
            state,
            source,
            source_display,
            destination,
            destination_display,
            source_undecodable,
            destination_undecodable,
            events,
            message_error,
        });
    }
    Ok(items)
}

fn decode_spelling(spelling: Option<Vec<u8>>) -> (Option<PathBuf>, bool) {
    match spelling {
        None => (None, false),
        Some(bytes) => {
            let columns = PathColumns {
                storage: None,
                platform: CURRENT_PLATFORM,
                spelling: bytes,
                display: None,
            };
            match columns.decode() {
                Some(path) => (Some(path), false),
                None => (None, true),
            }
        }
    }
}

fn run_state_from_row(state: &str, run_id: i64) -> StorageResult<RunState> {
    RunState::parse(state).ok_or_else(|| {
        invalid(format!(
            "operation run {run_id} has an unrecognized persisted state"
        ))
    })
}

fn read_run_state(tx: &Transaction<'_>, run_id: i64) -> StorageResult<RunState> {
    read_run_head(tx, run_id).map(|(state, _)| state)
}

fn read_run_head(tx: &Transaction<'_>, run_id: i64) -> StorageResult<(RunState, String)> {
    let row: Option<(String, String)> = tx
        .query_row(
            "SELECT state, started_at FROM operation_runs WHERE id=?1",
            [run_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| db_error("could not read the operation run", error))?;
    let Some((state, started_at)) = row else {
        return Err(invalid(format!("operation run {run_id} does not exist")));
    };
    Ok((run_state_from_row(&state, run_id)?, started_at))
}

fn parse_run_envelope(detail: Option<String>, run_id: i64) -> StorageResult<RunEnvelope> {
    match detail {
        None => Err(invalid(format!(
            "operation run {run_id} has no intent envelope; refusing to overwrite unknown detail"
        ))),
        Some(text) => {
            let envelope: RunEnvelope = serde_json::from_str(&text).map_err(|error| {
                invalid(format!(
                    "operation run {run_id} detail is not a recognized envelope; refusing to overwrite it: {error}"
                ))
            })?;
            if envelope.schema_version != RUN_ENVELOPE_VERSION {
                return Err(invalid(format!(
                    "operation run {run_id} detail uses envelope version {}; this build writes version {RUN_ENVELOPE_VERSION} and refuses to overwrite unknown data",
                    envelope.schema_version
                )));
            }
            Ok(envelope)
        }
    }
}

fn count_items(tx: &Transaction<'_>, run_id: i64) -> StorageResult<RunCounts> {
    let mut statement = tx
        .prepare("SELECT state, COUNT(*) FROM operation_items WHERE run_id=?1 GROUP BY state")
        .map_err(|error| db_error("could not count operation items", error))?;
    let rows = statement
        .query_map([run_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| db_error("could not count operation items", error))?;
    let mut counts = RunCounts::default();
    for row in rows {
        let (state, count) =
            row.map_err(|error| db_error("could not count operation items", error))?;
        let state = ItemState::parse(&state).ok_or_else(|| {
            invalid(format!(
                "operation run {run_id} holds an item with an unrecognized state"
            ))
        })?;
        let count = usize::try_from(count).map_err(|_| {
            db_error(
                "could not count operation items",
                "a negative item count was returned",
            )
        })?;
        counts.add(state, count);
    }
    Ok(counts)
}
