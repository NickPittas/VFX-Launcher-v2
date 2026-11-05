//! Native path identity, containment validation and read-only root inspection.
//!
//! Packet F1 of the Rust migration (`CONTRACTS.md` §1, `PLAN.md` packet F1).
//! Runtime filesystem values are [`PathBuf`], never display strings. Nothing in
//! this module canonicalizes a path or touches the filesystem except the two
//! explicitly stat-based helpers ([`inspect_root`], [`evidence_same_file`]) and
//! the existing-ancestor walk inside [`validate_destination`].
//!
//! Identity model: [`path_key`] computes a private, comparable lexical key from
//! an absolute-or-relative path against an explicit absolute base (the caller's
//! anchor replaces the process working directory). POSIX preserves case,
//! backslashes and non-UTF-8 bytes exactly; Windows normalizes separators,
//! drops `.`/resolves `..` lexically and folds case like legacy `normcase`.
//! No canonicalization is performed and none is required for offline catalog
//! lookup.
//!
//! Deliberate limits, not to be papered over by callers:
//!
//! - Lexical equality is not proof of the same file, and lexical inequality is
//!   not proof of different files (hardlinks, bind mounts, symlink spellings).
//!   [`relation`] classifies lexical shape only; [`evidence_same_file`] returns
//!   `Some` only when the OS gives definite identity evidence, `None` otherwise.
//!   Hardlinks are never "universally detected".
//! - Windows case folding is ASCII-only on the native encoded bytes. Legacy
//!   `normcase` lowercased full Unicode; this key deliberately avoids
//!   Unicode-aware folding so non-UTF-8/WTF-8 bytes keep byte identity.
//! - Verbatim (`\\?\`) Windows spellings are anchored as std parses them; no
//!   Win32-normalization equivalence is claimed for them.
//! - Drive-relative Windows inputs (`C:foo`) resolve against per-drive CWD
//!   state, which this module must not invent; they are rejected.
//! - [`validate_destination`] is snapshot validation only. It rejects traversal,
//!   absolute and foreign drive/UNC spellings, NUL and invalid native names,
//!   then walks existing ancestors and fails closed (category [`PathCategory::Escape`])
//!   on any symlink/reparse point in the chain, and on any stat error that
//!   prevents proving confinement. Execution must re-check containment with
//!   no-follow platform facilities at write time; this module makes no claim
//!   about races between validation and execution.
//! - Foreign-platform spellings are retained unresolved by [`retain_imported`];
//!   no mount/drive mapping is ever invented. A mounted Linux share and a
//!   Windows UNC path are equal only via an explicit user-supplied mapping
//!   (a later packet's concern).
//! - Portability: path text exported to JSON must come from [`utf8_spelling`]
//!   and be reported unsupported when it is `None`. The only lossy rendering in
//!   this module is diagnostic display (`Path`'s own `Display` inside error
//!   messages); originals are always preserved as bytes.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

// Trait methods used on Windows metadata (`file_attributes`,
// `volume_serial_number`, `file_index`). Unix metadata extension traits are
// imported inside the functions that need them.
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

/// Result alias for module-owned path errors.
pub type PathResult<T> = Result<T, PathError>;

/// Stable path-error/issue categories (module-owned; a later integration may
/// map these into the shared app error without renaming them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathCategory {
    /// Caller supplied something unusable (empty, absolute where relative is
    /// required, `..`, NUL, drive-relative, non-absolute base).
    InvalidInput,
    /// A Windows drive/UNC spelling was used where it cannot be honored.
    ForeignPlatform,
    /// The root does not exist or is not a directory.
    MissingRoot,
    /// The path could not be inspected (I/O failure, vanished, network root
    /// unreachable). Never treated as proof of absence.
    Unavailable,
    /// Inspection was refused by permissions.
    Permission,
    /// Containment could not be proven; the operation was refused.
    Escape,
    /// The path is understandable but outside what this build handles
    /// (skipped directory symlinks, unrepresentable metadata, …).
    Unsupported,
}

