-- E2E milestone: the Rust hello.wasm running on wasmcraft, invoked through the
-- crab ABI with component-encoded values. COBALT (or CC) only.
package.path = "host/?.lua;" .. package.path
local rt = require("runtime")
local cm = require("cmval")

local f = assert(io.open("modules/hello.wasm", "rb"))
local bytes = f:read("*a"); f:close()

local t0 = os.clock()
local w = rt.load_reactor(bytes, { mode = arg and arg[1] or "transpile" })
print(("loaded hello.wasm (%s mode) in %.1fs"):format(w.mode or "?", os.clock() - t0))

assert(w.schema_json:find('"greeter"', 1, true), "schema JSON missing greeter")
print("schema: " .. #w.schema_json .. " bytes of resolved-WIT JSON ok")

local greq = { kind = "record", fields = {
  { name = "name", type = "string" },
  { name = "excited", type = { kind = "option", elem = "bool" } },
} }

local passed, failed = 0, 0
local function check(desc, got, want)
  if got == want then passed = passed + 1; print("ok   " .. desc .. " -> " .. tostring(got))
  else failed = failed + 1; print("FAIL " .. desc .. ": got " .. tostring(got) .. " want " .. tostring(want)) end
end

-- greet, no excitement
local r = w:invoke("crab:hello/greeter@0.1.0#greet",
  cm.encode_params({ greq }, { { name = "steve" } }))
assert(r.ok, r.err)
check("greet(steve)", cm.decode("string", r.result), "Hello, steve!")

-- greet, excited
r = w:invoke("crab:hello/greeter@0.1.0#greet",
  cm.encode_params({ greq }, { { name = "crab", excited = true } }))
assert(r.ok, r.err)
check("greet(crab, excited)", cm.decode("string", r.result), "Hello, crab!!!")

-- add
local t1 = os.clock()
r = w:invoke("crab:hello/greeter@0.1.0#add", cm.encode_params({ "u32", "u32" }, { 40, 2 }))
assert(r.ok, r.err)
check("add(40,2)", cm.decode("u32", r.result), 42)
print(("warm invoke took %.2fs"):format(os.clock() - t1))

-- unknown function -> clean error
r = w:invoke("crab:hello/greeter@0.1.0#nope", "")
check("unknown fn errors", r.ok, false)
check("unknown fn message", (r.err or ""):find("unknown function") ~= nil, true)

print(string.format("%d/%d passed", passed, passed + failed))
if failed == 0 then print("ALL_PASS") else print("FAILED") end
