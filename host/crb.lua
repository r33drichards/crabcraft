-- crb — the crabcraft CLI.
--   crb deploy <manifest.yml> [gateway]    declarative deploy (WIRE.md sec 4)
--   crb ls [gateway]                       workloads + workers
--   crb invoke <name> <func> [json] [gw]   call a function (args as JSON)
--   crb schema <name> [gateway]            print a workload's interface
--   crb remove <name> [gateway]
-- Manifest:  name: hello / wasm: <url|file:path> / kind: reactor|command /
--            schema: <path-or-url to resolved-WIT json>   (reactor kind)
package.path = "host/?.lua;./?.lua;" .. package.path
local yaml = require("yaml")
local json = require("json")
local client = require("client")
local schema_mod = require("schema")
local cm = require("cmval")

local args = { ... }
local cmd = args[1]
if not cmd then
  print("usage: crb deploy <file.yml> | ls | invoke <name> <func> [json] | schema <name> | remove <name>")
  return
end

local function readfile(p)
  if type(fs) == "table" and fs.open then
    if not fs.exists(p) then return nil end
    local h = fs.open(p, "rb"); local d = h.readAll(); h.close(); return d
  end
  local f = io.open(p, "rb"); if not f then return nil end
  local d = f:read("*a"); f:close(); return d
end

local function fetch(pathOrUrl)
  if pathOrUrl:match("^https?://") then
    assert(http, "no http available to fetch " .. pathOrUrl)
    local r = assert(http.get(pathOrUrl), "fetch failed: " .. pathOrUrl)
    local d = r.readAll(); r.close(); return d
  end
  return assert(readfile(pathOrUrl), "file not found: " .. pathOrUrl)
end

if cmd == "deploy" then
  local file = assert(args[2], "crb deploy <file.yml>")
  local m = yaml.decode(assert(readfile(file), "manifest not found: " .. file))
  assert(m.name and (m.wasm or m.url), "manifest needs name + wasm")
  local spec = { name = m.name, wasm = m.wasm or m.url, kind = m.kind or "reactor", warm = m.warm }
  if m.schema then spec.schema = fetch(m.schema) end
  local C = client.connect(args[3])
  local r = C:deploy(spec)
  print(r.ok and ("deployed '" .. m.name .. "' (" .. spec.kind .. ")") or ("FAILED: " .. tostring(r.err)))

elseif cmd == "ls" then
  local C = client.connect(args[2])
  local r = C:list()
  if not r.ok then print("FAILED: " .. tostring(r.err)) return end
  print("WORKLOADS")
  for _, w in ipairs(r.workloads or {}) do
    print(("  %-12s %-8s %-9s worker=%s slot=%s"):format(w.name, w.kind or "?",
      w.state or "?", tostring(w.worker), tostring(w.slot)))
  end
  print("WORKERS")
  for _, w in ipairs(r.workers or {}) do
    print(("  #%-4d %-12s free-slots=%d %s"):format(w.id, tostring(w.label), w.free,
      w.alive and "alive" or "LOST"))
  end

elseif cmd == "schema" then
  local name = assert(args[2], "crb schema <name>")
  local C = client.connect(args[3])
  local sjson, err, kind = C:schema(name)
  if kind == "command" then print(name .. ": command kind (JSON in -> JSON out)") return end
  if not sjson then print("FAILED: " .. tostring(err)) return end
  local sc = schema_mod.load(sjson)
  for _, addr in ipairs(sc.list()) do
    local f = sc.functions[addr]
    local ps = {}
    for i, p in ipairs(f.params) do
      ps[i] = p.name .. ": " .. (type(p.type) == "string" and p.type or p.type.kind)
    end
    print(("%s(%s)%s"):format(addr, table.concat(ps, ", "),
      f.result and (" -> " .. (type(f.result) == "string" and f.result or f.result.kind)) or ""))
  end

elseif cmd == "invoke" then
  local name = assert(args[2], "crb invoke <name> <func> [json-args]")
  local func = assert(args[3], "crb invoke <name> <func> [json-args]")
  local argjson = args[4] or "{}"
  local C = client.connect(args[5])
  local w = C:workload(name)
  local argv = json.decode(argjson)
  local ok, res = pcall(function() return w[func](argv) end)
  if not ok then print("FAILED: " .. tostring(res)) return end
  if type(res) == "table" then print(json.encode(res)) else print(tostring(res)) end

elseif cmd == "remove" then
  local C = client.connect(args[3])
  local r = C:remove(assert(args[2], "crb remove <name>"))
  print(r.ok and r.output or ("FAILED: " .. tostring(r.err)))

else
  print("unknown command: " .. tostring(cmd))
end