impl PathCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::InvalidInput => "invalid path input",
            Self::ForeignPlatform => "foreign platform path",
            Self::MissingRoot => "root missing",
            Self::Unavailable => "unavailable",
            Self::Permission => "permission denied",
            Self::Escape => "escape refused",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Module-owned path error with a stable category, the affected path and a
/// readable cause. Category values are distinct per `CONTRACTS.md` §0.
#[derive(Clone, Debug)]
pub struct PathError {
    category: PathCategory,
    path: Option<PathBuf>,
    message: String,
}

impl PathError {
    pub fn new(category: PathCategory, path: Option<&Path>, message: impl Into<String>) -> Self {
        Self {
            category,
            path: path.map(Path::to_path_buf),
            message: message.into(),
        }
    }

    pub fn category(&self) -> PathCategory {
        self.category
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.path {
            Some(path) => {
                write!(
                    f,
                    "{} ({}): {}",
                    self.message,
                    self.category.label(),
                    path.display()
                )
            }
            None => write!(f, "{} ({})", self.message, self.category.label()),
        }
    }
}

impl std::error::Error for PathError {}

/// A non-fatal path problem observed while reading the filesystem: the original
/// path, a stable category and a readable cause. Discovery collects these; it
/// never acts on them beyond reporting.
#[derive(Clone, Debug)]
pub struct PathIssue {
    pub path: PathBuf,
    pub category: PathCategory,
    pub message: String,
}

impl PathIssue {
    pub fn new(path: &Path, category: PathCategory, message: impl Into<String>) -> Self {
        Self {
            path: path.to_path_buf(),
            category,
            message: message.into(),
        }
    }
}

/// Private, comparable native lexical identity of one anchored path.
///
/// Construction goes through [`path_key`]; the normalized components are
/// deliberately opaque. POSIX components are the exact encoded bytes (case and
/// backslashes significant); Windows components are ASCII-case-folded encoded
/// bytes with the drive/UNC prefix preserved. Derived ordering is over the
/// component sequence, so it is total and stable across runs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PathKey {
    components: Vec<Box<[u8]>>,
}

impl PathKey {
    /// Schema-1 catalog key: length-prefixed native components. Store the
    /// platform tag alongside it; display spellings are never identity keys.
    pub fn storage_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        for component in &self.components {
            bytes.extend_from_slice(&(component.len() as u64).to_le_bytes());
            bytes.extend_from_slice(component);
        }
        bytes
    }
}

/// Lexical shape relation between two keys. These are claims about normalized
/// spellings only — never about whether the filesystem holds the same file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathRelation {
    /// Identical normalized spellings.
    Same,
    /// `a` lies lexically inside `b` (`b` is an ancestor-or-self spelling of `a`).
    FirstInsideSecond,
    /// `b` lies lexically inside `a`.
    SecondInsideFirst,
    /// Neither spelling is a prefix of the other. Still not proof that the two
    /// paths cannot reach the same directory through links or mounts.
    Disjoint,
}

/// Observable filesystem identity of a root directory, from one stat snapshot.
/// POSIX: device + inode. Windows: volume serial + file index. Fields stay
/// `None` on platforms where they do not apply. This is continuity evidence,
/// not a reliable SMB connection ID: a disconnected share can remount to the
/// same values (or an empty mountpoint can keep the mountpoint's own values).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RootIdentity {
    unix: Option<(u64, u64)>,
    windows: Option<(u64, u64)>,
}

/// Observable identity of one regular file, from one stat snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    unix: Option<(u64, u64)>,
    windows: Option<(u64, u64)>,
}

/// A path spelling imported from legacy data together with the platform its
/// spelling indicates. The raw spelling is always retained verbatim; `native`
/// is `Some` only when the spelling belongs to the current platform. Unknown or
/// foreign spellings stay unresolved rather than being converted by guesswork.
#[derive(Clone, Debug)]
pub struct ImportedPath {
    pub raw: OsString,
    pub tag: PlatformTag,
    pub native: Option<PathBuf>,
}

/// Which platform a spelling looks like, judged only on distinctive markers
/// (UNC prefix, drive prefix, POSIX root). Anything else is [`PlatformTag::Unknown`]
/// — a POSIX name may legally contain backslashes, so backslash presence alone
/// proves nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformTag {
    Posix,
    Windows,
    Unknown,
}

