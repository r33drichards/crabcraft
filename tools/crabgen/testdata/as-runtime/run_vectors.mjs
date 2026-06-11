// Node driver for the AS WIRE conformance wasm (built by
// tests/as_vectors.rs from assembly/main.ts).
//
//   node run_vectors.mjs <path/to/vectors.wasm>
//
// 1. Instantiates the wasm with a fake `crabcraft.call` host (replies, in
//    order: ok 0x07; err "boom"; null ptr — written into guest memory via
//    re-entrant crab_alloc, exactly like the real host).
// 2. Calls _initialize (reactor shape; registers handlers / sets schema).
// 3. Smoke-tests the section-2 ABI from the host side: crab_schema,
//    crab_invoke on an unknown function, crab_invoke on the registered
//    ping handler.
// 4. Runs the in-wasm vectors table + edge cases via run(); prints the
//    failure log and exits non-zero on any failure.

import fs from "node:fs";
import process from "node:process";

const wasmPath = process.argv[2];
if (!wasmPath) {
  console.error("usage: node run_vectors.mjs <vectors.wasm>");
  process.exit(2);
}

let failures = 0;
function fail(what, detail) {
  console.log(`FAIL ${what}: ${detail}`);
  failures++;
}

let instance; // assigned after instantiation; the mesh fake needs it

const mem = () => new Uint8Array(instance.exports.memory.buffer);

// Write a [status][body] payload into guest memory as a LENBUF via the
// guest's own crab_alloc (re-entrant host->wasm call, like the real host).
function writeLenbuf(bytes) {
  const ptr = instance.exports.crab_alloc(4 + bytes.length) >>> 0;
  const view = new DataView(instance.exports.memory.buffer);
  view.setUint32(ptr, bytes.length, true);
  mem().set(bytes, ptr + 4);
  return ptr;
}

function readLenbuf(ptr) {
  ptr = ptr >>> 0;
  const view = new DataView(instance.exports.memory.buffer);
  const n = view.getUint32(ptr, true);
  return mem().slice(ptr + 4, ptr + 4 + n);
}

const utf8 = new TextEncoder();

let meshCalls = 0;
const imports = {
  crabcraft: {
    call(wlPtr, wlLen, fnPtr, fnLen, parPtr, parLen) {
      meshCalls++;
      const wl = Buffer.from(mem().slice(wlPtr >>> 0, (wlPtr >>> 0) + wlLen)).toString();
      const fn = Buffer.from(mem().slice(fnPtr >>> 0, (fnPtr >>> 0) + fnLen)).toString();
      if (wl !== "svc" || fn !== "a:b/c@0.1.0#d") {
        fail("mesh fake", `unexpected target ${wl} ${fn}`);
      }
      if (meshCalls === 1) {
        // guest sent uleb(7) params
        const par = mem().slice(parPtr >>> 0, (parPtr >>> 0) + parLen);
        if (Buffer.from(par).toString("hex") !== "07") {
          fail("mesh fake", `unexpected params ${Buffer.from(par).toString("hex")}`);
        }
        return writeLenbuf([0, 0x07]); // status 0, body 0x07
      }
      if (meshCalls === 2) {
        const msg = utf8.encode("boom");
        return writeLenbuf([1, msg.length, ...msg]); // status 1, "boom"
      }
      return 0; // null pointer => empty payload
    },
  },
};

const { instance: inst } = await WebAssembly.instantiate(
  fs.readFileSync(wasmPath),
  imports
);
instance = inst;

// Reactor shape: _initialize must exist (top-level registration) and the
// host calls it once.
if (typeof instance.exports._initialize !== "function") {
  fail("abi", "missing _initialize export");
} else {
  instance.exports._initialize();
}
for (const name of ["crab_alloc", "crab_schema", "crab_invoke", "memory"]) {
  if (!(name in instance.exports)) fail("abi", `missing ${name} export`);
}

// --- host-side ABI smoke ----------------------------------------------------

// crab_schema serves the schema set in _initialize.
{
  const schema = Buffer.from(readLenbuf(instance.exports.crab_schema())).toString();
  if (schema !== '{"smoke":true}') fail("crab_schema", `got ${schema}`);
}

function invoke(name, argBytes) {
  const nameBytes = typeof name === "string" ? utf8.encode(name) : name;
  const namePtr = instance.exports.crab_alloc(nameBytes.length) >>> 0;
  mem().set(nameBytes, namePtr);
  const argPtr = instance.exports.crab_alloc(Math.max(argBytes.length, 1)) >>> 0;
  mem().set(argBytes, argPtr);
  return readLenbuf(
    instance.exports.crab_invoke(namePtr, nameBytes.length, argPtr, argBytes.length)
  );
}

// Unknown function => status 1 + "unknown function: <name>".
{
  const reply = invoke("nope#x", new Uint8Array(0));
  const want = "unknown function: nope#x";
  const wantBytes = utf8.encode(want);
  const ok =
    reply[0] === 1 &&
    reply[1] === wantBytes.length &&
    Buffer.from(reply.slice(2)).toString() === want;
  if (!ok) fail("crab_invoke unknown", Buffer.from(reply).toString("hex"));
}

// Invalid-UTF-8 name bytes => status 1 + "invalid function name" (the name
// is host-provided; the guest validates before decoding it).
{
  const reply = invoke(new Uint8Array([0xff, 0xfe]), new Uint8Array(0));
  const want = "invalid function name";
  const got = reply[0] === 1 ? Buffer.from(reply.slice(2)).toString() : "(status 0)";
  if (got !== want) fail("crab_invoke bad name", got);
}

// Registered ping handler: u32(7) in => status 0, u32(8) out.
{
  const reply = invoke("test:x/y@0.1.0#ping", new Uint8Array([0x07]));
  if (Buffer.from(reply).toString("hex") !== "0008") {
    fail("crab_invoke ping", Buffer.from(reply).toString("hex"));
  }
}

// Handler decode error surfaces as "<name>: <codec error>".
{
  const reply = invoke("test:x/y@0.1.0#ping", new Uint8Array([0x07, 0x00]));
  const want = "test:x/y@0.1.0#ping: 1 trailing byte(s) after params";
  const got = reply[0] === 1 ? Buffer.from(reply.slice(2)).toString() : "(status 0)";
  if (got !== want) fail("crab_invoke trailing", got);
}

// --- in-wasm vectors + edge cases --------------------------------------------

const wasmFailures = instance.exports.run();
const logPtr = instance.exports.logPtr() >>> 0; // must come before logLen()
const logLen = instance.exports.logLen();
const log = Buffer.from(mem().slice(logPtr, logPtr + logLen)).toString();
if (log.length > 0) process.stdout.write(log);
failures += wasmFailures;

if (meshCalls !== 3) fail("mesh fake", `expected 3 mesh calls, saw ${meshCalls}`);

if (failures !== 0) {
  console.log(`${failures} failure(s)`);
  process.exit(1);
}
console.log(`ok: ${instance.exports.nvectors()} vectors + edge cases`);
