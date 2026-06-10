-- crabcraft client library: THE FACTORY (docs/WIRE.md section 5).
-- Connect to a gateway, then get schema-driven proxies for any workload:
--   local crab = require("client").connect()         -- or .connect("gateway")
--   local hello = crab:workload("hello")
--   print(hello.greet({ name = "steve", excited = true }))  -- typed mesh call
-- Proxies are generated from the workload's resolved-WIT schema at runtime:
-- params are plain Lua tables validated/encoded per the schema; results are
-- decoded back to Lua values. No codegen.
local PROTO = "crabcraft"
package.path = "host/?.lua;./?.lua;" .. package.path
local cm = require("cmval")
local json = require("json")
local schema_mod = require("schema")

local M = {}

local function open_modems()
  local ok = false
  if type(peripheral) == "table" and peripheral.find then
    peripheral.find("modem", function(n) rednet.open(n); ok = true end)
  end
  return ok
end

function M.connect(gwname, opts)
  opts = opts or {}
  assert(open_modems(), "client: no modem attached")
  local gw
  for _ = 1, opts.attempts or 4 do
    if gwname then gw = rednet.lookup(PROTO, gwname, 5)
    else local hosts = { rednet.lookup(PROTO, nil, 5) }; gw = hosts[1] end
    if gw then break end
  end
  assert(gw, "client: no crabcraft gateway on the network")

  local C = { gw = gw, seq = 0 }

  function C:request(msg, timeout)
    self.seq = self.seq + 1
    msg.id = ("c%d:%d"):format(os.getComputerID and os.getComputerID() or 0, self.seq)
    rednet.send(self.gw, msg, PROTO)
    local deadline = os.clock() + (timeout or 60)
    while os.clock() < deadline do
      local s, r = rednet.receive(PROTO, math.max(0.1, deadline - os.clock()))
      if s == self.gw and type(r) == "table" and r.id == msg.id then
        if r.status then print("(" .. tostring(r.status) .. ")")
        else return r end
      end
    end
    return { ok = false, err = "timed out waiting for gateway" }
  end

  -- raw operations
  function C:list() return self:request({ type = "list" }) end
  function C:deploy(spec) return self:request({ type = "deploy", name = spec.name,
    url = spec.wasm or spec.url, kind = spec.kind, schema = spec.schema, warm = spec.warm }) end
  function C:remove(name) return self:request({ type = "remove", name = name }) end
  function C:schema(name)
    local r = self:request({ type = "schema", name = name })
    if not r.ok then return nil, r.err end
    return r.schema, nil, r.kind
  end

  -- the factory: a proxy whose methods are the workload's WIT functions
  function C:workload(name)
    local sjson, err, kind = self:schema(name)
    if kind == "command" then
      -- command kind: one callable taking/returning JSON-able tables
      return setmetatable({}, { __call = function(_, body)
        local r = self:request({ type = "invoke", name = name,
          body = type(body) == "string" and body or json.encode(body) }, 120)
        if not r.ok then error("invoke failed: " .. tostring(r.err), 0) end
        local ok, decoded = pcall(json.decode, r.result)
        return ok and decoded or r.result
      end })
    end
    assert(sjson, "no schema for workload '" .. name .. "': " .. tostring(err))
    local sc = schema_mod.load(sjson)
    local proxy = { __name = name, __schema = sc }
    -- short name -> full address (unambiguous short names only)
    local short = {}
    for addr in pairs(sc.functions) do
      local fname = addr:match("#(.+)$")
      short[fname] = short[fname] == nil and addr or false
    end
    setmetatable(proxy, { __index = function(_, fname)
      local addr = sc.functions[fname] and fname or short[fname]
      if addr == false then error("ambiguous function name '" .. fname .. "' - use the full address", 0) end
      if not addr then error("no function '" .. fname .. "' on workload '" .. name .. "'", 0) end
      local f = sc.functions[addr]
      return function(argtbl)
        argtbl = argtbl or {}
        local values = {}
        -- sugar: a function with ONE record param accepts the record's fields
        -- directly: hello.greet{ name = "x" } instead of { req = { name = "x" } }
        if #f.params == 1 and argtbl[f.params[1].name] == nil and argtbl[1] == nil then
          values[1] = argtbl
        else
          -- positional or named: named keys win, else array order
          for i, p in ipairs(f.params) do
            if argtbl[p.name] ~= nil then values[i] = argtbl[p.name]
            else values[i] = argtbl[i] end
          end
        end
        local bytes = cm.encode_params(sc.param_types(addr), values)
        local r = self:request({ type = "invoke", name = name, func = addr, params = bytes }, 120)
        if not r.ok then error("invoke failed: " .. tostring(r.err), 0) end
        if f.result then return (cm.decode(f.result, r.result or "")) end
        return true
      end
    end })
    return proxy
  end

  return C
end

return M