/// Returns the comparable lexical key for `path`, with relative inputs resolved
/// against the explicit absolute `base` (never the process working directory).
/// Absolute inputs ignore `base`. Fails closed on drive-relative Windows
/// spellings and on a non-absolute base.
pub fn path_key(path: &Path, base: &Path) -> PathResult<PathKey> {
    let fold = anchored(path, base)?;
    let components = fold
        .kept
        .iter()
        .map(|part| normalized_bytes(part))
        .collect();
    Ok(PathKey { components })
}

/// The normalized native spelling (absolute, `.` removed, `..` resolved
/// lexically, original case and bytes preserved) for the same anchoring rules
/// as [`path_key`]. No separators are rewritten on POSIX; on Windows the
/// rebuilt spelling uses native separators because components are parsed from
/// a [`Path`]. This is a spelling, not a resolved filesystem identity.
pub fn normalize_lexical(path: &Path, base: &Path) -> PathResult<PathBuf> {
    let fold = anchored(path, base)?;
    Ok(join_kept(&fold.kept))
}

/// Lexical shape relation between two already-computed keys.
pub fn relation(a: &PathKey, b: &PathKey) -> PathRelation {
    if a == b {
        return PathRelation::Same;
    }
    let (shorter, longer) = if a.components.len() <= b.components.len() {
        (a, b)
    } else {
        (b, a)
    };
    if longer.components.starts_with(&shorter.components) {
        if shorter == a {
            PathRelation::SecondInsideFirst
        } else {
            PathRelation::FirstInsideSecond
        }
    } else {
        PathRelation::Disjoint
    }
}

/// Definite same-file evidence from one stat snapshot of both spellings, or
/// `None` when either stat fails or the platform offers no identity fields.
/// `Some(false)` means the two snapshots provably describe different files at
/// the same instant; `None` means "no evidence", never "different".
pub fn evidence_same_file(a: &Path, b: &Path) -> Option<bool> {
    let a_meta = fs::metadata(a).ok()?;
    let b_meta = fs::metadata(b).ok()?;
    Some(FileIdentity::from_metadata(&a_meta).same_file(&FileIdentity::from_metadata(&b_meta)))
}

/// Validates a destination the caller wants to create under an existing
/// absolute `root`, and returns the destination spelling `root.join(relative)`.
///
/// Rejected: absolute `relative` values, `..`/`.` components, NUL, foreign
/// drive/UNC spellings, invalid native names, a missing or non-directory root,
/// and any symlink/reparse point among the *existing* ancestors of the target
/// (including the final component if it already exists as a link). Any stat
/// error along the chain refuses the operation: confinement that cannot be
/// proven is treated as an escape. Components that do not exist yet cannot be
/// checked and are expected to be re-checked by execution with no-follow
/// facilities; this is snapshot validation only and makes no race-safety claim.
///
/// The root itself may be a symlink and is followed deliberately: it is the
/// user's explicit anchor, and everything below it is confined to where the
/// anchor resolves.
pub fn validate_destination(root: &Path, relative: &Path) -> PathResult<PathBuf> {
    if !root.is_absolute() {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            Some(root),
            "destination root must be an absolute path",
        ));
    }
    validate_relative_spelling(relative)?;
    inspect_root(root)?;
    check_chain_confinement(root, relative)?;
    Ok(root.join(relative))
}

/// Stats a root directory (following a symlink root deliberately, per
/// [`validate_destination`]) and returns its observable identity. `NotFound`
/// maps to [`PathCategory::MissingRoot`]; every other failure maps to
/// [`PathCategory::Permission`] or [`PathCategory::Unavailable`] and is never
/// read as "the files are gone".
pub fn inspect_root(path: &Path) -> PathResult<RootIdentity> {
    let meta = fs::metadata(path).map_err(|error| {
        let category = match error.kind() {
            ErrorKind::NotFound => PathCategory::MissingRoot,
            ErrorKind::PermissionDenied => PathCategory::Permission,
            _ => PathCategory::Unavailable,
        };
        let message = match error.kind() {
            ErrorKind::NotFound => "root does not exist".to_string(),
            ErrorKind::PermissionDenied => "root cannot be inspected".to_string(),
            _ => format!("root unavailable: {error}"),
        };
        PathError::new(category, Some(path), message)
    })?;
    if !meta.is_dir() {
        return Err(PathError::new(
            PathCategory::MissingRoot,
            Some(path),
            "root is not a directory",
        ));
    }
    Ok(RootIdentity::from_metadata(&meta))
}

