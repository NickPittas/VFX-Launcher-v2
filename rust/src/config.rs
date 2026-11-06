//! F2 configuration: versioned JSON settings and import-selection state.
//!
//! This module owns *user* configuration and the crash-safe file protocol it is
//! stored with — nothing else in the crate writes settings. It does not import
//! legacy INI/JSON input (that is the F3 `import` packet) and it never stores
//! provider credentials, tokens or passwords: AI authentication stays outside
//! this application (see `PLAN.md` decisions 3 and 4).
//!
//! Storage protocol (per `CONTRACTS.md` §3):
//! - one JSON envelope per file with `schema_version`;
//! - an unknown/older/newer version or malformed JSON is a readable error and
//!   the previous file stays untouched;
//! - a missing file loads the defaults (first run, not an error);
//! - saves go to a sibling temporary file in the destination directory, are
//!   flushed and `sync_all`ed, then replaced atomically, so a crash leaves either
//!   the old or the new file, never a truncated one. On Unix the rename is
//!   followed by a directory `sync_all`; on Windows the flush plus the native
//!   replace are the whole guarantee, because those APIs document no directory
//!   metadata flush (`ReplaceFileW`'s write-through flag is documented as not
//!   supported) and a plain `rename` never flushes directory metadata by itself.
//!
//! Path fields are UTF-8 strings in JSON (`CONTRACTS.md` §1). A native path that
//! is not representable as UTF-8 is refused at save time via
//! [`paths::utf8_spelling`](crate::paths::utf8_spelling) instead of being stored
//! lossily; the catalog keeps the lossless bytes for such paths.

use std::ffi::{OsStr, OsString};
use std::fmt;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Envelope version of `settings.json`. Bump only together with a load-time
/// migration or an explicit rejection of the old value.
pub const SETTINGS_SCHEMA_VERSION: u32 = 1;
/// Envelope version of the import-selection file.
pub const IMPORT_STATE_SCHEMA_VERSION: u32 = 1;

/// Result alias for module-owned configuration errors.
pub type ConfigResult<T> = Result<T, ConfigError>;

/// Stable configuration failure categories with readable causes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigCategory {
    /// File system read/write/rename/permission failure.
    Io,
    /// The file exists but cannot be parsed as JSON at all.
    InvalidJson,
    /// `schema_version` is missing, or older/newer than this build supports.
    UnsupportedVersion,
    /// Well-formed JSON that is not a valid value for this build (including a
    /// path that cannot be represented as UTF-8).
    InvalidValue,
}

impl ConfigCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::Io => "file I/O error",
            Self::InvalidJson => "invalid settings JSON",
            Self::UnsupportedVersion => "unsupported settings version",
            Self::InvalidValue => "invalid settings value",
        }
    }
}

/// Module-owned configuration error: category, affected file (when known) and a
/// cause a user can read.
#[derive(Clone, Debug)]
pub struct ConfigError {
    category: ConfigCategory,
    path: Option<PathBuf>,
    message: String,
}

impl ConfigError {
    pub fn new(category: ConfigCategory, path: Option<&Path>, message: impl Into<String>) -> Self {
        Self {
            category,
            path: path.map(Path::to_path_buf),
            message: message.into(),
        }
    }

    pub fn category(&self) -> ConfigCategory {
        self.category
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for ConfigError {
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

impl std::error::Error for ConfigError {}

/// A path stored in JSON. Serialization refuses non-UTF-8 native paths rather
/// than replacing bytes; deserialization accepts any string.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JsonPath(PathBuf);

impl JsonPath {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl From<PathBuf> for JsonPath {
    fn from(path: PathBuf) -> Self {
        Self(path)
    }
}

impl From<&Path> for JsonPath {
    fn from(path: &Path) -> Self {
        Self(path.to_path_buf())
    }
}

impl Serialize for JsonPath {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error as _;
        match crate::paths::utf8_spelling(&self.0) {
            Some(text) => serializer.serialize_str(text),
            None => Err(S::Error::custom(
                "path is not valid UTF-8; it cannot be stored in JSON without losing bytes",
            )),
        }
    }
}

impl<'de> Deserialize<'de> for JsonPath {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self(PathBuf::from(String::deserialize(deserializer)?)))
    }
}

/// Log verbosity selection. Levels exist from the start so logging call sites
/// do not need a second settings migration.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
}

impl LogLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }
}

