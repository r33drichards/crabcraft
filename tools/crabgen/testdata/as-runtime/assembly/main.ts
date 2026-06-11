// Entry file for the WIRE conformance build (compiled by asc from
// tests/as_vectors.rs). Mirrors the shape the Task-5.2 emitter generates for
// assembly/index.ts: re-export the ABI from gen/runtime, register handlers
// and set the schema at top level (compiled into _initialize via
// `--exportStart _initialize`).

import {
  Decoder,
  HandlerResult,
  registerHandler,
  setSchema,
  Sink,
} from "./gen/runtime";
import { failCount, logString } from "./harness";
import { runVectors, vectorCount } from "./vectors";
import { runEdgeCases } from "./edge";

export { crab_alloc, crab_schema, crab_invoke } from "./gen/runtime";

// ping: decodes one u32 param, returns u32(v + 1). Invoked from the node
// runner through the real crab_invoke ABI as an end-to-end smoke.
function ping(d: Decoder): HandlerResult {
  const v = d.u32();
  if (d.err !== null) return HandlerResult.fail(d.err!);
  const fin = d.finish("params");
  if (fin !== null) return HandlerResult.fail(fin!);
  const s = new Sink();
  s.u32(v + 1);
  if (s.err !== null) return HandlerResult.fail(s.err!);
  return HandlerResult.pass(s.bytes());
}

setSchema('{"smoke":true}');
registerHandler("test:x/y@0.1.0#ping", ping);

// run executes the generated vectors table + the edge cases; returns the
// failure count (details in the log).
export function run(): i32 {
  runVectors();
  runEdgeCases();
  return failCount();
}

export function nvectors(): i32 {
  return vectorCount();
}

// Failure log accessor: logPtr() UTF-8-encodes the log, keeps it referenced,
// and returns the data pointer; logLen() its byte length.
let logBuf: Uint8Array = new Uint8Array(0);

export function logPtr(): usize {
  logBuf = Uint8Array.wrap(String.UTF8.encode(logString()));
  return logBuf.dataStart;
}

export function logLen(): i32 {
  return logBuf.length;
}