/// The UTF-8 spelling of a path for portable JSON export, or `None` when the
/// native bytes are not UTF-8. Callers must report non-UTF-8 paths as
/// unsupported for portable output; they must never round-trip a lossy
/// replacement back into a [`PathBuf`].
pub fn utf8_spelling(path: &Path) -> Option<&str> {
    path.to_str()
}

/// Retains an imported spelling with its platform classification. `native` is
/// populated only when the spelling belongs to the current platform; otherwise
/// the value stays unresolved until an explicit user-supplied mapping exists.
pub fn retain_imported(raw: &OsStr) -> ImportedPath {
    let tag = detect_platform_tag(raw.as_encoded_bytes());
    let native = match tag {
        #[cfg(unix)]
        PlatformTag::Posix => Some(PathBuf::from(raw)),
        #[cfg(windows)]
        PlatformTag::Windows => Some(PathBuf::from(raw)),
        _ => None,
    };
    ImportedPath {
        raw: raw.to_os_string(),
        tag,
        native,
    }
}

/// Classifies a spelling by distinctive markers only. Leading `//` is left
/// Unknown: POSIX treats it as an implementation-defined root, not a UNC.
fn detect_platform_tag(bytes: &[u8]) -> PlatformTag {
    if bytes.starts_with(b"\\\\") {
        return PlatformTag::Windows;
    }
    if bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes.len() == 2 || bytes[2] == b'/' || bytes[2] == b'\\')
    {
        return PlatformTag::Windows;
    }
    if bytes.first() == Some(&b'/') {
        return PlatformTag::Posix;
    }
    PlatformTag::Unknown
}

struct Fold {
    kept: Vec<OsString>,
    /// Number of prefix/root anchors at the front of `kept`; `..` never pops
    /// below them (the parent of a root is the root, as in `normpath`).
    anchors: usize,
}

fn fold_components<'a>(fold: &mut Fold, components: impl Iterator<Item = Component<'a>>) {
    for component in components {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                fold.kept.push(component.as_os_str().to_os_string());
                fold.anchors += 1;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if fold.kept.len() > fold.anchors {
                    fold.kept.pop();
                }
            }
            Component::Normal(name) => fold.kept.push(name.to_os_string()),
        }
    }
}

/// Folds `path` (absolute alone, otherwise joined over the absolute `base`)
/// into normalized kept components, mirroring `normpath(abspath)` semantics
/// without touching the working directory.
fn anchored(path: &Path, base: &Path) -> PathResult<Fold> {
    let mut fold = Fold {
        kept: Vec::new(),
        anchors: 0,
    };
    if path.is_absolute() {
        fold_components(&mut fold, path.components());
        return Ok(fold);
    }
    #[cfg(windows)]
    if matches!(path.components().next(), Some(Component::Prefix(_))) {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            Some(path),
            "drive-relative Windows path has no root; use an absolute spelling",
        ));
    }
    if !base.is_absolute() {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            Some(base),
            "identity base must be an absolute path",
        ));
    }
    fold_components(&mut fold, base.components());
    fold_components(&mut fold, path.components());
    Ok(fold)
}

fn normalized_bytes(part: &OsStr) -> Box<[u8]> {
    let bytes = part.as_encoded_bytes();
    #[cfg(windows)]
    {
        return bytes.to_ascii_lowercase().into_boxed_slice();
    }
    #[cfg(not(windows))]
    {
        Box::from(bytes)
    }
}

