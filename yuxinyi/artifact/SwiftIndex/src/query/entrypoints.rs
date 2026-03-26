use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;

use crate::{db::Database, model::EntrypointResult};

/// List indexed entrypoints for the workspace.
pub fn run(workspace_root: &Path, db_override: Option<&Path>, json: bool) -> Result<()> {
    let db = Database::open_for_workspace(workspace_root, db_override)?;
    let results = list(db.conn())?;
    if json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else if results.is_empty() {
        println!("No entrypoints indexed");
    } else {
        println!("Indexed entrypoints:");
        for (index, result) in results.iter().enumerate() {
            println!();
            println!("{}. {} {}", index + 1, result.kind, result.path);
            if let Some(qualname) = &result.qualname {
                println!("   Symbol: {qualname}");
            }
            println!("   Score: {:.1}", result.score);
            println!("   Why: {}", result.why.join("; "));
        }
    }
    Ok(())
}

pub fn list(conn: &Connection) -> Result<Vec<EntrypointResult>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            e.kind,
            f.path,
            s.qualname,
            e.score,
            e.reason
        FROM entrypoints e
        JOIN files f ON f.id = e.file_id
        LEFT JOIN symbols s ON s.id = e.symbol_id
        ORDER BY e.score DESC, f.path, COALESCE(s.qualname, '')
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        let reason = row.get::<_, String>(4)?;
        Ok(EntrypointResult {
            kind: row.get(0)?,
            path: row.get(1)?,
            qualname: row.get(2)?,
            score: row.get(3)?,
            why: vec![reason],
        })
    })?;
    let mut results = Vec::new();
    for row in rows {
        results.push(row?);
    }
    Ok(results)
}
