//! F2 storage: one worker owns the writable SQLite catalog.
//!
//! Contract (`CONTRACTS.md` §3, `PLAN.md` decision 2): a single
//! [`rusqlite::Connection`] is created and used by one dedicated standard-library
//! thread. Callers submit typed requests through a bounded queue and poll owned
//! replies, so no egui frame ever runs a database call and no caller can hold or
//! share the connection.
//!
//! Catalog policy enforced here:
//! - the target path must be explicit and absolute, and the catalog is refused
//!   when the containing mount is a known network file system (SQLite over SMB
//!   is not a supported writable mode);
//! - `PRAGMA foreign_keys = ON` and a 5 s busy timeout on every connection;
//! - WAL only when the target is provably local, since
//!   [`connection`](rusqlite::Connection) journal mode is not imposed on a
//!   filesystem this code cannot certify;
//! - `PRAGMA application_id` + `PRAGMA user_version` identify the catalog, and
//!   schema creation/migration runs as one transaction. An existing database
//!   that is not this application's catalog — in particular the original legacy
//!   database, which has tables and no application id — is refused, never
//!   initialized or rewritten;
//! - paths are stored as native tagged bytes plus a separate UTF-8 display
//!   spelling, keyed by [`PathKey::storage_bytes`]. Foreign or unresolved
//!   spellings keep their source bytes and no key ([`PathColumns::unresolved`]).
//!
//! Not implemented here (later packets): legacy import (F3), project
//! registration and scan reconciliation (P1), transfer execution (X1). No
//! provider credentials, tokens or secrets are stored in this catalog.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};

use crate::paths::{PathKey, PlatformTag, retain_imported, utf8_spelling};

/// `PRAGMA application_id` value of a VFX Launcher catalog: ASCII "VFXL".
pub const APPLICATION_ID: i32 = 0x5646_584C;
/// Catalog schema version recorded in `PRAGMA user_version`.
pub const SCHEMA_VERSION: i64 = 1;
/// Bounded request queue: submitting beyond this returns a readable "busy
/// worker, try again" error instead of blocking the caller.
pub const REQUEST_QUEUE_BOUND: usize = 64;

const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Result alias for module-owned storage errors.
pub type StorageResult<T> = Result<T, StorageError>;

/// `users.id` references. Retention/tombstoning keeps historical attribution
/// valid, so ids are never reused.
pub type UserId = i64;
/// `projects.id` references.
pub type ProjectId = i64;
/// `files.id` references.
pub type FileId = i64;

/// Stable storage failure categories with readable causes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageCategory {
    /// The worker thread or the catalog it owns could not be started.
    WorkerStartup,
    /// The worker stopped, or ended without answering a request.
    WorkerGone,
    /// The bounded request queue is full.
    QueueFull,
    /// SQLite reported busy/locked (another connection holds the write lock).
    Busy,
    /// The database exists but is not this build's catalog schema.
    InvalidSchema,
    /// The catalog path is unusable (relative, missing directory, not a file).
    Path,
    /// File-system or disk failure while reading/writing the catalog.
    Io,
    /// The catalog target sits on a known network file system.
    RefusedLocation,
    /// Other SQLite/database failure.
    Database,
    /// The request itself is invalid (blank name, missing row, duplicate name).
    InvalidInput,
}

impl StorageCategory {
    pub fn label(self) -> &'static str {
        match self {
            Self::WorkerStartup => "catalog worker could not start",
            Self::WorkerGone => "catalog worker stopped",
            Self::QueueFull => "catalog worker is behind",
            Self::Busy => "catalog busy",
            Self::InvalidSchema => "unrecognized catalog database",
            Self::Path => "catalog path error",
            Self::Io => "catalog I/O error",
            Self::RefusedLocation => "catalog location refused",
            Self::Database => "catalog database error",
            Self::InvalidInput => "invalid catalog request",
        }
    }
}

/// Module-owned storage error: category, affected path (when known) and a cause
/// a user can read.
#[derive(Clone, Debug)]
pub struct StorageError {
    category: StorageCategory,
    path: Option<PathBuf>,
    message: String,
}

impl StorageError {
    pub fn new(category: StorageCategory, path: Option<&Path>, message: impl Into<String>) -> Self {
        Self {
            category,
            path: path.map(Path::to_path_buf),
            message: message.into(),
        }
    }

    pub fn category(&self) -> StorageCategory {
        self.category
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for StorageError {
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

impl std::error::Error for StorageError {}

/// Immutable description of an opened catalog, reported by
/// [`Catalog::info`] after the worker opened it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogInfo {
    pub path: PathBuf,
    pub schema_version: i64,
    /// Effective `journal_mode` (`"wal"` only on a provably local target).
    pub journal_mode: String,
    pub foreign_keys: bool,
}

/// A user record. Deleted users stay in the catalog with `deleted_at` set so
/// historical attribution remains resolvable; live queries filter them out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserRecord {
    pub id: UserId,
    pub username: String,
    pub is_admin: bool,
    pub created_at: String,
    pub deleted_at: Option<String>,
}

/// Field-wise user update. `None` leaves the stored value unchanged.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserChange {
    pub username: Option<String>,
    pub is_admin: Option<bool>,
}

