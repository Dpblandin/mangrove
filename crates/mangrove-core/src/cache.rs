use std::{
    fs,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::Mutex,
};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use crate::{model::CommitRow, Result};

// SQLite's Windows VFS opens databases without FILE_SHARE_DELETE. Keeping an
// idle handle there would prevent the supported cache-removal/reopen workflow.
// Unix readers can keep their statement cache without pinning the file name.
const MAX_IDLE_READS: usize = if cfg!(unix) { 4 } else { 0 };

pub(crate) fn connect(path: &Path, read_only: bool) -> Result<Connection> {
    ensure_owned_database(path)?;
    let flags = if read_only {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
    };
    let connection = Connection::open_with_flags(path, flags | OpenFlags::SQLITE_OPEN_NOFOLLOW)
        .map_err(|e| e.to_string())?;
    connection
        .busy_timeout(std::time::Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    if !read_only {
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| e.to_string())?;
    }
    Ok(connection)
}

fn ensure_owned_database(path: &Path) -> Result<()> {
    // Check sidecars on every acquisition, including reused connections. An
    // alias into the source repository must never bypass the containment guard.
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        ensure_owned_file(Path::new(&file))?;
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
}

impl FileIdentity {
    fn read(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        Ok(Self {
            modified: metadata.modified().ok(),
        })
    }
}

pub(crate) struct ReadPool {
    path: PathBuf,
    available: Mutex<Vec<(Connection, FileIdentity)>>,
}

impl ReadPool {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            available: Mutex::default(),
        }
    }

    pub(crate) fn acquire(&self) -> Result<ReadLease<'_>> {
        ensure_owned_database(&self.path)?;
        let identity = FileIdentity::read(&self.path)?;
        let reused = self
            .available
            .lock()
            .unwrap()
            .pop()
            .filter(|(_, previous)| previous == &identity);
        let connection = match reused {
            Some((connection, _)) => connection,
            None => connect(&self.path, true)?,
        };
        Ok(ReadLease {
            connection: Some(connection),
            identity: Some(identity),
            pool: self,
        })
    }
}

pub(crate) struct ReadLease<'a> {
    connection: Option<Connection>,
    identity: Option<FileIdentity>,
    pool: &'a ReadPool,
}

impl Deref for ReadLease<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.connection.as_ref().unwrap()
    }
}

impl DerefMut for ReadLease<'_> {
    fn deref_mut(&mut self) -> &mut Connection {
        self.connection.as_mut().unwrap()
    }
}

impl Drop for ReadLease<'_> {
    fn drop(&mut self) {
        let mut available = self.pool.available.lock().unwrap();
        // Parallel callers borrow independently; only idle connections are
        // bounded. Returning one never holds a SQLite transaction or a gate.
        if available.len() < MAX_IDLE_READS {
            available.push((
                self.connection.take().unwrap(),
                self.identity.take().unwrap(),
            ));
        }
    }
}

fn ensure_owned_file(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(format!(
                "Impossible de vérifier le fichier de cache : {error}"
            ))
        }
    };
    if !metadata.file_type().is_file() {
        return Err("Un fichier de cache ne doit pas être un lien ou un dossier.".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Err("Un fichier de cache ne doit pas partager de lien physique.".into());
        }
    }
    Ok(())
}

pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version == 3 {
        return Ok(());
    }
    connection
        .execute_batch("PRAGMA journal_mode=WAL; BEGIN IMMEDIATE;")
        .map_err(|e| e.to_string())?;
    // Another Workspace may have completed migration while this connection
    // waited for the writer. Recheck inside the exclusive migration snapshot.
    let version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    if version == 3 {
        return connection
            .execute_batch("COMMIT;")
            .map_err(|e| e.to_string());
    }
    if version != 2 {
        connection
            .execute_batch(
                "DROP TRIGGER IF EXISTS commits_insert;
             DROP TABLE IF EXISTS search;
             DROP TABLE IF EXISTS commits;
             DROP TABLE IF EXISTS walk;
             DROP TABLE IF EXISTS walk_build;
             DROP TABLE IF EXISTS refs_build;
             DROP TABLE IF EXISTS refs;
             DROP TABLE IF EXISTS meta;",
            )
            .map_err(|e| e.to_string())?;
    }
    if version == 2 {
        connection
            .execute_batch(
                "ALTER TABLE walk ADD COLUMN parents TEXT NOT NULL DEFAULT '[]';
                 UPDATE walk SET parents = COALESCE(
                     (SELECT parents FROM commits WHERE commits.sha = walk.sha), '[]');",
            )
            .map_err(|e| e.to_string())?;
    }
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS commits (
             id INTEGER PRIMARY KEY,
             sha TEXT NOT NULL UNIQUE,
             summary TEXT NOT NULL,
             message TEXT NOT NULL,
             author TEXT NOT NULL,
             email TEXT NOT NULL,
             time INTEGER NOT NULL,
             parents TEXT NOT NULL
         );
         CREATE VIRTUAL TABLE IF NOT EXISTS search USING fts5(
             summary, message, author, email,
             content='commits', content_rowid='id',
             tokenize='unicode61', prefix='2 3 4'
         );
         CREATE TRIGGER IF NOT EXISTS commits_insert AFTER INSERT ON commits BEGIN
             INSERT INTO search(rowid, summary, message, author, email)
             VALUES(new.id, new.summary, new.message, new.author, new.email);
         END;
         CREATE TABLE IF NOT EXISTS walk (
             position INTEGER PRIMARY KEY,
             sha TEXT NOT NULL UNIQUE,
             graph TEXT NOT NULL,
             parents TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS walk_build (
             position INTEGER PRIMARY KEY,
             sha TEXT NOT NULL UNIQUE,
             graph TEXT NOT NULL,
             parents TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS refs (
             name TEXT PRIMARY KEY,
             label TEXT NOT NULL,
             sha TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS refs_build (
             name TEXT PRIMARY KEY,
             label TEXT NOT NULL,
             sha TEXT NOT NULL
         );
         INSERT OR IGNORE INTO meta(key, value)
             SELECT 'lease_run', value FROM meta WHERE key = 'index_run';
         INSERT OR IGNORE INTO meta(key, value)
             SELECT 'lease_fingerprint', value FROM meta WHERE key = 'fingerprint';
         INSERT OR IGNORE INTO meta(key, value)
             SELECT 'indexed', CAST(COUNT(*) AS TEXT) FROM walk;
         PRAGMA user_version=3;
         COMMIT;",
        )
        .map_err(|e| e.to_string())
}

pub(crate) fn meta(connection: &Connection, key: &str) -> Result<Option<String>> {
    connection
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|e| e.to_string())
}

pub(crate) fn set_meta(connection: &Connection, key: &str, value: &str) -> Result<()> {
    connection
        .execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES(?1, ?2)",
            params![key, value],
        )
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) fn row_commit(row: &rusqlite::Row<'_>) -> rusqlite::Result<CommitRow> {
    let decode = |column: usize| -> rusqlite::Result<String> { row.get(column) };
    let parents = serde_json::from_str(&decode(5)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(5, rusqlite::types::Type::Text, Box::new(e))
    })?;
    let graph = serde_json::from_str(&decode(7)?).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(CommitRow {
        sha: row.get(0)?,
        summary: row.get(1)?,
        author: row.get(2)?,
        email: row.get(3)?,
        time: row.get(4)?,
        parents,
        position: row_usize(row, 6)?,
        graph,
    })
}

pub(crate) fn row_usize(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<usize> {
    let value: i64 = row.get(column)?;
    usize::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}

pub(crate) const COLUMNS: &str =
    "c.sha, c.summary, c.author, c.email, c.time, w.parents, w.position, w.graph";

pub(crate) fn search_terms(query: &str) -> String {
    let terms = query
        .split_whitespace()
        .filter(|term| term.chars().any(char::is_alphanumeric))
        .take(12)
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ");
    if terms.is_empty() {
        "\"\"".into()
    } else {
        terms
    }
}

pub(crate) fn like_prefix(query: &str) -> String {
    format!(
        "{}%",
        query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_")
    )
}
