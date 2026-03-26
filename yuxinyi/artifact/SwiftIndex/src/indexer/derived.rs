use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};

#[derive(Debug, Clone)]
struct FileInfo {
    id: i64,
    path: String,
    crate_name: Option<String>,
    module_path: Option<String>,
}

#[derive(Debug, Clone)]
struct SymbolInfo {
    id: i64,
    file_id: i64,
    name: String,
    qualname: String,
    kind: String,
    is_test: bool,
}

#[derive(Debug, Clone, Default)]
struct EdgeAccumulator {
    score: f64,
    reasons: Vec<String>,
}

type EntrypointRow = (String, Option<i64>, i64, f64, String);

/// Refresh derived relations that depend on the fully built SQLite graph.
pub fn refresh(conn: &mut Connection) -> Result<()> {
    let files = load_files(conn)?;
    let symbols = load_symbols(conn)?;
    let imports_by_file = load_imports(conn)?;
    let cochange = load_cochange(conn)?;
    let entrypoints = detect_entrypoints(conn, &files, &imports_by_file)?;

    let tx = conn
        .transaction()
        .context("Failed to start derived relations transaction")?;
    tx.execute("DELETE FROM symbol_test_edges", [])
        .context("Failed to clear symbol_test_edges")?;
    tx.execute("DELETE FROM file_test_edges", [])
        .context("Failed to clear file_test_edges")?;
    tx.execute("DELETE FROM entrypoints", [])
        .context("Failed to clear entrypoints")?;

    let symbol_scores = build_symbol_test_edges(&files, &symbols, &imports_by_file, &cochange);
    persist_symbol_test_edges(&tx, &symbol_scores)?;
    let file_scores = build_file_test_edges(
        &files,
        &symbols,
        &imports_by_file,
        &cochange,
        &symbol_scores,
    );
    persist_file_test_edges(&tx, &file_scores)?;
    persist_entrypoints(&tx, &entrypoints)?;

    tx.commit()
        .context("Failed to commit derived relations transaction")?;
    Ok(())
}

fn load_files(conn: &Connection) -> Result<Vec<FileInfo>> {
    let mut stmt =
        conn.prepare("SELECT id, path, crate_name, module_path FROM files ORDER BY path")?;
    let rows = stmt.query_map([], |row| {
        Ok(FileInfo {
            id: row.get(0)?,
            path: row.get(1)?,
            crate_name: row.get(2)?,
            module_path: row.get(3)?,
        })
    })?;

    let mut files = Vec::new();
    for row in rows {
        files.push(row?);
    }
    Ok(files)
}

fn load_symbols(conn: &Connection) -> Result<Vec<SymbolInfo>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT s.id, s.file_id, s.name, s.qualname, s.kind, COALESCE(s.is_test, 0)
        FROM symbols s
        ORDER BY s.file_id, s.start_line
        "#,
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(SymbolInfo {
            id: row.get(0)?,
            file_id: row.get(1)?,
            name: row.get(2)?,
            qualname: row.get(3)?,
            kind: row.get(4)?,
            is_test: row.get::<_, i64>(5)? != 0,
        })
    })?;

    let mut symbols = Vec::new();
    for row in rows {
        symbols.push(row?);
    }
    Ok(symbols)
}

fn load_imports(conn: &Connection) -> Result<HashMap<i64, Vec<String>>> {
    let mut stmt = conn
        .prepare("SELECT file_id, import_path FROM file_imports ORDER BY file_id, import_path")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;

    let mut imports = HashMap::new();
    for row in rows {
        let (file_id, import_path) = row?;
        imports
            .entry(file_id)
            .or_insert_with(Vec::new)
            .push(import_path);
    }
    Ok(imports)
}

fn load_cochange(conn: &Connection) -> Result<HashMap<(i64, i64), i64>> {
    let mut stmt = conn.prepare("SELECT file_id_a, file_id_b, cochange_count FROM git_cochange")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;

    let mut map = HashMap::new();
    for row in rows {
        let (left, right, count) = row?;
        map.insert((left, right), count);
    }
    Ok(map)
}

