# Artifacts

This directory contains reproducible materials for the compact retrieval submission.

## Files

- `reproduce.sh`: rebuild, retest, refresh the local index, and regenerate sample outputs
- `generated/compact-symbol-dispatch.json`: compact high-confidence symbol lookup
- `generated/compact-file-symbols.json`: compact file search example
- `generated/suggest-query-dispatch-expand1.json`: compact suggest example with moderate expansion
- `generated/explain-query-dispatch.json`: compact explain output
- `generated/verbose-symbol-dispatch.json`: legacy verbose compatibility output

## Reproduce

Run from the repository root:

```bash
bash artifacts/reproduce.sh
```

The script writes fresh outputs under `artifacts/generated/`.
