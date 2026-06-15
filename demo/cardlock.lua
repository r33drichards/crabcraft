-- cardlock: crabcraft public-key door. A ComputerCraft floppy is the "card"
-- and carries an Ed25519 PRIVATE key; the `auth` workload stores only the
-- matching PUBLIC key. Login is challenge-response: the reader makes a fresh
-- nonce, signs it LOCALLY with the floppy's key (so the key never leaves the
-- turtle), and asks `auth.verify` to check it against the stored public key.
--
--   wget https://github.com/r33drichards/crabcraft/releases/latest/download/cardlock.lua cardlock
--   cardlock init                         -- create the users table (once)
--   cardlock enroll alice {"role":"admin"}  -- register + write a fresh card
--   cardlock                              -- door mode: tap a card to unlock
--
-- enroll/init are thin (client only). Door mode also runs the wasmcraft engine
-- locally to sign, so a reader turtle needs: a wireless modem (reach gateway),
-- a disk drive, and enough room for the engine + auth.wasm (fetched on first run).
local LIBURL = "https://github.com/r33drichards/crabcraft/releases/latest/download/cardlib.lua"
local AUTHWASM_URL = "https://github.com/r33drichards/crabcraft/releases/latest/download/auth.wasm"

local STORE = "sqlite"        -- sqlite workload holding the users table
local AUTH = "auth"           -- the auth workload (for verify over the mesh)
local CARDFILE = "card.json"  -- {user_id, private_key} written on the floppy
local DOOR_SIDE = "back"      -- redstone side pulsed on a successful login
local A = "crab:auth/accounts@0.1.0#"

-- ---- bootstrap the card-reader runtime (client + local wasm engine) --------
if not fs.exists("cardlib") then
  io.write("fetching cardlib ... ")
  local r = assert(http.get(LIBURL), "cannot fetch cardlib")
  local h = fs.open("cardlib", "w") h.write(r.readAll()) h.close() r.close()
  print("ok")
end
local lib = dofile("cardlib")
local C = lib.client.connect()
local auth = C:workload(AUTH)

-- ---- thin (client-only) operations -----------------------------------------
local function init()
  return auth["init"]({ store = STORE })
end
local function register(username, meta)
  return auth["register"]({ store = STORE, username = username, meta = meta or "{}" })
end
local function verify(user_id, nonce, signature)
  return auth["verify"]({
    store = STORE, ["user-id"] = user_id, nonce = nonce, signature = signature,
  })
end

-- ---- card (floppy) helpers -------------------------------------------------
local function find_disk()
  for _, side in ipairs(peripheral.getNames and peripheral.getNames() or {}) do
    if peripheral.getType(side) == "drive" and disk.isPresent(side) and disk.hasData(side) then
      return side
    end
  end
end
local function wait_for_card(prompt)
  local side = find_disk()
  if side then return side end
  print(prompt or "insert a card (floppy) ...")
  while not side do os.pullEvent("disk"); side = find_disk() end
  return side
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
end
local function gen_nonce()
  math.randomseed((os.epoch and os.epoch("utc") or os.time()) + os.clock() * 1e6)
  local t = {}
  for i = 1, 32 do t[i] = ("%x"):format(math.random(0, 15)) end
  return table.concat(t)
end

-- ---- local signer: load auth.wasm on THIS machine to sign the challenge ----
-- (sign touches no storage, so no mesh is wired; the private key stays local.)
local function make_signer()
  if not fs.exists("auth.wasm") then
    io.write("fetching auth.wasm ... ")
    local r = assert(http.get(AUTHWASM_URL, nil, true), "cannot fetch auth.wasm")
    local h = fs.open("auth.wasm", "wb") h.write(r.readAll()) h.close() r.close()
    print("ok")
  end
  local h = fs.open("auth.wasm", "rb"); local bytes = h.readAll(); h.close()
  io.write("loading signer (engine) ... ")
  local w = lib.runtime.load_reactor(bytes, { mode = "transpile" })
  print("ok")
  local resty = { kind = "result", ok = "string", err = "string" }
  return function(private_key, nonce)
    local r = w:invoke(A .. "sign",
      lib.cmval.encode_params({ "string", "string" }, { private_key, nonce }))
    assert(r.ok, "sign abi error: " .. tostring(r.err))
    local d = lib.cmval.decode(resty, r.result)
    if d.is_err then error("sign: " .. tostring(d.err), 0) end
    return d.ok
  end
end

-- ---- subcommands -----------------------------------------------------------
local cmd = ({ ... })[1]

if cmd == "init" then
  local r = init()
  if r.is_err then error("init failed: " .. tostring(r.err), 0) end
  print("users table ready on '" .. STORE .. "'")

elseif cmd == "enroll" then
  local username = ({ ... })[2] or error("usage: cardlock enroll <username> [json-meta]", 0)
  local meta = ({ ... })[3] or "{}"
  local r = register(username, meta)
  if r.is_err then error("register failed: " .. tostring(r.err), 0) end
  local cred = lib.json.decode(r.ok) -- { user_id, public_key, private_key }
  local side = wait_for_card("insert a BLANK floppy to write " .. username .. "'s card ...")
  write_card(side, { user_id = cred.user_id, private_key = cred.private_key })
  pcall(disk.setLabel, side, "card:" .. username)
  print(("enrolled %s -> user_id %s"):format(username, cred.user_id))
  print("card written. keep it safe - the private key is only on the floppy.")

else
  -- door / reader mode: needs the local signer
  local sign = make_signer()
  print("cardlock ready - tap a card on the drive (Ctrl+T to quit)")
  while true do
    local side = wait_for_card()
    local card = read_card(side)
    if not (card and card.user_id and card.private_key) then
      print("unrecognized card (no " .. CARDFILE .. ")")
    else
      local nonce = gen_nonce()
      local ok, sig = pcall(sign, card.private_key, nonce)
      if not ok then
        print("sign error: " .. tostring(sig))
      else
        local r = verify(card.user_id, nonce, sig)
        if r.is_err then
          print("DENIED (" .. card.user_id .. "): " .. tostring(r.err))
        else
          local acct = lib.json.decode(r.ok)
          print("WELCOME " .. tostring(acct.username))
          if rs and rs.setOutput then
            rs.setOutput(DOOR_SIDE, true); sleep(2); rs.setOutput(DOOR_SIDE, false)
          end
        end
      end
    end
    while find_disk() == side do os.pullEvent("disk_eject") end -- debounce
  end
end