/// Rebuilds a [`PathBuf`] from kept components without [`PathBuf::push`]
/// separator heuristics: a separator is inserted between two parts exactly
/// when neither side already carries one (prefix `C:` + root `\`, UNC prefix
/// + root, root + name, name + name).
fn join_kept(kept: &[OsString]) -> PathBuf {
    const POSIX_SEP: &str = "/";
    const WINDOWS_SEP: &str = "\\";
    let separator: &str = if cfg!(windows) {
        WINDOWS_SEP
    } else {
        POSIX_SEP
    };
    let separator_byte = separator.as_bytes()[0];
    let mut rebuilt = OsString::new();
    let mut previous_ends_with_separator = false;
    for (index, part) in kept.iter().enumerate() {
        let bytes = part.as_encoded_bytes();
        if index > 0 && !previous_ends_with_separator && bytes.first() != Some(&separator_byte) {
            rebuilt.push(OsStr::new(separator));
        }
        rebuilt.push(part);
        previous_ends_with_separator = bytes.last() == Some(&separator_byte);
    }
    PathBuf::from(rebuilt)
}

/// Lexical rejection of unusable relative destination spellings, before any
/// filesystem access. Only the first component is checked for foreign Windows
/// drive/UNC spelling on POSIX: a later POSIX component named like `C:\x` is a
/// legal POSIX name and is preserved as such.
fn validate_relative_spelling(relative: &Path) -> PathResult<()> {
    if relative.as_os_str().is_empty() {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            Some(relative),
            "destination path is empty",
        ));
    }
    if relative.is_absolute() {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            Some(relative),
            "destination path must be relative to the destination root",
        ));
    }
    if relative.as_os_str().as_encoded_bytes().contains(&0) {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            Some(relative),
            "destination path contains a NUL byte",
        ));
    }
    for (index, component) in relative.components().enumerate() {
        match component {
            Component::Prefix(_) => {
                return Err(PathError::new(
                    PathCategory::ForeignPlatform,
                    Some(relative),
                    "drive or UNC prefixes are not allowed in a relative destination",
                ));
            }
            Component::RootDir => {
                return Err(PathError::new(
                    PathCategory::InvalidInput,
                    Some(relative),
                    "destination path must be relative to the destination root",
                ));
            }
            Component::CurDir => {
                return Err(PathError::new(
                    PathCategory::InvalidInput,
                    Some(relative),
                    "'.' components are not accepted in destinations",
                ));
            }
            Component::ParentDir => {
                return Err(PathError::new(
                    PathCategory::InvalidInput,
                    Some(relative),
                    "'..' components are not accepted in destinations",
                ));
            }
            Component::Normal(name) => {
                #[cfg(unix)]
                if index == 0 && looks_like_windows_foreign(name.as_encoded_bytes()) {
                    return Err(PathError::new(
                        PathCategory::ForeignPlatform,
                        Some(relative),
                        "Windows drive/UNC spelling cannot be a destination component here",
                    ));
                }
                #[cfg(windows)]
                validate_windows_name(name)?;
                let _ = index;
            }
        }
    }
    Ok(())
}

/// POSIX only: a first component that parses as a Windows drive or UNC
/// spelling is rejected as foreign rather than silently becoming a file whose
/// name happens to contain a colon or backslashes.
#[cfg(unix)]
fn looks_like_windows_foreign(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\\\\")
        || (bytes.len() >= 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes.len() == 2 || bytes[2] == b'/' || bytes[2] == b'\\'))
}

/// Windows component name rules: reserved characters, control bytes, trailing
/// dot/space (Win32 strips them, so the created name would differ), and
/// reserved device names compared on the stem before the first dot.
#[cfg(windows)]
fn validate_windows_name(name: &OsStr) -> PathResult<()> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let bytes = name.as_encoded_bytes();
    for byte in bytes {
        match byte {
            b'<' | b'>' | b':' | b'"' | b'|' | b'?' | b'*' => {
                return Err(PathError::new(
                    PathCategory::InvalidInput,
                    None,
                    format!(
                        "character '{}' is not allowed in a Windows file name",
                        *byte as char
                    ),
                ));
            }
            0x01..=0x1F => {
                return Err(PathError::new(
                    PathCategory::InvalidInput,
                    None,
                    "control characters are not allowed in a Windows file name",
                ));
            }
            _ => {}
        }
    }
    if matches!(bytes.last(), Some(b'.') | Some(b' ')) {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            None,
            "a Windows name cannot end with a dot or a space (Win32 would strip it)",
        ));
    }
    let stem = &bytes[..bytes
        .iter()
        .position(|byte| *byte == b'.')
        .unwrap_or(bytes.len())];
    if RESERVED
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved.as_bytes()))
    {
        return Err(PathError::new(
            PathCategory::InvalidInput,
            None,
            "reserved Windows device names cannot be used as file or folder names",
        ));
    }
    Ok(())
}

