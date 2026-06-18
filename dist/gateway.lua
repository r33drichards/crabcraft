-- crabcraft gateway (amalgamated; see host/gateway.lua)

local preload, loaded = {}, {}
local function require(n)
  if loaded[n] ~= nil then return loaded[n] end
  local f = preload[n] or error('module not bundled: '..n)
  local m = f(); if m == nil then m = true end
  loaded[n] = m; return m
end
preload["cron"] = function(...)
-- Cron schedules for crabcraft jobs (docs/WIRE.md section 6).
-- The gateway evaluates schedules against real-world UTC; crb uses the same
-- parser to reject bad schedules at deploy time. Grammar:
--   "@every <dur>"        intervals: 30s, 5m, 1h30m, 2d (gateway tick = ~2s)
--   "@hourly @daily @midnight @weekly @monthly @yearly @annually"
--   "min hour dom mon dow"  vixie-cron subset: * lists a,b,c ranges a-b
--                           steps */n a-b/n a/n  names jan-dec / sun-sat
--                           dow 0 and 7 = sunday
-- Vixie rule kept: when BOTH dom and dow are restricted, a time matches if
-- EITHER matches. No seconds field, no L/W/# extensions, no catch-up.
local M = {}

local MACROS = {
  ["@hourly"] = "0 * * * *", ["@daily"] = "0 0 * * *",
  ["@midnight"] = "0 0 * * *", ["@weekly"] = "0 0 * * 0",
  ["@monthly"] = "0 0 1 * *", ["@yearly"] = "0 0 1 1 *",
  ["@annually"] = "0 0 1 1 *",
}
local MON = { jan = 1, feb = 2, mar = 3, apr = 4, may = 5, jun = 6,
  jul = 7, aug = 8, sep = 9, oct = 10, nov = 11, dec = 12 }
local DOW = { sun = 0, mon = 1, tue = 2, wed = 3, thu = 4, fri = 5, sat = 6 }
local UNITS = { s = 1, m = 60, h = 3600, d = 86400 }

local function parse_every(s)
  local total, rest = 0, s
  while #rest > 0 do
    local n, u, tail = rest:match("^(%d+)([smhd])(.*)$")
    if not n then return nil, "bad duration '" .. s .. "' (use e.g. 30s, 5m, 1h30m)" end
    total = total + tonumber(n) * UNITS[u]
    rest = tail
  end
  if total < 1 then return nil, "@every duration must be at least 1s" end
  return total
end

local function field_value(tok, names, lo, hi)
  local v = tonumber(tok) or (names and names[tok:lower()])
  if type(v) ~= "number" or v % 1 ~= 0 or v < lo or v > hi then return nil end
  return v
end

-- one field -> set {n=true}, or nil for "*" (matches anything).
-- wrap: dow accepts 7 and stores it as 0 (both mean sunday).
local function parse_field(field, lo, hi, names, wrap)
  if field == "*" then return nil end
  local top = wrap and hi + 1 or hi
  local set = {}
  for part in (field .. ","):gmatch("([^,]*),") do
    local base, step = part:match("^(.-)/(%d+)$")
    base = base or part
    step = step and tonumber(step)
    if step and step < 1 then return nil, "step must be >= 1 in '" .. part .. "'" end
    local a, b
    if base == "*" then
      a, b = lo, hi
    else
      local x, y = base:match("^(.+)%-(.+)$")
      if x then
        a, b = field_value(x, names, lo, top), field_value(y, names, lo, top)
      else
        a = field_value(base, names, lo, top)
        b = step and top or a -- vixie: "n/step" runs n..top
      end
      if not a or not b or a > b then return nil, "bad value '" .. part .. "'" end
    end
    for v = a, b, step or 1 do
      set[wrap and v % (hi + 1) or v] = true
    end
  end
  return set
end

