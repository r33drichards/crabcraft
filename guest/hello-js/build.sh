#!/usr/bin/env bash
# Build the hello-js command module with Javy.
#
# javy acquisition (nixpkgs has no javy as of 2026-06): official release
# binary, sha256-verified:
#   curl -sL -o javy.gz https://github.com/bytecodealliance/javy/releases/download/v8.1.1/javy-arm-macos-v8.1.1.gz
#   gunzip javy.gz && chmod +x javy
#
# Usage: JAVY=/path/to/javy ./build.sh   (defaults to `javy` on PATH)
set -euo pipefail
cd "$(dirname "$0")"

"${JAVY:-javy}" build hello.js -o ../../modules/hello-js.wasm

# Functional check (command module: _start, JSON line on stdin -> stdout):
#   echo '{"fn":"greet","name":"x"}' | \
#     nix shell nixpkgs#wasmtime --command wasmtime run ../../modules/hello-js.wasm
ls -la ../../modules/hello-js.wasm
