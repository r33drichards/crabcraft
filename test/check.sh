#!/usr/bin/env bash
# test/check.sh — the repo's fmt/clippy/test/freshness gate.
#
# Run from anywhere; it cd's to the repo root and fetches its own toolchain
# via nix-shell (same pattern as the guest build.sh scripts, which invoke
# nix themselves). Fails on the first red step.
#
# Steps:
#   1. cargo fmt --check, scoped to -p crabgen only: the guest crates'
#      generated code (guest/hello/src/gen etc.) is emitted by crabgen's
#      templates and is NOT rustfmt-stable — formatting it would dirty
#      MANIFEST-tracked files. crabgen's own sources are the fmt surface.
#   2. cargo clippy -p crabgen --all-targets -- -D warnings
#   3. cargo test --workspace          (110+ tests incl. golden + compile tests)
#   4. cargo run -p crabgen -- check   (generated-code freshness: gen/ trees
#                                       match the committed WIT + MANIFEST)
set -euo pipefail
cd "$(dirname "$0")/.."

step() { echo; echo "==> $*"; }

step "cargo fmt --check (-p crabgen; guest gen/ code is not rustfmt-stable)"
nix-shell -p cargo rustc rustfmt --run 'cargo fmt -p crabgen -- --check'

step "cargo clippy -p crabgen --all-targets -- -D warnings"
nix-shell -p cargo rustc clippy --run 'cargo clippy -p crabgen --all-targets -- -D warnings'

step "cargo test --workspace"
nix-shell -p cargo rustc --run 'cargo test --workspace'

step "crabgen check (generated-code freshness)"
nix-shell -p cargo rustc --run 'cargo run -q -p crabgen -- check'

echo
echo "check.sh: all green"
