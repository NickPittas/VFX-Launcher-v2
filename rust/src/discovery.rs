//! Read-only filesystem discovery.
//!
//! Packet F4 of the Rust migration (`CONTRACTS.md` §1, `PLAN.md` packet F4),
//! built on the F1 path rules and the F0 [`crate::jobs`] worker seam.
//!
//! Guarantees of this module:
//!
//! - **Read-only.** The only filesystem calls made anywhere in this module are
//!   [`std::fs::read_dir`], [`std::fs::metadata`], [`std::fs::symlink_metadata`]
//!   and `DirEntry::{file_type, metadata}`. Nothing is created, renamed,
//!   deleted or written, and nothing here touches the catalog. Pruning stale
//!   catalog rows is a later packet's decision made from this module's
//!   evidence, never an action taken here.
//! - **Explicit root only.** Both entry points require an absolute root and
//!   never widen scope; the default walker does not follow directory
//!   symlinks/reparse points and reports every skipped link as a
//!   [`PathIssue`].
//! - **Distinct shapes.** Immediate-child directory listing
//!   ([`child_directories`], for registration) and recursive file walking
//!   ([`walk`]) are separate APIs with separate coverage results.
//! - **Explicit coverage.** Every snapshot states [`Coverage`]: `Complete`
//!   (all requested directories enumerated), `Partial` (some directory or
//!   metadata failed — a failed subtree is never an empty result), `Cancelled`
//!   (entries so far retained), or `RootUnavailable`. [`RootContinuity`]
//!   reports whether before/after root identity proves the same filesystem was
//!   seen throughout; an SMB root that disconnects mid-scan cannot prove
//!   continuity and callers must treat absence as "not seen", not "gone".
//! - **Deterministic order.** Results are sorted by case-folded display name,
//!   with the exact native path bytes as tie-break, so no two distinct paths
//!   can compare equal and non-UTF-8 names keep their identity.
//! - **Controls.** Pause/cancel flow through [`JobControl::checkpoint`] between
//!   directories, before and after every directory listing, and while a listing
//!   is still being collected, so a cancelled read reports
//!   [`Coverage::Cancelled`] instead of a completed directory. A single blocked
//!   OS call (a stalled network `read_dir`/`stat`) still cannot be interrupted;
//!   cancellation takes effect at the next entry the OS returns. Progress and
//!   diagnostics flow through [`JobEvents`]; the terminal `finish` mapping is
//!   the caller's job.
//!
//! Deliberate limits:
//!
//! - File symlinks are included and stamped through their target (matching the
//!   legacy `os.walk`/`getsize` behavior); broken links are recorded as issues
//!   and degrade coverage to `Partial`.
//! - Skipped directory links are policy, not failure: they are reported but do
//!   not by themselves turn `Complete` into `Partial`.
//! - `excludes` patterns containing wildcards refuse the scan;
//!   fnmatch-compatible excludes arrive with the
//!   backup packet. Literal patterns match any single component or the full
//!   root-relative path, case-insensitively, mirroring the legacy finder.
//! - Repeated observable directory identities are skipped and make coverage partial.
//! - Case-insensitive suffix and exclude matching folds valid native text with
//!   [`crate::paths::fold_native_text`] — Unicode lowercase, legacy `lower()`
//!   parity for non-ASCII names — while ill-formed bytes/units keep exact byte
//!   identity instead of being replaced by a lossy character.
//! - Only regular files, and links whose target is a regular file, become
//!   [`FileEntry`] values. FIFOs, sockets and device nodes are reported as
//!   `Unsupported` issues, degrade coverage to `Partial` and are never stamped
//!   as files; `Open`/copy planning later can never see a non-regular entry.
//! - [`std::fs::DirEntry::metadata`] does not populate Windows
//!   `volume_serial_number`/`file_index`, so Windows re-stats the entry path for
//!   the stamp. When no identity kind is available the stamp carries `None`
//!   rather than an evidence-free identity.
//! - [`ScanPurpose::Incoming`] and [`ScanPurpose::StructureExport`] have no
//!   filter preset here: their pattern/sequence discovery belongs to later
//!   packets and is not approximated.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use crate::jobs::{Checkpoint, JobControl, JobEvents, JobPhase, JobProgress};
use crate::paths::{
    FileIdentity, PathCategory, PathError, PathIssue, PathResult, RootIdentity, fold_native_text,
    inspect_root,
};

