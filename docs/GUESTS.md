# Writing guests with crabgen

crabgen scaffolds a complete crabcraft guest from a WIT file in any of four
language lanes — **Rust, Go (TinyGo), C++ (zig), TypeScript (AssemblyScript)**.
You edit the WIT and one impl file; the WIRE codec, the crab ABI, dispatch,
native type declarations, and mesh wrappers are generated. This doc is the shared model
across lanes; each scaffolded project gets its own README with lane-specific
build/deploy details, and [WIRE.md](WIRE.md) is the underlying protocol.

All lanes produce a wasm32-wasi(p1) **reactor** (`_initialize`, no `_start`),
SIMD-free, written to `modules/<name>.wasm`. Toolchains come from nix —
nothing to install.

## Quick start

From the repo root (crabgen finds the root by walking up to the first dir
with both `guest/` and `Cargo.toml`):

```console
$ nix-shell -p cargo rustc --run 'cargo run -p crabgen -- new my-mod --lang go'
created guest/my-mod (go)
```

(Below, `crabgen …` abbreviates that nix-shell + `cargo run -p crabgen --`
invocation; `cargo build -p crabgen` once and use `./target/debug/crabgen`
if you prefer.)

`--lang` is one of `rust`, `go`, `cpp`, `ts`. The project name must be a WIT
identifier (lowercase kebab-case). The scaffold includes a starter WIT
(`guest/my-mod/my-mod.wit`) exporting one `greet` function — replace it with
your interface, then:

```console
$ crabgen regen guest/my-mod
regenerated guest/my-mod/gen (go)
add these to impl.go:
  func (App) Shout(msg_ string) (string, error)
```

Paste the printed stubs into the impl file, implement them, build:

```console
$ ./guest/my-mod/build.sh        # → modules/my-mod.wasm + SIMD tripwire
```

Deploy with a manifest ([WIRE.md section 4](WIRE.md)) via the in-game `crb`
client:

```yaml
# my-mod.yml
name: my-mod
wasm: <URL serving modules/my-mod.wasm>
kind: reactor
schema: <URL serving the resolved-WIT JSON>   # serve a copy of gen/schema.json
```

```console
crb deploy my-mod.yml
crb invoke my-mod greet name=steve
```

## Project anatomy

The shared model — one WIT, one impl file, everything else generated:

```
guest/<name>/
├── <name>.wit          # source of truth — you edit this
├── README.md           # regenerated per-lane build/deploy/iterate instructions
├── build.sh            # scaffold-once; nix-pinned toolchain + SIMD tripwire
├── gen/                # regenerated wholesale, git-committed, never hand-edited
│   ├── MANIFEST        # crabgen version + WIT hash → powers `crabgen check`
│   ├── schema.json     # resolved WIT (what crab_schema serves)
│   └── <bindings...>   # codec + ABI + dispatch + typed signatures (go/cpp)
└── impl.<ext>          # the file you write (go/cpp: at the project root)
```

Where each lane deviates:

| Lane | Your code (scaffold-once) | Regenerated on every `regen` | Scaffold-once extras |
|---|---|---|---|
| go   | `impl.go` | `gen/`, `README.md` | `go.mod`, `build.sh` |
| rust | `src/app.rs` | `gen/` (MANIFEST + schema.json), `src/gen/`, `src/lib.rs`, `README.md` | `Cargo.toml`, `build.sh` (+ a root workspace-members entry, added by `new`) |
| cpp  | `impl.cpp` | `gen/`, `README.md` | `build.sh` |
| ts   | `assembly/impl.ts` | `gen/` (MANIFEST + schema.json), `assembly/gen/`, `assembly/index.ts`, `README.md` | `package.json`, `package-lock.json`, `build.sh` |

The rule: **everything in the "regenerated" column is crabgen-owned** —
committed to git, never hand-edited, rewritten wholesale by `regen` (this is
how `assembly/gen/mesh.ts` disappears when a WIT loses its imports). The
Rust bindings live in `src/gen/` (not `gen/`) because cargo compiles from
`src/`; same reason the AS bindings live in `assembly/gen/` (asc compiles
from `assembly/`). Impl files are written ONCE by `new` and never touched
again: if `regen` finds WIT functions missing from the impl, it prints the
typed signatures to paste in — it never edits your file.

## The maintenance loop

```
edit <name>.wit  →  crabgen check FAILS  →  crabgen regen guest/<name>
                →  paste printed stubs into the impl  →  ./build.sh
                →  redeploy (crb deploy)  →  invoke
```

`crabgen check` is the freshness gate — run it in CI / pre-commit. It
recomputes each project's WIT hash against `gen/MANIFEST` and fails loudly
on staleness, repo-wide:

```console
$ crabgen check
stale: guest/my-mod (WIT changed since gen/ was written)
  run: crabgen regen guest/my-mod
$ echo $?
1
```

`crabgen regen --all` regenerates every crabgen-managed project (a project
is any `guest/<dir>/` with both a single `.wit` and `gen/MANIFEST`;
hand-written guests are ignored). Regen is idempotent — same WIT in, same
bytes out.

