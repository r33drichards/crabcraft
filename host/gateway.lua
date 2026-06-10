-- crabcraft gateway: the control plane (docs/WIRE.md section 3).
-- Owns the workload registry (desired state), reconciles placements onto
-- worker slots, and routes invoke traffic. Run on a CC computer with a modem:
--   gateway [name]            (default name "gateway")
local PROTO = "crabcraft"
local CRAB_VERSION = "dev" -- stamped by tools/amalgamate.py
local GATEWAY_URL = "https://github.com/r33drichards/crabcraft/releases/latest/download/gateway.lua"
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
local registry = {}   -- name -> { url, kind, schema }  (persisted to disk)
local REGFILE = ".crab_registry"
local function save_registry()
  if type(fs) ~= "table" or not textutils then return end
  local h = fs.open(REGFILE, "w")
  if h then h.write(textutils.serialize(registry)); h.close() end
end
local function load_registry()
  if type(fs) ~= "table" or not fs.exists(REGFILE) then return end
  local h = fs.open(REGFILE, "r")
  if not h then return end
  local ok, t = pcall(textutils.unserialize, h.readAll())
  h.close()
  if ok and type(t) == "table" then registry = t end
end
load_registry()
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
  -- adaptive scale: biggest text that still fits the content
  local needed = 8 + 4 -- headers/spacing + log tail
  for _ in pairs(registry) do needed = needed + 1 end
  for _, w in pairs(workers) do
    needed = needed + 1
    for _ in pairs(w.slots) do needed = needed + 1 end
  end
  local W, H
  for _, scale in ipairs({ 5, 4.5, 4, 3.5, 3, 2.5, 2, 1.5, 1, 0.5 }) do
    mon.setTextScale(scale)
    W, H = mon.getSize()
    if (H >= needed and W >= 46) or scale == 0.5 then break end
  end
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
  local nworkers, inflight_n, used_total = 0, 0, 0
  for _, w in pairs(workers) do
    nworkers = nworkers + 1
    for _, u in pairs(w.used or {}) do used_total = used_total + (u or 0) end
  end
  for _ in pairs(inflight) do inflight_n = inflight_n + 1 end
  line(("crabcraft gateway '%s' v%s   up %ds   workers %d   inflight %d   storage %.1f MB")
    :format(name, CRAB_VERSION, os.clock() - started, nworkers, inflight_n,
      used_total / 1048576), colours.yellow)
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
    line(("  #%-4d %-12s v%-7s %d/%d free  %s"):format(wid, tostring(w.label),
      tostring(w.version or "?"), free, total, alive and "alive" or "LOST"),
      alive and colours.lime or colours.red)
    for slot, wl in pairs(w.slots) do
      local mb = ((w.used or {})[slot] or 0) / 1048576
      if wl == false then
        line(("    %-8s (free)  %.1fMB"):format(slot, mb), colours.grey)
      else
        local spec = registry[wl]
        local wasm = spec and spec.url and (spec.url:match("([^/]+)$") or spec.url) or "?"
        line(("    %-8s %-12s %-16s %.1fMB"):format(slot, tostring(wl), wasm, mb),
          alive and colours.white or colours.red)
      end
    end
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
    elseif (p.hbm or 0) >= 2 then
      -- two consecutive heartbeats contradicted this placement: it is gone
      -- (heartbeats are the freshness signal - reconcile ticks are not)
      dlog(("reconcile: '%s' contradicted by worker %d heartbeats - retrying"):format(wname, p.worker))
      cooldown[wname] = cooldown[wname] or {}
      cooldown[wname][p.worker] = os.clock()
      placements[wname] = nil
    end
  end
  -- adopt orphans: a slot already running this workload (e.g. from a disk
  -- after reboots) beats assigning a fresh copy elsewhere
  for wname, spec in pairs(registry) do
    if not placements[wname] then
      for wid, w in pairs(workers) do
        if os.clock() - w.last < 20 then
          for slot, wl in pairs(w.slots) do
            if wl == wname and (w.states or {})[slot] == "running" then
              placements[wname] = { worker = wid, slot = slot, state = "running" }
              dlog(("reconcile: adopted running '%s' on worker %d %s"):format(wname, wid, slot))
              break
            end
          end
        end
        if placements[wname] then break end
      end
    end
  end
  -- GC: drain slots running workloads not in the registry, AND duplicate
  -- copies of registered workloads that are not the placement (one workload,
  -- one slot - duplicates appear after failed-assign/recovery races)
  for wid, w in pairs(workers) do
    if os.clock() - w.last < 20 then
      for slot, wl in pairs(w.slots) do
        if wl ~= false then
          local placed = placements[wl]
          local is_placement = placed and placed.worker == wid and placed.slot == slot
          if not is_placement and (not registry[wl] or placed) then
            dlog(("reconcile: draining %s '%s' on worker %d %s"):format(
              registry[wl] and "duplicate" or "unknown", tostring(wl), wid, slot))
            rednet.send(wid, { type = "drain", slot = slot }, PROTO)
            w.slots[slot] = false
          end
        end
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
          url = spec.url, kind = spec.kind, warm = spec.warm,
          args = spec.args, body_file = spec.body_file, id = "asg:" .. wname }, PROTO)
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
    local used = {}
    for _, s in ipairs(msg.slots or {}) do
      slots[s.disk] = s.workload or false
      used[s.disk] = s.used
    end
    workers[sender] = { label = msg.worker, slots = slots, used = used, last = os.clock(), version = msg.version }
    dlog(("worker %d (%s) registered with %d slot(s)"):format(sender, tostring(msg.worker), #(msg.slots or {})))
    -- adopt existing placements (worker reboot recovery; running slots only)
    for _, s in ipairs(msg.slots or {}) do
      if s.workload and s.state == "running" and registry[s.workload] and not placements[s.workload] then
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
      workers[sender] = { label = msg.worker, slots = slots, last = os.clock(), version = msg.version }
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
      if msg.version then w.version = msg.version end
      w.states = w.states or {}
      w.used = w.used or {}
      for _, s in ipairs(msg.slots or {}) do
        w.slots[s.disk] = s.workload or false
        w.states[s.disk] = s.state
        w.used[s.disk] = s.used
      end
      for wname, p in pairs(placements) do
        if p.worker == sender then
          local slotw = w.slots[p.slot]
          local slots_state = w.states[p.slot]
          if slotw == wname and slots_state == "running" then
            p.state = "running"
            p.hbm = 0
          elseif slotw == wname then
            p.state = slots_state or p.state -- loading: hold, neither way
          else
            -- this heartbeat contradicts the placement (slot empty/other)
            p.hbm = (p.hbm or 0) + 1
          end
        end
      end
      if draw then pcall(draw) end
    end
  elseif t == "deploy" then
    if not msg.name or not msg.url then
      respond(sender, { ok = false, err = "deploy needs name and url" }, msg.id)
      return
    end
    if registry[msg.name] and not msg.force then
      respond(sender, { ok = false, err = "workload '" .. msg.name ..
        "' already deployed - crb remove " .. msg.name .. " first (or deploy force=true)" }, msg.id)
      return
    end
    registry[msg.name] = { url = msg.url, kind = msg.kind or "reactor", schema = msg.schema, warm = msg.warm, args = msg.args, body_file = msg.body_file }
    save_registry()
    dlog(("deploy '%s' (%s) registered"):format(msg.name, msg.kind or "reactor"))
    respond(sender, { ok = true, output = "registered " .. msg.name }, msg.id)
    reconcile()
  elseif t == "purge" then
    local n = 0
    for wname in pairs(registry) do n = n + 1 end
    for wname, p in pairs(placements) do
      rednet.send(p.worker, { type = "drain", slot = p.slot }, PROTO)
      if workers[p.worker] then workers[p.worker].slots[p.slot] = false end
    end
    registry, placements = {}, {}
    save_registry()
    dlog(("purge: %d workload(s) erased"):format(n))
    respond(sender, { ok = true, output = ("purged %d workload(s)"):format(n) }, msg.id)
  elseif t == "remove" then
    local p = placements[msg.name]
    if p then
      rednet.send(p.worker, { type = "drain", slot = p.slot }, PROTO)
      if workers[p.worker] then workers[p.worker].slots[p.slot] = false end
    end
    registry[msg.name] = nil
    save_registry()
    placements[msg.name] = nil
    respond(sender, { ok = true, output = "removed " .. tostring(msg.name) }, msg.id)
  elseif t == "update-workers" then
    local n = 0
    for wid in pairs(workers) do
      rednet.send(wid, { type = "update", url = msg.url, id = "upd:" .. wid }, PROTO)
      n = n + 1
    end
    dlog(("rollout: update sent to %d worker(s)"):format(n))
    respond(sender, { ok = true, output = ("update sent to %d worker(s)"):format(n) }, msg.id)
  elseif t == "update-gateway" then
    local r = http and http.get(msg.url or GATEWAY_URL, nil, true)
    if not r then
      respond(sender, { ok = false, err = "fetch failed" }, msg.id)
    else
      local me = (shell and shell.getRunningProgram and shell.getRunningProgram()) or "gateway"
      local h = fs.open(me, "wb") h.write(r.readAll()) h.close() r.close()
      respond(sender, { ok = true, output = "gateway updated - rebooting" }, msg.id)
      dlog("self-update: rebooting")
      os.sleep(0.5)
      os.reboot()
    end
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
        alive = os.clock() - w.last < 20, version = w.version }
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
      func = msg.func, params = msg.params, body = msg.body,
      session = msg.session, reset = msg.reset }, PROTO)
  elseif msg.id and tostring(msg.id):match("^asg:") and msg.ok ~= nil then
    local wname = tostring(msg.id):sub(5)
    if msg.ok == true then
      -- TLC-verified: confirm via the reply, not via view convergence -
      -- otherwise the age-out can unplace a healthy assignment seen through
      -- a stale view and the cluster livelocks (spec/crabcraft.tla)
      local p = placements[wname]
      if p and p.worker == sender then
        p.state = "running"
        p.hbm = 0
      end
    end
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
  elseif msg.id and t then
    -- version-skew safety: an unknown request fails loud instead of timing out
    respond(sender, { ok = false, err = "gateway v" .. CRAB_VERSION ..
      " does not understand '" .. tostring(t) .. "' - update the gateway" }, msg.id)
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
do
  local n = 0
  for _ in pairs(registry) do n = n + 1 end
  if n > 0 then dlog(("registry restored from disk: %d workload(s)"):format(n)) end
end
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
