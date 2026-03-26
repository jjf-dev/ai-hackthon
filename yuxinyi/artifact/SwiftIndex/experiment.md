# Experiment Log: Using Codex To Implement Compact Retrieval

## Objective

Use a coding agent to evolve an existing Rust code indexer without redesigning the system:

- add compact retrieval
- add progressive expansion
- preserve CLI compatibility
- keep the reranker intact
- keep the implementation incremental and compilable

## Environment

- Project: `repo-index` in `/root/SwiftIndex`
- Language: Rust
- Workflow: terminal-based Codex session with local file edits, compilation, tests, and CLI verification
- Data source: local SQLite index at `.swiftindex/index.db`

## Development Strategy

The implementation was intentionally staged instead of attempting a one-shot rewrite.

### Step 1. Read The Existing Query Surface

The first pass inspected:

- current CLI contract
- query modules
- ranking and reranking types
- existing JSON payload shapes
- existing integration test coverage

This reduced the risk of adding a new layer in the wrong place.

### Step 2. Add A Narrow New Abstraction

The first code change only introduced:

- `CompactResult<T>`
- `QueryBudget`
- generic `ScoredCandidate<T>`
- a new compactor module
- query options for `compact` and `expand`

This compiled before any query behavior changed.

### Step 3. Attach The Compactor After Reranking

The implementation explicitly avoided touching retrieval SQL or reranker heuristics.

The compactor runs after reranking and is responsible for:

- top-k reduction
- high-confidence top-1 preference
- reason trimming
- summary truncation
- deduplication
- total-size limiting

This was the key architectural decision because it preserved the original indexing and ranking design.

### Step 4. Migrate Query Interfaces Incrementally

Interfaces were migrated in this order:

1. `symbol`
2. `file`
3. `outline`
4. `neighbors`
5. `suggest`
6. `explain`

The staged order was important because `suggest` and `explain` compose other query surfaces.

### Step 5. Lock In Behavior With Tests

Validation included:

- unit tests for compactor behavior
- CLI integration test updates for the new compact payload shape
- explicit compatibility check for `--compact false`

## Effective Agent Patterns

### Pattern 1. Constrain The Insertion Point

The strongest prompt constraint was architectural:

- do not redesign the system
- do not change storage
- run compaction after reranking

That kept the agent focused on a surgical change instead of a rewrite.

### Pattern 2. Force Incremental Compilation

The requirement that each step compile was useful. It naturally pushed the work into narrow edits and exposed API mismatches early.

### Pattern 3. Treat Docs And Skills As Part Of The Deliverable

The code change was not the full result. The agent also updated the SwiftIndex navigation skill so future sessions know to use:

- compact mode by default
- `confidence`
- `is_exhaustive`
- `expansion_hint`
- `--expand 0|1|2`

This is exactly the kind of durable process asset that matters in agent-assisted development.

## Failures And Corrections

### Failure 1. `/trellis:start` Was Documented But Not Available

The repo instructions referenced `/trellis:start`, but the command was not installed in the shell environment.

Correction:

- read the underlying Trellis files directly
- continue without blocking on helper tooling

### Failure 2. Early Type Wiring Broke Build

After introducing compact options, the first compile failed because query entrypoints had not yet been updated to accept the new arguments.

Correction:

- wire the new `QueryOptions` through each query module
- keep compiling after each stage

### Failure 3. Query Examples Initially Returned Empty Results

Some validation queries used strings that were not represented in the current local index.

Correction:

- rebuild the release binary
- refresh the index
- validate with symbols and files known to exist in the repo, such as `dispatch` and `symbols.rs`

### Failure 4. Docs Can Drift From The Binary

Updating the SwiftIndex skill alone was not enough because it points at the release binary in `target/release`.

Correction:

- rebuild the release binary
- run real commands against that binary
- verify the documented compact behavior matches actual output

## Representative Commands Used

```bash
cargo check
cargo test
cargo build --release
./target/release/repo-index update --path /root/SwiftIndex --db /root/SwiftIndex/.swiftindex/index.db
./target/release/repo-index query --workspace /root/SwiftIndex --db /root/SwiftIndex/.swiftindex/index.db --json symbol dispatch
./target/release/repo-index query --workspace /root/SwiftIndex --db /root/SwiftIndex/.swiftindex/index.db --json --compact false symbol dispatch
./target/release/repo-index query --workspace /root/SwiftIndex --db /root/SwiftIndex/.swiftindex/index.db --json --expand 1 suggest "query dispatch"
```

## Why This Matters For Asterinas / Systems Work

Kernel and systems development often has two characteristics:

- source trees are large and structurally dense
- the next useful step is usually small and precise

That makes compact retrieval more valuable than human-browsing-oriented output. Agents perform better when the tooling:

- returns one likely symbol instead of ten
- explains confidence
- suggests the next expansion path
- avoids flooding the context window with signatures, outlines, and low-value candidates

## Outcome

The final result is not a new indexer. It is a precision improvement to an existing one:

- smaller default payloads
- better context efficiency for agents
- explicit expansion protocol
- preserved compatibility path for existing users

This is a good pattern for agent-assisted systems engineering: keep the system stable, add a thin protocol layer, and validate behavior through end-to-end commands instead of reasoning from code alone.
