use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use rusqlite::Connection;

use crate::{
    db::Database,
    model::{CompactResult, OutlineResult, SnippetResult},
    query::common::QueryOptions,
};

/// Retrieve the outline for a file.
pub fn run(
    workspace_root: &Path,
    db_override: Option<&Path>,
    path: &Path,
    json: bool,
    options: QueryOptions,
) -> Result<()> {
    let db = Database::open_for_workspace(workspace_root, db_override)?;
    if options.compact {
        let outline = get_outline_compact(db.conn(), workspace_root, path, options.expand)?;
        print_compact(&outline, json)?;
    } else {
        let outline = get_outline(db.conn(), workspace_root, path)?;
        print_verbose(&outline, json)?;
    }
    Ok(())
}

/// Read a line-bounded snippet from disk.
pub fn snippet(
    workspace_root: &Path,
    _db_override: Option<&Path>,
    path: &Path,
    start: usize,
    end: usize,
    json: bool,
) -> Result<()> {
    let mut snippet = read_snippet_from_workspace(workspace_root, path, start, end)?;
    snippet.why = vec!["direct line-range snippet request".to_string()];
    if json {
        println!("{}", serde_json::to_string_pretty(&snippet)?);
    } else {
        println!(
            "{}:{}-{}",
            snippet.path, snippet.start_line, snippet.end_line
        );
        println!("Why: {}", snippet.why.join("; "));
        println!();
        print!("{}", snippet.content);
        if !snippet.content.ends_with('\n') {
            println!();
        }
    }
    Ok(())
}

/// Load a snippet directly from the workspace filesystem.
pub fn read_snippet_from_workspace(
    workspace_root: &Path,
    path: &Path,
    start: usize,
    end: usize,
) -> Result<SnippetResult> {
    if start == 0 || end < start {
        bail!("Invalid line range {start}-{end}");
    }
    let absolute = resolve_workspace_file(workspace_root, path);
    let source = fs::read_to_string(&absolute)
        .with_context(|| format!("Failed to read {}", absolute.display()))?;
    let content = source
        .lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let line_no = index + 1;
            if (start..=end).contains(&line_no) {
                Some(line)
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    Ok(SnippetResult {
        path: normalize_workspace_path(workspace_root, path),
        start_line: start,
        end_line: end,
        content: format!("{content}\n"),
        why: Vec::new(),
    })
}

/// Resolve a DB path lookup to the normalized workspace-relative path.
pub fn normalize_workspace_path(workspace_root: &Path, path: &Path) -> String {
    if path.is_absolute() {
        path.strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    } else {
        path.to_string_lossy().replace('\\', "/")
    }
}

pub fn get_outline(conn: &Connection, workspace_root: &Path, path: &Path) -> Result<OutlineResult> {
    let normalized = normalize_workspace_path(workspace_root, path);
    let summary = conn
        .query_row(
            "SELECT COALESCE(summary, '') FROM files WHERE path = ?1",
            [&normalized],
            |row| row.get::<_, String>(0),
        )
        .with_context(|| format!("File `{normalized}` not found in index"))?;

    let imports = collect_strings(
        conn,
        "SELECT import_path FROM file_imports fi JOIN files f ON f.id = fi.file_id WHERE f.path = ?1 ORDER BY import_path",
        &normalized,
    )?;
    let top_level_symbols = collect_symbol_lines(
        conn,
        r#"
        SELECT kind, name, start_line, end_line
        FROM symbols s
        JOIN files f ON f.id = s.file_id
        WHERE f.path = ?1 AND s.parent_symbol_id IS NULL AND s.kind != 'impl'
        ORDER BY s.start_line
        "#,
        &normalized,
    )?;
    let impl_blocks = collect_symbol_lines(
        conn,
        r#"
        SELECT kind, name, start_line, end_line
        FROM symbols s
        JOIN files f ON f.id = s.file_id
        WHERE f.path = ?1 AND s.kind = 'impl'
        ORDER BY s.start_line
        "#,
        &normalized,
    )?;
    let test_functions = collect_symbol_lines(
        conn,
        r#"
        SELECT kind, name, start_line, end_line
        FROM symbols s
        JOIN files f ON f.id = s.file_id
        WHERE f.path = ?1 AND s.is_test = 1
        ORDER BY s.start_line
        "#,
        &normalized,
    )?;

    Ok(OutlineResult {
        path: normalized,
        imports,
        top_level_symbols,
        impl_blocks,
        test_functions,
        summary,
        why: vec!["workspace file outline lookup".to_string()],
    })
}

pub fn get_outline_compact(
    conn: &Connection,
    workspace_root: &Path,
    path: &Path,
    expand: u8,
) -> Result<CompactResult<OutlineResult>> {
    let mut outline = get_outline(conn, workspace_root, path)?;
    let mut truncated = false;

    truncated |= truncate_strings(
        &mut outline.imports,
        match expand {
            0 => 4,
            1 => 6,
            _ => 10,
        },
    );
    truncated |= truncate_strings(
        &mut outline.top_level_symbols,
        match expand {
            0 => 5,
            1 => 8,
            _ => 12,
        },
    );
    truncated |= truncate_strings(
        &mut outline.impl_blocks,
        match expand {
            0 => 3,
            1 => 5,
            _ => 8,
        },
    );
    truncated |= truncate_strings(
        &mut outline.test_functions,
        match expand {
            0 => 2,
            1 => 4,
            _ => 6,
        },
    );

    Ok(CompactResult {
        items: vec![outline],
        confidence: 0.98,
        is_exhaustive: !truncated,
        expansion_hint: if truncated {
            Some("expand outline or read top symbol snippet".to_string())
        } else {
            None
        },
    })
}

fn collect_strings(conn: &Connection, sql: &str, path: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([path], |row| row.get::<_, String>(0))?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    Ok(items)
}

fn collect_symbol_lines(conn: &Connection, sql: &str, path: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([path], |row| {
        Ok(format!(
            "{} {} [{}-{}]",
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, usize>(2)?,
            row.get::<_, usize>(3)?,
        ))
    })?;
    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    Ok(items)
}

fn resolve_workspace_file(workspace_root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace_root.join(path)
    }
}

