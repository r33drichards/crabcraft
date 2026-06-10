-- crabcraft gateway: the control plane (docs/WIRE.md section 3).
-- Owns the workload registry (desired state), reconciles placements onto
-- worker slots, and routes invoke traffic. Run on a CC computer with a modem:
--   gateway [name]            (default name "gateway")
local PROTO = "crabcraft"
local args = { ... }
-- --install: relaunch on every boot (daemon computers reboot on chunk unload)
if args[1] == "--install" and type(fs) == "table" then
  local h = fs.open("startup.lua", "w")
  h.write('shell.run("gateway"' .. (args[2] and (', "' .. args[2] .. '"') or "") .. ')\n')
  h.close()
  print("gateway: installed to startup.lua")
  table.remove(args, 1)
end
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
local cooldown = {}   -- wname -> { [wid] = clock of last assign failure }
local started = os.clock()

-- ---- monitor dashboard ---------------------------------------------------------
local mon = (type(peripheral) == "table" and peripheral.find) and peripheral.find("monitor") or nil
local LOG = {}
local draw -- fwd

local function dlog(msg)
  local line = ("[%6ds] %s"):format(os.clock() - started, msg)
  print(line)
  LOG[#LOG + 1] = line
  while #LOG > 60 do table.remove(LOG, 1) end
  if draw then pcall(draw) end
end

draw = function()
  if not mon then return end
  mon.setTextScale(0.5)
  local W, H = mon.getSize()
  local colour = mon.isColour and mon.isColour()
  local function c(fg) if colour then mon.setTextColour(fg) end end
  mon.setBackgroundColour(colours.black)
  mon.clear()
  local y = 1
  local function line(txt, fg)
    if y > H then return end
    mon.setCursorPos(1, y)
    c(fg or colours.white)
    mon.write(txt:sub(1, W))
    y = y + 1
  end
  local nworkers, inflight_n = 0, 0
  for _ in pairs(workers) do nworkers = nworkers + 1 end
  for _ in pairs(inflight) do inflight_n = inflight_n + 1 end
  line(("crabcraft gateway '%s'   up %ds   workers %d   inflight %d")
    :format(name, os.clock() - started, nworkers, inflight_n), colours.yellow)
  y = y + 1
  line("WORKLOADS", colours.lightBlue)
  local any = false
  for wname, spec in pairs(registry) do
    any = true
    local p = placements[wname]
    local state = p and (p.state or "?") or "pending"
    local fg = state == "running" and colours.lime
      or state == "assigning" and colours.yellow or colours.orange
    line(("  %-14s %-8s %-10s %s"):format(wname, spec.kind or "?", state,
      p and ("worker " .. p.worker .. " " .. p.slot) or "unscheduled"), fg)
  end
  if not any then line("  (none deployed - crb deploy <manifest.yml>)", colours.grey) end
  y = y + 1
  line("WORKERS", colours.lightBlue)
  any = false
  for wid, w in pairs(workers) do
    any = true
    local alive = os.clock() - w.last < 20
    local total, free = 0, 0
    for _, wl in pairs(w.slots) do
      total = total + 1
      if wl == false then free = free + 1 end
    end
    line(("  #%-4d %-12s %d/%d slots free   %s"):format(wid, tostring(w.label),
      free, total, alive and "alive" or "LOST"), alive and colours.lime or colours.red)
  end
  if not any then line("  (none registered - start worker computers)", colours.grey) end
  y = y + 1
  line(("LOG"):format(), colours.lightBlue)
  local room = H - y
  for i = math.max(1, #LOG - room + 1), #LOG do
    line("  " .. LOG[i], colours.grey)
  end
end

local function respond(to, reply, id)
  reply.id = id
  rednet.send(to, reply, PROTO)
end

local function find_placement_worker(wname)
  local p = placements[wname]
  return p and p.worker, p and p.slot
end

local function free_slot(wname)
  local cd = cooldown[wname] or {}
  for wid, w in pairs(workers) do
    if os.clock() - w.last < 20 and (not cd[wid] or os.clock() - cd[wid] > 60) then
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
  -- drop placements on dead workers, and assignments that never converged
  -- (e.g. the worker failed the fetch): the heartbeat is the source of truth
  for wname, p in pairs(placements) do
    local w = workers[p.worker]
    if not w or os.clock() - w.last > 20 then
      dlog(("reconcile: worker %s lost - unplacing '%s'"):format(tostring(p.worker), wname))
      placements[wname] = nil
    elseif p.state == "assigning" then
      p.age = (p.age or 0) + 1
      if p.age > 3 and w.slots[p.slot] ~= wname then
        dlog(("reconcile: '%s' never started on worker %d - retrying elsewhere"):format(wname, p.worker))
        cooldown[wname] = cooldown[wname] or {}
        cooldown[wname][p.worker] = os.clock()
        placements[wname] = nil
      end
    end
  end
  -- place unplaced workloads
  for wname, spec in pairs(registry) do
    if not placements[wname] then
      local wid, slot = free_slot(wname)
      if wid then
        placements[wname] = { worker = wid, slot = slot, state = "assigning" }
        workers[wid].slots[slot] = wname -- optimistic; heartbeat confirms
        dlog(("reconcile: assigning '%s' -> worker %d slot %s"):format(wname, wid, slot))
        rednet.send(wid, { type = "assign", slot = slot, name = wname,
          url = spec.url, kind = spec.kind, warm = spec.warm, id = "asg:" .. wname }, PROTO)
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
    if not w then
      -- gateway rebooted and lost its memory: adopt the worker from its
      -- heartbeat (picatd lesson: never depend on in-memory state surviving)
      local slots = {}
      for _, sl in ipairs(msg.slots or {}) do slots[sl.disk] = sl.workload or false end
      workers[sender] = { label = msg.worker, slots = slots, last = os.clock() }
      w = workers[sender]
      dlog(("adopted worker %d (%s) from heartbeat"):format(sender, tostring(msg.worker)))
      for _, sl in ipairs(msg.slots or {}) do
        if sl.workload and registry[sl.workload] and not placements[sl.workload] then
          placements[sl.workload] = { worker = sender, slot = sl.disk, state = sl.state or "running" }
        end
      end
    end
    if w then
      w.last = os.clock()
      for _, s in ipairs(msg.slots or {}) do
        w.slots[s.disk] = s.workload or false
        local p = s.workload and placements[s.workload]
        if p and p.worker == sender then p.state = s.state or "running" end
      end
      if draw then pcall(draw) end
    end
  elseif t == "deploy" then
    if not msg.name or not msg.url then
      respond(sender, { ok = false, err = "deploy needs name and url" }, msg.id)
      return
    end
    registry[msg.name] = { url = msg.url, kind = msg.kind or "reactor", schema = msg.schema, warm = msg.warm }
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
  elseif msg.id and tostring(msg.id):match("^asg:") and msg.ok ~= nil then
    local wname = tostring(msg.id):sub(5)
    if msg.ok == false then
      dlog(("assign '%s' FAILED on worker %d: %s"):format(wname, sender, tostring(msg.err)))
      cooldown[wname] = cooldown[wname] or {}
      cooldown[wname][sender] = os.clock()
      local p = placements[wname]
      if p and p.worker == sender then
        if workers[sender] then workers[sender].slots[p.slot] = false end
        placements[wname] = nil
      end
      reconcile()
    end
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
    if draw then pcall(draw) end
  end
end)

print(("gateway '%s' up on protocol '%s' - control loop every 5s"):format(name, PROTO))
if mon then dlog("dashboard on monitor") else print("(no monitor attached - dashboard off)") end
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
