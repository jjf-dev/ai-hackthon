use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

const CURRENT_SCHEMA_VERSION: i64 = 2;

/// Apply embedded schema migrations.
pub fn run(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    )
    .context("Failed to initialize schema metadata")?;

    let current = schema_version(conn)?;
    if current < 1 {
        conn.execute_batch(include_str!("migrations/001_initial.sql"))
            .context("Failed to apply schema migration 001_initial.sql")?;
        set_schema_version(conn, 1)?;
    }
    if current < 2 {
        conn.execute_batch(include_str!("migrations/002_v1_1.sql"))
            .context("Failed to apply schema migration 002_v1_1.sql")?;
        set_schema_version(conn, 2)?;
    }

    if schema_version(conn)? != CURRENT_SCHEMA_VERSION {
        set_schema_version(conn, CURRENT_SCHEMA_VERSION)?;
    }

    Ok(())
}

fn schema_version(conn: &Connection) -> Result<i64> {
    let value = conn
        .query_row(
            "SELECT value FROM schema_meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;

    Ok(value.as_deref().unwrap_or("0").parse::<i64>().unwrap_or(0))
}

fn set_schema_version(conn: &Connection, version: i64) -> Result<()> {
    conn.execute(
        "INSERT INTO schema_meta (key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![version.to_string()],
    )
    .context("Failed to persist schema version")?;
    Ok(())
}
