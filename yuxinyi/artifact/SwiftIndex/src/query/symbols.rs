use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::{
    db::Database,
    model::{CompactResult, SnippetResult, SymbolSearchResult},
    query::{
        common::{self, QueryOptions},
        compactor, outline,
    },
    ranking::reranker::{rerank_candidates, Candidate, CandidateType, ScoredCandidate},
};

#[derive(Debug)]
struct SymbolRow {
    symbol_id: i64,
    file_id: i64,
    name: String,
    qualname: String,
    kind: String,
    path: String,
    module_path: Option<String>,
    crate_name: Option<String>,
    line_range: (usize, usize),
    signature: Option<String>,
    summary: String,
    git_hotness: i64,
    is_test: bool,
}

/// Run symbol search.
pub fn run(
    workspace_root: &Path,
    db_override: Option<&Path>,
    query: &str,
    json: bool,
    options: QueryOptions,
) -> Result<()> {
    let db = Database::open_for_workspace(workspace_root, db_override)?;
    if options.compact {
        let results = search_compact(db.conn(), query, options.expand)?;
        print_compact(&results, query, json)?;
    } else {
        let results = search(db.conn(), query, options.expand)?;
        print_verbose(&results, query, json)?;
    }
    Ok(())
}

/// Execute the symbol search query and return ranked results.
pub fn search(conn: &Connection, query: &str, expand: u8) -> Result<Vec<SymbolSearchResult>> {
    let results = search_scored(conn, query)?
        .into_iter()
        .map(|item| to_symbol_result(item, expand))
        .take(20)
        .collect();
    Ok(results)
}

pub fn search_compact(
    conn: &Connection,
    query: &str,
    expand: u8,
) -> Result<CompactResult<SymbolSearchResult>> {
    let scored = search_scored(conn, query)?;
    let exact_match = scored
        .first()
        .is_some_and(|item| item.candidate.exact_name_match || item.candidate.exact_qualname_match);
    let confidence = common::confidence_from_scored_candidates(&scored, exact_match);
    let budget = compactor::symbol_budget(expand);
    let mapped = scored
        .into_iter()
        .map(|item| {
            let score = item.score;
            let reasons = item.reasons.clone();
            ScoredCandidate {
                candidate: to_symbol_result(item, expand),
                score,
                reasons,
            }
        })
        .collect();
    let mut compact = compactor::compact_results(mapped, &budget, confidence);
    if !compact.is_exhaustive {
        compact.expansion_hint = Some(if compact.items.first().is_some() {
            "read top symbol snippet or expand to neighbors".to_string()
        } else {
            "expand symbol search".to_string()
        });
    }
    Ok(compact)
}

