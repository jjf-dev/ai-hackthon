# Lessons Learned

## Most Transferable Lessons

### 1. Give The Agent A Narrow Architectural Boundary

The most effective instruction in this task was not stylistic. It was architectural:

- do not redesign the system
- do not touch storage
- keep reranking intact
- add compaction after reranking

For systems work, that kind of boundary is more valuable than verbose prompting.

### 2. Force Small Compilable Steps

Requiring each step to compile prevented the session from turning into a large speculative rewrite. This is especially important in Rust, where type-driven breakage is immediate and useful.

### 3. Default To Compact Machine-Oriented Output

Agents do not need long result lists by default. They need:

- a small candidate set
- a confidence signal
- an explicit next-step hint

This should shape tooling design, not just prompt design.

### 4. Preserve A Compatibility Escape Hatch

`--compact false` mattered. It reduced migration risk and made it easy to compare old versus new behavior during validation.

### 5. Validate With Real Commands, Not Just Tests

Unit tests and integration tests were necessary but not sufficient. The key extra step was running the release binary against a real local index and inspecting actual compact JSON payloads.

## Failures Worth Remembering

- Helper commands documented in repo instructions may not actually exist in the environment.
- Docs can drift from the release binary if the binary is not rebuilt.
- Query examples can fail for trivial reasons if the local index does not contain the expected terms.

## Recommendation For Future Agent Workflows

When building agent-facing developer tools for Asterinas or similar systems projects:

1. Design the output around the next agent action, not around human browsing.
2. Make expansion explicit and cheap.
3. Keep one compatibility mode for debugging and migration.
4. Update the related agent skill or playbook immediately after changing the tool contract.
