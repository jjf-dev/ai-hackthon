use std::{borrow::Cow, collections::HashSet};

use crate::{
    model::{
        CompactResult, FileSearchResult, NeighborItem, QueryBudget, SuggestTarget,
        SymbolSearchResult,
    },
    ranking::reranker::ScoredCandidate,
};

const HIGH_CONFIDENCE_THRESHOLD: f32 = 0.9;
const SUMMARY_LIMIT: usize = 140;
const REASON_LIMIT: usize = 96;

pub fn symbol_budget(expand: u8) -> QueryBudget {
    match expand {
        0 => QueryBudget {
            max_items: 3,
            max_total_chars: 720,
            max_reasons_per_item: 2,
        },
        1 => QueryBudget {
            max_items: 5,
            max_total_chars: 1_300,
            max_reasons_per_item: 3,
        },
        _ => QueryBudget {
            max_items: 8,
            max_total_chars: 2_000,
            max_reasons_per_item: 4,
        },
    }
}

pub fn file_budget(expand: u8) -> QueryBudget {
    match expand {
        0 => QueryBudget {
            max_items: 3,
            max_total_chars: 560,
            max_reasons_per_item: 2,
        },
        1 => QueryBudget {
            max_items: 5,
            max_total_chars: 1_000,
            max_reasons_per_item: 3,
        },
        _ => QueryBudget {
            max_items: 7,
            max_total_chars: 1_500,
            max_reasons_per_item: 4,
        },
    }
}

pub fn suggest_budget(expand: u8, minimal_items: usize, expanded_items: usize) -> QueryBudget {
    match expand {
        0 => QueryBudget {
            max_items: minimal_items,
            max_total_chars: 520,
            max_reasons_per_item: 2,
        },
        1 => QueryBudget {
            max_items: expanded_items,
            max_total_chars: 960,
            max_reasons_per_item: 3,
        },
        _ => QueryBudget {
            max_items: expanded_items + 2,
            max_total_chars: 1_400,
            max_reasons_per_item: 4,
        },
    }
}

pub trait CompactItem {
    fn dedupe_key(&self) -> Cow<'_, str>;
    fn estimated_chars(&self) -> usize;
    fn summary_mut(&mut self) -> Option<&mut String> {
        None
    }
    fn reasons_mut(&mut self) -> Option<&mut Vec<String>> {
        None
    }
}

pub fn compact_results<T>(
    raw: Vec<ScoredCandidate<T>>,
    budget: &QueryBudget,
    confidence: f32,
) -> CompactResult<T>
where
    T: CompactItem,
{
    let scores = raw.iter().map(|item| item.score as f32).collect::<Vec<_>>();
    let requested_items = if confidence >= HIGH_CONFIDENCE_THRESHOLD {
        1
    } else {
        decide_topk(&scores)
    }
    .min(budget.max_items.max(1));

    let mut seen = HashSet::new();
    let mut items = Vec::new();
    let mut total_chars = 0;
    let mut dropped = false;

    for scored in raw {
        let mut item = scored.candidate;
        let key = item.dedupe_key().into_owned();
        if !seen.insert(key) {
            dropped = true;
            continue;
        }

        trim_item(&mut item, budget);
        let item_chars = item.estimated_chars();
        if items.len() >= requested_items || total_chars + item_chars > budget.max_total_chars {
            dropped = true;
            continue;
        }

        total_chars += item_chars;
        items.push(item);
    }

    let is_exhaustive = !dropped;
    CompactResult {
        items,
        confidence: confidence.clamp(0.0, 1.0),
        is_exhaustive,
        expansion_hint: if is_exhaustive {
            None
        } else {
            Some("expand for additional results".to_string())
        },
    }
}

pub fn decide_topk(scores: &[f32]) -> usize {
    match scores {
        [] => 0,
        [_] => 1,
        [top, second, ..] => {
            let top = (*top).max(1.0);
            let gap = ((top - *second) / top).clamp(0.0, 1.0);
            if gap >= 0.35 {
                1
            } else {
                3
            }
        }
    }
}

fn trim_item<T>(item: &mut T, budget: &QueryBudget)
where
    T: CompactItem,
{
    if let Some(summary) = item.summary_mut() {
        *summary = truncate_line(summary, SUMMARY_LIMIT);
    }

    if let Some(reasons) = item.reasons_mut() {
        let mut trimmed = Vec::new();
        for reason in reasons.drain(..) {
            let reason = truncate_line(&reason, REASON_LIMIT);
            if trimmed.iter().any(|existing: &String| existing == &reason) {
                continue;
            }
            trimmed.push(reason);
            if trimmed.len() >= budget.max_reasons_per_item {
                break;
            }
        }
        *reasons = trimmed;
    }
}

