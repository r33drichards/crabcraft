# crabgen Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build `crabgen`, a WIT-driven scaffolder/codegen CLI so crabcraft guests in Rust/Go/C++/TypeScript(AssemblyScript) are one `.wit` + one impl file, with `new`/`regen`/`check` commands.

**Architecture:** New Rust workspace member `tools/crabgen` parses WIT via `wit-parser` into a small internal IR, then four language backends emit a per-project `gen/` (full WIRE codec runtime + WIT-derived bindings + dispatch + typed mesh stubs), a never-overwritten impl stub, build.sh, and README. Freshness is enforced by a WIT hash in `gen/MANIFEST`.

**Tech Stack:** Rust (clap, wit-parser with `serde` feature, serde_json, sha2, anyhow), TinyGo, zig c++ (wasm32-wasi, `-mno-simd128`), AssemblyScript `asc`, nix for all toolchains.

**Design doc:** `docs/plans/2026-06-10-crabgen-design.md` — read it first. Normative wire/ABI spec: `docs/WIRE.md` (section 1 = value codec, section 2 = guest ABI + `crabcraft.call` mesh import). Reference implementations: `guest/crab-sdk` (Rust codec/ABI, passes `wit/vectors.json`), `guest/hello-go/main.go` (hand-rolled Go subset — being replaced), `guest/sqlite-c/main.c` (C ABI patterns).

