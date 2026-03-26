CREATE TABLE IF NOT EXISTS symbol_test_edges (
    id INTEGER PRIMARY KEY,
    symbol_id INTEGER NOT NULL,
    test_symbol_id INTEGER NOT NULL,
    score REAL NOT NULL DEFAULT 0,
    reason TEXT NOT NULL,
    FOREIGN KEY (symbol_id) REFERENCES symbols(id) ON DELETE CASCADE,
    FOREIGN KEY (test_symbol_id) REFERENCES symbols(id) ON DELETE CASCADE,
    UNIQUE(symbol_id, test_symbol_id)
);

CREATE INDEX IF NOT EXISTS idx_symbol_test_edges_symbol ON symbol_test_edges(symbol_id);
CREATE INDEX IF NOT EXISTS idx_symbol_test_edges_test ON symbol_test_edges(test_symbol_id);

CREATE TABLE IF NOT EXISTS file_test_edges (
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL,
    test_file_id INTEGER NOT NULL,
    score REAL NOT NULL DEFAULT 0,
    reason TEXT NOT NULL,
    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE,
    FOREIGN KEY (test_file_id) REFERENCES files(id) ON DELETE CASCADE,
    UNIQUE(file_id, test_file_id)
);

CREATE INDEX IF NOT EXISTS idx_file_test_edges_file ON file_test_edges(file_id);
CREATE INDEX IF NOT EXISTS idx_file_test_edges_test ON file_test_edges(test_file_id);

CREATE TABLE IF NOT EXISTS entrypoints (
    id INTEGER PRIMARY KEY,
    kind TEXT NOT NULL,
    symbol_id INTEGER,
    file_id INTEGER NOT NULL,
    score REAL NOT NULL DEFAULT 0,
    reason TEXT NOT NULL,
    FOREIGN KEY (symbol_id) REFERENCES symbols(id) ON DELETE CASCADE,
    FOREIGN KEY (file_id) REFERENCES files(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_entrypoints_kind ON entrypoints(kind);
CREATE INDEX IF NOT EXISTS idx_entrypoints_symbol ON entrypoints(symbol_id);
CREATE INDEX IF NOT EXISTS idx_entrypoints_file ON entrypoints(file_id);
