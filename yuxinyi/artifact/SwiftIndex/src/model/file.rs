use serde::Serialize;

/// Indexed file metadata persisted in SQLite.
#[derive(Debug, Clone, Serialize)]
pub struct FileRecord {
    pub id: Option<i64>,
    pub path: String,
    pub crate_name: Option<String>,
    pub module_path: Option<String>,
    pub hash: String,
    pub size: i64,
    pub mtime_ns: Option<i64>,
    pub line_count: i64,
    pub symbol_count: i64,
    pub is_generated: bool,
    pub has_tests: bool,
    pub summary: String,
    pub indexed_at: i64,
}

/// Snippet-sized code block used for FTS and targeted reads.
#[derive(Debug, Clone, Serialize)]
pub struct ChunkRecord {
    pub id: Option<i64>,
    pub file_id: Option<i64>,
    pub symbol_id: Option<i64>,
    pub chunk_kind: String,
    pub start_line: i64,
    pub end_line: i64,
    pub summary: String,
    pub content: String,
}

/// Imported module or symbol recorded per file.
#[derive(Debug, Clone, Serialize)]
pub struct FileImportRecord {
    pub id: Option<i64>,
    pub file_id: Option<i64>,
    pub import_path: String,
    pub alias: Option<String>,
    pub is_glob: bool,
}

/// Git hotspot summary for a file.
#[derive(Debug, Clone, Serialize)]
pub struct GitFileStatRecord {
    pub file_id: i64,
    pub commit_count: i64,
    pub last_modified: Option<i64>,
    pub last_author: Option<String>,
    pub authors_json: String,
}

/// Pairwise co-change metric between files.
#[derive(Debug, Clone, Serialize)]
pub struct GitCochangeRecord {
    pub file_id_a: i64,
    pub file_id_b: i64,
    pub cochange_count: i64,
}
