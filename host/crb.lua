-- crb — the crabcraft CLI.
--   crb deploy <manifest.yml> [gateway]    declarative deploy (WIRE.md sec 4)
--   crb ls [gateway]                       workloads + workers
--   crb invoke <name> <func> [json] [gw]   call a function (args as JSON)
--   crb run <job> / crb logs <job>         jobs: trigger a run / run history
--   crb schema <name> [gateway]            print a workload's interface
--   crb remove <name> [gateway]
-- Manifest:  name: hello / wasm: <url|file:path> / kind: reactor|command|job /
--            schema: <path-or-url to resolved-WIT json>   (reactor + func jobs)
--            jobs add: schedule/func/args/body/retries/timeout/keep-warm
--            (WIRE.md section 6)
if package and package.path then package.path = "host/?.lua;./?.lua;" .. package.path end
local yaml = require("yaml")
local json = require("json")
local client = require("client")
local schema_mod = require("schema")
local cm = require("cmval")
local cron = require("cron")

local args = { ... }
-- -g <gateway> may appear anywhere (no trailing positional: the CC shell
-- splits on spaces, so values would get mistaken for a gateway name)
local GW = nil
local SESS = nil
for i = #args - 1, 1, -1 do
  if args[i] == "-g" then
    GW = args[i + 1]
    table.remove(args, i + 1); table.remove(args, i)
  elseif args[i] == "-s" then
    SESS = args[i + 1]
    table.remove(args, i + 1); table.remove(args, i)
  end
end
local cmd = args[1]
if not cmd then
  print("usage:")
  print("  crb deploy <file.yml>")
  print("  crb ls | schema <name> | rm <name> | purge | update")
  print("  crb invoke <name> <func> [args...]   (-s <session> for session kinds)")
  print("  crb run <job>                 (queue a run of a job now)")
  print("  crb logs <job> [n]            (a job's recent run history)")
  print("  crb reset <name> [-s <session>]")
  print("  crb gen <name> [outfile]      (generate a typed Lua client)")
  print("  (-g <gateway> anywhere to pick a gateway)")
  return
end

local function utcnow()
  if os.epoch then return math.floor(os.epoch("utc") / 1000) end
  return os.time()
end

-- key=value tokens -> argument table; values coerce json-ish
local function coerce(v)
  if v == "true" then return true end
  if v == "false" then return false end
  local n = tonumber(v)
  if n then return n end
  return v
