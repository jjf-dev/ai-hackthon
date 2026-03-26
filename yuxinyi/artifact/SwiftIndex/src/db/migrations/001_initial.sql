CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    crate_name TEXT,
    module_path TEXT,
    hash TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime_ns INTEGER,
    line_count INTEGER NOT NULL,
    symbol_count INTEGER NOT NULL DEFAULT 0,
    is_generated INTEGER NOT NULL DEFAULT 0,
    has_tests INTEGER NOT NULL DEFAULT 0,
    summary TEXT,
    indexed_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS symbols (
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL,
    parent_symbol_id INTEGER,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    qualname TEXT NOT NULL UNIQUE,
    signature TEXT,
    docs TEXT,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    is_async INTEGER NOT NULL DEFAULT 0,
    is_test INTEGER NOT NULL DEFAULT 0,
    visibility TEXT,
    return_type TEXT,
    summary TEXT,
    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE,
    FOREIGN KEY (parent_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS symbol_edges (
    id INTEGER PRIMARY KEY,
    from_symbol_id INTEGER,
    to_symbol_id INTEGER,
    edge_type TEXT NOT NULL,
    evidence TEXT,
    FOREIGN KEY (from_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE,
    FOREIGN KEY (to_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL,
    symbol_id INTEGER,
    chunk_kind TEXT NOT NULL,
    start_line INTEGER NOT NULL,
    end_line INTEGER NOT NULL,
    summary TEXT,
    content TEXT NOT NULL,
    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE,
    FOREIGN KEY (symbol_id) REFERENCES symbols(id) ON DELETE SET NULL
);

CREATE TABLE IF NOT EXISTS file_imports (
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL,
    import_path TEXT NOT NULL,
    alias TEXT,
    is_glob INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS git_file_stats (
    file_id INTEGER PRIMARY KEY,
    commit_count INTEGER NOT NULL DEFAULT 0,
    last_modified INTEGER,
    last_author TEXT,
    authors_json TEXT,
    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS git_cochange (
    id INTEGER PRIMARY KEY,
    file_id_a INTEGER NOT NULL,
    file_id_b INTEGER NOT NULL,
    cochange_count INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY (file_id_a) REFERENCES files(id) ON DELETE CASCADE,
    FOREIGN KEY (file_id_b) REFERENCES files(id) ON DELETE CASCADE,
    UNIQUE(file_id_a, file_id_b)
);

CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);
CREATE INDEX IF NOT EXISTS idx_files_crate ON files(crate_name);
CREATE INDEX IF NOT EXISTS idx_files_hash ON files(hash);

CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
CREATE INDEX IF NOT EXISTS idx_symbols_parent ON symbols(parent_symbol_id);
CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);
CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
CREATE INDEX IF NOT EXISTS idx_symbols_qualname ON symbols(qualname);

CREATE INDEX IF NOT EXISTS idx_symbol_edges_from ON symbol_edges(from_symbol_id);
CREATE INDEX IF NOT EXISTS idx_symbol_edges_to ON symbol_edges(to_symbol_id);
CREATE INDEX IF NOT EXISTS idx_symbol_edges_type ON symbol_edges(edge_type);

CREATE INDEX IF NOT EXISTS idx_chunks_file ON chunks(file_id);
CREATE INDEX IF NOT EXISTS idx_chunks_symbol ON chunks(symbol_id);

CREATE INDEX IF NOT EXISTS idx_file_imports_file ON file_imports(file_id);
CREATE INDEX IF NOT EXISTS idx_git_cochange_pair ON git_cochange(file_id_a, file_id_b);

CREATE VIRTUAL TABLE IF NOT EXISTS files_fts USING fts5(
    path,
    crate_name,
    module_path,
    summary,
    tokenize = 'unicode61'
);

CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
    name,
    qualname,
    signature,
    docs,
    summary,
    tokenize = 'unicode61'
);

CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    content,
    summary,
    tokenize = 'unicode61'
);
