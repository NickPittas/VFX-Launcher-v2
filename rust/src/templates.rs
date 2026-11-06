//! Structure template library and client names.
//!
//! Ported from FolderStructurer's on-disk administration surface
//! (read-only `gitclones/FolderStructurer`):
//! - `common/backend_api.py`: `Templates/` as one folder per template,
//!   `get_available_templates` (one entry per directory), `create_new_template`,
//!   `add_folder_to_template`, `remove_folder_from_template`,
//!   `save_template_structure` (folder renames) and `clients.json` as a plain
//!   sorted name list created on first use.
//! - `common/backend_api.py` marker files (`sequences.txt`/`shots.txt`) are the
//!   only way a template expands per sequence/shot, so the editor can toggle them.
//!
//! The library lives in the launcher data area (`<data_dir>/structure/`) instead
//! of the program directory, because the packaged application directory is not
//! writable. Layout:
//!   structure/library.json          ordered template registry (id + display name)
//!   structure/clients.json          `structure::ClientsFile` envelope
//!   structure/templates/<id>/…      the template folder exactly as planned from
//!
//! Deliberate deviations, all visible to the caller:
//! - renaming a template changes its display name only; the folder identity stays
//!   stable so saved selections and plans never point at a moved directory;
//! - every write goes through a same-directory temporary file and rename, so an
//!   interrupted save leaves the previous file intact;
//! - a malformed `library.json` or `clients.json` is reported and never
//!   overwritten, so hand-edited data is not silently replaced;
//! - directories found under `templates/` but absent from the registry are adopted
//!   on load (sorted by folder name), matching the source's folder-as-template rule.

use std::ffi::OsStr;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::structure::{self, ClientsFile, StructureTemplate, TemplateEntryKind, TemplateMarkers};
use crate::transfers::{TransferCategory, TransferError};

/// Subdirectory of the launcher data area this module owns.
pub const STRUCTURE_DIR: &str = "structure";
/// Template folder root inside [`STRUCTURE_DIR`].
pub const TEMPLATES_DIR: &str = "templates";
/// Ordered template registry file inside [`STRUCTURE_DIR`].
pub const LIBRARY_FILE: &str = "library.json";
/// Client name list inside [`STRUCTURE_DIR`].
pub const CLIENTS_FILE: &str = "clients.json";
/// Registry envelope version.
pub const LIBRARY_SCHEMA_VERSION: u32 = 1;

fn invalid(message: impl Into<String>) -> TransferError {
    TransferError::new(TransferCategory::InvalidInput, None, message)
}

fn unavailable(path: &Path, message: impl Into<String>) -> TransferError {
    TransferError::new(TransferCategory::Unavailable, Some(path), message)
}

/// One registry row: the folder id and the name the UI shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
}

/// Everything the administration dialog and the planning flows read at once.
#[derive(Clone, Debug, Default)]
pub struct LibrarySnapshot {
    pub templates: Vec<TemplateSummary>,
    pub clients: Vec<String>,
    /// Load-time repairs the user should see (adopted folders, dropped entries).
    pub warnings: Vec<String>,
}

impl LibrarySnapshot {
    pub fn template(&self, id: &str) -> Option<&TemplateSummary> {
        self.templates.iter().find(|entry| entry.id == id)
    }