end
local function kvargs(from)
  local t = {}
  for i = from, #args do
    local k, v = args[i]:match("^([%w_%-]+)=(.*)$")
    if k then t[k] = coerce(v)
    else t[#t + 1] = coerce(args[i]) end
  end
  return t
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

-- resolve a (possibly short) function name against a loaded schema
local function resolve_func(sc, fname)
  if sc.functions[fname] then return fname end
  local addr
  for a in pairs(sc.functions) do
    if a:match("#(.+)$") == fname then
      if addr then return nil, "ambiguous function '" .. fname .. "' - use the full address" end
      addr = a
    end
  end
  if not addr then return nil, "no function '" .. fname .. "' in the schema" end
  return addr
end

-- manifest args -> encoded params (same named/positional/single-record rules
-- as the client factory; bad args fail HERE, at deploy time)
local function encode_job_args(sc, addr, argtbl)
  local f = sc.functions[addr]
  argtbl = argtbl or {}
  local values = {}
  if #f.params == 1 and argtbl[f.params[1].name] == nil and argtbl[1] == nil then
    values[1] = argtbl
  else
    for i, p in ipairs(f.params) do
      if argtbl[p.name] ~= nil then values[i] = argtbl[p.name] else values[i] = argtbl[i] end
    end
  end
  return cm.encode_params(sc.param_types(addr), values)
end

if cmd == "deploy" then
  local file = assert(args[2], "crb deploy <file.yml>")
  local m = yaml.decode(assert(readfile(file), "manifest not found: " .. file))
  assert(m.name and (m.wasm or m.url), "manifest needs name + wasm")
  local spec = { name = m.name, wasm = m.wasm or m.url, kind = m.kind or "reactor", warm = m.warm, force = m.force, args = m.args, body_file = m["body-file"] }
  if m.schema then spec.schema = fetch(m.schema) end
  assert(m.schedule == nil or spec.kind == "job", "manifest: schedule needs kind: job")
  if spec.kind == "job" then
    if m.schedule then
      local parsed, perr = cron.parse(tostring(m.schedule))
      assert(parsed, "manifest schedule: " .. tostring(perr))
      spec.schedule = tostring(m.schedule)
    end
    spec.retries, spec.timeout, spec.keep = m.retries, m.timeout, m["keep-warm"]
    if m.func then
      -- a typed run: one function call on a reactor module, args encoded now
      spec.module = "reactor"
      assert(spec.schema, "manifest: a func job needs schema: to encode args")
      local sc = schema_mod.load(spec.schema)
      local addr, ferr = resolve_func(sc, m.func)
      assert(addr, "manifest func: " .. tostring(ferr))
      spec.func = addr
      spec.params = encode_job_args(sc, addr, m.args)
      spec.args = nil -- consumed as function args (argv is for command modules)
    else
      -- a command run: any wasi CLI, body on stdin (plus argv/body-file knobs)
      spec.module = "command"
      spec.body = m.body ~= nil and tostring(m.body) or nil
    end
  end
  local C = client.connect(GW)
  local r = C:deploy(spec)
  print(r.ok and (r.output or ("deployed '" .. m.name .. "' (" .. spec.kind .. ")"))
    or ("FAILED: " .. tostring(r.err)))

elseif cmd == "ls" then
  local C = client.connect(GW)
  local r = C:list()
  if not r.ok then print("FAILED: " .. tostring(r.err)) return end
  print("WORKLOADS")
  for _, w in ipairs(r.workloads or {}) do
    if w.kind == "job" then
      local last = "no runs"
      if w.last and w.last.t then
        last = (w.last.ok and "ok " or "ERR ") .. (utcnow() - w.last.t) .. "s ago"
      end
      print(("  %-12s %-8s %-9s runs=%-3d %s%s"):format(w.name, w.kind, w.state or "?",
        w.runs or 0, w.schedule and (w.schedule .. "  ") or "", last))
    else
      print(("  %-12s %-8s %-9s worker=%s slot=%s"):format(w.name, w.kind or "?",
        w.state or "?", tostring(w.worker), tostring(w.slot)))
    end
  end
  print("WORKERS")
  for _, w in ipairs(r.workers or {}) do
    print(("  #%-4d %-12s v%-7s free-slots=%d %s"):format(w.id, tostring(w.label),
      tostring(w.version or "?"), w.free, w.alive and "alive" or "LOST"))
  end

elseif cmd == "schema" then
  local name = assert(args[2], "crb schema <name>")
  local C = client.connect(GW)
  local sjson, err, kind = C:schema(name)
  if kind == "command" or kind == "session" then print(name .. ": " .. kind .. " kind (body in -> output out)") return end
  if kind == "job" and not sjson then print(name .. ": job (command module; body in -> output out)") return end
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

elseif cmd == "invoke" or cmd == "call" then
  -- GENERIC, schema-driven invocation. Argument forms, decided by the
  -- function's own WIT signature:
  --   exactly one string-typed param  -> the whole tail is that string
  --   anything else                   -> key=value pairs (or raw JSON)
  local name = assert(args[2], "crb invoke <name> <func> [args...]")
  local func = assert(args[3], "crb invoke <name> <func> [args...]")
  local from = 4
  local C = client.connect(GW)
  local w = C:workload(name)
  local argv
  local fdef = type(w) == "table" and w.__schema and (function()
    local addr = w.__schema.functions[func] and func
    if not addr then
      for a in pairs(w.__schema.functions) do
        if a:match("#(.+)$") == func then addr = a break end
      end
    end
    return addr and w.__schema.functions[addr]
  end)() or nil
  if args[from] and args[from]:sub(1, 1) == "{" then
    argv = json.decode(table.concat(args, " ", from)) -- raw JSON (rejoined)
  elseif fdef and #fdef.params == 1 and fdef.params[1].type == "string"
      and not (args[from] or ""):match("^[%w_%-]+=") then
    argv = { table.concat(args, " ", from) }          -- single string param: join tail
  elseif fdef and #fdef.params == 1 and type(fdef.params[1].type) == "table"
      and fdef.params[1].type.kind == "record"
      and #fdef.params[1].type.fields == 1 and fdef.params[1].type.fields[1].type == "string"
      and not (args[from] or ""):match("^[%w_%-]+=") then
    -- single record param with one string field: join tail into that field
    argv = { [fdef.params[1].type.fields[1].name] = table.concat(args, " ", from) }
  else
    argv = kvargs(from)                               -- key=value pairs
  end
  local ok, res = pcall(function()
    if fdef then return w[func](argv) end
    return w(argv, SESS) -- command/session kinds: the proxy itself is callable
  end)
  if not ok then print("FAILED: " .. tostring(res)) return end
  -- result rendering: unwrap result<ok,err>; tabulate {columns,rows} JSON
  if type(res) == "table" and res.is_err then
    print("ERROR: " .. tostring(res.err))
  elseif type(res) == "table" and res.is_ok then
    local body = res.ok
    local okj, parsed = pcall(json.decode, tostring(body))
    if okj and type(parsed) == "table" and parsed.columns then
      print(table.concat(parsed.columns, " | "))
      for _, row in ipairs(parsed.rows or {}) do
        local cells = {}
        for i, cell in ipairs(row) do cells[i] = tostring(cell) end
        print(table.concat(cells, " | "))
      end
      print(("(%s change(s))"):format(tostring(parsed.changes)))
    else
      print(tostring(body))
    end
  elseif type(res) == "table" then print(json.encode(res))
  else print(tostring(res)) end

elseif cmd == "gen" then
  -- crb gen <workload> [outfile]  -> a standalone typed Lua client module:
  --   local db = require("sqlite_client")
  --   print(db.exec("SELECT * FROM pets"))
  -- The module embeds the workload's schema; the runtime (crblib) is fetched
  -- from the release on first use.
  local name = assert(args[2], "crb gen <workload> [outfile]")
  local outfile = args[3] or (name .. "_client.lua")
  local C = client.connect(GW)
  local sjson, err, kind = C:schema(name)
  local LIBURL = "https://github.com/r33drichards/crabcraft/releases/latest/download/crblib.lua"
  local out = {}
  local function emit(l) out[#out + 1] = l end
  emit(("-- %s: generated by `crb gen %s` - a typed client for the '%s' workload."):format(outfile, name, name))
  emit("-- Usage:  local w = require(" .. string.format("%q", outfile:gsub("%.lua$", "")) .. ")")
  emit("local LIBURL = " .. string.format("%q", LIBURL))
  emit("local function lib()")
  emit("  if not _G.__crblib then")
  emit("    if not fs.exists(\"crblib\") then")
  emit("      local r = assert(http.get(LIBURL), \"cannot fetch crblib\")")
  emit("      local h = fs.open(\"crblib\", \"w\") h.write(r.readAll()) h.close() r.close()")
  emit("    end")
  emit("    _G.__crblib = dofile(\"crblib\")")
  emit("  end")
  emit("  return _G.__crblib")
  emit("end")
  emit("local NAME, KIND = " .. string.format("%q", name) .. ", " .. string.format("%q", kind or "reactor"))
  emit("local SCHEMA = " .. (sjson and string.format("%q", sjson) or "nil"))
  emit("local proxy")
  emit("local function ensure()")
  emit("  if not proxy then proxy = lib().client.connect():workload_from_schema(NAME, SCHEMA, KIND) end")
  emit("  return proxy")
  emit("end")
  emit("local M = {}")
  if kind == "command" or kind == "session" then
    emit("-- " .. kind .. " kind: one callable - body in (string or JSON-able table),")
    emit("-- decoded reply / raw output out")
    emit("setmetatable(M, { __call = function(_, body) return ensure()(body) end })")
  else
    assert(sjson, "no schema for workload '" .. name .. "': " .. tostring(err))
    local sc = schema_mod.load(sjson)
    for _, addr in ipairs(sc.list()) do
      local f = sc.functions[addr]
      local fname = addr:match("#(.+)$")
      local ps = {}
      for i, p in ipairs(f.params) do
        ps[i] = p.name .. ": " .. (type(p.type) == "string" and p.type or p.type.kind)
      end
      emit(("-- %s(%s)%s"):format(fname, table.concat(ps, ", "),
        f.result and (" -> " .. (type(f.result) == "string" and f.result or f.result.kind)) or ""))
      local lua_name = fname:gsub("%-", "_")
      if #f.params == 1 and f.params[1].type == "string" then
        emit(("M[%q] = function(s) return ensure()[%q]({ s }) end"):format(lua_name, addr))
      else
        emit(("M[%q] = function(a) return ensure()[%q](a or {}) end"):format(lua_name, addr))
      end
    end
  end
  emit("return M")
  local h
  if type(fs) == "table" and fs.open then h = fs.open(outfile, "w") h.write(table.concat(out, "\n")) h.close()
  else local f2 = assert(io.open(outfile, "w")) f2:write(table.concat(out, "\n")) f2:close() end
  print(("wrote %s (%d lines) - require(%q)"):format(outfile, #out, (outfile:gsub("%.lua$", ""))))

elseif cmd == "run" then
  -- crb run <job>: queue a run now (works for scheduled jobs too, like
  -- `kubectl create job --from=cronjob/x`)
  local name = assert(args[2], "crb run <job>")
  local C = client.connect(GW)
  local r = C:run(name)
  print(r.ok and r.output or ("FAILED: " .. tostring(r.err)))

elseif cmd == "logs" then
  -- crb logs <job> [n]: the last n completed runs (default all kept), oldest
  -- first; func-job outputs are decoded through the schema
  local name = assert(args[2], "crb logs <job> [n]")
  local C = client.connect(GW)
  local r = C:job_logs(name)
  if not r.ok then print("FAILED: " .. tostring(r.err)) return end
  local runs = r.runs or {}
  local from = tonumber(args[3]) and math.max(1, #runs - tonumber(args[3]) + 1) or 1
  local rdesc
  if r.module == "reactor" and r.func then
    local sjson = C:schema(name)
    if sjson then
      local okl, sc = pcall(schema_mod.load, sjson)
      if okl and sc.functions[r.func] then rdesc = sc.functions[r.func].result end
    end
  end
  for i = from, #runs do
    local e = runs[i]
    print(("run #%d  %-6s %ss ago%s%s"):format(e.n, e.ok and "ok" or "FAILED",
      e.t and (utcnow() - e.t) or "?",
      e.dur and ("  %.1fs"):format(e.dur) or "",
      (e.tries or 0) > 0 and ("  retries=" .. e.tries) or ""))
    if e.ok then
      local out = e.output
      if rdesc and out then
        local okd, v = pcall(cm.decode, rdesc, out)
        if okd then out = type(v) == "table" and json.encode(v) or tostring(v) end
      end
      if out ~= nil and #tostring(out) > 0 then print("  " .. tostring(out)) end
    else
      print("  " .. tostring(e.err))
    end
  end
  if #runs == 0 then print("no completed runs") end
  if r.cur then print(("(run #%d %s)"):format(r.cur.n, r.cur.phase)) end

elseif cmd == "remove" or cmd == "rm" or cmd == "del" or cmd == "delete" then
  local C = client.connect(GW)
  local r = C:remove(assert(args[2], "crb rm <name>"))
  print(r.ok and r.output or ("FAILED: " .. tostring(r.err)))

elseif cmd == "update" then
  -- crb update            -> roll out latest worker.lua to all workers, then
  --                          the gateway updates itself (reboots)
  -- crb update workers    -> workers only
  local what = args[2] or "all"
  local C = client.connect(GW)
  if what == "all" or what == "workers" then
    local r = C:request({ type = "update-workers" })
    print(r.ok and r.output or ("FAILED: " .. tostring(r.err)))
  end
  if what == "all" or what == "gateway" then
    local r = C:request({ type = "update-gateway" }, 30)
    print(r.ok and r.output or ("FAILED: " .. tostring(r.err)))
  end

elseif cmd == "reset" then
  -- crb reset picat [-s foo]   discard a named session's state
  local name = assert(args[2], "crb reset <workload> [-s session]")
  local C = client.connect(GW)
  local w = C:workload(name)
  assert(w.reset, "workload '" .. name .. "' is not a session kind")
  print(w.reset(SESS))

elseif cmd == "purge" then
  local C = client.connect(GW)
  local r = C:request({ type = "purge" })
  print(r.ok and r.output or ("FAILED: " .. tostring(r.err)))

else
  print("unknown command: " .. tostring(cmd))
end
