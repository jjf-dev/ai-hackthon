use std::{fs, path::Path};

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};

/// Open a SQLite connection with sane defaults for this tool.
pub fn open(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create database directory {}", parent.display()))?;
    }

    let flags = OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_URI;
    let conn = Connection::open_with_flags(path, flags)
        .with_context(|| format!("Failed to open SQLite database {}", path.display()))?;

    conn.execute_batch(
        r#"
        PRAGMA foreign_keys = ON;
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA temp_store = MEMORY;
        PRAGMA mmap_size = 268435456;
        "#,
    )
    .context("Failed to configure SQLite pragmas")?;

    Ok(conn)
}
