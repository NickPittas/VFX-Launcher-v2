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
//! - the data area is a trusted anchor: every path this module reads or writes is
//!   re-derived from it through [`paths::validate_destination`], so a symlink or
//!   reparse point planted anywhere in `structure/` (registry, template folder,
//!   marker file, staged folder) is refused instead of followed;
//! - the registry and the client list are written through
//!   [`config::write_versioned`] (unique `create_new` temporary name, flush,
//!   native replace, directory sync). A reported write failure is checked against
//!   what is actually on disk before anything is rolled back or claimed;
//! - deleting a template moves its folder aside inside `templates/` first, updates
//!   the registry, and only then drops the staged tree. If the registry update
//!   cannot be proven, the folder is left exactly where it is and its path is
//!   reported — an uncertain delete never removes the only copy;
//! - a malformed `library.json` or `clients.json` is reported and never
//!   overwritten, so hand-edited data is not silently replaced;
//! - directories found under `templates/` but absent from the registry are adopted
//!   on load (sorted by folder name), matching the source's folder-as-template rule;
//! - on a first run (no registry, no adopted folders) the built-in templates from
//!   [`crate::builtin_templates`] are installed, so the administration dialog is
//!   usable without hand-authoring a template first.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::builtin_templates;
use crate::config::{self, ConfigCategory};
use crate::paths;
use crate::paths::metadata_is_link as is_link;
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

/// Prefix of the working folders this module stages under `templates/`. They are
/// never adopted as templates and never dropped by a load.
const STAGING_PREFIX: &str = ".staging-";
const SEEDING_FILE: &str = "seeding.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeedProgress {
    schema_version: u32,
}

fn invalid(message: impl Into<String>) -> TransferError {
    TransferError::new(TransferCategory::InvalidInput, None, message)
}

fn unavailable(path: &Path, message: impl Into<String>) -> TransferError {
    TransferError::new(TransferCategory::Unavailable, Some(path), message)
}

fn unsupported(path: &Path, message: impl Into<String>) -> TransferError {
    TransferError::new(TransferCategory::Unsupported, Some(path), message)
}

fn refused(path: &Path, message: impl Into<String>) -> TransferError {
    TransferError::new(TransferCategory::Refused, Some(path), message)
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
    /// Load-time repairs the user should see (adopted folders, seeded presets,
    /// retained staging folders).
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
    /// Trusted anchor: the launcher data area, exactly as configured. Never
    /// re-derived from anything the administration UI supplies.
    data_dir: PathBuf,
}

