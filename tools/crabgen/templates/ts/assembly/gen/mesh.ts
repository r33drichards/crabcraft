// crabcraft service mesh client: the optional `crabcraft.call` host import
// (WIRE.md section 2). crabgen emits this template only when the module's
// WIT world has imports — an @external declaration in a referenced file
// lands in the wasm import section whether or not it is ever called, and
// wasmcraft would then require the host to provide it. Keeping it in a
// separate file is what keeps import-free modules import-free (same split
// as the Go template's mesh.go / mesh_wasm.go).
//
// Same error convention as runtime.ts: meshCall/parseMeshReply return a
// HandlerResult (exactly one of bytes / err set). Error message texts
// mirror templates/go/mesh.go exactly.

import { Decoder, HandlerResult, unpinAlloc } from "./runtime";

// WIRE.md section 2 optional import (module "crabcraft", field "call"):
// returns a pointer to a LENBUF reply the host wrote into guest memory via
// crab_alloc.
// @ts-ignore: decorator is AS-specific
@external("crabcraft", "call")
declare function crabcraft_call(
  wlPtr: usize,
  wlLen: u32,
  fnPtr: usize,
  fnLen: u32,
  parPtr: usize,
  parLen: u32
): usize;

// meshCall invokes `fn` (a `<instance>#<function>` address) on the workload
// named `workload` through the host mesh, passing WIRE-encoded params, and
// returns the encoded result value. A status-1 reply decodes the error
// string. The host addresses services BY NAME; placement is its problem.
export function meshCall(
  workload: string,
  fn: string,
  params: Uint8Array
): HandlerResult {
  const wl = Uint8Array.wrap(String.UTF8.encode(workload));
  const f = Uint8Array.wrap(String.UTF8.encode(fn));
  const ptr = crabcraft_call(
    wl.dataStart,
    <u32>wl.length,
    f.dataStart,
    <u32>f.length,
    params.dataStart,
    <u32>params.length
  );
  // Read the LENBUF ([u32 LE length][payload]); a null pointer reads as an
  // empty payload. The host wrote the reply via crab_alloc: copy out what
  // we need, then unpin so the buffer can be collected.
  let payload = new Uint8Array(0);
  if (ptr != 0) {
    const n = load<u32>(ptr);
    payload = new Uint8Array(<i32>n);
    if (n > 0) memory.copy(payload.dataStart, ptr + 4, <usize>n);
    unpinAlloc(ptr);
  }
  return parseMeshReply(payload);
}

// parseMeshReply splits a [status][body] mesh reply: status 0 returns the
// body (the encoded result value); status 1 decodes the body as the error
// string, which must consume the body exactly; anything else is a protocol
// error.
export function parseMeshReply(payload: Uint8Array): HandlerResult {
  if (payload.length == 0) return HandlerResult.fail("mesh call: empty reply");
  const status = payload[0];
  const body = payload.slice(1);
  if (status == 0) return HandlerResult.pass(body);
  if (status == 1) {
    const d = new Decoder(body);
    const msg = d.string();
    if (d.err !== null) {
      return HandlerResult.fail("mesh call: malformed error reply");
    }
    if (d.remaining() != 0) {
      return HandlerResult.fail(
        "mesh call: malformed error reply (trailing bytes)"
      );
    }
    return HandlerResult.fail(msg);
  }
  return HandlerResult.fail("mesh call: invalid reply status");
}
