# crabgen: WIT-driven guest project generator

> Errata (as-built): the e2e cases shipped as `test/e2e_crabgen.py` (not
> e2e_sim.py) and the deploy verb is `crb deploy` — `crb apply` below is
> historical and never existed.

Design validated 2026-06-10. Goal: guests stop hand-rolling the WIRE codec and
crab ABI. You write a `.wit` and one impl file; everything else is generated.

## Problem

Most guest code is boilerplate. hello-go is 254 lines, ~200 of them a
hand-copied uleb/string/option codec plus crab_alloc pinning, LENBUF replies,
and dispatch. The C guests repeat the same ~100+ lines. Rust is better
(crab-sdk) but still hand-mirrors WIT signatures as `Type::Record(...)`
trees. C++ and TypeScript lanes don't exist.

## The tool

`crabgen` — new Rust workspace member at `tools/crabgen`, using the official
`wit-parser` crate (same parser as wasm-tools, so the resolved-WIT JSON served
by `crab_schema` comes from the same source of truth). Run via nix + cargo.

```
crabgen new <name> --lang rust|go|cpp|ts   # scaffold guest/<name>/
crabgen regen [guest/<name> | --all]       # re-emit gen/ after a WIT edit
crabgen check                              # CI/pre-commit: all gen/ fresh?
```

## Project anatomy (identical across lanes)

```
guest/<name>/
├── <name>.wit          # source of truth — you edit this
├── README.md           # generated per-lane build/deploy/iterate instructions
├── build.sh            # generated; nix pins the toolchain
├── gen/                # regenerated wholesale, git-committed, never hand-edited
│   ├── schema.json     # resolved WIT (what crab_schema serves)
│   ├── MANIFEST        # crabgen version + WIT hash → powers `check`
│   └── <bindings...>   # codec + ABI + dispatch + typed signatures
└── impl.<ext>          # the ONLY file you write
```

The WIT lives per-project (self-contained guests), not in `wit/`. `regen`
produces `schema.json` itself, replacing the manual
`wasm-tools component wit --json` step. `check` recomputes the WIT hash
against `gen/MANIFEST` and fails loudly so stale bindings can't ship.

## Codegen architecture

`wit-parser` resolves the WIT; crabgen walks the world. **Exports** → typed
stub signatures + dispatch entries. **Imports** → typed mesh-caller stubs
over `crabcraft.call` (cross-workload calls become ordinary typed function
calls). A small internal IR (functions, params as WIRE type trees) sits
between wit-parser and the four backends so they share one traversal.

Each backend emits:

1. **Runtime** (template code, identical across projects): the FULL WIRE
   section-1 codec — uleb/sleb, all int widths, f32/f64, char, string, list,
   option, record, tuple, variant, enum, result, flags — plus the crab ABI
   (crab_alloc pinning, LENBUF, crab_schema, crab_invoke dispatch) and the
   mesh helper. Full coverage up front: any WIT just works, no
   "unsupported type" surprises.
2. **Bindings** (WIT-derived): per-function arg decoders, result encoders,
   dispatch table keyed `"<instance>#<func>"`, native type declarations for
   records/variants/enums, typed mesh-import wrappers.
3. **Stub impl** — written once by `new`, NEVER overwritten by `regen`. If
   regen finds WIT functions missing from the impl, it prints the typed
   signatures to paste in; it never edits your file.

## The four lanes

All lanes: wasm32 **reactor** (`_initialize`, no `_start`), SIMD-free, output
`modules/<name>.wasm`, toolchain via `nix shell`. Every build.sh ends with a
0xfd-SIMD-byte tripwire scan (the Javy lesson).

- **Rust**: generated bindings on top of `crab-sdk` (path dep) — reuses its
  tested codec/ABI/mesh; adds native structs/enums, `From`/`TryFrom` to
  `Value`, registry setup, typed mesh wrappers. impl.rs implements a
  generated trait. `new` adds the crate to workspace members.
- **Go**: TinyGo `-target=wasip1 -buildmode=c-shared -scheduler=none
  -no-debug` (proven hello-go recipe). `gen/` is a `gen` subpackage.
  records→structs, `option<T>`→`*T`, variants→tagged struct,
  `result<T,E>`→`(T, error)`. Alloc-pinning map (TinyGo GC is non-moving but
  collects unreferenced buffers).
- **C++**: `zig c++ -target wasm32-wasi -mno-simd128 -fno-exceptions
  -mexec-model=reactor`. `gen/crab.hpp/.cpp` over
  std::string/vector/optional/variant; generated `Result<T>`, no exceptions.
- **TypeScript = AssemblyScript** (decision: compile to wasm, NO interpreter
  embed — QuickJS lane stays separate/legacy). `asc` pinned via package.json,
  node via nix. `assembly/gen/` codec+dispatch in AS. AS can't express
  nullable primitives, so `option<primitive>` → generated `Option<T>` box
  class; reference types stay nullable. records→classes, variants→tagged
  class hierarchy. `--disable` flags keep output MVP-only.

## Testing

- **Conformance**: each generated runtime gets a test target driven by the
  existing `wit/vectors.json` (crab-sdk already passes these).
- **Golden**: emitter outputs snapshotted against fixture WITs.
- **e2e scaffold case** (per lane, in test/e2e_sim.py): `crabgen new` →
  build → deploy → invoke through the sim.
- **e2e maintenance-loop case** (per lane): scaffold+deploy, then the test
  EDITS the WIT (new function + new `option<>` field on an existing record),
  asserts `crabgen check` fails, runs `regen`, asserts stub signatures were
  reported, patches the impl, rebuilds, redeploys, invokes BOTH the old
  function (old encoding without the option still decodes — back-compat) and
  the new one. Covers staleness detection, regen idempotence, stub
  reporting, and WIT evolution in one flow.

## Maintenance loop (what the generated README teaches)

1. Edit `<name>.wit`.
2. `crabgen regen guest/<name>` — new-function signatures printed if any.
3. Fill in impl, `./build.sh`, deploy via existing `crb apply` flow.

`crabgen check` in CI/pre-commit. WIT versioning guidance (add funcs freely,
new inputs as `option<T>`, breaking = bump `@version`) embedded in the
starter WIT comments.

## Docs

- Per-project generated README: lane-specific commands.
- `docs/GUESTS.md`: shared model — anatomy, gen/-is-generated rule,
  type-mapping tables ×4 languages, mesh usage, troubleshooting (SIMD
  tripwire, TinyGo pinning, AS Option boxes).

## Rollout

Go → Rust → C++ → AssemblyScript. Existing guests stay; migrate hello-go
onto generated bindings as the proof. Each lane lands with vectors test +
both e2e cases passing.

## Out of scope (v1)

Streams/futures (WIRE bans them), resources, multi-interface worlds beyond
one export + N imports, Python lane.
