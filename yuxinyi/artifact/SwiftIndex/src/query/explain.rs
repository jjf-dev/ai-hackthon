use std::path::Path;

use anyhow::Result;
use rusqlite::Connection;

use crate::{
    db::Database,
    model::{CompactResult, ExplainResult, NeighborItem},
    query::{common::QueryOptions, files, neighbors, outline, symbols},
};

/// Compose a one-shot explanation payload for a natural language query.
pub fn run(
    workspace_root: &Path,
    db_override: Option<&Path>,
    query: &str,
    json: bool,
    options: QueryOptions,
) -> Result<()> {
    let db = Database::open_for_workspace(workspace_root, db_override)?;
    if options.compact {
        let result = explain_compact(db.conn(), workspace_root, query, options.expand)?;
        print_compact(&result, json)?;
    } else {
        let result = explain(db.conn(), workspace_root, query)?;
        print_verbose(&result, json)?;
    }
    Ok(())
}

pub fn explain(conn: &Connection, workspace_root: &Path, query: &str) -> Result<ExplainResult> {
    let top_symbols = symbols::search(conn, query, 2)?;
    let top_files = files::search(conn, query)?;

    let outline_summary = if let Some(file) = top_files.first() {
        let outline_result = outline::get_outline(conn, workspace_root, Path::new(&file.path))?;
        format!(
            "{} | top-level: {} | tests: {}",
            outline_result.summary,
            outline_result
                .top_level_symbols
                .iter()
                .take(4)
                .cloned()
                .collect::<Vec<_>>()
                .join(", "),
            outline_result.test_functions.len()
        )
    } else {
        "No matching file outline available".to_string()
    };

    let neighbors = if let Some(symbol) = top_symbols.first() {
        let neighbor_result = neighbors::get_neighbors(conn, &symbol.qualname)?;
        flatten_neighbors(&neighbor_result)
    } else {
        Vec::new()
    };

    let mut next_steps = Vec::new();
    if let Some(symbol) = top_symbols.first() {
        next_steps.push(format!("read {}", symbol.qualname));
    }
    if let Some(file) = top_files.first() {
        next_steps.push(format!("outline {}", file.path));
    }
    if neighbors.iter().any(|item| item.kind == "test") {
        next_steps.push("check related tests".to_string());
    }
    if next_steps.is_empty() {
        next_steps.push("refine the query with a symbol or module name".to_string());
    }

    Ok(ExplainResult {
        query: query.to_string(),
        top_symbols: top_symbols.into_iter().take(5).collect(),
        top_files: top_files.into_iter().take(5).collect(),
        outline_summary,
        neighbors,
        next_steps,
        why: vec![
            "combined symbol, file, outline, and neighbor retrieval".to_string(),
            "reranked with lexical, structural, git, test, and entrypoint signals".to_string(),
        ],
    })
}