/// State of one polled reply. `Ready`, `Failed` and `Finished` are terminal:
/// a reply is delivered once and a second poll never fabricates a value or a
/// worker failure.
pub enum ReplyState<T> {
    /// The worker has not answered yet.
    Waiting,
    /// The worker answered successfully; the reply is consumed.
    Ready(T),
    /// The request failed; the error is consumed.
    Failed(StorageError),
    /// This reply was already delivered.
    Finished,
}

/// Owned, typed reply for one submitted request. Dropping it is safe: the reply
/// is discarded and the worker continues.
pub struct Pending<T> {
    reply: Receiver<StorageResult<T>>,
    finished: bool,
}

impl<T> Pending<T> {
    /// Non-blocking poll; never touches the database and never waits.
    pub fn try_poll(&mut self) -> ReplyState<T> {
        if self.finished {
            return ReplyState::Finished;
        }
        match self.reply.try_recv() {
            Ok(Ok(value)) => {
                self.finished = true;
                ReplyState::Ready(value)
            }
            Ok(Err(error)) => {
                self.finished = true;
                ReplyState::Failed(error)
            }
            Err(TryRecvError::Empty) => ReplyState::Waiting,
            Err(TryRecvError::Disconnected) => {
                self.finished = true;
                ReplyState::Failed(StorageError::new(
                    StorageCategory::WorkerGone,
                    None,
                    "the catalog worker stopped before answering this request",
                ))
            }
        }
    }

    /// True once the reply was delivered or the worker disappeared.
    pub fn is_finished(&self) -> bool {
        self.finished
    }
}

/// Handle to the catalog worker. Cheap to keep in application state; every
/// method either queues work or opens a reply channel, never blocks on SQLite.
pub struct Catalog {
    path: PathBuf,
    requests: SyncSender<Request>,
}

type Request = Box<dyn for<'a> FnOnce(StorageResult<&'a mut Connection>) + Send + 'static>;

impl Catalog {
    /// Starts the worker for `path`. Opening, validating, migrating and
    /// configuring the database happen on the worker thread; the caller gets
    /// only a queue handle. The open result arrives through [`Catalog::info`],
    /// which is answered after the open attempt, and as the same readable error
    /// for every other queued request when the open failed.
    pub fn open(path: impl Into<PathBuf>) -> StorageResult<Self> {
        let path = path.into();
        let (requests, queue) = mpsc::sync_channel::<Request>(REQUEST_QUEUE_BOUND);
        let worker_path = path.clone();
        thread::Builder::new()
            .name("vfx-catalog".to_string())
            .spawn(move || run_worker(worker_path, queue))
            .map_err(|err| {
                StorageError::new(
                    StorageCategory::WorkerStartup,
                    Some(&path),
                    format!("could not start the catalog worker thread: {err}"),
                )
            })?;
        Ok(Self { path, requests })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reports the catalog the worker opened, including the effective journal
    /// mode and schema version.
    pub fn info(&self) -> StorageResult<Pending<CatalogInfo>> {
        let path = self.path.clone();
        self.submit(move |connection| {
            Ok(CatalogInfo {
                path: path.clone(),
                schema_version: pragma_i64(connection, "user_version")?,
                journal_mode: pragma_text(connection, "journal_mode")?,
                foreign_keys: pragma_i64(connection, "foreign_keys")? != 0,
            })
        })
    }

    /// Queues one request. `f` runs on the worker thread with the catalog
    /// connection; the result is delivered through the returned [`Pending`].
    pub fn submit<T, F>(&self, f: F) -> StorageResult<Pending<T>>
    where
        T: Send + 'static,
        F: for<'a> FnOnce(&'a mut Connection) -> StorageResult<T> + Send + 'static,
    {
        let (reply, pending) = mpsc::channel::<StorageResult<T>>();
        let request: Request = Box::new(move |connection| {
            let outcome = connection.and_then(f);
            // A dropped Pending is a cancelled read, not a worker failure.
            let _ = reply.send(outcome);
        });
        match self.requests.try_send(request) {
            Ok(()) => Ok(Pending {
                reply: pending,
                finished: false,
            }),
            Err(TrySendError::Full(_)) => Err(StorageError::new(
                StorageCategory::QueueFull,
                Some(&self.path),
                format!(
                    "the catalog worker already has {REQUEST_QUEUE_BOUND} requests queued; try again once it catches up"
                ),
            )),
            Err(TrySendError::Disconnected(_)) => Err(StorageError::new(
                StorageCategory::WorkerGone,
                Some(&self.path),
                "the catalog worker stopped; the catalog must be opened again",
            )),
        }
    }

