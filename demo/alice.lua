-- alice: a SLOW Picat job in session 'alice' (naive exponential fib).
-- Run alice, then run bob (another computer or multishell tab) while she
-- works: bob answers instantly - sessions execute concurrently.
--   alice        (fib 27; first call per session also boots the engine)
--   alice 29     (slower)
local LIBURL = "https://github.com/r33drichards/crabcraft/releases/latest/download/crblib.lua"
if not fs.exists("crblib") then
  local r = assert(http.get(LIBURL), "cannot fetch crblib")
  local h = fs.open("crblib", "w") h.write(r.readAll()) h.close() r.close()
end
local lib = dofile("crblib")

local n = tonumber(({ ... })[1]) or 27
local picat = lib.client.connect():workload("picat")
print(("alice: naive fib(%d) - this should take a while..."):format(n))
local t0 = os.clock()
local out = picat(([[
slowfib(0) = 1.
slowfib(1) = 1.
slowfib(N) = slowfib(N-1) + slowfib(N-2).

main => println(slowfib(%d)).
]]):format(n), "alice")
print(("alice: fib(%d) = %s  (%.1fs)"):format(n, tostring(out):gsub("%s+$", ""), os.clock() - t0))