/// Pause acknowledged every this many entries inside one directory.
const CHECKPOINT_EVERY: usize = 128;

/// Pause acknowledged every this many entries while listing child directories.
const CHILD_CHECKPOINT_EVERY: usize = 64;

// `DirEntryExt::file_attributes` on directory entries and `MetadataExt::file_attributes`
// in the unknown-readdir-type fallback.
#[cfg(windows)]
use std::os::windows::fs::{DirEntryExt, MetadataExt};

#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;

/// Which feature a scan serves. Only [`ScanPurpose::Projects`],
/// [`ScanPurpose::ExtensionCopy`] and [`ScanPurpose::Backup`] have presets in
/// this packet; the other variants exist so later packets keep the same type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScanPurpose {
    Projects,
    Incoming,
    ExtensionCopy,
    Backup,
    StructureExport,
}

/// How suffixes are matched, mirroring the two legacy filter styles:
/// project scans compared the file's last extension (`splitext`), extension
/// copy and backup matched the whole name suffix case-insensitively.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuffixMode {
    LastExtension,
    NameSuffix,
}

/// Suffix filter plus exclusions for one scan. Suffix matching is
/// case-insensitive (every legacy filter lowercased both sides), and the
/// exclusion name suffixes (project scans drop `.nk~` and `.nk.autosave`) are
/// checked before suffixes.
#[derive(Clone, Debug)]
pub struct ScanFilters {
    pub suffixes: Vec<String>,
    pub mode: SuffixMode,
    pub excluded_name_suffixes: Vec<String>,
    pub excludes: Vec<String>,
}

impl ScanFilters {
    pub fn matches_name(&self, name: &OsStr) -> bool {
        let bytes = name.as_encoded_bytes();
        if self
            .excluded_name_suffixes
            .iter()
            .any(|suffix| name_ends_with_case_insensitive(bytes, suffix.as_bytes()))
        {
            return false;
        }
        match self.mode {
            SuffixMode::LastExtension => last_extension_matches(bytes, &self.suffixes),
            SuffixMode::NameSuffix => self
                .suffixes
                .iter()
                .any(|suffix| name_ends_with_case_insensitive(bytes, suffix.as_bytes())),
        }
    }
}

/// Legacy tool filter defaults. `None` means "configured elsewhere, later
/// packet" — callers must not treat `None` as an empty (match-everything)
/// filter.
pub fn default_filters(purpose: ScanPurpose) -> Option<ScanFilters> {
    match purpose {
        ScanPurpose::Projects => Some(ScanFilters {
            suffixes: vec![".nk".to_string(), ".aep".to_string()],
            mode: SuffixMode::LastExtension,
            excluded_name_suffixes: vec![".nk~".to_string(), ".nk.autosave".to_string()],
            excludes: Vec::new(),
        }),
        ScanPurpose::ExtensionCopy => Some(ScanFilters {
            suffixes: vec![".nk".to_string(), ".autosave".to_string()],
            mode: SuffixMode::NameSuffix,
            excluded_name_suffixes: Vec::new(),
            excludes: Vec::new(),
        }),
        ScanPurpose::Backup => Some(ScanFilters {
            suffixes: vec![".nk".to_string(), ".aep".to_string()],
            mode: SuffixMode::NameSuffix,
            excluded_name_suffixes: Vec::new(),
            excludes: Vec::new(),
        }),
        ScanPurpose::Incoming | ScanPurpose::StructureExport => None,
    }
}

/// One scan request: an explicit absolute root, its purpose, whether the walk
/// descends into subdirectories, and the filters to apply. No constructor is
/// provided that silently substitutes an empty filter set.
#[derive(Clone, Debug)]
pub struct ScanRequest {
    pub root: PathBuf,
    pub purpose: ScanPurpose,
    pub recursive: bool,
    pub filters: ScanFilters,
}

/// Raw metadata preserved as the OS reported it. `modified` is `None` only
/// when the platform cannot represent the mtime (the entry itself is still
/// kept and the condition reported); identity is present when the platform
/// provides file-identity fields.
#[derive(Clone, Debug)]
pub struct FileStamp {
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub identity: Option<FileIdentity>,
}

/// One matched file with its native path and stamp.
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: PathBuf,
    pub stamp: FileStamp,
}