    /// Transaction entry point for later packets (P1 registration/scan commit,
    /// X1 run recording): `f` runs inside one deferred transaction owned by the
    /// worker. Dropping `f`'s error (or returning `Err`) rolls back; the
    /// transaction only commits when `f` returns `Ok`.
    pub fn transaction<T, F>(&self, f: F) -> StorageResult<Pending<T>>
    where
        T: Send + 'static,
        F: FnOnce(&mut Transaction<'_>) -> StorageResult<T> + Send + 'static,
    {
        self.submit(move |connection| {
            let mut transaction = connection
                .transaction()
                .map_err(|err| db_error("could not begin a catalog transaction", None, err))?;
            let value = f(&mut transaction)?;
            transaction
                .commit()
                .map_err(|err| db_error("could not commit the catalog transaction", None, err))?;
            Ok(value)
        })
    }

    pub fn list_users(&self) -> StorageResult<Pending<Vec<UserRecord>>> {
        self.submit(|connection| {
            let sql = format!("SELECT {USER_COLUMNS} FROM users ORDER BY lower(username), id");
            let mut statement = connection
                .prepare(&sql)
                .map_err(|err| db_error("could not read the user list", None, err))?;
            let rows = statement
                .query_map([], row_to_user)
                .map_err(|err| db_error("could not read the user list", None, err))?;
            let mut users = Vec::new();
            for row in rows {
                users.push(row.map_err(|err| db_error("could not read a user record", None, err))?);
            }
            Ok(users)
        })
    }

    /// Looks up one live (non-tombstoned) user by exact username. Used to
    /// restore the selected identity without assuming a row id.
    pub fn find_user(
        &self,
        username: impl Into<String>,
    ) -> StorageResult<Pending<Option<UserRecord>>> {
        let username = username.into();
        self.submit(move |connection| {
            let sql = format!(
                "SELECT {USER_COLUMNS} FROM users WHERE username = ?1 AND deleted_at IS NULL"
            );
            connection
                .query_row(&sql, params![username], row_to_user)
                .optional()
                .map_err(|err| db_error("could not look up the user", None, err))
        })
    }

    pub fn create_user(
        &self,
        username: impl Into<String>,
        is_admin: bool,
    ) -> StorageResult<Pending<UserRecord>> {
        let username = normalize_username(&username.into())?;
        self.submit(move |connection| {
            require_username_available(connection, &username, None)?;
            connection
                .execute(
                    "INSERT INTO users (username, is_admin, created_at) VALUES (?1, ?2, ?3)",
                    params![username, i64::from(is_admin), utc_timestamp_now()],
                )
                .map_err(|err| db_error("could not create the user", None, err))?;
            let id = connection.last_insert_rowid();
            read_user(connection, id)?.ok_or_else(|| {
                StorageError::new(
                    StorageCategory::Database,
                    None,
                    format!("user {id} was inserted but could not be read back"),
                )
            })
        })
    }

    pub fn update_user(
        &self,
        id: UserId,
        change: UserChange,
    ) -> StorageResult<Pending<UserRecord>> {
        if change.username.is_none() && change.is_admin.is_none() {
            return Err(StorageError::new(
                StorageCategory::InvalidInput,
                None,
                "the user update has no changed fields",
            ));
        }
        let username = match change.username {
            Some(ref raw) => Some(normalize_username(raw)?),
            None => None,
        };
        self.submit(move |connection| {
            let existing = require_live_user(connection, id)?;
            if let Some(username) = &username {
                require_username_available(connection, username, Some(existing.id))?;
            }
            connection
                .execute(
                    "UPDATE users SET username = COALESCE(?2, username), is_admin = COALESCE(?3, is_admin) WHERE id = ?1",
                    params![id, username, change.is_admin.map(i64::from)],
                )
                .map_err(|err| db_error("could not update the user", None, err))?;
            read_user(connection, id)?.ok_or_else(|| {
                StorageError::new(
                    StorageCategory::Database,
                    None,
                    format!("user {id} was updated but could not be read back"),
                )
            })
        })
    }

    /// Tombstones the user: the row, its activity log and its attribution stay
    /// readable, but the identity can no longer be selected or modified. The
    /// username becomes available to a new, separate identity.
    pub fn delete_user(&self, id: UserId) -> StorageResult<Pending<()>> {
        self.submit(move |connection| {
            let changed = connection
                .execute(
                    "UPDATE users SET deleted_at = ?2 WHERE id = ?1 AND deleted_at IS NULL",
                    params![id, utc_timestamp_now()],
                )
                .map_err(|err| db_error("could not delete the user", None, err))?;
            if changed == 0 {
                return Err(StorageError::new(
                    StorageCategory::InvalidInput,
                    None,
                    format!("user {id} does not exist or was already deleted"),
                ));
            }
            Ok(())
        })
    }
}

const USER_COLUMNS: &str = "id, username, is_admin, created_at, deleted_at";

fn row_to_user(row: &rusqlite::Row<'_>) -> rusqlite::Result<UserRecord> {
    Ok(UserRecord {
        id: row.get(0)?,
        username: row.get(1)?,
        is_admin: row.get::<_, i64>(2)? != 0,
        created_at: row.get(3)?,
        deleted_at: row.get(4)?,
    })
}

fn read_user(connection: &Connection, id: UserId) -> StorageResult<Option<UserRecord>> {
    let sql = format!("SELECT {USER_COLUMNS} FROM users WHERE id = ?1");
    connection
        .query_row(&sql, params![id], row_to_user)
        .optional()
        .map_err(|err| db_error("could not read the user", None, err))
}

fn require_live_user(connection: &Connection, id: UserId) -> StorageResult<UserRecord> {
    match read_user(connection, id)? {
        Some(user) if user.deleted_at.is_none() => Ok(user),
        Some(_) => Err(StorageError::new(
            StorageCategory::InvalidInput,
            None,
            format!("user {id} was deleted; the retained history cannot be modified"),
        )),
        None => Err(StorageError::new(
            StorageCategory::InvalidInput,
            None,
            format!("user {id} does not exist"),
        )),
    }
}

fn require_username_available(
    connection: &Connection,
    username: &str,
    except: Option<UserId>,
) -> StorageResult<()> {
    let taken: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM users WHERE username = ?1 AND deleted_at IS NULL AND id IS NOT ?2",
            params![username, except],
            |row| row.get(0),
        )
        .map_err(|err| db_error("could not check the user name", None, err))?;
    if taken > 0 {
        return Err(StorageError::new(
            StorageCategory::InvalidInput,
            None,
            format!("a user named {username:?} already exists"),
        ));
    }
    Ok(())
}