/// Walks the existing prefix of `root/relative` and refuses confinement it
/// cannot prove: a symlink or reparse point anywhere in the chain (including
/// the final component) is an escape; any non-`NotFound` stat error is an
/// unprovable confinement. Absent components are fine — they do not exist yet,
/// and execution re-checks the whole chain.
fn check_chain_confinement(root: &Path, relative: &Path) -> PathResult<()> {
    #[cfg(windows)]
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            continue;
        };
        current.push(name);
        match fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(PathError::new(
                        PathCategory::Escape,
                        Some(&current),
                        "a symlink sits in the destination chain; refusing without resolving it",
                    ));
                }
                #[cfg(windows)]
                if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    return Err(PathError::new(
                        PathCategory::Escape,
                        Some(&current),
                        "a reparse point (junction/mount/symlink) sits in the destination chain",
                    ));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                let category = match error.kind() {
                    ErrorKind::PermissionDenied => PathCategory::Permission,
                    _ => PathCategory::Unavailable,
                };
                return Err(PathError::new(
                    category,
                    Some(&current),
                    "destination confinement cannot be proven; refusing to continue",
                ));
            }
        }
    }
    Ok(())
}

impl RootIdentity {
    #[cfg(unix)]
    fn from_metadata(meta: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            unix: Some((meta.dev(), meta.ino())),
            windows: None,
        }
    }

    #[cfg(windows)]
    fn from_metadata(meta: &fs::Metadata) -> Self {
        Self {
            unix: None,
            windows: meta
                .volume_serial_number()
                .zip(meta.file_index())
                .map(|(volume, index)| (u64::from(volume), index)),
        }
    }

    /// True only with positive matching evidence: at least one evidence kind
    /// must be present on both sides, and every shared kind must agree.
    pub fn same_identity(&self, other: &RootIdentity) -> bool {
        identity_verdict(self.unix, other.unix, self.windows, other.windows)
    }
}

impl FileIdentity {
    #[cfg(unix)]
    pub(crate) fn from_metadata(meta: &fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            unix: Some((meta.dev(), meta.ino())),
            windows: None,
        }
    }

    #[cfg(windows)]
    pub(crate) fn from_metadata(meta: &fs::Metadata) -> Self {
        Self {
            unix: None,
            windows: meta
                .volume_serial_number()
                .zip(meta.file_index())
                .map(|(volume, index)| (u64::from(volume), index)),
        }
    }

    /// Same contract as [`RootIdentity::same_identity`]: definite agreement on
    /// shared evidence, no claim when evidence is missing or disjoint.
    pub fn same_file(&self, other: &FileIdentity) -> bool {
        identity_verdict(self.unix, other.unix, self.windows, other.windows)
    }
}

/// `Some(agreement)` per evidence kind present on both sides, `None` when that
/// kind is missing on either side. Two identities with no shared evidence kind
/// cannot be compared and report `false` — never a positive claim.
fn identity_verdict(
    a_unix: Option<(u64, u64)>,
    b_unix: Option<(u64, u64)>,
    a_windows: Option<(u64, u64)>,
    b_windows: Option<(u64, u64)>,
) -> bool {
    match (
        evidence_pair(a_unix, b_unix),
        evidence_pair(a_windows, b_windows),
    ) {
        (Some(same), Some(other)) => same && other,
        (Some(same), None) => same,
        (None, Some(same)) => same,
        (None, None) => false,
    }
}

fn evidence_pair(a: Option<(u64, u64)>, b: Option<(u64, u64)>) -> Option<bool> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x == y),
        _ => None,
    }
}