fn build_symbol_test_edges(
    files: &[FileInfo],
    symbols: &[SymbolInfo],
    imports_by_file: &HashMap<i64, Vec<String>>,
    cochange: &HashMap<(i64, i64), i64>,
) -> HashMap<(i64, i64), EdgeAccumulator> {
    let files_by_id = files
        .iter()
        .map(|file| (file.id, file))
        .collect::<HashMap<_, _>>();
    let symbols_by_file = group_symbols_by_file(symbols);
    let source_symbols = symbols
        .iter()
        .filter(|symbol| !is_test_symbol(symbol, &files_by_id))
        .collect::<Vec<_>>();
    let test_symbols = symbols
        .iter()
        .filter(|symbol| is_test_symbol(symbol, &files_by_id))
        .collect::<Vec<_>>();
    let names_by_key = source_symbols.iter().fold(
        HashMap::<String, Vec<&SymbolInfo>>::new(),
        |mut acc, symbol| {
            acc.entry(normalize_name(&symbol.name))
                .or_insert_with(Vec::new)
                .push(*symbol);
            acc
        },
    );

    let mut scores = HashMap::new();
    for test_symbol in test_symbols {
        let Some(test_file) = files_by_id.get(&test_symbol.file_id) else {
            continue;
        };
        let mut local = HashMap::<i64, EdgeAccumulator>::new();

        if let Some(same_file_symbols) = symbols_by_file.get(&test_symbol.file_id) {
            for symbol in same_file_symbols {
                if symbol.id == test_symbol.id || is_test_symbol(symbol, &files_by_id) {
                    continue;
                }
                add_reason(
                    &mut local,
                    symbol.id,
                    28.0,
                    "same file test module".to_string(),
                );
            }
        }

        let normalized_test_name = normalize_name(&test_symbol.name);
        if !normalized_test_name.is_empty() {
            for target in hinted_symbol_names(&normalized_test_name, &names_by_key) {
                add_reason(
                    &mut local,
                    target.id,
                    48.0,
                    format!("test name mentions {}", target.name),
                );
            }
        }

        if let Some(imports) = imports_by_file.get(&test_symbol.file_id) {
            for import_path in imports {
                let import_tail = import_path
                    .split("::")
                    .last()
                    .map(normalize_name)
                    .unwrap_or_default();
                if let Some(import_matches) = names_by_key.get(&import_tail) {
                    for symbol in import_matches {
                        add_reason(
                            &mut local,
                            symbol.id,
                            26.0,
                            format!("test imports {}", symbol.name),
                        );
                    }
                }
                for symbol in &source_symbols {
                    if import_path.ends_with(&symbol.qualname)
                        || import_path.ends_with(&symbol.name)
                        || import_path.contains(&symbol.name)
                    {
                        add_reason(
                            &mut local,
                            symbol.id,
                            18.0,
                            format!("import path references {}", symbol.name),
                        );
                    }
                }
            }
        }

        for symbol in &source_symbols {
            let Some(source_file) = files_by_id.get(&symbol.file_id) else {
                continue;
            };
            if test_file.crate_name == source_file.crate_name
                && same_module_family(
                    test_file.module_path.as_deref(),
                    source_file.module_path.as_deref(),
                )
            {
                add_reason(
                    &mut local,
                    symbol.id,
                    16.0,
                    "same crate test module".to_string(),
                );
            }

            if let Some(count) = lookup_cochange(cochange, test_symbol.file_id, symbol.file_id) {
                add_reason(
                    &mut local,
                    symbol.id,
                    (count.min(12) as f64) * 2.5,
                    format!("cochanged {} times", count),
                );
            }
        }

        for (symbol_id, edge) in local {
            if edge.score < 24.0 {
                continue;
            }
            scores.insert((symbol_id, test_symbol.id), edge);
        }
    }

    scores
}

