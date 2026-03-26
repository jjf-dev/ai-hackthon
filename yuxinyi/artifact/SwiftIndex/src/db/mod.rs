pub mod connection;
pub mod migrations;
pub mod store;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;

/// Thin database wrapper that owns the SQLite connection.
pub struct Database {
    conn: Connection,
    path: PathBuf,
}

impl Database {
    /// Open the database and ensure migrations are applied.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = connection::open(path)?;
        migrations::run(&conn)?;
        Ok(Self {
            conn,
            path: path.to_path_buf(),
        })
    }

    /// Create or open the default database under the workspace root.
    pub fn open_for_workspace(workspace_root: &Path, override_path: Option<&Path>) -> Result<Self> {
        let db_path = override_path
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace_root.join(".swiftindex").join("index.db"));
        Self::open(&db_path).with_context(|| {
            format!(
                "Failed to open index database at {}",
                db_path.as_path().display()
            )
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}
