use serde::Serialize;

/// Unified compact payload returned by query interfaces.
#[derive(Debug, Clone, Serialize)]
pub struct CompactResult<T> {
    pub items: Vec<T>,
    pub confidence: f32,
    pub is_exhaustive: bool,
    pub expansion_hint: Option<String>,
}

/// Size budget applied during compact post-processing.
#[derive(Debug, Clone, Copy)]
pub struct QueryBudget {
    pub max_items: usize,
    pub max_total_chars: usize,
    pub max_reasons_per_item: usize,
}

/// Search hit for symbol lookups.
#[derive(Debug, Clone, Serialize)]
pub struct SymbolSearchResult {
    pub qualname: String,
    pub kind: String,
    pub file_path: String,
    pub line_range: (usize, usize),
    pub signature: Option<String>,
    pub summary: String,
    pub score: f64,
    pub why: Vec<String>,
}

/// Search hit for file lookups.
#[derive(Debug, Clone, Serialize)]
pub struct FileSearchResult {
    pub path: String,
    pub crate_name: Option<String>,
    pub module_path: Option<String>,
    pub summary: String,
    pub symbol_count: usize,
    pub score: f64,
    pub why: Vec<String>,
}

/// Outline payload for a single file.
#[derive(Debug, Clone, Serialize)]
pub struct OutlineResult {
    pub path: String,
    pub imports: Vec<String>,
    pub top_level_symbols: Vec<String>,
    pub impl_blocks: Vec<String>,
    pub test_functions: Vec<String>,
    pub summary: String,
    pub why: Vec<String>,
}

/// Short symbol/file relationship summary.
#[derive(Debug, Clone, Serialize)]
pub struct NeighborItem {
    pub label: String,
    pub kind: String,
    pub detail: Option<String>,
    pub why: Vec<String>,
}

/// Neighbor payload around a symbol.
#[derive(Debug, Clone, Serialize)]
pub struct NeighborResult {
    pub qualname: String,
    pub file_path: String,
    pub parent_symbol: Option<String>,
    pub methods: Vec<NeighborItem>,
    pub impl_relations: Vec<NeighborItem>,
    pub likely_callees: Vec<NeighborItem>,
    pub related_tests: Vec<NeighborItem>,
    pub cochanged_files: Vec<NeighborItem>,
    pub entrypoints: Vec<NeighborItem>,
    pub why: Vec<String>,
}

/// Line-based snippet read result.
#[derive(Debug, Clone, Serialize)]
pub struct SnippetResult {
    pub path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub content: String,
    pub why: Vec<String>,
}

/// Structured suggestion item for likely edit targets.
#[derive(Debug, Clone, Serialize)]
pub struct SuggestTarget {
    pub path: String,
    pub qualname: Option<String>,
    pub summary: Option<String>,
    pub score: f64,
    pub why: Vec<String>,
}

/// Suggestion payload for likely edit targets.
#[derive(Debug, Clone, Serialize)]
pub struct SuggestResult {
    pub files: Vec<SuggestTarget>,
    pub symbols: Vec<SuggestTarget>,
    pub tests: Vec<SuggestTarget>,
    pub why: Vec<String>,
}

/// Indexed entrypoint surfaced for navigation and suggestions.
#[derive(Debug, Clone, Serialize)]
pub struct EntrypointResult {
    pub kind: String,
    pub path: String,
    pub qualname: Option<String>,
    pub score: f64,
    pub why: Vec<String>,
}

/// One-shot context payload for agent consumption.
#[derive(Debug, Clone, Serialize)]
pub struct ExplainResult {
    pub query: String,
    pub top_symbols: Vec<SymbolSearchResult>,
    pub top_files: Vec<FileSearchResult>,
    pub outline_summary: String,
    pub neighbors: Vec<NeighborItem>,
    pub next_steps: Vec<String>,
    pub why: Vec<String>,
}

/// High-level database statistics.
#[derive(Debug, Clone, Serialize)]
pub struct StatsResult {
    pub files: usize,
    pub symbols: usize,
    pub chunks: usize,
    pub edges: usize,
    pub git_stats_files: usize,
    pub git_cochange_pairs: usize,
}
