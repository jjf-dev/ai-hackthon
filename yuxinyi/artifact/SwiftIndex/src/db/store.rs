use anyhow::{Context, Result};
use rusqlite::{params, Connection, Error as SqlError, ErrorCode, Transaction};

use crate::{
    extractor::{ExtractedChunk, ExtractedSymbol, PendingEdge},
    git::stats::GitMetrics,
    model::{FileImportRecord, FileRecord},
};

/// Minimal file state needed for incremental updates.
#[derive(Debug, Clone)]
pub struct IndexedFileState {
    pub file_id: i64,
    pub path: String,
    pub hash: String,
    pub size: i64,
    pub mtime_ns: Option<i64>,
}

/// Remove all indexed content before a full rebuild.
pub fn clear_all(conn: &mut Connection) -> Result<()> {
    let tx = conn
        .transaction()
        .context("Failed to start reset transaction")?;
    tx.execute_batch(
        r#"
        DELETE FROM chunks_fts;
        DELETE FROM symbols_fts;
        DELETE FROM files_fts;
        DELETE FROM entrypoints;
        DELETE FROM file_test_edges;
        DELETE FROM symbol_test_edges;
        DELETE FROM git_cochange;
        DELETE FROM git_file_stats;
        DELETE FROM file_imports;
        DELETE FROM symbol_edges;
        DELETE FROM chunks;
        DELETE FROM symbols;
        DELETE FROM files;
        "#,
    )
    .context("Failed to clear existing index data")?;
    tx.commit().context("Failed to commit full reset")?;
    Ok(())
}

/// Load persisted file states for incremental comparison.
pub fn load_file_states(conn: &Connection) -> Result<Vec<IndexedFileState>> {
    let mut stmt =
        conn.prepare("SELECT id, path, hash, size, mtime_ns FROM files ORDER BY path")?;
    let rows = stmt.query_map([], |row| {
        Ok(IndexedFileState {
            file_id: row.get(0)?,
            path: row.get(1)?,
            hash: row.get(2)?,
            size: row.get(3)?,
            mtime_ns: row.get(4)?,
        })
    })?;
    let mut states = Vec::new();
    for row in rows {
        states.push(row?);
    }
    Ok(states)
}

/// Load file ids keyed by workspace-relative path.
pub fn load_file_id_map(conn: &Connection) -> Result<std::collections::HashMap<String, i64>> {
    let mut stmt = conn.prepare("SELECT id, path FROM files")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut mapping = std::collections::HashMap::new();
    for row in rows {
        let (id, path) = row?;
        mapping.insert(path, id);
    }
    Ok(mapping)
}

/// Delete a file bundle and its FTS rows inside an existing transaction.
pub fn delete_file_bundle(tx: &Transaction<'_>, file_id: i64) -> Result<()> {
    tx.execute(
        "DELETE FROM chunks_fts WHERE rowid IN (SELECT id FROM chunks WHERE file_id = ?1)",
        [file_id],
    )
    .with_context(|| format!("Failed to delete chunk FTS rows for file_id={file_id}"))?;
    tx.execute(
        "DELETE FROM symbols_fts WHERE rowid IN (SELECT id FROM symbols WHERE file_id = ?1)",
        [file_id],
    )
    .with_context(|| format!("Failed to delete symbol FTS rows for file_id={file_id}"))?;
    tx.execute("DELETE FROM files_fts WHERE rowid = ?1", [file_id])
        .with_context(|| format!("Failed to delete file FTS row for file_id={file_id}"))?;
    tx.execute("DELETE FROM files WHERE id = ?1", [file_id])
        .with_context(|| format!("Failed to delete file record for file_id={file_id}"))?;
    Ok(())
}

/// Refresh metadata for unchanged content when only mtime or size changed.
pub fn touch_file_metadata(
    tx: &Transaction<'_>,
    file_id: i64,
    size: i64,
    mtime_ns: Option<i64>,
    indexed_at: i64,
) -> Result<()> {
    tx.execute(
        "UPDATE files SET size = ?2, mtime_ns = ?3, indexed_at = ?4 WHERE id = ?1",
        params![file_id, size, mtime_ns, indexed_at],
    )
    .with_context(|| format!("Failed to refresh metadata for file_id={file_id}"))?;
    Ok(())
}

