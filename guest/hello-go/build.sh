#!/usr/bin/env bash
# Build the hello-go reactor module. Run from this directory:
#   nix shell nixpkgs#tinygo --command ./build.sh
#
# -buildmode=c-shared is what makes this a REACTOR: the wasm exports
# `_initialize` (run once by the host) and has no `_start` requirement at
# invoke time. -scheduler=none / -no-debug keep the binary small.
set -euo pipefail
cd "$(dirname "$0")"

# The schema is canonical in wit/; go:embed can't reach parent dirs, so copy.
cp ../../wit/hello-go.json schema.json

tinygo build -o ../../modules/hello-go.wasm \
  -target=wasip1 -buildmode=c-shared \
  -scheduler=none -no-debug \
  .

ls -la ../../modules/hello-go.wasm
