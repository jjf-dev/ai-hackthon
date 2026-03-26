use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

use crate::{
    db::Database,
    model::{CompactResult, NeighborItem, NeighborResult},
    query::common::QueryOptions,
};

/// Retrieve lightweight neighbors for a symbol.
pub fn run(
    workspace_root: &Path,
    db_override: Option<&Path>,
    qualname: &str,
    json: bool,
    options: QueryOptions,
) -> Result<()> {
    let db = Database::open_for_workspace(workspace_root, db_override)?;
    if options.compact {
        let result = get_neighbors_compact(db.conn(), qualname, options.expand)?;
        print_compact(&result, json)?;
    } else {
        let result = get_neighbors(db.conn(), qualname)?;
        print_verbose(&result, json)?;
    }
    Ok(())
}

/// Resolve neighbors around a symbol.
pub fn get_neighbors(conn: &Connection, qualname: &str) -> Result<NeighborResult> {
    let symbol = conn
        .query_row(
            r#"
            SELECT s.id, s.name, s.parent_symbol_id, f.id, f.path
            FROM symbols s
            JOIN files f ON f.id = s.file_id
            WHERE s.qualname = ?1
            "#,
            [qualname],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?
        .with_context(|| format!("Symbol `{qualname}` not found"))?;

    let parent_symbol = match symbol.2 {
        Some(parent_id) => conn
            .query_row(
                "SELECT qualname FROM symbols WHERE id = ?1",
                [parent_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?,
        None => None,
    };

    let methods = direct_child_functions(conn, symbol.0)?;
    let fallback_methods = if methods.is_empty() {
        methods_for_namespace(conn, qualname)?
    } else {
        methods
    };
    let impl_relations = impl_relations(conn, symbol.0, qualname)?;
    let likely_callees = call_neighbors(conn, symbol.0)?;
    let related_tests = related_tests(conn, symbol.0, &symbol.1, symbol.3)?;
    let cochanged_files = cochanged_files(conn, symbol.3)?;
    let entrypoints = entrypoints(conn, symbol.0, symbol.3)?;

    Ok(NeighborResult {
        qualname: qualname.to_string(),
        file_path: symbol.4,
        parent_symbol,
        methods: fallback_methods,
        impl_relations,
        likely_callees,
        related_tests,
        cochanged_files,
        entrypoints,
        why: vec!["expanded symbol graph neighborhood".to_string()],
    })
}

pub fn get_neighbors_compact(
    conn: &Connection,
    qualname: &str,
    expand: u8,
) -> Result<CompactResult<NeighborResult>> {
    let mut result = get_neighbors(conn, qualname)?;
    let mut truncated = false;
    truncated |= limit_items(
        &mut result.methods,
        match expand {
            0 => 3,
            1 => 5,
            _ => 7,
        },
    );
    truncated |= limit_items(
        &mut result.impl_relations,
        match expand {
            0 => 3,
            1 => 5,
            _ => 7,
        },
    );
    truncated |= limit_items(
        &mut result.likely_callees,
        match expand {
            0 => 3,
            1 => 5,
            _ => 7,
        },
    );
    truncated |= limit_items(
        &mut result.related_tests,
        match expand {
            0 => 2,
            1 => 4,
            _ => 6,
        },
    );
    truncated |= limit_items(
        &mut result.cochanged_files,
        match expand {
            0 => 2,
            1 => 4,
            _ => 6,
        },
    );
    truncated |= limit_items(
        &mut result.entrypoints,
        match expand {
            0 => 2,
            1 => 3,
            _ => 4,
        },
    );

    Ok(CompactResult {
        items: vec![result],
        confidence: 0.93,
        is_exhaustive: !truncated,
        expansion_hint: if truncated {
            Some("expand to neighbors or related tests".to_string())
        } else {
            None
        },
    })
}

fn direct_child_functions(conn: &Connection, parent_symbol_id: i64) -> Result<Vec<NeighborItem>> {
    let mut stmt = conn.prepare(
        "SELECT qualname, kind, COALESCE(summary, '') FROM symbols WHERE parent_symbol_id = ?1 AND kind = 'fn' ORDER BY start_line LIMIT 20",
    )?;
    let rows = stmt.query_map([parent_symbol_id], |row| {
        Ok(item(
            row.get(0)?,
            row.get(1)?,
            Some(row.get(2)?),
            "direct child function".to_string(),
        ))
    })?;
    collect_items(rows)
}

fn methods_for_namespace(conn: &Connection, qualname: &str) -> Result<Vec<NeighborItem>> {
    let pattern = format!("{qualname}::%");
    let mut stmt = conn.prepare(
        "SELECT qualname, kind, COALESCE(summary, '') FROM symbols WHERE qualname LIKE ?1 AND kind = 'fn' ORDER BY qualname LIMIT 20",
    )?;
    let rows = stmt.query_map([pattern], |row| {
        Ok(item(
            row.get(0)?,
            row.get(1)?,
            Some(row.get(2)?),
            "same namespace function".to_string(),
        ))
    })?;
    collect_items(rows)
}

fn impl_relations(conn: &Connection, symbol_id: i64, qualname: &str) -> Result<Vec<NeighborItem>> {
    let mut items = Vec::new();
    let mut stmt = conn.prepare(
        r#"
        SELECT
            COALESCE(other.qualname, edge.evidence),
            COALESCE(other.kind, edge.edge_type),
            edge.evidence
        FROM symbol_edges edge
        LEFT JOIN symbols other
            ON (CASE
                    WHEN edge.from_symbol_id = ?1 THEN edge.to_symbol_id
                    ELSE edge.from_symbol_id
                END) = other.id
        WHERE edge.edge_type = 'implements'
          AND (edge.from_symbol_id = ?1 OR edge.to_symbol_id = ?1)
        LIMIT 20
        "#,
    )?;
    let rows = stmt.query_map([symbol_id], |row| {
        let detail = row.get::<_, Option<String>>(2)?;
        Ok(item(
            label_from_row(row.get(0)?, detail.clone()),
            row.get::<_, Option<String>>(1)?
                .unwrap_or_else(|| "implements".to_string()),
            detail.map(pretty_evidence),
            "trait or impl relation".to_string(),
        ))
    })?;
    items.extend(collect_items(rows)?);

    let pattern = format!("{qualname}::%");
    let mut stmt = conn.prepare(
        "SELECT qualname, kind, COALESCE(summary, '') FROM symbols WHERE kind = 'impl' AND qualname LIKE ?1 LIMIT 20",
    )?;
    let rows = stmt.query_map([pattern], |row| {
        Ok(item(
            row.get(0)?,
            row.get(1)?,
            Some(row.get(2)?),
            "matching impl block".to_string(),
        ))
    })?;
    items.extend(collect_items(rows)?);
    Ok(dedup_items(items))
}

fn call_neighbors(conn: &Connection, symbol_id: i64) -> Result<Vec<NeighborItem>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            target.qualname,
            target.kind,
            edge.evidence
        FROM symbol_edges edge
        LEFT JOIN symbols target ON target.id = edge.to_symbol_id
        WHERE edge.from_symbol_id = ?1 AND edge.edge_type = 'calls'
        LIMIT 20
        "#,
    )?;
    let rows = stmt.query_map([symbol_id], |row| {
        let evidence = row.get::<_, Option<String>>(2)?;
        Ok(item(
            label_from_row(row.get(0)?, evidence.clone()),
            row.get::<_, Option<String>>(1)?
                .unwrap_or_else(|| "calls".to_string()),
            evidence.map(pretty_evidence),
            "outgoing call edge".to_string(),
        ))
    })?;
    collect_items(rows)
}

fn related_tests(
    conn: &Connection,
    symbol_id: i64,
    symbol_name: &str,
    file_id: i64,
) -> Result<Vec<NeighborItem>> {
    let mut items = Vec::new();
    let mut stmt = conn.prepare(
        r#"
        SELECT
            tester.qualname,
            'test',
            ste.reason
        FROM symbol_test_edges ste
        JOIN symbols tester ON tester.id = ste.test_symbol_id
        WHERE ste.symbol_id = ?1
        ORDER BY ste.score DESC
        LIMIT 20
        "#,
    )?;
    let rows = stmt.query_map([symbol_id], |row| {
        Ok(item(
            row.get(0)?,
            row.get(1)?,
            Some(row.get(2)?),
            "derived symbol-to-test mapping".to_string(),
        ))
    })?;
    items.extend(collect_items(rows)?);

    let like = format!("%{symbol_name}%");
    let mut stmt = conn.prepare(
        r#"
        SELECT
            tester.qualname,
            'test',
            COALESCE(tester.summary, edge.evidence)
        FROM symbol_edges edge
        JOIN symbols tester ON tester.id = edge.from_symbol_id
        WHERE edge.edge_type = 'tests'
          AND (edge.to_symbol_id = ?1 OR COALESCE(edge.evidence, '') LIKE ?2)
        LIMIT 20
        "#,
    )?;
    let rows = stmt.query_map(params![symbol_id, like], |row| {
        Ok(item(
            row.get(0)?,
            row.get(1)?,
            Some(row.get(2)?),
            "lightweight tests edge".to_string(),
        ))
    })?;
    items.extend(collect_items(rows)?);

    let mut stmt = conn.prepare(
        r#"
        SELECT
            f.path,
            'test_file',
            fte.reason
        FROM file_test_edges fte
        JOIN files f ON f.id = fte.test_file_id
        WHERE fte.file_id = ?1
        ORDER BY fte.score DESC
        LIMIT 10
        "#,
    )?;
    let rows = stmt.query_map([file_id], |row| {
        Ok(item(
            row.get(0)?,
            row.get(1)?,
            Some(row.get(2)?),
            "related test file".to_string(),
        ))
    })?;
    items.extend(collect_items(rows)?);
    Ok(dedup_items(items))
}

fn cochanged_files(conn: &Connection, file_id: i64) -> Result<Vec<NeighborItem>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            f.path,
            'file',
            printf('cochanged %d times', gc.cochange_count)
        FROM git_cochange gc
        JOIN files f
          ON f.id = CASE
                WHEN gc.file_id_a = ?1 THEN gc.file_id_b
                ELSE gc.file_id_a
              END
        WHERE gc.file_id_a = ?1 OR gc.file_id_b = ?1
        ORDER BY gc.cochange_count DESC, f.path
        LIMIT 20
        "#,
    )?;
    let rows = stmt.query_map([file_id], |row| {
        Ok(item(
            row.get(0)?,
            row.get(1)?,
            Some(row.get(2)?),
            "git cochange neighbor".to_string(),
        ))
    })?;
    collect_items(rows)
}

