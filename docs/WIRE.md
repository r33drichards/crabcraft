# crabcraft wire format (v0)

crabcraft is wasm orchestration for ComputerCraft (more k8s than Lambda):
WIT-defined interfaces, wRPC-style invocation over **rednet**, wasi-p1 guest
modules executed by [wasmcraft](https://github.com/r33drichards/wasmcraft).
This document is the single normative reference; every implementation (guest
SDKs in any language, the Lua hosts) MUST conform to it.

**The contract is this ABI, not a language.** A workload is ANY `.wasm` file
(wasm32-wasip1) that either exports the section-2 functions (`reactor` kind:
warm instances, typed WIT interfaces) or is a plain wasi command module
(`command` kind: `_start`, JSON on stdin/stdout). Rust, TinyGo, zig-built
C/C++, and AssemblyScript are worked examples, not a whitelist -
anything that compiles to wasi-p1 and speaks the ABI deploys identically. An
interpreter compiled to wasm is itself a valid workload, which is how
script-style invocation is supported: deploy the interpreter, send it code.

It implements a **subset** of the wRPC SPEC (https://github.com/bytecodealliance/wrpc/blob/main/SPEC.md):
synchronous parameters/results in the root frame only. Streams, futures,
resources, and async indexed paths are NOT implemented (v0 gap, documented).

## 1. Value encoding (component-model value encoding, subset)

All multi-byte primitives little-endian. `uleb(n)`/`sleb(n)` = unsigned/signed
LEB128.

| WIT type | encoding |
|---|---|
| `bool` | 1 byte: 0 or 1 |
| `u8 u16 u32 u64` | `uleb(value)` |
| `s8 s16 s32 s64` | `sleb(value)` |
| `f32` | 4 bytes IEEE-754 LE |
| `f64` | 8 bytes IEEE-754 LE |
| `char` | `uleb(unicode scalar value)` |
| `string` | `uleb(byte length)` then UTF-8 bytes |
| `list<T>` | `uleb(count)` then each element |
| `record` | each field in declaration order, concatenated (no tags) |
| `tuple<...>` | each member in order |
| `variant` | `uleb(case index)` then payload (if the case has one) |
| `enum` | `uleb(case index)` |
| `option<T>` | 1 byte: 0 = none; 1 = some, then `T` |
| `result<T,E>` | 1 byte: 0 = ok then `T` (if any); 1 = err then `E` (if any) |
| `flags` | `ceil(n/8)` bytes, bit i = flag i, LE byte order |

A function's **params** are encoded as each parameter in declaration order,
concatenated. The **result** is the encoded result type (functions in v0 have
zero or one result).

Caveat (Lua hosts): `u64`/`s64` beyond 2^53 lose precision in Lua numbers; the
codec MUST round-trip the encoded bytes faithfully even so. Schemas targeting
CC clients should prefer `u32`/`s32`.

## 2. Guest ABI (host <-> wasm module)

Guests are wasm32-wasip1 **reactor** modules (no `_start`; export
`_initialize` if needed, the host calls it once when present).

Required exports:

```
crab_alloc(len: i32) -> i32        ; allocate len bytes, return ptr
crab_schema() -> i32               ; ptr to LENBUF (see below) holding the
                                   ; resolved-WIT JSON for this module
crab_invoke(name_ptr: i32, name_len: i32,
            arg_ptr: i32, arg_len: i32) -> i32   ; ptr to LENBUF reply
```

`LENBUF` = 4 bytes u32 LE byte-length, followed by that many payload bytes.
Buffers returned by the guest stay valid until the next `crab_invoke` /
`crab_schema` call (host copies immediately).

`crab_invoke` arguments: `name` = UTF-8 function address
`<instance>#<function>` (e.g. `crab:hello/greeter@0.1.0#greet`); `arg` = the
encoded params per section 1. Reply payload:

```
[status: 1 byte] [body]
  status 0 = ok    body = encoded result value (empty if no result)
  status 1 = error body = string (uleb len + utf8 message)
```

Unknown function name => status 1 with message `unknown function: <name>`.

### Optional import: the service mesh (`crabcraft.call`)

A guest MAY import (module `"crabcraft"`, field `"call"`):

```
call(wl_ptr: i32, wl_len: i32,      ; target workload NAME (utf-8)
     fn_ptr: i32, fn_len: i32,      ; function address <instance>#<function>
     par_ptr: i32, par_len: i32)    ; encoded params (section 1)
  -> i32                            ; ptr to LENBUF reply
```

The reply LENBUF payload uses the same `[status][body]` shape as crab_invoke
replies (0 = ok + encoded result, 1 = error string). The host routes the call
through the gateway to wherever the target workload runs - guests address
SERVICES BY NAME and never know about placement (that is the mesh). The host
writes the reply into guest memory via `crab_alloc`. Re-entrancy is not
guaranteed in v0: a workload calling itself (directly or via a cycle) may
deadlock; gateways apply timeouts.

## 3. Orchestration model (more k8s than Lambda)

Three roles on one rednet protocol `"crabcraft"`:

- **gateway** (control plane, one per cluster): owns the workload REGISTRY
  (desired state: name -> {url, kind, schema}), runs a RECONCILE loop
  (assign unscheduled workloads to free worker slots; reschedule when a worker
  misses heartbeats), and ROUTES invoke requests to the right worker.
- **workers** (data plane): a CC computer with one or more DISKS. One workload
  per disk: the disk holds the fetched wasm + all files the workload writes
  (its persistent volume - WASI fs root is the disk, so module state survives
  reboots and travels with the floppy). Workers register with the gateway,
  heartbeat their slots, fetch+run assignments on wasmcraft.
- **clients**: talk only to the gateway.

### Workload kinds

- `kind = "reactor"` (Rust, TinyGo, ...): the section-2 crab ABI. Instantiated
  once and kept WARM; each invoke is a `crab_invoke` call. Params/results =
  section-1 encoded values; interface defined in WIT.
- `kind = "command"` (any wasi CLI): a wasi command
  module. Each invoke runs `_start` with the request JSON on stdin; stdout is
  the reply JSON. No WIT/schema typing (schema request returns a stub); slower
  (boot per invoke) but ANY wasi binary works unmodified.
- `kind = "job"`: run-to-completion (the k8s Jobs resource), optionally on a
  cron schedule. A gateway-side concept built from the two kinds above -
  workers never see it. Section 6.

### Messages (every request carries client-chosen `id`, echoed in replies;
replies are `{id, ok=true, ...}` or `{id, ok=false, err}`; long operations may
emit interim `{id, status}` notes)

client -> gateway:
```lua
{ id, type="deploy", name="hello", url="https://...wasm", kind="reactor",
  schema="<resolved-WIT json>" }            -- register desired state
  -- kind="job" adds: module="command"|"reactor", body | func+params,
  --                  schedule, retries, timeout, keep   (section 6)
{ id, type="remove", name }
{ id, type="list" }                         -- registry + placement + health
{ id, type="schema", name }                 -- the workload's schema JSON
{ id, type="invoke", name, func="crab:hello/greeter@0.1.0#greet",
  params=<binary string> }                  -- reactor kind
{ id, type="invoke", name, body="<json>" }  -- command kind
{ id, type="run", name }                    -- queue a run of a job NOW
{ id, type="job-logs", name }               -- a job's recent run history
{ id, type="ping" }
```

worker -> gateway:
```lua
{ id, type="register", worker=<label>, slots={ {disk="disk", workload=nil}, ... } }
{ id, type="heartbeat", worker, slots={ {disk, workload, state} } } -- every ~5s
```

gateway -> worker:
```lua
{ id, type="assign", slot="disk", name, url, kind }   -- fetch + run
{ id, type="drain",  slot="disk" }                    -- stop + wipe assignment
{ id, type="invoke", name, func, params | body }      -- routed client request
```

The gateway relays worker invoke replies back to the requesting client
unchanged (id-matched). Scheduling is dumb-and-honest v0: first free slot
wins; a workload missing for N heartbeats is rescheduled to another free slot
(its DISK data does not migrate - state is volume-local, exactly like a pod
losing its node-local volume).

## 4. Manifests (declarative deploys)

`crb deploy <file.yml>` applies a manifest (YAML subset: scalars, nested maps,
lists of scalars; no anchors/multi-doc):

```yaml
name: hello
wasm: https://example.com/hello.wasm     # fetched by the assigned worker
kind: reactor                             # or: command | job
schema: https://example.com/hello.json    # resolved-WIT JSON (reactor kind);
                                          # url or inline path on the client
```

A job manifest (section 6) adds the run payload and policy:

```yaml
name: greet-job
wasm: https://example.com/hello.wasm
kind: job
schema: https://example.com/hello.json    # required when func: is used
func: greet                # typed run: one call on a reactor module ...
args:                      # ... with these args (validated + encoded by crb)
  name: crabcraft
# body: '{"op":"dump"}'    # OR an untyped run: command module, body on stdin
schedule: "*/5 * * * *"    # optional: cron (omit = one run at deploy)
retries: 1                 # optional: per-run retry budget (default 0)
timeout: 120               # optional: per-run seconds (default 600)
keep-warm: true            # optional: hold the slot between runs (default false)
```

The client reads the manifest, fetches `schema` itself if it is a URL/path,
and sends a single `deploy` message carrying the schema inline (workers never
need wasm-tools). For `func:` jobs it also resolves the function address and
encodes `args:` per section 1 at deploy time - bad schedules, unknown
functions, and ill-typed args all fail the deploy, and the gateway never
needs the codec.

## 5. Schema-driven clients (the factory)

Clients fetch the module's resolved-WIT JSON (`wasm-tools component wit
--json`) via the `schema` request, then encode/decode values generically from
it — no per-interface codegen. The JSON's `interfaces[].functions` +
`types` tables drive both validation and the section-1 codec.

## 6. Jobs and cron (run-to-completion)

`kind = "job"` is the k8s Jobs resource: desired state is "this module RAN",
not "this module is running". With `schedule:` it is the CronJob resource.
Jobs are purely a control-plane concept — a run is an ordinary `assign`, then
exactly one `invoke`, then a `drain`, so any worker version executes jobs
unmodified.

**The run payload** is one of:
- `module = "command"`: a wasi command module; `body` goes to stdin, stdout is
  the recorded output (`argv`/`body-file` knobs from the command kind apply).
- `module = "reactor"`: one typed call — `func` (full address) with `params`
  pre-encoded by the client at deploy time (section 4).

**Run state machine** (gateway): `pending` (waiting for a free slot) ->
`placing` (assigned; module fetching/transpiling) -> `running` (the run invoke
is in flight) -> a history entry `{ n, ok, t, dur, tries, output | err }`. The
gateway keeps the last 5 runs per job (outputs truncated at 4 KB) in
`.crab_jobs`, so history survives gateway reboots; an IN-FLIGHT run does not —
its leftover slot is detected and drained, and the run number is left as a
gap. `list` rows for jobs carry `{ state, schedule, runs, ok, fail, skip,
last }`; `job-logs` returns the full history.

**Semantics** (v0, deliberately boring):
- One run at a time per job (placements are keyed by name). A firing that
  finds the previous run still active is SKIPPED and counted (`skip`) — k8s
  `concurrencyPolicy: Forbid`.
- An unscheduled job runs once at deploy (k8s: creating a Job runs it). `run`
  queues another run any time, scheduled or not.
- Failures (invoke error, per-run `timeout`, worker lost mid-run) consume the
  `retries` budget with a fresh placement per attempt; the final failure is
  recorded. Unschedulable runs wait in `pending` indefinitely (visible in
  `list`); `remove` or a forced redeploy clears them.
- `keep = true` (manifest `keep-warm`) holds the slot — and the transpiled
  module — between successful runs instead of draining; for schedules too
  fast to re-fetch/re-transpile each time. The cost: the slot stays occupied.
- Schedules evaluate against REAL-WORLD UTC (`os.epoch("utc")`), not Minecraft
  time. Grammar: 5-field vixie cron (`*` lists `a,b` ranges `a-b` steps `*/n
  a-b/n a/n`, `jan-dec`/`sun-sat` names, dow `0|7` = sunday, dom+dow both
  restricted = OR), the `@hourly @daily @midnight @weekly @monthly @yearly`
  macros, or `@every <dur>` (`30s`, `5m`, `1h30m`; ~2 s resolution from the
  gateway's cron tick). Minute schedules fire at most once per matching
  minute. NO catch-up: firings due while the gateway is down are missed, and
  `@every` re-phases from gateway boot.
