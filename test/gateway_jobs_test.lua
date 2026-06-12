-- Job + cron state-machine test for host/gateway.lua on plain Lua (5.4) -
-- no CraftOS needed. A fake CC world (rednet/os/fs/peripheral) runs the real
-- gateway chunk as a coroutine; the harness plays worker + client and warps
-- the clock, so schedules and timeouts run in zero real time.
--   lua5.4 test/gateway_jobs_test.lua
-- Covers: service placement regression, one-shot command/reactor jobs,
-- retries, final failure, @every + five-field schedules, concurrency skip,
-- run timeout, worker loss mid-run, manual run, keep-warm reuse, the invoke
-- guard, deploy validation, remove, and gateway-reboot recovery (persisted
-- history + leftover slot drain).
package.path = "host/?.lua;" .. package.path

local passed, failed = 0, 0
local function check(desc, cond, extra)
  if cond then passed = passed + 1
  else
    failed = failed + 1
    print("FAIL " .. desc .. (extra and (" -- " .. tostring(extra)) or ""))
  end
end

-- ---- minimal textutils.serialize (CC-compatible enough for round-trips) ----
local function ser(v)
  local t = type(v)
  if t == "table" then
    local parts = {}
    for k, val in pairs(v) do
      parts[#parts + 1] = "[" .. ser(k) .. "]=" .. ser(val)
    end
    return "{" .. table.concat(parts, ",") .. "}"
  elseif t == "string" then
    return string.format("%q", v)
  else
    return tostring(v)
  end
end

-- ---- one fake CC world per scenario -----------------------------------------
-- epoch0 is an exact UTC hour boundary so five-field schedules are predictable
local EPOCH0 = 1749999600

local function new_world(opts)
  opts = opts or {}
  local w = { clock = 0, timers = {}, ntimer = 0, sent = {}, disk = opts.disk or {},
    log = {} }

  local env = setmetatable({}, { __index = _G })
  env.package = false -- gateway skips its package.path shim
  env.require = function(n)
    if n == "cron" then return dofile("host/cron.lua") end
    error("module not bundled: " .. n)
  end
  env.print = function(...)
    local p = {}
    for i = 1, select("#", ...) do p[i] = tostring(select(i, ...)) end
    w.log[#w.log + 1] = table.concat(p, " ")
  end
  env.os = {
    clock = function() return w.clock end,
    epoch = function() return (EPOCH0 + w.clock) * 1000 end,
    time = function() return EPOCH0 + w.clock end,
    date = function(fmt, t) return os.date(fmt, t) end,
    startTimer = function(s)
      w.ntimer = w.ntimer + 1
      w.timers[w.ntimer] = w.clock + s
      return w.ntimer
    end,
    pullEvent = function(filter)
      while true do
        local ev = { coroutine.yield(filter) }
        if not filter or ev[1] == filter then return table.unpack(ev) end
      end
    end,
    pullEventRaw = function() return coroutine.yield() end,
    getComputerLabel = function() return "gw" end,
    sleep = function() end,
    reboot = function() error("unexpected reboot") end,
  }
  env.rednet = {
    open = function() end,
    host = function() end,
    lookup = function() end,
    send = function(to, msg, proto) w.sent[#w.sent + 1] = { to = to, msg = msg } end,
    receive = function(proto)
      while true do
        local ev, a, b, c = coroutine.yield("rednet_message")
        if ev == "rednet_message" and (not proto or c == proto) then return a, b end
      end
    end,
  }
  env.peripheral = {
    find = function(ptype, filter)
      if ptype == "modem" then
        if filter then filter("back") end
        return "back"
      end
    end,
  }
  env.fs = {
    exists = function(p) return w.disk[p] ~= nil end,
    open = function(p, mode)
      if mode == "w" or mode == "wb" then
        local buf = {}
        return { write = function(d) buf[#buf + 1] = d end,
          close = function() w.disk[p] = table.concat(buf) end }
      end
      local d = w.disk[p]
      if d == nil then return nil end
      return { readAll = function() return d end, close = function() end }
    end,
  }
  env.textutils = {
    serialize = ser,
    unserialize = function(s) return assert(load("return " .. s, "=ser", "t", {}))() end,
  }

  local chunk = assert(loadfile("host/gateway.lua", "t", env))
  w.co = coroutine.create(chunk)

  local function resume(...)
    assert(coroutine.status(w.co) ~= "dead", "gateway exited early")
    local ok, err = coroutine.resume(w.co, ...)
    if not ok then error("gateway crashed: " .. tostring(err), 2) end
  end
  resume() -- boot: runs until the main loop's first pullEventRaw

  function w.inject(sender, msg) resume("rednet_message", sender, msg, "crabcraft") end

  -- with w.auto_accept on, the scripted worker acks assigns the moment they
  -- arrive (a real worker confirms within a second; sitting on an assign for
  -- many virtual seconds makes the gateway - correctly - unplace it)
  w.n_assigns = 0
  local function pump_assigns()
    if not w.auto_accept then return end
    while true do
      local m = w.pop_type("assign", w.WID)
      if not m then return end
      w.n_assigns = w.n_assigns + 1
      w.slots[m.msg.slot] = m.msg.name
      w.inject(w.WID, { id = m.msg.id, ok = true })
    end
  end

  -- advance the clock, firing every due timer in order (reconcile + cron run
  -- off these, so warping time drives the whole control plane). The scripted
  -- worker heartbeats every 5s along the way - like a real one - until a test
  -- turns w.hb_auto off to simulate losing it.
  function w.tick(seconds)
    local target = w.clock + seconds
    pump_assigns()
    while true do
      local bestid, bestat
      for id, at in pairs(w.timers) do
        if at <= target and (not bestat or at < bestat) then bestid, bestat = id, at end
      end
      local hbat = w.hb_auto and (w.last_hb + 5) or nil
      if hbat and hbat > target then hbat = nil end
      if not bestid and not hbat then break end
      if hbat and (not bestat or hbat <= bestat) then
        w.clock = hbat
        w.worker_heartbeat()
      else
        w.clock = bestat
        w.timers[bestid] = nil
        resume("timer", bestid)
      end
      pump_assigns()
    end
    w.clock = target
  end

  -- pop the first sent message matching pred (or any), nil if none
  function w.pop(pred)
    for i, m in ipairs(w.sent) do
      if not pred or pred(m) then return table.remove(w.sent, i) end
    end
  end
  function w.pop_type(t, to)
    return w.pop(function(m)
      return m.msg.type == t and (to == nil or m.to == to)
    end)
  end
  function w.pop_reply(id)
    return w.pop(function(m) return m.msg.id == id and m.msg.type == nil end)
  end
  function w.clear() w.sent = {} end

  -- ---- a scripted worker (id 1) ---------------------------------------------
  local WID = 1
  w.WID = WID
  w.slots = {}
  function w.worker_register(nslots)
    local list = {}
    for i = 1, nslots or 2 do
      w.slots["disk" .. i] = false
      list[#list + 1] = { disk = "disk" .. i }
    end
    w.inject(WID, { type = "register", worker = "w1", slots = list, version = "t", id = "reg" })
    w.pop_reply("reg")
    w.hb_auto = true
    w.last_hb = w.clock
  end
  function w.worker_heartbeat()
    local list = {}
    for s, wl in pairs(w.slots) do
      if wl then list[#list + 1] = { disk = s, workload = wl, state = "running" }
      else list[#list + 1] = { disk = s } end
    end
    w.hb_auto = true -- a heartbeating worker keeps heartbeating (until told not to)
    w.last_hb = w.clock
    w.inject(WID, { type = "heartbeat", worker = "w1", slots = list, version = "t" })
  end
  -- accept the next assign: update the slot mirror + ack (the TLC-verified
  -- confirm path; no heartbeat needed for the placement to count as running)
  function w.accept_assign(expect_kind)
    local m = w.pop_type("assign", WID)
    assert(m, "no assign was sent")
    if expect_kind then
      check("assign kind = " .. expect_kind, m.msg.kind == expect_kind, m.msg.kind)
    end
    w.n_assigns = w.n_assigns + 1
    w.slots[m.msg.slot] = m.msg.name
    w.inject(WID, { id = m.msg.id, ok = true })
    return m.msg
  end
  function w.apply_drains()
    local n = 0
    while true do
      local m = w.pop_type("drain", WID)
      if not m then return n end
      w.slots[m.msg.slot] = false
      n = n + 1
    end
  end
  -- answer the pending job/run invoke
  function w.answer_invoke(reply)
    local m = w.pop_type("invoke", WID)
    assert(m, "no invoke was sent")
    reply.type = "invoke-reply"
    reply.id = m.msg.id
    w.inject(WID, reply)
    return m.msg
  end

  -- ---- a scripted client (id 9) ---------------------------------------------
  local CID = 9
  local seq = 0
  function w.request(msg)
    seq = seq + 1
    msg.id = "c9:" .. seq
    w.inject(CID, msg)
    local r = w.pop_reply(msg.id)
    assert(r, "no reply to " .. tostring(msg.type))
    return r.msg
  end
  function w.list_row(name)
    local r = w.request({ type = "list" })
    for _, row in ipairs(r.workloads or {}) do
      if row.name == name then return row end
    end
  end

  return w
end

-- =============================================================================
-- T1: services still place and route (regression guard around the job changes)
do
  local w = new_world()
  w.worker_register(2)
  local r = w.request({ type = "deploy", name = "svc", url = "file:svc.wasm",
    kind = "reactor", schema = "{}" })
  check("T1 service deploy ok", r.ok == true, r.err)
  w.accept_assign("reactor")
  check("T1 service running", w.list_row("svc").state == "running")
  w.inject(9, { type = "invoke", id = "c9:inv", name = "svc", func = "f#g", params = "p" })
  local inv = w.pop_type("invoke", w.WID)
  check("T1 invoke routed to worker", inv and inv.msg.func == "f#g")
  w.inject(w.WID, { type = "invoke-reply", id = "c9:inv", ok = true, result = "R" })
  local rep = w.pop_reply("c9:inv")
  check("T1 invoke reply relayed", rep and rep.msg.ok == true and rep.msg.result == "R")
end

-- T2: one-shot command job - deploy runs it once, result lands in history,
-- slot is drained, nothing re-places it afterwards
do
  local w = new_world()
  w.worker_register(2)
  local r = w.request({ type = "deploy", name = "report", url = "file:r.wasm",
    kind = "job", module = "command", body = '{"day":"today"}' })
  check("T2 job deploy ok", r.ok == true, r.err)
  check("T2 deploy queues run #1", tostring(r.output):find("run #1", 1, true) ~= nil, r.output)
  w.accept_assign("command")
  local inv = w.answer_invoke({ ok = true, result = "REPORT DONE" })
  check("T2 run invoke carries the body", inv.body == '{"day":"today"}')
  check("T2 slot drained after the run", w.apply_drains() == 1)
  local row = w.list_row("report")
  check("T2 state succeeded", row.state == "succeeded", row.state)
  check("T2 runs counted", row.runs == 1 and row.ok == 1 and row.fail == 0)
  local logs = w.request({ type = "job-logs", name = "report" })
  check("T2 logs hold the output", logs.ok and logs.runs[1].output == "REPORT DONE")
  check("T2 logs record duration", type(logs.runs[1].dur) == "number")
  w.tick(30)
  check("T2 nothing re-placed", w.pop_type("assign") == nil)
end

-- T3: reactor-function job - the run invoke carries func + pre-encoded params
do
  local w = new_world()
  w.worker_register(2)
  w.request({ type = "deploy", name = "greet-job", url = "file:h.wasm", kind = "job",
    module = "reactor", schema = "{}", func = "crab:hello/greeter@0.1.0#greet",
    params = "\5cron\1\1" })
  w.accept_assign("reactor")
  local inv = w.answer_invoke({ ok = true, result = "\14Hello, cron!!!" })
  check("T3 func forwarded", inv.func == "crab:hello/greeter@0.1.0#greet")
  check("T3 params forwarded", inv.params == "\5cron\1\1")
  w.apply_drains()
  local logs = w.request({ type = "job-logs", name = "greet-job" })
  check("T3 binary result kept", logs.runs[1].output == "\14Hello, cron!!!")
end

-- T4: retries - a failed attempt re-places and reruns; the run only fails for
-- good once the budget is spent
do
  local w = new_world()
  w.worker_register(2)
  w.request({ type = "deploy", name = "flaky", url = "file:f.wasm", kind = "job",
    module = "command", retries = 1 })
  w.accept_assign()
  w.answer_invoke({ ok = false, err = "boom" })
  check("T4 failed attempt drains", w.apply_drains() == 1)
  w.tick(6) -- reconcile re-places the retry
  w.accept_assign()
  w.answer_invoke({ ok = true, result = "second time lucky" })
  w.apply_drains()
  local row = w.list_row("flaky")
  check("T4 retry succeeded", row.state == "succeeded" and row.ok == 1 and row.fail == 0)
  local logs = w.request({ type = "job-logs", name = "flaky" })
  check("T4 history shows tries=1", logs.runs[1].tries == 1)

  -- budget spent: retries=0 job fails outright
  w.request({ type = "deploy", name = "doomed", url = "file:d.wasm", kind = "job",
    module = "command" })
  w.accept_assign()
  w.answer_invoke({ ok = false, err = "kaput" })
  w.apply_drains()
  local drow = w.list_row("doomed")
  check("T4 no-retry job fails", drow.state == "failed" and drow.fail == 1)
  local dlogs = w.request({ type = "job-logs", name = "doomed" })
  check("T4 error recorded", dlogs.runs[1].err == "kaput")
end

-- T5: @every schedule - no run at deploy, runs come due on the clock, and a
-- still-active run makes the next firing a counted skip (concurrency: forbid)
do
  local w = new_world()
  w.worker_register(2)
  w.auto_accept = true
  local r = w.request({ type = "deploy", name = "ticker", url = "file:t.wasm",
    kind = "job", module = "command", schedule = "@every 30s" })
  check("T5 scheduled deploy ok", r.ok == true, r.err)
  w.tick(4)
  check("T5 no run before due", w.n_assigns == 0)
  check("T5 state scheduled", w.list_row("ticker").state == "scheduled")
  w.tick(30)
  w.answer_invoke({ ok = true, result = "tick 1" })
  w.apply_drains()
  w.tick(32)
  w.answer_invoke({ ok = true, result = "tick 2" })
  w.apply_drains()
  check("T5 two scheduled runs", w.list_row("ticker").runs == 2)
  -- leave the third run hanging (no reply) across the next due times
  w.tick(32)
  check("T5 third run fired", w.pop_type("invoke") ~= nil)
  w.tick(64) -- two more firings come due while run #3 is still in flight
  local row = w.list_row("ticker")
  check("T5 overlapping firings skipped", row.runs == 3 and (row.skip or 0) >= 2, row.skip)
end

-- T6: five-field schedule on the UTC clock - "0 * * * *" fires exactly once
-- in the matching minute (EPOCH0 is an exact hour boundary)
do
  local w = new_world()
  w.worker_register(2)
  w.auto_accept = true
  w.request({ type = "deploy", name = "hourly", url = "file:h.wasm", kind = "job",
    module = "command", schedule = "0 * * * *" })
  -- the boot minute IS minute 0 of the hour, so the first firing is immediate
  w.tick(4)
  w.answer_invoke({ ok = true, result = "first" })
  w.apply_drains()
  w.tick(50) -- still inside minute 0: must not fire again
  check("T6 one firing per matching minute", w.list_row("hourly").runs == 1)
  w.tick(1800) -- minute 30: no match
  check("T6 no off-minute firing", w.list_row("hourly").runs == 1)
  w.tick(1800) -- crosses the next hour boundary
  w.answer_invoke({ ok = true, result = "second" })
  w.apply_drains()
  check("T6 fires again next hour", w.list_row("hourly").runs == 2)
end

-- T7: per-run timeout - no reply within timeout fails the attempt
do
  local w = new_world()
  w.worker_register(2)
  w.request({ type = "deploy", name = "slow", url = "file:s.wasm", kind = "job",
    module = "command", timeout = 5 })
  w.accept_assign()
  check("T7 invoke went out", w.pop_type("invoke") ~= nil)
  w.tick(15) -- reconcile sweep crosses the 5s deadline
  w.apply_drains()
  local row = w.list_row("slow")
  check("T7 run timed out -> failed", row.state == "failed", row.state)
  local logs = w.request({ type = "job-logs", name = "slow" })
  check("T7 timeout recorded", tostring(logs.runs[1].err):find("timed out") ~= nil)
  -- a reply that arrives after the timeout is stale and changes nothing
  w.inject(w.WID, { type = "invoke-reply", id = "job:slow:1:0", ok = true, result = "late" })
  check("T7 late reply ignored", w.list_row("slow").state == "failed")
end

-- T8: worker lost mid-run - missed heartbeats fail the attempt
do
  local w = new_world()
  w.worker_register(2)
  w.request({ type = "deploy", name = "orphan", url = "file:o.wasm", kind = "job",
    module = "command" })
  w.accept_assign()
  check("T8 run started", w.pop_type("invoke") ~= nil)
  w.hb_auto = false -- the worker goes silent
  w.tick(25) -- no heartbeats: worker is declared lost, placement dropped
  local row = w.list_row("orphan")
  check("T8 lost worker fails the run", row.state == "failed", row.state)
  local logs = w.request({ type = "job-logs", name = "orphan" })
  check("T8 reason recorded", tostring(logs.runs[1].err):find("worker lost") ~= nil)
end

-- T9: manual runs + the invoke guard + remove
do
  local w = new_world()
  w.worker_register(2)
  w.request({ type = "deploy", name = "task", url = "file:t.wasm", kind = "job",
    module = "command", schedule = "@every 1h" })
  local r = w.request({ type = "run", name = "task" })
  check("T9 manual run queued", r.ok == true and tostring(r.output):find("#1") ~= nil, r.err)
  local r2 = w.request({ type = "run", name = "task" })
  check("T9 second run refused while active", r2.ok == false)
  w.accept_assign()
  w.answer_invoke({ ok = true, result = "done" })
  w.apply_drains()
  local r3 = w.request({ type = "invoke", name = "task", func = "x", params = "" })
  check("T9 invoke on a job is refused", r3.ok == false and tostring(r3.err):find("is a job") ~= nil)
  local r4 = w.request({ type = "run", name = "nosuch" })
  check("T9 run on unknown name errs", r4.ok == false)
  local r5 = w.request({ type = "remove", name = "task" })
  check("T9 remove ok", r5.ok == true)
  check("T9 logs gone after remove", w.request({ type = "job-logs", name = "task" }).ok == false)
end

-- T10: keep-warm - the placement survives between runs (one assign total)
do
  local w = new_world()
  w.worker_register(2)
  w.auto_accept = true
  w.request({ type = "deploy", name = "warmjob", url = "file:w.wasm", kind = "job",
    module = "command", schedule = "@every 20s", keep = true })
  w.tick(24)
  w.answer_invoke({ ok = true, result = "one" })
  check("T10 keep-warm: no drain after success", w.apply_drains() == 0)
  w.tick(22) -- next due: reuses the live placement, fires straight away
  w.answer_invoke({ ok = true, result = "two" })
  check("T10 one assign for two runs", w.n_assigns == 1, w.n_assigns)
  check("T10 two runs on one placement", w.list_row("warmjob").runs == 2)
end

-- T11: deploy validation
do
  local w = new_world()
  w.worker_register(2)
  local r = w.request({ type = "deploy", name = "bad", url = "u", kind = "job",
    module = "command", schedule = "61 * * * *" })
  check("T11 bad schedule rejected", r.ok == false and tostring(r.err):find("minute") ~= nil, r.err)
  local r2 = w.request({ type = "deploy", name = "bad2", url = "u", kind = "reactor",
    schedule = "@every 5m" })
  check("T11 schedule on a service rejected", r2.ok == false)
end

-- T12: gateway reboot - registry + run history persist; a leftover job slot
-- on the worker is drained, not adopted
do
  local w = new_world()
  w.worker_register(2)
  w.request({ type = "deploy", name = "keeper", url = "file:k.wasm", kind = "job",
    module = "command", schedule = "@every 1h" })
  w.request({ type = "run", name = "keeper" })
  w.accept_assign()
  w.answer_invoke({ ok = true, result = "before reboot" })
  w.apply_drains()
  -- second run is mid-flight when the gateway dies
  w.request({ type = "run", name = "keeper" })
  w.accept_assign()
  check("T12 run #2 in flight", w.pop_type("invoke") ~= nil)

  local w2 = new_world({ disk = w.disk }) -- same disk = same .crab_registry/.crab_jobs
  -- the worker still has the leftover slot from run #2; it heartbeats first
  w2.slots = w.slots
  w2.worker_heartbeat()
  w2.tick(6)
  check("T12 leftover job slot drained", w2.apply_drains() >= 1)
  local row = w2.list_row("keeper")
  check("T12 history survived the reboot", row ~= nil and row.ok == 1, row and row.ok)
  local logs = w2.request({ type = "job-logs", name = "keeper" })
  check("T12 output survived the reboot", logs.runs[1].output == "before reboot")
  check("T12 in-flight run was abandoned", logs.cur == nil)
  w2.auto_accept = true
  w2.tick(3600)
  w2.answer_invoke({ ok = true, result = "after reboot" })
  check("T12 schedule resumed", w2.list_row("keeper").runs >= 3)
end

print(("gateway_jobs_test: %d passed, %d failed"):format(passed, failed))
if failed > 0 then error("gateway jobs test FAILED") end
