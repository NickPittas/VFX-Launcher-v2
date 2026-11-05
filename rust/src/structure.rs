//! S1: structure planning — templates, naming presets, clients, sequence/shot
//! trees and validated operation plans for new projects and add-to-existing
//! shots.
//!
//! Pure planning: nothing here writes the filesystem. The read-only inspection
//! it does perform (template folders, project roots, numbering listings)
//! mirrors the original sources' own planning-time checks; execution happens
//! only through [`crate::transfers`] plans of `CreateDirectory`/`CreateFile`
//! actions, validated by that module before any run.
//!
//! Ported from the reference sources (read-only `gitclones/` clones plus the
//! legacy `ui_slick/` panel in this repository):
//! - `FolderStructurer/common/backend_api.py`: template replication with
//!   `sequences.txt`/`shots.txt` markers, `template.<ext>` per-project rename,
//!   Sphere/KENT folder labels, strict numbering (`compute_next_project_number`),
//!   root naming (`build_project_root_name`), marker expansion and the direct
//!   add-to-existing flow.
//! - `FolderStructurer/widgets/shot_sequence_widgets.py`,
//!   `tabs/add_to_existing_project_tab.py` and `main.py`: explicitly named
//!   sequences, rename, shots under a chosen sequence, independent
//!   (sequence-less) shots and multi-target additions with the "comp" path
//!   rule for `project/`+`render/` and Nuke scripts.
//! - `ui_slick/new_project_panel.py` with `ui_slick/structure.txt`: the
//!   Launcher dated preset (`<code>_<client>_<name>_<YYYY-MM-DD>`, loose code
//!   regex `^\d+_.*\d{4}-\d{2}-\d{2}`, manifest directory preset, underscores
//!   rejected in client/project name).
//!
//! Nuke script *contents* belong to the S2 `nuke.rs` packet, so planning takes
//! a caller-provided [`ScriptContentSource`]; this module owns script file
//! names and locations exactly as the source builds them.
//!
//! Deliberate deviations, each contract-mandated (CONTRACTS.md §4/§6):
//! - an unreadable numbering root blocks auto-numbering instead of suggesting
//!   `00001`; a missing root still yields `00001`, as in the source;
//! - duplicate sequence/shot names within one addition are rejected instead of
//!   silently collapsed by `makedirs(exist_ok=True)`;
//! - existing Nuke scripts are never overwritten on add-to-existing (the
//!   source's plain `open(..., "w")` becomes a visible `Skip` conflict);
//! - template-marker add-to-existing targets every existing marker directory
//!   of the chosen template (the source hardcoded two base folders per layout);
//! - a missing parent project root refuses planning instead of implicitly
//!   creating the parent chain like `makedirs` did.
//!
//! Client-list persistence keeps the config-module conventions
//! (`schema_version` envelope) but file wiring belongs to the config/import
//! packets; this module owns the typed value and its list semantics.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::paths;
use crate::transfers::{
    ConflictPolicy, OperationPlan, PlanOrigin, PlannedAction, StoredPath, TransferCategory,
    TransferError,
};

/// Marker file that expands one folder per named sequence (case-insensitive
/// ASCII name match, like the source's `lower()` comparison).
pub const SEQUENCE_MARKER_FILE: &str = "sequences.txt";
/// Marker file that expands shots under its directory.
pub const SHOT_MARKER_FILE: &str = "shots.txt";
/// Working directory created beside every planned shot folder.
pub const SHOT_PROJECT_DIR: &str = "project";
/// Render directory created beside every planned shot folder.
pub const SHOT_RENDER_DIR: &str = "render";
/// Fixed script suffix; the file name is `<sequence?>_<shot>_comp_v001.nk`.
pub const SCRIPT_SUFFIX: &str = "_comp_v001.nk";
/// Sequence/shot line edits were capped at 25 characters by the source widgets.
pub const MAX_SEQUENCE_OR_SHOT_CHARS: usize = 25;
/// Envelope version of the clients resource, following the config convention.
pub const CLIENTS_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// Errors and small validators
// ---------------------------------------------------------------------------

fn invalid(message: impl Into<String>) -> TransferError {
    TransferError::new(TransferCategory::InvalidInput, None, message)
}

/// One plain, path-safe component: trimmed, non-empty, no separators,
/// traversal, NUL or control characters; legitimate Unicode is preserved.
fn validate_component(raw: &str, what: &str) -> Result<String, TransferError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid(format!("{what} cannot be empty")));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(invalid(format!(
            "{what} {trimmed:?} is a relative-path component"
        )));
    }
    let rejected = trimmed
        .chars()
        .find(|&c| c == '/' || c == '\\' || c == '\0' || c.is_control());
    if let Some(found) = rejected {
        return Err(invalid(format!(
            "{what} cannot contain {found:?}; names are single path components"
        )));
    }
    Ok(trimmed.to_string())
}

/// Component validation plus the 25-character cap the source widgets enforced.
fn validate_sequence_or_shot(raw: &str, what: &str) -> Result<String, TransferError> {
    let name = validate_component(raw, what)?;
    if name.chars().count() > MAX_SEQUENCE_OR_SHOT_CHARS {
        return Err(invalid(format!(
            "{what} is longer than {MAX_SEQUENCE_OR_SHOT_CHARS} characters"
        )));
    }
    Ok(name)
}

/// `Option` flavour: blank input means "not provided".
fn validate_optional(raw: &str, what: &str) -> Result<Option<String>, TransferError> {
    if raw.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(validate_component(raw, what)?))
}

fn validate_optional_digits(raw: &str, what: &str) -> Result<Option<String>, TransferError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if !trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid(format!("{what} must contain digits only")));
    }
    Ok(Some(trimmed.to_string()))
}

/// Validates the date the caller captured once for preview and execution
/// ("preview captures the chosen date once"). Strict `YYYY-MM-DD` shape plus a
/// real calendar date, so a plan cannot encode `2026-02-31`.
fn validate_captured_date(raw: &str) -> Result<String, TransferError> {
    let bytes = raw.as_bytes();
    let shape = bytes.len() == 10
        && bytes.iter().enumerate().all(|(index, &byte)| match index {
            4 | 7 => byte == b'-',
            _ => byte.is_ascii_digit(),
        });
    if !shape {
        return Err(invalid(format!(
            "project date {raw:?} must be YYYY-MM-DD; capture it once so preview and execution agree"
        )));
    }
    let part = |range: std::ops::Range<usize>, what: &str| -> Result<u32, TransferError> {
        raw[range]
            .parse::<u32>()
            .map_err(|_| invalid(format!("project date {raw:?} has an invalid {what}")))
    };
    let year = part(0..4, "year")? as i32;
    let month = part(5..7, "month")?;
    let day = part(8..10, "day")?;
    if month == 0 || month > 12 || day == 0 || day > days_in_month(year, month) {
        return Err(invalid(format!(
            "project date {raw:?} is not a real calendar date"
        )));
    }
    Ok(raw.to_string())
}

fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        // February with the standard leap rule.
        _ if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        _ => 28,
    }
}

// ---------------------------------------------------------------------------
// Naming presets and project root names
// ---------------------------------------------------------------------------

/// Which naming convention builds the project root folder name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NamingPreset {
    /// Launcher: required numeric code and client, no underscores in
    /// client/name (`ui_slick/new_project_panel.py`).
    LauncherDated,
    /// FolderStructurer: optional number and client
    /// (`backend_api.build_project_root_name`).
    FolderStructurer,
}

impl NamingPreset {
    pub fn label(self) -> &'static str {
        match self {
            Self::LauncherDated => "Launcher (dated)",
            Self::FolderStructurer => "FolderStructurer",
        }
    }
}

/// Builds the base project root name: `ProjectNumber_[ClientName_]ProjectName`
/// (FolderStructurer) or the required `Code_Client_Name` (Launcher). The date
/// suffix is appended by [`project_root_name_with_date`].
pub fn build_project_root_name(
    preset: NamingPreset,
    number: Option<&str>,
    client: Option<&str>,
    name: &str,
) -> Result<String, TransferError> {
    let name = validate_component(name, "project name")?;
    let number = number.map_or(Ok(None), |raw| {
        validate_optional_digits(raw, "project number")
    })?;
    let client = client.map_or(Ok(None), |raw| validate_optional(raw, "client name"))?;
    match preset {
        NamingPreset::LauncherDated => {
            // Guards instead of moves so both options stay usable below.
            if number.is_none() {
                return Err(invalid("the Launcher preset requires a project code"));
            }
            if client.is_none() {
                return Err(invalid("the Launcher preset requires a client"));
            }
            let client_value = client.as_deref().unwrap_or_default();
            for (what, value) in [("client", client_value), ("project name", name.as_str())] {
                if value.contains('_') {
                    return Err(invalid(format!(
                        "the Launcher preset rejects underscores in the {what} (used as separator)"
                    )));
                }
            }
        }
        NamingPreset::FolderStructurer => {}
    }
    let mut parts: Vec<&str> = Vec::with_capacity(3);
    if let Some(value) = number.as_deref() {
        parts.push(value);
    }
    if let Some(value) = client.as_deref() {
        parts.push(value);
    }
    parts.push(&name);
    Ok(parts.join("_"))
}

/// Base name plus the once-captured date, e.g. `25051_Picky_Show_2026-11-06`.
pub fn project_root_name_with_date(
    preset: NamingPreset,
    number: Option<&str>,
    client: Option<&str>,
    name: &str,
    date: &str,
) -> Result<String, TransferError> {
    let date = validate_captured_date(date)?;
    Ok(format!(
        "{}_{date}",
        build_project_root_name(preset, number, client, name)?
    ))
}

// ---------------------------------------------------------------------------
// Auto numbering
// ---------------------------------------------------------------------------

fn immediate_directory_names(root: &Path) -> Result<Vec<String>, TransferError> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        // The source treated a missing destination as empty; keep that.
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            // Contract override: an unreadable root must not suggest 00001.
            return Err(TransferError::new(
                TransferCategory::Unavailable,
                Some(root),
                format!("project root cannot be listed for numbering: {error}"),
            ));
        }
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            TransferError::new(
                TransferCategory::Unavailable,
                Some(root),
                format!("project root listing failed: {error}"),
            )
        })?;
        // Follows symlinks like Python's `entry.is_dir()`; unreadable entries
        // are skipped exactly like the source's error-swallowing loop.
        let is_dir = fs::metadata(entry.path())
            .map(|meta| meta.is_dir())
            .unwrap_or(false);
        if is_dir {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    Ok(names)
}

/// `^(\d+)_.*\d{4}-\d{2}-\d{2}` from `new_project_panel.NAME_RE`, hand-parsed
/// because this crate has no regex dependency. Returns the code value.
fn loose_launcher_code(name: &str) -> Option<u64> {
    let (digits, rest) = name.split_once('_')?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let value = parse_digit_prefix(digits)?;
    contains_date_token(rest).then_some(value)
}

fn contains_date_token(text: &str) -> bool {
    let bytes = text.as_bytes();
    if bytes.len() < 10 {
        return false;
    }
    bytes.windows(10).any(|token| {
        token[4] == b'-'
            && token[7] == b'-'
            && token
                .iter()
                .enumerate()
                .all(|(index, &byte)| index == 4 || index == 7 || byte.is_ascii_digit())
    })
}

fn strict_number_prefix(name: &str) -> Option<(u64, usize)> {
    let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 5 || !name[digits.len()..].starts_with('_') {
        return None;
    }
    Some((parse_digit_prefix(&digits)?, digits.len()))
}

/// Big-endian digit fold; absurdly long numbers are skipped rather than
/// wrapped (the source used unbounded Python ints).
fn parse_digit_prefix(digits: &str) -> Option<u64> {
    digits.bytes().try_fold(0u64, |value, byte| {
        value
            .checked_mul(10)
            .and_then(|shifted| shifted.checked_add(u64::from(byte - b'0')))
    })
}

/// Next Launcher code: highest `^\d+_.*\d{4}-\d{2}-\d{2}` immediate directory
/// plus one, zero-padded to 5 digits (`new_project_panel._next_code`).
pub fn next_launcher_code(root: &Path) -> Result<String, TransferError> {
    let mut best: u64 = 0;
    for name in immediate_directory_names(root)? {
        if let Some(value) = loose_launcher_code(&name) {
            best = best.max(value);
        }
    }
    Ok(format!("{:0>5}", best.saturating_add(1)))
}

/// Next FolderStructurer number: highest `^\d{5,}_` immediate directory plus
/// one, zero-padded to at least 5 digits and the width of the existing maximum
/// (`backend_api.compute_next_project_number`).
pub fn next_folder_structurer_number(root: &Path) -> Result<String, TransferError> {
    let mut best: u64 = 0;
    let mut width = 5usize;
    for name in immediate_directory_names(root)? {
        if let Some((value, digits)) = strict_number_prefix(&name) {
            if value > best {
                best = value;
                width = width.max(digits);
            }
        }
    }
    Ok(format!("{:0width$}", best.saturating_add(1), width = width))
}

fn auto_number(parent: &Path, preset: NamingPreset) -> Result<String, TransferError> {
    match preset {
        NamingPreset::LauncherDated => next_launcher_code(parent),
        NamingPreset::FolderStructurer => next_folder_structurer_number(parent),
    }
}

