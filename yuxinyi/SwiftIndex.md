# SwiftIndex Architecture

## Overview

SwiftIndex (repo-index) is a lightweight Rust code indexer designed for AI agents. It builds a local SQLite index from source code and provides compact, context-efficient query interfaces for code navigation.

**Core Innovation**: Compact Retrieval + Progressive Expansion
Returns the smallest useful answer by default, allowing agents to expand results only when needed.

## System Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    CLI Interface                        │
│         build | update | query | stats                  │
└─────────────────────────────────────────────────────────┘
                           ↓
┌─────────────────────────────────────────────────────────┐
│                   Query Engine                          │
│  • Symbol/File Search    • Code Outline                 │
│  • Neighbor Discovery    • Edit Suggestions             │
│  • Context Explanation                                  │
└─────────────────────────────────────────────────────────┘
                           ↓
┌─────────────────────────────────────────────────────────┐
│              Compact Retrieval Layer                    │
│  • Confidence-aware top-k selection                     │
│  • Progressive expansion (--expand 0|1|2)               │
│  • Size budget enforcement                              │
└─────────────────────────────────────────────────────────┘
                           ↓
┌─────────────────────────────────────────────────────────┐
│               Multi-Signal Reranker                     │
│  Lexical • Structural • Git • Relationship signals      │
└─────────────────────────────────────────────────────────┘
                           ↓
┌─────────────────────────────────────────────────────────┐
│              SQLite + FTS5 Storage                      │
│  Files • Symbols • Chunks • Edges • Git Metrics         │
└─────────────────────────────────────────────────────────┘
                           ↑
┌─────────────────────────────────────────────────────────┐
│                 Indexing Pipeline                       │
│  Scan → Parse → Extract → Store → Derive Relations      │
└─────────────────────────────────────────────────────────┘
```

## Indexing Pipeline

### 1. Workspace Scanning
- Discovers Rust workspace via `Cargo.toml`
- Recursively scans `.rs` files
- Filters build artifacts and dependencies

### 2. Code Extraction
For each source file:
- **Parse**: tree-sitter generates AST
- **Extract Symbols**: functions, structs, traits, impls with qualified names
- **Extract Chunks**: code snippets for semantic search
- **Extract Edges**: call relationships, test mappings, imports
- **Generate Summaries**: concise descriptions for symbols and files

### 3. Storage
- **SQLite**: structured metadata (files, symbols, edges)
- **FTS5**: full-text search index on code chunks
- **Git Integration**: commit frequency, co-change patterns

### 4. Derived Relations
- Symbol call graphs
- Test-to-implementation mappings
- Entrypoint identification (main, tests, benchmarks)

## Query Pipeline

### 1. Candidate Recall
Multiple retrieval strategies run in parallel:
- Exact name/path matching
- FTS5 full-text search
- Fuzzy matching
- Relationship traversal (callers, callees, tests)

### 2. Unified Reranking
Multi-signal scoring combines:
- **Lexical**: exact match (+1000), prefix (+120), fuzzy (0-100)
- **Structural**: same module/crate bonus
- **Git**: file hotness, co-change frequency
- **Relationships**: call edges, test mappings, entrypoints

### 3. Compact Post-Processing ⭐
The key innovation layer:
- **Confidence Estimation**: exact matches → high confidence
- **Dynamic Top-K**:
  - High confidence → return top-1 only
  - Ambiguous → return top-3
  - Expansion mode → return top-5/8
- **Budget Enforcement**: trim summaries, limit total chars
- **Expansion Hints**: guide next query if needed

### 4. Result Format
```json
{
  "items": [...],
  "confidence": 0.85,
  "is_exhaustive": false,
  "expansion_hint": "expand to see more candidates"
}
```

## Query Interfaces

| Interface | Purpose | Example |
|-----------|---------|---------|
| `symbol` | Find functions, structs, traits | `symbol "dispatch"` |
| `file` | Find source files | `file "reranker"` |
| `outline` | Get file structure | `outline src/main.rs` |
| `neighbors` | Discover related code | `neighbors "rerank_candidates"` |
| `suggest` | Get edit targets | `suggest "add logging"` |
| `explain` | One-shot context bundle | `explain "ranking logic"` |

## Progressive Expansion

Agents can control result size:

```bash
--expand 0  # Minimal (top-1/3, ~700 chars)
--expand 1  # Moderate (top-5, ~1300 chars)
--expand 2  # Wide (top-8, ~2000 chars)
```

## Design Principles

1. **Compact by Default**: Return the smallest useful answer
2. **Agent-First**: Optimize for LLM context windows, not human browsing
3. **Surgical Changes**: Compaction runs after reranking, preserving existing logic
4. **Incremental**: Support both fresh builds and fast updates
5. **Local-First**: No network dependencies, embedded SQLite

## Technology Stack

- **Language**: Rust 2021
- **Database**: SQLite with FTS5
- **Parser**: tree-sitter + tree-sitter-rust
- **CLI**: clap 4.5
- **Serialization**: serde + serde_json

## Key Files

- `src/indexer/build.rs` - Index construction
- `src/query/compactor.rs` - Compact retrieval logic
- `src/ranking/reranker.rs` - Multi-signal ranking
- `src/extractor/symbols.rs` - AST extraction
- `src/db/` - SQLite storage layer