fn truncate_line(value: &str, limit: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= limit {
        return normalized;
    }

    let mut truncated = normalized
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

impl CompactItem for SymbolSearchResult {
    fn dedupe_key(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.qualname)
    }

    fn estimated_chars(&self) -> usize {
        self.qualname.len()
            + self.file_path.len()
            + self.summary.len()
            + self
                .signature
                .as_ref()
                .map(|value| value.len())
                .unwrap_or_default()
            + self.why.iter().map(String::len).sum::<usize>()
            + 32
    }

    fn summary_mut(&mut self) -> Option<&mut String> {
        Some(&mut self.summary)
    }

    fn reasons_mut(&mut self) -> Option<&mut Vec<String>> {
        Some(&mut self.why)
    }
}

impl CompactItem for FileSearchResult {
    fn dedupe_key(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.path)
    }

    fn estimated_chars(&self) -> usize {
        self.path.len()
            + self.summary.len()
            + self
                .crate_name
                .as_ref()
                .map(|value| value.len())
                .unwrap_or_default()
            + self
                .module_path
                .as_ref()
                .map(|value| value.len())
                .unwrap_or_default()
            + self.why.iter().map(String::len).sum::<usize>()
            + 24
    }

    fn summary_mut(&mut self) -> Option<&mut String> {
        Some(&mut self.summary)
    }

    fn reasons_mut(&mut self) -> Option<&mut Vec<String>> {
        Some(&mut self.why)
    }
}

impl CompactItem for SuggestTarget {
    fn dedupe_key(&self) -> Cow<'_, str> {
        if let Some(qualname) = &self.qualname {
            Cow::Borrowed(qualname)
        } else {
            Cow::Borrowed(&self.path)
        }
    }

    fn estimated_chars(&self) -> usize {
        self.path.len()
            + self
                .qualname
                .as_ref()
                .map(|value| value.len())
                .unwrap_or_default()
            + self
                .summary
                .as_ref()
                .map(|value| value.len())
                .unwrap_or_default()
            + self.why.iter().map(String::len).sum::<usize>()
            + 24
    }

    fn summary_mut(&mut self) -> Option<&mut String> {
        self.summary.as_mut()
    }

    fn reasons_mut(&mut self) -> Option<&mut Vec<String>> {
        Some(&mut self.why)
    }
}

impl CompactItem for NeighborItem {
    fn dedupe_key(&self) -> Cow<'_, str> {
        Cow::Owned(format!("{}:{}", self.kind, self.label))
    }

    fn estimated_chars(&self) -> usize {
        self.label.len()
            + self.kind.len()
            + self
                .detail
                .as_ref()
                .map(|value| value.len())
                .unwrap_or_default()
            + self.why.iter().map(String::len).sum::<usize>()
            + 24
    }

    fn summary_mut(&mut self) -> Option<&mut String> {
        self.detail.as_mut()
    }

    fn reasons_mut(&mut self) -> Option<&mut Vec<String>> {
        Some(&mut self.why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct DummyItem {
        key: String,
        summary: String,
        why: Vec<String>,
    }

    impl CompactItem for DummyItem {
        fn dedupe_key(&self) -> Cow<'_, str> {
            Cow::Borrowed(&self.key)
        }

        fn estimated_chars(&self) -> usize {
            self.key.len() + self.summary.len() + self.why.iter().map(String::len).sum::<usize>()
        }

        fn summary_mut(&mut self) -> Option<&mut String> {
            Some(&mut self.summary)
        }

        fn reasons_mut(&mut self) -> Option<&mut Vec<String>> {
            Some(&mut self.why)
        }
    }

    #[test]
    fn high_confidence_prefers_top_one() {
        let raw = vec![
            ScoredCandidate {
                candidate: DummyItem {
                    key: "best".to_string(),
                    summary: "top hit".to_string(),
                    why: vec![
                        "exact".to_string(),
                        "duplicate".to_string(),
                        "duplicate".to_string(),
                    ],
                },
                score: 100.0,
                reasons: vec![],
            },
            ScoredCandidate {
                candidate: DummyItem {
                    key: "second".to_string(),
                    summary: "runner up".to_string(),
                    why: vec!["fuzzy".to_string()],
                },
                score: 20.0,
                reasons: vec![],
            },
        ];

        let result = compact_results(
            raw,
            &QueryBudget {
                max_items: 3,
                max_total_chars: 200,
                max_reasons_per_item: 1,
            },
            0.96,
        );

        assert_eq!(result.items.len(), 1);
        assert!(result.items[0].why.len() <= 1);
    }

    #[test]
    fn ambiguous_results_stay_capped() {
        let raw = vec![
            make_dummy("a", 10.0),
            make_dummy("b", 9.7),
            make_dummy("c", 9.5),
            make_dummy("d", 9.4),
        ];

        let result = compact_results(
            raw,
            &QueryBudget {
                max_items: 3,
                max_total_chars: 500,
                max_reasons_per_item: 2,
            },
            0.62,
        );

        assert_eq!(result.items.len(), 3);
        assert!(!result.is_exhaustive);
    }

    fn make_dummy(key: &str, score: f64) -> ScoredCandidate<DummyItem> {
        ScoredCandidate {
            candidate: DummyItem {
                key: key.to_string(),
                summary: format!("{key} summary"),
                why: vec!["reason".to_string()],
            },
            score,
            reasons: vec![],
        }
    }
}