fn entrypoints(conn: &Connection, symbol_id: i64, file_id: i64) -> Result<Vec<NeighborItem>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT
            COALESCE(s.qualname, f.path),
            e.kind,
            e.reason
        FROM entrypoints e
        JOIN files f ON f.id = e.file_id
        LEFT JOIN symbols s ON s.id = e.symbol_id
        WHERE e.symbol_id = ?1 OR e.file_id = ?2
        ORDER BY e.score DESC
        LIMIT 10
        "#,
    )?;
    let rows = stmt.query_map(params![symbol_id, file_id], |row| {
        Ok(item(
            row.get(0)?,
            row.get(1)?,
            Some(row.get(2)?),
            "indexed entrypoint".to_string(),
        ))
    })?;
    collect_items(rows)
}

fn item(label: String, kind: String, detail: Option<String>, why: String) -> NeighborItem {
    NeighborItem {
        label,
        kind,
        detail,
        why: vec![why],
    }
}

fn collect_items<T>(rows: rusqlite::MappedRows<'_, T>) -> Result<Vec<NeighborItem>>
where
    T: FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<NeighborItem>,
{
    let mut items = Vec::new();
    for row in rows {
        items.push(row?);
    }
    Ok(items)
}

fn print_items(label: &str, items: &[NeighborItem]) {
    if items.is_empty() {
        return;
    }
    println!();
    println!("{label}:");
    for item in items {
        if let Some(detail) = &item.detail {
            println!(
                "  - {} [{}] {} | {}",
                item.label,
                item.kind,
                detail,
                item.why.join("; ")
            );
        } else {
            println!("  - {} [{}] {}", item.label, item.kind, item.why.join("; "));
        }
    }
}

fn dedup_items(items: Vec<NeighborItem>) -> Vec<NeighborItem> {
    let mut seen = std::collections::HashSet::new();
    let mut deduped = Vec::new();
    for item in items {
        if seen.insert((item.label.clone(), item.kind.clone())) {
            deduped.push(item);
        }
    }
    deduped
}

fn pretty_evidence(raw: String) -> String {
    if let Ok(value) = serde_json::from_str::<Value>(&raw) {
        if let Some(callee) = value.get("callee").and_then(Value::as_str) {
            if let Some(line) = value.get("line").and_then(Value::as_i64) {
                return format!("callee={callee}, line={line}");
            }
        }
        if let Some(import_path) = value.get("import").and_then(Value::as_str) {
            return format!("import={import_path}");
        }
        if let Some(target) = value.get("target_hint").and_then(Value::as_str) {
            return format!("target_hint={target}");
        }
        if let Some(trait_name) = value.get("trait").and_then(Value::as_str) {
            return format!("trait={trait_name}");
        }
    }
    raw
}

fn label_from_row(label: Option<String>, evidence: Option<String>) -> String {
    label.unwrap_or_else(|| {
        evidence
            .and_then(|raw| {
                serde_json::from_str::<Value>(&raw).ok().and_then(|value| {
                    value
                        .get("callee")
                        .and_then(Value::as_str)
                        .or_else(|| value.get("import").and_then(Value::as_str))
                        .or_else(|| value.get("target_hint").and_then(Value::as_str))
                        .or_else(|| value.get("trait").and_then(Value::as_str))
                        .map(ToString::to_string)
                })
            })
            .unwrap_or_else(|| "unknown".to_string())
    })
}

fn print_compact(result: &CompactResult<NeighborResult>, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
        return Ok(());
    }

    if let Some(item) = result.items.first() {
        println!("{}", item.qualname);
        println!(
            "File: {} | confidence {:.2} | exhaustive {}",
            item.file_path, result.confidence, result.is_exhaustive
        );
        if let Some(hint) = &result.expansion_hint {
            println!("Hint: {hint}");
        }
        if let Some(parent) = &item.parent_symbol {
            println!("Parent: {parent}");
        }
        print_items("Methods", &item.methods);
        print_items("Impl/Trait Relations", &item.impl_relations);
        print_items("Likely Callees", &item.likely_callees);
        print_items("Related Tests", &item.related_tests);
        print_items("Cochanged Files", &item.cochanged_files);
        print_items("Entrypoints", &item.entrypoints);
    }
    Ok(())
}

fn print_verbose(result: &NeighborResult, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        println!("{}", result.qualname);
        println!("File: {}", result.file_path);
        println!("Why: {}", result.why.join("; "));
        if let Some(parent) = &result.parent_symbol {
            println!("Parent: {parent}");
        }
        print_items("Methods", &result.methods);
        print_items("Impl/Trait Relations", &result.impl_relations);
        print_items("Likely Callees", &result.likely_callees);
        print_items("Related Tests", &result.related_tests);
        print_items("Cochanged Files", &result.cochanged_files);
        print_items("Entrypoints", &result.entrypoints);
    }
    Ok(())
}

fn limit_items<T>(items: &mut Vec<T>, max_items: usize) -> bool {
    let truncated = items.len() > max_items;
    items.truncate(max_items);
    truncated
}