/// Enumeration coverage of a scan. Never inferred from entry counts: an empty
/// `Complete` scan of a proven-empty root is valid, and a failed subtree is
/// never an empty result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coverage {
    Complete,
    Partial,
    Cancelled,
    RootUnavailable,
}

/// Mount continuity, beyond matching before/after stat identities. This walker
/// always returns Uncertain; it has no trusted continuous mount observation.
/// Catalog pruning must remain suppressed without additional evidence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootContinuity {
    Proven,
    Uncertain,
}

/// Terminal discovery report. Discovery itself never deletes catalog rows.
#[derive(Clone, Debug)]
pub struct ScanSnapshot {
    pub root: PathBuf,
    pub purpose: ScanPurpose,
    pub entries: Vec<FileEntry>,
    pub coverage: Coverage,
    pub continuity: RootContinuity,
    pub errors: Vec<PathIssue>,
    pub root_identity: Option<RootIdentity>,
}

/// Immediate-child directory listing for registration discovery: no descent,
/// sorted by case-folded name then full native path.
#[derive(Clone, Debug)]
pub struct ChildDirectories {
    pub root: PathBuf,
    pub directories: Vec<PathBuf>,
    pub coverage: Coverage,
    pub issues: Vec<PathIssue>,
}

/// Recursively (or, with `recursive == false`, at the root only) collects
/// files matching `request.filters` under the explicit absolute root. Returns
/// `Err` only for an invalid request; every filesystem outcome — including
/// cancellation and an unavailable root — is an `Ok` snapshot with explicit
/// coverage. This is snapshot evidence only; execution re-validates anything
/// it acts on.
pub fn walk(
    request: &ScanRequest,
    control: &JobControl,
    events: &JobEvents,
) -> PathResult<ScanSnapshot> {
    // The original Launcher and extension copier count first, then process the
    // matching files against that denominator. Keep both real filesystem passes.
    if matches!(
        request.purpose,
        ScanPurpose::Projects | ScanPurpose::ExtensionCopy
    ) {
        let counted = walk_pass(request, control, events, None)?;
        if counted.coverage != Coverage::Complete {
            return Ok(counted);
        }
        let total = counted.entries.len() as u64;
        drop(counted);
        events.progress(
            JobPhase::Scanning,
            JobProgress {
                total: Some(total),
                current: Some(request.root.clone()),
                ..JobProgress::default()
            },
        );
        return walk_pass(request, control, events, Some(total));
    }
    walk_pass(request, control, events, None)
}

