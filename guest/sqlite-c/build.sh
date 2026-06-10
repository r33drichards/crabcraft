#!/usr/bin/env bash
# Build the sqlite-c reactor module. Run from anywhere:
#   nix shell nixpkgs#zig --command guest/sqlite-c/build.sh
# (or just ./build.sh if zig is already on PATH)
#
# - The schema is canonical in wit/sqlite.json; embedded via xxd -i into a
#   generated header (schema_json.h) so crab_schema can serve it verbatim.
# - Same SQLite defines as wasmcraft's wq.wasm build (tools/build-fixtures).
# - -mexec-model=reactor + -Wl,--export-memory is what makes this a crab-ABI
#   reactor: exports memory/_initialize/crab_alloc/crab_schema/crab_invoke.
# - -mno-simd128 guarantees no v128 ops (CC-side interpreters reject SIMD).
set -euo pipefail
cd "$(dirname "$0")"

if ! command -v zig >/dev/null 2>&1; then
  exec nix shell nixpkgs#zig --command "$0" "$@"
fi

export ZIG_GLOBAL_CACHE_DIR="${ZIG_GLOBAL_CACHE_DIR:-$PWD/.zig-cache}"
export ZIG_LOCAL_CACHE_DIR="${ZIG_LOCAL_CACHE_DIR:-$PWD/.zig-cache}"

# embed the resolved-WIT schema (xxd derives the symbol names from the file
# name: schema_json[] / schema_json_len)
cp ../../wit/sqlite.json schema_json
xxd -i schema_json > schema_json.h
rm schema_json

SQLITE_FLAGS="-DSQLITE_THREADSAFE=0 -DSQLITE_DEFAULT_MEMSTATUS=0 -DSQLITE_DQS=0 \
  -DSQLITE_OMIT_LOAD_EXTENSION -DSQLITE_OMIT_DEPRECATED -DSQLITE_OMIT_WAL -DSQLITE_TEMP_STORE=3"

echo "== compiling sqlite-c reactor (~15s) =="
zig cc -target wasm32-wasi -mexec-model=reactor -Oz -mno-simd128 \
  $SQLITE_FLAGS -Wl,--export-memory \
  -o ../../modules/sqlite.wasm main.c sqlite3.c

ls -la ../../modules/sqlite.wasm
