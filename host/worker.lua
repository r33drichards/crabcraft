-- crabcraft worker: the data plane (docs/WIRE.md section 3).
-- A computer with disks; one workload per disk (the disk = the volume).
--   worker [gatewayName]      (default: first crabcraft host found)
-- Needs beside it: runtime.lua, cmval.lua, json.lua, and the wasmcraft bundle.
local PROTO = "crabcraft"
local args = { ... }
if args[1] == "--install" and type(fs) == "table" then
  table.remove(args, 1)
  local rest = ""
  for _, a in ipairs(args) do rest = rest .. ', "' .. a .. '"' end
  local h = fs.open("startup.lua", "w")
  h.write('shell.run("worker"' .. rest .. ')\n')
  h.close()
  print("worker: installed to startup.lua")
end
local gwname = args[1]
if gwname == "--slots" then gwname = nil end

package.path = "host/?.lua;./?.lua;" .. package.path
local rt = require("runtime")
local cm = require("cmval")
local json = require("json")

local label = (os.getComputerLabel and os.getComputerLabel())
  or ("worker-" .. tostring(os.getComputerID and os.getComputerID() or 0))

local opened = false
if type(peripheral) == "table" and peripheral.find then
  peripheral.find("modem", function(n) rednet.open(n); opened = true end)
end
if not opened then print("worker: no modem attached."); return end

-- ---- find the gateway ---------------------------------------------------------
-- picatd lesson: busy computers answer dns lookups too slowly for the window;
-- cache the id after first contact and ping it directly on later boots.
local gw
do
  local f = io.open(".crab_gateway", "r")
  local cached = f and tonumber(f:read("*a")); if f then f:close() end
  if cached then
    rednet.send(cached, { type = "ping", id = "wkr:ping" }, PROTO)
    local t = os.clock()
    while os.clock() - t < 10 do
      local s, r = rednet.receive(PROTO, 10 - (os.clock() - t))
      if s == cached and type(r) == "table" and r.id == "wkr:ping" then gw = cached; break end
    end
    if not gw then print("worker: cached gateway #" .. cached .. " silent - rediscovering") end
  end
end
if not gw then
  for attempt = 1, 6 do
    if gwname then gw = rednet.lookup(PROTO, gwname, 5)
    else local hosts = { rednet.lookup(PROTO, nil, 5) }; gw = hosts[1] end
    if gw then break end
    print("worker: no gateway answered lookup " .. attempt .. "/6")
  end
end
if not gw then print("worker: no gateway on the network."); return end
do local f = io.open(".crab_gateway", "w"); if f then f:write(tostring(gw)); f:close() end end
print("worker: gateway is #" .. gw)

-- ---- slots: one workload per disk ----------------------------------------------
-- Real CC: every mounted disk drive is a slot. Fallback (sim / no drives):
-- local directories slot1..slotN (N = 1, or --slots n).
local function fexists(p)
  if type(fs) == "table" then return fs.exists(p) end
  local f = io.open(p, "r"); if f then f:close(); return true end
  local d = io.open(p .. "/.keep", "r"); if d then d:close(); return true end
  return false
end
local function mkdir(p)
  if type(fs) == "table" then fs.makeDir(p)
  elseif os.execute then os.execute("mkdir -p '" .. p .. "'") end
end
local function readfile(p)
  if type(fs) == "table" and fs.open then
    if not fs.exists(p) then return nil end
    local h = fs.open(p, "rb"); local d = h.readAll(); h.close(); return d
  end
  local f = io.open(p, "rb"); if not f then return nil end
  local d = f:read("*a"); f:close(); return d
end
local function writefile(p, d)
  if type(fs) == "table" and fs.open then
    local h = fs.open(p, "wb"); h.write(d); h.close(); return
  end
  local f = assert(io.open(p, "wb")); f:write(d); f:close()
end
local function delfile(p)
  if type(fs) == "table" then pcall(fs.delete, p) else pcall(os.remove, p) end
end