/// Replace git-derived metrics after a build or update.
pub fn replace_git_metrics(
    conn: &mut Connection,
    file_ids_by_path: &std::collections::HashMap<String, i64>,
    metrics: &GitMetrics,
) -> Result<()> {
    let tx = conn
        .transaction()
        .context("Failed to start git metrics transaction")?;
    tx.execute("DELETE FROM git_cochange", [])
        .context("Failed to clear git_cochange")?;
    tx.execute("DELETE FROM git_file_stats", [])
        .context("Failed to clear git_file_stats")?;

    for stat in &metrics.file_stats {
        let Some(file_id) = file_ids_by_path.get(&stat.path) else {
            continue;
        };
        tx.execute(
            r#"
            INSERT INTO git_file_stats (file_id, commit_count, last_modified, last_author, authors_json)
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
            params![
                file_id,
                stat.commit_count,
                stat.last_modified,
                stat.last_author,
                serde_json::to_string(&stat.authors)?,
            ],
        )
        .with_context(|| format!("Failed to insert git stats for {}", stat.path))?;
    }

    for cochange in &metrics.cochanges {
        let Some(file_id_a) = file_ids_by_path.get(&cochange.path_a) else {
            continue;
        };
        let Some(file_id_b) = file_ids_by_path.get(&cochange.path_b) else {
            continue;
        };
        if file_id_a == file_id_b {
            continue;
        }
        let (left, right) = if file_id_a < file_id_b {
            (*file_id_a, *file_id_b)
        } else {
            (*file_id_b, *file_id_a)
        };
        tx.execute(
            r#"
            INSERT INTO git_cochange (file_id_a, file_id_b, cochange_count)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(file_id_a, file_id_b)
            DO UPDATE SET cochange_count = excluded.cochange_count
            "#,
            params![left, right, cochange.cochange_count],
        )
        .with_context(|| {
            format!(
                "Failed to insert git cochange for {} and {}",
                cochange.path_a, cochange.path_b
            )
        })?;
    }

    tx.commit().context("Failed to commit git metrics")?;
    Ok(())
}

