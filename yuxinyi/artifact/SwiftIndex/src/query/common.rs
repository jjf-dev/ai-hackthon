use std::collections::{HashMap, HashSet};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::ranking::reranker::{Candidate, ScoredCandidate};

#[derive(Debug, Clone, Copy)]
pub struct QueryOptions {
    pub compact: bool,
    pub expand: u8,
}

impl QueryOptions {
    pub fn new(compact: bool, expand: u8) -> Self {
        Self { compact, expand }
    }
}

pub(crate) fn make_fts_query(query: &str, allow_slash: bool) -> Option<String> {
    let terms = query
        .split(|ch: char| !(ch.is_alphanumeric() || ch == '_' || (allow_slash && ch == '/')))
        .filter(|term| !term.is_empty())
        .map(|term| format!("{term}*"))
        .collect::<Vec<_>>();
    if terms.is_empty() {
        None
    } else {
        Some(terms.join(" OR "))
    }
}

pub(crate) fn apply_textual_features(candidate: &mut Candidate, query: &str) {
    let normalized_query = normalize_text(query);
    let normalized_name = normalize_text(&candidate.name);
    let normalized_path = normalize_text(&candidate.path);
    let normalized_qualname = candidate
        .qualname
        .as_ref()
        .map(|value| normalize_text(value))
        .unwrap_or_default();
    let basename = candidate
        .path
        .rsplit('/')
        .next()
        .map(normalize_text)
        .unwrap_or_default();

    candidate.exact_name_match = !normalized_query.is_empty()
        && (normalized_query == normalized_name || normalized_query == basename);
    candidate.exact_qualname_match =
        !normalized_query.is_empty() && normalized_query == normalized_qualname;
    candidate.exact_path_match =
        !normalized_query.is_empty() && normalized_query == normalized_path;
    candidate.prefix_match = !normalized_query.is_empty()
        && (normalized_name.starts_with(&normalized_query)
            || normalized_qualname.starts_with(&normalized_query)
            || normalized_path.starts_with(&normalized_query)
            || basename.starts_with(&normalized_query));
    candidate.substring_match = !normalized_query.is_empty()
        && (normalized_name.contains(&normalized_query)
            || normalized_qualname.contains(&normalized_query)
            || normalized_path.contains(&normalized_query)
            || basename.contains(&normalized_query));
    candidate.fuzzy_score = fuzzy_score(
        &normalized_query,
        &[
            normalized_name.as_str(),
            normalized_qualname.as_str(),
            normalized_path.as_str(),
            basename.as_str(),
        ],
    );
    candidate.low_signal = is_low_signal_path(&candidate.path);
    candidate.is_test = candidate.is_test || is_test_path(&candidate.path);
}

pub(crate) fn merge_candidate(candidates: &mut HashMap<String, Candidate>, candidate: Candidate) {
    candidates
        .entry(candidate.key.clone())
        .and_modify(|existing| {
            existing.exact_name_match |= candidate.exact_name_match;
            existing.exact_qualname_match |= candidate.exact_qualname_match;
            existing.exact_path_match |= candidate.exact_path_match;
            existing.prefix_match |= candidate.prefix_match;
            existing.substring_match |= candidate.substring_match;
            existing.fuzzy_score = existing.fuzzy_score.max(candidate.fuzzy_score);
            existing.fts_score = existing.fts_score.max(candidate.fts_score);
            existing.chunk_fts_score = existing.chunk_fts_score.max(candidate.chunk_fts_score);
            existing.git_hotness = existing.git_hotness.max(candidate.git_hotness);
            existing.low_signal |= candidate.low_signal;
            existing.is_test |= candidate.is_test;
            if existing.summary.is_empty() {
                existing.summary = candidate.summary.clone();
            }
            if existing.signature.is_none() {
                existing.signature = candidate.signature.clone();
            }
            if existing.line_range.is_none() {
                existing.line_range = candidate.line_range;
            }
            if existing.symbol_count.is_none() {
                existing.symbol_count = candidate.symbol_count;
            }
            for reason in &candidate.reasons {
                if !existing.reasons.iter().any(|current| current == reason) {
                    existing.reasons.push(reason.clone());
                }
            }
        })
        .or_insert(candidate);
}

