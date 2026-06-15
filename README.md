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

## Card kiosk (a React monitor gated on cardlock auth)

`cardui` is a kiosk station whose CC **monitor** renders a real React
sign-in/sign-up UI (the [wasmcraft web engine](https://github.com/r33drichards/wasmcraft):
QuickJS + react-reconciler on a monitor) gated on the `auth` workload. A floppy
is the "card" — it carries an Ed25519 **private** key; `auth` stores only the
public key, and login is a challenge/response signed locally, so the key never
crosses the network.

Station hardware: one computer with a **wireless modem**, a **disk drive**, a
**monitor**, and redstone on the `back` side to a door. With a gateway + worker
running and `sqlite` + `auth` deployed:

```
crb deploy manifests/sqlite.yml      # holds the users table
crb deploy manifests/auth.yml        # register / sign / verify

wget https://github.com/r33drichards/crabcraft/releases/latest/download/cardui.lua cardui
cardui init     # create the users table (once)
cardui          # kiosk mode: 🔒 LOCKED on the monitor
```

Tap **Sign up** (type a username at the keyboard, insert a blank floppy → the
card is written and its `user_id` shown), then tap **Sign in** and tap the card
on the drive → **Welcome** + the door pulses. `cardui` fetches the web engine
from the wasmcraft release and the React bundle + `auth.wasm` signer from the
crabcraft release on first run. Bridge + design:
[docs/plans/2026-06-14-cardui-react-gate-design.md](docs/plans/2026-06-14-cardui-react-gate-design.md).

## Tests

- `test/check.sh` — the dev gate: rustfmt (`-p crabgen` only; guest `gen/`
  code is generated and not rustfmt-stable), clippy `-D warnings`,
  `cargo test --workspace`, and `crabgen check` (generated-code freshness).
  Run it before committing; it fetches its own toolchain via nix-shell.
- `host/cmval_selftest.lua`, `host/cmval_vectors.lua` — codec round-trips +
  golden-vector parity with the Rust SDK (run on lua5.4 and Cobalt)
- `host/hello_smoke.lua`, `host/sqlite_smoke.lua` — guests on the engine (Cobalt)
- `host/auth_smoke.lua` — `auth.wasm` register→sign→verify on the engine (Cobalt)
- `host/cardui_smoke.lua` — the cardui bridge (tap→command→`web_message`→frame)
  on the real web engine with a fake mesh: sign-in grant/deny, sign-up enroll.
  **11/11.** Needs a `web.wasm` built with `web_message` (the wasmcraft web
  engine; staged in `test/vendor/`).
- `test/e2e_sim.py` — full cluster in the CraftOS-PC simulator: gateway + 2
  workers + client; deploy/reconcile/reschedule, typed invokes, cross-module
  mesh, all language lanes, sqlite volume persistence. **10/10.**
- `python3 test/e2e_crabgen.py go rust cpp ts` — the full crabgen developer
  workflow per lane (scaffold, build, deploy, invoke; then WIT evolution with
  check/regen) in the same simulator. **40/40** (~26 min).
