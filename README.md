# crabcraft

Wasm orchestration for ComputerCraft — more Kubernetes than Lambda.

Deploy `.wasm` workloads (compiled from **any language** that targets
wasm32-wasip1 and speaks the [crab ABI](docs/WIRE.md)) onto a cluster of
in-game computers. A **gateway** runs the control loop (registration,
placement, rescheduling, routing); **workers** are computers with disk drives
— one workload per disk, the disk is the volume; **clients** get schema-driven
typed proxies generated at runtime from WIT interfaces. Execution is
[wasmcraft](https://github.com/r33drichards/wasmcraft) (pure-Lua wasm engine):
modules are **transpiled at deploy time** and served warm.

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
| JS | `guest/hello-js` (QuickJS, SIMD-free) | `hello-js.wasm` 0.6 MB | command (JSON stdin/stdout) |
| Rust mesh | `guest/caller` | `caller.wasm` 90 KB | reactor; calls other workloads |
| Python | `guest/hello-py` | documented; RustPython passes but 26 MB > floppy | command |
| C++ | crabgen `--lang cpp` (zig c++) | scaffolded per project | reactor (typed WIT) |
| TypeScript | crabgen `--lang ts` (AssemblyScript) | scaffolded per project | reactor (typed WIT) |

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

## Tests

- `host/cmval_selftest.lua`, `host/cmval_vectors.lua` — codec round-trips +
  golden-vector parity with the Rust SDK (run on lua5.4 and Cobalt)
- `host/hello_smoke.lua`, `host/sqlite_smoke.lua` — guests on the engine (Cobalt)
- `test/e2e_sim.py` — full cluster in the CraftOS-PC simulator: gateway + 2
  workers + client; deploy/reconcile/reschedule, typed invokes, cross-module
  mesh, all language lanes, sqlite volume persistence. **9/9.**
