-- cardui: a React monitor kiosk gated on public-key cardlock auth. One computer
-- with a monitor + disk drive + wireless modem renders web/cardui (the React
-- sign-in/sign-up UI) via the wasmcraft web engine; monitor taps drive the auth
-- flow over a console.log/web_message bridge, and a valid card pulses the door.
--
--   wget https://github.com/r33drichards/crabcraft/releases/latest/download/cardui.lua cardui
--   cardui init     -- create the users table (once)
--   cardui          -- kiosk mode: locked screen on the monitor
--
-- Reader-station hardware: a wireless modem (reach the gateway), a disk drive
-- (cards + the local signer), a monitor (the UI), and enough room for the web
-- engine + auth.wasm (fetched on first run). The screen-flow logic lives in
-- cardui_core.lua (shared with host/cardui_smoke.lua); this file wires the real
-- engine, mesh client, peripherals, and event loop around it.
local WASM = "https://github.com/r33drichards/wasmcraft/releases/latest/download/"
local CRAB = "https://github.com/r33drichards/crabcraft/releases/latest/download/"
local FILES = {
  ["wasmcraft"]    = WASM .. "wasmcraft.lua", -- engine bundle (wasm interp/jit + wasi)
  ["webrender"]    = WASM .. "webrender.lua", -- frame-buffer renderer
  ["web.wasm"]     = WASM .. "web.wasm",      -- web engine (QuickJS + draw protocol)
  ["cardui.html"]  = CRAB .. "cardui.html",   -- page that mounts #root + cardapp.js
  ["cardapp.js"]   = CRAB .. "cardapp.js",    -- the bundled React app
  ["cardlib"]      = CRAB .. "cardlib.lua",   -- mesh client + crab runtime + json/cmval
  ["cardui_core"]  = CRAB .. "cardui_core.lua",
  ["auth.wasm"]    = CRAB .. "auth.wasm",     -- local signer (Ed25519)
}

local STORE, AUTH = "sqlite", "auth"
local CARDFILE, DOOR_SIDE = "card.json", "back"
local A = "crab:auth/accounts@0.1.0#"

-- fetch a file on first use (binary for .wasm); local copy always wins.
local function ensure(name)
  if fs.exists(name) then return name end
  local url = FILES[name] or error("cardui: no source url for " .. name, 0)
  local bin = name:match("%.wasm$") ~= nil
  io.write("fetching " .. name .. " ... ")
  local r = assert(http.get(url, nil, bin), "cannot fetch " .. url)
  local h = fs.open(name, bin and "wb" or "w"); h.write(r.readAll()); h.close(); r.close()
  print("ok")
  return name
end

local lib = dofile(ensure("cardlib"))   -- { client, runtime, json, cmval, schema }

-- ---- mesh client (register / verify run on the auth workload) ---------------
local C = assert(lib.client.connect(), "cannot reach the gateway")
local authwl = C:workload(AUTH)
local auth = { verify = authwl["verify"], register = authwl["register"] }

-- ---- card (floppy) helpers (lifted from demo/cardlock.lua) ------------------
local function find_disk()
  for _, side in ipairs(peripheral.getNames and peripheral.getNames() or {}) do
    if peripheral.getType(side) == "drive" and disk.isPresent(side) and disk.hasData(side) then
      return side
    end
  end
end
local function card_path(side) return fs.combine(disk.getMountPath(side), CARDFILE) end
local function read_card(side)
  local p = card_path(side)
  if not fs.exists(p) then return nil end
  local h = fs.open(p, "r"); local s = h.readAll(); h.close()
  local ok, c = pcall(textutils.unserializeJSON or function(x) return lib.json.decode(x) end, s)
  return ok and c or nil
end
local function write_card(side, card)
  local h = fs.open(card_path(side), "w")
  h.write((textutils.serializeJSON or lib.json.encode)(card)); h.close()
  pcall(disk.setLabel, side, "card")
end
local function gen_nonce()
  math.randomseed((os.epoch and os.epoch("utc") or os.time()) + os.clock() * 1e6)
  local t = {}
  for i = 1, 32 do t[i] = ("%x"):format(math.random(0, 15)) end
  return table.concat(t)
end
local function door()
  if rs and rs.setOutput then rs.setOutput(DOOR_SIDE, true); sleep(2); rs.setOutput(DOOR_SIDE, false) end
end
local function read_username()
  io.write("username: "); return read()
end