    pub fn name_of(&self, id: &str) -> Option<&str> {
        self.template(id).map(|entry| entry.name.as_str())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LibraryFile {
    schema_version: u32,
    templates: Vec<LibraryEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LibraryEntry {
    id: String,
    name: String,
}

/// The on-disk template library. Cheap to copy: every method reads what it needs.
#[derive(Clone, Debug)]
pub struct TemplateLibrary {
    root: PathBuf,
}

impl TemplateLibrary {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            root: data_dir.join(STRUCTURE_DIR),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn templates_dir(&self) -> PathBuf {
        self.root.join(TEMPLATES_DIR)
    }

    fn library_path(&self) -> PathBuf {
        self.root.join(LIBRARY_FILE)
    }

    fn clients_path(&self) -> PathBuf {
        self.root.join(CLIENTS_FILE)
    }

    /// Folder of one registered template; callers must have validated the id.
    pub fn template_dir(&self, id: &str) -> PathBuf {
        self.templates_dir().join(id)
    }

    // ------------------------------------------------------------------ load

    /// Reads the registry, the client list and any load-time repairs.
    ///
    /// A missing registry is normal (first run) and adopts every folder already
    /// present. A registry or client file this build cannot parse is an error:
    /// nothing is rewritten over it.
    pub fn load(&self) -> Result<LibrarySnapshot, TransferError> {
        let mut warnings = Vec::new();
        let templates = self.load_registry(&mut warnings)?;
        let clients = self.load_clients()?;
        Ok(LibrarySnapshot {
            templates,
            clients,
            warnings,
        })
    }

    fn load_registry(
        &self,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<TemplateSummary>, TransferError> {
        let mut entries = match fs::read(self.library_path()) {
            Ok(bytes) => parse_library(&bytes, &self.library_path())?,
            Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                return Err(unavailable(
                    &self.library_path(),
                    format!("the template registry cannot be read: {error}"),
                ));
            }
        };
        entries.retain(|entry| {
            if is_plain_component(&entry.id) {
                true
            } else {
                warnings.push(format!(
                    "template registry entry {:?} is not a usable folder name and was ignored",
                    entry.id
                ));
                false
            }
        });

        // Adopt folders the registry does not know about, as the source treated
        // every folder under Templates/ as a template.
        let mut adopted = Vec::new();
        match fs::read_dir(self.templates_dir()) {
            Ok(read) => {
                for entry in read {
                    let entry = entry.map_err(|error| {
                        unavailable(
                            &self.templates_dir(),
                            format!("the template folder cannot be listed: {error}"),
                        )
                    })?;
                    let name = entry.file_name();
                    let Some(id) = name.to_str() else {
                        warnings.push(
                            "a template folder name is not valid UTF-8 and was ignored".to_string(),
                        );
                        continue;
                    };
                    if !is_plain_component(id)
                        || !fs::metadata(entry.path())
                            .map(|meta| meta.is_dir())
                            .unwrap_or(false)
                        || entries.iter().any(|existing| existing.id == id)
                    {
                        continue;
                    }
                    adopted.push(TemplateSummary {
                        id: id.to_string(),
                        name: id.to_string(),
                    });
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(unavailable(
                    &self.templates_dir(),
                    format!("the template folder cannot be listed: {error}"),
                ));
            }
        }
        if !adopted.is_empty() {
            adopted.sort_by(|a, b| a.name.cmp(&b.name));
            warnings.push(format!(
                "{} template folder(s) found on disk were added to the library",
                adopted.len()
            ));
            entries.extend(adopted);
            self.write_registry(&entries)?;
        }
        Ok(entries)
    }

    fn load_clients(&self) -> Result<Vec<String>, TransferError> {
        match fs::read(self.clients_path()) {
            Ok(bytes) => {
                let file: ClientsFile = serde_json::from_slice(&bytes).map_err(|error| {
                    invalid(format!(
                        "{} is not a client list this build can read: {error}",
                        self.clients_path().display()
                    ))
                })?;
                if file.schema_version != structure::CLIENTS_SCHEMA_VERSION {
                    return Err(invalid(format!(
                        "{} has schema version {}; this build reads version {}",
                        self.clients_path().display(),
                        file.schema_version,
                        structure::CLIENTS_SCHEMA_VERSION
                    )));
                }
                structure::normalize_clients(&file.clients)
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(unavailable(
                &self.clients_path(),
                format!("the client list cannot be read: {error}"),
            )),
        }
    }

    /// Persists the whole client list (validated, deduplicated, sorted).
    pub fn save_clients(&self, clients: &[String]) -> Result<Vec<String>, TransferError> {
        let normalized = structure::normalize_clients(clients)?;
        let file = ClientsFile {
            schema_version: structure::CLIENTS_SCHEMA_VERSION,
            clients: normalized.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|error| invalid(format!("the client list cannot be encoded: {error}")))?;
        write_atomic(&self.clients_path(), &bytes)?;
        Ok(normalized)
    }

    // -------------------------------------------------------------- snapshot

    /// Reads one template folder into the planning snapshot.
    pub fn snapshot(&self, summary: &TemplateSummary) -> Result<StructureTemplate, TransferError> {
        let dir = self.template_dir(&summary.id);
        structure::read_template_directory(summary.id.clone(), summary.name.clone(), &dir)
    }

    // ------------------------------------------------------------ registry

    fn read_registry(&self) -> Result<Vec<TemplateSummary>, TransferError> {
        match fs::read(self.library_path()) {
            Ok(bytes) => parse_library(&bytes, &self.library_path()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(unavailable(
                &self.library_path(),
                format!("the template registry cannot be read: {error}"),
            )),
        }
    }

    fn write_registry(&self, entries: &[TemplateSummary]) -> Result<(), TransferError> {
        let file = LibraryFile {
            schema_version: LIBRARY_SCHEMA_VERSION,
            templates: entries
                .iter()
                .map(|entry| LibraryEntry {
                    id: entry.id.clone(),
                    name: entry.name.clone(),
                })
                .collect(),
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(|error| {
            invalid(format!("the template registry cannot be encoded: {error}"))
        })?;
        write_atomic(&self.library_path(), &bytes)
    }

    fn require(&self, entries: &[TemplateSummary], id: &str) -> Result<(), TransferError> {
        if !is_plain_component(id) || !entries.iter().any(|entry| entry.id == id) {
            return Err(invalid(format!("template {id:?} is not in the library")));
        }
        Ok(())
    }

    fn unique_id(&self, name: &str, entries: &[TemplateSummary]) -> String {
        let base = slug(name);
        let mut candidate = base.clone();
        let mut counter = 2u32;
        while entries.iter().any(|entry| entry.id == candidate)
            || self.template_dir(&candidate).exists()
        {
            candidate = format!("{base}-{counter}");
            counter += 1;
        }
        candidate
    }

    // ------------------------------------------------------------ templates

    /// Creates an empty template folder and appends it to the registry.
    pub fn create(&self, name: &str) -> Result<Vec<TemplateSummary>, TransferError> {
        let name = display_name(name)?;
        let mut entries = self.read_registry()?;
        let id = self.unique_id(&name, &entries);
        fs::create_dir_all(self.templates_dir()).map_err(|error| {
            unavailable(
                &self.templates_dir(),
                format!("the template library cannot be created: {error}"),
            )
        })?;
        let dir = self.template_dir(&id);
        fs::create_dir(&dir).map_err(|error| {
            unavailable(
                &dir,
                format!("the template folder cannot be created: {error}"),
            )
        })?;
        entries.push(TemplateSummary {
            id,
            name: name.clone(),
        });
        if let Err(error) = self.write_registry(&entries) {
            let _ = fs::remove_dir(&dir);
            return Err(error);
        }
        Ok(entries)
    }

    /// Copies a folder tree into the library as a new template. The source is read
    /// through the same template reader planning uses, so a tree that could not be
    /// planned (symlinks, special files) is refused instead of copied half-way.
    pub fn import(&self, source: &Path, name: &str) -> Result<Vec<TemplateSummary>, TransferError> {
        let snapshot = structure::read_template_directory("import", "import", source)?;
        self.install(name, &snapshot)
    }

    /// Installs a snapshot as a new template folder. Used by import and by the
    /// built-in Launcher manifest on a first run with an empty library.
    pub fn install(
        &self,
        name: &str,
        snapshot: &StructureTemplate,
    ) -> Result<Vec<TemplateSummary>, TransferError> {
        let name = display_name(name)?;
        let mut entries = self.read_registry()?;
        let id = self.unique_id(&name, &entries);
        let dir = self.template_dir(&id);
        fs::create_dir_all(&self.templates_dir()).map_err(|error| {
            unavailable(
                &self.templates_dir(),
                format!("the template library cannot be created: {error}"),
            )
        })?;
        fs::create_dir(&dir).map_err(|error| {
            unavailable(
                &dir,
                format!("the template folder cannot be created: {error}"),
            )
        })?;
        let written = self.write_snapshot(&dir, snapshot).and_then(|()| {
            entries.push(TemplateSummary {
                id: id.clone(),
                name: name.clone(),
            });
            self.write_registry(&entries)
        });
        if let Err(error) = written {
            let _ = fs::remove_dir_all(&dir);
            return Err(error);
        }
        Ok(entries)
    }

    fn write_snapshot(
        &self,
        dir: &Path,
        snapshot: &StructureTemplate,
    ) -> Result<(), TransferError> {
        for entry in &snapshot.entries {
            let target = dir.join(&entry.relative);
            match &entry.kind {
                TemplateEntryKind::Directory { markers } => {
                    fs::create_dir_all(&target).map_err(|error| {
                        unavailable(
                            &target,
                            format!("the template folder cannot be created: {error}"),
                        )
                    })?;
                    if markers.sequences {
                        write_atomic(&target.join(structure::SEQUENCE_MARKER_FILE), b"")?;
                    }
                    if markers.shots {
                        write_atomic(&target.join(structure::SHOT_MARKER_FILE), b"")?;
                    }
                }
                TemplateEntryKind::File { contents } => {
                    write_atomic(&target, contents)?;
                }
                TemplateEntryKind::ProjectTemplateFile {
                    contents,
                    extension,
                } => {
                    let mut file_name = std::ffi::OsString::from("template.");
                    file_name.push(extension);
                    write_atomic(&target.with_file_name(file_name), contents)?;
                }
            }
        }
        Ok(())
    }

    /// Renames the display name of one template. The folder id never moves.
    pub fn rename(&self, id: &str, new_name: &str) -> Result<Vec<TemplateSummary>, TransferError> {
        let new_name = display_name(new_name)?;
        let mut entries = self.read_registry()?;
        self.require(&entries, id)?;
        let Some(entry) = entries.iter_mut().find(|entry| entry.id == id) else {
            return Err(invalid(format!("template {id:?} is not in the library")));
        };
        entry.name = new_name;
        self.write_registry(&entries)?;
        Ok(entries)
    }

    /// Moves one template one place up (`-1`) or down (`+1`) in the registry.
    pub fn reorder(&self, id: &str, delta: i8) -> Result<Vec<TemplateSummary>, TransferError> {
        let mut entries = self.read_registry()?;
        self.require(&entries, id)?;
        let Some(position) = entries.iter().position(|entry| entry.id == id) else {
            return Err(invalid(format!("template {id:?} is not in the library")));
        };
        let target = if delta < 0 {
            position.checked_sub(1)
        } else {
            Some(position + 1).filter(|next| *next < entries.len())
        };
        let Some(target) = target else {
            return Ok(entries);
        };
        entries.swap(position, target);
        self.write_registry(&entries)?;
        Ok(entries)
    }

    /// Removes a template from the registry and deletes its folder.
    pub fn delete(&self, id: &str) -> Result<Vec<TemplateSummary>, TransferError> {
        let mut entries = self.read_registry()?;
        self.require(&entries, id)?;
        let dir = self.template_dir(id);
        if fs::symlink_metadata(&dir).is_ok() {
            if fs::symlink_metadata(&dir)
                .map(|meta| meta.is_symlink())
                .unwrap_or(false)
            {
                return Err(TransferError::new(
                    TransferCategory::Unsupported,
                    Some(&dir),
                    "the template folder is a symbolic link; remove it by hand",
                ));
            }
            fs::remove_dir_all(&dir).map_err(|error| {
                unavailable(
                    &dir,
                    format!("the template folder cannot be removed: {error}"),
                )
            })?;
        }
        entries.retain(|entry| entry.id != id);
        self.write_registry(&entries)?;
        Ok(entries)
    }

    // ------------------------------------------------------- template edits

    /// Creates one folder inside a template, below `parent` (`""` for the root).
    pub fn add_folder(&self, id: &str, parent: &Path, name: &str) -> Result<(), TransferError> {
        let entries = self.read_registry()?;
        self.require(&entries, id)?;
        let name = component(name, "folder name")?;
        let root = self.template_dir(id);
        let parent_dir = confined(&root, parent, "template parent folder")?;
        if !parent_dir.is_dir() {
            return Err(invalid(format!(
                "template folder {} does not exist",
                parent.display()
            )));
        }
        let target = parent_dir.join(&name);
        if fs::symlink_metadata(&target).is_ok() {
            return Err(TransferError::new(
                TransferCategory::Refused,
                Some(&target),
                "a folder or file with that name already exists in the template",
            ));
        }
        fs::create_dir(&target).map_err(|error| {
            unavailable(
                &target,
                format!("the template folder cannot be created: {error}"),
            )
        })
    }

    /// Deletes one folder and everything below it.
    pub fn remove_folder(&self, id: &str, relative: &Path) -> Result<(), TransferError> {
        let entries = self.read_registry()?;
        self.require(&entries, id)?;
        let root = self.template_dir(id);
        let target = confined(&root, relative, "template folder")?;
        if relative.as_os_str().is_empty() {
            return Err(invalid("the template root itself cannot be removed"));
        }
        let meta = fs::symlink_metadata(&target).map_err(|error| {
            unavailable(
                &target,
                format!("the template folder cannot be inspected: {error}"),
            )
        })?;
        if !meta.is_dir() {
            return Err(invalid(format!(
                "{} is not a template folder",
                relative.display()
            )));
        }
        fs::remove_dir_all(&target).map_err(|error| {
            unavailable(
                &target,
                format!("the template folder cannot be removed: {error}"),
            )
        })
    }

    /// Renames one folder (and its contents) inside a template.
    pub fn rename_folder(
        &self,
        id: &str,
        relative: &Path,
        new_name: &str,
    ) -> Result<(), TransferError> {
        let entries = self.read_registry()?;
        self.require(&entries, id)?;
        let new_name = component(new_name, "folder name")?;
        let root = self.template_dir(id);
        if relative.as_os_str().is_empty() {
            return Err(invalid("the template root itself cannot be renamed"));
        }
        let target = confined(&root, relative, "template folder")?;
        if !target.is_dir() {
            return Err(invalid(format!(
                "template folder {} does not exist",
                relative.display()
            )));
        }
        let renamed = target.with_file_name(&new_name);
        if renamed == target {
            return Ok(());
        }
        if fs::symlink_metadata(&renamed).is_ok() {
            return Err(TransferError::new(
                TransferCategory::Refused,
                Some(&renamed),
                "a folder or file with that name already exists in the template",
            ));
        }
        fs::rename(&target, &renamed).map_err(|error| {
            unavailable(
                &target,
                format!("the template folder cannot be renamed: {error}"),
            )
        })
    }

    /// Adds or removes the sequence/shot marker files of one template folder.
    pub fn set_markers(
        &self,
        id: &str,
        relative: &Path,
        markers: TemplateMarkers,
    ) -> Result<(), TransferError> {
        let entries = self.read_registry()?;
        self.require(&entries, id)?;
        let root = self.template_dir(id);
        let target = confined(&root, relative, "template folder")?;
        if !target.is_dir() {
            return Err(invalid(format!(
                "template folder {} does not exist",
                relative.display()
            )));
        }
        let apply = |file: &str, wanted: bool| -> Result<(), TransferError> {
            let path = target.join(file);
            let present = fs::symlink_metadata(&path).is_ok();
            match (wanted, present) {
                (true, false) => write_atomic(&path, b""),
                (false, true) => fs::remove_file(&path).map_err(|error| {
                    unavailable(&path, format!("the marker file cannot be removed: {error}"))
                }),
                _ => Ok(()),
            }
        };
        apply(structure::SEQUENCE_MARKER_FILE, markers.sequences)?;
        apply(structure::SHOT_MARKER_FILE, markers.shots)?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Validation and file helpers
// ---------------------------------------------------------------------------

/// Registry and folder ids are single path components, never paths.
fn is_plain_component(raw: &str) -> bool {
    !raw.is_empty()
        && raw != "."
        && raw != ".."
        && !raw.contains('/')
        && !raw.contains('\\')
        && !raw.contains('\0')
        && !raw.chars().any(char::is_control)
}

/// One plain, path-safe component for a folder name the user typed.
fn component(raw: &str, what: &str) -> Result<String, TransferError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(invalid(format!("{what} cannot be empty")));
    }
    if !is_plain_component(trimmed) {
        return Err(invalid(format!(
            "{what} {trimmed:?} must be a single folder name without separators or control characters"
        )));
    }
    Ok(trimmed.to_string())
}

/// A template display name is a label, not necessarily a folder name, but the
/// source rejected separators in a new template name; adopt the same rule so a
/// name never looks like a path.
fn display_name(raw: &str) -> Result<String, TransferError> {
    component(raw, "template name")
}

fn confined(root: &Path, relative: &Path, what: &str) -> Result<PathBuf, TransferError> {
    let plain = relative
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    if !plain {
        return Err(invalid(format!(
            "{what} {} must be plain relative components",
            relative.display()
        )));
    }
    Ok(root.join(relative))
}

fn parse_library(bytes: &[u8], path: &Path) -> Result<Vec<TemplateSummary>, TransferError> {
    let file: LibraryFile = serde_json::from_slice(bytes).map_err(|error| {
        invalid(format!(
            "{} is not a template registry this build can read: {error}",
            path.display()
        ))
    })?;
    if file.schema_version != LIBRARY_SCHEMA_VERSION {
        return Err(invalid(format!(
            "{} has schema version {}; this build reads version {}",
            path.display(),
            file.schema_version,
            LIBRARY_SCHEMA_VERSION
        )));
    }
    Ok(file
        .templates
        .into_iter()
        .map(|entry| TemplateSummary {
            id: entry.id,
            name: entry.name,
        })
        .collect())
}

/// Same-directory temporary file plus rename: an interrupted save leaves the
/// previous file untouched instead of a truncated registry.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), TransferError> {
    let Some(parent) = path.parent() else {
        return Err(invalid(format!(
            "{} has no parent folder to write into",
            path.display()
        )));
    };
    fs::create_dir_all(parent)
        .map_err(|error| unavailable(parent, format!("the folder cannot be created: {error}")))?;
    let mut temporary = path
        .file_name()
        .map(OsStr::to_os_string)
        .unwrap_or_default();
    temporary.push(".tmp");
    let temporary = path.with_file_name(temporary);
    if let Err(error) = fs::write(&temporary, bytes) {
        let _ = fs::remove_file(&temporary);
        return Err(unavailable(
            &temporary,
            format!("the file cannot be written: {error}"),
        ));
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(unavailable(
            path,
            format!("the file cannot be replaced atomically: {error}"),
        ));
    }
    Ok(())
}

/// Folder id from a display name: lowercase ASCII alphanumerics with single
/// dashes. A name with no ASCII alphanumerics keeps a stable fallback so ids stay
/// unique and readable.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "template".to_string()
    } else {
        slug
    }
}