/// External application commands. Mirrors the legacy INI `Paths` values; no new
/// option is invented here.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Applications {
    pub nuke_path: Option<JsonPath>,
    pub after_effects_path: Option<JsonPath>,
    /// Legacy Linux Nuke launch command. Tokenized at launch time (P2), never
    /// executed through a shell.
    pub nuke_launch_cmd_linux: Option<String>,
}

/// Project registration preferences (legacy `project_directories`, a
/// comma-separated list of directory names, is stored as a list here).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectDiscovery {
    pub project_directory_names: Vec<String>,
}

impl Default for ProjectDiscovery {
    fn default() -> Self {
        Self {
            project_directory_names: ["project", "projects", "Project", "Projects"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        }
    }
}

/// Versioned application settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u32,
    #[serde(default)]
    pub applications: Applications,
    #[serde(default)]
    pub project_discovery: ProjectDiscovery,
    /// Chosen identity for attribution, by username: the legacy default is the
    /// `admin` record, and its row id is never assumed to be 1.
    #[serde(default)]
    pub selected_username: Option<String>,
    #[serde(default)]
    pub log_level: LogLevel,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SETTINGS_SCHEMA_VERSION,
            applications: Applications::default(),
            project_discovery: ProjectDiscovery::default(),
            selected_username: None,
            log_level: LogLevel::default(),
        }
    }
}

/// The single legacy source selected for the next import. One source per import;
/// unrelated installations are never merged silently (`CONTRACTS.md` §3). The
/// F3 importer fills `last_import_at`; this module only stores the choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportState {
    pub schema_version: u32,
    #[serde(default)]
    pub selected_source: Option<JsonPath>,
    #[serde(default)]
    pub last_import_at: Option<String>,
}

impl Default for ImportState {
    fn default() -> Self {
        Self {
            schema_version: IMPORT_STATE_SCHEMA_VERSION,
            selected_source: None,
            last_import_at: None,
        }
    }
}

/// Reader/writer for the settings file.
#[derive(Clone, Debug)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the settings, or the defaults when the file does not exist. An
    /// unreadable, malformed, wrongly-versioned or invalid file is an error; it
    /// is never replaced by the defaults.
    pub fn load(&self) -> ConfigResult<Settings> {
        let settings = read_versioned(&self.path, "settings file", SETTINGS_SCHEMA_VERSION)?
            .unwrap_or_default();
        validate_settings(&settings)?;
        Ok(settings)
    }

    /// Writes the settings atomically. Validation runs before any file is
    /// touched, so a rejected value leaves the previous file in place.
    pub fn save(&self, settings: &Settings) -> ConfigResult<()> {
        validate_settings(settings)?;
        write_versioned(&self.path, "settings file", settings)
    }
}

/// Reader/writer for the import-selection file.
#[derive(Clone, Debug)]
pub struct ImportStateStore {
    path: PathBuf,
}

impl ImportStateStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> ConfigResult<ImportState> {
        Ok(read_versioned(
            &self.path,
            "import selection file",
            IMPORT_STATE_SCHEMA_VERSION,
        )?
        .unwrap_or_default())
    }

    pub fn save(&self, state: &ImportState) -> ConfigResult<()> {
        if state.schema_version != IMPORT_STATE_SCHEMA_VERSION {
            return Err(ConfigError::new(
                ConfigCategory::InvalidValue,
                Some(&self.path),
                format!(
                    "refusing to write the import selection with schema_version {}; this build writes {IMPORT_STATE_SCHEMA_VERSION}",
                    state.schema_version
                ),
            ));
        }
        write_versioned(&self.path, "import selection file", state)
    }
}

/// Platform data directory for application-owned files, independent of any
/// legacy installation: `$XDG_DATA_HOME/vfx-launcher` (or
/// `$HOME/.local/share/vfx-launcher`) on Unix, `%APPDATA%\VFX Launcher` on
/// Windows. Relative or empty environment values are refused because the
/// catalog path must be absolute.
pub fn default_data_dir() -> ConfigResult<PathBuf> {
    #[cfg(windows)]
    {
        let app_data = std::env::var_os("APPDATA").filter(|value| !value.is_empty());
        let app_data = app_data.ok_or_else(|| {
            ConfigError::new(
                ConfigCategory::Io,
                None,
                "APPDATA is not set, so the application data directory cannot be determined",
            )
        })?;
        let root = PathBuf::from(app_data);
        return absolute_dir(root.join("VFX Launcher"));
    }
    #[cfg(not(windows))]
    {
        let xdg = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty());
        let base = match xdg {
            Some(value) => PathBuf::from(value),
            None => {
                let home = std::env::var_os("HOME").filter(|value| !value.is_empty());
                let home = home.ok_or_else(|| {
                    ConfigError::new(
                        ConfigCategory::Io,
                        None,
                        "neither XDG_DATA_HOME nor HOME is set, so the application data directory cannot be determined",
                    )
                })?;
                PathBuf::from(home).join(".local/share")
            }
        };
        absolute_dir(base.join("vfx-launcher"))
    }
}

