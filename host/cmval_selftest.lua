-- Round-trip self-test for cmval + json (runs on lua5.4 and Cobalt).
-- Golden-vector cross-check against wit/vectors.json happens separately once
-- the Rust SDK emits it (cmval_vectors.lua).
package.path = "host/?.lua;" .. package.path
local cm = require("cmval")
local json = require("json")

local passed, failed = 0, 0
local function eq(a, b)
  if type(a) ~= type(b) then return false end
  if type(a) ~= "table" then
    if type(a) == "number" then return math.abs(a - b) <= 1e-6 * math.max(1, math.abs(a)) end
    return a == b
  end
  for k, v in pairs(a) do if not eq(v, b[k]) then return false end end
  for k, v in pairs(b) do if not eq(v, a[k]) then return false end end
  return true
end
local function rt(desc, ty, v)
  local ok, err = pcall(function()
    local bytes = cm.encode(ty, v)
    local back, pos = cm.decode(ty, bytes)
    assert(pos == #bytes + 1, "did not consume all bytes")
    assert(eq(v, back), "value mismatch")
  end)
  if ok then passed = passed + 1
  else failed = failed + 1; print("FAIL " .. desc .. ": " .. tostring(err)) end
end

rt("bool", "bool", true); rt("bool f", "bool", false)
rt("u8", "u8", 255)
rt("u32 small", "u32", 5)
rt("u32 multi", "u32", 624485)
rt("u64 big", "u64", 2 ^ 50)
rt("s32 -1", "s32", -1)
rt("s32 -624485", "s32", -624485)
rt("s64", "s64", -2 ^ 40)
rt("f32", "f32", 1.5)
rt("f64", "f64", -2.75)
rt("f64 pi", "f64", math.pi)
rt("char A", "char", 65)
rt("string", "string", "hello \195\169\226\152\131")
rt("list u32", { kind = "list", elem = "u32" }, { 1, 2, 3 })
rt("list empty", { kind = "list", elem = "string" }, {})
local greq = { kind = "record", fields = {
  { name = "name", type = "string" },
  { name = "excited", type = { kind = "option", elem = "bool" } },
} }
rt("record none", greq, { name = "steve" })
rt("record some", greq, { name = "steve", excited = true })
rt("tuple", { kind = "tuple", members = { "u32", "string" } }, { 7, "x" })
rt("enum", { kind = "enum", cases = { "a", "b", "c" } }, "c")
local var = { kind = "variant", cases = { { name = "none" }, { name = "num", type = "u32" } } }
rt("variant payload", var, { case = "num", value = 9 })
rt("variant bare", var, { case = "none" })
rt("option none", { kind = "option", elem = "string" }, nil)
rt("option some", { kind = "option", elem = "string" }, "yo")
rt("result ok", { kind = "result", ok = "u32", err = "string" }, { is_ok = true, ok = 4 })
rt("result err", { kind = "result", ok = "u32", err = "string" }, { is_err = true, err = "boom" })
local fl = { kind = "flags", names = { "a", "b", "c", "d", "e", "f", "g", "h", "i", "j" } }
rt("flags", fl, { b = true, d = true, j = true })

-- exercise the handcoded float paths even where string.pack exists
do
  local sp, su = string.pack, string.unpack
  string.pack, string.unpack = nil, nil
  package.loaded["cmval"] = nil
  local cm2 = require("cmval")
  local function rt2(desc, ty, v)
    local bytes = cm2.encode(ty, v)
    local back = cm2.decode(ty, bytes)
    if not eq(v, back) then failed = failed + 1; print("FAIL nopack " .. desc)
    else passed = passed + 1 end
  end
  rt2("f64 nopack", "f64", -2.75)
  rt2("f64 pi nopack", "f64", math.pi)
  rt2("f64 small nopack", "f64", 2 ^ -1050)
  rt2("f32 nopack", "f32", 1.5)
  rt2("f32 neg nopack", "f32", -0.25)
  string.pack, string.unpack = sp, su
end

-- json sanity
do
  local v = json.decode('{"a":[1,2,{"b":"x\\n"}],"c":true,"d":null,"e":-1.5e2}')
  assert(v.a[3].b == "x\n" and v.c == true and v.e == -150, "json decode")
  local rt_ = json.decode(json.encode({ x = { 1, 2, 3 }, y = "hi" }))
  assert(rt_.y == "hi" and rt_.x[2] == 2, "json round trip")
  passed = passed + 2
end

print(string.format("%d/%d assertions passed", passed, passed + failed))
if failed == 0 then print("ALL_PASS") else print("FAILED") end
