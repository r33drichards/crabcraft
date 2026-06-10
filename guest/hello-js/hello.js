// crabcraft hello-js: a `kind = "command"` workload (WIRE.md section 3).
// Each invoke runs _start with ONE LINE of request JSON on stdin and the
// reply JSON on stdout:
//
//   stdin : {"fn":"greet","name":"x","excited":true} | {"fn":"add","a":1,"b":2}
//   stdout: {"ok":true,"result":"Hello from JS, x!!!"} | {"ok":true,"result":3}
//           {"ok":false,"err":"..."} on any failure
//
// NOTE: this Javy-flavored source (Javy.IO) is kept as the contract
// reference. The SHIPPED module is built from hello-embed.js + main.c
// (QuickJS compiled SIMD-free for the pure-Lua wasmcraft engine): see
// build.sh. Keep the two JS files' logic in sync.

function readStdin() {
  const chunks = [];
  const buf = new Uint8Array(4096);
  let total = 0;
  while (true) {
    const n = Javy.IO.readSync(0, buf);
    if (n <= 0) break;
    chunks.push(buf.slice(0, n));
    total += n;
  }
  const all = new Uint8Array(total);
  let off = 0;
  for (const c of chunks) {
    all.set(c, off);
    off += c.length;
  }
  return new TextDecoder().decode(all);
}

function writeStdout(s) {
  const bytes = new TextEncoder().encode(s);
  let off = 0;
  while (off < bytes.length) {
    off += Javy.IO.writeSync(1, bytes.subarray(off));
  }
}

let reply;
try {
  // One line of JSON per invocation (trailing newline tolerated).
  const req = JSON.parse(readStdin().split("\n")[0]);
  if (req.fn === "greet") {
    if (typeof req.name !== "string") throw new Error("greet: 'name' must be a string");
    const bang = req.excited === true ? "!!!" : "!";
    reply = { ok: true, result: "Hello from JS, " + req.name + bang };
  } else if (req.fn === "add") {
    if (typeof req.a !== "number" || typeof req.b !== "number") {
      throw new Error("add: 'a' and 'b' must be numbers");
    }
    // u32 wrap-around semantics, matching the reactor implementations.
    reply = { ok: true, result: (req.a + req.b) >>> 0 };
  } else {
    throw new Error("unknown fn: " + req.fn);
  }
} catch (e) {
  reply = { ok: false, err: String(e && e.message ? e.message : e) };
}
writeStdout(JSON.stringify(reply) + "\n");