pub fn default_settings_path() -> ConfigResult<PathBuf> {
    Ok(default_data_dir()?.join("settings.json"))
}

pub fn default_import_state_path() -> ConfigResult<PathBuf> {
    Ok(default_data_dir()?.join("import-selection.json"))
}

/// Default local catalog file. Never a legacy database path: importing legacy
/// data reads that source and writes a separate new catalog (`PLAN.md` decision 2).
pub fn default_catalog_path() -> ConfigResult<PathBuf> {
    Ok(default_data_dir()?.join("catalog.sqlite3"))
}

/// Envelope version of `active-workspace.json`, the pointer to the workspace
/// layout an import adopted.
pub const WORKSPACE_SCHEMA_VERSION: u32 = 1;

/// File name of the adopted-workspace pointer, beside `settings.json` and
/// `catalog.sqlite3` in the data root.
pub const WORKSPACE_MANIFEST_FILE: &str = "active-workspace.json";

/// Which directory under the data root this installation owns after an import.
///
/// The pointer is a separate file so that an installation which never imported
/// anything keeps the original flat layout and needs no migration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceManifest {
    pub schema_version: u32,
    /// Workspace directory relative to the data root. Absolute spellings and
    /// `..` components are refused, never normalized away.
    pub workspace_relative: JsonPath,
}

/// Path of the adopted-workspace pointer for `data_root`.
pub fn workspace_manifest_path(data_root: &Path) -> PathBuf {
    data_root.join(WORKSPACE_MANIFEST_FILE)
}

/// Resolves the directory this installation reads its catalog, settings and
/// reports from.
///
/// - no manifest: `data_root` itself, the original layout. A first run and an
///   installation that never imported anything are unchanged, and nothing is
///   created here.
/// - a manifest naming a workspace: `data_root/<workspace_relative>` after
///   [`paths::validate_destination`](crate::paths::validate_destination), so an
///   absolute spelling, a `..` component, a NUL, an invalid native name or a
///   symlink/reparse ancestor is refused rather than resolved. The directory
///   must exist.
/// - a malformed manifest, a version this build does not support, an unusable
///   spelling or a workspace that is gone is an error. It never resolves to a
///   fresh empty layout, because a lost workspace would then look like a lost
///   catalog instead of being reported.
///
/// Callers open the catalog, the settings and the reports only from the
/// returned directory, and only after this returns `Ok`.
pub fn resolve_workspace(data_root: &Path) -> ConfigResult<PathBuf> {
    require_absolute(data_root, "data root")?;
    let manifest_path = workspace_manifest_path(data_root);
    let manifest = read_versioned::<WorkspaceManifest>(
        &manifest_path,
        "workspace manifest",
        WORKSPACE_SCHEMA_VERSION,
    )?;
    let Some(manifest) = manifest else {
        return Ok(data_root.to_path_buf());
    };
    let workspace =
        crate::paths::validate_destination(data_root, manifest.workspace_relative.as_path())
            .map_err(|error| {
                ConfigError::new(
                    ConfigCategory::InvalidValue,
                    Some(&manifest_path),
                    format!("the workspace manifest does not name a usable directory: {error}"),
                )
            })?;
    require_prepared_workspace(
        manifest.workspace_relative.as_path(),
        &workspace,
        &manifest_path,
    )?;
    Ok(workspace)
}

