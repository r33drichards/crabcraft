# crabcraft

Wasm orchestration for ComputerCraft — more Kubernetes than Lambda.

Deploy `.wasm` workloads (compiled from **any language** that targets
wasm32-wasip1 and speaks the [crab ABI](docs/WIRE.md)) onto a cluster of
in-game computers. A **gateway** runs the control loop (registration,
placement, rescheduling, routing, jobs + cron); **workers** are computers with
disk drives — one workload per disk, the disk is the volume; **clients** get
schema-driven typed proxies generated at runtime from WIT interfaces.
Execution is [wasmcraft](https://github.com/r33drichards/wasmcraft) (pure-Lua
wasm engine): modules are **transpiled at deploy time** and served warm.

Interfaces are defined in [WIT](https://component-model.bytecodealliance.org/design/wit.html);
the wire format implements a subset of the
[wRPC](https://github.com/bytecodealliance/wrpc/blob/main/SPEC.md) model
(component-model value encoding, synchronous root frames) over rednet.
Workloads can call each other by name through the mesh (`crabcraft.call`).

## Worked language lanes (all e2e-tested on a simulated cluster)

| Lane | Source | Artifact | Kind |
|---|---|---|---|
| Rust | `guest/hello` (+ `guest/crab-sdk`) | `hello.wasm` 88 KB | reactor (typed WIT) |
| Go | `guest/hello-go` (TinyGo) | `hello-go.wasm` 97 KB | reactor (typed WIT) |
| C + SQLite | `guest/sqlite-c` | `sqlite.wasm` 740 KB | reactor; db persists on the volume |
| Rust mesh | `guest/caller` | `caller.wasm` 90 KB | reactor; calls other workloads |
| C++ | `guest/hello-cpp` (zig c++) | `hello-cpp.wasm` 43 KB | reactor (typed WIT) |
| TypeScript | `guest/hello-ts` (AssemblyScript) | `hello-ts.wasm` 19 KB | reactor (typed WIT) |

**Write your own guest:** [docs/GUESTS.md](docs/GUESTS.md) — `crabgen`
scaffolds a complete guest from a WIT file in Rust, Go, C++, or TypeScript
(`cargo run -p crabgen -- new my-mod --lang go`); you write one impl file.

## In-game quickstart

Gateway (computer + modem):
```
wget https://github.com/r33drichards/crabcraft/releases/latest/download/gateway.lua gateway
gateway --install
gateway
```

Workers (computer + modem + disk drive with a floppy; repeat per node):
```
wget https://github.com/r33drichards/crabcraft/releases/latest/download/worker.lua worker
worker --install
worker
```
The worker fetches the wasmcraft engine on first run, registers its disks as
slots, and recovers workloads from disk after reboots.

Client (pocket computer with wireless modem, or any computer):
```
wget https://github.com/r33drichards/crabcraft/releases/latest/download/crb.lua crb
```

Deploy SQLite from a manifest:
```yaml
# sqlite.yml
name: sqlite
wasm: https://github.com/r33drichards/crabcraft/releases/latest/download/sqlite.wasm
kind: reactor
schema: https://github.com/r33drichards/crabcraft/releases/latest/download/sqlite.json
```
```
crb deploy sqlite.yml
crb ls
crb invoke sqlite exec CREATE TABLE pets(name,kind)
crb invoke sqlite exec INSERT INTO pets VALUES('ferris','crab')
crb invoke sqlite exec SELECT * FROM pets
```
The database file lives on whichever worker's floppy the scheduler picked —
survives reboots, travels with the disk.

## Jobs and cron

Services aren't the only shape: `kind: job` is run-to-completion (the k8s
**Jobs** resource), and `schedule:` makes it a **CronJob**. A run = place on a
free slot, execute once (a typed `func:` call on a reactor module, or a
command module with `body:` on stdin), record the result, free the slot:

```yaml
# greet-job.yml
name: greet-job
wasm: https://github.com/r33drichards/crabcraft/releases/latest/download/hello.wasm
kind: job
schema: https://github.com/r33drichards/crabcraft/releases/latest/download/hello.json
func: greet
args:
  name: crabcraft
schedule: "@every 1m"     # vixie cron ("*/5 * * * *") or @every; UTC; omit = run once now
keep-warm: true           # hold the slot between runs (skip re-transpiles)
```
```
crb deploy greet-job.yml
crb run greet-job          -- queue a run now (scheduled or not)
crb logs greet-job         -- recent runs: status, timing, decoded output
```
Retries (`retries:`), per-run timeouts (`timeout:`), skip-if-still-running
concurrency, and persisted run history are documented in
[WIRE.md section 6](docs/WIRE.md#6-jobs-and-cron-run-to-completion).

## Tests

- `test/check.sh` — the dev gate: rustfmt (`-p crabgen` only; guest `gen/`
  code is generated and not rustfmt-stable), clippy `-D warnings`,
  `cargo test --workspace`, and `crabgen check` (generated-code freshness).
  Run it before committing; it fetches its own toolchain via nix-shell.
- `host/cmval_selftest.lua`, `host/cmval_vectors.lua` — codec round-trips +
  golden-vector parity with the Rust SDK (run on lua5.4 and Cobalt)
- `host/cron_selftest.lua` — cron schedule parser/matcher, synthetic times
  (lua5.4 and Cobalt)
- `test/gateway_jobs_test.lua`, `test/crb_jobs_test.lua` — the job/cron state
  machine driven through the REAL gateway and crb chunks on plain lua5.4: a
  fake CC world with a warpable clock plays worker + client (placement,
  retries, timeouts, schedules, keep-warm, reboot recovery; manifest
  validation and arg encoding).
- `host/hello_smoke.lua`, `host/sqlite_smoke.lua` — guests on the engine (Cobalt)
- `test/e2e_sim.py` — full cluster in the CraftOS-PC simulator: gateway + 2
  workers + client; deploy/reconcile/reschedule, typed invokes, cross-module
  mesh, all language lanes, sqlite volume persistence, jobs + cron. **13/13.**
- `python3 test/e2e_crabgen.py go rust cpp ts` — the full crabgen developer
  workflow per lane (scaffold, build, deploy, invoke; then WIT evolution with
  check/regen) in the same simulator. **40/40** (~26 min).