fn walk_pass(
    request: &ScanRequest,
    control: &JobControl,
    events: &JobEvents,
    total: Option<u64>,
) -> PathResult<ScanSnapshot> {
    let phase = if total.is_none()
        && matches!(
            request.purpose,
            ScanPurpose::Projects | ScanPurpose::ExtensionCopy
        ) {
        JobPhase::Counting
    } else {
        JobPhase::Scanning
    };
    if !request.root.is_absolute() {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            Some(&request.root),
            "scan root must be an absolute path",
        ));
    }
    let mut issues: Vec<PathIssue> = Vec::new();
    let mut root_identity = match inspect_root(&request.root) {
        Ok(identity) => Some(identity),
        Err(error) => {
            report_issue(
                &mut issues,
                events,
                &request.root,
                error.category(),
                error.message().to_string(),
            );
            return Ok(ScanSnapshot {
                root: request.root.clone(),
                purpose: request.purpose,
                entries: Vec::new(),
                coverage: Coverage::RootUnavailable,
                continuity: RootContinuity::Uncertain,
                errors: issues,
                root_identity: None,
            });
        }
    };
    let excludes = build_excludes(&request.filters, events)?;
    let mut entries: Vec<FileEntry> = Vec::new();
    let mut degraded = false;
    let mut cancelled = false;
    let mut visited = HashSet::new();
    let mut stack = vec![PendingDir {
        absolute: request.root.clone(),
        relative: PathBuf::new(),
    }];
    'outer: while let Some(pending) = stack.pop() {
        match control.checkpoint() {
            Checkpoint::Cancelled => {
                cancelled = true;
                break 'outer;
            }
            Checkpoint::Continue => {}
        }
        match inspect_root(&pending.absolute) {
            Ok(identity) if !visited.insert(identity) => {
                report_issue(
                    &mut issues,
                    events,
                    &pending.absolute,
                    PathCategory::Unsupported,
                    "Directory identity was already scanned; repeated mount or alias skipped."
                        .into(),
                );
                degraded = true;
                continue;
            }
            Err(error) => {
                report_issue(
                    &mut issues,
                    events,
                    &pending.absolute,
                    error.category(),
                    error.message().into(),
                );
                degraded = true;
                continue;
            }
            Ok(_) => {}
        }
        events.progress(
            phase,
            JobProgress {
                completed: entries.len() as u64,
                total,
                bytes_done: 0,
                bytes_total: None,
                current: Some(pending.absolute.clone()),
            },
        );
        let dir_entries = match read_sorted(&pending.absolute, control) {
            Listing::Entries(dir_entries) => dir_entries,
            Listing::Cancelled => {
                cancelled = true;
                break 'outer;
            }
            Listing::Failed(category, message) => {
                report_issue(&mut issues, events, &pending.absolute, category, message);
                degraded = true;
                continue;
            }
        };
        for (index, entry) in dir_entries.into_iter().enumerate() {
            if index % CHECKPOINT_EVERY == 0 && control.checkpoint() == Checkpoint::Cancelled {
                cancelled = true;
                break 'outer;
            }
            let child_relative = pending.relative.join(entry.file_name());
            if request.purpose == ScanPurpose::Projects && is_project_autosave(&child_relative) {
                continue;
            }
            if is_excluded(&child_relative, &excludes) {
                continue;
            }
            match visit_entry(&entry, &pending.relative, &request.filters) {
                Visit::Skip => {}
                Visit::Issue(category, message) => {
                    report_issue(&mut issues, events, &entry.path(), category, message);
                    degraded = true;
                }
                Visit::Keep(file_entry, issue) => {
                    entries.push(file_entry);
                    events.progress(
                        phase,
                        JobProgress {
                            completed: entries.len() as u64,
                            total,
                            current: Some(entry.path()),
                            ..JobProgress::default()
                        },
                    );
                    if let Some((category, message)) = issue {
                        report_issue(&mut issues, events, &entry.path(), category, message);
                        degraded = true;
                    }
                }
                Visit::Descend(absolute, relative) => {
                    if request.recursive {
                        stack.push(PendingDir { absolute, relative });
                    }
                }
            }
            if index % CHECKPOINT_EVERY == 0 {
                events.progress(
                    phase,
                    JobProgress {
                        completed: entries.len() as u64,
                        total,
                        current: Some(entry.path()),
                        ..JobProgress::default()
                    },
                );
            }
        }
    }
    entries.sort_by(|a, b| display_sort_key(&a.path).cmp(&display_sort_key(&b.path)));
    // Matching endpoint identities cannot prove uninterrupted mount continuity.
    let continuity = RootContinuity::Uncertain;
    if !cancelled {
        match inspect_root(&request.root) {
            Ok(later) => {
                let same = root_identity.is_some_and(|before| before.same_identity(&later));
                if !same {
                    report_issue(
                        &mut issues,
                        events,
                        &request.root,
                        PathCategory::Unavailable,
                        "root identity could not be re-proven after the scan; treat unseen files as unverified, not deleted".to_string(),
                    );
                    degraded = true;
                }
                root_identity = Some(later);
            }
            Err(_) => {
                report_issue(
                    &mut issues,
                    events,
                    &request.root,
                    PathCategory::Unavailable,
                    "root identity could not be re-proven after the scan; treat unseen files as unverified, not deleted".to_string(),
                );
                degraded = true;
            }
        }
    }
    let coverage = if cancelled {
        Coverage::Cancelled
    } else if degraded {
        Coverage::Partial
    } else {
        Coverage::Complete
    };
    Ok(ScanSnapshot {
        root: request.root.clone(),
        purpose: request.purpose,
        entries,
        coverage,
        continuity,
        errors: issues,
        root_identity,
    })
}

