use std::collections::BTreeMap;

use crate::model::{SymbolKind, SymbolRecord};

/// Produce a concise summary for a file or symbol from static heuristics.
pub fn summarize(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| part.trim())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

/// Build a rules-based summary for a symbol.
pub fn summarize_symbol(symbol: &SymbolRecord) -> String {
    let mut parts = vec![format!("{} {}", symbol.kind.as_str(), symbol.name)];
    if symbol.is_async {
        parts.push("async".to_string());
    }
    if symbol.is_test {
        parts.push("test".to_string());
    }
    if let Some(return_type) = &symbol.return_type {
        if return_type.contains("Result") {
            parts.push("returns Result".to_string());
        } else {
            parts.push(format!("returns {return_type}"));
        }
    }
    if let Some(signature) = &symbol.signature {
        if signature.contains("&self")
            || signature.contains("&mut self")
            || signature.contains(" self")
        {
            parts.push("method-like".to_string());
        }
    }
    if let Some(docs) = &symbol.docs {
        let first_line = docs.lines().next().unwrap_or_default().trim();
        if !first_line.is_empty() {
            parts.push(first_line.to_string());
        }
    }

    summarize(&parts)
}

/// Build a rules-based summary for a file.
pub fn summarize_file(
    crate_name: Option<&str>,
    module_path: Option<&str>,
    symbols: &[SymbolRecord],
) -> String {
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut has_async = false;
    let mut has_tests = false;
    let mut has_impl = false;
    let mut has_traits = false;
    for symbol in symbols {
        *counts.entry(symbol.kind.as_str()).or_default() += 1;
        has_async |= symbol.is_async;
        has_tests |= symbol.is_test;
        has_impl |= symbol.kind == SymbolKind::Impl;
        has_traits |= symbol.kind == SymbolKind::Trait;
    }

    let mut parts = Vec::new();
    if let Some(crate_name) = crate_name {
        parts.push(format!("crate {crate_name}"));
    }
    if let Some(module_path) = module_path {
        parts.push(format!("module {module_path}"));
    }
    if !counts.is_empty() {
        let distribution = counts
            .into_iter()
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("contains {distribution}"));
    }
    if has_async {
        parts.push("includes async code".to_string());
    }
    if has_tests {
        parts.push("contains tests".to_string());
    }
    if has_traits {
        parts.push("defines trait contracts".to_string());
    }
    if has_impl {
        parts.push("implements behavior blocks".to_string());
    }

    summarize(&parts)
}
