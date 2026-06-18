-- Job-manifest + CLI test for host/crb.lua on plain Lua (5.4): the real crb
-- chunk runs against a stubbed client module, so deploys capture the wire
-- spec instead of needing a modem. Validates manifest -> deploy translation
-- (func resolution + arg encoding against the real hello schema), deploy-time
-- rejects, and the run/logs/ls rendering.
--   lua5.4 test/crb_jobs_test.lua
package.path = "host/?.lua;" .. package.path
local cm = require("cmval")
local schema_mod = require("schema")

local passed, failed = 0, 0
local function check(desc, cond, extra)
  if cond then passed = passed + 1
  else
    failed = failed + 1
    print("FAIL " .. desc .. (extra and (" -- " .. tostring(extra)) or ""))
  end
end

local HELLO_SCHEMA do
  local f = assert(io.open("guest/hello/gen/schema.json", "rb"))
  HELLO_SCHEMA = f:read("*a")
  f:close()
end
local GREET = "crab:hello/greeter@0.1.0#greet"

-- ---- stub client: capture what crb would put on the wire --------------------
local captured = { replies = {} }
local fakeC = {}
function fakeC:request(msg)
  captured.last = msg
  local r = captured.replies[msg.type]
  return r and r(msg) or { ok = true }
end
function fakeC:deploy(spec)
  captured.deploy = spec
  return { ok = true, output = "registered " .. spec.name .. " (stub)" }
end
function fakeC:run(name) return self:request({ type = "run", name = name }) end
function fakeC:job_logs(name) return self:request({ type = "job-logs", name = name }) end
function fakeC:list() return self:request({ type = "list" }) end
function fakeC:schema(name)
  local r = self:request({ type = "schema", name = name })
  if not r.ok then return nil, r.err end
  return r.schema, nil, r.kind
end
package.loaded.client = { connect = function() return fakeC end }

local function write_manifest(text)
  local f = assert(io.open("/tmp/crb_test_manifest.yml", "w"))
  f:write(text)
  f:close()
  return "/tmp/crb_test_manifest.yml"
end