fn persist_symbol_test_edges(
    tx: &rusqlite::Transaction<'_>,
    scores: &HashMap<(i64, i64), EdgeAccumulator>,
) -> Result<()> {
    let mut stmt = tx.prepare(
        r#"
        INSERT INTO symbol_test_edges (symbol_id, test_symbol_id, score, reason)
        VALUES (?1, ?2, ?3, ?4)
        "#,
    )?;
    for ((symbol_id, test_symbol_id), edge) in scores {
        stmt.execute(params![
            symbol_id,
            test_symbol_id,
            edge.score,
            join_reasons(&edge.reasons),
        ])
        .with_context(|| {
            format!(
                "Failed to insert symbol_test_edges row for symbol_id={} test_symbol_id={}",
                symbol_id, test_symbol_id
            )
        })?;
    }
    Ok(())
}

fn build_file_test_edges(
    files: &[FileInfo],
    symbols: &[SymbolInfo],
    imports_by_file: &HashMap<i64, Vec<String>>,
    cochange: &HashMap<(i64, i64), i64>,
    symbol_scores: &HashMap<(i64, i64), EdgeAccumulator>,
) -> HashMap<(i64, i64), EdgeAccumulator> {
    let symbols_by_id = symbols
        .iter()
        .map(|symbol| (symbol.id, symbol))
        .collect::<HashMap<_, _>>();
    let test_files = files
        .iter()
        .filter(|file| is_test_path(&file.path))
        .collect::<Vec<_>>();
    let mut scores = HashMap::<(i64, i64), EdgeAccumulator>::new();

    for ((symbol_id, test_symbol_id), edge) in symbol_scores {
        let Some(symbol) = symbols_by_id.get(symbol_id) else {
            continue;
        };
        let Some(test_symbol) = symbols_by_id.get(test_symbol_id) else {
            continue;
        };
        let pair = (symbol.file_id, test_symbol.file_id);
        let entry = scores.entry(pair).or_default();
        entry.score += edge.score;
        entry.reasons.extend(edge.reasons.clone());
    }

    for test_file in test_files {
        for source_file in files {
            if source_file.id == test_file.id {
                continue;
            }
            let entry = scores.entry((source_file.id, test_file.id)).or_default();
            if source_file.crate_name == test_file.crate_name
                && same_module_family(
                    source_file.module_path.as_deref(),
                    test_file.module_path.as_deref(),
                )
            {
                entry.score += 22.0;
                entry.reasons.push("same crate test module".to_string());
            }

            if let Some(imports) = imports_by_file.get(&test_file.id) {
                let file_stem = file_stem(&source_file.path);
                if imports.iter().any(|import_path| {
                    import_path.contains(&file_stem)
                        || source_file
                            .module_path
                            .as_ref()
                            .is_some_and(|module| import_path.contains(module))
                }) {
                    entry.score += 18.0;
                    entry
                        .reasons
                        .push(format!("test imports {}", source_file.path));
                }
            }

            if let Some(count) = lookup_cochange(cochange, source_file.id, test_file.id) {
                entry.score += (count.min(12) as f64) * 2.0;
                entry.reasons.push(format!("cochanged {} times", count));
            }
        }
    }

    scores.retain(|_, edge| edge.score >= 20.0);
    scores
}

fn persist_file_test_edges(
    tx: &rusqlite::Transaction<'_>,
    scores: &HashMap<(i64, i64), EdgeAccumulator>,
) -> Result<()> {
    let mut stmt = tx.prepare(
        r#"
        INSERT INTO file_test_edges (file_id, test_file_id, score, reason)
        VALUES (?1, ?2, ?3, ?4)
        "#,
    )?;
    for ((file_id, test_file_id), edge) in scores {
        stmt.execute(params![
            file_id,
            test_file_id,
            edge.score,
            join_reasons(&edge.reasons)
        ])
        .with_context(|| {
            format!(
                "Failed to insert file_test_edges row for file_id={} test_file_id={}",
                file_id, test_file_id
            )
        })?;
    }
    Ok(())
}

