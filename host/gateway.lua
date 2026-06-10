-- crabcraft gateway: the control plane (docs/WIRE.md section 3).
-- Owns the workload registry (desired state), reconciles placements onto
-- worker slots, and routes invoke traffic. Run on a CC computer with a modem:
--   gateway [name]            (default name "gateway")
local PROTO = "crabcraft"
local args = { ... }
local name = args[1] or (os.getComputerLabel and os.getComputerLabel()) or "gateway"

local opened = false
if type(peripheral) == "table" and peripheral.find then
  peripheral.find("modem", function(n) rednet.open(n); opened = true end)
end
if not opened then print("gateway: no modem attached."); return end
rednet.host(PROTO, name)

-- ---- state -------------------------------------------------------------------
local registry = {}   -- name -> { url, kind, schema }
local workers = {}    -- wid -> { label, slots = { [slot] = workload|false }, last }
local placements = {} -- name -> { worker = wid, slot = s, state = "assigning"|"running" }
local inflight = {}   -- reqid -> { from = senderid, t = clock }
local started = os.clock()

local function dlog(msg) print(("[%6ds] %s"):format(os.clock() - started, msg)) end

local function respond(to, reply, id)
  reply.id = id
  rednet.send(to, reply, PROTO)
end

local function find_placement_worker(wname)
  local p = placements[wname]
  return p and p.worker, p and p.slot
end

local function free_slot()
  for wid, w in pairs(workers) do
    if os.clock() - w.last < 20 then
      for slot, wl in pairs(w.slots) do
        if wl == false then return wid, slot end
      end
    end
  end
end

