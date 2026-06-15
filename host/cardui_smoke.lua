-- E2E bridge test for the cardui kiosk. Drives the REAL wasmcraft web engine
-- (the build with web_message) rendering CardApp, plus a FAKE mesh + fake card
-- I/O, through demo/cardui_core.lua. Asserts the full bridge round-trip
-- tap -> {op} command -> host action -> web_message -> updated frame for
-- sign-in (granted + denied) and sign-up (enrolled). Crypto is NOT under test
-- here (host/auth_smoke.lua covers register/sign/verify on the real engine); the
-- fake mesh returns canned results, so the signer is a stub. COBALT only.
--   nix-shell --run \
--     "../../../wasmcraft/.worktrees/web-message-channel/tools/cobalt host/cardui_smoke.lua [jit]"
local MODE = (arg and arg[1]) or "jit"

local lib = assert(loadfile("dist/cardlib.lua"))()        -- for lib.json
local json = lib.json

-- ---- file helpers ----------------------------------------------------------
local function read_file(p)
  local f = assert(io.open(p, "rb"), "cannot open " .. p)
  local d = f:read("*a"); f:close(); return d
end
local function write_file(p, s)
  local f = assert(io.open(p, "wb"), "cannot write " .. p)
  f:write(s); f:close()
end

-- co-locate the page + its bundle so the engine can fopen cardapp.js relative to
-- the mounted root (test/vendor is gitignored; web.wasm already lives there).
local ROOT = "test/vendor"
write_file(ROOT .. "/cardui.html", read_file("web/cardui/cardui.html"))
write_file(ROOT .. "/cardapp.js", read_file("web/site/cardapp.js"))

-- ---- load the wasmcraft web engine bundle + drive web.wasm -----------------
local engine = assert(loadfile(ROOT .. "/wasmcraft.lua"))()
local bytes = read_file(ROOT .. "/web.wasm")

local frames, cmds = {}, {}
local function writefn(s) frames[#frames + 1] = s end
local function errfn(s)
  for line in (s .. "\n"):gmatch("([^\n]*)\n") do
    local body = line:match("^\1CRB (.+)$")
    if body then
      local ok, c = pcall(json.decode, body)
      if ok then cmds[#cmds + 1] = c end
    end
  end
end
local function frame() local s = table.concat(frames); frames = {}; return s end

local wasi = engine.wasi
local module = engine.load(bytes)
local hostfs = (engine.hostfs and engine.hostfs(ROOT))
            or (wasi.io_hostfs and wasi.io_hostfs(ROOT))
local host = wasi.make({
  write = writefn, writeerr = errfn, args = { "web.wasm" }, fs = hostfs, root = ROOT,
})
local inst = engine.instantiate(module, { wasi_snapshot_preview1 = host }, { mode = MODE })
inst:call("_initialize")

local function wstr(s)
  local p = inst:call("web_malloc", #s + 1)
  inst.memory:storestr(p, s); inst.memory:set8(p + #s, 0)
  return p
end

-- first frame
local pp = wstr("cardui.html"); inst:call("web_init", pp, 51); inst:call("web_free", pp)
local f0 = frame()

-- ---- assertion harness (mirrors host/auth_smoke.lua) -----------------------
local passed, failed = 0, 0
local function check(desc, cond)
  if cond then passed = passed + 1; print("ok   " .. desc)
  else failed = failed + 1; print("FAIL " .. desc) end
end
-- draw protocol text line: "T x y fg bg <text>"; return 0-based x,y of the first
-- line containing `sub` (loose on the colour fields so it survives format tweaks).
local function find_cell(fr, sub)
  for line in (fr .. "\n"):gmatch("([^\n]*)\n") do
    local x, y = line:match("^T (%d+) (%d+) ")
    if x and line:find(sub, 1, true) then return tonumber(x), tonumber(y) end
  end
end

check("first frame renders LOCKED", f0:find("LOCKED", 1, true) ~= nil)
if not f0:find("LOCKED", 1, true) then
  print(("%d/%d assertions passed"):format(passed, passed + failed)); print("FAILED"); return
end

-- ---- fakes + the core under test -------------------------------------------
local door_pulses = 0
local fake = { card = { user_id = "u1", private_key = "deadbeef" },
               verify_result = nil, register_result = nil, written = nil }

local core_factory = assert(loadfile("demo/cardui_core.lua"))()
local core = core_factory({
  inst = inst, cmds = cmds, json = json, store = "sqlite",
  auth = {
    verify = function(_) return fake.verify_result end,
    register = function(_) return fake.register_result end,
  },
  sign = function(_, _) return "00" end,           -- canned (crypto not under test)
  read_card = function(_) return fake.card end,
  write_card = function(_, c) fake.written = c end,
  gen_nonce = function() return "nonce" end,
  read_username = function() return "bob" end,
  door = function() door_pulses = door_pulses + 1 end,
})

-- simulate a 1-based monitor tap on the cell holding `label` in frame `fr`
local function tap(fr, label)
  local x, y = find_cell(fr, label)
  assert(x, "button not found in frame: " .. label)
  core.on_tap(x + 1, y + 1)
  return frame()
end

-- 1. sign in -> await card (React flips to awaitcard; host pushes status note)
local f1 = tap(f0, "Sign in")
check("tap Sign in -> await-card prompt", f1:find("Tap your card", 1, true) ~= nil)

-- 2. card present, mesh verify granted -> Welcome + door pulse
fake.verify_result = { is_err = false,
  ok = json.encode({ user_id = "u1", username = "alice", meta = { role = "admin" } }) }
core.on_disk("right"); local f2 = frame()
check("granted -> Welcome alice", f2:find("Welcome", 1, true) ~= nil and f2:find("alice", 1, true) ~= nil)
check("granted pulses the door", door_pulses == 1)

-- 3. relock, sign in again, mesh verify denied -> DENIED
local fl = tap(f2, "Lock")
check("Lock -> back to LOCKED", fl:find("LOCKED", 1, true) ~= nil)
local f3 = tap(fl, "Sign in")
check("re sign-in -> await card again", f3:find("Tap your card", 1, true) ~= nil)
fake.verify_result = { is_err = true, err = "access denied" }
core.on_disk("right"); local f4 = frame()
check("denied -> DENIED frame", f4:find("DENIED", 1, true) ~= nil)

-- 4. sign up -> enroll -> write card (register_result set BEFORE the tap, since
--    handle(signup) registers synchronously during on_tap)
local fl2 = tap(f4, "Back")
check("DENIED Back -> LOCKED", fl2:find("LOCKED", 1, true) ~= nil)
fake.register_result = { is_err = false,
  ok = json.encode({ user_id = "7c12", public_key = "pk", private_key = "sk" }) }
local f5 = tap(fl2, "Sign up")
check("sign up -> Enrolling prompt", f5:find("Enrolling", 1, true) ~= nil)
core.on_disk("right"); local f6 = frame()
check("enrolled -> shows user_id 7c12", f6:find("7c12", 1, true) ~= nil)
check("enrolled wrote the card", fake.written ~= nil and fake.written.user_id == "7c12")

print(("%d/%d assertions passed"):format(passed, passed + failed))
print(failed == 0 and "ALL_PASS" or "FAILED")