/// Records `prepared_directory` as the adopted workspace, atomically.
///
/// Validation runs before the pointer is written: both paths must be absolute,
/// the prepared directory must exist, and its spelling relative to `data_root`
/// must survive [`paths::validate_destination`](crate::paths::validate_destination)
/// — no absolute or `..` component, no NUL, valid native names, and no
/// symlink/reparse point among its existing ancestors.
///
/// Nothing is copied, moved, rolled back or deleted here. Validation failures
/// leave the selection unchanged. A write error can follow atomic replacement
/// when the directory flush fails; the caller must reread the selector and report
/// uncertain durability, never claim rollback or delete the prepared workspace. The caller opens the catalog, the
/// settings and the reports only after this returns `Ok`, so a prepared
/// workspace that cannot be opened is reported instead of being replaced by an
/// empty layout.
///
/// The two arguments must be spelled from the same root: the relative name is
/// computed lexically, so a differently spelled data root is refused.
pub fn adopt_workspace(data_root: &Path, prepared_directory: &Path) -> ConfigResult<()> {
    require_absolute(data_root, "data root")?;
    require_absolute(prepared_directory, "prepared workspace")?;
    let relative = prepared_directory.strip_prefix(data_root)
        .map_err(|_| {
            ConfigError::new(
                ConfigCategory::InvalidValue,
                Some(prepared_directory),
                format!(
                    "the prepared workspace is not inside the data root {}; refusing to adopt a directory this application does not own",
                    data_root.display()
                ),
            )
        })?;
    if relative.as_os_str().is_empty() {
        return Err(ConfigError::new(
            ConfigCategory::InvalidValue,
            Some(prepared_directory),
            "the prepared workspace is the data root itself; there is nothing to adopt and the original layout already applies",
        ));
    }
    let workspace = crate::paths::validate_destination(data_root, relative).map_err(|error| {
        ConfigError::new(
            ConfigCategory::InvalidValue,
            Some(prepared_directory),
            format!("the prepared workspace cannot be adopted: {error}"),
        )
    })?;
    require_prepared_workspace(relative, &workspace, prepared_directory)?;
    let manifest = WorkspaceManifest {
        schema_version: WORKSPACE_SCHEMA_VERSION,
        workspace_relative: JsonPath::from(relative.to_path_buf()),
    };
    write_versioned(
        &workspace_manifest_path(data_root),
        "workspace manifest",
        &manifest,
    )
}

fn require_absolute(path: &Path, what: &str) -> ConfigResult<()> {
    if !path.is_absolute() {
        return Err(ConfigError::new(
            ConfigCategory::InvalidValue,
            Some(path),
            format!(
                "the {what} must be an absolute path; a relative path would be resolved against the process working directory"
            ),
        ));
    }
    Ok(())
}

fn require_prepared_workspace(
    relative: &Path,
    directory: &Path,
    reported: &Path,
) -> ConfigResult<()> {
    let mut components = relative.components();
    if components.next()
        != Some(std::path::Component::Normal(std::ffi::OsStr::new(
            "imports",
        )))
        || components.next().is_none()
    {
        return Err(ConfigError::new(
            ConfigCategory::InvalidValue,
            Some(reported),
            "The selected workspace must be a prepared import below the application's imports folder.",
        ));
    }
    require_existing_directory(directory, reported)?;
    for name in [
        "catalog.sqlite3",
        "settings.json",
        "report.json",
        "capture.json",
    ] {
        let path =
            crate::paths::validate_destination(directory, Path::new(name)).map_err(|error| {
                ConfigError::new(
                    ConfigCategory::InvalidValue,
                    Some(reported),
                    format!("The prepared workspace contains an unsafe {name} path: {error}"),
                )
            })?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            ConfigError::new(
                ConfigCategory::Io,
                Some(&path),
                format!("The prepared workspace is incomplete: {error}"),
            )
        })?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ConfigError::new(
                ConfigCategory::InvalidValue,
                Some(&path),
                "A prepared workspace file is missing or is not a regular file. The current workspace remains selected.",
            ));
        }
    }
    Ok(())
}

fn require_existing_directory(directory: &Path, reported: &Path) -> ConfigResult<()> {
    match fs::metadata(directory) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(ConfigError::new(
            ConfigCategory::InvalidValue,
            Some(reported),
            format!(
                "the adopted workspace {} is not a directory; it is reported instead of falling back to an empty layout",
                directory.display()
            ),
        )),
        Err(err) if err.kind() == ErrorKind::NotFound => Err(ConfigError::new(
            ConfigCategory::Io,
            Some(directory),
            "the adopted workspace is missing; it is reported instead of falling back to an empty layout",
        )),
        Err(err) => Err(ConfigError::new(
            ConfigCategory::Io,
            Some(directory),
            format!("the adopted workspace cannot be inspected: {err}"),
        )),
    }
}

