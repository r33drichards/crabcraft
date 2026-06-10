-- Cross-implementation check: the Lua codec must produce/consume EXACTLY the
-- bytes the Rust SDK generated into wit/vectors.json (ground truth).
-- Vector JSON conventions (guest/crab-sdk/src/vectors.rs):
--   type: {kind=..., ...} descriptors; value: record=object, option=null|inline,
--   variant={"case":idx,"payload":...}, result={"ok":..}|{"err":..},
--   flags=[set indices], u64>2^53 = decimal string, char = codepoint number.
package.path = "host/?.lua;" .. package.path
local cm = require("cmval")
local json = require("json")

local function readfile(p)
  local f = assert(io.open(p, "rb")); local d = f:read("*a"); f:close(); return d
end
local vectors = json.decode(readfile("wit/vectors.json"))

local function hex2bin(h)
  return (h:gsub("%x%x", function(b) return string.char(tonumber(b, 16)) end))
end
local function bin2hex(s)
  return (s:gsub(".", function(c) return string.format("%02x", c:byte()) end))
end

-- vector type descriptor -> cmval descriptor
local function conv_type(t)
  local k = t.kind
  if k == "list" then return { kind = "list", elem = conv_type(t.element) } end
  if k == "record" then
    local fields = {}
    for i, f in ipairs(t.fields) do fields[i] = { name = f.name, type = conv_type(f.type) } end
    return { kind = "record", fields = fields }
  end
  if k == "tuple" then
    local m = {}
    for i, mt in ipairs(t.members) do m[i] = conv_type(mt) end
    return { kind = "tuple", members = m }
  end
  if k == "variant" then
    local cases = {}
    for i, c in ipairs(t.cases) do
      cases[i] = { name = c.name, type = c.payload and conv_type(c.payload) or nil }
    end
    return { kind = "variant", cases = cases }
  end
  if k == "enum" then return { kind = "enum", cases = t.cases } end
  if k == "option" then return { kind = "option", elem = conv_type(t.inner) } end
  if k == "result" then
    return { kind = "result", ok = t.ok and conv_type(t.ok) or nil,
      err = t.err and conv_type(t.err) or nil }
  end
  if k == "flags" then
    local names = {}
    for i = 1, t.count do names[i] = "f" .. (i - 1) end
    return { kind = "flags", names = names }
  end
  return k -- primitive
end

-- vector value JSON -> cmval Lua value (needs the cmval descriptor for shape)
local function conv_value(ty, v)
  if type(ty) == "string" then
    return v -- u64 decimal strings pass through; codec handles them
  end
  local k = ty.kind
  if k == "list" then
    local out = {}
    for i, e in ipairs(v) do out[i] = conv_value(ty.elem, e) end
    return out
  elseif k == "record" then
    local out = {}
    for _, f in ipairs(ty.fields) do
      local fv = v[f.name]
      if fv ~= nil then
        local fty = f.type
        if type(fty) == "table" and fty.kind == "option" then
          out[f.name] = conv_value(fty.elem, fv)
        else
          out[f.name] = conv_value(fty, fv)
        end
      end
    end
    return out
  elseif k == "tuple" then
    local out = {}
    for i, mty in ipairs(ty.members) do out[i] = conv_value(mty, v[i]) end
    return out
  elseif k == "variant" then
    local case = ty.cases[v.case + 1] -- vector uses 0-based index
    local out = { case = case.name }
    if case.type then out.value = conv_value(case.type, v.payload) end
    return out
  elseif k == "enum" then
    return ty.cases[v + 1] or v -- index or name
  elseif k == "option" then
    if v == nil then return nil end
    return conv_value(ty.elem, v)
  elseif k == "result" then
    if v.ok ~= nil or (v.err == nil and v.is_ok ~= false) and v.ok ~= nil then
      return { is_ok = true, ok = ty.ok and conv_value(ty.ok, v.ok) or nil }
    elseif v.err ~= nil then
      return { is_err = true, err = ty.err and conv_value(ty.err, v.err) or nil }
    elseif next(v) == nil then
      return { is_ok = true } -- {"ok": null} degenerates to {} in JSON-null land
    end
    -- {"ok": null} for no-payload ok arrives as empty table handled above
    return { is_ok = true }
  elseif k == "flags" then
    local out = {}
    for _, idx in ipairs(v) do out["f" .. idx] = true end
    return out
  end
  return v
end

-- result vectors with {"err": ...} need the err branch even when conv guesses ok:
local function conv_result_value(ty, v)
  if v.err ~= nil then
    return { is_err = true, err = ty.err and conv_value(ty.err, v.err) or nil }
  end
  return { is_ok = true, ok = ty.ok and (v.ok ~= nil and conv_value(ty.ok, v.ok) or nil) or nil }
end

local passed, failed = 0, 0
for _, vec in ipairs(vectors) do
  local ok, err = pcall(function()
    local ty = conv_type(vec.type)
    local val
    if type(ty) == "table" and ty.kind == "result" then
      val = conv_result_value(ty, vec.value)
    else
      val = conv_value(ty, vec.value)
    end
    local golden = hex2bin(vec.hex)
    -- 1. our encode must equal the golden bytes exactly
    local mine = cm.encode(ty, val)
    if mine ~= golden then
      error(("encode mismatch: got %s want %s"):format(bin2hex(mine), vec.hex))
    end
    -- 2. our decode of the golden bytes must round-trip through our encode
    local back, pos = cm.decode(ty, golden)
    assert(pos == #golden + 1, "decode did not consume all bytes")
    local re = cm.encode(ty, back)
    if re ~= golden then
      error(("re-encode mismatch: got %s want %s"):format(bin2hex(re), vec.hex))
    end
  end)
  if ok then passed = passed + 1
  else
    failed = failed + 1
    print("FAIL [" .. vec.desc .. "] " .. tostring(err))
  end
end

print(string.format("%d/%d golden vectors passed", passed, passed + failed))
if failed == 0 then print("ALL_PASS") else print("FAILED") end
