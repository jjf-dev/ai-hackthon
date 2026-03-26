use std::{collections::HashMap, path::Path};

use anyhow::Result;
use rusqlite::{params, Connection};

use crate::{
    db::Database,
    model::{CompactResult, SuggestResult, SuggestTarget},
    query::{
        common::{self, QueryOptions},
        compactor, files, symbols,
    },
    ranking::reranker::{CandidateType, ScoredCandidate},
};

/// Suggest likely edit targets for a free-form query.
pub fn run(
    workspace_root: &Path,
    db_override: Option<&Path>,
    query: &str,
    json: bool,
    options: QueryOptions,
) -> Result<()> {
    let db = Database::open_for_workspace(workspace_root, db_override)?;
    if options.compact {
        let results = suggest_compact(db.conn(), query, options.expand)?;
        print_compact(&results, query, json)?;
    } else {
        let results = suggest(db.conn(), query)?;
        print_verbose(&results, query, json)?;
    }
    Ok(())
}

/// Produce heuristic edit-target suggestions.
pub fn suggest(conn: &Connection, query: &str) -> Result<SuggestResult> {
    let symbol_candidates = symbols::search_scored(conn, query)?;
    let file_candidates = files::search_scored(conn, query)?;
    let tests = collect_related_tests(conn, &symbol_candidates, &file_candidates)?;

    let mut files = file_candidates
        .iter()
        .filter(|candidate| !candidate.candidate.is_test)
        .map(to_target)
        .collect::<Vec<_>>();
    dedup_targets(&mut files);
    files.retain(|target| !common::is_test_path(&target.path));
    files.truncate(4);

    let mut symbols = symbol_candidates
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.candidate.candidate_type,
                CandidateType::Symbol | CandidateType::Entrypoint
            ) && !candidate.candidate.is_test
        })
        .map(to_target)
        .collect::<Vec<_>>();
    dedup_targets(&mut symbols);
    symbols.truncate(4);

    let mut test_targets = symbol_candidates
        .iter()
        .filter(|candidate| candidate.candidate.is_test)
        .map(to_target)
        .collect::<Vec<_>>();
    test_targets.extend(tests.into_values());
    dedup_targets(&mut test_targets);
    test_targets.truncate(4);

    Ok(SuggestResult {
        files,
        symbols,
        tests: test_targets,
        why: vec![
            "suggestions combine symbol and file retrieval".to_string(),
            "ranking includes structural, git, entrypoint, and test-mapping signals".to_string(),
        ],
    })
}

pub fn suggest_compact(
    conn: &Connection,
    query: &str,
    expand: u8,
) -> Result<CompactResult<SuggestResult>> {
    let symbol_candidates = symbols::search_scored(conn, query)?;
    let file_candidates = files::search_scored(conn, query)?;
    let tests = collect_related_tests(conn, &symbol_candidates, &file_candidates)?;

    let symbol_confidence = common::confidence_from_scored_candidates(
        &symbol_candidates,
        symbol_candidates.first().is_some_and(|item| {
            item.candidate.exact_name_match || item.candidate.exact_qualname_match
        }),
    );
    let file_confidence = common::confidence_from_scored_candidates(
        &file_candidates,
        file_candidates
            .first()
            .is_some_and(|item| item.candidate.exact_name_match || item.candidate.exact_path_match),
    );

    let file_results = compactor::compact_results(
        file_candidates
            .iter()
            .filter(|candidate| !candidate.candidate.is_test)
            .cloned()
            .map(to_target_scored)
            .collect(),
        &compactor::suggest_budget(expand, 3, 5),
        file_confidence,
    );

    let symbol_results = compactor::compact_results(
        symbol_candidates
            .iter()
            .filter(|candidate| {
                matches!(
                    candidate.candidate.candidate_type,
                    CandidateType::Symbol | CandidateType::Entrypoint
                ) && !candidate.candidate.is_test
            })
            .cloned()
            .map(to_target_scored)
            .collect(),
        &compactor::suggest_budget(expand, 3, 5),
        symbol_confidence,
    );

    let mut test_targets = symbol_candidates
        .iter()
        .filter(|candidate| candidate.candidate.is_test)
        .map(to_target)
        .collect::<Vec<_>>();
    test_targets.extend(tests.into_values());
    dedup_targets(&mut test_targets);
    test_targets.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let test_results = compactor::compact_results(
        test_targets
            .into_iter()
            .map(|target| ScoredCandidate {
                score: target.score,
                reasons: target.why.clone(),
                candidate: target,
            })
            .collect(),
        &compactor::suggest_budget(expand, 2, 4),
        symbol_confidence.max(file_confidence) * 0.9,
    );

    let is_exhaustive =
        file_results.is_exhaustive && symbol_results.is_exhaustive && test_results.is_exhaustive;
    let expansion_hint = if is_exhaustive {
        None
    } else if !test_results.items.is_empty() {
        Some("expand to related tests".to_string())
    } else {
        Some("expand to neighbors or file search".to_string())
    };

    Ok(CompactResult {
        items: vec![SuggestResult {
            files: file_results.items,
            symbols: symbol_results.items,
            tests: test_results.items,
            why: vec![
                "suggestions combine symbol and file retrieval".to_string(),
                "ranking includes structural, git, entrypoint, and test-mapping signals"
                    .to_string(),
            ],
        }],
        confidence: symbol_confidence.max(file_confidence),
        is_exhaustive,
        expansion_hint,
    })
}