fn absolute_dir(path: PathBuf) -> ConfigResult<PathBuf> {
    if !path.is_absolute() {
        return Err(ConfigError::new(
            ConfigCategory::InvalidValue,
            Some(&path),
            "the application data directory must be an absolute path",
        ));
    }
    Ok(path)
}

fn validate_settings(settings: &Settings) -> ConfigResult<()> {
    if settings.schema_version != SETTINGS_SCHEMA_VERSION {
        return Err(ConfigError::new(
            ConfigCategory::InvalidValue,
            None,
            format!(
                "refusing to write settings with schema_version {}; this build writes {SETTINGS_SCHEMA_VERSION}",
                settings.schema_version
            ),
        ));
    }
    if let Some(username) = &settings.selected_username {
        if username.trim().is_empty() {
            return Err(ConfigError::new(
                ConfigCategory::InvalidValue,
                None,
                "the selected user name cannot be blank",
            ));
        }
    }
    for name in &settings.project_discovery.project_directory_names {
        let trimmed = name.trim();
        let rejects = trimmed.is_empty()
            || trimmed != name
            || name.contains('/')
            || name.contains('\\')
            || name.chars().any(char::is_control);
        if rejects {
            return Err(ConfigError::new(
                ConfigCategory::InvalidValue,
                None,
                format!(
                    "project directory name {name:?} must be one non-empty directory name without separators or surrounding spaces"
                ),
            ));
        }
    }
    Ok(())
}

/// Reads and version-checks one JSON envelope. `Ok(None)` means "no file yet".
fn read_versioned<T: DeserializeOwned>(
    path: &Path,
    kind: &str,
    supported: u32,
) -> ConfigResult<Option<T>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(ConfigError::new(
                ConfigCategory::Io,
                Some(path),
                format!("could not read the {kind}: {err}"),
            ));
        }
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).map_err(|err| {
        ConfigError::new(
            ConfigCategory::InvalidJson,
            Some(path),
            format!("the {kind} is not valid JSON: {err}"),
        )
    })?;
    let version = match value.get("schema_version") {
        None => {
            return Err(ConfigError::new(
                ConfigCategory::UnsupportedVersion,
                Some(path),
                format!(
                    "the {kind} has no schema_version field; this build supports schema version {supported}"
                ),
            ));
        }
        Some(field) => field.as_u64().ok_or_else(|| {
            ConfigError::new(
                ConfigCategory::InvalidValue,
                Some(path),
                format!(
                    "the {kind} has a non-integer schema_version ({field}); this build supports schema version {supported}"
                ),
            )
        })?,
    };
    if version != u64::from(supported) {
        let cause = if version > u64::from(supported) {
            format!(
                "the {kind} was written by a newer version (schema_version {version}); this build supports {supported}"
            )
        } else {
            format!(
                "the {kind} has unsupported schema_version {version}; this build supports {supported}"
            )
        };
        return Err(ConfigError::new(
            ConfigCategory::UnsupportedVersion,
            Some(path),
            cause,
        ));
    }
    let parsed = serde_json::from_value(value).map_err(|err| {
        ConfigError::new(
            ConfigCategory::InvalidValue,
            Some(path),
            format!("the {kind} does not match schema version {supported}: {err}"),
        )
    })?;
    Ok(Some(parsed))
}

pub(crate) fn write_versioned<T: Serialize>(
    path: &Path,
    kind: &str,
    value: &T,
) -> ConfigResult<()> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|err| {
        ConfigError::new(
            ConfigCategory::InvalidValue,
            Some(path),
            format!("the {kind} cannot be encoded as JSON: {err}"),
        )
    })?;
    bytes.push(b'\n');
    write_atomic(path, &bytes)
}