fn normalize_username(raw: &str) -> StorageResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(StorageError::new(
            StorageCategory::InvalidInput,
            None,
            "the user name cannot be blank",
        ));
    }
    if trimmed.chars().any(char::is_control) {
        return Err(StorageError::new(
            StorageCategory::InvalidInput,
            None,
            "the user name cannot contain control characters",
        ));
    }
    if trimmed.chars().count() > 64 {
        return Err(StorageError::new(
            StorageCategory::InvalidInput,
            None,
            "the user name is longer than 64 characters",
        ));
    }
    Ok(trimmed.to_string())
}

/// Native path columns: comparable key bytes, the platform the bytes belong to,
/// the exact native spelling and a separate UTF-8 display spelling.
///
/// `storage` is [`PathKey::storage_bytes`] for a resolved native path and `None`
/// for an imported spelling this build cannot resolve (foreign platform or no
/// explicit base); those keep their retained source bytes in `spelling` and are
/// reported, not converted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathColumns {
    pub storage: Option<Vec<u8>>,
    pub platform: &'static str,
    pub spelling: Vec<u8>,
    pub display: Option<String>,
}

/// Platform tag name of the bytes stored for the current build.
pub const CURRENT_PLATFORM: &str = if cfg!(windows) { "windows" } else { "posix" };

impl PathColumns {
    /// Columns for a path that is resolved natively: the key always comes from
    /// [`crate::paths::path_key`] with an explicit base.
    pub fn native(path: &Path, key: &PathKey) -> Self {
        Self {
            storage: Some(key.storage_bytes()),
            platform: CURRENT_PLATFORM,
            spelling: encode_native_spelling(path),
            display: utf8_spelling(path).map(str::to_owned),
        }
    }

    /// Columns for a spelling retained as imported, unresolved data. The bytes
    /// are the source spelling (never re-encoded), and no catalog key is
    /// invented for it.
    pub fn unresolved(raw: &OsStr) -> Self {
        let imported = retain_imported(raw);
        Self {
            storage: None,
            platform: platform_name(imported.tag),
            spelling: raw.as_encoded_bytes().to_vec(),
            display: raw.to_str().map(str::to_owned),
        }
    }

    /// Decodes the native spelling, and only when the stored platform is this
    /// build's platform. Foreign or unknown spellings stay unresolved.
    pub fn decode(&self) -> Option<PathBuf> {
        if self.platform != CURRENT_PLATFORM {
            return None;
        }
        decode_native_spelling(&self.spelling)
    }
}

fn platform_name(tag: PlatformTag) -> &'static str {
    match tag {
        PlatformTag::Posix => "posix",
        PlatformTag::Windows => "windows",
        PlatformTag::Unknown => "unknown",
    }
}

#[cfg(unix)]
fn encode_native_spelling(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn encode_native_spelling(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    let mut bytes = Vec::new();
    for unit in path.as_os_str().encode_wide() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    bytes
}

#[cfg(unix)]
fn decode_native_spelling(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(OsString::from_vec(bytes.to_vec())))
}

#[cfg(windows)]
fn decode_native_spelling(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    let mut units = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        units.push(u16::from_le_bytes([pair[0], pair[1]]));
    }
    if units.len() * 2 != bytes.len() {
        return None;
    }
    Some(PathBuf::from(OsString::from_wide(&units)))
}