// ---------------------------------------------------------------------------
// Sequence/shot model (explicit names, rename, chosen parent, independents)
// ---------------------------------------------------------------------------

/// One UI sequence/shot entry, as the source widgets collected them:
/// `sequence: None` marks independent shots (`shots_without_sequence`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShotGroup {
    pub sequence: Option<String>,
    pub shots: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedSequence {
    pub name: String,
    pub shots: Vec<String>,
}

/// The validated sequence/shot tree behind the source widget semantics:
/// explicitly named sequences, rename, shots under a chosen sequence and
/// independent shots. Duplicates are rejected instead of silently collapsed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SequenceShotTree {
    sequences: Vec<NamedSequence>,
    independent_shots: Vec<String>,
}

impl SequenceShotTree {
    /// Normalises UI data (`_prepare_sequences_and_shots` plus
    /// `shots_without_sequence`): trims, drops blanks, validates every name.
    pub fn from_groups(groups: &[ShotGroup]) -> Result<Self, TransferError> {
        let mut tree = Self::default();
        for group in groups {
            match &group.sequence {
                Some(raw) => tree.push_named_group(raw, &group.shots)?,
                None => tree.push_independent(&group.shots)?,
            }
        }
        Ok(tree)
    }

    fn push_named_group(&mut self, raw: &str, shots: &[String]) -> Result<(), TransferError> {
        let name = validate_sequence_or_shot(raw, "sequence name")?;
        if self.sequences.iter().any(|seq| seq.name == name) {
            return Err(invalid(format!("sequence {name:?} is listed twice")));
        }
        let mut validated = Vec::with_capacity(shots.len());
        for shot in shots {
            let shot = validate_sequence_or_shot(shot, "shot name")?;
            if validated.contains(&shot) {
                return Err(invalid(format!(
                    "shot {shot:?} is listed twice in sequence {name:?}"
                )));
            }
            validated.push(shot);
        }
        self.sequences.push(NamedSequence {
            name,
            shots: validated,
        });
        Ok(())
    }

    fn push_independent(&mut self, shots: &[String]) -> Result<(), TransferError> {
        for shot in shots {
            let shot = validate_sequence_or_shot(shot, "shot name")?;
            if self.independent_shots.contains(&shot) {
                return Err(invalid(format!(
                    "independent shot {shot:?} is listed twice"
                )));
            }
            self.independent_shots.push(shot);
        }
        Ok(())
    }

    /// Adds a new explicitly named sequence (the source never generated names).
    pub fn add_sequence(&mut self, raw: &str) -> Result<(), TransferError> {
        let name = validate_sequence_or_shot(raw, "sequence name")?;
        if self.sequences.iter().any(|seq| seq.name == name) {
            return Err(invalid(format!("sequence {name:?} already exists")));
        }
        self.sequences.push(NamedSequence {
            name,
            shots: Vec::new(),
        });
        Ok(())
    }

    /// Renames a sequence, keeping its shots (the editable tree behaviour).
    pub fn rename_sequence(&mut self, old: &str, new: &str) -> Result<(), TransferError> {
        let new = validate_sequence_or_shot(new, "sequence name")?;
        if !self.sequences.iter().any(|seq| seq.name == old) {
            return Err(invalid(format!("sequence {old:?} does not exist")));
        }
        if new != old && self.sequences.iter().any(|seq| seq.name == new) {
            return Err(invalid(format!("sequence {new:?} already exists")));
        }
        if let Some(sequence) = self.sequences.iter_mut().find(|seq| seq.name == old) {
            sequence.name = new;
        }
        Ok(())
    }

    pub fn remove_sequence(&mut self, name: &str) -> Result<(), TransferError> {
        let position = self
            .sequences
            .iter()
            .position(|seq| seq.name == name)
            .ok_or_else(|| invalid(format!("sequence {name:?} does not exist")))?;
        self.sequences.remove(position);
        Ok(())
    }

    /// Adds shots under a chosen sequence, or as independent shots when
    /// `sequence` is `None`.
    pub fn add_shots(
        &mut self,
        sequence: Option<&str>,
        shots: &[String],
    ) -> Result<(), TransferError> {
        match sequence {
            Some(name) => {
                let name = validate_sequence_or_shot(name, "sequence name")?;
                let target = self
                    .sequences
                    .iter_mut()
                    .find(|seq| seq.name == name)
                    .ok_or_else(|| invalid(format!("sequence {name:?} does not exist")))?;
                for shot in shots {
                    let shot = validate_sequence_or_shot(shot, "shot name")?;
                    if target.shots.contains(&shot) {
                        return Err(invalid(format!("shot {shot:?} already exists in {name:?}")));
                    }
                    target.shots.push(shot);
                }
                Ok(())
            }
            None => self.push_independent(shots),
        }
    }

    pub fn remove_shot(&mut self, sequence: Option<&str>, shot: &str) -> Result<(), TransferError> {
        let position = match sequence {
            Some(name) => self
                .sequences
                .iter()
                .find(|seq| seq.name == name)
                .ok_or_else(|| invalid(format!("sequence {name:?} does not exist")))?
                .shots
                .iter()
                .position(|existing| existing == shot),
            None => self
                .independent_shots
                .iter()
                .position(|existing| existing == shot),
        };
        let position =
            position.ok_or_else(|| invalid(format!("shot {shot:?} does not exist there")))?;
        match sequence {
            Some(name) => {
                if let Some(seq) = self.sequences.iter_mut().find(|seq| seq.name == name) {
                    seq.shots.remove(position);
                }
            }
            None => {
                self.independent_shots.remove(position);
            }
        }
        Ok(())
    }

    pub fn named(&self) -> &[NamedSequence] {
        &self.sequences
    }

    pub fn independent_shots(&self) -> &[String] {
        &self.independent_shots
    }

    /// Back to UI-shaped groups; independent shots form the `None` group.
    pub fn groups(&self) -> Vec<ShotGroup> {
        let mut groups: Vec<ShotGroup> = self
            .sequences
            .iter()
            .map(|seq| ShotGroup {
                sequence: Some(seq.name.clone()),
                shots: seq.shots.clone(),
            })
            .collect();
        if !self.independent_shots.is_empty() {
            groups.push(ShotGroup {
                sequence: None,
                shots: self.independent_shots.clone(),
            });
        }
        groups
    }