pub(crate) fn search_scored(conn: &Connection, query: &str) -> Result<Vec<ScoredCandidate>> {
    let mut candidates: HashMap<String, Candidate> = HashMap::new();
    let like = format!("%{query}%");

    let mut stmt = conn.prepare(
        r#"
        SELECT
            s.id,
            f.id,
            s.name,
            s.qualname,
            s.kind,
            f.path,
            f.module_path,
            f.crate_name,
            s.start_line,
            s.end_line,
            s.signature,
            COALESCE(s.summary, ''),
            COALESCE(gfs.commit_count, 0),
            COALESCE(s.is_test, 0)
        FROM symbols s
        JOIN files f ON f.id = s.file_id
        LEFT JOIN git_file_stats gfs ON gfs.file_id = f.id
        WHERE s.name = ?1
           OR s.qualname = ?1
           OR s.name LIKE ?2
           OR s.qualname LIKE ?2
           OR COALESCE(s.summary, '') LIKE ?2
        LIMIT 240
        "#,
    )?;
    let rows = stmt.query_map(params![query, like], |row| {
        Ok(symbol_candidate(SymbolRow {
            symbol_id: row.get(0)?,
            file_id: row.get(1)?,
            name: row.get(2)?,
            qualname: row.get(3)?,
            kind: row.get(4)?,
            path: row.get(5)?,
            module_path: row.get(6)?,
            crate_name: row.get(7)?,
            line_range: (row.get::<_, usize>(8)?, row.get::<_, usize>(9)?),
            signature: row.get(10)?,
            summary: row.get(11)?,
            git_hotness: row.get(12)?,
            is_test: row.get::<_, i64>(13)? != 0,
        }))
    })?;
    for row in rows {
        let mut candidate = row?;
        common::apply_textual_features(&mut candidate, query);
        if candidate
            .summary
            .to_ascii_lowercase()
            .contains(&query.to_ascii_lowercase())
        {
            candidate.reasons.push("summary match".to_string());
        }
        common::merge_candidate(&mut candidates, candidate);
    }

    if let Some(fts_query) = common::make_fts_query(query, false) {
        let mut stmt = conn.prepare(
            r#"
            SELECT
                s.id,
                f.id,
                s.name,
                s.qualname,
                s.kind,
                f.path,
                f.module_path,
                f.crate_name,
                s.start_line,
                s.end_line,
                s.signature,
                COALESCE(s.summary, ''),
                COALESCE(gfs.commit_count, 0),
                COALESCE(s.is_test, 0)
            FROM symbols_fts
            JOIN symbols s ON s.id = symbols_fts.rowid
            JOIN files f ON f.id = s.file_id
            LEFT JOIN git_file_stats gfs ON gfs.file_id = f.id
            WHERE symbols_fts MATCH ?1
            LIMIT 120
            "#,
        )?;
        let rows = stmt.query_map([fts_query], |row| {
            Ok(symbol_candidate(SymbolRow {
                symbol_id: row.get(0)?,
                file_id: row.get(1)?,
                name: row.get(2)?,
                qualname: row.get(3)?,
                kind: row.get(4)?,
                path: row.get(5)?,
                module_path: row.get(6)?,
                crate_name: row.get(7)?,
                line_range: (row.get::<_, usize>(8)?, row.get::<_, usize>(9)?),
                signature: row.get(10)?,
                summary: row.get(11)?,
                git_hotness: row.get(12)?,
                is_test: row.get::<_, i64>(13)? != 0,
            }))
        })?;
        for row in rows {
            let mut candidate = row?;
            candidate.fts_score = 20.0;
            candidate.reasons.push("symbol full-text match".to_string());
            common::apply_textual_features(&mut candidate, query);
            common::merge_candidate(&mut candidates, candidate);
        }
    }

    if let Some(fts_query) = common::make_fts_query(query, false) {
        let mut stmt = conn.prepare(
            r#"
            SELECT
                s.id,
                f.id,
                s.name,
                s.qualname,
                s.kind,
                f.path,
                f.module_path,
                f.crate_name,
                s.start_line,
                s.end_line,
                s.signature,
                COALESCE(s.summary, ''),
                COALESCE(gfs.commit_count, 0),
                COALESCE(s.is_test, 0)
            FROM chunks_fts
            JOIN chunks c ON c.id = chunks_fts.rowid
            JOIN files f ON f.id = c.file_id
            JOIN symbols s ON s.id = c.symbol_id
            LEFT JOIN git_file_stats gfs ON gfs.file_id = f.id
            WHERE chunks_fts MATCH ?1
            LIMIT 80
            "#,
        )?;
        let rows = stmt.query_map([fts_query], |row| {
            Ok(symbol_candidate(SymbolRow {
                symbol_id: row.get(0)?,
                file_id: row.get(1)?,
                name: row.get(2)?,
                qualname: row.get(3)?,
                kind: row.get(4)?,
                path: row.get(5)?,
                module_path: row.get(6)?,
                crate_name: row.get(7)?,
                line_range: (row.get::<_, usize>(8)?, row.get::<_, usize>(9)?),
                signature: row.get(10)?,
                summary: row.get(11)?,
                git_hotness: row.get(12)?,
                is_test: row.get::<_, i64>(13)? != 0,
            }))
        })?;
        for row in rows {
            let mut candidate = row?;
            candidate.chunk_fts_score = 16.0;
            candidate.reasons.push("chunk content match".to_string());
            common::apply_textual_features(&mut candidate, query);
            common::merge_candidate(&mut candidates, candidate);
        }
    }

    let mut collected = candidates.into_values().collect::<Vec<_>>();
    common::enrich_candidates(conn, query, &mut collected)?;
    let mut scored = rerank_candidates(query, collected);
    scored.truncate(20);
    Ok(scored)
}

/// Read the snippet for a symbol by its qualname.
pub fn read_symbol(
    workspace_root: &Path,
    db_override: Option<&Path>,
    qualname: &str,
    json: bool,
) -> Result<()> {
    let db = Database::open_for_workspace(workspace_root, db_override)?;
    let snippet = get_symbol_snippet(db.conn(), workspace_root, qualname)?;
    print_snippet(&snippet, json)?;
    Ok(())
}

