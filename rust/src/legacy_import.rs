//! Read-only legacy capture and staged catalog conversion. Sources are never initialized.
//!
//! Three legacy inputs are imported into one staged area: the SQLite catalog
//! (read inside a consistent snapshot), `app_settings.ini` through
//! [`crate::legacy_ini::import_settings`], and the per-user
//! `favorites_<username>.json` sidecars. Sidecar favorites are unioned with the
//! SQL `favorite_projects` rows: a numeric entry tries the legacy project ID
//! first, an absolute path is matched against the imported roots on its own
//! platform, and every other or unmatched value is reported instead of guessed.
//! Malformed INI and favorites envelopes refuse preparation before staging.
use crate::jobs::{Checkpoint, JobControl, JobEvents, JobPhase, JobProgress};
use crate::storage::{Catalog, PathColumns, StorageCategory, StorageError};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params, types::ValueRef};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const TABLES: [&str; 7] = [
    "users",
    "projects",
    "project_files",
    "favorite_projects",
    "recent_projects",
    "file_access_log",
    "user_activity_log",
];

#[derive(Clone, Debug)]
pub struct LegacySources {
    pub database: PathBuf,
    pub ini: Option<PathBuf>,
    pub favorites: Vec<PathBuf>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum LegacyValue {
    Null,
    Integer(i64),
    /// Preserve the exact SQLite float, including non-finite bit patterns.
    RealBits(u64),
    Text(String),
    InvalidText(Vec<u8>),
    Blob(Vec<u8>),
}
type LegacyRow = BTreeMap<String, LegacyValue>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CapturedSidecar {
    pub path: crate::transfers::StoredPath,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyCapture {
    pub schema_version: u32,
    pub database: crate::transfers::StoredPath,
    pub captured_at: String,
    pub tables: BTreeMap<String, Vec<LegacyRow>>,
    pub ini: Option<CapturedSidecar>,
    pub favorites: Vec<CapturedSidecar>,
}

/// One read transaction covers schema and all rows; sidecars are read once and
/// refused if their held file changes during capture.
pub fn capture(
    sources: &LegacySources,
    control: &JobControl,
    events: &JobEvents,
) -> Result<LegacyCapture, String> {
    if !sources.database.is_absolute() {
        return Err("Choose an absolute legacy database path.".into());
    }
    let held = File::open(&sources.database)
        .map_err(|error| format!("The legacy database could not be opened: {error}"))?;
    let before = held.metadata().map_err(|error| error.to_string())?;
    if !before.is_file() {
        return Err("The selected legacy database is not a regular file.".into());
    }
    let mut connection = Connection::open_with_flags(
        &sources.database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        format!("The legacy database could not be read without modifying it: {error}")
    })?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|error| error.to_string())?;
    connection
        .pragma_update(None, "query_only", true)
        .map_err(|error| error.to_string())?;
    let transaction = connection
        .transaction()
        .map_err(|error| error.to_string())?;
    let mut tables = BTreeMap::new();
    for (index, name) in TABLES.iter().enumerate() {
        if control.checkpoint() == Checkpoint::Cancelled {
            return Err("Legacy import was cancelled; the current workspace is unchanged.".into());
        }
        let exists = transaction
            .query_row(
                "SELECT type FROM sqlite_schema WHERE name=?1",
                [name],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if exists.as_deref() != Some("table") {
            return Err(format!(
                "The legacy database is missing its {name} table; nothing was imported."
            ));
        }
        let mut statement = transaction
            .prepare(&format!("SELECT * FROM \"{name}\" ORDER BY id"))
            .map_err(|error| format!("The legacy {name} schema is not recognized: {error}"))?;
        let columns = statement
            .column_names()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let required: &[&str] = match *name {
            "users" => &["id", "username"],
            "projects" => &["id", "name", "path"],
            "project_files" => &["id", "project_id", "filename", "filepath"],
            "recent_projects" | "favorite_projects" => &["id", "user_id", "project_id"],
            "file_access_log" => &["id", "file_id", "user_id", "accessed_at"],
            "user_activity_log" => &["id", "username", "action_type", "description", "timestamp"],
            _ => &[],
        };
        if let Some(missing) = required
            .iter()
            .find(|required| !columns.iter().any(|column| column == **required))
        {
            return Err(format!(
                "The legacy {name} table is missing its {missing} column; nothing was imported."
            ));
        }
        let mut cursor = statement.query([]).map_err(|error| error.to_string())?;
        let mut rows = Vec::new();
        while let Some(row) = cursor.next().map_err(|error| error.to_string())? {
            if rows.len() % 128 == 0 && control.checkpoint() == Checkpoint::Cancelled {
                return Err(
                    "Legacy import was cancelled; the current workspace is unchanged.".into(),
                );
            }
            let mut values = BTreeMap::new();
            for (column, name) in columns.iter().enumerate() {
                let value = match row.get_ref(column).map_err(|error| error.to_string())? {
                    ValueRef::Null => LegacyValue::Null,
                    ValueRef::Integer(value) => LegacyValue::Integer(value),
                    ValueRef::Real(value) => LegacyValue::RealBits(value.to_bits()),
                    ValueRef::Text(value) => match std::str::from_utf8(value) {
                        Ok(value) => LegacyValue::Text(value.into()),
                        Err(_) => LegacyValue::InvalidText(value.to_vec()),
                    },
                    ValueRef::Blob(value) => LegacyValue::Blob(value.to_vec()),
                };
                values.insert(name.clone(), value);
            }
            rows.push(values);
            if rows.len() % 128 == 0 {
                events.progress(
                    JobPhase::Counting,
                    JobProgress {
                        completed: rows.len() as u64,
                        current: Some(sources.database.clone()),
                        ..Default::default()
                    },
                );
            }
        }
        tables.insert((*name).to_owned(), rows);
        events.progress(
            JobPhase::Scanning,
            JobProgress {
                completed: (index + 1) as u64,
                total: Some(TABLES.len() as u64),
                current: Some(sources.database.clone()),
                ..Default::default()
            },
        );
    }
    transaction.commit().map_err(|error| error.to_string())?;
    let after = fs::metadata(&sources.database).map_err(|error| error.to_string())?;
    if crate::paths::FileIdentity::from_metadata(&before)
        .zip(crate::paths::FileIdentity::from_metadata(&after))
        .is_some_and(|(before, after)| !before.same_file(&after))
    {
        return Err(
            "The legacy database path was replaced while importing; choose it again.".into(),
        );
    }
    let ini = sources.ini.as_deref().map(capture_sidecar).transpose()?;
    let favorites = sources
        .favorites
        .iter()
        .map(|path| capture_sidecar(path))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(LegacyCapture {
        schema_version: 1,
        database: crate::transfers::StoredPath::absolute(&sources.database)
            .map_err(|error| error.to_string())?,
        captured_at: crate::storage::utc_timestamp_now(),
        tables,
        ini,
        favorites,
    })
}

fn capture_sidecar(path: &Path) -> Result<CapturedSidecar, String> {
    if !path.is_absolute() {
        return Err("Choose absolute import sidecar paths.".into());
    }
    let mut file = File::open(path)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
    let before = file.metadata().map_err(|error| error.to_string())?;
    if !before.is_file() || before.len() > 16 * 1024 * 1024 {
        return Err(format!(
            "{} must be a regular sidecar smaller than 16 MiB.",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    let after = file.metadata().map_err(|error| error.to_string())?;
    let named = fs::metadata(path).map_err(|error| error.to_string())?;
    if bytes.len() > 16 * 1024 * 1024
        || before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || crate::paths::FileIdentity::from_metadata(&before)
            .zip(crate::paths::FileIdentity::from_metadata(&named))
            .is_some_and(|(before, named)| !before.same_file(&named))
    {
        return Err(format!(
            "{} changed while being read; retry the import.",
            path.display()
        ));
    }
    Ok(CapturedSidecar {
        path: crate::transfers::StoredPath::absolute(path).map_err(|error| error.to_string())?,
        bytes,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RowStatus {
    Imported,
    Combined,
    Unresolved,
    ImportedWithUnresolvedReferences,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RowDisposition {
    pub status: RowStatus,
    pub table: String,
    pub legacy_id: Option<i64>,
    pub local_table: Option<String>,
    pub local_id: Option<i64>,
    pub message: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportReport {
    pub schema_version: u32,
    pub captured_at: String,
    pub rows: Vec<RowDisposition>,
    pub issues: Vec<String>,
    pub source_counts: BTreeMap<String, usize>,
}

#[derive(Clone, Debug)]
pub struct PreparedImport {
    pub directory: PathBuf,
    pub report: ImportReport,
}

#[derive(Clone, Default)]
struct Mappings {
    ids: HashMap<(String, i64), i64>,
}

fn text(row: &LegacyRow, name: &str) -> Option<String> {
    match row.get(name)? {
        LegacyValue::Text(value) => Some(value.clone()),
        LegacyValue::Integer(value) => Some(value.to_string()),
        LegacyValue::RealBits(value) => Some(f64::from_bits(*value).to_string()),
        _ => None,
    }
}
fn integer(row: &LegacyRow, name: &str) -> Option<i64> {
    match row.get(name)? {
        LegacyValue::Integer(value) => Some(*value),
        _ => None,
    }
}
fn required(row: &LegacyRow, name: &str) -> Result<String, String> {
    text(row, name)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!("{name} is missing or cannot be represented as text; the raw row was retained.")
        })
}
fn mapped(mappings: &Mappings, table: &str, id: Option<i64>) -> Result<i64, String> {
    id.and_then(|id| mappings.ids.get(&(table.into(), id)).copied())
        .ok_or_else(|| {
            format!("The referenced {table} row could not be resolved; the raw row was retained.")
        })
}
fn database_error(error: rusqlite::Error) -> StorageError {
    StorageError::new(StorageCategory::Database, None, error.to_string())
}

fn imported_path(raw: &str) -> Result<PathColumns, String> {
    let imported = crate::paths::retain_imported(std::ffi::OsStr::new(raw));
    if let Some(path) = imported.native {
        let key = crate::paths::path_key(&path, &path).map_err(|error| error.to_string())?;
        return Ok(PathColumns::native(&path, &key));
    }
    let platform = match imported.tag {
        crate::paths::PlatformTag::Posix => "posix",
        crate::paths::PlatformTag::Windows => "windows",
        crate::paths::PlatformTag::Unknown => "unknown",
    };
    let spelling = if platform == "windows" {
        raw.encode_utf16().flat_map(u16::to_le_bytes).collect()
    } else {
        raw.as_bytes().to_vec()
    };
    Ok(PathColumns {
        storage: None,
        platform,
        spelling,
        display: Some(raw.into()),
    })
}

struct Converted {
    status: RowStatus,
    table: Option<&'static str>,
    id: Option<i64>,
    message: String,
}
fn converted(table: &'static str, id: i64, message: &str) -> Converted {
    Converted {
        status: RowStatus::Imported,
        table: Some(table),
        id: Some(id),
        message: message.into(),
    }
}

impl Converted {
    fn combined(mut self) -> Self {
        self.status = RowStatus::Combined;
        self
    }
}

fn convert_row(
    connection: &Connection,
    mappings: &Mappings,
    table: &str,
    row: &LegacyRow,
    at: &str,
) -> Result<Converted, String> {
    let sql = |error: rusqlite::Error| error.to_string();
    let id =
        integer(row, "id").ok_or("The row has no integer ID; its raw values were retained.")?;
    match table {
        "users" => {
            let username = required(row, "username")?;
            if username.trim().is_empty() || username.chars().count() > 64 {
                return Err(
                    "The username does not fit the new catalog; its raw row was retained.".into(),
                );
            }
            let admin = integer(row, "is_admin").unwrap_or(0);
            if admin != 0 && admin != 1 {
                return Err(
                    "The administrator flag is neither 0 nor 1; its raw row was retained.".into(),
                );
            }
            if let Some(existing) = connection
                .query_row(
                    "SELECT id FROM users WHERE username=?1",
                    [&username],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(sql)?
            {
                return Ok(converted(
                    "users",
                    existing,
                    "Combined with the same exact username; the original row remains in capture.json.",
                ).combined());
            }
            connection
                .execute(
                    "INSERT INTO users(id,username,is_admin,created_at) VALUES(?1,?2,?3,?4)",
                    params![
                        id,
                        username,
                        admin,
                        text(row, "created_at").unwrap_or_default()
                    ],
                )
                .map_err(sql)?;
            Ok(converted("users", id, "Imported with original ID."))
        }
        "projects" => {
            let name = required(row, "name")?;
            let raw = required(row, "path")?;
            let path = imported_path(&raw)?;
            let existing = connection.query_row("SELECT id FROM projects WHERE root_platform=?1 AND ((?2 IS NOT NULL AND root_storage=?2) OR (?2 IS NULL AND root_storage IS NULL AND root_display=?3)) ORDER BY id LIMIT 1", params![path.platform,path.storage,path.display], |row| row.get::<_,i64>(0)).optional().map_err(sql)?;
            let local = if let Some(existing) = existing {
                existing
            } else {
                let author = text(row, "added_by")
                    .map(|name| {
                        connection
                            .query_row("SELECT id FROM users WHERE username=?1", [name], |row| {
                                row.get::<_, i64>(0)
                            })
                            .optional()
                    })
                    .transpose()
                    .map_err(sql)?
                    .flatten();
                connection.execute("INSERT INTO projects(id,name,root_storage,root_platform,root_spelling,root_display,added_by,added_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)", params![id,name,path.storage,path.platform,path.spelling,path.display,author,text(row,"added_at").unwrap_or_default()]).map_err(sql)?;
                id
            };
            let folders = text(row, "folders").unwrap_or_else(|| raw.clone());
            for folder in folders
                .lines()
                .map(str::trim)
                .filter(|folder| !folder.is_empty())
            {
                let folder = imported_path(folder)?;
                connection.execute("INSERT INTO project_scan_folders(project_id,storage,platform,spelling,display,added_at) SELECT ?1,?2,?3,?4,?5,?6 WHERE NOT EXISTS(SELECT 1 FROM project_scan_folders WHERE project_id=?1 AND platform=?3 AND spelling=?4)", params![local,folder.storage,folder.platform,folder.spelling,folder.display,at]).map_err(sql)?;
            }
            let converted = converted(
                "projects",
                local,
                if existing.is_some() {
                    "Combined the same normalized root; conflicting source metadata remains in capture.json."
                } else {
                    "Imported with original ID; unavailable folders remain registered without scanning."
                },
            );
            Ok(if existing.is_some() {
                converted.combined()
            } else {
                converted
            })
        }
        "project_files" => {
            let project = mapped(mappings, "projects", integer(row, "project_id"))?;
            let raw = required(row, "filepath")?;
            let path = imported_path(&raw)?;
            let filename = required(row, "filename")?;
            let existing = connection.query_row("SELECT id FROM files WHERE platform=?1 AND ((?2 IS NOT NULL AND storage=?2) OR (?2 IS NULL AND storage IS NULL AND display=?3)) ORDER BY id LIMIT 1", params![path.platform,path.storage,path.display], |row| row.get::<_,i64>(0)).optional().map_err(sql)?;
            let local = if let Some(existing) = existing {
                existing
            } else {
                let version = text(row, "version");
                let number = version
                    .as_ref()
                    .and_then(|version| version.trim_start_matches(['v', 'V']).parse::<i64>().ok());
                let modified_raw = text(row, "last_modified");
                let modified_ns = match row.get("last_modified") {
                    Some(LegacyValue::Integer(seconds)) => seconds.checked_mul(1_000_000_000),
                    Some(LegacyValue::RealBits(bits)) => {
                        let nanos = f64::from_bits(*bits) * 1e9;
                        if nanos.is_finite() && nanos >= i64::MIN as f64 && nanos < i64::MAX as f64
                        {
                            Some(nanos.round() as i64)
                        } else {
                            None
                        }
                    }
                    _ => modified_raw
                        .as_ref()
                        .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
                        .and_then(|date| date.timestamp_nanos_opt()),
                };
                connection.execute("INSERT INTO files(id,storage,platform,spelling,display,filename,version,version_number,filetype,modified_unix_ns,modified_raw,first_seen_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",params![id,path.storage,path.platform,path.spelling,path.display,filename,version,number,text(row,"filetype"),modified_ns,modified_raw,at]).map_err(sql)?;
                id
            };
            connection.execute("INSERT INTO project_files(project_id,file_id,added_at) VALUES(?1,?2,?3) ON CONFLICT(project_id,file_id) DO NOTHING",params![project,local,at]).map_err(sql)?;
            let converted = converted(
                "files",
                local,
                if existing.is_some() {
                    "Combined the same file path while preserving this project membership and the raw source metadata."
                } else {
                    "Imported with original file ID; raw modification time retained."
                },
            );
            Ok(if existing.is_some() {
                converted.combined()
            } else {
                converted
            })
        }
        "favorite_projects" | "recent_projects" => {
            let user = mapped(mappings, "users", integer(row, "user_id"))?;
            let project = mapped(mappings, "projects", integer(row, "project_id"))?;
            let destination_table = if table == "favorite_projects" {
                "favorites"
            } else {
                "recents"
            };
            let existing: bool = connection.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {destination_table} WHERE user_id=?1 AND project_id=?2)"), params![user,project], |row| row.get(0)).map_err(sql)?;
            if table == "favorite_projects" {
                connection.execute("INSERT INTO favorites(user_id,project_id,created_at) VALUES(?1,?2,?3) ON CONFLICT(user_id,project_id) DO NOTHING",params![user,project,at]).map_err(sql)?;
            } else {
                connection.execute("INSERT INTO recents(user_id,project_id,accessed_at) VALUES(?1,?2,?3) ON CONFLICT(user_id,project_id) DO UPDATE SET accessed_at=MAX(recents.accessed_at,excluded.accessed_at)",params![user,project,text(row,"accessed_at").unwrap_or_default()]).map_err(sql)?;
            }
            Ok(Converted {
                status: if existing {
                    RowStatus::Combined
                } else {
                    RowStatus::Imported
                },
                table: None,
                id: None,
                message: format!(
                    "Imported/coalesced for user {user}, project {project}; composite mapping and original timestamp remain in this report/capture."
                ),
            })
        }
        "file_access_log" => {
            let file = mapped(mappings, "project_files", integer(row, "file_id"));
            let user = mapped(mappings, "users", integer(row, "user_id"));
            let unresolved = file.is_err() || user.is_err();
            connection.execute("INSERT INTO file_access_log(id,file_id,user_id,accessed_at) VALUES(?1,?2,?3,?4)",params![id,file.ok(),user.ok(),text(row,"accessed_at").unwrap_or_default()]).map_err(sql)?;
            let mut converted = converted(
                "file_access_log",
                id,
                if unresolved {
                    "Imported log with unresolved references left NULL; raw original IDs remain in capture.json."
                } else {
                    "Imported log with original ID and mapped references."
                },
            );
            if unresolved {
                converted.status = RowStatus::ImportedWithUnresolvedReferences;
            }
            Ok(converted)
        }
        "user_activity_log" => {
            let username = text(row, "username").unwrap_or_default();
            let user = connection
                .query_row(
                    "SELECT id FROM users WHERE username=?1",
                    [&username],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(sql)?;
            connection.execute("INSERT INTO user_activity_log(id,user_id,username,action_type,description,at) VALUES(?1,?2,?3,?4,?5,?6)",params![id,user,username,text(row,"action_type").unwrap_or_default(),text(row,"description").unwrap_or_default(),text(row,"timestamp").unwrap_or_default()]).map_err(sql)?;
            Ok(converted(
                "user_activity_log",
                id,
                "Imported original log ID, username and raw local timestamp without inventing a timezone.",
            ))
        }
        _ => Err("This legacy table is not supported.".into()),
    }
}

/// Sidecar favorites: the legacy UI wrote `favorites_<username>.json`, an
/// unversioned JSON array of project IDs and/or path spellings, next to the INI
/// (`ui_slick/favorites_manager.py`), so the SQL `favorite_projects` rows are
/// not the whole picture. Values resolve against the selected legacy database
/// first and then against imported project roots; unresolved values are
/// reported, never guessed.
fn sidecar_username(sidecar: &CapturedSidecar) -> Option<&str> {
    sidecar
        .path
        .path()
        .file_name()?
        .to_str()?
        .strip_prefix("favorites_")?
        .strip_suffix(".json")
        .filter(|name| !name.is_empty())
}

fn sidecar_label(sidecar: &CapturedSidecar) -> String {
    sidecar
        .path
        .path()
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .map(str::to_owned)
        .unwrap_or_else(|| sidecar.path.path().display().to_string())
}

/// Resolves one sidecar entry. A JSON number or a numeric string is a legacy
/// project ID and is looked up in the conversion map; an absolute path spelling
/// is matched against the imported roots on its own platform and normalized
/// root key. Anything else (relative names, `None`, basenames) is unresolved.
fn resolve_favorite(
    connection: &Connection,
    mappings: &Mappings,
    value: &serde_json::Value,
) -> Result<i64, String> {
    let id = match value {
        serde_json::Value::Number(number) => number
            .as_i64()
            .ok_or_else(|| format!("{number} is not an integer project ID"))?,
        serde_json::Value::String(spelling) => {
            let trimmed = spelling.trim();
            match trimmed.parse::<i64>() {
                Ok(id) => id,
                Err(_) => {
                    if trimmed.is_empty() {
                        return Err("the value is an empty string".into());
                    }
                    let imported = crate::paths::retain_imported(std::ffi::OsStr::new(trimmed));
                    if imported.tag == crate::paths::PlatformTag::Unknown {
                        return Err(format!(
                            "{spelling:?} is neither a project ID nor an absolute path, and a basename is never guessed"
                        ));
                    }
                    let path = imported_path(trimmed)?;
                    return connection
                        .query_row(
                            "SELECT id FROM projects WHERE root_platform=?1 AND ((?2 IS NOT NULL AND root_storage=?2) OR (?2 IS NULL AND root_storage IS NULL AND root_display=?3)) ORDER BY id LIMIT 1",
                            params![path.platform, path.storage, path.display],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| {
                            format!(
                                "{spelling:?} does not match any imported project root on its platform"
                            )
                        });
                }
            }
        }
        other => {
            return Err(format!(
                "{other} is neither a project ID nor a path spelling; the legacy UI only stored those"
            ));
        }
    };
    mappings
        .ids
        .get(&("projects".to_string(), id))
        .copied()
        .ok_or_else(|| format!("no imported project carries legacy ID {id}"))
}

/// Imports one `favorites_<username>.json` into `favorites`, keeping the SQL
/// favorites already imported for that user. Per-value outcomes are returned as
/// dispositions; only catalog I/O errors abort.
fn import_favorite_values(
    connection: &Connection,
    mappings: &Mappings,
    user: i64,
    label: &str,
    bytes: &[u8],
    at: &str,
) -> Result<(Vec<RowDisposition>, Vec<String>), rusqlite::Error> {
    let values: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(values) => values,
        Err(error) => {
            return Ok((
                Vec::new(),
                vec![format!(
                    "{label} is not readable JSON ({error}); its entries were not imported and the SQL favorites are unchanged. The raw bytes remain in capture.json."
                )],
            ));
        }
    };
    let Some(values) = values.as_array() else {
        return Ok((
            Vec::new(),
            vec![format!(
                "{label} is not the unversioned JSON array the legacy UI wrote; its entries were not imported. The raw bytes remain in capture.json."
            )],
        ));
    };
    let mut dispositions = Vec::new();
    let mut issues = Vec::new();
    for value in values {
        let spelled = value.to_string();
        // An unversioned JSON array has no row IDs, so the entry's provenance is
        // its raw value in the message, not a fabricated legacy_id.
        let (project, message) = match resolve_favorite(connection, mappings, value) {
            Ok(project) => {
                connection.execute(
                    "INSERT INTO favorites(user_id,project_id,created_at) VALUES(?1,?2,?3) ON CONFLICT(user_id,project_id) DO NOTHING",
                    params![user, project, at],
                )?;
                (
                    Some(project),
                    format!(
                        "Imported the sidecar favorite {spelled} for user {user} as project {project}; the SQL favorite for the same pair is kept. The sidecar carries no timestamp, so {at} is the import time."
                    ),
                )
            }
            Err(reason) => (
                None,
                format!(
                    "The sidecar favorite {spelled} for user {user} stays unresolved: {reason}. The original value remains in capture.json."
                ),
            ),
        };
        if project.is_none() {
            issues.push(format!("{label}: {message}"));
        }
        dispositions.push(RowDisposition {
            status: if project.is_some() {
                RowStatus::Imported
            } else {
                RowStatus::Unresolved
            },
            table: label.to_string(),
            legacy_id: None,
            local_table: project.map(|_| "favorites".to_string()),
            local_id: project,
            message,
        });
    }
    Ok((dispositions, issues))
}

/// Always creates a separate catalog/config area. No selection is changed here.
pub fn prepare_import(
    sources: &LegacySources,
    data_root: &Path,
    control: &JobControl,
    events: &JobEvents,
) -> Result<PreparedImport, String> {
    if !data_root.is_absolute() {
        return Err("The application data root must be absolute.".into());
    }
    let capture = capture(sources, control, events)?;
    for sidecar in &capture.favorites {
        let values: serde_json::Value =
            serde_json::from_slice(&sidecar.bytes).map_err(|error| {
                format!(
                    "{} cannot be read as favorites JSON: {error}. Nothing was staged.",
                    sidecar_label(sidecar)
                )
            })?;
        if !values.is_array() {
            return Err(format!(
                "{} must contain the original favorites array. Nothing was staged.",
                sidecar_label(sidecar)
            ));
        }
    }
    // The INI is applied before anything is staged: the INI adapter never
    // returns a partial result, so a malformed file aborts here and no staged
    // area is left behind from a source this build could not read.
    let mut settings = crate::config::Settings::default();
    let mut ini_issues = Vec::new();
    if let Some(sidecar) = &capture.ini {
        let label = sidecar_label(sidecar);
        match crate::legacy_ini::import_settings(&sidecar.bytes, &settings) {
            Ok((imported, issues)) => {
                settings = imported;
                ini_issues.extend(issues.into_iter().map(|issue| format!("{label}: {issue}")));
            }
            Err(reason) => {
                return Err(format!(
                    "The legacy settings file {label} could not be imported: {reason}. Nothing was staged and the current workspace is unchanged."
                ));
            }
        }
    }
    let name = format!(
        "import-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos()
    );
    let parent = crate::paths::validate_destination(data_root, Path::new("imports"))
        .map_err(|error| format!("The import staging folder cannot be used: {error}"))?;
    fs::create_dir_all(&parent).map_err(|error| error.to_string())?;
    let directory = parent.join(name);
    fs::create_dir(&directory).map_err(|error| error.to_string())?;
    crate::config::write_versioned(
        &directory.join("capture.json"),
        "legacy source snapshot",
        &capture,
    )
    .map_err(|error| error.to_string())?;
    let catalog =
        Catalog::open(directory.join("catalog.sqlite3")).map_err(|error| error.to_string())?;
    let outcome = (|| -> Result<ImportReport, String> {
        catalog
            .info()
            .map_err(|error| error.to_string())?
            .wait()
            .map_err(|error| error.to_string())?;
        let source = imported_path(
            sources
                .database
                .to_str()
                .ok_or("The source database path cannot be represented in import metadata.")?,
        )?;
        let at = capture.captured_at.clone();
        let origin=catalog.transaction(move |connection| {
            connection.execute("INSERT INTO import_origins(source_kind,source_spelling,source_display,imported_at,notes) VALUES('legacy_sqlite',?1,?2,?3,?4)",params![source.spelling,source.display,at,"Raw consistent snapshot: capture.json; dispositions: report.json"]).map_err(database_error)?;
            Ok(connection.last_insert_rowid())
        }).map_err(|error|error.to_string())?.wait().map_err(|error|error.to_string())?;
        let mut mappings = Mappings::default();
        let mut report = ImportReport {
            schema_version: 1,
            captured_at: capture.captured_at.clone(),
            rows: Vec::new(),
            issues: Vec::new(),
            source_counts: capture
                .tables
                .iter()
                .map(|(table, rows)| (table.clone(), rows.len()))
                .collect(),
        };
        let total = capture.tables.values().map(Vec::len).sum::<usize>();
        let mut completed = 0;
        for table in TABLES {
            let rows = capture
                .tables
                .get(table)
                .ok_or("A captured legacy table disappeared.")?;
            for batch in rows.chunks(256) {
                if control.checkpoint() == Checkpoint::Cancelled {
                    return Err(format!(
                        "Import cancelled. The current workspace is unchanged; partial staging data remains at {}.",
                        directory.display()
                    ));
                }
                let batch = batch.to_vec();
                let at = capture.captured_at.clone();
                let (next,dispositions)=catalog.transaction(move |transaction| {
                    let mut mappings=mappings; let mut dispositions=Vec::new();
                    for row in batch {
                        let legacy_id=integer(&row,"id");
                        let mut savepoint=transaction.savepoint().map_err(database_error)?;
                        match convert_row(&savepoint,&mappings,table,&row,&at) {
                            Ok(converted) => {
                                if let (Some(local_table),Some(local_id),Some(legacy_id))=(converted.table,converted.id,legacy_id) {
                                    savepoint.execute("INSERT INTO import_origin_rows(origin_id,legacy_table,legacy_id,local_table,local_id) VALUES(?1,?2,?3,?4,?5)",params![origin,table,legacy_id,local_table,local_id]).map_err(database_error)?;
                                }
                                savepoint.commit().map_err(database_error)?;
                                if let (Some(local_id),Some(legacy_id))=(converted.id,legacy_id) { mappings.ids.insert((table.into(),legacy_id),local_id); }
                                dispositions.push(RowDisposition { status:converted.status,table:table.into(),legacy_id,local_table:converted.table.map(str::to_owned),local_id:converted.id,message:converted.message });
                            }
                            Err(message) => { savepoint.rollback().map_err(database_error)?; savepoint.commit().map_err(database_error)?; dispositions.push(RowDisposition { status:RowStatus::Unresolved,table:table.into(),legacy_id,local_table:None,local_id:None,message }); }
                        }
                    }
                    Ok((mappings,dispositions))
                }).map_err(|error|error.to_string())?.wait().map_err(|error|error.to_string())?;
                mappings = next;
                completed += dispositions.len();
                report.rows.extend(dispositions);
                events.progress(
                    JobPhase::Validating,
                    JobProgress {
                        completed: completed as u64,
                        total: Some(total as u64),
                        current: Some(sources.database.clone()),
                        ..Default::default()
                    },
                );
            }
        }
        let users = catalog
            .list_users()
            .map_err(|error| error.to_string())?
            .wait()
            .map_err(|error| error.to_string())?;
        settings.selected_username = users
            .iter()
            .find(|user| user.username == "admin")
            .or(users.first())
            .map(|user| user.username.clone());
        report.issues.extend(ini_issues);
        if let Some(sidecar) = &capture.ini {
            let label = sidecar_label(sidecar);
            report.rows.push(RowDisposition {
                status: RowStatus::Imported,
                table: label.clone(),
                legacy_id: None,
                local_table: Some("settings".to_string()),
                local_id: None,
                message: format!(
                    "The legacy settings this build consumes were applied from {label} over fresh defaults; values it has no consumer for are reported under issues, with their raw text retained in capture.json."
                ),
            });
        }
        let sidecar_total = capture.favorites.len();
        for (index, sidecar) in capture.favorites.iter().enumerate() {
            if control.checkpoint() == Checkpoint::Cancelled {
                return Err(format!(
                    "Import cancelled. The current workspace is unchanged; staged data remains at {}.",
                    directory.display()
                ));
            }
            let label = sidecar_label(sidecar);
            let Some(username) = sidecar_username(sidecar).map(str::to_owned) else {
                report.issues.push(format!(
                    "{label} is not named favorites_<username>.json; its entries were not imported."
                ));
                continue;
            };
            let bytes = sidecar.bytes.clone();
            let captured_at = capture.captured_at.clone();
            let sidecar_mappings = mappings.clone();
            let sidecar_name = label.clone();
            let (rows, issues) = catalog
                .transaction(move |connection| {
                    let user = connection
                        .query_row(
                            "SELECT id FROM users WHERE username=?1",
                            [&username],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(database_error)?;
                    let Some(user) = user else {
                        return Ok((
                            Vec::new(),
                            vec![format!(
                                "{sidecar_name} belongs to the legacy user {username:?}, who was not imported; its entries were not imported and are never attached to another user."
                            )],
                        ));
                    };
                    import_favorite_values(
                        connection,
                        &sidecar_mappings,
                        user,
                        &sidecar_name,
                        &bytes,
                        &captured_at,
                    )
                    .map_err(database_error)
                })
                .map_err(|error| error.to_string())?
                .wait()
                .map_err(|error| error.to_string())?;
            report.rows.extend(rows);
            report.issues.extend(issues);
            events.progress(
                JobPhase::Validating,
                JobProgress {
                    completed: (total + index + 1) as u64,
                    total: Some((total + sidecar_total) as u64),
                    current: Some(sidecar.path.path().to_path_buf()),
                    ..Default::default()
                },
            );
        }
        crate::config::SettingsStore::new(directory.join("settings.json"))
            .save(&settings)
            .map_err(|error| error.to_string())?;
        catalog
            .submit(|connection| {
                let integrity: String = connection
                    .query_row("PRAGMA quick_check", [], |row| row.get(0))
                    .map_err(database_error)?;
                if integrity != "ok" {
                    return Err(StorageError::new(
                        StorageCategory::InvalidSchema,
                        None,
                        integrity,
                    ));
                }
                let mut statement = connection
                    .prepare("PRAGMA foreign_key_check")
                    .map_err(database_error)?;
                if statement
                    .query([])
                    .map_err(database_error)?
                    .next()
                    .map_err(database_error)?
                    .is_some()
                {
                    return Err(StorageError::new(
                        StorageCategory::InvalidSchema,
                        None,
                        "The staged import has unresolved foreign-key constraints.",
                    ));
                }
                Ok(())
            })
            .map_err(|error| error.to_string())?
            .wait()
            .map_err(|error| error.to_string())?;
        Ok(report)
    })();
    catalog.shutdown().join();
    let report = outcome?;
    crate::config::write_versioned(
        &directory.join("report.json"),
        "legacy import report",
        &report,
    )
    .map_err(|error| error.to_string())?;
    Ok(PreparedImport { directory, report })
}