`test/e2e_crabgen.py` runs this exact loop end-to-end per lane against the
local simulator (scaffold → build → deploy → invoke, then WIT edit → stale
check → regen → stub → rebuild → redeploy → both old and new functions).

### WIT evolution rules

- **Add functions freely** — backwards compatible.
- **New inputs as `option<T>`** — old callers simply omit them; the typed
  client re-encodes against the new schema with an option-none byte, and
  old encodings still decode.
- **Anything else is breaking** — bump the package version
  (`@0.1.0` → `@0.2.0`); the daemon serves both versions side by side.

## Type mapping

How each WIT type lands in your impl signatures (from the backend headers in
`tools/crabgen/src/backend_*.rs`, snapshot-tested in
`tools/crabgen/tests/golden/*/full/`):

| WIT | Rust | Go | C++ | AssemblyScript |
|---|---|---|---|---|
| `bool` | `bool` | `bool` | `bool` | `bool` |
| `u8 u16 u32 u64` | `u8 u16 u32 u64` | `uint8…uint64` | `uint8_t…uint64_t` | `u8 u16 u32 u64` |
| `s8 s16 s32 s64` | `i8 i16 i32 i64` | `int8…int64` | `int8_t…int64_t` | `i8 i16 i32 i64` |
| `f32 f64` | `f32 f64` | `float32 float64` | `float double` | `f32 f64` |
| `char` | `char` | `rune` | `uint32_t` | `u32` |
| `string` | `String` | `string` | `std::string` (UTF-8) | `string` (UTF-16 in memory) |
| `list<T>` | `Vec<T>` | `[]T` | `std::vector<T>` | `Array<T>` |
| `option<T>` | `Option<T>` | `*T` (nil = none) | `std::optional<T>` | `T \| null` for reference types; generated box class (`OptionBool` etc.) for value types and nested options |
| `tuple<A, B, …>` | `(A, B, …)` | anonymous `struct{ F0 A; F1 B; … }` | `std::tuple<A, B, …>` | generated class `Tuple2F32F32` etc., fields `f0…fN-1` |
| `record` | struct, `pub` snake_case fields | struct, exported PascalCase fields | struct, snake_case fields, `{}`-init | class, camelCase fields, all initialized |
| `variant` | enum with payload tuple variants | `struct{ Tag int; … }` + tag consts + `New<Variant><Case>` constructors | `std::variant` alias; payload cases are `<Variant><Case>{T value;}` structs, empty cases `std::monostate` | class with `tag: i32` + payload fields + `TAG_*` consts + `new<Case>` factories |
| `enum` | fieldless enum | `type X int` + consts | `enum class X : uint32_t` | i32-backed `enum` |
| `flags` | u64 newtype + SCREAMING bit consts | `type X uint64` + `1<<i` consts | `struct { uint64_t bits; }` + `static constexpr` consts | `class { bits: u64 }` + SCREAMING consts |
| `result<T, E>` (value position) | `Result<T, E>` | generic `Result[T, E]` | `gen::Result<T, E>` | generated `Result<TokT><TokE>` class (`isErr` selects the side) |

(All lanes cap `flags` at 64 members; `char` is a validated unicode scalar
on the wire; a `result` in function-return position is special — next
section.)

Name casing is mechanical everywhere: WIT kebab-case → PascalCase by
capitalizing each dash segment (`echo-everything` → `EchoEverything`,
`a-u8` → `AU8` — no acronym table), and functions/params/fields follow the
lane's convention (Rust/C++ snake_case, Go exported PascalCase +
lowerCamel params, AS camelCase). Names that would hit a language keyword
or a generated local get a trailing underscore (`type` → `type_`); a WIT
name that collides with a generated/runtime identifier fails `regen`
loudly instead of emitting broken code.

## Errors and results

One classification rule across all four lanes (the "Ret" rule). When your
impl function reports an error, what happens depends on the WIT return type:

| WIT return type | An impl error becomes… |
|---|---|
| none, or a plain value `T` | a **status-1 reply** carrying the message — a function-level failure ([WIRE.md section 2](WIRE.md)) |
| `result<T, string>` (or `result` with absent err payload) | the **WIRE result err case** — a normal status-0 reply whose payload is `err(message)`; typed clients see it as a domain error |
| `result<T, E>` for any other `E` | you return the result *value* yourself; the separate error channel still means **status 1** |

The distinction matters because status-1 is "the call failed" (transport-ish,
no typed payload) while the result err case is part of your interface — so
`result<T, string>` returns get the ergonomic mapping: your lane's native
error channel feeds the err case directly.

Per-lane mechanism for that error channel:

| Lane | Impl signature shape | Error = |
|---|---|---|
| Go | `func (App) F(…) (T, error)` (or just `error`) | non-nil `error` |
| Rust | trait method `fn f(&self, …) -> Result<T, String>` | `Err(msg)` |
| C++ | `crab::Res<V> f(…)` in `namespace impl` | non-empty `.err` (no exceptions; built `-fno-exceptions`) |
| AS | `function f(…): ResT` (monomorphic `Res*` classes, `ResVoid` for no value) | `ResT.fail(msg)` vs `ResT.ok(v)` (no exceptions, no closures) |