-- parse(expr) -> { every = seconds } | { min, hour, dom, mon, dow } | nil, err
-- (field sets are nil for "*"; match() treats nil as match-anything)
function M.parse(expr)
  if type(expr) ~= "string" then return nil, "schedule must be a string" end
  local s = expr:match("^%s*(.-)%s*$")
  local dur = s:match("^@every%s+(%S+)$")
  if dur then
    local secs, err = parse_every(dur)
    if not secs then return nil, err end
    return { every = secs }
  end
  s = MACROS[s:lower()] or s
  if s:sub(1, 1) == "@" then return nil, "unknown macro '" .. expr .. "'" end
  local f = {}
  for tok in s:gmatch("%S+") do f[#f + 1] = tok end
  if #f ~= 5 then
    return nil, "cron schedule needs 5 fields (min hour dom mon dow), " ..
      "@every <dur>, or a @macro - got '" .. expr .. "'"
  end
  local c, err = {}
  c.min, err = parse_field(f[1], 0, 59)
  if err then return nil, "minute: " .. err end
  c.hour, err = parse_field(f[2], 0, 23)
  if err then return nil, "hour: " .. err end
  c.dom, err = parse_field(f[3], 1, 31)
  if err then return nil, "day-of-month: " .. err end
  c.mon, err = parse_field(f[4], 1, 12, MON)
  if err then return nil, "month: " .. err end
  c.dow, err = parse_field(f[5], 0, 6, DOW, true)
  if err then return nil, "day-of-week: " .. err end
  return c
end

local function hit(set, v) return set == nil or set[v] == true end

-- match(parsed, tm): tm is an os.date("*t")-shaped table; pass os.date("!*t")
-- for the UTC semantics the gateway uses. Interval schedules never time-match.
function M.match(c, tm)
  if c.every then return false end
  if not (hit(c.min, tm.min) and hit(c.hour, tm.hour) and hit(c.mon, tm.month)) then
    return false
  end
  local dow = (tm.wday - 1) % 7 -- lua wday 1=sunday -> cron 0=sunday
  if c.dom and c.dow then return c.dom[tm.day] == true or c.dow[dow] == true end
  return hit(c.dom, tm.day) and hit(c.dow, dow)
end

return M

end
-- crabcraft gateway: the control plane (docs/WIRE.md section 3).
-- Owns the workload registry (desired state), reconciles placements onto
-- worker slots, routes invoke traffic, and drives jobs (run-to-completion
-- workloads, optionally on a cron schedule - WIRE.md section 6).
-- Run on a CC computer with a modem:
--   gateway [name]            (default name "gateway")
-- Needs beside it: cron.lua (the dist build inlines it).
local PROTO = "crabcraft"
local CRAB_VERSION = "0.3.0" -- stamped by tools/amalgamate.py
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

if package and package.path then package.path = "host/?.lua;./?.lua;" .. package.path end
local cron = require("cron")

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
local inflight = {}   -- reqid -> { from = senderid, t = clock } | { job = wname, t, deadline }
local cooldown = {}   -- wname -> { [wid] = clock of last assign failure }
local started = os.clock()

-- ---- jobs (kind = "job": run-to-completion, optionally cron-scheduled) ---------
-- jobstate[name] = { seq, ok, fail, skip, runs = {history},
--   cur = { n, phase = "pending"|"placing"|"running", tries, id, started, worker },
--   c = parsed schedule, lastmin/next = cron bookkeeping }
-- Run history (not cur - in-flight runs do not survive a gateway reboot) is
-- persisted so `crb logs` works across reboots.
local jobstate = {}
local JOBFILE = ".crab_jobs"
local function save_jobs()
  if type(fs) ~= "table" or not textutils then return end
  local t = {}
  for jname, js in pairs(jobstate) do
    if registry[jname] then
      t[jname] = { seq = js.seq, ok = js.ok, fail = js.fail, skip = js.skip, runs = js.runs }
    end
  end
  local h = fs.open(JOBFILE, "w")
  if h then h.write(textutils.serialize(t)); h.close() end
end
local function load_jobs()
  if type(fs) ~= "table" or not fs.exists(JOBFILE) then return end
  local h = fs.open(JOBFILE, "r")
  if not h then return end
  local ok, t = pcall(textutils.unserialize, h.readAll())
  h.close()
  if ok and type(t) == "table" then
    for jname, js in pairs(t) do
      js.runs = js.runs or {}
      jobstate[jname] = js
    end
  end
end
load_jobs()
local function jobstate_for(jname)
  local js = jobstate[jname]
  if not js then
    js = { seq = 0, ok = 0, fail = 0, skip = 0, runs = {} }
    jobstate[jname] = js
  end
  return js
end

local function utc_now() -- real-world epoch seconds (cron runs on UTC)
  if os.epoch then return math.floor(os.epoch("utc") / 1000) end
  return os.time()
end

-- ---- monitor dashboard ---------------------------------------------------------
local mon = (type(peripheral) == "table" and peripheral.find) and peripheral.find("monitor") or nil
local LOG = {}
local buttons = {} -- { {y, x1, x2, worker, wl, sess}, ... } rebuilt each draw
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
  local LW = math.max(40, math.floor(W * 0.62)) -- left column width
  local y = 1
  local function line(txt, fg)
    if y > H then return end
    mon.setCursorPos(1, y)
    c(fg or colours.white)
    mon.write(txt:sub(1, LW))
    y = y + 1
  end
  -- right column: session debug + [X] cancel buttons (touch)
  buttons = {}
  local ry = 1
  local rx = LW + 2
  local function rline(txt, fg)
    if ry > H or rx > W then return end
    mon.setCursorPos(rx, ry)
    c(fg or colours.white)
    mon.write(txt:sub(1, W - rx + 1))
    ry = ry + 1
  end
  rline("SESSIONS", colours.lightBlue)
  for wid, w in pairs(workers) do
    for slot, sess in pairs(w.sessions or {}) do
      local wl = w.slots[slot]
      if sess and wl and #sess > 0 then
        rline(("%s @ worker %d"):format(tostring(wl), wid), colours.yellow)
        for _, e in ipairs(sess) do
          local state = e.busy and "BUSY" or (e.booted and "idle" or "boot")
          local lbl = ("  %-10s %-4s q%-2d"):format(e.name, state, e.queued or 0)
          local fg = e.busy and colours.orange or colours.lime
          if ry <= H and rx + #lbl + 4 <= W then
            mon.setCursorPos(rx, ry)
            c(fg)
            mon.write(lbl)
            c(colours.red)
            mon.write(" [X]")
            buttons[#buttons + 1] = { y = ry, x1 = rx + #lbl + 1, x2 = rx + #lbl + 3,
              worker = wid, wl = wl, sess = e.name }
            ry = ry + 1
          end
        end
      end
    end
  end
  if #buttons == 0 then rline("  (none active)", colours.grey) end
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
    local info = p and ("worker " .. p.worker .. " " .. p.slot) or "unscheduled"
    local fg = state == "running" and colours.lime
      or state == "assigning" and colours.yellow or colours.orange
    if spec.kind == "job" then
      local js = jobstate[wname] or {}
      local last = js.runs and js.runs[#js.runs]
      state = js.cur and ("run #" .. js.cur.n)
        or (last and (last.ok and "succeeded" or "failed"))
        or (spec.schedule and "scheduled" or "idle")
      info = (spec.schedule and (spec.schedule .. "  ") or "")
        .. (last and ((last.ok and "ok " or "ERR ") .. (utc_now() - last.t) .. "s ago")
          or "no runs yet")
      fg = js.cur and colours.yellow
        or (last and (last.ok and colours.lime or colours.red)) or colours.cyan
    end
    line(("  %-14s %-8s %-10s %s"):format(wname, spec.kind or "?", state, info), fg)
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

-- ---- job state machine -----------------------------------------------------
-- A run: pending (waiting for a slot) -> placing (assigned, module loading) ->
-- running (the one run invoke is in flight) -> a history entry. Workers know
-- nothing about jobs: a run is a plain assign + invoke + drain.
local function job_release(wname) -- free the slot a job run occupied
  local p = placements[wname]
  if p then
    rednet.send(p.worker, { type = "drain", slot = p.slot }, PROTO)
    if workers[p.worker] then workers[p.worker].slots[p.slot] = false end
    placements[wname] = nil
  end
end

local function trunc(s)
  s = tostring(s or "")
  if #s > 4096 then return s:sub(1, 4096) .. ("...[%d bytes truncated]"):format(#s - 4096) end
  return s
end

local function job_finish(wname, js, okflag, output, err)
  local cur = js.cur
  local entry = { n = cur.n, tries = cur.tries, ok = okflag or false, t = utc_now(),
    dur = cur.started and (os.clock() - cur.started) or nil, worker = cur.worker }
  if okflag then entry.output = trunc(output) else entry.err = trunc(err) end
  js.runs[#js.runs + 1] = entry
  while #js.runs > 5 do table.remove(js.runs, 1) end
  if okflag then js.ok = js.ok + 1 else js.fail = js.fail + 1 end
  js.cur = nil
  local spec = registry[wname]
  -- keep-warm jobs hold their placement between runs (no refetch/retranspile);
  -- everything else frees the slot, and failures always start fresh
  if not (okflag and spec and spec.keep) then job_release(wname) end
  save_jobs()
  dlog(("job '%s' run #%d %s%s"):format(wname, entry.n,
    okflag and "ok" or ("FAILED: " .. tostring(err)),
    entry.dur and (" (%.1fs)"):format(entry.dur) or ""))
end

local function job_attempt_failed(wname, js, spec, err)
  local cur = js.cur
  if not cur then return end
  if cur.id then inflight[cur.id] = nil; cur.id = nil end
  job_release(wname)
  if cur.tries < (spec.retries or 0) then
    cur.tries = cur.tries + 1
    cur.phase = "pending"
    dlog(("job '%s' run #%d attempt failed (%s) - retry %d/%d"):format(
      wname, cur.n, tostring(err), cur.tries, spec.retries or 0))
  else
    job_finish(wname, js, false, nil, err)
  end
end

local function job_fire(wname, js, spec) -- send the run's one invoke
  local cur, p = js.cur, placements[wname]
  if not (cur and p and p.state == "running") then return end
  cur.phase = "running"
  cur.started = os.clock()
  cur.worker = p.worker
  cur.id = ("job:%s:%d:%d"):format(wname, cur.n, cur.tries)
  inflight[cur.id] = { job = wname, t = os.clock(),
    deadline = os.clock() + (spec.timeout or 600) }
  if spec.module == "reactor" then
    rednet.send(p.worker, { type = "invoke", id = cur.id, name = wname,
      func = spec.func, params = spec.params }, PROTO)
  else
    rednet.send(p.worker, { type = "invoke", id = cur.id, name = wname,
      body = spec.body or "" }, PROTO)
  end
end

local function job_maybe_fire(wname) -- placement just confirmed running?
  local spec, js = registry[wname], jobstate[wname]
  if spec and spec.kind == "job" and js and js.cur and js.cur.phase == "placing" then
    job_fire(wname, js, spec)
  end
end

local function job_run_create(wname, js)
  if js.cur then
    return nil, ("run #%d of '%s' is still %s"):format(js.cur.n, wname, js.cur.phase)
  end
  js.seq = js.seq + 1
  js.cur = { n = js.seq, phase = "pending", tries = 0 }
  save_jobs() -- persist seq now: a run lost to a reboot leaves a numbered gap
  return js.cur
end

local function job_tick() -- called from reconcile, before the placer
  for wname, spec in pairs(registry) do
    if spec.kind == "job" then
      local js = jobstate_for(wname)
      local cur, p = js.cur, placements[wname]
      if not cur then
        -- a placement with no active run is a leftover (gateway rebooted
        -- mid-run) unless this is a warm completed job holding its slot
        if p and not spec.keep then job_release(wname) end
      elseif cur.phase == "pending" and p and p.state == "running" then
        cur.phase = "placing" -- keep-warm: reuse the live placement
        job_fire(wname, js, spec)
      elseif cur.phase == "placing" and not p then
        cur.phase = "pending" -- assign failed / worker lost: re-place
      elseif cur.phase == "placing" and p and p.state == "running" then
        job_fire(wname, js, spec) -- backstop; usually fired from the ack
      elseif cur.phase == "running" and not p then
        -- worker lost mid-run: the reply will never come
        if cur.id then inflight[cur.id] = nil; cur.id = nil end
        job_attempt_failed(wname, js, spec, "worker lost mid-run")
      end
    end
  end
end

local cron_warned = {}
local function cron_tick() -- create runs for scheduled jobs that are due
  local created = 0
  local now = utc_now()
  local tm = os.date("!*t", now)
  local curmin = math.floor(now / 60)
  for wname, spec in pairs(registry) do
    if spec.kind == "job" and spec.schedule then
      local js = jobstate_for(wname)
      if not js.c and not cron_warned[wname] then
        local c, cerr = cron.parse(spec.schedule)
        js.c = c
        if c and c.every then js.next = now + c.every end
        if not c then
          cron_warned[wname] = true
          dlog(("job '%s': bad schedule '%s' (%s) - never fires"):format(
            wname, tostring(spec.schedule), tostring(cerr)))
        end
      end
      local due = false
      if js.c and js.c.every then
        if now >= (js.next or 0) then js.next = now + js.c.every; due = true end
      elseif js.c then
        -- minute schedules: at most one decision per matching UTC minute,
        -- no catch-up for minutes the gateway slept through
        if js.lastmin ~= curmin and cron.match(js.c, tm) then
          js.lastmin = curmin
          due = true
        end
      end
      if due then
        if js.cur then
          js.skip = js.skip + 1 -- previous run still active: skip this firing
        elseif job_run_create(wname, js) then
          created = created + 1
        end
      end
    end
  end
  return created
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
  -- after reboots) beats assigning a fresh copy elsewhere. Jobs are never
  -- adopted: a job slot with no live run is a leftover and gets drained below
  for wname, spec in pairs(registry) do
    if not placements[wname] and spec.kind ~= "job" then
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
          -- a job slot that is not the live run's placement is a leftover
          -- (jobs are never adopted; their slots are transient by design)
          local leftover_job = registry[wl] and registry[wl].kind == "job"
          if not is_placement and (not registry[wl] or placed or leftover_job) then
            dlog(("reconcile: draining %s '%s' on worker %d %s"):format(
              not registry[wl] and "unknown" or leftover_job and "leftover job" or "duplicate",
              tostring(wl), wid, slot))
            rednet.send(wid, { type = "drain", slot = slot }, PROTO)
            w.slots[slot] = false
          end
        end
      end
    end
  end
  -- drive job runs (may flip runs to pending so the placer below sees them)
  job_tick()
  -- place unplaced workloads (jobs only while a run is waiting for a slot)
  for wname, spec in pairs(registry) do
    if not placements[wname] then
      local cur = spec.kind == "job" and jobstate[wname] and jobstate[wname].cur
      if spec.kind ~= "job" or (cur and cur.phase == "pending") then
        local wid, slot = free_slot(wname)
        if wid then
          placements[wname] = { worker = wid, slot = slot, state = "assigning" }
          workers[wid].slots[slot] = wname -- optimistic; heartbeat confirms
          dlog(("reconcile: assigning '%s' -> worker %d slot %s"):format(wname, wid, slot))
          rednet.send(wid, { type = "assign", slot = slot, name = wname,
            url = spec.url, kind = spec.kind == "job" and spec.module or spec.kind,
            warm = spec.warm, args = spec.args, body_file = spec.body_file,
            id = "asg:" .. wname }, PROTO)
          if cur then cur.phase = "placing" end
        end
      end
    end
  end
  -- expire stale inflight entries (job runs carry their own deadline)
  for id, e in pairs(inflight) do
    if e.job then
      if os.clock() > (e.deadline or 0) then
        inflight[id] = nil
        local js, spec = jobstate[e.job], registry[e.job]
        if js and spec and js.cur and js.cur.id == id then
          js.cur.id = nil
          job_attempt_failed(e.job, js, spec, "run timed out")
        end
      end
    elseif os.clock() - e.t > 120 then
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
    -- adopt existing placements (worker reboot recovery; running slots only;
    -- never jobs - leftover job slots are drained by reconcile)
    for _, s in ipairs(msg.slots or {}) do
      if s.workload and s.state == "running" and registry[s.workload]
          and registry[s.workload].kind ~= "job" and not placements[s.workload] then
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
        if sl.workload and registry[sl.workload] and registry[sl.workload].kind ~= "job"
            and not placements[sl.workload] then
          placements[sl.workload] = { worker = sender, slot = sl.disk, state = sl.state or "running" }
        end
      end
    end
    if w then
      w.last = os.clock()
      if msg.version then w.version = msg.version end
      w.states = w.states or {}
      w.used = w.used or {}
      w.sessions = w.sessions or {}
      for _, s in ipairs(msg.slots or {}) do
        w.slots[s.disk] = s.workload or false
        w.states[s.disk] = s.state
        w.used[s.disk] = s.used
        w.sessions[s.disk] = s.sessions
      end
      for wname, p in pairs(placements) do
        if p.worker == sender then
          local slotw = w.slots[p.slot]
          local slots_state = w.states[p.slot]
          if slotw == wname and slots_state == "running" then
            p.state = "running"
            p.hbm = 0
            job_maybe_fire(wname) -- a placed job run waits for exactly this
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
    if msg.schedule and msg.kind ~= "job" then
      respond(sender, { ok = false, err = "schedule needs kind: job" }, msg.id)
      return
    end
    if msg.schedule then
      local c, cerr = cron.parse(msg.schedule)
      if not c then
        respond(sender, { ok = false, err = "bad schedule: " .. tostring(cerr) }, msg.id)
        return
      end
    end
    local old = registry[msg.name]
    if (old and old.kind == "job") or msg.kind == "job" then
      job_release(msg.name) -- a redeploy mid-run starts over cleanly
      jobstate[msg.name] = nil
      cron_warned[msg.name] = nil
    end
    registry[msg.name] = { url = msg.url, kind = msg.kind or "reactor", schema = msg.schema,
      warm = msg.warm, args = msg.args, body_file = msg.body_file,
      module = msg.kind == "job" and (msg.module or "command") or nil,
      func = msg.func, params = msg.params, body = msg.body, keep = msg.keep,
      schedule = msg.schedule, retries = msg.retries, timeout = msg.timeout }
    save_registry()
    save_jobs()
    dlog(("deploy '%s' (%s%s) registered"):format(msg.name, msg.kind or "reactor",
      msg.schedule and (" @ " .. msg.schedule) or ""))
    local note = ""
    if msg.kind == "job" then
      if msg.schedule then
        note = " (job, schedule '" .. msg.schedule .. "')"
      else
        job_run_create(msg.name, jobstate_for(msg.name)) -- like k8s: a Job runs on create
        note = " (job, run #1 queued)"
      end
    end
    respond(sender, { ok = true, output = "registered " .. msg.name .. note }, msg.id)
    reconcile()
  elseif t == "purge" then
    local n = 0
    for wname in pairs(registry) do n = n + 1 end
    for wname, p in pairs(placements) do
      rednet.send(p.worker, { type = "drain", slot = p.slot }, PROTO)
      if workers[p.worker] then workers[p.worker].slots[p.slot] = false end
    end
    registry, placements, jobstate, cron_warned = {}, {}, {}, {}
    save_registry()
    save_jobs()
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
    if jobstate[msg.name] then
      local cur = jobstate[msg.name].cur
      if cur and cur.id then inflight[cur.id] = nil end
      jobstate[msg.name] = nil
      cron_warned[msg.name] = nil
      save_jobs()
    end
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
      local row = { name = wname, kind = spec.kind, url = spec.url,
        worker = p and p.worker, slot = p and p.slot, state = p and p.state or "pending" }
      if spec.kind == "job" then
        local js = jobstate_for(wname)
        local last = js.runs[#js.runs]
        row.state = js.cur and js.cur.phase
          or (last and (last.ok and "succeeded" or "failed"))
          or (spec.schedule and "scheduled" or "idle")
        row.schedule = spec.schedule
        row.runs, row.ok, row.fail, row.skip = js.seq, js.ok, js.fail, js.skip
        if last then row.last = { n = last.n, ok = last.ok, t = last.t, dur = last.dur } end
      end
      out[#out + 1] = row
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
  elseif t == "run" then
    local spec = registry[msg.name]
    if not spec then
      respond(sender, { ok = false, err = "no workload '" .. tostring(msg.name) .. "'" }, msg.id)
    elseif spec.kind ~= "job" then
      respond(sender, { ok = false, err = "'" .. msg.name .. "' is not a job (kind " ..
        tostring(spec.kind) .. ")" }, msg.id)
    else
      local js = jobstate_for(msg.name)
      local cur, rerr = job_run_create(msg.name, js)
      if not cur then
        respond(sender, { ok = false, err = rerr }, msg.id)
      else
        dlog(("job '%s' run #%d queued (manual)"):format(msg.name, cur.n))
        respond(sender, { ok = true, output = ("queued run #%d of '%s'"):format(cur.n, msg.name) }, msg.id)
        reconcile()
      end
    end
  elseif t == "job-logs" then
    local spec = registry[msg.name]
    if not spec or spec.kind ~= "job" then
      respond(sender, { ok = false, err = "no job '" .. tostring(msg.name) .. "'" }, msg.id)
    else
      local js = jobstate_for(msg.name)
      respond(sender, { ok = true, runs = js.runs, schedule = spec.schedule,
        module = spec.module, func = spec.func, seq = js.seq, skip = js.skip,
        cur = js.cur and { n = js.cur.n, phase = js.cur.phase, tries = js.cur.tries } }, msg.id)
    end
  elseif t == "invoke" then
    local spec = registry[msg.name]
    if spec and spec.kind == "job" then
      respond(sender, { ok = false, err = "'" .. msg.name ..
        "' is a job - use crb run / crb logs " .. msg.name }, msg.id)
      return
    end
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
        job_maybe_fire(wname)
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
      if e.job then
        local js, spec = jobstate[e.job], registry[e.job]
        if js and spec and js.cur and js.cur.id == msg.id then -- else: stale reply
          js.cur.id = nil
          if msg.ok then job_finish(e.job, js, true, msg.result)
          else job_attempt_failed(e.job, js, spec, msg.err or "invoke failed") end
        end
      else
        respond(e.from, { ok = msg.ok, result = msg.result, err = msg.err }, msg.id)
      end
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

spawn(function() -- dashboard touch: cancel sessions
  while true do
    local _, _, x, ty = os.pullEvent("monitor_touch")
    for _, b in ipairs(buttons) do
      if ty == b.y and x >= b.x1 and x <= b.x2 then
        dlog(("dashboard: cancelling session '%s' of '%s' on worker %d"):format(b.sess, b.wl, b.worker))
        rednet.send(b.worker, { type = "cancel-session", name = b.wl, session = b.sess }, PROTO)
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

spawn(function() -- cron: create runs for scheduled jobs that come due
  while true do
    local timer = os.startTimer(2)
    repeat local ev, p = os.pullEvent("timer") until p == timer
    local ok, err = pcall(function()
      if cron_tick() > 0 then reconcile() end
    end)
    if not ok then dlog("cron error: " .. tostring(err)) end
  end
end)

print(("gateway '%s' up on protocol '%s' - control loop every 5s, cron tick every 2s"):format(name, PROTO))
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
