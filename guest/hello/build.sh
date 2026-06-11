#!/usr/bin/env bash
# Build the hello reactor module (scaffolded by crabgen; edit freely — this
# file is written once and never overwritten).
#
# cdylib + wasm32-wasip1 = a REACTOR: the wasm exports the crab_* ABI and
# needs no `_start` at invoke time. rustup-from-nix because nixpkgs' plain
# rustc ships no wasm32-wasip1 std; if the stable toolchain lacks the
# target, run `rustup target add wasm32-wasip1` once.
set -euo pipefail
cd "$(dirname "$0")"

nix shell nixpkgs#rustup --command cargo build --release --target wasm32-wasip1 -p hello
cp ../../target/wasm32-wasip1/release/hello.wasm ../../modules/hello.wasm

# SIMD tripwire: the wasmcraft engine refuses 0xfd-prefixed (SIMD) opcodes.
# wasm-objdump prints opcode bytes as "fd 0c" (no 0x prefix) and SIMD
# mnemonics as v128.*/i8x16.*/... after the "|" column; anchor the mnemonic
# check to that column so a symbol name containing e.g. "i32x4" can't
# false-positive. The byte check can in rare cases match a wrapped non-SIMD
# instruction's continuation byte (loud + debuggable, accepted).
disasm="$(nix shell nixpkgs#wabt --command wasm-objdump -d ../../modules/hello.wasm)"
if grep -qE '\| +(v128|i8x16|i16x8|i32x4|i64x2|f32x4|f64x2|f16x8)\.' <<<"$disasm" ||
   grep -qE '^ *[0-9a-f]+: fd' <<<"$disasm"; then
  echo 'FATAL: SIMD opcodes in output wasm' >&2
  exit 1
fi

ls -la ../../modules/hello.wasm
