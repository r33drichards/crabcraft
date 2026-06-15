-- E2E: the Rust auth.wasm running on the real wasmcraft engine, exercised
-- through the crab ABI with a FAKE in-memory sqlite wired as the mesh target
-- (load_reactor opts.call). Proves the module loads (so it is SIMD-free, which
-- the engine refuses), and that register -> sign -> verify works end to end,
-- including the Ed25519 sign/verify running on the interpreter. COBALT/CC only.
--   cd crabcraft && ../wasmcraft/tools/cobalt host/auth_smoke.lua jit
package.path = "host/?.lua;" .. package.path
local rt = require("runtime")
local cm = require("cmval")
local json = require("json")

local f = assert(io.open("modules/auth.wasm", "rb"))
local bytes = f:read("*a"); f:close()

-- ---- a tiny fake "sqlite" workload: just enough SQL for auth's statements ---
local rows = {} -- user_id -> { user_id, pubkey, username, meta }
local function reply(tbl) return json.encode(tbl) end
local function quoted(sql) -- pull successive '...'-quoted values in order
  local out, i = {}, 1
  for v in sql:gmatch("'(.-)'") do out[#out + 1] = v; i = i + 1 end
  return out
end
local function fake_sqlite(_wl, _fn, par)
  local sql = cm.decode("string", par)
  if sql:find("^CREATE TABLE") then
    return true, cm.encode({ kind = "result", ok = "string", err = "string" },
      { ok = reply({ columns = {}, rows = {}, changes = 0 }) })
  elseif sql:find("^INSERT INTO users") then
    local v = quoted(sql) -- user_id, pubkey, username, meta
    rows[v[1]] = { v[1], v[2], v[3], v[4] }
    return true, cm.encode({ kind = "result", ok = "string", err = "string" },
      { ok = reply({ columns = {}, rows = {}, changes = 1 }) })
  elseif sql:find("^SELECT pubkey") then
    local v = quoted(sql) -- WHERE user_id = '<id>'
    local r = rows[v[1]]
    local out = r and { { r[2], r[3], r[4] } } or {}
    return true, cm.encode({ kind = "result", ok = "string", err = "string" },
      { ok = reply({ columns = { "pubkey", "username", "meta" }, rows = out, changes = 0 }) })
  end
  return false, "fake sqlite: unhandled SQL: " .. sql
end

local t0 = os.clock()
local w = rt.load_reactor(bytes, { mode = arg and arg[1] or "transpile", call = fake_sqlite })
print(("loaded auth.wasm (%s) in %.1fs - SIMD-free (engine accepted it)")
  :format(w.mode or "?", os.clock() - t0))
assert(w.schema_json:find('"accounts"', 1, true), "schema missing accounts interface")

local resty = { kind = "result", ok = "string", err = "string" }
local unitres = { kind = "result", err = "string" } -- init: result<_, string>
local function call(addr, types, vals, rty)
  local r = w:invoke(addr, cm.encode_params(types, vals))
  assert(r.ok, "abi error: " .. tostring(r.err))
  return cm.decode(rty or resty, r.result)
end
local A = "crab:auth/accounts@0.1.0#"

local passed, failed = 0, 0
local function check(desc, cond)
  if cond then passed = passed + 1; print("ok   " .. desc)
  else failed = failed + 1; print("FAIL " .. desc) end
end

-- init + register
assert(not call(A .. "init", { "string" }, { "db" }, unitres).is_err, "init failed")
local reg = call(A .. "register", { "string", "string", "string" },
  { "db", "alice", '{"role":"admin"}' })
assert(not reg.is_err, "register: " .. tostring(reg.err))
local cred = json.decode(reg.ok)
check("register returns 64-hex public key", #cred.public_key == 64)
check("register returns 64-hex private key", #cred.private_key == 64)

-- sign a fresh nonce LOCALLY (this is what the turtle does)
local nonce = "nonce-" .. tostring(os.clock())
local t1 = os.clock()
local s = call(A .. "sign", { "string", "string" }, { cred.private_key, nonce })
assert(not s.is_err, "sign: " .. tostring(s.err))
check("sign returns 128-hex signature", #s.ok == 128)
print(("  (Ed25519 sign on the engine: %.2fs)"):format(os.clock() - t1))

-- verify the good signature
local t2 = os.clock()
local v = call(A .. "verify", { "string", "string", "string", "string" },
  { "db", cred.user_id, nonce, s.ok })
check("verify(valid) succeeds", not v.is_err)
if not v.is_err then
  local acct = json.decode(v.ok)
  check("verify returns the right user", acct.username == "alice")
  check("verify returns meta", acct.meta.role == "admin")
end
print(("  (Ed25519 verify on the engine: %.2fs)"):format(os.clock() - t2))

-- negatives: replayed signature for a different nonce, and an unknown user
check("verify(wrong nonce) denied",
  call(A .. "verify", { "string", "string", "string", "string" },
    { "db", cred.user_id, "different-nonce", s.ok }).is_err)
check("verify(unknown user) denied",
  call(A .. "verify", { "string", "string", "string", "string" },
    { "db", "deadbeefdeadbeef", nonce, s.ok }).is_err)

print(string.format("%d/%d passed", passed, passed + failed))
if failed == 0 then print("ALL_PASS") else print("FAILED") end
