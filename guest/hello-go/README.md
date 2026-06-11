# hello-go — crabcraft guest (Go lane)

Scaffolded by crabgen. `hello-go.wit` is the source of truth; `gen/` is
GENERATED — never edit it, crabgen rewrites it wholesale on every regen.
Your code lives in `impl.go` (crabgen never touches it).

## Build

    ./build.sh

TinyGo (via nix) builds a wasip1 reactor at `../../modules/hello-go.wasm`,
then the script fails hard if any SIMD (0xfd) opcodes snuck in — the
wasmcraft engine refuses them. Host-side unit tests run with plain go:
`nix shell nixpkgs#go --command go test ./...`.

## Deploy

Add a manifest and apply it in-game with `crb apply` (exported interface:
`crab:hello-go/api@0.1.0`):

```yaml
name: hello-go
wasm: <URL serving modules/hello-go.wasm>
kind: reactor
schema: <URL serving the resolved-WIT JSON>   # serve a copy of gen/schema.json
```

## Maintenance loop

1. Edit `hello-go.wit`.
2. `crabgen regen guest/hello-go` — rewrites `gen/` and prints typed
   signatures for any functions missing from `impl.go`.
3. Paste the stubs into `impl.go` and implement them.
4. `./build.sh`, redeploy, invoke.

`crabgen check` (run it in CI/pre-commit) fails while `gen/` is stale.

## WIT versioning

Backwards-compatible evolution: add new functions freely, and add new
inputs as `option<T>` (old callers simply omit them). Anything else is a
breaking change: bump the package version (`@0.1.0` → `@0.2.0`) and the
daemon serves both versions side by side.
