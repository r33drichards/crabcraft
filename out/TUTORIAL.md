# Running Picat on crabcraft (migrating from picatd)

Audience: you previously ran Picat in-game with **picatd** (the standalone
wasmcraft daemon) and the `pic` client. crabcraft replaces that single-daemon
setup with a small orchestrator: the same warm-Picat behavior, plus
scheduling, reboot recovery, fleet updates, and a uniform client model shared
with every other wasm workload. This tutorial gets you from zero to
`fib(20) = 10946` and then to your own programs.

## TL;DR

```
-- gateway computer (modem; monitor optional but nice):
wget https://github.com/r33drichards/crabcraft/releases/latest/download/gateway.lua gateway
gateway --install
gateway

-- each worker computer (modem + disk drive with a floppy):
wget https://github.com/r33drichards/crabcraft/releases/latest/download/worker.lua worker
worker --install
worker

-- pocket computer (wireless modem):
wget https://github.com/r33drichards/crabcraft/releases/latest/download/crb.lua crb
wget https://github.com/r33drichards/crabcraft/releases/latest/download/picat.yml picat.yml
crb deploy picat.yml
wget https://github.com/r33drichards/crabcraft/releases/latest/download/fib.lua fib
fib 20
```

Wait for the gateway dashboard (or `crb ls`) to show picat `running` before
the first `fib` — the initial boot transpiles 5.3 MB of Picat once and takes
a few minutes. After that, invokes answer in seconds.

## Concept map: picatd -> crabcraft

| picatd | crabcraft |
|---|---|
| `picatd` daemon on one computer | `kind: session` workload placed on some worker's disk by the gateway |
| `picatd --install` | `worker --install` (and the gateway re-places workloads after any reboot) |
| `pic <daemon> <goal>` one-shots | `fib`-style scripts via the client library, or `crb invoke` |
| `pic -n foo` named sessions | `-s foo` on invokes: per-session engines + state, booted on demand, executing concurrently |
| daemon monitor dashboard | gateway monitor dashboard (workloads, workers, versions, log) |
| manual file copies to update | `crb update` rolls the whole fleet |
| daemon id caching, liveness polling in `pic` | done by the gateway + client library; you just call |

The big semantic carry-over: **the session is warm**. The Picat runtime boots
once at deploy and stays resident; each invoke writes your program to the
workload's volume as `req.pi`, loads it into the live REPL, and runs `main`.
Like picatd — and unlike crabcraft's `command` kind, which would re-boot the
5 MB runtime on every call (~10x slower per invoke).

## 1. The manifest

`picat.yml` (a release asset; also in `manifests/`):

```yaml
name: picat
wasm: https://github.com/r33drichards/wasmcraft/releases/latest/download/picat.wasm
kind: session
```

`crb deploy picat.yml` registers it; the gateway's control loop picks a free
worker slot, the worker fetches the wasm to its floppy and boots the session.
The registry is durable: gateway reboots, worker reboots, and chunk unloads
all converge back to this state (the reconciliation logic is model-checked —
see `spec/crabcraft.tla`).

## 2. fib, the canned demo

```
wget https://github.com/r33drichards/crabcraft/releases/latest/download/fib.lua fib
fib        -- fib(10) = 89
fib 20     -- fib(20) = 10946
```

`fib` is ~25 lines and is the template for everything else: it formats a
Picat program, sends it, prints the output and the round-trip time.

## 3. Your own programs

A workload invoke takes a **complete Picat program** (with `main`) and
returns its stdout. Script pattern:

```lua
-- myprog (a file on any computer with a wireless modem)
local LIBURL = "https://github.com/r33drichards/crabcraft/releases/latest/download/crblib.lua"
if not fs.exists("crblib") then
  local r = assert(http.get(LIBURL))
  local h = fs.open("crblib", "w") h.write(r.readAll()) h.close() r.close()
end
local lib = dofile("crblib")

local picat = lib.client.connect():workload("picat")

local out = picat([[
queens(N, Q) =>
  Q = new_list(N), Q :: 1..N,
  all_different(Q),
  all_different([$Q[I] - I : I in 1..N]),
  all_different([$Q[I] + I : I in 1..N]),
  solve(Q).

main => queens(8, Q), println(Q).
]])
print(out)
```

Or generate a typed client module once and `require` it from any script:

```
crb gen picat
```
```lua
local picat = require("picat_client")
print(picat("main => println(fib(10)).\nfib(0)=1.\nfib(1)=1.\nfib(N)=fib(N-1)+fib(N-2)."))
```

## 4. Semantics you should know (vs picatd)

- **Definitions persist within the session.** Each invoke `cl`-loads your
  program into the same live REPL, so predicates defined in earlier invokes
  remain until redefined. A worker reboot or redeploy gives a fresh session
  (the gateway restarts it automatically). To force a reset:
  `crb rm picat && crb deploy picat.yml`.
- **Named sessions, shared execution** (picatd's `pic -n` model): pass a
  session name and you get a separate live engine with separate state, booted
  on first use, executing CONCURRENTLY with every other session:
  `crb invoke picat run ... -s alice`, or from a script
  `picat("program", "alice")`. Reset one with `crb reset picat -s alice`
  (script: `picat.reset("alice")`). Omitting the name uses `main`. For
  spreading load across WORKERS, deploy the same wasm under more names
  (`picat2.yml`) - sessions share their workload's slot and computer.
- **Placement is the gateway's job.** The workload may land on any worker
  with a free disk; its files (programs, anything it writes) live on that
  floppy and survive reboots. `crb ls` shows where everything is.
- **Latency profile**: deploy-time boot is minutes (once per session, on a
  session's first use); warm invokes are seconds plus your program's runtime.
  The client waits up to 300s.
- **Timeouts mean look at the dashboard.** `workload 'picat' is not running
  (state: ...)` right after a gateway reboot is the reconciler converging;
  retry in ~10s. Persistent `assign FAILED ... fetch failed` usually means
  the floppy is too small — raise `diskSpaceLimit` in the CC:T server config
  (picat.wasm is 5.3 MB).

## 5. Operating the cluster

```
crb ls               -- workloads, placements, workers, versions
crb rm picat         -- remove one workload (volume data stays on the floppy)
crb purge            -- remove everything
crb update           -- roll worker.lua to every worker, then the gateway
```

The gateway dashboard (any attached monitor) shows live state: workloads with
placement and color-coded health, per-slot detail (`/disk picat picat.wasm`),
worker versions, and the reconciler's log — deploys, adoptions after reboots,
failed assigns with reasons, GC of duplicates.

## Why trust the orchestration?

The reconcile/adoption/GC/orphan logic is modeled in TLA+
(`spec/crabcraft.tla`) and checked with TLC against worker crashes, gateway
reboots, assign failures, and message loss: no stuck states, and the cluster
provably converges to exactly one running copy per deployed workload. Run it
yourself: `nix shell nixpkgs#tlaplus --command tlc -deadlock spec/crabcraft.tla`.
The full stack is also exercised end-to-end in a CraftOS-PC simulator on every
change (`test/e2e_sim.py`, 9 checks across Rust/Go/C/JS workloads, the mesh,
and SQLite volume persistence).
