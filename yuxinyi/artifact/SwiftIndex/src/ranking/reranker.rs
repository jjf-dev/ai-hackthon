use std::cmp::Ordering;

/// Shared candidate kinds routed through the unified reranker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateType {
    Symbol,
    File,
    Test,
    Entrypoint,
}

/// Query candidate assembled before scoring.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub key: String,
    pub candidate_type: CandidateType,
    pub name: String,
    pub qualname: Option<String>,
    pub path: String,
    pub module_path: Option<String>,
    pub crate_name: Option<String>,
    pub summary: String,
    pub detail_kind: String,
    pub signature: Option<String>,
    pub line_range: Option<(usize, usize)>,
    pub symbol_id: Option<i64>,
    pub file_id: Option<i64>,
    pub symbol_count: Option<usize>,
    pub exact_name_match: bool,
    pub exact_qualname_match: bool,
    pub exact_path_match: bool,
    pub prefix_match: bool,
    pub substring_match: bool,
    pub fuzzy_score: f64,
    pub fts_score: f64,
    pub chunk_fts_score: f64,
    pub same_module: bool,
    pub edge_hits: usize,
    pub edge_reason: Option<String>,
    pub cochange_count: i64,
    pub git_hotness: i64,
    pub test_mapping_count: usize,
    pub test_reason: Option<String>,
    pub entrypoint_hits: usize,
    pub entrypoint_reason: Option<String>,
    pub low_signal: bool,
    pub is_test: bool,
    pub reasons: Vec<String>,
}

/// Scored search candidate with stable explanations.
#[derive(Debug, Clone)]
pub struct ScoredCandidate<T = Candidate> {
    pub candidate: T,
    pub score: f64,
    pub reasons: Vec<String>,
}

/// Apply unified multi-signal ranking for all search surfaces.
pub fn rerank_candidates(
    query: &str,
    candidates: Vec<Candidate>,
) -> Vec<ScoredCandidate<Candidate>> {
    let wants_test = wants_test(query);
    let wants_fixture = wants_fixture(query);
    let mut scored = candidates
        .into_iter()
        .map(|candidate| {
            let mut score = 0.0;
            let mut reasons = candidate.reasons.clone();

            if candidate.exact_name_match {
                score += match candidate.candidate_type {
                    CandidateType::Symbol | CandidateType::Test => 1_000.0,
                    CandidateType::File => 180.0,
                    CandidateType::Entrypoint => 150.0,
                };
                reasons.push(exact_name_reason(&candidate).to_string());
            }
            if candidate.exact_qualname_match {
                score += 420.0;
                reasons.push("exact qualified name match".to_string());
            }
            if candidate.exact_path_match {
                score += 320.0;
                reasons.push("exact file path match".to_string());
            }
            if candidate.prefix_match {
                score += 95.0;
                reasons.push("prefix lexical match".to_string());
            }
            if candidate.substring_match {
                score += 32.0;
                reasons.push("substring lexical match".to_string());
            }
            if candidate.fuzzy_score >= 0.35 {
                score += candidate.fuzzy_score * 48.0;
                reasons.push("fuzzy match".to_string());
            }

            let fts_total = candidate.fts_score.min(28.0) + candidate.chunk_fts_score.min(18.0);
            if fts_total > 0.0 {
                score += fts_total;
                reasons.push(if candidate.chunk_fts_score > candidate.fts_score {
                    "chunk content full-text match".to_string()
                } else {
                    "full-text match".to_string()
                });
            }

            if candidate.same_module {
                score += 42.0;
                reasons.push("same module as query".to_string());
            }
            if candidate.edge_hits > 0 {
                score += (candidate.edge_hits.min(6) as f64) * 6.0;
                reasons.push(
                    candidate
                        .edge_reason
                        .clone()
                        .unwrap_or_else(|| "connected through symbol graph".to_string()),
                );
            }
            if candidate.cochange_count > 0 {
                score += (candidate.cochange_count.min(25) as f64).sqrt() * 5.0;
                reasons.push(format!("cochanged {} times", candidate.cochange_count));
            }
            if candidate.git_hotness > 0 {
                score += (candidate.git_hotness.min(250) as f64).ln_1p() * 4.0;
                reasons.push(format!(
                    "hot file in git history ({} commits)",
                    candidate.git_hotness
                ));
            }
            if candidate.test_mapping_count > 0 {
                score += match candidate.candidate_type {
                    CandidateType::Test => 32.0,
                    _ => 16.0,
                };
                score += (candidate.test_mapping_count.min(4) as f64) * 3.0;
                reasons.push(
                    candidate
                        .test_reason
                        .clone()
                        .unwrap_or_else(|| "has mapped tests".to_string()),
                );
            }
            if candidate.entrypoint_hits > 0 {
                score += 12.0 + (candidate.entrypoint_hits.min(3) as f64) * 4.0;
                reasons.push(
                    candidate
                        .entrypoint_reason
                        .clone()
                        .unwrap_or_else(|| "reachable from indexed entrypoint".to_string()),
                );
            }

            if candidate.is_test && !wants_test {
                score -= 26.0;
            }
            if candidate.low_signal && !wants_fixture {
                score -= 24.0;
            }

            let reasons = dedup_reasons(reasons, fallback_reason(&candidate));
            ScoredCandidate {
                candidate,
                score,
                reasons,
            }
        })
        .collect::<Vec<_>>();

    scored.sort_by(compare_scored);
    scored
}

