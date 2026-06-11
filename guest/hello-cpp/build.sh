#!/usr/bin/env bash
# Build the hello-cpp reactor module (scaffolded by crabgen; edit freely — this
# file is written once and never overwritten).
#
# -mexec-model=reactor = a WASI REACTOR: wasi-libc provides `_initialize`
# (run once by the host) and no `_start` is needed at invoke time.
# -mno-simd128 keeps SIMD opcodes out (the wasmcraft engine refuses them);
# -fno-exceptions/-fno-rtti match the runtime's crab::Res calling convention.
# gen/*.cpp is a glob ON PURPOSE: regen adds/removes gen/mesh.cpp as the WIT
# gains/loses imports, and this scaffold-once script must keep linking the
# right set without edits.
set -euo pipefail
cd "$(dirname "$0")"

nix shell nixpkgs#zig --command zig c++ \
  -target wasm32-wasi -mexec-model=reactor -mno-simd128 \
  -fno-exceptions -fno-rtti -std=c++17 -Oz -Wl,--export-memory \
  -o ../../modules/hello-cpp.wasm \
  impl.cpp gen/*.cpp

# SIMD tripwire: the wasmcraft engine refuses 0xfd-prefixed (SIMD) opcodes.
# wasm-objdump prints opcode bytes as "fd 0c" (no 0x prefix) and SIMD
# mnemonics as v128.*/i8x16.*/... after the "|" column; anchor the mnemonic
# check to that column so a symbol name containing e.g. "i32x4" can't
# false-positive. The byte check can in rare cases match a wrapped non-SIMD
# instruction's continuation byte (loud + debuggable, accepted).
disasm="$(nix shell nixpkgs#wabt --command wasm-objdump -d ../../modules/hello-cpp.wasm)"
if grep -qE '\| +(v128|i8x16|i16x8|i32x4|i64x2|f32x4|f64x2|f16x8)\.' <<<"$disasm" ||
   grep -qE '^ *[0-9a-f]+: fd' <<<"$disasm"; then
  echo 'FATAL: SIMD opcodes in output wasm' >&2
  exit 1
fi

ls -la ../../modules/hello-cpp.wasm