/// Persist one fully extracted file inside an existing transaction.
pub fn insert_file_bundle(
    tx: &Transaction<'_>,
    file: &FileRecord,
    imports: &[FileImportRecord],
    symbols: &[ExtractedSymbol],
    chunks: &[ExtractedChunk],
    edges: &[PendingEdge],
) -> Result<i64> {
    tx.execute(
        r#"
        INSERT INTO files (
            path, crate_name, module_path, hash, size, mtime_ns, line_count,
            symbol_count, is_generated, has_tests, summary, indexed_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
        "#,
        params![
            file.path,
            file.crate_name,
            file.module_path,
            file.hash,
            file.size,
            file.mtime_ns,
            file.line_count,
            file.symbol_count,
            file.is_generated as i64,
            file.has_tests as i64,
            file.summary,
            file.indexed_at,
        ],
    )
    .with_context(|| format!("Failed to insert file {}", file.path))?;
    let file_id = tx.last_insert_rowid();

    tx.execute(
        "INSERT INTO files_fts(rowid, path, crate_name, module_path, summary) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![file_id, file.path, file.crate_name, file.module_path, file.summary],
    )
    .with_context(|| format!("Failed to update files FTS for {}", file.path))?;

    for import in imports {
        tx.execute(
            "INSERT INTO file_imports (file_id, import_path, alias, is_glob) VALUES (?1, ?2, ?3, ?4)",
            params![file_id, import.import_path, import.alias, import.is_glob as i64],
        )
        .with_context(|| format!("Failed to insert import for {}", file.path))?;
    }

    let mut symbol_ids = Vec::with_capacity(symbols.len());
    for symbol in symbols {
        let parent_symbol_id = symbol
            .parent_local
            .and_then(|index| symbol_ids.get(index).copied());
        let qualname = insert_symbol_record(tx, file_id, parent_symbol_id, file, symbol)?;
        let symbol_id = tx.last_insert_rowid();
        symbol_ids.push(symbol_id);

        tx.execute(
            "INSERT INTO symbols_fts(rowid, name, qualname, signature, docs, summary) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                symbol_id,
                symbol.record.name,
                qualname,
                symbol.record.signature,
                symbol.record.docs,
                symbol.record.summary,
            ],
        )
        .with_context(|| format!("Failed to update symbol FTS in {}", file.path))?;
    }

    for chunk in chunks {
        let symbol_id = chunk
            .symbol_local
            .and_then(|index| symbol_ids.get(index).copied());
        tx.execute(
            r#"
            INSERT INTO chunks (file_id, symbol_id, chunk_kind, start_line, end_line, summary, content)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
            params![
                file_id,
                symbol_id,
                chunk.record.chunk_kind,
                chunk.record.start_line,
                chunk.record.end_line,
                chunk.record.summary,
                chunk.record.content,
            ],
        )
        .with_context(|| format!("Failed to insert chunk in {}", file.path))?;
        let chunk_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO chunks_fts(rowid, content, summary) VALUES (?1, ?2, ?3)",
            params![chunk_id, chunk.record.content, chunk.record.summary],
        )
        .with_context(|| format!("Failed to update chunk FTS in {}", file.path))?;
    }

    for edge in edges {
        let from_symbol_id = edge
            .from_local
            .and_then(|index| symbol_ids.get(index).copied());
        let to_symbol_id = edge
            .to_local
            .and_then(|index| symbol_ids.get(index).copied());
        tx.execute(
            "INSERT INTO symbol_edges (from_symbol_id, to_symbol_id, edge_type, evidence) VALUES (?1, ?2, ?3, ?4)",
            params![from_symbol_id, to_symbol_id, edge.edge_type.as_str(), edge.evidence],
        )
        .with_context(|| format!("Failed to insert edge in {}", file.path))?;
    }

    Ok(file_id)
}

fn insert_symbol_record(
    tx: &Transaction<'_>,
    file_id: i64,
    parent_symbol_id: Option<i64>,
    file: &FileRecord,
    symbol: &ExtractedSymbol,
) -> Result<String> {
    let mut qualname = symbol.record.qualname.clone();
    let suffix = fallback_symbol_suffix(file, symbol);
    let mut attempts = 0usize;

    loop {
        let result = tx.execute(
            r#"
            INSERT INTO symbols (
                file_id, parent_symbol_id, kind, name, qualname, signature, docs,
                start_line, end_line, is_async, is_test, visibility, return_type, summary
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
            "#,
            params![
                file_id,
                parent_symbol_id,
                symbol.record.kind.as_str(),
                symbol.record.name,
                qualname,
                symbol.record.signature,
                symbol.record.docs,
                symbol.record.start_line,
                symbol.record.end_line,
                symbol.record.is_async as i64,
                symbol.record.is_test as i64,
                symbol.record.visibility,
                symbol.record.return_type,
                symbol.record.summary,
            ],
        );

        match result {
            Ok(_) => return Ok(qualname),
            Err(error) if is_symbol_qualname_conflict(&error) => {
                attempts += 1;
                qualname = format!("{}{}{}", symbol.record.qualname, suffix, attempts);
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Failed to insert symbol in {}", file.path));
            }
        }
    }
}

fn fallback_symbol_suffix(file: &FileRecord, symbol: &ExtractedSymbol) -> String {
    let path = file.path.replace('/', "::");
    format!(
        "@{}::L{}::K{}::",
        path,
        symbol.record.start_line,
        symbol.record.kind.as_str()
    )
}

fn is_symbol_qualname_conflict(error: &SqlError) -> bool {
    matches!(
        error,
        SqlError::SqliteFailure(inner, Some(message))
            if inner.code == ErrorCode::ConstraintViolation && message.contains("symbols.qualname")
    )
}