local function run_crb(...)
  local chunk = assert(loadfile("host/crb.lua"))
  local out = {}
  local realprint = print
  _G.print = function(...)
    local p = {}
    for i = 1, select("#", ...) do p[i] = tostring(select(i, ...)) end
    out[#out + 1] = table.concat(p, " ")
  end
  local ok, err = pcall(chunk, ...)
  _G.print = realprint
  return ok, ok and table.concat(out, "\n") or tostring(err)
end

-- ---- func-job manifest -> wire spec ------------------------------------------
do
  local mf = write_manifest([[
name: greet-job
wasm: https://example/hello.wasm
kind: job
schema: guest/hello/gen/schema.json
func: greet
args:
  name: batch
  excited: true
schedule: "@every 5m"
retries: 2
timeout: 120
keep-warm: true
]])
  local ok, out = run_crb("deploy", mf)
  check("func job deploys", ok, out)
  local d = captured.deploy
  check("kind job", d.kind == "job")
  check("module reactor", d.module == "reactor")
  check("short func resolved to address", d.func == GREET, d.func)
  local sc = schema_mod.load(HELLO_SCHEMA)
  local want = cm.encode_params(sc.param_types(GREET), { { name = "batch", excited = true } })
  check("args encoded like the proxy would", d.params == want)
  check("schedule passed", d.schedule == "@every 5m")
  check("retries/timeout passed", d.retries == 2 and d.timeout == 120)
  check("keep-warm -> keep", d.keep == true)
  check("argv not sent for func jobs", d.args == nil)
end

-- ---- command-job manifest ------------------------------------------------------
do
  local mf = write_manifest([[
name: backup
wasm: https://example/tool.wasm
kind: job
body: '{"op":"dump"}'
schedule: "0 4 * * *"
]])
  local ok, out = run_crb("deploy", mf)
  check("command job deploys", ok, out)
  local d = captured.deploy
  check("module command", d.module == "command")
  check("body passed", d.body == '{"op":"dump"}')
  check("no func/params", d.func == nil and d.params == nil)
end

-- ---- deploy-time rejects --------------------------------------------------------
do
  local ok, err = run_crb("deploy", write_manifest(
    "name: x\nwasm: u\nkind: job\nfunc: greet\nschema: guest/hello/gen/schema.json\nschedule: \"61 * * * *\"\n"))
  check("bad schedule rejected at deploy", not ok and err:find("minute") ~= nil, err)
  ok, err = run_crb("deploy", write_manifest(
    "name: x\nwasm: u\nkind: reactor\nschedule: \"@every 5m\"\n"))
  check("schedule on a service rejected", not ok and err:find("kind: job") ~= nil, err)
  ok, err = run_crb("deploy", write_manifest(
    "name: x\nwasm: u\nkind: job\nfunc: nosuch\nschema: guest/hello/gen/schema.json\n"))
  check("unknown func rejected", not ok and err:find("no function") ~= nil, err)
  ok, err = run_crb("deploy", write_manifest(
    "name: x\nwasm: u\nkind: job\nfunc: greet\n"))
  check("func job without schema rejected", not ok and err:find("schema") ~= nil, err)
  ok, err = run_crb("deploy", write_manifest(
    "name: x\nwasm: u\nkind: job\nfunc: greet\nschema: guest/hello/gen/schema.json\nargs:\n  excited: true\n"))
  check("missing required arg rejected", not ok, err)
end

-- ---- crb run ---------------------------------------------------------------------
do
  captured.replies["run"] = function() return { ok = true, output = "queued run #4 of 'backup'" } end
  local ok, out = run_crb("run", "backup")
  check("crb run sends run", ok and captured.last.type == "run" and captured.last.name == "backup")
  check("crb run prints the ack", out:find("queued run #4", 1, true) ~= nil, out)
end

-- ---- crb logs: func-job output decodes through the schema -------------------------
do
  local enc = cm.encode("string", "Hello, batch!!!")
  captured.replies["job-logs"] = function()
    return { ok = true, module = "reactor", func = GREET, seq = 2,
      runs = {
        { n = 1, ok = false, t = os.time() - 90, err = "boom", tries = 1 },
        { n = 2, ok = true, t = os.time() - 30, dur = 2.5, output = enc },
      },
      cur = { n = 3, phase = "pending" } }
  end
  captured.replies["schema"] = function()
    return { ok = true, schema = HELLO_SCHEMA, kind = "job" }
  end
  local ok, out = run_crb("logs", "greet-job")
  check("logs runs", ok, out)
  check("logs shows the failure", out:find("run #1  FAILED", 1, true) and out:find("boom", 1, true))
  check("logs decodes the typed output", out:find("Hello, batch!!!", 1, true) ~= nil, out)
  check("logs notes retries", out:find("retries=1", 1, true) ~= nil, out)
  check("logs shows the active run", out:find("run #3 pending", 1, true) ~= nil, out)
  local ok2, out2 = run_crb("logs", "greet-job", "1")
  check("logs n limits history", ok2 and not out2:find("run #1", 1, true) and out2:find("run #2", 1, true), out2)
end

-- ---- crb ls renders job rows --------------------------------------------------------
do
  captured.replies["list"] = function()
    return { ok = true, workers = {}, workloads = {
      { name = "hello", kind = "reactor", state = "running", worker = 2, slot = "disk" },
      { name = "greet-job", kind = "job", state = "succeeded", schedule = "@every 5m",
        runs = 7, ok = 7, fail = 0, last = { n = 7, ok = true, t = os.time() - 12 } },
    } }
  end
  local ok, out = run_crb("ls")
  check("ls runs", ok, out)
  check("ls shows the service row", out:find("worker=2", 1, true) ~= nil, out)
  check("ls shows the job row", out:find("runs=7", 1, true) and out:find("@every 5m", 1, true)
    and out:find("ok 12s ago", 1, true), out)
end

print(("crb_jobs_test: %d passed, %d failed"):format(passed, failed))
if failed > 0 then error("crb jobs test FAILED") end