    pub fn is_empty(&self) -> bool {
        self.sequences.is_empty() && self.independent_shots.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

/// Which marker files a template directory carried; markers are never copied.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TemplateMarkers {
    pub sequences: bool,
    pub shots: bool,
}

impl TemplateMarkers {
    pub fn any(self) -> bool {
        self.sequences || self.shots
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TemplateEntryKind {
    /// A directory to create; `markers` records expansion behaviour.
    Directory { markers: TemplateMarkers },
    /// A regular file replicated with these captured contents.
    File { contents: Vec<u8> },
    /// A `template.<ext>` file renamed to `<project root>_v001.<ext>` per
    /// project (the source's per-project template files).
    ProjectTemplateFile {
        contents: Vec<u8>,
        extension: std::ffi::OsString,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateEntry {
    /// Plain relative path, forward-slash spelling as imported.
    pub relative: PathBuf,
    pub kind: TemplateEntryKind,
}

/// A template snapshot: stable local ID (never a filesystem path), display
/// name, and ordered relative entries. Directory templates are read from disk
/// once; planning afterwards is pure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructureTemplate {
    pub id: String,
    pub name: String,
    pub entries: Vec<TemplateEntry>,
}

impl StructureTemplate {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            entries: Vec::new(),
        }
    }

    pub fn add_directory(
        &mut self,
        relative: &Path,
        markers: TemplateMarkers,
    ) -> Result<(), TransferError> {
        let relative = validate_relative_components(relative)?;
        self.insert(relative, TemplateEntryKind::Directory { markers })
    }

    /// Adds a regular file; `template.<ext>` stems become per-project files
    /// and marker names are reserved for directories.
    pub fn add_file(&mut self, relative: &Path, contents: Vec<u8>) -> Result<(), TransferError> {
        let relative = validate_relative_components(relative)?;
        let name = relative
            .file_name()
            .map(OsStr::to_string_lossy)
            .unwrap_or_default();
        if name.eq_ignore_ascii_case(SEQUENCE_MARKER_FILE)
            || name.eq_ignore_ascii_case(SHOT_MARKER_FILE)
        {
            return Err(invalid(format!(
                "{SEQUENCE_MARKER_FILE}/{SHOT_MARKER_FILE} are reserved marker names"
            )));
        }
        let kind = classify_template_file(relative.file_name().unwrap_or_default(), contents);
        self.insert(relative, kind)
    }

    fn insert(&mut self, relative: PathBuf, kind: TemplateEntryKind) -> Result<(), TransferError> {
        if self.entries.iter().any(|entry| entry.relative == relative) {
            return Err(invalid(format!(
                "template already has an entry at {}",
                relative.display()
            )));
        }
        self.entries.push(TemplateEntry { relative, kind });
        self.sort_entries();
        Ok(())
    }

    pub(crate) fn sort_entries(&mut self) {
        self.entries
            .sort_unstable_by(|a, b| entry_order(&a.relative).cmp(&entry_order(&b.relative)));
    }

    pub fn has_marker_directories(&self) -> bool {
        self.entries.iter().any(|entry| match &entry.kind {
            TemplateEntryKind::Directory { markers } => markers.any(),
            _ => false,
        })
    }
}

fn entry_order(relative: &Path) -> Vec<u8> {
    relative.as_os_str().as_encoded_bytes().to_vec()
}

fn validate_relative_components(relative: &Path) -> Result<PathBuf, TransferError> {
    if relative.as_os_str().is_empty() {
        return Err(invalid("template paths cannot be empty"));
    }
    let plain = relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)));
    if !plain {
        return Err(invalid(format!(
            "template path {} must be plain relative components",
            relative.display()
        )));
    }
    Ok(relative.to_path_buf())
}

fn classify_template_file(name: &OsStr, contents: Vec<u8>) -> TemplateEntryKind {
    let as_path = Path::new(name);
    let stem_matches_template = as_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().eq_ignore_ascii_case("template"))
        .unwrap_or(false);
    match as_path.extension() {
        Some(extension) if stem_matches_template => TemplateEntryKind::ProjectTemplateFile {
            contents,
            extension: extension.to_os_string(),
        },
        _ => TemplateEntryKind::File { contents },
    }
}

/// Walks one template folder once (read-only) and captures its shape and file
/// contents: directories (with marker flags), replicated files, and
/// `template.<ext>` per-project files. Entries are stored sorted, parents
/// before children. Symlinked or special entries are refused rather than
/// guessed (the source skipped symlinked directories silently).
pub fn read_template_directory(
    id: impl Into<String>,
    name: impl Into<String>,
    dir: &Path,
) -> Result<StructureTemplate, TransferError> {
    let meta = fs::metadata(dir).map_err(|error| {
        TransferError::new(
            TransferCategory::Unavailable,
            Some(dir),
            format!("template folder cannot be inspected: {error}"),
        )
    })?;
    if !meta.is_dir() {
        return Err(TransferError::new(
            TransferCategory::InvalidInput,
            Some(dir),
            "template path is not a directory",
        ));
    }
    let mut template = StructureTemplate::new(id, name);
    read_template_level(dir, Path::new(""), &mut template)?;
    template.sort_entries();
    Ok(template)
}

enum TemplateChild {
    Directory(PathBuf),
    File(PathBuf),
}

fn read_template_level(
    dir: &Path,
    relative: &Path,
    template: &mut StructureTemplate,
) -> Result<(), TransferError> {
    let children = sorted_template_children(dir)?;
    let mut markers = TemplateMarkers::default();
    let mut subdirectories = Vec::new();
    for child in children {
        match child {
            TemplateChild::Directory(path) => subdirectories.push(path),
            TemplateChild::File(path) => {
                let contents = fs::read(&path).map_err(|error| {
                    TransferError::new(
                        TransferCategory::Unavailable,
                        Some(&path),
                        format!("template file cannot be read: {error}"),
                    )
                })?;
                let name = path
                    .file_name()
                    .map(OsStr::to_string_lossy)
                    .unwrap_or_default();
                if name.eq_ignore_ascii_case(SEQUENCE_MARKER_FILE) {
                    markers.sequences = true;
                } else if name.eq_ignore_ascii_case(SHOT_MARKER_FILE) {
                    markers.shots = true;
                } else {
                    let entry_relative = relative.join(path.file_name().unwrap_or_default());
                    let kind =
                        classify_template_file(path.file_name().unwrap_or_default(), contents);
                    template.entries.push(TemplateEntry {
                        relative: entry_relative,
                        kind,
                    });
                }
            }
        }
    }
    if !relative.as_os_str().is_empty() {
        template.entries.push(TemplateEntry {
            relative: relative.to_path_buf(),
            kind: TemplateEntryKind::Directory { markers },
        });
    }
    for subdirectory in subdirectories {
        let child_relative = relative.join(subdirectory.file_name().unwrap_or_default());
        read_template_level(&subdirectory, &child_relative, template)?;
    }
    Ok(())
}

