#!/usr/bin/env bash
# Build the full reactor module (scaffolded by crabgen; edit freely — this
# file is written once and never overwritten).
#
# -buildmode=c-shared is what makes this a REACTOR: the wasm exports
# `_initialize` (run once by the host) and has no `_start` requirement at
# invoke time. -scheduler=none / -no-debug keep the binary small.
set -euo pipefail
cd "$(dirname "$0")"

nix shell nixpkgs#tinygo --command tinygo build -o ../../modules/full.wasm \
  -target=wasip1 -buildmode=c-shared -scheduler=none -no-debug .

# SIMD tripwire: the wasmcraft engine refuses 0xfd-prefixed (SIMD) opcodes.
if nix shell nixpkgs#wabt --command wasm-objdump -d ../../modules/full.wasm | grep -q '0xfd'; then
  echo 'FATAL: SIMD opcodes in output wasm' >&2
  exit 1
fi

ls -la ../../modules/full.wasm
