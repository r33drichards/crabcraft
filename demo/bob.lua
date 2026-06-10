-- bob: an INSTANT Picat job in session 'bob'. Run while alice is mid-solve
-- to see shared execution: bob is not stuck behind her.
--   bob          (first call boots bob's engine - run once to warm him up)
local LIBURL = "https://github.com/r33drichards/crabcraft/releases/latest/download/crblib.lua"
if not fs.exists("crblib") then
  local r = assert(http.get(LIBURL), "cannot fetch crblib")
  local h = fs.open("crblib", "w") h.write(r.readAll()) h.close() r.close()
end
local lib = dofile("crblib")

local picat = lib.client.connect():workload("picat")
local t0 = os.clock()
local out = picat([[
main => X = 2 + 3, printf("bob says %w\n", X).
]], "bob")
print(("bob: %s  (%.1fs)"):format(tostring(out):gsub("%s+$", ""), os.clock() - t0))
