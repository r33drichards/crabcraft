#!/usr/bin/env bash
# Build the hello-go reactor module (scaffolded by crabgen; edit freely — this
# file is written once and never overwritten).
#
# -buildmode=c-shared is what makes this a REACTOR: the wasm exports
# `_initialize` (run once by the host) and has no `_start` requirement at
# invoke time. -scheduler=none / -no-debug keep the binary small.
set -euo pipefail
cd "$(dirname "$0")"

nix shell nixpkgs#tinygo --command tinygo build -o ../../modules/hello-go.wasm \
  -target=wasip1 -buildmode=c-shared -scheduler=none -no-debug .

# SIMD tripwire: the wasmcraft engine refuses 0xfd-prefixed (SIMD) opcodes.
# wasm-objdump prints opcode bytes as "fd 0c" (no 0x prefix) and SIMD
# mnemonics as v128.*/i8x16.*/...; match both (same pattern as hello-js).
disasm="$(nix shell nixpkgs#wabt --command wasm-objdump -d ../../modules/hello-go.wasm)"
if grep -qE 'v128|i8x16|i16x8|i32x4|i64x2|f32x4|f64x2' <<<"$disasm" ||
   grep -qE '^ *[0-9a-f]+: fd' <<<"$disasm"; then
  echo 'FATAL: SIMD opcodes in output wasm' >&2
  exit 1
fi

ls -la ../../modules/hello-go.wasm
