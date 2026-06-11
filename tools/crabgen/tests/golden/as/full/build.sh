#!/usr/bin/env bash
# Build the full reactor module (scaffolded by crabgen; edit freely — this
# file is written once and never overwritten).
#
# The module is an AssemblyScript REACTOR: the top-level register() call in
# assembly/index.ts compiles into the module start function, which
# --exportStart turns into the exported `_initialize` (run once by the host)
# instead of a wasm start section. --use abort= removes the env.abort import
# (abort() traps); --runtime incremental is the full AS GC (the runtime
# template pins host-visible buffers). asc is pinned by package.json +
# package-lock.json; npm ci restores it when node_modules is missing or
# carries a different version.
set -euo pipefail
cd "$(dirname "$0")"

if ! grep -qs '"version": "0.28.18"' node_modules/assemblyscript/package.json; then
  nix shell nixpkgs#nodejs --command npm ci --no-audit --no-fund
fi

# SIMD note: asc disables the wasm simd feature by default (0.28 has no
# `--disable simd`, only an opt-in `--enable simd` — never add it); the
# tripwire below proves the artifact stays SIMD-free.
nix shell nixpkgs#nodejs --command npx asc assembly/index.ts \
  -o ../../modules/full.wasm \
  --exportStart _initialize --use abort= --runtime incremental \
  --optimizeLevel 3 --shrinkLevel 1

# SIMD tripwire: the wasmcraft engine refuses 0xfd-prefixed (SIMD) opcodes.
# wasm-objdump prints opcode bytes as "fd 0c" (no 0x prefix) and SIMD
# mnemonics as v128.*/i8x16.*/... after the "|" column; anchor the mnemonic
# check to that column so a symbol name containing e.g. "i32x4" can't
# false-positive. The byte check can in rare cases match a wrapped non-SIMD
# instruction's continuation byte (loud + debuggable, accepted).
disasm="$(nix shell nixpkgs#wabt --command wasm-objdump -d ../../modules/full.wasm)"
if grep -qE '\| +(v128|i8x16|i16x8|i32x4|i64x2|f32x4|f64x2|f16x8)\.' <<<"$disasm" ||
   grep -qE '^ *[0-9a-f]+: fd' <<<"$disasm"; then
  echo 'FATAL: SIMD opcodes in output wasm' >&2
  exit 1
fi

ls -la ../../modules/full.wasm