/// Current UTC timestamp as `YYYY-MM-DDTHH:MM:SSZ`. New catalog timestamps are
/// always UTC with an explicit `Z`; imported legacy strings stay raw in their
/// own columns.
pub fn utc_timestamp_now() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    utc_timestamp(seconds)
}

/// Formats UNIX seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn utc_timestamp(unix_seconds: u64) -> String {
    let days = (unix_seconds / 86_400) as i64;
    let remainder = unix_seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        remainder / 3600,
        (remainder % 3600) / 60,
        remainder % 60
    )
}

/// Days since 1970-01-01 to civil date (Howard Hinnant's algorithm, proleptic
/// Gregorian, using Euclidean division so pre-epoch values stay correct).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = shifted_month + if shifted_month < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}

fn run_worker(path: PathBuf, queue: Receiver<Request>) {
    match open_catalog(&path) {
        Ok(mut connection) => {
            while let Ok(request) = queue.recv() {
                request(Ok(&mut connection));
            }
            // Dropping the connection here closes WAL cleanly and checkpoints.
        }
        Err(error) => {
            // Nothing was opened; every queued request gets the same readable
            // cause instead of hanging or panicking.
            for request in queue {
                request(Err(error.clone()));
            }
        }
    }
}

fn open_catalog(path: &Path) -> StorageResult<Connection> {
    if !path.is_absolute() {
        return Err(StorageError::new(
            StorageCategory::Path,
            Some(path),
            "the catalog path must be absolute",
        ));
    }
    let location = classify_location(&resolved_location_path(path)?);
    if let Some((file_system, mount_point)) = &location.network {
        return Err(StorageError::new(
            StorageCategory::RefusedLocation,
            Some(path),
            format!(
                "the catalog must be local; this path is on a {file_system} network mount ({}), and SQLite over a network share is not a supported writable mode",
                mount_point.display()
            ),
        ));
    }
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|err| {
                StorageError::new(
                    StorageCategory::Io,
                    Some(parent),
                    format!("could not create the catalog directory: {err}"),
                )
            })?;
        }
    }

    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let mut connection = Connection::open_with_flags(path, flags)
        .map_err(|err| db_error("could not open the catalog", Some(path), err))?;
    connection
        .busy_timeout(BUSY_TIMEOUT)
        .map_err(|err| db_error("could not apply the 5 second busy timeout", Some(path), err))?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|err| db_error("could not enable foreign keys", Some(path), err))?;
    if pragma_i64(&connection, "foreign_keys")? != 1 {
        return Err(StorageError::new(
            StorageCategory::Database,
            Some(path),
            "foreign key enforcement could not be enabled on this connection",
        ));
    }
    // Recognize the database before changing its persistent journal mode.
    prepare_schema(&mut connection, path)?;
    if location.local {
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .map_err(|err| db_error("could not switch the catalog to WAL", Some(path), err))?;
        let mode = pragma_text(&connection, "journal_mode")?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StorageError::new(
                StorageCategory::Database,
                Some(path),
                format!("the local catalog could not be switched to WAL (journal_mode = {mode})"),
            ));
        }
    }
    Ok(connection)
}

/// Recognizes the catalog before touching it: refuses foreign/legacy databases,
/// migrates older catalogs transactionally and verifies a current one.
fn prepare_schema(connection: &mut Connection, path: &Path) -> StorageResult<()> {
    let application_id = pragma_i64(connection, "application_id")?;
    let version = pragma_i64(connection, "user_version")?;
    if application_id != 0 && application_id != i64::from(APPLICATION_ID) {
        return Err(StorageError::new(
            StorageCategory::InvalidSchema,
            Some(path),
            format!(
                "this database belongs to another application (application_id 0x{application_id:08X}); refusing to use or modify it"
            ),
        ));
    }
    if application_id == 0 {
        let tables = user_tables(connection)?;
        if !tables.is_empty() {
            return Err(StorageError::new(
                StorageCategory::InvalidSchema,
                Some(path),
                format!(
                    "unrecognized existing database with {} tables and no VFX Launcher application id ({}); a legacy or foreign database is refused and left untouched",
                    tables.len(),
                    tables.join(", ")
                ),
            ));
        }
        // An empty file: create this application's catalog.
        migrate(connection, 0, path)?;
        return verify_tables(connection, path);
    }
    if version > SCHEMA_VERSION {
        return Err(StorageError::new(
            StorageCategory::InvalidSchema,
            Some(path),
            format!(
                "the catalog uses schema version {version}, but this build supports {SCHEMA_VERSION}; refusing to downgrade it"
            ),
        ));
    }
    migrate(connection, version, path)?;
    verify_tables(connection, path)
}