pub fn explain_compact(
    conn: &Connection,
    workspace_root: &Path,
    query: &str,
    expand: u8,
) -> Result<CompactResult<ExplainResult>> {
    let symbol_result = symbols::search_compact(conn, query, expand)?;
    let file_result = files::search_compact(conn, query, expand)?;

    let top_symbol_limit = match expand {
        0 => 2,
        1 => 3,
        _ => 4,
    };
    let top_file_limit = match expand {
        0 => 2,
        1 => 3,
        _ => 4,
    };

    let mut top_symbols = symbol_result.items;
    let symbol_truncated = top_symbols.len() > top_symbol_limit;
    top_symbols.truncate(top_symbol_limit);

    let mut top_files = file_result.items;
    let file_truncated = top_files.len() > top_file_limit;
    top_files.truncate(top_file_limit);

    let outline_summary = if let Some(file) = top_files.first() {
        let outline_result =
            outline::get_outline_compact(conn, workspace_root, Path::new(&file.path), expand)?;
        if let Some(item) = outline_result.items.first() {
            format!(
                "{} | top-level: {} | tests: {}",
                item.summary,
                item.top_level_symbols.join(", "),
                item.test_functions.len()
            )
        } else {
            "No matching file outline available".to_string()
        }
    } else {
        "No matching file outline available".to_string()
    };

    let mut neighbors = if let Some(symbol) = top_symbols.first() {
        let neighbor_result = neighbors::get_neighbors_compact(conn, &symbol.qualname, expand)?;
        neighbor_result
            .items
            .first()
            .map(flatten_neighbors)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let neighbor_limit = match expand {
        0 => 4,
        1 => 6,
        _ => 8,
    };
    let neighbor_truncated = neighbors.len() > neighbor_limit;
    neighbors.truncate(neighbor_limit);

    let mut next_steps = Vec::new();
    if let Some(symbol) = top_symbols.first() {
        next_steps.push(format!("read top symbol snippet: {}", symbol.qualname));
    }
    if let Some(file) = top_files.first() {
        next_steps.push(format!("outline {}", file.path));
    }
    if neighbors
        .iter()
        .any(|item| item.kind == "test" || item.kind == "test_file")
    {
        next_steps.push("expand to related tests".to_string());
    } else if !top_symbols.is_empty() {
        next_steps.push("expand to neighbors".to_string());
    }
    if next_steps.is_empty() {
        next_steps.push("refine the query with a symbol or module name".to_string());
    }
    let next_step_limit = match expand {
        0 => 3,
        1 => 4,
        _ => 5,
    };
    let next_steps_truncated = next_steps.len() > next_step_limit;
    next_steps.truncate(next_step_limit);

    let is_exhaustive = symbol_result.is_exhaustive
        && file_result.is_exhaustive
        && !symbol_truncated
        && !file_truncated
        && !neighbor_truncated
        && !next_steps_truncated;
    let expansion_hint = if is_exhaustive {
        None
    } else if neighbors
        .iter()
        .any(|item| item.kind == "test" || item.kind == "test_file")
    {
        Some("expand to related tests".to_string())
    } else {
        Some("read top symbol snippet or expand to neighbors".to_string())
    };

    Ok(CompactResult {
        items: vec![ExplainResult {
            query: query.to_string(),
            top_symbols,
            top_files,
            outline_summary,
            neighbors,
            next_steps,
            why: vec![
                "combined symbol, file, outline, and neighbor retrieval".to_string(),
                "reranked with lexical, structural, git, test, and entrypoint signals".to_string(),
            ],
        }],
        confidence: symbol_result.confidence.max(file_result.confidence),
        is_exhaustive,
        expansion_hint,
    })
}

fn flatten_neighbors(result: &crate::model::NeighborResult) -> Vec<NeighborItem> {
    let mut items = Vec::new();
    items.extend(result.likely_callees.iter().cloned());
    items.extend(result.related_tests.iter().cloned());
    items.extend(result.entrypoints.iter().cloned());
    items.extend(result.impl_relations.iter().cloned());
    items.truncate(8);
    items
}

fn print_compact(result: &CompactResult<ExplainResult>, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
        return Ok(());
    }

    if let Some(item) = result.items.first() {
        println!(
            "Query: {} | confidence {:.2} | exhaustive {}",
            item.query, result.confidence, result.is_exhaustive
        );
        if let Some(hint) = &result.expansion_hint {
            println!("Hint: {hint}");
        }
        println!("Why: {}", item.why.join("; "));
        if let Some(symbol) = item.top_symbols.first() {
            println!();
            println!("Top symbol: {}", symbol.qualname);
            println!("  Why: {}", symbol.why.join("; "));
        }
        if let Some(file) = item.top_files.first() {
            println!();
            println!("Top file: {}", file.path);
            println!("  Why: {}", file.why.join("; "));
        }
        println!();
        println!("Outline summary: {}", item.outline_summary);
        if !item.neighbors.is_empty() {
            println!();
            println!("Neighbors:");
            for neighbor in &item.neighbors {
                println!(
                    "  - {} [{}] {}",
                    neighbor.label,
                    neighbor.kind,
                    neighbor.why.join("; ")
                );
            }
        }
        if !item.next_steps.is_empty() {
            println!();
            println!("Next steps:");
            for step in &item.next_steps {
                println!("  - {step}");
            }
        }
    }
    Ok(())
}

fn print_verbose(result: &ExplainResult, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
    } else {
        println!("Query: {}", result.query);
        println!("Why: {}", result.why.join("; "));
        if let Some(symbol) = result.top_symbols.first() {
            println!();
            println!("Top symbol: {}", symbol.qualname);
            println!("  Score: {:.1}", symbol.score);
            println!("  Why: {}", symbol.why.join("; "));
        }
        if let Some(file) = result.top_files.first() {
            println!();
            println!("Top file: {}", file.path);
            println!("  Score: {:.1}", file.score);
            println!("  Why: {}", file.why.join("; "));
        }
        println!();
        println!("Outline summary: {}", result.outline_summary);
        if !result.neighbors.is_empty() {
            println!();
            println!("Neighbors:");
            for item in &result.neighbors {
                println!("  - {} [{}] {}", item.label, item.kind, item.why.join("; "));
            }
        }
        if !result.next_steps.is_empty() {
            println!();
            println!("Next steps:");
            for step in &result.next_steps {
                println!("  - {step}");
            }
        }
    }
    Ok(())
}
