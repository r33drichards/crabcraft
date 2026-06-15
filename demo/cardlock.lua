-- cardlock: crabcraft demo - a floppy-disk "NFC card" reader backed by the
-- `auth` workload (argon2) + a sqlite workload (storage). A floppy IS the card:
-- its disk id is the card id, and a random key file on it (auth.key) is the
-- secret. Insert a card to log in; the secret is argon2id-verified server-side
-- and never stored in the clear.
--
--   wget https://github.com/r33drichards/crabcraft/releases/latest/download/cardlock.lua cardlock
--   cardlock init                      -- create the users table (once)
--   cardlock enroll alice {"role":"admin"}   -- then insert a blank floppy
--   cardlock                           -- reader/door mode: tap a card to unlock
--
-- Hardware: a computer + wireless modem (to reach the gateway) + a disk drive.
-- In door mode it pulses redstone on the configured side when a card is valid.
local LIBURL = "https://github.com/r33drichards/crabcraft/releases/latest/download/crblib.lua"

local STORE = "sqlite"        -- name of the sqlite workload holding the users
local AUTH = "auth"           -- name of the auth workload
local KEYFILE = "auth.key"    -- secret file written on the card (the floppy)
local DOOR_SIDE = "back"      -- redstone side pulsed on a successful login

-- ---- bootstrap the client runtime (same pattern as demo/pets.lua) ----------
if not fs.exists("crblib") then
  io.write("fetching crblib ... ")
  local r = assert(http.get(LIBURL), "cannot fetch crblib")
  local h = fs.open("crblib", "w") h.write(r.readAll()) h.close() r.close()
  print("ok")
end
local lib = dofile("crblib")
local C = lib.client.connect()
local auth = C:workload(AUTH)

-- typed proxy calls keep the WIT kebab names for functions AND params.
local function init()
  return auth["init"]({ store = STORE })
end
local function enroll(username, card_id, secret, meta)
  return auth["enroll-card"]({
    store = STORE, username = username,
    ["card-id"] = card_id, ["card-secret"] = secret, meta = meta or "{}",
  })
end
local function login(card_id, secret)
  return auth["login-card"]({
    store = STORE, ["card-id"] = card_id, ["card-secret"] = secret,
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
  while true do
    os.pullEvent("disk")
    side = find_disk()
    if side then return side end
  end
end

local function card_id(side) return tostring(disk.getID(side)) end

local function read_secret(side)
  local path = fs.combine(disk.getMountPath(side), KEYFILE)
  if not fs.exists(path) then return nil end
  local h = fs.open(path, "r"); local s = h.readAll(); h.close()
  return (s:gsub("%s+$", ""))
end

-- 32 hex chars of entropy for a fresh card. CC's RNG is weak; for a real
-- deployment seed from a better source. Good enough for a high-entropy demo.
local function gen_secret()
  local t = {}
  math.randomseed((os.epoch and os.epoch("utc") or os.time()) + os.clock() * 1e6)
  for i = 1, 32 do t[i] = ("%x"):format(math.random(0, 15)) end
  return table.concat(t)
end

local function write_secret(side, secret)
  local path = fs.combine(disk.getMountPath(side), KEYFILE)
  local h = fs.open(path, "w"); h.write(secret); h.close()
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
  local side = wait_for_card("insert a BLANK floppy to enroll " .. username .. " ...")
  local id = card_id(side)
  local secret = read_secret(side)
  if not secret then               -- blank card: write a fresh key
    secret = gen_secret()
    write_secret(side, secret)
    disk.setLabel(side, "card:" .. username)
  end
  local r = enroll(username, id, secret, meta)
  if r.is_err then error("enroll failed: " .. tostring(r.err), 0) end
  print(("enrolled %s -> card %s"):format(username, id))

else
  -- reader / door mode
  print("cardlock ready - tap a card on the drive (Ctrl+T to quit)")
  while true do
    local side = wait_for_card()
    local id = card_id(side)
    local secret = read_secret(side)
    if not secret then
      print("card " .. id .. ": not provisioned (no " .. KEYFILE .. ")")
    else
      local r = login(id, secret)
      if r.is_err then
        print("DENIED card " .. id .. ": " .. tostring(r.err))
      else
        local acct = lib.json.decode(r.ok)
        print("WELCOME " .. tostring(acct.username) .. " (card " .. id .. ")")
        if rs and rs.setOutput then
          rs.setOutput(DOOR_SIDE, true); sleep(2); rs.setOutput(DOOR_SIDE, false)
        end
      end
    end
    -- debounce: wait for the card to be removed before reading again
    while find_disk() == side do os.pullEvent("disk_eject") end
  end
end
