#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
OUT_DIR="$ROOT_DIR/artifacts/generated"
DB_PATH="$ROOT_DIR/.swiftindex/index.db"
BIN="$ROOT_DIR/target/release/repo-index"

mkdir -p "$OUT_DIR"

cd "$ROOT_DIR"

cargo test
cargo build --release

"$BIN" update --path "$ROOT_DIR" --db "$DB_PATH"

"$BIN" query --workspace "$ROOT_DIR" --db "$DB_PATH" --json symbol dispatch \
  > "$OUT_DIR/compact-symbol-dispatch.json"

"$BIN" query --workspace "$ROOT_DIR" --db "$DB_PATH" --json file symbols.rs \
  > "$OUT_DIR/compact-file-symbols.json"

"$BIN" query --workspace "$ROOT_DIR" --db "$DB_PATH" --json --expand 1 suggest "query dispatch" \
  > "$OUT_DIR/suggest-query-dispatch-expand1.json"

"$BIN" query --workspace "$ROOT_DIR" --db "$DB_PATH" --json explain "query dispatch" \
  > "$OUT_DIR/explain-query-dispatch.json"

"$BIN" query --workspace "$ROOT_DIR" --db "$DB_PATH" --json --compact false symbol dispatch \
  > "$OUT_DIR/verbose-symbol-dispatch.json"

printf 'Generated artifacts in %s\n' "$OUT_DIR"