pub(crate) fn enrich_candidates(
    conn: &Connection,
    query: &str,
    candidates: &mut [Candidate],
) -> Result<()> {
    let anchor_modules = collect_anchor_modules(query, candidates);
    let anchor_files = collect_anchor_ids(candidates, |candidate| candidate.file_id);
    let anchor_symbols = collect_anchor_ids(candidates, |candidate| candidate.symbol_id);

    let mut edge_degree_stmt = conn.prepare(
        "SELECT COUNT(*) FROM symbol_edges WHERE from_symbol_id = ?1 OR to_symbol_id = ?1",
    )?;
    let mut edge_reason_stmt = conn.prepare(
        r#"
        SELECT
            edge.edge_type,
            source.qualname,
            target.qualname,
            edge.evidence
        FROM symbol_edges edge
        LEFT JOIN symbols source ON source.id = edge.from_symbol_id
        LEFT JOIN symbols target ON target.id = edge.to_symbol_id
        WHERE edge.from_symbol_id = ?1 OR edge.to_symbol_id = ?1
        LIMIT 8
        "#,
    )?;
    let mut cochange_stmt = conn.prepare(
        r#"
        SELECT cochange_count
        FROM git_cochange
        WHERE (file_id_a = ?1 AND file_id_b = ?2)
           OR (file_id_a = ?2 AND file_id_b = ?1)
        "#,
    )?;
    let mut symbol_tests_stmt = conn.prepare(
        r#"
        SELECT COUNT(*), (
            SELECT reason
            FROM symbol_test_edges
            WHERE symbol_id = ?1
            ORDER BY score DESC
            LIMIT 1
        )
        FROM symbol_test_edges
        WHERE symbol_id = ?1
        "#,
    )?;
    let mut reverse_symbol_tests_stmt = conn.prepare(
        r#"
        SELECT COUNT(*), (
            SELECT reason
            FROM symbol_test_edges
            WHERE test_symbol_id = ?1
            ORDER BY score DESC
            LIMIT 1
        )
        FROM symbol_test_edges
        WHERE test_symbol_id = ?1
        "#,
    )?;
    let mut file_tests_stmt = conn.prepare(
        r#"
        SELECT COUNT(*), (
            SELECT reason
            FROM file_test_edges
            WHERE file_id = ?1
            ORDER BY score DESC
            LIMIT 1
        )
        FROM file_test_edges
        WHERE file_id = ?1
        "#,
    )?;
    let mut reverse_file_tests_stmt = conn.prepare(
        r#"
        SELECT COUNT(*), (
            SELECT reason
            FROM file_test_edges
            WHERE test_file_id = ?1
            ORDER BY score DESC
            LIMIT 1
        )
        FROM file_test_edges
        WHERE test_file_id = ?1
        "#,
    )?;
    let mut entrypoints_stmt = conn.prepare(
        r#"
        SELECT COUNT(*), (
            SELECT reason
            FROM entrypoints
            WHERE (?1 IS NOT NULL AND symbol_id = ?1) OR file_id = ?2
            ORDER BY score DESC
            LIMIT 1
        )
        FROM entrypoints
        WHERE (?1 IS NOT NULL AND symbol_id = ?1) OR file_id = ?2
        "#,
    )?;

    for candidate in candidates.iter_mut() {
        let module_key = module_key(candidate);
        if module_key
            .as_ref()
            .is_some_and(|module| anchor_modules.contains(module))
        {
            candidate.same_module = true;
        }

        if let Some(symbol_id) = candidate.symbol_id {
            candidate.edge_hits = edge_degree_stmt
                .query_row([symbol_id], |row| row.get::<_, i64>(0))
                .unwrap_or(0)
                .max(0) as usize;
            candidate.edge_reason =
                best_edge_reason(&mut edge_reason_stmt, symbol_id, &anchor_symbols)?;

            let (count, reason) = if candidate.is_test {
                reverse_symbol_tests_stmt.query_row([symbol_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
                })?
            } else {
                symbol_tests_stmt.query_row([symbol_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
                })?
            };
            candidate.test_mapping_count = count.max(0) as usize;
            candidate.test_reason = reason;
        }

        if let Some(file_id) = candidate.file_id {
            let mut best_cochange = 0;
            for anchor_file_id in &anchor_files {
                if *anchor_file_id == file_id {
                    continue;
                }
                let value = cochange_stmt
                    .query_row(params![file_id, anchor_file_id], |row| row.get::<_, i64>(0))
                    .optional()?
                    .unwrap_or(0);
                best_cochange = best_cochange.max(value);
            }
            candidate.cochange_count = best_cochange;

            if candidate.test_mapping_count == 0 {
                let (count, reason) = if candidate.is_test {
                    reverse_file_tests_stmt.query_row([file_id], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
                    })?
                } else {
                    file_tests_stmt.query_row([file_id], |row| {
                        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
                    })?
                };
                candidate.test_mapping_count = count.max(0) as usize;
                candidate.test_reason = candidate.test_reason.clone().or(reason);
            }

            let (count, reason) = entrypoints_stmt
                .query_row(params![candidate.symbol_id, file_id], |row| {
                    Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?))
                })?;
            candidate.entrypoint_hits = count.max(0) as usize;
            candidate.entrypoint_reason = reason;
        }
    }

    Ok(())
}

pub(crate) fn confidence_from_scored_candidates(
    scored: &[ScoredCandidate<Candidate>],
    exact_match: bool,
) -> f32 {
    if scored.is_empty() {
        return 0.0;
    }
    if exact_match {
        return 0.99;
    }

    let top = scored[0].score.max(1.0);
    let second = scored.get(1).map(|item| item.score).unwrap_or(0.0);
    let gap = ((top - second) / top).clamp(0.0, 1.0) as f32;
    let normalized_top = (top / 180.0).clamp(0.0, 1.0) as f32;
    (0.35 + normalized_top * 0.3 + gap * 0.35).clamp(0.0, 0.99)
}