/// Applies schema steps newer than `from` inside one transaction, including the
/// `user_version` bump, so a failed migration leaves the previous schema and
/// version in place.
fn migrate(connection: &mut Connection, from: i64, path: &Path) -> StorageResult<()> {
    if from >= SCHEMA_VERSION {
        return Ok(());
    }
    let transaction = connection
        .transaction()
        .map_err(|err| db_error("could not begin the schema migration", Some(path), err))?;
    if from == 0 {
        transaction
            .pragma_update(None, "application_id", APPLICATION_ID)
            .map_err(|err| {
                db_error(
                    "could not stamp the catalog application id",
                    Some(path),
                    err,
                )
            })?;
    }
    let mut current = from;
    for (version, statements) in SCHEMA_STEPS {
        if *version <= current {
            continue;
        }
        transaction
            .execute_batch(statements)
            .map_err(|err| db_error("could not create the catalog schema", Some(path), err))?;
        transaction
            .pragma_update(None, "user_version", *version)
            .map_err(|err| {
                db_error(
                    "could not record the catalog schema version",
                    Some(path),
                    err,
                )
            })?;
        current = *version;
    }
    if current != SCHEMA_VERSION {
        return Err(StorageError::new(
            StorageCategory::InvalidSchema,
            Some(path),
            format!(
                "no transactional migration path from schema version {from} to {SCHEMA_VERSION}"
            ),
        ));
    }
    transaction
        .commit()
        .map_err(|err| db_error("could not commit the catalog schema", Some(path), err))
}

fn user_tables(connection: &Connection) -> StorageResult<Vec<String>> {
    let mut statement = connection
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .map_err(|err| db_error("could not read the catalog table list", None, err))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|err| db_error("could not read the catalog table list", None, err))?;
    let mut names = Vec::new();
    for row in rows {
        names.push(row.map_err(|err| db_error("could not read a catalog table name", None, err))?);
    }
    Ok(names)
}

fn verify_tables(connection: &Connection, path: &Path) -> StorageResult<()> {
    let present: HashSet<String> = user_tables(connection)?.into_iter().collect();
    let missing: Vec<&str> = EXPECTED_TABLES
        .iter()
        .copied()
        .filter(|name| !present.contains(*name))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(StorageError::new(
        StorageCategory::InvalidSchema,
        Some(path),
        format!(
            "the catalog schema is incomplete; missing tables: {}",
            missing.join(", ")
        ),
    ))
}

fn pragma_i64(connection: &Connection, name: &str) -> StorageResult<i64> {
    connection
        .pragma_query_value(None, name, |row| row.get::<_, i64>(0))
        .map_err(|err| db_error(&format!("could not read PRAGMA {name}"), None, err))
}

fn pragma_text(connection: &Connection, name: &str) -> StorageResult<String> {
    connection
        .pragma_query_value(None, name, |row| row.get::<_, String>(0))
        .map_err(|err| db_error(&format!("could not read PRAGMA {name}"), None, err))
}

fn db_error(context: &str, path: Option<&Path>, error: rusqlite::Error) -> StorageError {
    let (category, cause) = match &error {
        rusqlite::Error::SqliteFailure(failure, message) => {
            let detail = match message {
                Some(text) if !text.is_empty() => text.clone(),
                _ => format!("SQLite code {:?}", failure.code),
            };
            match failure.code {
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked => (
                    StorageCategory::Busy,
                    format!("the catalog is busy or locked by another connection ({detail})"),
                ),
                rusqlite::ErrorCode::ConstraintViolation => (
                    StorageCategory::InvalidInput,
                    format!("the request conflicts with existing catalog data ({detail})"),
                ),
                rusqlite::ErrorCode::CannotOpen => (
                    StorageCategory::Path,
                    format!("the catalog file could not be opened ({detail})"),
                ),
                rusqlite::ErrorCode::ReadOnly | rusqlite::ErrorCode::DiskFull => (
                    StorageCategory::Io,
                    format!("the catalog file could not be written ({detail})"),
                ),
                _ => (
                    StorageCategory::Database,
                    format!("the catalog database reported an error ({detail})"),
                ),
            }
        }
        rusqlite::Error::QueryReturnedNoRows => (
            StorageCategory::InvalidInput,
            "the requested catalog row does not exist".to_string(),
        ),
        other => (
            StorageCategory::Database,
            format!("the catalog database reported an error: {other}"),
        ),
    };
    StorageError::new(category, path, format!("{context}: {cause}"))
}

/// Table set created by schema version 1; used to detect a truncated or foreign
/// catalog that carries our application id.
const EXPECTED_TABLES: &[&str] = &[
    "users",
    "projects",
    "project_scan_folders",
    "files",
    "project_files",
    "favorites",
    "recents",
    "file_access_log",
    "user_activity_log",
    "import_origins",
    "import_origin_rows",
    "operation_runs",
    "operation_items",
];

/// Ordered schema steps: `(user_version after the step, statements)`.
const SCHEMA_STEPS: &[(i64, &str)] = &[(1, SCHEMA_V1)];