fn sorted_template_children(dir: &Path) -> Result<Vec<TemplateChild>, TransferError> {
    let entries = fs::read_dir(dir).map_err(|error| {
        TransferError::new(
            TransferCategory::Unavailable,
            Some(dir),
            format!("template folder cannot be listed: {error}"),
        )
    })?;
    let mut children: Vec<(std::ffi::OsString, TemplateChild)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            TransferError::new(
                TransferCategory::Unavailable,
                Some(dir),
                format!("template listing failed: {error}"),
            )
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let link = fs::symlink_metadata(&path).map_err(|error| {
            TransferError::new(
                TransferCategory::Unavailable,
                Some(&path),
                format!("template entry cannot be inspected: {error}"),
            )
        })?;
        let child = if link.is_symlink() {
            // Broken or pointing anywhere: refuse instead of guessing. A link
            // to a regular file keeps copy semantics (the source's copy2
            // followed file links); directory links are refused.
            match fs::metadata(&path) {
                Ok(meta) if !meta.is_dir() => TemplateChild::File(path),
                _ => {
                    return Err(TransferError::new(
                        TransferCategory::Unsupported,
                        Some(&path),
                        "template entry is a symbolic link; the source skipped or followed it unpredictably",
                    ));
                }
            }
        } else if link.is_dir() {
            TemplateChild::Directory(path)
        } else if link.is_file() {
            TemplateChild::File(path)
        } else {
            return Err(TransferError::new(
                TransferCategory::Unsupported,
                Some(&path),
                "template entry is neither a regular file nor a directory",
            ));
        };
        children.push((name, child));
    }
    children.sort_unstable_by(|a, b| a.0.as_encoded_bytes().cmp(&b.0.as_encoded_bytes()));
    Ok(children.into_iter().map(|(_, child)| child).collect())
}

/// Pure rename of one node (and its descendants) inside a template snapshot —
/// the tree-editing half of the source's `save_template_structure`, without
/// the disk writes, which belong to a later template-persistence packet.
pub fn rename_template_node(
    template: &StructureTemplate,
    relative: &Path,
    new_name: &str,
) -> Result<StructureTemplate, TransferError> {
    let target = validate_relative_components(relative)?;
    let new_name = validate_component(new_name, "template folder name")?;
    if !template
        .entries
        .iter()
        .any(|entry| entry.relative == target)
    {
        return Err(invalid(format!(
            "template has no entry at {}",
            target.display()
        )));
    }
    let renamed = match target.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(&new_name),
        _ => PathBuf::from(&new_name),
    };
    if renamed != target
        && template
            .entries
            .iter()
            .any(|entry| entry.relative == renamed)
    {
        return Err(TransferError::new(
            TransferCategory::Refused,
            Some(&renamed),
            "a template entry already exists at the new name",
        ));
    }
    let mut renamed_template = template.clone();
    for entry in &mut renamed_template.entries {
        if entry.relative == target || entry.relative.starts_with(&target) {
            let suffix = entry
                .relative
                .strip_prefix(&target)
                .unwrap_or(Path::new(""));
            entry.relative = if suffix.as_os_str().is_empty() {
                renamed.clone()
            } else {
                renamed.join(suffix)
            };
        }
    }
    renamed_template.sort_entries();
    Ok(renamed_template)
}

/// The Launcher dated preset's directory manifest (`ui_slick/structure.txt`),
/// as a template without markers — the source copied it verbatim with no
/// sequence/shot expansion.
pub fn launcher_default_template() -> StructureTemplate {
    const LAUNCHER_STRUCTURE_MANIFEST: &[&str] = &[
        "2D",
        "2D/Illustrations",
        "2D/Images",
        "2D/Storyboard",
        "3D",
        "3D/Models",
        "3D/Previz",
        "3D/Renders",
        "3D/Textures",
        "Audio",
        "Audio/Mixes",
        "Audio/Music",
        "Audio/SFX",
        "Audio/VO",
        "Deliverables",
        "Deliverables/Final",
        "Deliverables/Masters",
        "Deliverables/Review",
        "Documents",
        "Projects",
        "Projects/After Effects",
        "Projects/Nuke",
        "Projects/Premiere",
        "Projects/Resolve",
        "Projects/Tracking",
        "Renders",
        "Renders/AE",
        "Renders/Nuke",
        "Renders/Resolve",
        "Video",
        "Video/Footage",
        "Video/Guides",
        "Video/XML-AAF-EDL",
    ];
    let mut template = StructureTemplate::new("launcher-default", "Launcher");
    for line in LAUNCHER_STRUCTURE_MANIFEST {
        // Static manifest: every line is validated once here; a bad constant is
        // a build-time bug, not a runtime condition.
        template
            .add_directory(Path::new(line), TemplateMarkers::default())
            .expect("launcher manifest constants are plain relative paths");
    }
    template
}

// ---------------------------------------------------------------------------
// Nuke settings and the S2 content hook
// ---------------------------------------------------------------------------

/// Script generation options, exactly the options the source collected
/// (fps/resolution/proxy/ACES); contents live in the S2 `nuke.rs` packet.
#[derive(Clone, Debug, PartialEq)]
pub struct NukeSettings {
    pub fps: f64,
    pub width: u32,
    pub height: u32,
    pub proxy: bool,
    pub aces: bool,
}

fn validate_nuke_settings(settings: &NukeSettings) -> Result<(), TransferError> {
    if !settings.fps.is_finite() || settings.fps <= 0.0 {
        return Err(invalid("Nuke fps must be a positive finite number"));
    }
    if settings.width == 0 || settings.height == 0 {
        return Err(invalid("Nuke resolution must be non-zero"));
    }
    Ok(())
}

/// What the content source needs to render one script's bytes.
#[derive(Clone, Copy, Debug)]
pub struct ScriptContentRequest<'a> {
    /// `None` for independent shots; the source passed `""` in that case.
    pub sequence: Option<&'a str>,
    pub shot: &'a str,
    pub settings: &'a NukeSettings,
}

/// Caller-provided script contents (the future `nuke.rs`). Returning `None`
/// skips that one script with a visible warning; an `Err` aborts the whole
/// plan. No placeholder content is ever written.
pub trait ScriptContentSource {
    fn script_contents(
        &self,
        request: &ScriptContentRequest<'_>,
    ) -> Result<Option<Vec<u8>>, String>;
}

/// `<seq>_<shot>_comp_v001.nk`, or `<shot>_comp_v001.nk` without a sequence —
/// the source's exact file-name rule.
pub fn script_file_name(sequence: Option<&str>, shot: &str) -> String {
    match sequence {
        Some(sequence) => format!("{sequence}_{shot}{SCRIPT_SUFFIX}"),
        None => format!("{shot}{SCRIPT_SUFFIX}"),
    }
}