fn print_compact(result: &CompactResult<OutlineResult>, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
        return Ok(());
    }

    if let Some(outline) = result.items.first() {
        println!("{}", outline.path);
        println!(
            "Summary: {} | confidence {:.2} | exhaustive {}",
            outline.summary, result.confidence, result.is_exhaustive
        );
        if let Some(hint) = &result.expansion_hint {
            println!("Hint: {hint}");
        }
        print_outline_lists(outline);
    }
    Ok(())
}

fn print_verbose(outline: &OutlineResult, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(outline)?);
    } else {
        println!("{}", outline.path);
        println!("Summary: {}", outline.summary);
        println!("Why: {}", outline.why.join("; "));
        print_outline_lists(outline);
    }
    Ok(())
}

fn print_outline_lists(outline: &OutlineResult) {
    if !outline.imports.is_empty() {
        println!();
        println!("Imports:");
        for import in &outline.imports {
            println!("  - {import}");
        }
    }
    if !outline.top_level_symbols.is_empty() {
        println!();
        println!("Top-level symbols:");
        for item in &outline.top_level_symbols {
            println!("  - {item}");
        }
    }
    if !outline.impl_blocks.is_empty() {
        println!();
        println!("Impl blocks:");
        for item in &outline.impl_blocks {
            println!("  - {item}");
        }
    }
    if !outline.test_functions.is_empty() {
        println!();
        println!("Tests:");
        for item in &outline.test_functions {
            println!("  - {item}");
        }
    }
}

fn truncate_strings(items: &mut Vec<String>, max_items: usize) -> bool {
    let was_truncated = items.len() > max_items;
    items.truncate(max_items);
    was_truncated
}