pub(crate) fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.contains("tests.rs")
        || lower.contains("/integration/")
        || lower.contains("test_")
        || lower.contains("_test")
}

pub(crate) fn is_low_signal_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("fixture")
        || lower.contains("fixtures")
        || lower.contains("snapshot")
        || lower.contains("snapshots")
        || lower.ends_with(".generated.rs")
        || lower.ends_with("/bindings.rs")
}

pub(crate) fn normalize_text(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric() || matches!(*ch, '_' | ':' | '/'))
        .collect::<String>()
        .to_ascii_lowercase()
}

fn collect_anchor_modules(query: &str, candidates: &[Candidate]) -> HashSet<String> {
    let mut modules = module_hints_from_query(query);
    let best_strength = candidates
        .iter()
        .map(anchor_strength)
        .max()
        .unwrap_or_default();

    for candidate in candidates {
        if anchor_strength(candidate) == best_strength && best_strength > 0 {
            if let Some(module) = module_key(candidate) {
                modules.insert(module);
            }
        }
    }
    modules
}

fn collect_anchor_ids<F>(candidates: &[Candidate], mut selector: F) -> HashSet<i64>
where
    F: FnMut(&Candidate) -> Option<i64>,
{
    let best_strength = candidates
        .iter()
        .map(anchor_strength)
        .max()
        .unwrap_or_default();
    let mut ids = HashSet::new();
    for candidate in candidates {
        if anchor_strength(candidate) == best_strength && best_strength > 0 {
            if let Some(id) = selector(candidate) {
                ids.insert(id);
            }
        }
    }
    ids
}

fn module_hints_from_query(query: &str) -> HashSet<String> {
    let mut modules = HashSet::new();
    if let Some((prefix, _)) = query.rsplit_once("::") {
        modules.insert(prefix.to_string());
    }
    if let Some((prefix, _)) = query.rsplit_once('/') {
        modules.insert(prefix.replace('/', "::"));
    }
    modules
}

fn module_key(candidate: &Candidate) -> Option<String> {
    if let Some(module) = &candidate.module_path {
        return Some(module.clone());
    }
    if let Some(qualname) = &candidate.qualname {
        if let Some((prefix, _)) = qualname.rsplit_once("::") {
            return Some(prefix.to_string());
        }
    }
    candidate
        .path
        .rsplit_once('/')
        .map(|(prefix, _)| prefix.replace('/', "::"))
}

fn anchor_strength(candidate: &Candidate) -> usize {
    let mut score = 0usize;
    if candidate.exact_name_match {
        score += 4;
    }
    if candidate.exact_qualname_match || candidate.exact_path_match {
        score += 3;
    }
    if candidate.prefix_match {
        score += 2;
    }
    if candidate.substring_match {
        score += 1;
    }
    if candidate.fuzzy_score > 0.7 {
        score += 1;
    }
    score
}

fn best_edge_reason(
    stmt: &mut rusqlite::Statement<'_>,
    symbol_id: i64,
    _anchor_symbols: &HashSet<i64>,
) -> Result<Option<String>> {
    let rows = stmt.query_map([symbol_id], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<String>>(3)?,
        ))
    })?;

    let mut fallback = None;
    for row in rows {
        let (edge_type, source, target, _evidence) = row?;
        let reason = match edge_type.as_str() {
            "calls" => source
                .map(|label| format!("called by {}", short_label(&label)))
                .or_else(|| target.map(|label| format!("calls {}", short_label(&label)))),
            "implements" => target
                .or(source)
                .map(|label| format!("implements {}", short_label(&label))),
            "tests" => source
                .or(target)
                .map(|label| format!("covered by {}", short_label(&label))),
            _ => source
                .or(target)
                .map(|label| format!("related to {}", short_label(&label))),
        };
        if reason.is_some() {
            fallback = reason;
        }
    }
    Ok(fallback)
}

fn fuzzy_score(query: &str, haystacks: &[&str]) -> f64 {
    if query.is_empty() {
        return 0.0;
    }
    haystacks
        .iter()
        .map(|haystack| subsequence_ratio(query, haystack))
        .fold(0.0, f64::max)
}

fn subsequence_ratio(query: &str, haystack: &str) -> f64 {
    if haystack.is_empty() {
        return 0.0;
    }
    let mut matched = 0usize;
    let mut chars = query.chars();
    let mut current = chars.next();
    for ch in haystack.chars() {
        if Some(ch) == current {
            matched += 1;
            current = chars.next();
            if current.is_none() {
                break;
            }
        }
    }
    matched as f64 / query.len().max(1) as f64
}

fn short_label(label: &str) -> String {
    label.rsplit("::").next().unwrap_or(label).to_string()
}
