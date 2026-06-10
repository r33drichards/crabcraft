# crabcraft wire format (v0)

crabcraft is "Lambda with wasm" for ComputerCraft: WIT-defined interfaces,
wRPC-style invocation over **rednet**, wasi-p1 guest modules running on
[wasmcraft](https://github.com/r33drichards/wasmcraft). This document is the
single normative reference for the three layers; the Rust guest SDK and the
Lua host MUST both conform to it.

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
- `kind = "command"` (Javy/QuickJS JS, Python, any wasi CLI): a wasi command
  module. Each invoke runs `_start` with the request JSON on stdin; stdout is
  the reply JSON. No WIT/schema typing (schema request returns a stub); slower
  (boot per invoke) but ANY wasi binary works unmodified.

### Messages (every request carries client-chosen `id`, echoed in replies;
replies are `{id, ok=true, ...}` or `{id, ok=false, err}`; long operations may
emit interim `{id, status}` notes)

client -> gateway:
```lua
{ id, type="deploy", name="hello", url="https://...wasm", kind="reactor",
  schema="<resolved-WIT json>" }            -- register desired state
{ id, type="remove", name }
{ id, type="list" }                         -- registry + placement + health
{ id, type="schema", name }                 -- the workload's schema JSON
{ id, type="invoke", name, func="crab:hello/greeter@0.1.0#greet",
  params=<binary string> }                  -- reactor kind
{ id, type="invoke", name, body="<json>" }  -- command kind
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
kind: reactor                             # or: command
schema: https://example.com/hello.json    # resolved-WIT JSON (reactor kind);
                                          # url or inline path on the client
```

The client reads the manifest, fetches `schema` itself if it is a URL/path,
and sends a single `deploy` message carrying the schema inline (workers never
need wasm-tools).

## 5. Schema-driven clients (the factory)

Clients fetch the module's resolved-WIT JSON (`wasm-tools component wit
--json`) via the `schema` request, then encode/decode values generically from
it — no per-interface codegen. The JSON's `interfaces[].functions` +
`types` tables drive both validation and the section-1 codec.