impl TemplateLibrary {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
        }
    }

    /// The library folder inside the data area. This is a spelling for display,
    /// not a validated path: mutations go through [`Self::write_below`].
    pub fn root(&self) -> PathBuf {
        self.data_dir.join(STRUCTURE_DIR)
    }

    /// The template folder root, spelled from the anchor. Unvalidated; see
    /// [`Self::root`].
    pub fn templates_dir(&self) -> PathBuf {
        self.root().join(TEMPLATES_DIR)
    }

    /// Folder of one registered template, spelled from the anchor. Unvalidated;
    /// every mutation resolves it again through [`Self::write_below`].
    pub fn template_dir(&self, id: &str) -> PathBuf {
        self.templates_dir().join(id)
    }

    // ---------------------------------------------------------------- anchor

    /// The trusted anchor, with every existing ancestor checked without following
    /// links.
    fn anchor(&self) -> Result<&Path, TransferError> {
        reject_linked_ancestors(&self.data_dir)?;
        Ok(&self.data_dir)
    }

    /// Library-relative spelling of one template folder, rejecting ids that are
    /// not a single plain path component.
    fn template_relative(&self, id: &str) -> Result<PathBuf, TransferError> {
        if !is_plain_component(id) {
            return Err(invalid(format!(
                "template {id:?} is not a usable folder name"
            )));
        }
        Ok(Path::new(TEMPLATES_DIR).join(id))
    }

    /// Resolves `relative` inside the data area for reading. `Ok(None)` means the
    /// data area does not exist yet (first run), never that a file is missing.
    fn read_below(&self, relative: &Path, what: &str) -> Result<Option<PathBuf>, TransferError> {
        let anchor = self.anchor()?;
        let relative = Path::new(STRUCTURE_DIR).join(relative);
        match fs::symlink_metadata(anchor) {
            Ok(meta) => check_anchor_meta(anchor, &meta)?,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(unavailable(
                    anchor,
                    format!("the launcher data folder cannot be inspected: {error}"),
                ));
            }
        }
        paths::validate_destination(anchor, &relative)
            .map(Some)
            .map_err(|error| {
                TransferError::new(
                    TransferCategory::Refused,
                    error.path(),
                    format!("{what} {} cannot be used: {error}", relative.display()),
                )
            })
    }

    /// Resolves `relative` inside the data area for a mutation, creating the data
    /// area when the first run has not created it yet and re-checking it after.
    fn write_below(&self, relative: &Path, what: &str) -> Result<PathBuf, TransferError> {
        let anchor = self.anchor()?.to_path_buf();
        let relative = Path::new(STRUCTURE_DIR).join(relative);
        if fs::symlink_metadata(&anchor).is_err() {
            fs::create_dir_all(&anchor).map_err(|error| {
                unavailable(
                    &anchor,
                    format!("the launcher data folder cannot be created: {error}"),
                )
            })?;
        }
        let meta = fs::symlink_metadata(&anchor).map_err(|error| {
            unavailable(
                &anchor,
                format!("the launcher data folder cannot be inspected: {error}"),
            )
        })?;
        check_anchor_meta(&anchor, &meta)?;
        paths::validate_destination(&anchor, &relative).map_err(|error| {
            TransferError::new(
                TransferCategory::Refused,
                error.path(),
                format!("{what} {} cannot be used: {error}", relative.display()),
            )
        })
    }

    /// The `templates/` folder, created when needed and re-validated afterwards.
    fn templates_folder_for_write(&self) -> Result<PathBuf, TransferError> {
        let what = "the template folder";
        let dir = self.write_below(Path::new(TEMPLATES_DIR), what)?;
        fs::create_dir_all(&dir).map_err(|error| {
            unavailable(
                &dir,
                format!("the template folder cannot be created: {error}"),
            )
        })?;
        // Check the chain again now that it exists: creation must not have run
        // through a link placed after the first check.
        self.write_below(Path::new(TEMPLATES_DIR), what)
    }

    /// The template folder of one id, checked as a real directory.
    fn template_root(&self, id: &str, mutating: bool) -> Result<PathBuf, TransferError> {
        let relative = self.template_relative(id)?;
        let path = if mutating {
            self.write_below(&relative, "the template folder")?
        } else {
            self.read_below(&relative, "the template folder")?
                .ok_or_else(|| {
                    invalid(format!(
                        "template {id:?} has no folder in the launcher library"
                    ))
                })?
        };
        let meta = fs::symlink_metadata(&path).map_err(|error| {
            unavailable(
                &path,
                format!("the template folder cannot be inspected: {error}"),
            )
        })?;
        if is_link(&meta) {
            return Err(unsupported(
                &path,
                "the template folder is a symbolic link; a linked template folder is not used",
            ));
        }
        if !meta.is_dir() {
            return Err(refused(&path, format!("template {id:?} is not a folder")));
        }
        Ok(path)
    }

    /// One relative path below a checked template folder, with the same no-follow
    /// ancestor rules as a destination. An empty relative path is the folder itself.
    fn inside(&self, root: &Path, relative: &Path, what: &str) -> Result<PathBuf, TransferError> {
        if relative.as_os_str().is_empty() {
            return Ok(root.to_path_buf());
        }
        paths::validate_destination(root, relative).map_err(|error| {
            TransferError::new(
                TransferCategory::Refused,
                error.path(),
                format!("{what} {} cannot be used: {error}", relative.display()),
            )
        })
    }

    /// Fresh, unused working name next to `beside`. An existing entry is never
    /// reused or overwritten.
    fn staging_path(&self, beside: &Path) -> Result<PathBuf, TransferError> {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        let parent = beside.parent().ok_or_else(|| {
            invalid(format!(
                "{} has no parent folder to stage a template in",
                beside.display()
            ))
        })?;
        for _ in 0..64 {
            let candidate = parent.join(format!(
                "{STAGING_PREFIX}{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::symlink_metadata(&candidate) {
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(candidate),
                Err(error) => {
                    return Err(unavailable(
                        &candidate,
                        format!("the staging folder cannot be inspected: {error}"),
                    ));
                }
                Ok(_) => continue,
            }
        }
        Err(refused(
            parent,
            "no free staging name could be found in the template folder",
        ))
    }

    // ------------------------------------------------------------------ load

    /// Reads the registry, the client list and any load-time repairs.
    ///
    /// A missing registry is normal (first run): folders already present are
    /// adopted, and an otherwise empty library is seeded with the built-in
    /// templates. A registry or client file this build cannot parse, and any link
    /// inside the library, is an error: nothing is rewritten over it.
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
        let registry_path = self.read_below(Path::new(LIBRARY_FILE), "the template registry")?;
        let mut registry_absent = true;
        let mut entries = match &registry_path {
            Some(path) => match fs::read(path) {
                Ok(bytes) => {
                    registry_absent = false;
                    parse_library(&bytes, path)?
                }
                Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
                Err(error) => {
                    return Err(unavailable(
                        path,
                        format!("the template registry cannot be read: {error}"),
                    ));
                }
            },
            None => Vec::new(),
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
        // every folder under Templates/ as a template. Links are never followed
        // or adopted, and this module's own staging folders are reported instead.
        let mut adopted = Vec::new();
        if let Some(dir) = self.read_below(Path::new(TEMPLATES_DIR), "the template folder")? {
            match fs::read_dir(&dir) {
                Ok(read) => {
                    for entry in read {
                        let entry = entry.map_err(|error| {
                            unavailable(
                                &dir,
                                format!("the template folder cannot be listed: {error}"),
                            )
                        })?;
                        let path = entry.path();
                        let name = entry.file_name();
                        let Some(id) = name.to_str() else {
                            warnings.push(
                                "a template folder name is not valid UTF-8 and was ignored"
                                    .to_string(),
                            );
                            continue;
                        };
                        if !is_plain_component(id) {
                            continue;
                        }
                        if is_staging_name(id) {
                            warnings.push(format!(
                                "staging folder {} from an interrupted template operation was retained",
                                path.display()
                            ));
                            continue;
                        }
                        let meta = fs::symlink_metadata(&path).map_err(|error| {
                            unavailable(
                                &path,
                                format!("the template folder cannot be inspected: {error}"),
                            )
                        })?;
                        if is_link(&meta) || !meta.is_dir() {
                            continue;
                        }
                        if entries.iter().any(|existing| existing.id == id) {
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
                        &dir,
                        format!("the template folder cannot be listed: {error}"),
                    ));
                }
            }
        }

        let mut changed = false;
        if !adopted.is_empty() {
            adopted.sort_by(|a, b| a.name.cmp(&b.name));
            warnings.push(format!(
                "{} template folder(s) found on disk were added to the library",
                adopted.len()
            ));
            entries.extend(adopted);
            changed = true;
        }

        if changed {
            self.commit_registry(&entries)?;
        }
        let seeding_path = self.read_below(Path::new(SEEDING_FILE), "built-in template setup")?;
        let resuming = match seeding_path.as_ref().map(fs::read) {
            Some(Ok(bytes)) => {
                let progress: SeedProgress = serde_json::from_slice(&bytes).map_err(|error| {
                    invalid(format!(
                        "The built-in template setup record cannot be read: {error}"
                    ))
                })?;
                if progress.schema_version != 1 {
                    return Err(invalid(
                        "The built-in template setup record is from an unsupported version.",
                    ));
                }
                true
            }
            Some(Err(error)) if error.kind() != ErrorKind::NotFound => {
                return Err(invalid(format!(
                    "The built-in template setup record cannot be read: {error}"
                )));
            }
            _ => false,
        };
        if resuming || (registry_absent && entries.is_empty()) {
            let path = self.write_below(Path::new(SEEDING_FILE), "built-in template setup")?;
            if !resuming {
                config::write_versioned(
                    &path,
                    "built-in template setup",
                    &SeedProgress { schema_version: 1 },
                )
                .map_err(config_error)?;
            }
            entries = self.seed_builtin_templates(warnings)?;
            fs::remove_file(&path).map_err(|error| unavailable(&path, format!("The built-in templates were installed, but their setup record could not be removed: {error}")))?;
            sync_directory(&self.root())?;
        }
        Ok(entries)
    }

    /// First run: every built-in template becomes a real template folder.
    fn seed_builtin_templates(
        &self,
        warnings: &mut Vec<String>,
    ) -> Result<Vec<TemplateSummary>, TransferError> {
        let mut entries = self.read_registry()?;
        for template in builtin_templates::builtin_templates()? {
            if entries.iter().any(|entry| entry.name == template.name) {
                continue;
            }
            entries = self.install(&template.name, &template)?;
            warnings.push(format!(
                "built-in template {:?} was created in the launcher library",
                template.name
            ));
        }
        Ok(entries)
    }

    fn load_clients(&self) -> Result<Vec<String>, TransferError> {
        let Some(path) = self.read_below(Path::new(CLIENTS_FILE), "the client list")? else {
            return Ok(Vec::new());
        };
        match fs::read(&path) {
            Ok(bytes) => parse_clients(&bytes, &path),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(unavailable(
                &path,
                format!("the client list cannot be read: {error}"),
            )),
        }
    }

    /// Persists the whole client list (validated, deduplicated, sorted).
    pub fn save_clients(&self, clients: &[String]) -> Result<Vec<String>, TransferError> {
        self.load_clients()?;
        let normalized = structure::normalize_clients(clients)?;
        let file = ClientsFile {
            schema_version: structure::CLIENTS_SCHEMA_VERSION,
            clients: normalized.clone(),
        };
        let path = self.write_below(Path::new(CLIENTS_FILE), "the client list")?;
        if let Err(error) = config::write_versioned(&path, "client list", &file) {
            if self.load_clients().as_ref().ok() != Some(&normalized) {
                return Err(config_error(error));
            }
        }
        Ok(normalized)
    }

    /// Merges the names of an existing clients file into this library.
    ///
    /// Both shapes the sources wrote are accepted: the plain JSON array
    /// FolderStructurer's `clients.json` holds, and this module's versioned
    /// envelope. The merged list is validated, sorted and persisted.
    pub fn import_clients(&self, path: &Path) -> Result<Vec<String>, TransferError> {
        let meta = fs::metadata(path).map_err(|error| match error.kind() {
            ErrorKind::NotFound => invalid(format!("{} does not exist", path.display())),
            _ => unavailable(
                path,
                format!("the client list cannot be inspected: {error}"),
            ),
        })?;
        if !meta.is_file() {
            return Err(invalid(format!("{} is not a clients file", path.display())));
        }
        let bytes = fs::read(path).map_err(|error| {
            unavailable(path, format!("the client list cannot be read: {error}"))
        })?;
        let imported = parse_clients(&bytes, path)?;
        let mut merged = self.load_clients()?;
        merged.extend(imported);
        self.save_clients(&merged)
    }

    // -------------------------------------------------------------- snapshot

    /// Reads one template folder into the planning snapshot.
    pub fn snapshot(&self, summary: &TemplateSummary) -> Result<StructureTemplate, TransferError> {
        if !is_plain_component(&summary.id) {
            return Err(invalid(format!(
                "template {:?} is not a usable folder name",
                summary.id
            )));
        }
        let dir = self.template_root(&summary.id, false)?;
        structure::read_template_directory(summary.id.clone(), summary.name.clone(), &dir)
    }

    // ------------------------------------------------------------ registry

    fn read_registry(&self) -> Result<Vec<TemplateSummary>, TransferError> {
        let Some(path) = self.read_below(Path::new(LIBRARY_FILE), "the template registry")? else {
            return Ok(Vec::new());
        };
        match fs::read(&path) {
            Ok(bytes) => parse_library(&bytes, &path),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(unavailable(
                &path,
                format!("the template registry cannot be read: {error}"),
            )),
        }
    }

    /// Writes the registry and reports what really happened.
    ///
    /// [`config::write_versioned`] can fail after the replacement already landed
    /// (its directory sync runs after the rename), so a failure is checked against
    /// the file on disk before a caller rolls anything back.
    fn try_write_registry(&self, entries: &[TemplateSummary]) -> RegistryWrite {
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
        let path = match self.write_below(Path::new(LIBRARY_FILE), "the template registry") {
            Ok(path) => path,
            Err(error) => return RegistryWrite::Failed(error),
        };
        match config::write_versioned(&path, "template registry", &file) {
            Ok(()) => RegistryWrite::Written,
            Err(error) => {
                let reported = config_error(error);
                match fs::read(&path).map(|bytes| parse_library(&bytes, &path)) {
                    Ok(Ok(current)) if same_entries(&current, entries) => RegistryWrite::Confirmed,
                    _ => RegistryWrite::Failed(reported),
                }
            }
        }
    }

    fn commit_registry(&self, entries: &[TemplateSummary]) -> Result<(), TransferError> {
        match self.try_write_registry(entries) {
            RegistryWrite::Written | RegistryWrite::Confirmed => Ok(()),
            RegistryWrite::Failed(error) => Err(error),
        }
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
            || fs::symlink_metadata(self.template_dir(&candidate)).is_ok()
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
        let templates = self.templates_folder_for_write()?;
        let dir = templates.join(&id);
        match fs::symlink_metadata(&dir) {
            Ok(_) => {
                return Err(refused(
                    &dir,
                    "a folder with that template name already exists in the library",
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(unavailable(
                    &dir,
                    format!("the template folder cannot be inspected: {error}"),
                ));
            }
        }
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
        if let Err(error) = self.commit_registry(&entries) {
            // The folder is empty: dropping it cannot lose anything.
            let _ = fs::remove_dir(&dir);
            return Err(error);
        }
        Ok(entries)
    }

    /// Copies a folder tree into the library as a new template. The source is read
    /// through the same template reader planning uses, so a tree that could not be
    /// planned (symlinks, special files) is refused instead of copied half-way.
    pub fn import(&self, source: &Path, name: &str) -> Result<Vec<TemplateSummary>, TransferError> {
        // The reader follows a linked root; refuse it here so "import this folder"
        // never copies a tree the user did not pick.
        let meta = fs::symlink_metadata(source).map_err(|error| {
            unavailable(
                source,
                format!("the folder to import cannot be inspected: {error}"),
            )
        })?;
        if is_link(&meta) {
            return Err(unsupported(
                source,
                "the folder to import is a symbolic link; choose its real folder",
            ));
        }
        if !meta.is_dir() {
            return Err(invalid(format!("{} is not a folder", source.display())));
        }
        let snapshot = structure::read_template_directory("import", "import", source)?;
        self.install(name, &snapshot)
    }

    /// Installs a snapshot as a new template folder. Used by import and by the
    /// built-in templates on a first run with an empty library.
    ///
    /// The tree is written into a staging folder inside `templates/` and renamed
    /// into place only when complete, so a load never adopts a half-written
    /// template and an existing folder is never clobbered.
    pub fn install(
        &self,
        name: &str,
        snapshot: &StructureTemplate,
    ) -> Result<Vec<TemplateSummary>, TransferError> {
        snapshot.validate()?;
        let name = display_name(name)?;
        let mut entries = self.read_registry()?;
        let id = self.unique_id(&name, &entries);
        let templates = self.templates_folder_for_write()?;
        let staging = self.staging_path(&templates.join(&id))?;
        fs::create_dir(&staging).map_err(|error| {
            unavailable(
                &staging,
                format!("the template staging folder cannot be created: {error}"),
            )
        })?;
        if let Err(error) = self.write_snapshot(&staging, snapshot) {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }
        let sync_warning = match self.publish(&staging, &templates, &id) {
            Ok(warning) => warning,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        let dir = templates.join(&id);
        entries.push(TemplateSummary {
            id: id.clone(),
            name: name.clone(),
        });
        if let Err(error) = self.commit_registry(&entries) {
            return Err(unavailable(
                &dir,
                format!(
                    "The template is installed at {}, but its registry entry could not be confirmed: {}. The installed copy was retained; reload the library before retrying.",
                    dir.display(),
                    error.message()
                ),
            ));
        }
        if let Some(message) = sync_warning {
            return Err(unavailable(
                &dir,
                format!(
                    "The template is installed and registered at {}, but folder durability could not be confirmed: {message}. Keep this folder and reload the library.",
                    dir.display()
                ),
            ));
        }
        Ok(entries)
    }

    /// Renames the finished staging folder into its final place, refusing to
    /// replace an existing folder.
    fn publish(
        &self,
        staging: &Path,
        templates: &Path,
        id: &str,
    ) -> Result<Option<String>, TransferError> {
        let dir = templates.join(id);
        match fs::symlink_metadata(&dir) {
            Ok(_) => {
                return Err(refused(
                    &dir,
                    "a template folder with that name already exists in the library",
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(unavailable(
                    &dir,
                    format!("the template folder cannot be inspected: {error}"),
                ));
            }
        }
        fs::rename(staging, &dir).map_err(|error| {
            unavailable(
                &dir,
                format!("the staged template folder cannot be published: {error}"),
            )
        })?;
        Ok(sync_directory(templates)
            .err()
            .map(|error| error.message().to_string()))
    }

    fn write_snapshot(
        &self,
        dir: &Path,
        snapshot: &StructureTemplate,
    ) -> Result<(), TransferError> {
        for entry in &snapshot.entries {
            if !plain_relative(&entry.relative) {
                return Err(invalid(format!(
                    "template entry {} must be plain relative components",
                    entry.relative.display()
                )));
            }
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
                        write_new(&target.join(structure::SEQUENCE_MARKER_FILE), b"")?;
                    }
                    if markers.shots {
                        write_new(&target.join(structure::SHOT_MARKER_FILE), b"")?;
                    }
                }
                TemplateEntryKind::File { contents } => {
                    write_new(&target, contents)?;
                }
                TemplateEntryKind::ProjectTemplateFile {
                    contents,
                    extension,
                } => {
                    let mut file_name = OsString::from("template.");
                    file_name.push(extension);
                    write_new(&target.with_file_name(file_name), contents)?;
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
        self.commit_registry(&entries)?;
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
        self.commit_registry(&entries)?;
        Ok(entries)
    }

    /// Removes a template from the registry and deletes its folder.
    ///
    /// The folder is moved aside first; the staged tree is dropped only after the
    /// registry update is proven. If the update cannot be proven, the folder is
    /// put back or left where it is, and its path is reported.
    pub fn delete(&self, id: &str) -> Result<Vec<TemplateSummary>, TransferError> {
        let mut entries = self.read_registry()?;
        self.require(&entries, id)?;
        let relative = self.template_relative(id)?;
        let dir = self.write_below(&relative, "the template folder")?;
        let staged = match fs::symlink_metadata(&dir) {
            Ok(meta) if meta.is_dir() => {
                let staging = self.staging_path(&dir)?;
                fs::rename(&dir, &staging).map_err(|error| {
                    unavailable(
                        &dir,
                        format!(
                            "the template folder cannot be moved aside before deletion: {error}"
                        ),
                    )
                })?;
                Some(staging)
            }
            Ok(_) => {
                return Err(unsupported(
                    &dir,
                    "the template folder is not a directory; remove it by hand",
                ));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => {
                return Err(unavailable(
                    &dir,
                    format!("the template folder cannot be inspected: {error}"),
                ));
            }
        };
        entries.retain(|entry| entry.id != id);
        match self.try_write_registry(&entries) {
            RegistryWrite::Written | RegistryWrite::Confirmed => {
                if let Some(staged) = &staged {
                    // The registry no longer references this tree: dropping it is
                    // the delete the user asked for. A failure here is reported by
                    // the next load, which lists the retained staging folder.
                    let _ = fs::remove_dir_all(staged);
                }
                Ok(entries)
            }
            RegistryWrite::Failed(error) => {
                let Some(staged) = staged else {
                    return Err(error);
                };
                match self.read_registry() {
                    Ok(current) if current.iter().any(|entry| entry.id == id) => {
                        match fs::rename(&staged, &dir) {
                            Ok(()) => Err(error),
                            Err(restore) => Err(TransferError::new(
                                TransferCategory::Unavailable,
                                Some(&staged),
                                format!(
                                    "the template registry was not updated ({}), and the template folder could not be moved back ({}); its contents are retained at {}",
                                    error.message(),
                                    restore,
                                    staged.display()
                                ),
                            )),
                        }
                    }
                    Ok(_) => Err(TransferError::new(
                        TransferCategory::Unavailable,
                        Some(&staged),
                        format!(
                            "template {id:?} was removed from the registry, but its folder was retained at {} because the registry update reported: {}",
                            staged.display(),
                            error.message()
                        ),
                    )),
                    Err(read) => Err(TransferError::new(
                        TransferCategory::Unavailable,
                        Some(&staged),
                        format!(
                            "the template registry state could not be confirmed ({}), so the folder of template {id:?} was kept at {} and not deleted",
                            read.message(),
                            staged.display()
                        ),
                    )),
                }
            }
        }
    }

    // ------------------------------------------------------- template edits

    /// Creates one folder inside a template, below `parent` (`""` for the root).
    pub fn add_folder(&self, id: &str, parent: &Path, name: &str) -> Result<(), TransferError> {
        let entries = self.read_registry()?;
        self.require(&entries, id)?;
        let name = component(name, "folder name")?;
        let root = self.template_root(id, true)?;
        let parent_dir = self.inside(&root, parent, "template parent folder")?;
        let meta = fs::symlink_metadata(&parent_dir).map_err(|error| {
            unavailable(
                &parent_dir,
                format!("the template folder cannot be inspected: {error}"),
            )
        })?;
        if !meta.is_dir() {
            return Err(invalid(format!(
                "template folder {} does not exist",
                parent.display()
            )));
        }
        let target = parent_dir.join(&name);
        if fs::symlink_metadata(&target).is_ok() {
            return Err(refused(
                &target,
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
        if relative.as_os_str().is_empty() {
            return Err(invalid("the template root itself cannot be removed"));
        }
        let root = self.template_root(id, true)?;
        let target = self.inside(&root, relative, "template folder")?;
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
        if relative.as_os_str().is_empty() {
            return Err(invalid("the template root itself cannot be renamed"));
        }
        let root = self.template_root(id, true)?;
        let target = self.inside(&root, relative, "template folder")?;
        let meta = fs::symlink_metadata(&target).map_err(|error| {
            unavailable(
                &target,
                format!("the template folder cannot be inspected: {error}"),
            )
        })?;
        if !meta.is_dir() {
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
            return Err(refused(
                &renamed,
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
        let root = self.template_root(id, true)?;
        let target = self.inside(&root, relative, "template folder")?;
        let meta = fs::symlink_metadata(&target).map_err(|error| {
            unavailable(
                &target,
                format!("the template folder cannot be inspected: {error}"),
            )
        })?;
        if !meta.is_dir() {
            return Err(invalid(format!(
                "template folder {} does not exist",
                relative.display()
            )));
        }
        let apply = |file: &str, wanted: bool| -> Result<(), TransferError> {
            let path = target.join(file);
            let present = fs::symlink_metadata(&path);
            match (wanted, present) {
                (true, Err(error)) if error.kind() == ErrorKind::NotFound => write_new(&path, b""),
                (true, Ok(meta)) if is_link(&meta) => Err(unsupported(
                    &path,
                    "the marker file is a symbolic link; refusing to replace it",
                )),
                (true, Ok(_)) => Ok(()),
                (false, Err(error)) if error.kind() == ErrorKind::NotFound => Ok(()),
                (false, Ok(meta)) if is_link(&meta) => Err(unsupported(
                    &path,
                    "the marker file is a symbolic link; remove it by hand",
                )),
                (false, Ok(_)) => fs::remove_file(&path).map_err(|error| {
                    unavailable(&path, format!("the marker file cannot be removed: {error}"))
                }),
                (_, Err(error)) => Err(unavailable(
                    &path,
                    format!("the marker file cannot be inspected: {error}"),
                )),
            }
        };
        apply(structure::SEQUENCE_MARKER_FILE, markers.sequences)?;
        apply(structure::SHOT_MARKER_FILE, markers.shots)?;
        Ok(())
    }
}

/// Outcome of a registry write, including the "failed but already on disk" case.
enum RegistryWrite {
    /// The helper reported success.
    Written,
    /// The helper reported a failure, and the registry on disk is already the
    /// value that was written.
    Confirmed,
    /// Nothing can be proven about the registry; the reported error stands.
    Failed(TransferError),
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

/// This module's own working folder names, never adopted as templates.
fn is_staging_name(raw: &str) -> bool {
    raw.starts_with(STAGING_PREFIX)
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

/// Plain relative components only: no root, no `.`/`..`, no prefix.
fn plain_relative(relative: &Path) -> bool {
    relative.as_os_str().is_empty()
        || relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

/// The data area itself must be a real directory: a linked data area is refused
/// rather than followed, because everything this module owns lives below it.
fn check_anchor_meta(anchor: &Path, meta: &fs::Metadata) -> Result<(), TransferError> {
    if is_link(meta) {
        return Err(unsupported(
            anchor,
            "the launcher data folder is a symbolic link; a linked data area is not used",
        ));
    }
    if !meta.is_dir() {
        return Err(refused(
            anchor,
            "the launcher data folder is not a directory",
        ));
    }
    Ok(())
}

/// Checks every existing ancestor of the data area without following links.
/// Missing components are skipped but their parents are still checked, so a link
/// above a folder that does not exist yet cannot hide a write outside the area.
fn reject_linked_ancestors(path: &Path) -> Result<(), TransferError> {
    if !path.is_absolute() {
        return Err(invalid(format!(
            "the launcher data folder {} must be an absolute path",
            path.display()
        )));
    }
    if path
        .components()
        .any(|part| matches!(part, Component::CurDir | Component::ParentDir))
    {
        return Err(invalid(format!(
            "the launcher data folder {} must not contain '.' or '..'",
            path.display()
        )));
    }
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if is_link(&meta) {
                    return Err(unsupported(
                        ancestor,
                        "a symbolic link sits in the launcher data folder path; a linked data area is not used",
                    ));
                }
                if !meta.is_dir() {
                    return Err(refused(
                        ancestor,
                        "the launcher data folder path runs through a non-directory",
                    ));
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(unavailable(
                    ancestor,
                    format!("the launcher data folder path cannot be inspected: {error}"),
                ));
            }
        }
    }
    Ok(())
}

/// Parses one clients file: the legacy plain JSON array FolderStructurer wrote,
/// or this module's versioned envelope.
fn parse_clients(bytes: &[u8], path: &Path) -> Result<Vec<String>, TransferError> {
    if let Ok(legacy) = serde_json::from_slice::<Vec<String>>(bytes) {
        return structure::normalize_clients(&legacy);
    }
    let file: ClientsFile = serde_json::from_slice(bytes).map_err(|error| {
        invalid(format!(
            "{} is not a client list this build can read: {error}",
            path.display()
        ))
    })?;
    if file.schema_version != structure::CLIENTS_SCHEMA_VERSION {
        return Err(invalid(format!(
            "{} has schema version {}; this build reads version {}",
            path.display(),
            file.schema_version,
            structure::CLIENTS_SCHEMA_VERSION
        )));
    }
    structure::normalize_clients(&file.clients)
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

/// Registry identity used by the read-back check: same ids and names, same order.
fn same_entries(current: &[TemplateSummary], intended: &[TemplateSummary]) -> bool {
    current.len() == intended.len()
        && current
            .iter()
            .zip(intended)
            .all(|(a, b)| a.id == b.id && a.name == b.name)
}

fn config_error(error: config::ConfigError) -> TransferError {
    let category = match error.category() {
        ConfigCategory::Io => TransferCategory::Unavailable,
        ConfigCategory::InvalidJson
        | ConfigCategory::UnsupportedVersion
        | ConfigCategory::InvalidValue => TransferCategory::InvalidInput,
    };
    TransferError::new(category, error.path(), error.message().to_string())
}

/// Writes one new file: `create_new` never truncates or follows an existing
/// entry, and the flush makes the staged tree durable before it is published.
fn write_new(path: &Path, bytes: &[u8]) -> Result<(), TransferError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            unavailable(parent, format!("the folder cannot be created: {error}"))
        })?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| unavailable(path, format!("the file cannot be created: {error}")))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| unavailable(path, format!("the file cannot be written: {error}")))
}

/// Flushes a directory entry after files or folders inside it were renamed, so a
/// completed operation survives a crash. No claim is made where the platform
/// cannot flush a directory.
#[cfg(unix)]
fn sync_directory(dir: &Path) -> Result<(), TransferError> {
    File::open(dir)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            unavailable(
                dir,
                format!("the folder entry could not be synced: {error}"),
            )
        })
}

#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> Result<(), TransferError> {
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