/// Fetch a symbol-backed snippet for reuse by other query flows.
pub fn get_symbol_snippet(
    conn: &Connection,
    workspace_root: &Path,
    qualname: &str,
) -> Result<SnippetResult> {
    let row = conn
        .query_row(
            r#"
            SELECT f.path, s.start_line, s.end_line
            FROM symbols s
            JOIN files f ON f.id = s.file_id
            WHERE s.qualname = ?1
            "#,
            [qualname],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, usize>(1)?,
                    row.get::<_, usize>(2)?,
                ))
            },
        )
        .optional()?
        .with_context(|| format!("Symbol `{qualname}` not found"))?;

    let mut snippet =
        outline::read_snippet_from_workspace(workspace_root, Path::new(&row.0), row.1, row.2)?;
    snippet.why = vec!["exact qualified symbol lookup".to_string()];
    Ok(snippet)
}

fn symbol_candidate(row: SymbolRow) -> Candidate {
    Candidate {
        key: format!("symbol:{}", row.qualname),
        candidate_type: if row.is_test {
            CandidateType::Test
        } else {
            CandidateType::Symbol
        },
        name: row.name,
        qualname: Some(row.qualname),
        path: row.path,
        module_path: row.module_path,
        crate_name: row.crate_name,
        summary: row.summary,
        detail_kind: row.kind,
        signature: row.signature,
        line_range: Some(row.line_range),
        symbol_id: Some(row.symbol_id),
        file_id: Some(row.file_id),
        symbol_count: None,
        exact_name_match: false,
        exact_qualname_match: false,
        exact_path_match: false,
        prefix_match: false,
        substring_match: false,
        fuzzy_score: 0.0,
        fts_score: 0.0,
        chunk_fts_score: 0.0,
        same_module: false,
        edge_hits: 0,
        edge_reason: None,
        cochange_count: 0,
        git_hotness: row.git_hotness,
        test_mapping_count: 0,
        test_reason: None,
        entrypoint_hits: 0,
        entrypoint_reason: None,
        low_signal: false,
        is_test: row.is_test,
        reasons: Vec::new(),
    }
}

fn to_symbol_result(scored: ScoredCandidate, expand: u8) -> SymbolSearchResult {
    SymbolSearchResult {
        qualname: scored
            .candidate
            .qualname
            .unwrap_or_else(|| scored.candidate.name.clone()),
        kind: scored.candidate.detail_kind,
        file_path: scored.candidate.path,
        line_range: scored.candidate.line_range.unwrap_or((0, 0)),
        signature: if expand > 0 {
            scored.candidate.signature
        } else {
            None
        },
        summary: scored.candidate.summary,
        score: scored.score,
        why: scored.reasons,
    }
}

fn print_compact(
    results: &CompactResult<SymbolSearchResult>,
    query: &str,
    json: bool,
) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(results)?);
    } else if results.items.is_empty() {
        println!("No symbols found for `{query}`");
    } else {
        println!(
            "Symbols: {} | confidence {:.2} | exhaustive {}",
            results.items.len(),
            results.confidence,
            results.is_exhaustive
        );
        if let Some(hint) = &results.expansion_hint {
            println!("Hint: {hint}");
        }
        for (index, result) in results.items.iter().enumerate() {
            println!();
            println!("{}. {} ({})", index + 1, result.qualname, result.kind);
            println!(
                "   Location: {}:{}-{}",
                result.file_path, result.line_range.0, result.line_range.1
            );
            if let Some(signature) = &result.signature {
                println!("   Signature: {signature}");
            }
            println!("   Summary: {}", result.summary);
            println!("   Why: {}", result.why.join("; "));
        }
    }
    Ok(())
}

fn print_verbose(results: &[SymbolSearchResult], query: &str, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(results)?);
    } else if results.is_empty() {
        println!("No symbols found for `{query}`");
    } else {
        println!("Found {} symbols:", results.len());
        for (index, result) in results.iter().enumerate() {
            println!();
            println!("{}. {} ({})", index + 1, result.qualname, result.kind);
            println!(
                "   File: {}:{}-{}",
                result.file_path, result.line_range.0, result.line_range.1
            );
            if let Some(signature) = &result.signature {
                println!("   Signature: {signature}");
            }
            println!("   Summary: {}", result.summary);
            println!("   Score: {:.1}", result.score);
            println!("   Why: {}", result.why.join("; "));
        }
    }
    Ok(())
}

fn print_snippet(snippet: &SnippetResult, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(snippet)?);
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