**Conventions for every task:** run cargo through nix (`nix-shell -p cargo rustc --run '...'` or the user's preferred nix invocation — never bare cargo). Commit after every green step. TDD: test first, watch it fail, minimal code, watch it pass.

---

## Phase 1: crate skeleton + WIT loading + IR

### Task 1.1: Workspace member `tools/crabgen`

**Files:**
- Modify: `Cargo.toml` (workspace members += `"tools/crabgen"`)
- Create: `tools/crabgen/Cargo.toml`
- Create: `tools/crabgen/src/main.rs`

```toml
# tools/crabgen/Cargo.toml
[package]
name = "crabgen"
version = "0.1.0"
edition = "2021"
description = "WIT-driven guest project generator for crabcraft"

[dependencies]
anyhow = "1"
clap = { version = "4", features = ["derive"] }
wit-parser = { version = "0.212", features = ["serde"] }
serde_json = "1"
sha2 = "0.10"

[dev-dependencies]
tempfile = "3"
```

(Pin `wit-parser` to whatever version `nix-shell -p cargo --run 'cargo add --dry-run wit-parser'` resolves; the `serde` feature must exist so `Resolve` serializes to the same JSON `wasm-tools component wit --json` produces — verify by serializing `wit/hello.wit`'s resolve and diffing against `wit/hello.json`.)

**Steps:**
1. Add the member, create the crate with a `main.rs` that just calls `clap::Parser` on a `Cli` enum with `New`/`Regen`/`Check` stubs returning `anyhow::bail!("unimplemented")`.
2. `cargo build -p crabgen` → compiles. `cargo run -p crabgen -- --help` shows three subcommands.
3. Commit: `feat(crabgen): crate skeleton with new/regen/check CLI`

### Task 1.2: WIT → IR

**Files:**
- Create: `tools/crabgen/src/ir.rs`, `tools/crabgen/src/wit.rs`
- Create: `tools/crabgen/tests/fixtures/full.wit` (exercises EVERY WIRE type: all int widths, f32/f64, char, string, list, option, record, tuple, variant, enum, result, flags, plus one imported interface for mesh stubs)
- Test: `tools/crabgen/tests/ir.rs`

IR shape (keep it this small):

```rust
pub struct Module {
    pub package: String,            // "crab:hello@0.1.0"
    pub world: String,
    pub exports: Vec<Iface>,        // v1: exactly one
    pub imports: Vec<Iface>,        // mesh stubs
    pub schema_json: String,        // serde-serialized Resolve
}
pub struct Iface { pub instance: String /* "crab:hello/greeter@0.1.0" */, pub funcs: Vec<Func>, pub types: Vec<NamedTy> }
pub struct Func { pub wit_name: String, pub params: Vec<(String, Ty)>, pub result: Option<Ty> }
pub struct NamedTy { pub wit_name: String, pub ty: Ty }
pub enum Ty { Bool, U8, U16, U32, U64, S8, S16, S32, S64, F32, F64, Char, String,
    List(Box<Ty>), Option(Box<Ty>), Tuple(Vec<Ty>),
    Record(Vec<(String, Ty)>), Variant(Vec<(String, Option<Ty>)>),
    Enum(Vec<String>), Flags(Vec<String>),
    Result(Option<Box<Ty>>, Option<Box<Ty>>),
    Named(String) /* reference to a NamedTy, resolved per-iface */ }
```

**Steps:**
1. Test first: load `fixtures/full.wit`, assert package/world/instance strings, function count, a specific record's field order, a variant's case order, and that `schema_json` parses as JSON containing `"worlds"`.
2. Implement `wit::load(path) -> anyhow::Result<Module>` with `Resolve::default()` + `push_path`; walk the single world; error with a clear message on >1 export interface, resources, streams/futures.
3. Add an error-case test: a WIT with a `resource` → `Err` containing "unsupported".
4. Schema fidelity test: load `wit/hello.wit`, `serde_json::to_value(&resolve)`, compare to `serde_json::from_str(include_str!("../../../wit/hello.json"))`. If it differs structurally, schema.json from crabgen is the new canonical form — update the assertion to round-trip instead, and note it in the commit message.
5. Commit: `feat(crabgen): WIT loader and internal IR`

### Task 1.3: MANIFEST + `check` + `regen` plumbing (language-agnostic)

**Files:**
- Create: `tools/crabgen/src/manifest.rs`, `tools/crabgen/src/project.rs`
- Test: `tools/crabgen/tests/check.rs`

MANIFEST format (3 lines, exact):

```
crabgen 0.1.0
lang go
wit-sha256 <hex of the .wit file bytes>
```

**Steps:**
1. Tests: in a tempdir, write a fake `guest/x/x.wit` + `gen/MANIFEST` with matching hash → `check` passes; mutate the WIT → `check` fails listing `guest/x` and suggesting `crabgen regen`.
2. Implement: `project::discover(repo_root)` = every `guest/*/` containing both `*.wit` and `gen/MANIFEST`; `check` recomputes; `regen` = load WIT → call backend (trait below) → rewrite `gen/` wholesale (delete dir, recreate) → write MANIFEST + `gen/schema.json`.
3. Backend trait both `new` and `regen` drive:

```rust
pub trait Backend {
    fn lang(&self) -> &'static str;
    fn generate(&self, m: &Module, dir: &Path) -> Result<()>;     // gen/ contents
    fn scaffold(&self, m: &Module, dir: &Path) -> Result<()>;     // impl stub, build.sh, README, lang files — ONLY if absent
    fn missing_impls(&self, m: &Module, dir: &Path) -> Result<Vec<String>>; // typed signatures not found in impl file (substring scan)
}
```

4. `regen` prints `missing_impls` output as "add these to impl.<ext>:" — it never edits the impl file.
5. Commit: `feat(crabgen): manifest freshness, check, regen driver`

---

## Phase 2: Go lane (biggest win, lands the patterns)

### Task 2.1: Go runtime template (the full WIRE codec, once)

**Files:**
- Create: `tools/crabgen/templates/go/runtime.go` (embedded via `include_str!`)
- Create: `tools/crabgen/templates/go/runtime_test.go` (vectors test, copied into projects too)

`runtime.go` is `package gen`, no per-WIT content. Contents, in order:
- uleb/sleb encode + decode with the bit-cap rules from `guest/hello-go/main.go` (`uleb(bits)` max-bytes + overflow check — copy that logic, it matches crab-sdk).
- Full codec: every WIRE section-1 type as `encodeX`/`decodeX` helpers over a `Decoder{buf, pos}` (f32/f64 via `math.Float32bits` LE; char = uleb scalar with surrogate/range validation; flags = `ceil(n/8)` bytes LE).
- ABI: `allocs map[uintptr][]byte` pinning, `reply []byte`, `lenbuf/replyOK/replyErr`, `//go:wasmexport crab_alloc / crab_schema / crab_invoke` where `crab_invoke` looks up `handlers[name]` (`map[string]func(*Decoder) ([]byte, error)`) — bindings populate it.
- Mesh: `//go:wasmimport crabcraft call` + `func MeshCall(workload, fn string, params []byte) ([]byte, error)` decoding the LENBUF `[status][body]` reply. Guard: only emitted/linked when the WIT has imports (TinyGo fails at instantiation on missing imports otherwise — keep mesh in a separate `mesh.go` template emitted conditionally).
- `runtime_test.go`: reads `wit/vectors.json` (path via env `CRAB_VECTORS`), for each entry builds the type from the JSON `type` descriptor, decodes `hex` → re-encodes → asserts byte-identical; values checked for scalars/strings. This runs under plain `go test` (host), no TinyGo needed.

**Steps:**
1. Write `runtime_test.go` driver first; create a scratch module under `tools/crabgen/testdata/go-runtime/` (go.mod + the two files symlinked/copied) and a cargo test `tests/go_vectors.rs` that shells out: `nix shell nixpkgs#go --command go test ./...` in that dir with `CRAB_VECTORS` set. Watch it fail (no runtime yet).
2. Port + extend the hello-go codec to full coverage. Iterate until all vectors pass.
3. Commit: `feat(crabgen): Go runtime template passes WIRE conformance vectors`

### Task 2.2: Go bindings emitter

**Files:**
- Create: `tools/crabgen/src/backend_go.rs`
- Test: `tools/crabgen/tests/golden_go.rs` + `tools/crabgen/tests/golden/go/full/` (snapshot of generated tree for `fixtures/full.wit`)

Type mapping (document in code header): records→structs, `option<T>`→`*T`, `list<T>`→`[]T`, tuple→struct with `F0..Fn`, variant→struct `{Tag int; <Case> *T ...}` with constructor funcs, enum→`type X int` + consts, flags→`type X uint64`, `result<T,E>`→decoded into Go `(T, error)` at the boundary when E=string, else generated `Result` struct. Errors from impls → status-1 reply.

Generated files in `gen/`: `runtime.go` (+`mesh.go` if imports), `bindings.go` (types, per-func `decodeArgs`/`encodeResult`, `handlers` registration in `init()`, `Impl` interface, `func SetImpl(Impl)`), `schema.go` (`//go:embed schema.json` accessor), `schema.json`, `MANIFEST`, `runtime_test.go`.

Scaffold files (once): `impl.go` (`package main`, `type App struct{}`, one method per export returning `fmt.Errorf("unimplemented: <fn>")`, `func init(){ gen.SetImpl(App{}) }`, empty `func main()`), `go.mod` (module `crabcraft.local/<name>`; `replace` not needed — gen is a subpackage), `build.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail; cd "$(dirname "$0")"
nix shell nixpkgs#tinygo --command tinygo build -o ../../modules/<name>.wasm \
  -target=wasip1 -buildmode=c-shared -scheduler=none -no-debug .
# SIMD tripwire: wasmcraft refuses 0xfd-prefixed opcodes
if nix shell nixpkgs#wabt --command wasm-objdump -d ../../modules/<name>.wasm | grep -q '0xfd'; then
  echo 'FATAL: SIMD opcodes in output' >&2; exit 1; fi
```

plus `README.md` (build/deploy/iterate: build.sh, `crb apply` manifest snippet, the regen loop).

`missing_impls`: for each export func, substring-search `impl.go` for `) <GoName>(` — absent ⇒ emit the full method signature.

**Steps:**
1. Golden test first: run backend on `fixtures/full.wit` into a tempdir, diff against `tests/golden/go/full/` (test has an `UPDATE_GOLDEN=1` regenerate mode). First run records the snapshot; review it by hand carefully once.
2. Compile test: cargo test that runs `go vet`/`go build` (host go, not tinygo — fast) on the generated full.wit project to prove it type-checks.
3. Implement emitter until both pass.
4. Commit: `feat(crabgen): Go backend — typed bindings, dispatch, mesh stubs, scaffold`

### Task 2.3: `crabgen new` end-to-end + migrate hello-go

**Steps:**
1. Wire `new`/`regen`/`check` to the Go backend. Manual smoke: `cargo run -p crabgen -- new smoke --lang go`, then `./guest/smoke/build.sh` produces `modules/smoke.wasm`; delete `guest/smoke` after.
2. Migrate: `crabgen new hello-go2 --lang go`, copy `wit/hello-go.wit` content in as the project WIT, implement `greet`/`add` in impl.go (port the 20 lines of logic from `guest/hello-go/main.go`), build, then in the sim deploy it and invoke greet/add (reuse the `hello-go` case in `test/e2e_sim.py` as the template). When green, replace `guest/hello-go` with the generated project (keep the name `hello-go`), delete the old main.go.
3. Commit: `feat(crabgen): hello-go migrated to generated bindings`

### Task 2.4: e2e — scaffold + maintenance-loop cases

**Files:**
- Create: `test/e2e_crabgen.py` (imports helpers from `test/e2e_sim.py`; needs the local craftos-mcp per the craftos2-local-sim memory; parametrized by lane, starts with `go`)

Case A (scaffold): `crabgen new e2e-<lane>` → write a 5-line impl (greet) → build.sh → deploy via sim → invoke → assert reply.

Case B (maintenance): then EDIT the project WIT — add `shout: func(msg: string) -> string` and add `loud: option<bool>` to the greet record → assert `crabgen check` exits nonzero naming the project → `crabgen regen` → assert stdout contains the `Shout` signature → append the impl method → rebuild → redeploy → invoke `greet` with the OLD encoding (no `loud` field bytes... NOTE: record fields are concatenated, so old encoding lacks the option byte — per WIT evolution rules the NEW schema decodes old payloads only if the client re-encodes against the new schema; what back-compat actually means here: invoke greet with `loud` explicitly absent (`option` none byte) and assert old behavior) → invoke `shout` → assert both.

**Steps:** write Case A, run, fix; write Case B, run, fix. Commit: `test(crabgen): e2e scaffold + maintenance loop (go)`.

---

## Phase 3: Rust lane

### Task 3.1: Rust backend

**Files:**
- Create: `tools/crabgen/src/backend_rust.rs`, `tools/crabgen/templates/rust/*`
- Test: `tools/crabgen/tests/golden_rust.rs` + golden snapshot; compile test via `cargo check` on the generated project

Layout: `src/lib.rs` (generated thin: `mod gen; mod app; crab_sdk::export_abi!(schema: gen::SCHEMA, init: gen::setup);`), `src/gen/mod.rs` (generated: native structs/enums with `TryFrom<Value>`/`Into<Value>`, a `pub trait <World>Impl` with one method per export, `setup(®istry)` registering `Type` trees and adapter closures that decode `Vec<Value>` → native, call `crate::app::App`, encode back; typed mesh wrappers over `crab_sdk::mesh_call` when imports exist — enable the `mesh` feature then), `src/app.rs` (scaffold-once: `pub struct App; impl <World>Impl for App { ...unimplemented stubs... }`), Cargo.toml with `crab-sdk = { path = "../crab-sdk" }`, crate-type cdylib. `new --lang rust` also appends the crate to the root workspace `members` (idempotent edit). `gen/schema.json` + `MANIFEST` at project root like other lanes; `SCHEMA` via `include_str!("../../gen/schema.json")`.

build.sh: `cargo build --release --target wasm32-wasip1 -p <name>` + copy `target/.../<name>.wasm` to `modules/` + the same SIMD tripwire. (Toolchain via nix; wasm32-wasip1 target availability — if plain nixpkgs rustc lacks the target, use `nix shell nixpkgs#rustup`-provisioned or fenix; resolve once and bake the working invocation into the template.)

`missing_impls`: search `src/app.rs` for `fn <rust_name>(`.

**Steps:** golden test → compile test (`cargo check` on generated full.wit project — codec correctness is already crab-sdk's tested job) → implement → migrate `guest/hello` onto generated bindings (its 50 lines become ~15 in app.rs) → run existing `test/e2e_sim.py` (hello case must stay green) → extend `test/e2e_crabgen.py` lanes list with `rust` (both cases). Commits per green step.

---

## Phase 4: C++ lane

### Task 4.1: C++ runtime template + vectors

**Files:**
- Create: `tools/crabgen/templates/cpp/crab.hpp`, `crab.cpp` (codec: `std::string`/`std::vector`/`std::optional`/`std::variant`, `Result<T>` with `std::string` error, no exceptions; ABI exports via `__attribute__((export_name("crab_alloc")))` etc.; pinning = `std::map<uintptr_t, std::vector<uint8_t>>`; mesh import `__attribute__((import_module("crabcraft"), import_name("call")))`, weak/conditional like Go)
- Create: `tools/crabgen/testdata/cpp-runtime/vectors_main.cpp` — NATIVE build (plain `zig c++`, not wasm) that loads `wit/vectors.json` (vendor a single-header JSON reader or generate the vector table into a .inc with a tiny build step in the cargo test) and round-trips; cargo test `tests/cpp_vectors.rs` shells out to build+run it.

### Task 4.2: C++ bindings emitter

Mapping: record→struct, variant→`std::variant<...>` + named alias, enum→`enum class`, option→`std::optional`, list→`std::vector`, tuple→`std::tuple`, flags→`uint64_t` + constants, string→`std::string` (validate UTF-8 in codec). Dispatch: generated `gen/bindings.cpp` declares `extern` impl functions (declared in `gen/bindings.hpp`), so a missing impl = LINK ERROR naming the symbol — that plus `missing_impls` substring scan of `impl.cpp`.

build.sh: `nix shell nixpkgs#zig --command zig c++ -target wasm32-wasi -mno-simd128 -fno-exceptions -O2 -mexec-model=reactor -o ../../modules/<name>.wasm impl.cpp gen/bindings.cpp gen/crab.cpp` + schema embedded as generated `gen/schema_json.h` (xxd-style array, like sqlite-c's `schema_json.h`) + SIMD tripwire.

**Steps:** vectors test → golden test → compile test (zig c++ to wasm succeeds on full.wit project) → e2e lanes += `cpp` (scaffold a real greet impl) → commits per step.

---

## Phase 5: TypeScript (AssemblyScript) lane

### Task 5.1: AS runtime template + vectors

**Files:**
- Create: `tools/crabgen/templates/ts/` — `assembly/gen/runtime.ts` (codec over `Uint8Array`/`DataView`; `Option<T>` box class for primitives, nullable refs otherwise; u64/s64 as AS `u64`/`i64` natively), ABI exports (`export function crab_alloc/...` — AS exports are wasm exports directly; pinning via a `Map<usize, Uint8Array>`), mesh via `@external("crabcraft", "call") declare function ...` (conditional file).
- Vectors: `assembly/gen/runtime` compiled by `asc` to wasm, driven by a Node script `tools/crabgen/testdata/ts-runtime/run_vectors.mjs` that instantiates the wasm and feeds `wit/vectors.json` hex through exported test hooks (`asc` test build exposes `__vectors_roundtrip(ptr, len)`); cargo test shells `nix shell nixpkgs#nodejs --command sh -c 'npm ci && node run_vectors.mjs'`.

Key build flags (in template `package.json` + build.sh): `asc assembly/index.ts -o ../../modules/<name>.wasm --runtime minimal --exportRuntime false --disable simd --use abort=` (resolve exact flag set during implementation; the invariants are: reactor-shaped exports, no `_start` needed by the host, MVP features only) + SIMD tripwire.

### Task 5.2: AS bindings emitter

Mapping: record→class, variant→abstract base + case subclasses with `tag`, enum→AS enum, option→`Option<T>`/nullable, result→generated `Result<T,E>` class (no exceptions across the boundary; impls return Result or use a generated `err(msg)` helper). `assembly/index.ts` generated (imports gen + `impl.ts`, registers dispatch); `assembly/impl.ts` scaffold-once with typed exported functions. `missing_impls` scans `impl.ts` for `export function <name>(`.

**Steps:** vectors → golden → compile (asc on full.wit project — expect iteration here; AS generics/nullability will fight the emitter, simplify mappings where needed and document deviations in GUESTS.md) → e2e lanes += `ts` → commits per step.

---

## Phase 6: docs + enforcement

### Task 6.1: `docs/GUESTS.md`
Anatomy, the gen/-is-generated rule, maintenance loop, type-mapping table ×4 lanes, mesh usage, troubleshooting (SIMD tripwire, TinyGo GC pinning, AS Option boxes, zig link errors = missing impls, u64>2^53 Lua caveat from WIRE.md). Link from README.md. Update `docs/guest-sdk.md` to point at crabgen as the front door.

### Task 6.2: enforcement + release
- Add `crabgen check` to whatever pre-commit/CI exists (if none: add a `test/check.sh` that runs fmt+test+`crabgen check`, mention in README).
- Run the full suite: cargo tests (golden, vectors ×3 shells), `python3 test/e2e_sim.py` (9/9 must stay green), `python3 test/e2e_crabgen.py` (4 lanes × 2 cases).
- Final commit; merge per superpowers:finishing-a-development-branch. Release per the immutable-releases memory: new version tag, never clobber assets.

---

## Verification checklist (end state)
- [ ] `crabgen new x --lang {rust,go,cpp,ts}` each → `./build.sh` → wasm in `modules/`, SIMD-free
- [ ] All four runtimes pass `wit/vectors.json`
- [ ] `crabgen check` fails on WIT edit, `regen` fixes + prints new stub signatures
- [ ] hello-go and hello migrated; old e2e_sim 9/9 green
- [ ] e2e_crabgen: scaffold + maintenance loop green ×4 lanes
- [ ] GUESTS.md written; design doc cross-linked
