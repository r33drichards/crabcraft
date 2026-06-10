#!/usr/bin/env bash
# Build pipeline for the hello-py command-kind workload (WIRE.md section 3):
# a wasi python interpreter running hello.py with the request JSON on stdin
# and the reply JSON on stdout (same contract as guest/hello-js).
#
# STATUS: documented, binary NOT shipped — every workable wasi python
# interpreter blows the ~10MB floppy budget. Sizes measured 2026-06-09:
#
#   interpreter                                   .wasm size   verdict
#   ------------------------------------------------------------------
#   RustPython 0.4.0 (freeze-stdlib, this script)   30.2 MB    works; too big
#   ... after wasm-opt -Oz                          26.5 MB    still too big
#   CPython 3.12 (vmware-labs wlr, wasi-sdk 20)    ~25 MB      too big
#   CPython 3.14.5 (brettcannon wasi_sdk-24 zip)    14.2 MB zip too big
#   micropython (wapm/wasmer registries)            gone       CDN dead (HTTP 526)
#   micropython (nixpkgs)                           native     no wasi build
#   nixpkgs python*-wasi                            absent     —
#
# The RustPython pipeline below WAS built and functionally verified:
#
#   echo '{"fn":"greet","name":"Crab","excited":true}' | \
#     wasmtime run --dir .::/app rustpython.wasm /app/hello.py
#   -> {"ok": true, "result": "Hello from Python, Crab!!!"}
#
# (all four contract cases pass: greet, greet excited, add, unknown fn).
# If the floppy budget ever grows, this is the lane to revive. NOTE:
# RustPython master needs rustc >= 1.95; tag 0.4.0 builds on stable 1.91.
set -euo pipefail
cd "$(dirname "$0")"

echo "NOTE: produces a ~30MB module - exceeds the 10MB floppy cap." >&2
echo "Deliberately not wired into modules/; see header." >&2

git clone --depth 1 --branch 0.4.0 https://github.com/RustPython/RustPython /tmp/rustpython
cd /tmp/rustpython
nix shell nixpkgs#rustup --command \
  cargo build --release --target wasm32-wasip1 --no-default-features --features freeze-stdlib

ls -la target/wasm32-wasip1/release/rustpython.wasm