-- ---- local signer: run auth.wasm on THIS machine to sign the challenge ------
-- (sign touches no storage, so the private key never leaves the turtle.)
local function make_signer()
  local h = fs.open(ensure("auth.wasm"), "rb"); local bytes = h.readAll(); h.close()
  local w = lib.runtime.load_reactor(bytes, { mode = "transpile" })
  local resty = { kind = "result", ok = "string", err = "string" }
  return function(private_key, nonce)
    local r = w:invoke(A .. "sign", lib.cmval.encode_params({ "string", "string" }, { private_key, nonce }))
    assert(r.ok, "sign abi error: " .. tostring(r.err))
    local d = lib.cmval.decode(resty, r.result)
    if d.is_err then error("sign: " .. tostring(d.err), 0) end
    return d.ok
  end
end

-- ---- subcommands -----------------------------------------------------------
local cmd = ({ ... })[1]
if cmd == "init" then
  local r = authwl["init"]({ store = STORE })
  if r.is_err then error("init failed: " .. tostring(r.err), 0) end
  print("users table ready on '" .. STORE .. "'")
  return
end

-- ---- web engine: load web.wasm + render CardApp on the monitor --------------
-- (start_engine / wstr / device + render adapted from wasmcraft dist/browser.lua)
local engine = dofile(ensure("wasmcraft"))
local WR = dofile(ensure("webrender"))
ensure("cardui.html"); ensure("cardapp.js")

local function wstr(inst, s)
  local p = inst:call("web_malloc", #s + 1)
  inst.memory:storestr(p, s); inst.memory:set8(p + #s, 0)
  return p
end

-- device: prefer a monitor, fall back to the terminal
local function pick_device()
  if type(peripheral) == "table" and peripheral.find then
    local mon = peripheral.find("monitor"); if mon then return "monitor", mon end
  end
  return "term", term
end
local kind, dev = pick_device()
if kind == "monitor" and dev.setPaletteColour and term.nativePaletteColour then
  for i = 0, 15 do dev.setPaletteColour(2 ^ i, term.nativePaletteColour(2 ^ i)) end
end
local cols, drows = 51, nil
if dev.setTextScale then dev.setTextScale(1) end
cols, drows = dev.getSize()

local function render(fr)
  dev.setBackgroundColor(1); dev.clear()
  WR.paint_blit(dev, fr, drows)
end

-- React -> host commands arrive on stderr as "\1CRB <json>" lines.
local cmds = {}
local function errfn(s)
  for line in (s .. "\n"):gmatch("([^\n]*)\n") do
    local body = line:match("^\1CRB (.+)$")
    if body then local ok, c = pcall(lib.json.decode, body); if ok then cmds[#cmds + 1] = c end end
  end
end

local function start_engine(bytes, writefn)
  if engine.set_yield and type(os) == "table" and os.queueEvent and os.pullEvent then
    engine.set_yield(function() os.queueEvent("cardui_yield"); os.pullEvent("cardui_yield") end, 200000)
  end
  local wasi = engine.wasi
  local module = engine.load(bytes)
  local hostfs = (engine.hostfs and engine.hostfs(".")) or (wasi.io_hostfs and wasi.io_hostfs("."))
  local host = wasi.make({ write = writefn, writeerr = errfn, args = { "web.wasm" }, fs = hostfs, root = "." })
  local inst = engine.instantiate(module, { wasi_snapshot_preview1 = host }, { mode = "transpile" })
  inst:call("_initialize")
  return inst
end

local webbytes = (function() local h = fs.open("web.wasm", "rb"); local b = h.readAll(); h.close(); return b end)()
local sink = WR.line_sink(WR.make_parser(render))
local inst = start_engine(webbytes, sink)

-- first frame
local pp = wstr(inst, "cardui.html"); inst:call("web_init", pp, cols); inst:call("web_free", pp)

-- ---- wire the shared core + run the event loop -----------------------------
local core = dofile(ensure("cardui_core"))({
  inst = inst, cmds = cmds, json = lib.json, store = STORE,
  auth = auth, sign = make_signer(),
  read_card = read_card, write_card = write_card, gen_nonce = gen_nonce,
  read_username = read_username, door = door,
})

print("cardui ready - tap the monitor to begin (Ctrl+T to quit)")
while true do
  local ev = { os.pullEvent() }
  local e = ev[1]
  if e == "monitor_touch" or e == "mouse_click" then
    core.on_tap(ev[3], ev[4])
  elseif e == "disk" then
    local side = find_disk(); if side then core.on_disk(side) end
  elseif (e == "char" and ev[2] == "q") or e == "terminate" then
    break
  end
end