fn detect_entrypoints(
    conn: &Connection,
    files: &[FileInfo],
    imports_by_file: &HashMap<i64, Vec<String>>,
) -> Result<Vec<EntrypointRow>> {
    let files_by_id = files
        .iter()
        .map(|file| (file.id, file))
        .collect::<HashMap<_, _>>();
    let mut rows = Vec::new();
    let mut seen = HashSet::new();

    for file in files {
        if file.path.ends_with("/main.rs") || file.path == "src/main.rs" {
            push_entrypoint(
                &mut rows,
                &mut seen,
                "main",
                None,
                file.id,
                80.0,
                "binary crate root (main.rs)".to_string(),
            );
        }
        if file.path.ends_with("/lib.rs") || file.path == "src/lib.rs" {
            push_entrypoint(
                &mut rows,
                &mut seen,
                "main",
                None,
                file.id,
                42.0,
                "library crate root (lib.rs)".to_string(),
            );
        }
        if imports_by_file.get(&file.id).is_some_and(|imports| {
            imports
                .iter()
                .any(|import_path| import_path.contains("clap"))
        }) {
            push_entrypoint(
                &mut rows,
                &mut seen,
                "cli",
                None,
                file.id,
                36.0,
                "CLI surface via clap imports".to_string(),
            );
        }
    }

    let mut stmt = conn.prepare(
        r#"
        SELECT
            s.id,
            s.file_id,
            s.name,
            s.qualname,
            COALESCE(s.signature, ''),
            COALESCE(c.content, '')
        FROM symbols s
        LEFT JOIN chunks c ON c.symbol_id = s.id
        WHERE s.kind = 'fn'
        "#,
    )?;
    let symbol_rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, String>(5)?,
        ))
    })?;

    for row in symbol_rows {
        let (symbol_id, file_id, name, _qualname, signature, content) = row?;
        let Some(file) = files_by_id.get(&file_id) else {
            continue;
        };
        let joined = format!("{signature}\n{content}");
        let lower = joined.to_ascii_lowercase();

        if name == "main"
            || lower.contains("#[tokio::main]")
            || lower.contains("# [ tokio :: main ]")
        {
            let reason =
                if lower.contains("#[tokio::main]") || lower.contains("# [ tokio :: main ]") {
                    "async runtime entry via #[tokio::main]".to_string()
                } else {
                    "main function entrypoint".to_string()
                };
            push_entrypoint(
                &mut rows,
                &mut seen,
                "main",
                Some(symbol_id),
                file_id,
                96.0,
                reason,
            );
        }
        if lower.contains("tokio::spawn")
            || lower.contains("spawn(async")
            || lower.contains("spawn ( async")
        {
            push_entrypoint(
                &mut rows,
                &mut seen,
                "task",
                Some(symbol_id),
                file_id,
                54.0,
                "spawns async tasks with tokio::spawn".to_string(),
            );
        }
        if is_http_handler(&lower) {
            push_entrypoint(
                &mut rows,
                &mut seen,
                "http",
                Some(symbol_id),
                file_id,
                60.0,
                "HTTP handler signature heuristic".to_string(),
            );
        }
        if lower.contains("clap::")
            || lower.contains("parser::parse(")
            || lower.contains("command::parse(")
            || file.path.ends_with("/main.rs")
                && imports_by_file.get(&file_id).is_some_and(|imports| {
                    imports
                        .iter()
                        .any(|import_path| import_path.contains("clap"))
                })
        {
            push_entrypoint(
                &mut rows,
                &mut seen,
                "cli",
                Some(symbol_id),
                file_id,
                52.0,
                "CLI entry via clap parsing".to_string(),
            );
        }
    }

    Ok(rows)
}

fn persist_entrypoints(tx: &rusqlite::Transaction<'_>, rows: &[EntrypointRow]) -> Result<()> {
    let mut stmt = tx.prepare(
        r#"
        INSERT INTO entrypoints (kind, symbol_id, file_id, score, reason)
        VALUES (?1, ?2, ?3, ?4, ?5)
        "#,
    )?;
    for (kind, symbol_id, file_id, score, reason) in rows {
        stmt.execute(params![kind, symbol_id, file_id, score, reason])
            .with_context(|| {
                format!(
                    "Failed to insert entrypoint {} for file_id={} symbol_id={:?}",
                    kind, file_id, symbol_id
                )
            })?;
    }
    Ok(())
}

