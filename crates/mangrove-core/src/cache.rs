use std::{fs, path::Path};

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use crate::{model::CommitRow, Result};

pub(crate) fn connect(path: &Path, read_only: bool) -> Result<Connection> {
    // SQLite may write its main file and sidecars. A cache file aliased into a
    // source repository would bypass Workspace's directory containment guard.
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        ensure_owned_file(Path::new(&file))?;
    }
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
    if version == 2 {
        return Ok(());
    }
    if version != 2 {
        connection
            .execute_batch(
                "DROP TRIGGER IF EXISTS commits_insert;
             DROP TABLE IF EXISTS search;
             DROP TABLE IF EXISTS commits;
             DROP TABLE IF EXISTS walk;
             DROP TABLE IF EXISTS refs;
             DROP TABLE IF EXISTS meta;",
            )
            .map_err(|e| e.to_string())?;
    }
    connection
        .execute_batch(
            "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
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
             graph TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS refs (
             name TEXT PRIMARY KEY,
             label TEXT NOT NULL,
             sha TEXT NOT NULL
         );
         PRAGMA user_version=2;",
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
    "c.sha, c.summary, c.author, c.email, c.time, c.parents, w.position, w.graph";

pub(crate) fn search_terms(query: &str) -> String {
    query
        .split_whitespace()
        .take(12)
        .map(|term| format!("\"{}\"*", term.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" AND ")
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
