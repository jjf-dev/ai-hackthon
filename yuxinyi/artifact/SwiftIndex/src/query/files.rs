use std::{collections::HashMap, path::Path};

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::{
    db::Database,
    model::{CompactResult, FileSearchResult},
    query::{
        common::{self, QueryOptions},
        compactor,
    },
    ranking::reranker::{rerank_candidates, Candidate, CandidateType, ScoredCandidate},
};

/// Run file search.
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
        let results = search(db.conn(), query)?;
        print_verbose(&results, query, json)?;
    }
    Ok(())
}

/// Execute the file search query and return ranked results.
pub fn search(conn: &Connection, query: &str) -> Result<Vec<FileSearchResult>> {
    let results = search_scored(conn, query)?
        .into_iter()
        .map(to_file_result)
        .take(20)
        .collect();
    Ok(results)
}

pub fn search_compact(
    conn: &Connection,
    query: &str,
    expand: u8,
) -> Result<CompactResult<FileSearchResult>> {
    let scored = search_scored(conn, query)?;
    let exact_match = scored
        .first()
        .is_some_and(|item| item.candidate.exact_path_match || item.candidate.exact_name_match);
    let confidence = common::confidence_from_scored_candidates(&scored, exact_match);
    let budget = compactor::file_budget(expand);
    let mapped = scored
        .into_iter()
        .map(|item| {
            let score = item.score;
            let reasons = item.reasons.clone();
            ScoredCandidate {
                candidate: to_file_result(item),
                score,
                reasons,
            }
        })
        .collect();
    let mut compact = compactor::compact_results(mapped, &budget, confidence);
    if !compact.is_exhaustive {
        compact.expansion_hint = Some(if compact.items.first().is_some() {
            "read top file outline or expand file search".to_string()
        } else {
            "expand file search".to_string()
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
            f.id,
            f.path,
            f.crate_name,
            f.module_path,
            COALESCE(f.summary, ''),
            f.symbol_count,
            COALESCE(gfs.commit_count, 0)
        FROM files f
        LEFT JOIN git_file_stats gfs ON gfs.file_id = f.id
        WHERE f.path = ?1
           OR f.path LIKE ?2
           OR COALESCE(f.summary, '') LIKE ?2
        LIMIT 240
        "#,
    )?;
    let rows = stmt.query_map(params![query, like], |row| {
        Ok(file_candidate(
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
            row.get::<_, usize>(5)?,
            row.get(6)?,
        ))
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

    if let Some(fts_query) = common::make_fts_query(query, true) {
        let mut stmt = conn.prepare(
            r#"
            SELECT
                f.id,
                f.path,
                f.crate_name,
                f.module_path,
                COALESCE(f.summary, ''),
                f.symbol_count,
                COALESCE(gfs.commit_count, 0)
            FROM files_fts
            JOIN files f ON f.id = files_fts.rowid
            LEFT JOIN git_file_stats gfs ON gfs.file_id = f.id
            WHERE files_fts MATCH ?1
            LIMIT 120
            "#,
        )?;
        let rows = stmt.query_map([fts_query], |row| {
            Ok(file_candidate(
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get::<_, usize>(5)?,
                row.get(6)?,
            ))
        })?;
        for row in rows {
            let mut candidate = row?;
            candidate.fts_score = 18.0;
            candidate.reasons.push("file full-text match".to_string());
            common::apply_textual_features(&mut candidate, query);
            common::merge_candidate(&mut candidates, candidate);
        }
    }

    if let Some(fts_query) = common::make_fts_query(query, false) {
        let mut stmt = conn.prepare(
            r#"
            SELECT
                f.id,
                f.path,
                f.crate_name,
                f.module_path,
                COALESCE(f.summary, ''),
                f.symbol_count,
                COALESCE(gfs.commit_count, 0)
            FROM chunks_fts
            JOIN chunks c ON c.id = chunks_fts.rowid
            JOIN files f ON f.id = c.file_id
            LEFT JOIN git_file_stats gfs ON gfs.file_id = f.id
            WHERE chunks_fts MATCH ?1
            LIMIT 80
            "#,
        )?;
        let rows = stmt.query_map([fts_query], |row| {
            Ok(file_candidate(
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get::<_, usize>(5)?,
                row.get(6)?,
            ))
        })?;
        for row in rows {
            let mut candidate = row?;
            candidate.chunk_fts_score = 14.0;
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

fn file_candidate(
    file_id: i64,
    path: String,
    crate_name: Option<String>,
    module_path: Option<String>,
    summary: String,
    symbol_count: usize,
    git_hotness: i64,
) -> Candidate {
    let name = path.rsplit('/').next().unwrap_or(&path).to_string();
    Candidate {
        key: format!("file:{path}"),
        candidate_type: if common::is_test_path(&path) {
            CandidateType::Test
        } else {
            CandidateType::File
        },
        name,
        qualname: None,
        path,
        module_path,
        crate_name,
        summary,
        detail_kind: "file".to_string(),
        signature: None,
        line_range: None,
        symbol_id: None,
        file_id: Some(file_id),
        symbol_count: Some(symbol_count),
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
        git_hotness,
        test_mapping_count: 0,
        test_reason: None,
        entrypoint_hits: 0,
        entrypoint_reason: None,
        low_signal: false,
        is_test: false,
        reasons: Vec::new(),
    }
}

fn to_file_result(scored: ScoredCandidate) -> FileSearchResult {
    FileSearchResult {
        path: scored.candidate.path,
        crate_name: scored.candidate.crate_name,
        module_path: scored.candidate.module_path,
        summary: scored.candidate.summary,
        symbol_count: scored.candidate.symbol_count.unwrap_or_default(),
        score: scored.score,
        why: scored.reasons,
    }
}

fn print_compact(results: &CompactResult<FileSearchResult>, query: &str, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(results)?);
    } else if results.items.is_empty() {
        println!("No files found for `{query}`");
    } else {
        println!(
            "Files: {} | confidence {:.2} | exhaustive {}",
            results.items.len(),
            results.confidence,
            results.is_exhaustive
        );
        if let Some(hint) = &results.expansion_hint {
            println!("Hint: {hint}");
        }
        for (index, result) in results.items.iter().enumerate() {
            println!();
            println!("{}. {}", index + 1, result.path);
            println!("   Summary: {}", result.summary);
            println!("   Why: {}", result.why.join("; "));
        }
    }
    Ok(())
}

fn print_verbose(results: &[FileSearchResult], query: &str, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(results)?);
    } else if results.is_empty() {
        println!("No files found for `{query}`");
    } else {
        println!("Found {} files:", results.len());
        for (index, result) in results.iter().enumerate() {
            println!();
            println!("{}. {}", index + 1, result.path);
            if let Some(crate_name) = &result.crate_name {
                println!("   Crate: {crate_name}");
            }
            if let Some(module_path) = &result.module_path {
                println!("   Module: {module_path}");
            }
            println!("   Symbols: {}", result.symbol_count);
            println!("   Summary: {}", result.summary);
            println!("   Score: {:.1}", result.score);
            println!("   Why: {}", result.why.join("; "));
        }
    }
    Ok(())
}