// ---------------------------------------------------------------------------
// Clients
// ---------------------------------------------------------------------------

/// Clients resource value (`clients.json` in the source); the envelope follows
/// the config-module convention. File wiring belongs to the config packets.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClientsFile {
    pub schema_version: u32,
    pub clients: Vec<String>,
}

impl Default for ClientsFile {
    fn default() -> Self {
        Self {
            schema_version: CLIENTS_SCHEMA_VERSION,
            clients: Vec::new(),
        }
    }
}

/// Trims and validates one client name; Unicode display spelling is preserved.
pub fn normalize_client(raw: &str) -> Result<String, TransferError> {
    validate_component(raw, "client name")
}

/// Trims, drops blanks, deduplicates and sorts (code-point order, the same
/// ordering Python's `list.sort()` produced for these strings).
pub fn normalize_clients(raw: &[String]) -> Result<Vec<String>, TransferError> {
    let mut seen = HashSet::new();
    let mut clients = Vec::with_capacity(raw.len());
    for value in raw {
        let name = normalize_client(value)?;
        if seen.insert(name.clone()) {
            clients.push(name);
        }
    }
    clients.sort();
    Ok(clients)
}

/// Adds one client; `Ok(false)` when it already existed, as in the source's
/// duplicate check. The list stays alphabetically sorted.
pub fn add_client(clients: &mut Vec<String>, raw: &str) -> Result<bool, TransferError> {
    let name = normalize_client(raw)?;
    if clients.iter().any(|existing| existing == &name) {
        return Ok(false);
    }
    clients.push(name);
    clients.sort();
    Ok(true)
}

// ---------------------------------------------------------------------------
// Inputs and planners
// ---------------------------------------------------------------------------

/// New-project request (CONTRACTS.md §4). `date` must carry the once-captured
/// `YYYY-MM-DD`; `shots` holds named groups and independent shots
/// (`sequence: None`); `template` is passed separately to [`plan_structure`].
#[derive(Clone, Debug, PartialEq)]
pub struct CreateProject {
    pub parent: PathBuf,
    pub number: Option<String>,
    pub client: Option<String>,
    pub name: String,
    pub date: Option<String>,
    pub naming: NamingPreset,
    pub shots: Vec<ShotGroup>,
    pub nuke: Option<NukeSettings>,
}

fn absolute_existing_dir(path: &Path, what: &str) -> Result<PathBuf, TransferError> {
    if !path.is_absolute() {
        return Err(TransferError::new(
            TransferCategory::InvalidInput,
            Some(path),
            format!("{what} must be an absolute path"),
        ));
    }
    let meta = fs::metadata(path).map_err(|error| {
        TransferError::new(
            TransferCategory::Unavailable,
            Some(path),
            format!("{what} cannot be inspected: {error}"),
        )
    })?;
    if !meta.is_dir() {
        return Err(TransferError::new(
            TransferCategory::InvalidInput,
            Some(path),
            format!("{what} is not a directory"),
        ));
    }
    Ok(path.to_path_buf())
}

/// Accumulates relative actions and rejects duplicate destinations up front
/// (transfers validation also rejects them; planning fails earlier with the
/// offending path instead of one blocked item per duplicate).
struct PlanAccumulator {
    destination_root: PathBuf,
    add_to_existing: bool,
    directories: Vec<PathBuf>,
    files: Vec<(PathBuf, Vec<u8>)>,
    seen: HashSet<PathBuf>,
}

impl PlanAccumulator {
    fn new(destination_root: PathBuf, add_to_existing: bool) -> Self {
        Self {
            destination_root,
            add_to_existing,
            directories: Vec::new(),
            files: Vec::new(),
            seen: HashSet::new(),
        }
    }

    fn dir(&mut self, relative: PathBuf) -> Result<(), TransferError> {
        if !self.seen.insert(relative.clone()) {
            return Err(invalid(format!(
                "plan builds the destination {} twice",
                relative.display()
            )));
        }
        self.directories.push(relative);
        Ok(())
    }

    fn file(&mut self, relative: PathBuf, contents: Vec<u8>) -> Result<(), TransferError> {
        if !self.seen.insert(relative.clone()) {
            return Err(invalid(format!(
                "plan builds the destination {} twice",
                relative.display()
            )));
        }
        self.files.push((relative, contents));
        Ok(())
    }

    /// Directories sorted parents-first (a parent path is always a byte prefix
    /// of its children), then generated files; both after the new exclusive
    /// root action.
    fn finish(
        self,
        id: impl Into<String>,
        revision: u64,
        warnings: Vec<String>,
    ) -> Result<OperationPlan, TransferError> {
        let plan_id = id.into();
        let destination_root = StoredPath::absolute(&self.destination_root)?;
        let mut directories = self.directories;
        directories.sort_unstable_by(|a, b| entry_order(a).cmp(&entry_order(b)));
        let mut files = self.files;
        files.sort_unstable_by(|a, b| entry_order(&a.0).cmp(&entry_order(&b.0)));
        let mut actions = Vec::with_capacity(directories.len() + files.len());
        for relative in directories {
            actions.push(PlannedAction::CreateDirectory {
                relative: StoredPath::from_native(&relative, &self.destination_root)?,
            });
        }
        for (relative, contents) in files {
            actions.push(PlannedAction::CreateFile {
                relative: StoredPath::from_native(&relative, &self.destination_root)?,
                contents,
            });
        }
        Ok(OperationPlan {
            id: plan_id,
            revision,
            origin: PlanOrigin::Structure,
            destination_root,
            add_to_existing: self.add_to_existing,
            policy: ConflictPolicy::Skip,
            source_root: None,
            source_evidence: None,
            warnings,
            actions,
        })
    }
}