local slots = {} -- slotname -> { dir, meta = {name,kind,url}|nil, w = reactor|nil, module = decoded|nil, queue = {} }
do
  local dirs = {}
  if type(peripheral) == "table" and peripheral.getNames then
    for _, pname in ipairs(peripheral.getNames()) do
      if peripheral.getType(pname) == "drive" then
        local mp = peripheral.call(pname, "getMountPath")
        if mp then dirs[#dirs + 1] = "/" .. mp end
      end
    end
  end
  if #dirs == 0 then
    local n = 1
    for i = 1, #args do if args[i] == "--slots" then n = tonumber(args[i + 1]) or 1 end end
    for i = 1, n do
      dirs[#dirs + 1] = "slot" .. i
      mkdir("slot" .. i)
    end
  end
  for _, d in ipairs(dirs) do
    slots[d] = { dir = d, queue = {} }
    local meta = readfile(d .. "/crab-meta.json")
    if meta then
      local ok, m = pcall(json.decode, meta)
      if ok then slots[d].meta = m end
    end
  end
end

local function slot_list()
  local out = {}
  for sname, s in pairs(slots) do
    out[#out + 1] = { disk = sname, workload = s.meta and s.meta.name or nil,
      state = s.meta and (s.w and "running" or (s.meta.kind == "command" and s.module and "running" or "loading")) or nil }
  end
  return out
end

-- ---- mesh: guests calling other workloads through the gateway -------------------
local mesh_seq = 0
local mesh_replies = {}
local function mesh_call(target, func, params)
  mesh_seq = mesh_seq + 1
  local id = "mesh:" .. tostring(os.getComputerID and os.getComputerID() or 0) .. ":" .. mesh_seq
  rednet.send(gw, { type = "invoke", id = id, name = target, func = func, params = params }, PROTO)
  local deadline = os.clock() + 60
  while os.clock() < deadline do
    if mesh_replies[id] ~= nil then
      local r = mesh_replies[id]
      mesh_replies[id] = nil
      if r.ok then return true, r.result or "" end
      return false, r.err or "mesh call failed"
    end
    os.pullEvent("crab_mesh")
  end
  return false, "mesh call timed out: " .. target
end

-- ---- workload lifecycle ----------------------------------------------------------
local function start_slot(sname)
  local s = slots[sname]
  if not s.meta then return end
  local wasm = readfile(s.dir .. "/workload.wasm")
  if not wasm then print("worker: slot " .. sname .. " missing workload.wasm"); s.meta = nil; return end
  if s.meta.kind == "command" then
    s.module = rt.engine().load(wasm) -- decode once; _start per invoke
    print(("worker: '%s' (command) ready in %s"):format(s.meta.name, sname))
  else
    local t0 = os.clock()
    s.w = rt.load_reactor(wasm, { mode = "transpile", root = s.dir, name = s.meta.name,
      call = mesh_call })
    print(("worker: '%s' (reactor) warm in %s (%.1fs)"):format(s.meta.name, sname, os.clock() - t0))
  end
end

local function assign(msg)
  local s = slots[msg.slot]
  if not s then return { ok = false, err = "no slot " .. tostring(msg.slot) } end
  local wasm
  local file = msg.url:match("^file:(.+)$")
  if file then
    wasm = readfile(file)
    if not wasm then return { ok = false, err = "file not found: " .. file } end
  else
    if not http then return { ok = false, err = "no http on this worker" } end
    local r, herr = http.get(msg.url, nil, true) -- binary
    if not r then return { ok = false, err = "fetch failed: " .. tostring(herr) } end
    wasm = r.readAll()
    r.close()
  end
  writefile(s.dir .. "/workload.wasm", wasm)
  s.meta = { name = msg.name, kind = msg.kind or "reactor", url = msg.url }
  writefile(s.dir .. "/crab-meta.json", json.encode(s.meta))
  s.w, s.module = nil, nil
  local ok, err = pcall(start_slot, msg.slot)
  if not ok then return { ok = false, err = "start failed: " .. tostring(err) } end
  return { ok = true }
end

local function drain(msg)
  local s = slots[msg.slot]
  if s then
    s.meta, s.w, s.module = nil, nil, nil
    delfile(s.dir .. "/crab-meta.json")
    delfile(s.dir .. "/workload.wasm")
    print("worker: drained " .. msg.slot)
  end
end

local function find_workload(name)
  for sname, s in pairs(slots) do
    if s.meta and s.meta.name == name then return s end
  end
end

local function do_invoke(msg)
  local s = find_workload(msg.name)
  if not s then return { ok = false, err = "workload not here: " .. tostring(msg.name) } end
  if s.meta.kind == "command" then
    local out, err = rt.run_command(nil, msg.body or "", { module = s.module, root = s.dir,
      name = s.meta.name, mode = "transpile" })
    if not out then return { ok = false, err = err } end
    return { ok = true, result = out }
  end
  if not s.w then return { ok = false, err = "workload still loading" } end
  local r = s.w:invoke(msg.func, msg.params or "")
  return { ok = r.ok, result = r.result, err = r.err }
end

-- ---- tasks ----------------------------------------------------------------------
local tasks = {}
local function spawn(fn) tasks[#tasks + 1] = { co = coroutine.create(fn) } end
local jobs = {} -- queued assigns/invokes

spawn(function() -- receiver
  while true do
    local sender, msg = rednet.receive(PROTO)
    if type(msg) == "table" then
      if msg.type == "invoke-reply" or (msg.id and tostring(msg.id):match("^mesh:") and msg.ok ~= nil) then
        mesh_replies[msg.id] = msg
        os.queueEvent("crab_mesh")
      elseif msg.type == "assign" or msg.type == "invoke" or msg.type == "drain" then
        jobs[#jobs + 1] = { sender = sender, msg = msg }
        os.queueEvent("crab_work")
      end
    end
  end
end)

spawn(function() -- worker loop
  -- boot recovery: restart whatever the disks already hold
  for sname, s in pairs(slots) do
    if s.meta then
      local ok, err = pcall(start_slot, sname)
      if not ok then print("worker: recover " .. sname .. " failed: " .. tostring(err)) end
    end
  end
  -- register
  rednet.send(gw, { type = "register", worker = label, slots = slot_list(), id = "reg" }, PROTO)
  while true do
    if #jobs == 0 then
      os.pullEvent("crab_work")
    else
      local job = table.remove(jobs, 1)
      local m = job.msg
      if m.type == "assign" then
        local r = assign(m)
        r.id = m.id
        rednet.send(job.sender, r, PROTO)
        rednet.send(gw, { type = "heartbeat", worker = label, slots = slot_list() }, PROTO)
      elseif m.type == "drain" then
        drain(m)
        rednet.send(gw, { type = "heartbeat", worker = label, slots = slot_list() }, PROTO)
      elseif m.type == "invoke" then
        local ok, r = pcall(do_invoke, m)
        if not ok then r = { ok = false, err = "worker error: " .. tostring(r) } end
        rednet.send(job.sender, { type = "invoke-reply", id = m.id,
          ok = r.ok, result = r.result, err = r.err }, PROTO)
      end
    end
  end
end)

spawn(function() -- heartbeat
  while true do
    local timer = os.startTimer(5)
    repeat local _, p = os.pullEvent("timer") until p == timer
    rednet.send(gw, { type = "heartbeat", worker = label, slots = slot_list() }, PROTO)
  end
end)

local nslots = 0
for _ in pairs(slots) do nslots = nslots + 1 end
print(("worker '%s' up: %d slot(s), gateway #%d"):format(label, nslots, gw))
local ev = {}
while true do
  for i = #tasks, 1, -1 do
    local t = tasks[i]
    if t.filter == nil or t.filter == ev[1] or ev[1] == "terminate" then
      local ok, f = coroutine.resume(t.co, (table.unpack or unpack)(ev))
      if not ok then print("worker task error: " .. tostring(f)); table.remove(tasks, i)
      elseif coroutine.status(t.co) == "dead" then table.remove(tasks, i)
      else t.filter = f end
    end
  end
  ev = { os.pullEventRaw() }
  if ev[1] == "terminate" then print("worker: stopped.") return end
end