fn collect_related_tests(
    conn: &Connection,
    symbol_candidates: &[ScoredCandidate],
    file_candidates: &[ScoredCandidate],
) -> Result<HashMap<String, SuggestTarget>> {
    let mut targets: HashMap<String, SuggestTarget> = HashMap::new();
    let mut symbol_stmt = conn.prepare(
        r#"
        SELECT
            f.path,
            t.qualname,
            ste.score,
            ste.reason,
            COALESCE(t.summary, '')
        FROM symbol_test_edges ste
        JOIN symbols t ON t.id = ste.test_symbol_id
        JOIN files f ON f.id = t.file_id
        WHERE ste.symbol_id = ?1
        ORDER BY ste.score DESC
        LIMIT 5
        "#,
    )?;
    let mut file_stmt = conn.prepare(
        r#"
        SELECT
            f.path,
            NULL,
            fte.score,
            fte.reason,
            COALESCE(f.summary, '')
        FROM file_test_edges fte
        JOIN files f ON f.id = fte.test_file_id
        WHERE fte.file_id = ?1
        ORDER BY fte.score DESC
        LIMIT 5
        "#,
    )?;

    for candidate in symbol_candidates.iter().take(3) {
        if let Some(symbol_id) = candidate.candidate.symbol_id {
            let rows = symbol_stmt.query_map([symbol_id], |row| {
                Ok(SuggestTarget {
                    path: row.get(0)?,
                    qualname: row.get(1)?,
                    summary: row.get(4)?,
                    score: row.get::<_, f64>(2)? + candidate.score * 0.15,
                    why: vec![
                        row.get::<_, String>(3)?,
                        format!(
                            "suggested from {}",
                            candidate
                                .candidate
                                .qualname
                                .clone()
                                .unwrap_or_else(|| candidate.candidate.name.clone())
                        ),
                    ],
                })
            })?;
            for row in rows {
                let target = row?;
                targets
                    .entry(test_key(&target))
                    .and_modify(|existing| {
                        existing.score = existing.score.max(target.score);
                        existing.why.extend(target.why.clone());
                        if existing.summary.is_none() {
                            existing.summary = target.summary.clone();
                        }
                    })
                    .or_insert(target);
            }
        }
    }

    for candidate in file_candidates.iter().take(3) {
        if let Some(file_id) = candidate.candidate.file_id {
            let rows = file_stmt.query_map(params![file_id], |row| {
                Ok(SuggestTarget {
                    path: row.get(0)?,
                    qualname: row.get(1)?,
                    summary: row.get(4)?,
                    score: row.get::<_, f64>(2)? + candidate.score * 0.1,
                    why: vec![
                        row.get::<_, String>(3)?,
                        format!("suggested from {}", candidate.candidate.path),
                    ],
                })
            })?;
            for row in rows {
                let target = row?;
                targets
                    .entry(test_key(&target))
                    .and_modify(|existing| {
                        existing.score = existing.score.max(target.score);
                        existing.why.extend(target.why.clone());
                        if existing.summary.is_none() {
                            existing.summary = target.summary.clone();
                        }
                    })
                    .or_insert(target);
            }
        }
    }

    for target in targets.values_mut() {
        dedup_why(&mut target.why);
    }
    Ok(targets)
}

