# repo-index: Compact Retrieval for Agent-First Rust Code Navigation

`repo-index` is a lightweight local indexer for Rust workspaces on Linux. It builds a SQLite + FTS5 index from source code and exposes agent-oriented query interfaces for symbols, files, outlines, neighbors, explanations, snippets, and edit-target suggestions.

This submission focuses on one system-level improvement for agent workflows:

`Compact Retrieval + Progressive Expansion`

The goal is to reduce context usage for coding agents by returning the smallest useful answer for the next action instead of dumping long ranked lists by default.

## Problem

The original query flow already supported symbol search, file search, outlines, explain, and edit-target suggestions, but the default result shape still optimized too much for human browsing:

- queries could return longer lists than an agent usually needs
- the caller had to infer whether top-1 was already reliable
- interfaces did not expose a consistent compact payload
- expansion strategy was implicit instead of explicit

For agent-driven kernel and systems work, that wastes context budget and slows iteration.

## What This Work Adds

- unified compact result payload for query interfaces
- confidence-aware top-k selection
- progressive expansion via `--expand 0|1|2`
- compact-by-default output for `symbol`, `file`, `outline`, `neighbors`, `suggest`, and `explain`
- explicit `expansion_hint` to guide the next query
- compatibility escape hatch with `--compact false`

Example compact payload:

```json
{
  "items": [...],
  "confidence": 0.82,
  "is_exhaustive": false,
  "expansion_hint": "read top symbol snippet or expand to neighbors"
}
```

## Architecture

The system keeps the original indexing and reranking pipeline intact. The change is a post-processing layer after reranking.

### Indexing Layer

- `src/indexer/`: builds and updates the workspace index
- `src/extractor/`: extracts symbols, summaries, chunks, edges, and derived relations
- `src/db/`: stores everything in embedded SQLite with FTS5

### Retrieval Layer

- `src/query/symbols.rs`: symbol search and `read-symbol`
- `src/query/files.rs`: file search
- `src/query/outline.rs`: compact file outline and snippets
- `src/query/neighbors.rs`: related callees, tests, cochanged files, entrypoints
- `src/query/suggest.rs`: edit-target suggestions
- `src/query/explain.rs`: one-shot compact context bundle

### Ranking Layer

- `src/ranking/reranker.rs`: lexical, FTS, structural, git, test, and entrypoint-aware reranking

### Compact Retrieval Layer

- `src/model/query.rs`: `CompactResult<T>` and `QueryBudget`
- `src/query/compactor.rs`: post-rerank compaction, top-k decision, truncation, deduplication, and size-budget enforcement
- `src/query/common.rs`: query options and confidence estimation
- `src/cli/mod.rs`: query-wide `--compact` and `--expand` flags

## Retrieval Flow

```text
Rust workspace
  -> build/update index
  -> raw candidates from SQL/FTS/chunk recall
  -> unified reranker
  -> compactor
  -> compact JSON payload
  -> agent decides whether to expand
```

Detailed behavior:

1. Fetch candidates from the existing DB and FTS tables.
2. Rerank with the existing multi-signal reranker.
3. Estimate confidence from exact-match signals and score gaps.
4. Choose top-k dynamically:
   - top-1 for exact/high-confidence results
   - top-3 for ambiguous results
   - wider recall only with explicit expansion
5. Trim summaries and reasons under a per-query budget.
6. Return `expansion_hint` when more context may still be needed.

## Key Features

### 1. Compact by Default

`symbol`, `file`, `outline`, `neighbors`, `suggest`, and `explain` now return compact payloads by default.

### 2. Progressive Expansion

- `--expand 0`: minimal answer
- `--expand 1`: moderate expansion
- `--expand 2`: wider recall

### 3. Confidence-Aware Top-K

- exact symbol/path matches prefer top-1
- large top1/top2 gaps also prefer top-1
- ambiguous queries are capped to a small set

### 4. Compatibility

`--compact false` preserves the old verbose payload shape so existing consumers do not break immediately.

## Usage

Build:

```bash
cargo build --release
```

Build or update an index:

```bash
./target/release/repo-index build --path /path/to/repo
./target/release/repo-index update --path /path/to/repo
```

Important: query-wide options must appear before the subcommand.

```bash
./target/release/repo-index query --workspace /repo --db /repo/.swiftindex/index.db --json symbol dispatch
./target/release/repo-index query --workspace /repo --db /repo/.swiftindex/index.db --json --expand 1 suggest "query dispatch"
./target/release/repo-index query --workspace /repo --db /repo/.swiftindex/index.db --json --compact false symbol dispatch
```

## Validation

This work was validated with:

- `cargo test`
- `cargo build --release`
- end-to-end CLI checks on the local `SwiftIndex` repository
- compact and verbose output compatibility checks
- updated SwiftIndex navigation skill documentation for the new compact protocol

## Repository Map For Review

- [src/model/query.rs](/root/SwiftIndex/src/model/query.rs)
- [src/query/compactor.rs](/root/SwiftIndex/src/query/compactor.rs)
- [src/query/symbols.rs](/root/SwiftIndex/src/query/symbols.rs)
- [src/query/files.rs](/root/SwiftIndex/src/query/files.rs)
- [src/query/outline.rs](/root/SwiftIndex/src/query/outline.rs)
- [src/query/neighbors.rs](/root/SwiftIndex/src/query/neighbors.rs)
- [src/query/suggest.rs](/root/SwiftIndex/src/query/suggest.rs)
- [src/query/explain.rs](/root/SwiftIndex/src/query/explain.rs)
- [src/cli/mod.rs](/root/SwiftIndex/src/cli/mod.rs)
- [tests/cli_flow.rs](/root/SwiftIndex/tests/cli_flow.rs)

## Submission Files

- `README.md`: architecture, functionality, and validation summary
- `experiment.md`: how the agent workflow was organized, what worked, what failed, and how it was corrected
- `artifacts/`: reproduction script and sample outputs