fn push_entrypoint(
    rows: &mut Vec<(String, Option<i64>, i64, f64, String)>,
    seen: &mut HashSet<(String, Option<i64>, i64)>,
    kind: &str,
    symbol_id: Option<i64>,
    file_id: i64,
    score: f64,
    reason: String,
) {
    let key = (kind.to_string(), symbol_id, file_id);
    if seen.insert(key.clone()) {
        rows.push((key.0, key.1, key.2, score, reason));
    }
}

fn group_symbols_by_file(symbols: &[SymbolInfo]) -> HashMap<i64, Vec<&SymbolInfo>> {
    let mut grouped = HashMap::<i64, Vec<&SymbolInfo>>::new();
    for symbol in symbols {
        grouped.entry(symbol.file_id).or_default().push(symbol);
    }
    grouped
}

fn add_reason(scores: &mut HashMap<i64, EdgeAccumulator>, key: i64, score: f64, reason: String) {
    let entry = scores.entry(key).or_default();
    entry.score += score;
    entry.reasons.push(reason);
}

fn normalize_name(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric() || *ch == '_')
        .collect::<String>()
        .to_ascii_lowercase()
}

fn hinted_symbol_names<'a>(
    normalized_test_name: &str,
    names_by_key: &'a HashMap<String, Vec<&'a SymbolInfo>>,
) -> Vec<&'a SymbolInfo> {
    let mut matches = Vec::new();
    for (name, symbols) in names_by_key {
        if name.len() < 3 {
            continue;
        }
        if normalized_test_name.contains(name) {
            matches.extend(symbols.iter().copied());
        }
    }
    matches
}

fn lookup_cochange(cochange: &HashMap<(i64, i64), i64>, left: i64, right: i64) -> Option<i64> {
    if left == right {
        return None;
    }
    let pair = if left < right {
        (left, right)
    } else {
        (right, left)
    };
    cochange.get(&pair).copied()
}

fn join_reasons(reasons: &[String]) -> String {
    let mut unique = Vec::new();
    for reason in reasons {
        if !unique.iter().any(|existing: &String| existing == reason) {
            unique.push(reason.clone());
        }
        if unique.len() >= 4 {
            break;
        }
    }
    unique.join("; ")
}

fn same_module_family(left: Option<&str>, right: Option<&str>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            normalize_module_family(left) == normalize_module_family(right)
        }
        _ => false,
    }
}

fn normalize_module_family(module: &str) -> String {
    module
        .split("::")
        .filter(|segment| {
            !matches!(
                *segment,
                "tests" | "test" | "integration" | "bench" | "benches" | "spec"
            )
        })
        .collect::<Vec<_>>()
        .join("::")
}

fn file_stem(path: &str) -> String {
    path.rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".rs")
        .to_string()
}

fn is_test_symbol(symbol: &SymbolInfo, files_by_id: &HashMap<i64, &FileInfo>) -> bool {
    symbol.is_test
        || symbol.name.starts_with("test_")
        || symbol.name.starts_with("should_")
        || symbol.name.starts_with("it_")
        || symbol.kind == "fn"
            && files_by_id
                .get(&symbol.file_id)
                .is_some_and(|file| is_test_path(&file.path))
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.contains("tests.rs")
        || lower.contains("/integration/")
        || lower.contains("test_")
        || lower.contains("_test")
}

fn is_http_handler(lower: &str) -> bool {
    lower.contains("#[get")
        || lower.contains("#[post")
        || lower.contains("#[put")
        || lower.contains("#[delete")
        || lower.contains("#[route")
        || lower.contains("httprequest")
        || lower.contains("httpresponse")
        || lower.contains("impl responder")
        || lower.contains("json<")
        || lower.contains("state<")
        || lower.contains("path<")
        || lower.contains("query<")
}