/// Lists the immediate child directories of `root` without descending,
/// including directory symlinks (legacy `scandir`/`is_dir` followed links when
/// proposing registration candidates), sorted by case-folded name then full
/// native path. Returns `Err` only for a non-absolute root.
pub fn child_directories(
    root: &Path,
    control: &JobControl,
    events: &JobEvents,
) -> PathResult<ChildDirectories> {
    if !root.is_absolute() {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            Some(root),
            "discovery root must be an absolute path",
        ));
    }
    let mut issues: Vec<PathIssue> = Vec::new();
    let mut directories: Vec<PathBuf> = Vec::new();
    if let Err(error) = inspect_root(root) {
        report_issue(
            &mut issues,
            events,
            root,
            error.category(),
            error.message().to_string(),
        );
        return Ok(ChildDirectories {
            root: root.to_path_buf(),
            directories,
            coverage: Coverage::RootUnavailable,
            issues,
        });
    }
    let mut coverage = Coverage::Complete;
    let entries = match read_sorted(root, control) {
        Listing::Entries(entries) => entries,
        Listing::Cancelled => {
            return Ok(ChildDirectories {
                root: root.to_path_buf(),
                directories,
                coverage: Coverage::Cancelled,
                issues,
            });
        }
        Listing::Failed(category, message) => {
            report_issue(&mut issues, events, root, category, message);
            return Ok(ChildDirectories {
                root: root.to_path_buf(),
                directories,
                coverage: Coverage::Partial,
                issues,
            });
        }
    };
    for (index, entry) in entries.into_iter().enumerate() {
        if index % CHILD_CHECKPOINT_EVERY == 0 && control.checkpoint() == Checkpoint::Cancelled {
            coverage = Coverage::Cancelled;
            break;
        }
        match entry_shape(&entry) {
            EntryShape::Directory | EntryShape::DirectoryLink => directories.push(entry.path()),
            EntryShape::Unreadable(error) => {
                report_issue(
                    &mut issues,
                    events,
                    &entry.path(),
                    io_category(&error),
                    format!("could not inspect entry: {error}"),
                );
                coverage = Coverage::Partial;
            }
            EntryShape::File(_) | EntryShape::FileLink(_) | EntryShape::Special => {}
        }
    }
    directories.sort_by(|a, b| display_sort_key(a).cmp(&display_sort_key(b)));
    Ok(ChildDirectories {
        root: root.to_path_buf(),
        directories,
        coverage,
        issues,
    })
}

struct PendingDir {
    absolute: PathBuf,
    relative: PathBuf,
}

enum Visit {
    Skip,
    Issue(PathCategory, String),
    Keep(FileEntry, Option<(PathCategory, String)>),
    Descend(PathBuf, PathBuf),
}

/// Classifies one directory entry into what the walker should do with it.
enum EntryShape {
    Directory,
    /// Regular file, stamped from its own metadata.
    File(fs::Metadata),
    /// Directory symlink or reparse point: never descended by this walker.
    DirectoryLink,
    /// Symlink resolving to a regular file; stamped through the target.
    FileLink(fs::Metadata),
    /// FIFO, socket, device node or a link to one: not a planable file.
    Special,
    Unreadable(std::io::Error),
}

fn entry_shape(entry: &std::fs::DirEntry) -> EntryShape {
    let file_type = match entry.file_type() {
        Ok(file_type) => file_type,
        Err(error) => return EntryShape::Unreadable(error),
    };
    if file_type.is_symlink() {
        return follow_link(&entry.path());
    }
    if file_type.is_dir() {
        #[cfg(windows)]
        if entry.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return EntryShape::DirectoryLink;
        }
        return EntryShape::Directory;
    }
    // Regular files, non-regular special files, and platforms whose readdir type
    // was unknown: resolve through no-follow metadata. Only a regular file is a
    // planable file; special kinds must not inherit `open`/copy semantics.
    match entry.metadata() {
        Ok(meta) if meta.file_type().is_symlink() => follow_link(&entry.path()),
        Ok(meta) if meta.is_dir() => {
            #[cfg(windows)]
            if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return EntryShape::DirectoryLink;
            }
            EntryShape::Directory
        }
        Ok(meta) if meta.is_file() => EntryShape::File(meta),
        Ok(_) => EntryShape::Special,
        Err(error) => EntryShape::Unreadable(error),
    }
}

fn follow_link(path: &Path) -> EntryShape {
    match fs::metadata(path) {
        Ok(meta) if meta.is_dir() => EntryShape::DirectoryLink,
        Ok(meta) if meta.is_file() => EntryShape::FileLink(meta),
        Ok(_) => EntryShape::Special,
        Err(error) => EntryShape::Unreadable(error),
    }
}