fn compare_scored(
    left: &ScoredCandidate<Candidate>,
    right: &ScoredCandidate<Candidate>,
) -> Ordering {
    right
        .score
        .partial_cmp(&left.score)
        .unwrap_or(Ordering::Equal)
        .then_with(|| {
            right
                .candidate
                .exact_name_match
                .cmp(&left.candidate.exact_name_match)
        })
        .then_with(|| {
            right
                .candidate
                .exact_qualname_match
                .cmp(&left.candidate.exact_qualname_match)
        })
        .then_with(|| {
            right
                .candidate
                .prefix_match
                .cmp(&left.candidate.prefix_match)
        })
        .then_with(|| {
            right
                .candidate
                .fuzzy_score
                .partial_cmp(&left.candidate.fuzzy_score)
                .unwrap_or(Ordering::Equal)
        })
        .then_with(|| left.candidate.path.cmp(&right.candidate.path))
        .then_with(|| left.candidate.name.cmp(&right.candidate.name))
}

fn dedup_reasons(reasons: Vec<String>, fallback: &str) -> Vec<String> {
    let mut deduped = Vec::new();
    for reason in reasons {
        if reason.trim().is_empty() {
            continue;
        }
        if !deduped.iter().any(|existing: &String| existing == &reason) {
            deduped.push(reason);
        }
    }
    if deduped.is_empty() {
        deduped.push(fallback.to_string());
    }
    deduped
}

fn exact_name_reason(candidate: &Candidate) -> &'static str {
    match candidate.candidate_type {
        CandidateType::File => "exact file name match",
        CandidateType::Entrypoint => "exact entrypoint name match",
        CandidateType::Symbol | CandidateType::Test => "exact symbol name match",
    }
}

fn fallback_reason(candidate: &Candidate) -> &'static str {
    match candidate.candidate_type {
        CandidateType::File => "file candidate from fallback retrieval",
        CandidateType::Entrypoint => "entrypoint candidate from fallback retrieval",
        CandidateType::Symbol | CandidateType::Test => "symbol candidate from fallback retrieval",
    }
}

fn wants_test(query: &str) -> bool {
    let query = query.to_ascii_lowercase();
    query.contains("test") || query.contains("spec") || query.contains("integration")
}

fn wants_fixture(query: &str) -> bool {
    let query = query.to_ascii_lowercase();
    wants_test(&query) || query.contains("fixture") || query.contains("snapshot")
}
