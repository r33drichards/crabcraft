-- fib: crabcraft demo - run a Picat program on the cluster from a script.
--   crb deploy picat.yml      (manifests/picat.yml)
--   wget .../fib.lua fib
--   fib            (or: fib 20)
local LIBURL = "https://github.com/r33drichards/crabcraft/releases/latest/download/crblib.lua"
if not fs.exists("crblib") then
  local r = assert(http.get(LIBURL), "cannot fetch crblib")
  local h = fs.open("crblib", "w") h.write(r.readAll()) h.close() r.close()
end
local lib = dofile("crblib")

local n = tonumber(({ ... })[1]) or 10
local PROGRAM = ([[
fib(0) = 1.
fib(1) = 1.
fib(N) = fib(N-1) + fib(N-2).

main => println(fib(%d)).
]]):format(n)

local C = lib.client.connect()
local picat = C:workload("picat")
print(("running fib(%d) on the cluster..."):format(n))
local t0 = os.clock()
local out = picat(PROGRAM)
print(("fib(%d) = %s  (%.1fs round trip)"):format(n, tostring(out):gsub("%s+$", ""), os.clock() - t0))