/// AE saves normal `.aep` files in its Auto-Save folder, so extension filtering
/// alone cannot distinguish them. Also hide explicitly named autosave projects.
pub fn is_project_autosave(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().as_encoded_bytes();
        let normalized: Vec<u8> = name
            .iter()
            .filter(|byte| !matches!(**byte, b' ' | b'-' | b'_'))
            .map(u8::to_ascii_lowercase)
            .collect();
        normalized == b"adobeaftereffectsautosave" || normalized == b"autosave"
    }) || path.file_name().is_some_and(|name| {
        let name = name.as_encoded_bytes().to_ascii_lowercase();
        name.ends_with(b".aep")
            && [
                b"auto-save".as_slice(),
                b"auto_save",
                b"auto save",
                b"autosave",
            ]
            .iter()
            .any(|word| name.windows(word.len()).any(|part| part == *word))
    })
}

fn visit_entry(entry: &std::fs::DirEntry, parent_relative: &Path, filters: &ScanFilters) -> Visit {
    let name = entry.file_name();
    let child_relative = parent_relative.join(&name);
    match entry_shape(entry) {
        EntryShape::Unreadable(error) => Visit::Issue(
            io_category(&error),
            format!("could not inspect entry: {error}"),
        ),
        EntryShape::DirectoryLink => Visit::Issue(
            PathCategory::Unsupported,
            "directory symlink/reparse point not followed".to_string(),
        ),
        EntryShape::Directory => Visit::Descend(entry.path(), child_relative),
        EntryShape::Special => Visit::Issue(
            PathCategory::Unsupported,
            "special file kind (FIFO, socket or device) is not a regular file; not included"
                .to_string(),
        ),
        EntryShape::File(meta) | EntryShape::FileLink(meta) => {
            if !filters.matches_name(&name) {
                return Visit::Skip;
            }
            let (stamp, issue) = stamp_from(&entry.path(), &meta);
            Visit::Keep(
                FileEntry {
                    path: entry.path(),
                    stamp,
                },
                issue,
            )
        }
    }
}

/// Builds the stamp from the metadata the walker already has. Windows identity
/// fields are absent from `DirEntry::metadata`, so the entry path is re-statted
/// there; a failed re-stat keeps the stamp without identity evidence and reports
/// the metadata failure instead of inventing an identity.
fn stamp_from(path: &Path, meta: &fs::Metadata) -> (FileStamp, Option<(PathCategory, String)>) {
    let mut issue = None;
    #[cfg(windows)]
    let stamp_meta = match fs::metadata(path) {
        Ok(fresh) => fresh,
        Err(error) => {
            issue = Some((
                io_category(&error),
                format!("could not re-stat entry for file identity: {error}"),
            ));
            meta.clone()
        }
    };
    #[cfg(not(windows))]
    let stamp_meta = {
        // Only Windows re-stats; the path parameter is unused elsewhere.
        let _ = path;
        meta.clone()
    };

    let modified = match stamp_meta.modified() {
        Ok(time) => Some(time),
        Err(_) => {
            issue = Some((
                PathCategory::Unsupported,
                "modified time is not representable; entry kept without a stamp".to_string(),
            ));
            None
        }
    };
    (
        FileStamp {
            size: stamp_meta.len(),
            modified,
            identity: FileIdentity::from_metadata(&stamp_meta),
        },
        issue,
    )
}

/// Outcome of one directory listing: entries, a cancelled read, or a per-
/// directory failure that degrades coverage instead of aborting the scan.
enum Listing {
    Entries(Vec<std::fs::DirEntry>),
    Cancelled,
    Failed(PathCategory, String),
}

fn read_sorted(directory: &Path, control: &JobControl) -> Listing {
    // Cancellation is observed before the read, after it (covering the empty
    // directory case) and periodically while entries are collected, so a
    // cancelled listing is never reported as a complete one.
    if control.checkpoint() == Checkpoint::Cancelled {
        return Listing::Cancelled;
    }
    let read = match fs::read_dir(directory) {
        Ok(read) => read,
        Err(error) => {
            return Listing::Failed(
                io_category(&error),
                format!("cannot read directory: {error}"),
            );
        }
    };
    let mut entries: Vec<std::fs::DirEntry> = Vec::new();
    for (index, entry) in read.enumerate() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                return Listing::Failed(
                    io_category(&error),
                    format!("cannot list directory contents: {error}"),
                );
            }
        };
        entries.push(entry);
        if index % CHECKPOINT_EVERY == 0 && control.checkpoint() == Checkpoint::Cancelled {
            return Listing::Cancelled;
        }
    }
    if control.checkpoint() == Checkpoint::Cancelled {
        return Listing::Cancelled;
    }
    entries.sort_by(|a, b| {
        a.file_name()
            .as_encoded_bytes()
            .cmp(b.file_name().as_encoded_bytes())
    });
    Listing::Entries(entries)
}

