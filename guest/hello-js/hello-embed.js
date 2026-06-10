// crabcraft hello-js: QuickJS-embedded variant of hello.js (same contract,
// see hello.js / WIRE.md section 3). The C host (main.c) reads all of stdin,
// exposes it as the global string `__input`, evaluates this script, and
// prints the script's completion value (the reply JSON line) to stdout.
//
//   stdin : {"fn":"greet","name":"x","excited":true} | {"fn":"add","a":1,"b":2}
//   stdout: {"ok":true,"result":"Hello from JS, x!!!"} | {"ok":true,"result":3}
//           {"ok":false,"err":"..."} on any failure

let reply;
try {
  // One line of JSON per invocation (trailing newline tolerated).
  const req = JSON.parse(String(__input).split("\n")[0]);
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
// Final expression statement = program completion value, printed by main.c.
JSON.stringify(reply) + "\n";