/// Same-directory temporary file, flush, `sync_all`, native replace, directory
/// sync where the platform supports it.
///
/// Durability, stated exactly:
/// - the new bytes are `sync_all`ed before anything replaces the destination, so
///   a successful return never reports contents that are still only in the page
///   cache;
/// - on Unix the replacement is `rename(2)` followed by a directory `sync_all`,
///   which is what makes the *name change* durable;
/// - on Windows the replacement is `replace_file`: `ReplaceFileW` when the
///   destination exists (it keeps the destination's identity, attributes and
///   ACLs) or `MoveFileExW` with `MOVEFILE_WRITE_THROUGH` when it does not. No
///   directory-metadata flush is claimed there: `ReplaceFileW`'s write-through
///   flag is documented as not supported, and neither API documents flushing the
///   parent directory entry.
fn write_atomic(path: &Path, bytes: &[u8]) -> ConfigResult<()> {
    static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    let parent = parent.ok_or_else(|| {
        ConfigError::new(
            ConfigCategory::InvalidValue,
            Some(path),
            "the settings path has no parent directory",
        )
    })?;
    fs::create_dir_all(parent).map_err(|err| {
        ConfigError::new(
            ConfigCategory::Io,
            Some(parent),
            format!("could not create the settings directory: {err}"),
        )
    })?;

    let mut name = path
        .file_name()
        .map(OsStr::to_os_string)
        .unwrap_or_else(|| OsString::from("settings"));
    name.push(format!(
        ".{}.{}.tmp",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let temporary = parent.join(name);

    // Never truncate or follow a pre-existing temporary path.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|err| {
            ConfigError::new(
                ConfigCategory::Io,
                Some(&temporary),
                format!("could not create the temporary settings file: {err}"),
            )
        })?;
    let written = (|| {
        file.write_all(bytes)?;
        file.sync_all()
    })();
    drop(file);
    if let Err(err) = written {
        let _ = fs::remove_file(&temporary);
        return Err(ConfigError::new(
            ConfigCategory::Io,
            Some(&temporary),
            format!("could not write the temporary settings file: {err}"),
        ));
    }
    if let Err(err) = replace_file(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(ConfigError::new(
            ConfigCategory::Io,
            Some(path),
            format!("could not replace the settings file: {err}"),
        ));
    }
    sync_parent_directory(parent, path)?;
    Ok(())
}

/// Windows replacement: `ReplaceFileW` requires the destination to exist, so a
/// missing destination falls back to `MoveFileExW` with write-through.
#[cfg(windows)]
fn replace_file(temporary: &Path, path: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, GetLastError};
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_WRITE_THROUGH, MoveFileExW, ReplaceFileW,
    };

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let replaced = wide(path);
    let replacement = wide(temporary);
    // SAFETY: both buffers are NUL-terminated UTF-16 strings that outlive the
    // call; no backup name and no dedicated exclude/reserved blocks are passed.
    let replaced_ok = unsafe {
        ReplaceFileW(
            replaced.as_ptr(),
            replacement.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if replaced_ok != 0 {
        return Ok(());
    }
    // SAFETY: GetLastError reads the calling thread's last error, which the
    // immediately preceding API call set.
    let replace_error = unsafe { GetLastError() };
    if replace_error != ERROR_FILE_NOT_FOUND {
        return Err(std::io::Error::from_raw_os_error(replace_error as i32));
    }
    // SAFETY: same NUL-terminated buffers; MOVEFILE_WRITE_THROUGH makes the call
    // return only once the file is actually on disk.
    let moved = unsafe {
        MoveFileExW(
            replacement.as_ptr(),
            replaced.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved != 0 {
        return Ok(());
    }
    // SAFETY: as above; MoveFileExW set the thread error on failure.
    let move_error = unsafe { GetLastError() };
    Err(std::io::Error::from_raw_os_error(move_error as i32))
}

/// Unix replacement: `rename(2)` is atomic within the directory, and the caller
/// syncs the directory afterwards.
#[cfg(not(windows))]
fn replace_file(temporary: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(temporary, path)
}

#[cfg(unix)]
fn sync_parent_directory(parent: &Path, path: &Path) -> ConfigResult<()> {
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|err| {
            ConfigError::new(
                ConfigCategory::Io,
                Some(path),
                format!(
                    "the settings file was renamed but the directory could not be synced: {err}"
                ),
            )
        })
}

#[cfg(not(unix))]
fn sync_parent_directory(_parent: &Path, _path: &Path) -> ConfigResult<()> {
    // No documented way to flush a directory entry on Windows: `ReplaceFileW`'s
    // write-through flag is documented as unsupported and `MoveFileExW`'s
    // write-through covers the moved file itself. The new bytes are already
    // `sync_all`ed and the replace is done by `replace_file`, so no
    // directory-durability claim is made here.
    Ok(())
}
