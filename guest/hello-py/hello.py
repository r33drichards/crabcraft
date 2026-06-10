# crabcraft hello-py: a `kind = "command"` workload (WIRE.md section 3).
# Same stdin/stdout JSON contract as guest/hello-js:
#
#   stdin : {"fn":"greet","name":"x","excited":true} | {"fn":"add","a":1,"b":2}
#   stdout: {"ok":true,"result":"Hello from Python, x!!!"} | {"ok":true,"result":3}
#           {"ok":false,"err":"..."} on any failure
import json
import sys


def handle(req):
    fn = req.get("fn")
    if fn == "greet":
        name = req.get("name")
        if not isinstance(name, str):
            raise ValueError("greet: 'name' must be a string")
        bang = "!!!" if req.get("excited") is True else "!"
        return "Hello from Python, %s%s" % (name, bang)
    if fn == "add":
        a, b = req.get("a"), req.get("b")
        if not isinstance(a, int) or not isinstance(b, int):
            raise ValueError("add: 'a' and 'b' must be numbers")
        # u32 wrap-around semantics, matching the reactor implementations.
        return (a + b) & 0xFFFFFFFF
    raise ValueError("unknown fn: %s" % fn)


try:
    request = json.loads(sys.stdin.readline())
    reply = {"ok": True, "result": handle(request)}
except Exception as exc:  # noqa: BLE001 - everything becomes an err reply
    reply = {"ok": False, "err": str(exc)}

sys.stdout.write(json.dumps(reply) + "\n")