/// Minimum catalog schema (`CONTRACTS.md` §3). Native path columns are
/// `*_storage` (key bytes), `platform`, `*_spelling` (native bytes) and `*_display`
/// (UTF-8 spelling or NULL). Tombstoning, not deletion, preserves attribution.
const SCHEMA_V1: &str = r#"
CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL CHECK (length(trim(username)) BETWEEN 1 AND 64),
    is_admin INTEGER NOT NULL DEFAULT 0 CHECK (is_admin IN (0, 1)),
    created_at TEXT NOT NULL,
    deleted_at TEXT
);
CREATE UNIQUE INDEX users_live_username ON users(username) WHERE deleted_at IS NULL;

CREATE TABLE projects (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    root_storage BLOB,
    root_platform TEXT NOT NULL CHECK (root_platform IN ('posix', 'windows', 'unknown')),
    root_spelling BLOB NOT NULL,
    root_display TEXT,
    added_by INTEGER REFERENCES users(id) ON DELETE SET NULL,
    added_at TEXT NOT NULL,
    removed_at TEXT,
    UNIQUE (root_platform, root_storage)
);

CREATE TABLE project_scan_folders (
    id INTEGER PRIMARY KEY,
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    storage BLOB,
    platform TEXT NOT NULL CHECK (platform IN ('posix', 'windows', 'unknown')),
    spelling BLOB NOT NULL,
    display TEXT,
    added_at TEXT NOT NULL,
    UNIQUE (project_id, storage)
);

CREATE TABLE files (
    id INTEGER PRIMARY KEY,
    storage BLOB,
    platform TEXT NOT NULL CHECK (platform IN ('posix', 'windows', 'unknown')),
    spelling BLOB NOT NULL,
    display TEXT,
    filename TEXT NOT NULL,
    version TEXT,
    version_number INTEGER,
    filetype TEXT,
    size_bytes INTEGER,
    modified_unix_ns INTEGER,
    modified_raw TEXT,
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT,
    UNIQUE (platform, storage)
);
CREATE INDEX files_by_filename ON files(filename);

CREATE TABLE project_files (
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    added_at TEXT NOT NULL,
    last_seen_at TEXT,
    PRIMARY KEY (project_id, file_id)
) WITHOUT ROWID;
CREATE INDEX project_files_by_file ON project_files(file_id);

CREATE TABLE favorites (
    user_id INTEGER NOT NULL REFERENCES users(id),
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL,
    PRIMARY KEY (user_id, project_id)
) WITHOUT ROWID;

CREATE TABLE recents (
    user_id INTEGER NOT NULL REFERENCES users(id),
    project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    accessed_at TEXT NOT NULL,
    PRIMARY KEY (user_id, project_id)
) WITHOUT ROWID;

CREATE TABLE file_access_log (
    id INTEGER PRIMARY KEY,
    file_id INTEGER REFERENCES files(id) ON DELETE SET NULL,
    user_id INTEGER REFERENCES users(id),
    accessed_at TEXT NOT NULL
);
CREATE INDEX file_access_log_by_file ON file_access_log(file_id, accessed_at);

CREATE TABLE user_activity_log (
    id INTEGER PRIMARY KEY,
    user_id INTEGER REFERENCES users(id),
    username TEXT NOT NULL,
    action_type TEXT NOT NULL,
    description TEXT NOT NULL,
    at TEXT NOT NULL
);

CREATE TABLE import_origins (
    id INTEGER PRIMARY KEY,
    source_kind TEXT NOT NULL,
    source_spelling BLOB NOT NULL,
    source_display TEXT,
    imported_at TEXT NOT NULL,
    notes TEXT
);

CREATE TABLE import_origin_rows (
    origin_id INTEGER NOT NULL REFERENCES import_origins(id) ON DELETE CASCADE,
    legacy_table TEXT NOT NULL,
    legacy_id INTEGER NOT NULL,
    local_table TEXT NOT NULL,
    local_id INTEGER NOT NULL,
    PRIMARY KEY (origin_id, legacy_table, legacy_id)
) WITHOUT ROWID;

CREATE TABLE operation_runs (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('structure', 'incoming', 'extension_copy', 'backup', 'ai')),
    state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'succeeded', 'partial', 'failed', 'cancelled', 'interrupted')),
    started_at TEXT NOT NULL,
    finished_at TEXT,
    detail TEXT
);