fn to_target(candidate: &ScoredCandidate) -> SuggestTarget {
    SuggestTarget {
        path: candidate.candidate.path.clone(),
        qualname: candidate.candidate.qualname.clone(),
        summary: if candidate.candidate.summary.is_empty() {
            None
        } else {
            Some(candidate.candidate.summary.clone())
        },
        score: candidate.score,
        why: candidate.reasons.clone(),
    }
}

fn to_target_scored(candidate: ScoredCandidate) -> ScoredCandidate<SuggestTarget> {
    let score = candidate.score;
    let reasons = candidate.reasons.clone();
    ScoredCandidate {
        candidate: to_target(&candidate),
        score,
        reasons,
    }
}

fn dedup_targets(targets: &mut Vec<SuggestTarget>) {
    let mut seen = HashMap::<String, usize>::new();
    let mut deduped: Vec<SuggestTarget> = Vec::new();
    targets.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for target in targets.drain(..) {
        let key = test_key(&target);
        if let Some(index) = seen.get(&key).copied() {
            deduped[index].score = deduped[index].score.max(target.score);
            deduped[index].why.extend(target.why);
            if deduped[index].summary.is_none() {
                deduped[index].summary = target.summary;
            }
            dedup_why(&mut deduped[index].why);
            continue;
        }
        seen.insert(key, deduped.len());
        deduped.push(target);
    }
    *targets = deduped;
}

fn dedup_why(reasons: &mut Vec<String>) {
    let mut unique = Vec::new();
    for reason in reasons.drain(..) {
        if !unique.iter().any(|existing: &String| existing == &reason) {
            unique.push(reason);
        }
    }
    *reasons = unique;
}

fn test_key(target: &SuggestTarget) -> String {
    if let Some(qualname) = &target.qualname {
        format!("symbol:{qualname}")
    } else {
        format!("file:{}", target.path)
    }
}

fn print_targets(label: &str, targets: &[SuggestTarget]) {
    if targets.is_empty() {
        return;
    }
    println!();
    println!("{label}:");
    for (index, target) in targets.iter().enumerate() {
        println!("  {}. {}", index + 1, target.path);
        if let Some(qualname) = &target.qualname {
            println!("     Symbol: {qualname}");
        }
        if let Some(summary) = &target.summary {
            println!("     Summary: {summary}");
        }
        println!("     Score: {:.1}", target.score);
        println!("     Why: {}", target.why.join("; "));
    }
}

fn print_compact(result: &CompactResult<SuggestResult>, query: &str, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(result)?);
        return Ok(());
    }

    if let Some(payload) = result.items.first() {
        if payload.files.is_empty() && payload.symbols.is_empty() && payload.tests.is_empty() {
            println!("No edit targets suggested for `{query}`");
            return Ok(());
        }
        println!(
            "Suggested edit targets | confidence {:.2} | exhaustive {}",
            result.confidence, result.is_exhaustive
        );
        if let Some(hint) = &result.expansion_hint {
            println!("Hint: {hint}");
        }
        print_targets("Files", &payload.files);
        print_targets("Symbols", &payload.symbols);
        print_targets("Tests", &payload.tests);
        println!();
        println!("Why: {}", payload.why.join("; "));
    }
    Ok(())
}

fn print_verbose(results: &SuggestResult, query: &str, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(results)?);
    } else if results.files.is_empty() && results.symbols.is_empty() && results.tests.is_empty() {
        println!("No edit targets suggested for `{query}`");
    } else {
        println!("Suggested edit targets:");
        print_targets("Files", &results.files);
        print_targets("Symbols", &results.symbols);
        print_targets("Tests", &results.tests);
        println!();
        println!("Why: {}", results.why.join("; "));
    }
    Ok(())
}