fn report_issue(
    issues: &mut Vec<PathIssue>,
    events: &JobEvents,
    path: &Path,
    category: PathCategory,
    message: String,
) {
    issues.push(PathIssue::new(path, category, message.clone()));
    events.diagnostic(
        JobPhase::Scanning,
        format!("{}: {} [{}]", message, path.display(), category.label()),
    );
}

/// Case-folded display name first, exact native path bytes as tie-break: the
/// fold is only an ordering aid, identity stays with the untouched bytes.
fn display_sort_key(path: &Path) -> (String, Vec<u8>) {
    let name = path.file_name().unwrap_or(path.as_os_str());
    (
        name.to_string_lossy().to_lowercase(),
        path.as_os_str().as_encoded_bytes().to_vec(),
    )
}

fn io_category(error: &std::io::Error) -> PathCategory {
    match error.kind() {
        ErrorKind::PermissionDenied => PathCategory::Permission,
        _ => PathCategory::Unavailable,
    }
}

/// Literal-only exclude plan. Unsupported wildcard patterns refuse the scan.
struct ExcludePlan {
    components: Vec<Vec<u8>>,
    full: Vec<Vec<u8>>,
}

fn build_excludes(filters: &ScanFilters, events: &JobEvents) -> PathResult<ExcludePlan> {
    let mut plan = ExcludePlan {
        components: Vec::new(),
        full: Vec::new(),
    };
    for pattern in &filters.excludes {
        let bytes = pattern.as_bytes();
        if bytes.iter().any(|byte| matches!(byte, b'*' | b'?' | b'[')) {
            return Err(PathError::new(
                PathCategory::Unsupported,
                None,
                format!(
                    "Wildcard exclusion '{pattern}' requires the backup matcher; scan refused."
                ),
            ));
        }
        if bytes.iter().all(|byte| *byte == b'/') {
            events.diagnostic(
                JobPhase::Scanning,
                format!("Exclude pattern '{pattern}' has no path content and is ignored."),
            );
            continue;
        }
        plan.components.push(fold_native_text(bytes));
        plan.full.push(exclude_full_form(bytes));
    }
    Ok(plan)
}

/// Folded bytes, with backslashes treated as separators on Windows only (on
/// POSIX a backslash is an ordinary name byte).
fn exclude_full_form(bytes: &[u8]) -> Vec<u8> {
    let lowered = fold_native_text(bytes);
    #[cfg(windows)]
    {
        return lowered
            .iter()
            .map(|byte| if *byte == b'\\' { b'/' } else { *byte })
            .collect();
    }
    #[cfg(not(windows))]
    lowered
}

fn is_excluded(relative: &Path, plan: &ExcludePlan) -> bool {
    if plan.components.is_empty() && plan.full.is_empty() {
        return false;
    }
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        let lowered = fold_native_text(name.as_encoded_bytes());
        if plan.components.iter().any(|pattern| *pattern == lowered) {
            return true;
        }
    }
    let lowered = exclude_full_form(relative.as_os_str().as_encoded_bytes());
    plan.full.iter().any(|pattern| *pattern == lowered)
}

fn name_ends_with_case_insensitive(bytes: &[u8], suffix: &[u8]) -> bool {
    if suffix.is_empty() {
        return false;
    }
    let folded_suffix = fold_native_text(suffix);
    fold_native_text(bytes).ends_with(&folded_suffix)
}

fn last_extension_matches(bytes: &[u8], suffixes: &[String]) -> bool {
    let Some(dot) = bytes.iter().rposition(|byte| *byte == b'.') else {
        return false;
    };
    // `splitext` parity: leading dots belong to the stem, so `.nk` and `..nk`
    // have no extension. At least one non-dot byte must precede the final dot.
    if dot == 0 || bytes[..dot].iter().all(|byte| *byte == b'.') {
        return false;
    }
    let extension = &bytes[dot + 1..];
    let folded_extension = fold_native_text(extension);
    suffixes.iter().any(|suffix| {
        let suffix = suffix.as_bytes();
        let suffix = suffix.strip_prefix(b".").unwrap_or(suffix);
        !suffix.is_empty() && folded_extension == fold_native_text(suffix)
    })
}
