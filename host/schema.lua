-- Resolved-WIT JSON (wasm-tools component wit --json) -> cmval descriptors.
-- This is what makes clients DATA-DRIVEN: given a schema, we can encode params
-- and decode results for any function with zero codegen.
--   local sc = schema.load(json_string_or_table)
--   local fn = sc.functions["crab:hello/greeter@0.1.0#greet"]
--   fn.params  -> { {name=, type=<cmval descriptor>}, ... }
--   fn.result  -> <cmval descriptor> | nil
--   sc.list()  -> sorted function addresses
local json = require("json")
local M = {}

local function conv_type(resolved, t)
  if type(t) == "string" then return t end -- primitive name
  -- numeric index into the top-level types array (0-based)
  if type(t) == "number" then
    local entry = resolved.types[t + 1]
      or error("schema: dangling type index " .. tostring(t))
    return conv_type(resolved, entry.kind)
  end
  -- kind table forms
  if t.record then
    local fields = {}
    for i, f in ipairs(t.record.fields) do
      fields[i] = { name = f.name, type = conv_type(resolved, f.type) }
    end
    return { kind = "record", fields = fields }
  elseif t.option then
    return { kind = "option", elem = conv_type(resolved, t.option) }
  elseif t.list then
    return { kind = "list", elem = conv_type(resolved, t.list) }
  elseif t.tuple then
    local m = {}
    for i, mt in ipairs(t.tuple.types) do m[i] = conv_type(resolved, mt) end
    return { kind = "tuple", members = m }
  elseif t.variant then
    local cases = {}
    for i, c in ipairs(t.variant.cases) do
      cases[i] = { name = c.name, type = c.type and conv_type(resolved, c.type) or nil }
    end
    return { kind = "variant", cases = cases }
  elseif t.enum then
    local cases = {}
    for i, c in ipairs(t.enum.cases) do
      cases[i] = type(c) == "table" and c.name or c
    end
    return { kind = "enum", cases = cases }
  elseif t.result then
    return { kind = "result",
      ok = t.result.ok and conv_type(resolved, t.result.ok) or nil,
      err = t.result.err and conv_type(resolved, t.result.err) or nil }
  elseif t.flags then
    local names = {}
    for i, fl in ipairs(t.flags.flags) do
      names[i] = type(fl) == "table" and fl.name or fl
    end
    return { kind = "flags", names = names }
  elseif t.type then -- type alias
    return conv_type(resolved, t.type)
  end
  error("schema: unsupported type kind: " .. json.encode(t))
end

function M.load(src)
  local resolved = type(src) == "string" and json.decode(src) or src
  local sc = { functions = {}, raw = resolved }
  for ifidx, iface in ipairs(resolved.interfaces or {}) do
    -- instance address: package name + / + interface name (pkg carries @version)
    local pkg = resolved.packages[(iface.package or 0) + 1]
    local pkgname = pkg and pkg.name or "unknown:pkg"
    -- wasm-tools may render the version inside the package name already
    local base, ver = pkgname:match("^(.-)@(.+)$")
    local instance
    if base then instance = base .. "/" .. iface.name .. "@" .. ver
    else instance = pkgname .. "/" .. iface.name end
    for fname, f in pairs(iface.functions or {}) do
      local params = {}
      for i, p in ipairs(f.params or {}) do
        params[i] = { name = p.name, type = conv_type(resolved, p.type) }
      end
      local result = nil
      if f.result ~= nil then result = conv_type(resolved, f.result) end
      local addr = instance .. "#" .. fname
      sc.functions[addr] = {
        addr = addr, instance = instance, name = fname,
        params = params, result = result,
      }
    end
  end
  function sc.list()
    local out = {}
    for addr in pairs(sc.functions) do out[#out + 1] = addr end
    table.sort(out)
    return out
  end
  -- param types as a plain array (for cmval.encode_params)
  function sc.param_types(addr)
    local f = sc.functions[addr] or error("schema: no function " .. addr)
    local t = {}
    for i, p in ipairs(f.params) do t[i] = p.type end
    return t
  end
  return sc
end

return M