Decode failures of incoming params are handled before your code runs: a
status-1 reply with `bad params:` context (go/cpp/ts generated handlers; in
Rust crab-sdk's registry does the decoding).

## Mesh: calling other workloads

`import`ed interfaces in your world become typed wrapper functions — a
cross-workload call ([WIRE.md, `crabcraft.call`](WIRE.md)) looks like an
ordinary function call. Every wrapper takes the **target workload name as
its first argument** (crabgen never bakes one in; placement is the host's
problem) and its error covers transport failures, remote status-1 failures,
AND the WIT result err case:

```go
// Go: gen/imports.go
err := gen.TelemetryReport("telemetry-prod", sample)
```
```rust
// Rust: src/gen/mod.rs (needs crab-sdk's "mesh" feature — regen reminds you)
gen::telemetry_report("telemetry-prod", sample)?;
```
```cpp
// C++: gen/bindings.hpp
crab::Res<std::monostate> r = gen::telemetry_report("telemetry-prod", sample);
```
```ts
// AS: assembly/gen/bindings.ts
const r = telemetryReport("telemetry-prod", sample);
```

A module that declares the `crabcraft.call` import requires the host to
provide it, so **import-free worlds stay import-free**: every lane emits its
mesh runtime (`mesh.go`/`mesh.{hpp,cpp}`/`mesh.ts`; Rust gates the
`crab-sdk` "mesh" feature) only when the WIT actually has imports, and
`regen` removes it again when imports go away. In the AS lane, asc
additionally tree-shakes: the `@external` declaration only lands in the
wasm import section once an impl actually calls a wrapper.

## Troubleshooting

| Symptom | What it means |
|---|---|
| `FATAL: SIMD opcodes in output wasm` from build.sh | The tripwire at the end of every build.sh found 0xfd (SIMD) opcodes — the wasmcraft engine refuses them, so the build fails here instead of at deploy. The scaffolded flags are SIMD-free (`-mno-simd128` for C++; asc has SIMD off by default — never add `--enable simd`); a changed flag or a SIMD-using dependency snuck it in. Remove it. |
| C++ first build takes minutes | zig builds libc++ for wasm32-wasi once per zig cache; subsequent builds are fast. Don't run several first builds in parallel — they race on the cache. |
| C++ link error `undefined symbol: impl::…` | Intentional enforcement: `gen/bindings.hpp` declares your `namespace impl` functions and the generated dispatch calls them, so a missing definition in `impl.cpp` fails the link naming the symbol. `crabgen regen` prints the missing signatures. |
| Go: corrupted buffers / crashes after GC | TinyGo's GC is non-moving but collects unreferenced buffers. The generated runtime pins every `crab_alloc` buffer (and the live reply) in a map and unpins per invoke — don't hand-roll allocation, and keep host-visible data reachable. Same pinning scheme in the AS runtime. |
| AS: `option<u32>` isn't `u32 \| null` | AS can't express nullable value types, so options over numbers/bool/char/enum (and nested options) are generated box classes: `null` = none, `new OptionU32(v)` = some. Reference types (string, arrays, classes) stay plain nullable. |
| AS: closures don't compile | AS has no closure state — handlers and everything the bindings call are plain function references. Write top-level functions in `impl.ts`. |
| AS: `invalid utf-8 in string` on encode | AS strings are UTF-16 and may contain lone surrogates, which have no UTF-8 form; the encoder rejects them rather than emit WTF-8. |
| ts first build hits the network | `npm ci` restores the pinned toolchain (assemblyscript **0.28.18**, via package.json + package-lock.json) into `node_modules/` when missing. After that, builds are offline. |
| Rust: `wasm32-wasip1` target missing | build.sh uses rustup-from-nix (nixpkgs' plain rustc ships no wasip1 std); if the stable toolchain lacks the target, `rustup target add wasm32-wasip1` once. |
| u64/s64 values look wrong on the client | Lua hosts lose precision beyond 2^53 ([WIRE.md](WIRE.md) caveat). Guests are exact (native 64-bit in all four lanes); the loss is client-side. |
| `crabgen check` fails on a project you didn't touch | check is repo-wide on purpose — every crabgen-managed project must be fresh. Run the printed `crabgen regen guest/<name>`. |

All toolchains are nix-pinned by the scaffolded build.sh (`nix shell
nixpkgs#tinygo` / `#zig` / `#rustup` / `#nodejs` / `#wabt`); the only
non-nix pin is asc, locked by the committed package-lock.json.

## Where to look next

| For | See |
|---|---|
| Lane-specific build/deploy/iterate details | `guest/<name>/README.md` (generated per project) |
| The wire protocol, ABI, manifests, orchestration | [WIRE.md](WIRE.md) |
| What crab-sdk does under the hood (Rust lane) | [guest-sdk.md](guest-sdk.md) |
| CLI usage | `cargo run -p crabgen -- --help` |
| The design and rationale | [plans/2026-06-10-crabgen-design.md](plans/2026-06-10-crabgen-design.md) |
| The canonical end-to-end walkthrough (as code) | `test/e2e_crabgen.py` |
