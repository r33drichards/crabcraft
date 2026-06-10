-- Smoke test for the C sqlite workload (guest/sqlite-c -> modules/sqlite.wasm):
-- the crab ABI implemented in plain C, with disk persistence through WASI
-- (opts.root is the workload volume; crab.db must survive a fresh instance).
-- Run with Cobalt from the repo root:
--   /Users/robertwendt/wasmcraft/tools/cobalt host/sqlite_smoke.lua /tmp/some-empty-dir
-- arg[1] = volume root (REQUIRED, should be an empty temp dir), arg[2] = mode.
package.path = "host/?.lua;" .. package.path
local rt = require("runtime")
local cm = require("cmval")
local json = require("json")

local a1, a2 = ...
local root = assert(a1 or (arg and arg[1]), "usage: sqlite_smoke.lua <volume-root-dir> [mode]")
local mode = a2 or (arg and arg[2]) or "transpile"

local f = assert(io.open("modules/sqlite.wasm", "rb"))
local bytes = f:read("*a"); f:close()

local res_ty = { kind = "result", ok = "string", err = "string" }
local fn = "crab:sqlite/db@0.1.0#exec"

local function exec(w, sql)
  local r = w:invoke(fn, cm.encode_params({ "string" }, { sql }))
  assert(r.ok, "ABI-level error: " .. tostring(r.err)) -- WIT err is NOT an ABI error
  return cm.decode(res_ty, r.result)
end

local passed, failed = 0, 0
local function check(desc, cond, detail)
  if cond then passed = passed + 1; print("ok   " .. desc)
  else failed = failed + 1; print("FAIL " .. desc .. (detail and (": " .. tostring(detail)) or "")) end
end

local t0 = os.clock()
local w = rt.load_reactor(bytes, { mode = mode, root = root })
print(("loaded sqlite.wasm (%s mode) in %.1fs"):format(w.mode or "?", os.clock() - t0))
check("schema mentions crab:sqlite", w.schema_json:find("crab:sqlite", 1, true) ~= nil)

-- 1. CREATE TABLE
local r = exec(w, "CREATE TABLE t(a,b)")
check("CREATE TABLE ok", r.is_ok, r.err)
local doc = json.decode(r.ok)
check("CREATE returns JSON doc", #doc.columns == 0 and #doc.rows == 0)

-- 2. INSERT two rows, changes=2
r = exec(w, "INSERT INTO t VALUES(1,'x'),(2,'y')")
check("INSERT ok", r.is_ok, r.err)
doc = json.decode(r.ok)
check("INSERT changes=2", doc.changes == 2, "changes=" .. tostring(doc.changes))

-- 3. SELECT both rows back
r = exec(w, "SELECT * FROM t ORDER BY a")
check("SELECT ok", r.is_ok, r.err)
print("SELECT json: " .. r.ok)
doc = json.decode(r.ok)
check("SELECT columns a,b", doc.columns[1] == "a" and doc.columns[2] == "b")
check("SELECT two rows", #doc.rows == 2)
check("row 1 = [1,'x']", doc.rows[1][1] == 1 and doc.rows[1][2] == "x")
check("row 2 = [2,'y']", doc.rows[2][1] == 2 and doc.rows[2][2] == "y")

-- 4. broken SQL -> WIT-level err case with a message (still ABI-ok)
r = exec(w, "BROKEN SQL")
check("broken SQL is WIT err", r.is_err)
check("broken SQL has message", type(r.err) == "string" and #r.err > 0, r.err)
print("sqlite error message: " .. tostring(r.err))

-- 5. only one statement per exec (documented behavior)
r = exec(w, "SELECT 1; SELECT 2")
check("multi-statement rejected", r.is_err and r.err:find("one statement") ~= nil, r.err)

-- 6. PERSISTENCE: a brand-new instance over the same volume must see the rows
local w2 = rt.load_reactor(bytes, { mode = mode, root = root })
r = exec(w2, "SELECT count(*) AS n, group_concat(b) AS bs FROM t")
check("2nd instance SELECT ok", r.is_ok, r.err)
if r.is_ok then
  doc = json.decode(r.ok)
  check("persistence: 2 rows on disk", doc.rows[1][1] == 2, json.encode(doc))
  check("persistence: values intact", doc.rows[1][2] == "x,y", json.encode(doc))
end

print(string.format("%d/%d passed", passed, passed + failed))
if failed == 0 then print("ALL_PASS") else print("FAILED") end