-- ---- scheduler (picatd-style dynamic coroutines) ------------------------------
local tasks = {}
local function spawn(fn) tasks[#tasks + 1] = { co = coroutine.create(fn) } end

local function reconcile()
  -- drop placements on dead workers
  for wname, p in pairs(placements) do
    local w = workers[p.worker]
    if not w or os.clock() - w.last > 20 then
      dlog(("reconcile: worker %s lost - unplacing '%s'"):format(tostring(p.worker), wname))
      placements[wname] = nil
    end
  end
  -- place unplaced workloads
  for wname, spec in pairs(registry) do
    if not placements[wname] then
      local wid, slot = free_slot()
      if wid then
        placements[wname] = { worker = wid, slot = slot, state = "assigning" }
        workers[wid].slots[slot] = wname -- optimistic; heartbeat confirms
        dlog(("reconcile: assigning '%s' -> worker %d slot %s"):format(wname, wid, slot))
        rednet.send(wid, { type = "assign", slot = slot, name = wname,
          url = spec.url, kind = spec.kind }, PROTO)
      end
    end
  end
  -- expire stale inflight entries
  for id, e in pairs(inflight) do
    if os.clock() - e.t > 120 then
      respond(e.from, { ok = false, err = "invoke timed out in gateway" }, id)
      inflight[id] = nil
    end
  end
end

local function handle(sender, msg)
  local t = msg.type
  if t == "ping" then
    respond(sender, { ok = true, output = name }, msg.id)
  elseif t == "register" then
    local slots = {}
    for _, s in ipairs(msg.slots or {}) do slots[s.disk] = s.workload or false end
    workers[sender] = { label = msg.worker, slots = slots, last = os.clock() }
    dlog(("worker %d (%s) registered with %d slot(s)"):format(sender, tostring(msg.worker), #(msg.slots or {})))
    -- adopt existing placements (worker reboot recovery)
    for _, s in ipairs(msg.slots or {}) do
      if s.workload and registry[s.workload] and not placements[s.workload] then
        placements[s.workload] = { worker = sender, slot = s.disk, state = "running" }
        dlog(("adopted '%s' on worker %d"):format(s.workload, sender))
      end
    end
    respond(sender, { ok = true }, msg.id)
  elseif t == "heartbeat" then
    local w = workers[sender]
    if w then
      w.last = os.clock()
      for _, s in ipairs(msg.slots or {}) do
        w.slots[s.disk] = s.workload or false
        local p = s.workload and placements[s.workload]
        if p and p.worker == sender then p.state = s.state or "running" end
      end
    end
  elseif t == "deploy" then
    if not msg.name or not msg.url then
      respond(sender, { ok = false, err = "deploy needs name and url" }, msg.id)
      return
    end
    registry[msg.name] = { url = msg.url, kind = msg.kind or "reactor", schema = msg.schema }
    dlog(("deploy '%s' (%s) registered"):format(msg.name, msg.kind or "reactor"))
    respond(sender, { ok = true, output = "registered " .. msg.name }, msg.id)
    reconcile()
  elseif t == "remove" then
    local p = placements[msg.name]
    if p then
      rednet.send(p.worker, { type = "drain", slot = p.slot }, PROTO)
      if workers[p.worker] then workers[p.worker].slots[p.slot] = false end
    end
    registry[msg.name] = nil
    placements[msg.name] = nil
    respond(sender, { ok = true, output = "removed " .. tostring(msg.name) }, msg.id)
  elseif t == "list" then
    local out = {}
    for wname, spec in pairs(registry) do
      local p = placements[wname]
      out[#out + 1] = { name = wname, kind = spec.kind, url = spec.url,
        worker = p and p.worker, slot = p and p.slot, state = p and p.state or "pending" }
    end
    local ws = {}
    for wid, w in pairs(workers) do
      local free = 0
      for _, wl in pairs(w.slots) do if wl == false then free = free + 1 end end
      ws[#ws + 1] = { id = wid, label = w.label, free = free,
        alive = os.clock() - w.last < 20 }
    end
    respond(sender, { ok = true, workloads = out, workers = ws }, msg.id)
  elseif t == "schema" then
    local spec = registry[msg.name]
    if not spec then respond(sender, { ok = false, err = "no workload " .. tostring(msg.name) }, msg.id)
    else respond(sender, { ok = true, schema = spec.schema, kind = spec.kind }, msg.id) end
  elseif t == "invoke" then
    local wid = find_placement_worker(msg.name)
    local p = placements[msg.name]
    if not wid or (p and p.state ~= "running") then
      respond(sender, { ok = false, err = "workload '" .. tostring(msg.name) ..
        "' is not running (state: " .. tostring(p and p.state or "absent") .. ")" }, msg.id)
      return
    end
    inflight[msg.id] = { from = sender, t = os.clock() }
    rednet.send(wid, { type = "invoke", id = msg.id, name = msg.name,
      func = msg.func, params = msg.params, body = msg.body }, PROTO)
  elseif t == "invoke-reply" then
    local e = inflight[msg.id]
    if e then
      inflight[msg.id] = nil
      respond(e.from, { ok = msg.ok, result = msg.result, err = msg.err }, msg.id)
    end
  end
end

spawn(function()
  while true do
    local sender, msg = rednet.receive(PROTO)
    if type(msg) == "table" then
      local ok, err = pcall(handle, sender, msg)
      if not ok then
        dlog("handler error: " .. tostring(err))
        if msg.id then respond(sender, { ok = false, err = "gateway error: " .. tostring(err) }, msg.id) end
      end
    end
  end
end)

spawn(function()
  while true do
    local timer = os.startTimer(5)
    repeat local ev, p = os.pullEvent("timer") until p == timer
    local ok, err = pcall(reconcile)
    if not ok then dlog("reconcile error: " .. tostring(err)) end
  end
end)

print(("gateway '%s' up on protocol '%s' - control loop every 5s"):format(name, PROTO))
local ev = {}
while true do
  for i = #tasks, 1, -1 do
    local t = tasks[i]
    if t.filter == nil or t.filter == ev[1] or ev[1] == "terminate" then
      local ok, f = coroutine.resume(t.co, (table.unpack or unpack)(ev))
      if not ok then print("gateway task error: " .. tostring(f)); table.remove(tasks, i)
      elseif coroutine.status(t.co) == "dead" then table.remove(tasks, i)
      else t.filter = f end
    end
  end
  ev = { os.pullEventRaw() }
  if ev[1] == "terminate" then print("gateway: stopped.") return end
end
