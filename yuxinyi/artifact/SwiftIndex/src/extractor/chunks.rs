use crate::model::ChunkRecord;

/// Build an FTS chunk for a symbol-sized source range.
pub fn chunk_for_symbol(
    chunk_kind: &str,
    start_line: i64,
    end_line: i64,
    summary: String,
    content: String,
) -> ChunkRecord {
    ChunkRecord {
        id: None,
        file_id: None,
        symbol_id: None,
        chunk_kind: chunk_kind.to_string(),
        start_line,
        end_line,
        summary,
        content,
    }
}
