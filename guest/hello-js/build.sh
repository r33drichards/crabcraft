#!/usr/bin/env bash
# Build the hello-js command module: QuickJS compiled to SIMD-free wasm32-wasi.
#
# Why not Javy: Javy's prebuilt QuickJS uses wasm SIMD (0xfd-prefixed
# opcodes), which the pure-Lua wasmcraft engine cannot run. Instead we
# compile QuickJS from source with `zig cc -target wasm32-wasi -mno-simd128`.
#
# Wiring (no quickjs-libc/std/os needed): main.c reads all of stdin, exposes
# it to JS as the global string `__input`, JS_Evals hello-embed.js (embedded
# via xxd -i), and prints the script's completion value -- the reply JSON
# line -- to stdout. hello-embed.js mirrors hello.js's contract exactly.
#
# Engine: QuickJS 2025-09-13-2 (bellard.org, same version as nixpkgs#quickjs).
# One source patch: dtoa.c's `#include <setjmp.h>` is unused and removed
# (wasm32-wasi has no setjmp/longjmp without the EH proposal).
# -DEMSCRIPTEN is QuickJS's own knob for "wasm-ish platform": it disables
# CONFIG_ATOMICS (pthreads) and CONFIG_STACK_CHECK, both unavailable on wasi.
#
# All tools come from nix. Usage: ./build.sh
set -euo pipefail
cd "$(dirname "$0")"

QJS_VERSION="2025-09-13-2"
QJS_URL="https://bellard.org/quickjs/quickjs-${QJS_VERSION}.tar.xz"
QJS_SHA256="996c6b5018fc955ad4d06426d0e9cb713685a00c825aa5c0418bd53f7df8b0b4"
QJS_DIR="build/quickjs-2025-09-13" # tarball's top-level dir has no -2 suffix

mkdir -p build

# 1. Fetch + verify + extract QuickJS sources.
if [ ! -f "$QJS_DIR/quickjs.c" ]; then
  curl -sL -o build/quickjs.tar.xz "$QJS_URL"
  echo "$QJS_SHA256  build/quickjs.tar.xz" | shasum -a 256 -c -
  tar xf build/quickjs.tar.xz -C build
fi

# 2. Patch: dtoa.c includes setjmp.h but never uses it; wasi can't provide it.
sed -i.orig \
  's|#include <setjmp.h>|/* #include <setjmp.h> -- unused; wasm32-wasi has no sjlj (crabcraft patch) */|' \
  "$QJS_DIR/dtoa.c"

# 3. Embed the JS source for main.c.
nix shell nixpkgs#xxd --command xxd -i hello-embed.js > build/hello_embed.h

# 4. Compile. The critical flag is -mno-simd128 (wasmcraft has no SIMD);
#    -mcpu=generic keeps the rest of the feature set at the wasm MVP baseline
#    plus sign-ext / nontrapping-fptoint / memory.copy+fill, all of which
#    wasmcraft supports. Atomics and reference types stay off by default.
nix shell nixpkgs#zig --command zig cc \
  -target wasm32-wasi -Oz -mcpu=generic -mno-simd128 \
  -D_GNU_SOURCE -DEMSCRIPTEN -DCONFIG_VERSION="\"$QJS_VERSION\"" \
  -I "$QJS_DIR" -I build \
  main.c \
  "$QJS_DIR/quickjs.c" \
  "$QJS_DIR/libregexp.c" \
  "$QJS_DIR/libunicode.c" \
  "$QJS_DIR/cutils.c" \
  "$QJS_DIR/dtoa.c" \
  -Wl,--strip-all \
  -o build/hello-js.wasm

# 5. Verify: zero SIMD instructions (0xfd opcode prefix / v128 mnemonics).
nix shell nixpkgs#wabt --command wasm-objdump -d build/hello-js.wasm > build/disasm.txt
if grep -qE 'v128|i8x16|i16x8|i32x4|i64x2|f32x4|f64x2' build/disasm.txt ||
   grep -qE '^ *[0-9a-f]+: fd' build/disasm.txt; then
  echo "ERROR: SIMD instructions found in build/hello-js.wasm" >&2
  exit 1
fi

# 6. Smoke test, then install.
out=$(echo '{"fn":"greet","name":"x"}' |
  nix shell nixpkgs#wasmtime --command wasmtime run build/hello-js.wasm)
[ "$out" = '{"ok":true,"result":"Hello from JS, x!"}' ] ||
  { echo "ERROR: smoke test failed: $out" >&2; exit 1; }

cp build/hello-js.wasm ../../modules/hello-js.wasm
ls -la ../../modules/hello-js.wasm