/// Plans a new project from a template snapshot. The destination root of the
/// plan is the *parent*: the project folder itself is the first
/// `CreateDirectory` action, and with `add_to_existing: false` transfers
/// validation creates it exclusively, failing safely on numbering collisions.
pub fn plan_structure(
    id: impl Into<String>,
    revision: u64,
    input: &CreateProject,
    template: &StructureTemplate,
    scripts: Option<&dyn ScriptContentSource>,
) -> Result<OperationPlan, TransferError> {
    let parent = absolute_existing_dir(&input.parent, "destination parent")?;
    let date = input.date.as_deref().ok_or_else(|| {
        invalid("a captured project date is required so preview and execution share one name")
    })?;
    let date = validate_captured_date(date)?;
    let number = match input.number.as_deref() {
        Some(raw) => validate_optional_digits(raw, "project number")?,
        None => Some(auto_number(&parent, input.naming)?),
    };
    let base = build_project_root_name(
        input.naming,
        number.as_deref(),
        input.client.as_deref(),
        &input.name,
    )?;
    let final_name = format!("{base}_{date}");
    let tree = SequenceShotTree::from_groups(&input.shots)?;
    if let Some(settings) = &input.nuke {
        validate_nuke_settings(settings)?;
    }
    let mut acc = PlanAccumulator::new(parent, false);
    acc.dir(PathBuf::from(&final_name))?;
    let mut warnings = Vec::new();
    expand_template(
        &mut acc,
        template,
        &tree,
        &final_name,
        input.nuke.as_ref(),
        scripts,
        &mut warnings,
    )?;
    if !tree.is_empty() && !template.has_marker_directories() {
        warnings.push(
            "the template has no sequence/shot markers, so no shots were expanded into it"
                .to_string(),
        );
    }
    acc.finish(id, revision, warnings)
}

/// Replicates the template snapshot into planned actions under the project
/// root name, expanding marker directories with the sequence/shot tree.
fn expand_template(
    acc: &mut PlanAccumulator,
    template: &StructureTemplate,
    tree: &SequenceShotTree,
    final_name: &str,
    nuke: Option<&NukeSettings>,
    scripts: Option<&dyn ScriptContentSource>,
    warnings: &mut Vec<String>,
) -> Result<(), TransferError> {
    for entry in &template.entries {
        let relative = PathBuf::from(final_name).join(&entry.relative);
        match &entry.kind {
            TemplateEntryKind::Directory { markers } => {
                acc.dir(relative.clone())?;
                if markers.any() && !tree.is_empty() {
                    expand_marker(acc, &relative, *markers, tree, nuke, scripts, warnings)?;
                }
            }
            TemplateEntryKind::File { contents } => acc.file(relative, contents.clone())?,
            TemplateEntryKind::ProjectTemplateFile {
                contents,
                extension,
            } => {
                let mut renamed = std::ffi::OsString::from(format!("{final_name}_v001."));
                renamed.push(extension);
                let target = match relative.parent() {
                    Some(parent) if !parent.as_os_str().is_empty() => parent.join(renamed),
                    _ => PathBuf::from(renamed),
                };
                acc.file(target, contents.clone())?;
            }
        }
    }
    Ok(())
}

/// Marker expansion, identical for new projects and add-to-existing:
/// sequences under `sequences.txt` directories; independent shots and nested
/// sequence/shot pairs (flattened when the directory only carries
/// `shots.txt`) under `shots.txt` directories, each with its `project/` and
/// `render/` folders and Nuke script named per the source rules.
fn expand_marker(
    acc: &mut PlanAccumulator,
    marker_relative: &Path,
    markers: TemplateMarkers,
    tree: &SequenceShotTree,
    nuke: Option<&NukeSettings>,
    scripts: Option<&dyn ScriptContentSource>,
    warnings: &mut Vec<String>,
) -> Result<(), TransferError> {
    for sequence in tree.named() {
        if markers.sequences {
            acc.dir(marker_relative.join(&sequence.name))?;
        }
    }
    if !markers.shots {
        return Ok(());
    }
    for shot in tree.independent_shots() {
        shot_block(
            acc,
            marker_relative,
            None,
            shot,
            true,
            nuke,
            scripts,
            warnings,
        )?;
    }
    for sequence in tree.named() {
        for shot in &sequence.shots {
            if markers.sequences {
                let seq_dir = marker_relative.join(&sequence.name);
                shot_block(
                    acc,
                    &seq_dir,
                    Some(&sequence.name),
                    shot,
                    true,
                    nuke,
                    scripts,
                    warnings,
                )?;
            } else {
                // shots.txt without sequences.txt: shots sit directly under the
                // marker directory and the script name drops the sequence.
                shot_block(
                    acc,
                    marker_relative,
                    None,
                    shot,
                    true,
                    nuke,
                    scripts,
                    warnings,
                )?;
            }
        }
    }
    Ok(())
}

/// One shot folder: the folder itself, its `project/`+`render/` pair, and the
/// Nuke script whose contents come from the caller-provided source.
#[allow(clippy::too_many_arguments)]
fn shot_block(
    acc: &mut PlanAccumulator,
    parent_relative: &Path,
    sequence: Option<&str>,
    shot: &str,
    create_script_dirs: bool,
    nuke: Option<&NukeSettings>,
    scripts: Option<&dyn ScriptContentSource>,
    warnings: &mut Vec<String>,
) -> Result<(), TransferError> {
    let shot_dir = parent_relative.join(shot);
    acc.dir(shot_dir.clone())?;
    let Some(settings) = nuke else {
        if create_script_dirs {
            // New-project template semantics create project/render regardless;
            // add-to-existing sources created them only with scripts.
            acc.dir(shot_dir.join(SHOT_PROJECT_DIR))?;
            acc.dir(shot_dir.join(SHOT_RENDER_DIR))?;
        }
        return Ok(());
    };
    acc.dir(shot_dir.join(SHOT_PROJECT_DIR))?;
    acc.dir(shot_dir.join(SHOT_RENDER_DIR))?;
    let Some(source) = scripts else {
        return Err(invalid(
            "a Nuke script content source is required when Nuke generation is requested; \
             the S2 nuke.rs packet provides one",
        ));
    };
    let request = ScriptContentRequest {
        sequence,
        shot,
        settings,
    };
    let script_path = shot_dir
        .join(SHOT_PROJECT_DIR)
        .join(script_file_name(sequence, shot));
    match source.script_contents(&request) {
        Ok(Some(contents)) => acc.file(script_path, contents),
        Ok(None) => {
            warnings.push(format!(
                "no script content was provided for {sequence:?}/{shot}; its Nuke script is skipped"
            ));
            Ok(())
        }
        Err(reason) => Err(TransferError::new(
            TransferCategory::InvalidInput,
            Some(&script_path),
            format!("the Nuke script content source refused the request: {reason}"),
        )),
    }
}