CREATE TABLE operation_items (
    id INTEGER PRIMARY KEY,
    run_id INTEGER NOT NULL REFERENCES operation_runs(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending', 'running', 'copied', 'moved', 'created', 'skipped', 'copied_source_retained', 'failed', 'cancelled')),
    source_spelling BLOB,
    source_display TEXT,
    destination_spelling BLOB,
    destination_display TEXT,
    message TEXT,
    UNIQUE (run_id, ordinal)
);
"#;

/// Where the catalog file lives, as far as this build can prove.
#[derive(Clone, Debug)]
struct TargetLocation {
    /// The target is on a mount this build can certify as local, so WAL is
    /// applied. `false` means "unknown", not "remote".
    local: bool,
    /// Set only when the containing mount is a known network file system.
    network: Option<(String, PathBuf)>,
}

impl TargetLocation {
    fn unknown() -> Self {
        Self {
            local: false,
            network: None,
        }
    }
}

/// Network file systems whose SQLite locking cannot be trusted. This is a
/// best-effort refusal: a bind-mounted remote directory inside a local mount is
/// not distinguishable from a local one, so a positive local result is not a
/// certificate either.
const NETWORK_FILE_SYSTEMS: &[&str] = &[
    "9p",
    "afs",
    "beegfs",
    "ceph",
    "cifs",
    "cvmfs",
    "davfs",
    "davfs2",
    "fuse.davfs2",
    "fuse.nfs",
    "fuse.rclone",
    "fuse.s3fs",
    "fuse.smbnetfs",
    "fuse.sshfs",
    "gcsfuse",
    "glusterfs",
    "lustre",
    "ncpfs",
    "nfs",
    "nfs4",
    "s3fs",
    "smb2",
    "smb3",
    "smbfs",
];

#[cfg(target_os = "linux")]
fn classify_location(path: &Path) -> TargetLocation {
    match linux_mount_containing(path) {
        Some((file_system, mount_point)) if is_network_file_system(&file_system) => {
            TargetLocation {
                local: false,
                network: Some((file_system, mount_point)),
            }
        }
        Some(_) => TargetLocation {
            local: true,
            network: None,
        },
        None => TargetLocation::unknown(),
    }
}

#[cfg(not(target_os = "linux"))]
fn classify_location(path: &Path) -> TargetLocation {
    // Windows drive types and non-Linux Unix mounts are not inspected here: the
    // path alone cannot certify a local target, so WAL is not imposed and the
    // limitation is reported instead of guessed.
    let _ = path;
    TargetLocation::unknown()
}

#[cfg(target_os = "linux")]
fn is_network_file_system(file_system: &str) -> bool {
    NETWORK_FILE_SYSTEMS.contains(&file_system)
}

/// Resolve existing ancestors so a local symlink cannot disguise a network
/// target. Missing suffix components are appended without creating anything.
fn resolved_location_path(path: &Path) -> StorageResult<PathBuf> {
    let mut ancestor = path.to_path_buf();
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(&ancestor) {
            Ok(mut resolved) => {
                for component in suffix.into_iter().rev() {
                    resolved.push(component);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor.file_name().ok_or_else(|| {
                    StorageError::new(
                        StorageCategory::Path,
                        Some(path),
                        "could not resolve the catalog location",
                    )
                })?;
                suffix.push(name.to_os_string());
                if !ancestor.pop() {
                    return Err(StorageError::new(
                        StorageCategory::Path,
                        Some(path),
                        "could not resolve the catalog location",
                    ));
                }
            }
            Err(error) => {
                return Err(StorageError::new(
                    StorageCategory::Io,
                    Some(&ancestor),
                    format!("could not inspect the catalog location: {error}"),
                ));
            }
        }
    }
}

/// `(file system type, mount point)` of the longest mount point containing
/// `path`, from `/proc/self/mountinfo`. Lexical containment only: the path does
/// not have to exist, and symlinks are not resolved.
#[cfg(target_os = "linux")]
fn linux_mount_containing(path: &Path) -> Option<(String, PathBuf)> {
    let text = fs::read_to_string("/proc/self/mountinfo").ok()?;
    let mut best: Option<(usize, String, PathBuf)> = None;
    for line in text.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        if fields.len() < 6 {
            continue;
        }
        let separator = fields.iter().position(|field| *field == "-")?;
        let file_system = fields.get(separator + 1)?;
        let mount_bytes = unescape_mountinfo(fields[4]);
        let mount_point = os_string_from_bytes(mount_bytes);
        let mount_point = match mount_point {
            Some(value) => PathBuf::from(value),
            None => continue,
        };
        if !path.starts_with(&mount_point) {
            continue;
        }
        let depth = mount_point.components().count();
        let better = match &best {
            Some((best_depth, _, _)) => depth > *best_depth,
            None => true,
        };
        if better {
            best = Some((depth, (*file_system).to_string(), mount_point));
        }
    }
    best.map(|(_, file_system, mount_point)| (file_system, mount_point))
}

#[cfg(target_os = "linux")]
fn os_string_from_bytes(bytes: Vec<u8>) -> Option<OsString> {
    use std::os::unix::ffi::OsStringExt;
    Some(OsString::from_vec(bytes))
}

/// `/proc/self/mountinfo` escapes space, tab, newline and backslash as `\NNN`
/// octal sequences.
#[cfg(target_os = "linux")]
fn unescape_mountinfo(field: &str) -> Vec<u8> {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let digits = &field[index + 1..index + 4];
            if let Ok(value) = u8::from_str_radix(digits, 8) {
                out.push(value);
                index += 4;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}