/// Plans sequence/shot additions into every existing marker directory of the
/// chosen template inside an existing project (the destination template the UI
/// shows is exactly these directories). Missing marker directories become
/// visible warnings instead of silent skips; none existing is an error.
pub fn plan_add_shots(
    id: impl Into<String>,
    revision: u64,
    project: &Path,
    shots: &[ShotGroup],
    template: &StructureTemplate,
    nuke: Option<&NukeSettings>,
    scripts: Option<&dyn ScriptContentSource>,
) -> Result<OperationPlan, TransferError> {
    let project = absolute_existing_dir(project, "project")?;
    let tree = SequenceShotTree::from_groups(shots)?;
    if tree.is_empty() {
        return Err(invalid("no sequences or shots were provided"));
    }
    if let Some(settings) = nuke {
        validate_nuke_settings(settings)?;
    }
    let mut acc = PlanAccumulator::new(project.clone(), true);
    let mut warnings = Vec::new();
    let mut matched = 0usize;
    for entry in &template.entries {
        let TemplateEntryKind::Directory { markers } = &entry.kind else {
            continue;
        };
        if !markers.any() {
            continue;
        }
        match fs::metadata(project.join(&entry.relative)) {
            Ok(meta) if meta.is_dir() => {
                matched += 1;
                expand_marker(
                    &mut acc,
                    &entry.relative,
                    *markers,
                    &tree,
                    nuke,
                    scripts,
                    &mut warnings,
                )?;
            }
            Ok(_) => warnings.push(format!(
                "template destination {} exists but is not a directory; skipped",
                entry.relative.display()
            )),
            Err(_) => warnings.push(format!(
                "template destination {} is missing in the project; skipped",
                entry.relative.display()
            )),
        }
    }
    if matched == 0 {
        return Err(invalid(
            "none of the template destinations exist in this project; nothing can be added",
        ));
    }
    acc.finish(id, revision, warnings)
}

/// Lexical confinement of a user-chosen target folder, then the same
/// containment rules `paths::validate_destination` applies to destinations
/// (read-only snapshot checks; execution revalidates).
fn confine_target(project: &Path, target: &Path) -> Result<PathBuf, TransferError> {
    if !target.is_absolute() {
        return Err(TransferError::new(
            TransferCategory::InvalidInput,
            Some(target),
            "target folders must be absolute paths",
        ));
    }
    let relative = target.strip_prefix(project).map_err(|_| {
        TransferError::new(
            TransferCategory::InvalidInput,
            Some(target),
            "target folder is not inside the project",
        )
    })?;
    if relative.as_os_str().is_empty() {
        return Err(TransferError::new(
            TransferCategory::InvalidInput,
            Some(target),
            "the project root itself is not a valid target",
        ));
    }
    let plain = relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)));
    if !plain {
        return Err(TransferError::new(
            TransferCategory::InvalidInput,
            Some(target),
            "target folder must be a plain path under the project",
        ));
    }
    paths::validate_destination(project, relative).map_err(|error| {
        TransferError::new(
            TransferCategory::Refused,
            Some(target),
            format!("target folder cannot be used: {error}"),
        )
    })
}

/// The source's `folder_or_parents_has_comp`: "comp" appears (case-insensitive
/// substring) in the target's own name, any ancestor up to the project root,
/// or the project root's name.
fn target_is_comp(relative: &Path, project: &Path) -> bool {
    let contains = |text: &std::ffi::OsStr| text.to_string_lossy().to_lowercase().contains("comp");
    let in_components = relative
        .components()
        .any(|component| contains(component.as_os_str()));
    let in_project_name = project.file_name().map(contains).unwrap_or(false);
    in_components || in_project_name
}

/// Plans sequence/shot additions into caller-chosen existing folders of an
/// existing project — the checked-tree flow (`add_to_existing_project_tab.py`
/// and `main.py handle_existing_create`): named sequences as subfolders,
/// sequence-less shots directly in the target, and `project/`+`render/` plus
/// Nuke scripts only where the target path matches the "comp" rule. With
/// `parent_as_sequence`, an independent shot's script is named
/// `<target>_<shot>_comp_v001.nk`, exactly as the checkbox did.
#[allow(clippy::too_many_arguments)]
pub fn plan_add_to_targets(
    id: impl Into<String>,
    revision: u64,
    project: &Path,
    targets: &[PathBuf],
    shots: &[ShotGroup],
    nuke: Option<&NukeSettings>,
    parent_as_sequence: bool,
    scripts: Option<&dyn ScriptContentSource>,
) -> Result<OperationPlan, TransferError> {
    let project = absolute_existing_dir(project, "project")?;
    let tree = SequenceShotTree::from_groups(shots)?;
    if tree.is_empty() {
        return Err(invalid("no sequences or shots were provided"));
    }
    if let Some(settings) = nuke {
        validate_nuke_settings(settings)?;
    }
    let mut warnings = Vec::new();
    let mut unique: Vec<PathBuf> = Vec::new();
    let mut seen = HashSet::new();
    for target in targets {
        let confined = confine_target(&project, target)?;
        if seen.insert(confined.clone()) {
            unique.push(confined);
        } else {
            warnings.push(format!("duplicate target {} ignored", target.display()));
        }
    }
    if unique.is_empty() {
        return Err(invalid(
            "no target folders inside the project were provided",
        ));
    }
    let mut acc = PlanAccumulator::new(project.clone(), true);
    for target in unique {
        let relative = target
            .strip_prefix(&project)
            .map_err(|_| invalid("target escaped its project during planning"))?;
        let is_comp = target_is_comp(relative, &project);
        for sequence in tree.named() {
            let sequence_dir = relative.join(&sequence.name);
            acc.dir(sequence_dir.clone())?;
            for shot in &sequence.shots {
                shot_block(
                    &mut acc,
                    &sequence_dir,
                    Some(&sequence.name),
                    shot,
                    is_comp,
                    nuke,
                    scripts,
                    &mut warnings,
                )?;
            }
        }
        let script_sequence: Option<String> = if parent_as_sequence {
            target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        } else {
            None
        };
        for shot in tree.independent_shots() {
            shot_block(
                &mut acc,
                relative,
                script_sequence.as_deref(),
                shot,
                is_comp,
                nuke,
                scripts,
                &mut warnings,
            )?;
        }
    }
    acc.finish(id, revision, warnings)
}

// ---------------------------------------------------------------------------
// Existing-project structure detection
// ---------------------------------------------------------------------------

/// Which layout an existing project uses, by the source's own detection order:
/// a `Projects` folder means KENT; `04_vfx`/`05_comp` mean Sphere; anything
/// else defaults to Sphere (`backend_api.add_to_existing_project`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StructureKind {
    Sphere,
    Kent,
}

impl StructureKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sphere => "Sphere",
            Self::Kent => "KENT",
        }
    }
}

pub fn detect_structure_kind(project: &Path) -> Result<StructureKind, TransferError> {
    absolute_existing_dir(project, "project")?;
    let is_dir = |name: &str| {
        fs::metadata(project.join(name))
            .map(|meta| meta.is_dir())
            .unwrap_or(false)
    };
    if is_dir("Projects") {
        return Ok(StructureKind::Kent);
    }
    if is_dir("04_vfx") || is_dir("05_comp") {
        return Ok(StructureKind::Sphere);
    }
    Ok(StructureKind::Sphere)
}
