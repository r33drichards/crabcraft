-- Self-test for cron.lua (runs on lua5.4 and Cobalt): parser + matcher only,
-- no clock - match() takes an os.date("*t")-shaped table, so every case here
-- is synthetic and deterministic. The gateway's job state machine around it
-- is covered by test/gateway_jobs_test.lua.
package.path = "host/?.lua;" .. package.path
local cron = require("cron")

local passed, failed = 0, 0
local function check(desc, ok)
  if ok then passed = passed + 1
  else failed = failed + 1; print("FAIL " .. desc) end
end

-- tm(min, hour, day, month, wday): wday is lua-style, 1 = sunday
local function tm(min, hour, day, month, wday)
  return { min = min, hour = hour, day = day, month = month, wday = wday or 1 }
end

local function matches(expr, t)
  local c, err = cron.parse(expr)
  assert(c, tostring(err))
  return cron.match(c, t)
end

-- ---- @every ---------------------------------------------------------------
local c = assert(cron.parse("@every 30s"))
check("@every 30s", c.every == 30)
check("@every 5m", assert(cron.parse("@every 5m")).every == 300)
check("@every 1h30m", assert(cron.parse("@every 1h30m")).every == 5400)
check("@every 2d", assert(cron.parse("@every 2d")).every == 172800)
check("@every never time-matches", cron.match(c, tm(0, 0, 1, 1)) == false)

-- ---- macros ---------------------------------------------------------------
check("@hourly at :00", matches("@hourly", tm(0, 7, 15, 6)))
check("@hourly not at :01", not matches("@hourly", tm(1, 7, 15, 6)))
check("@daily at midnight", matches("@daily", tm(0, 0, 15, 6)))
check("@midnight = @daily", matches("@midnight", tm(0, 0, 15, 6)))
check("@weekly on sunday", matches("@weekly", tm(0, 0, 15, 6, 1)))
check("@weekly not monday", not matches("@weekly", tm(0, 0, 15, 6, 2)))
check("@monthly on the 1st", matches("@monthly", tm(0, 0, 1, 6)))
check("@yearly jan 1", matches("@yearly", tm(0, 0, 1, 1)))
check("@yearly not feb 1", not matches("@yearly", tm(0, 0, 1, 2)))

-- ---- five-field basics ----------------------------------------------------
check("* * * * * matches anything", matches("* * * * *", tm(59, 23, 31, 12, 7)))
check("exact minute", matches("30 * * * *", tm(30, 11, 5, 3)))
check("exact minute misses", not matches("30 * * * *", tm(31, 11, 5, 3)))
check("min+hour", matches("15 9 * * *", tm(15, 9, 1, 1)))
check("min+hour wrong hour", not matches("15 9 * * *", tm(15, 10, 1, 1)))

-- ---- steps, ranges, lists ---------------------------------------------------
check("*/5 at 0", matches("*/5 * * * *", tm(0, 0, 1, 1)))
check("*/5 at 55", matches("*/5 * * * *", tm(55, 0, 1, 1)))
check("*/5 not 3", not matches("*/5 * * * *", tm(3, 0, 1, 1)))
check("range 10-20", matches("10-20 * * * *", tm(15, 0, 1, 1)))
check("range edge", matches("10-20 * * * *", tm(20, 0, 1, 1)))
check("range miss", not matches("10-20 * * * *", tm(21, 0, 1, 1)))
check("range+step 0-30/10 hits 20", matches("0-30/10 * * * *", tm(20, 0, 1, 1)))
check("range+step 0-30/10 misses 25", not matches("0-30/10 * * * *", tm(25, 0, 1, 1)))
check("vixie n/step runs to top", matches("50/3 * * * *", tm(56, 0, 1, 1)))
check("list 1,15,45", matches("1,15,45 * * * *", tm(45, 0, 1, 1)))
check("list miss", not matches("1,15,45 * * * *", tm(2, 0, 1, 1)))
check("list of ranges", matches("0-5,55-59 * * * *", tm(57, 0, 1, 1)))

-- ---- names + dow wrap -------------------------------------------------------
check("month name jan", matches("0 0 1 jan *", tm(0, 0, 1, 1)))
check("month name misses feb", not matches("0 0 1 jan *", tm(0, 0, 1, 2)))
check("dow name range mon-fri hits wed", matches("0 9 * * mon-fri", tm(0, 9, 3, 6, 4)))
check("dow name range mon-fri misses sun", not matches("0 9 * * mon-fri", tm(0, 9, 3, 6, 1)))
check("dow 7 is sunday", matches("0 0 * * 7", tm(0, 0, 3, 6, 1)))
check("dow 0 is sunday", matches("0 0 * * 0", tm(0, 0, 3, 6, 1)))
check("dow range 5-7 wraps to sunday", matches("0 0 * * 5-7", tm(0, 0, 3, 6, 1)))
check("dow range 5-7 hits friday", matches("0 0 * * 5-7", tm(0, 0, 3, 6, 6)))
check("dow range 5-7 misses tuesday", not matches("0 0 * * 5-7", tm(0, 0, 3, 6, 3)))

-- ---- vixie dom/dow OR rule --------------------------------------------------
-- both restricted: EITHER matches ("the 13th, or any friday")
check("dom|dow: 13th not friday", matches("0 0 13 * fri", tm(0, 0, 13, 6, 4)))
check("dom|dow: friday not 13th", matches("0 0 13 * fri", tm(0, 0, 20, 6, 6)))
check("dom|dow: neither", not matches("0 0 13 * fri", tm(0, 0, 20, 6, 4)))
-- only one restricted: plain AND
check("dom only: day must match", not matches("0 0 13 * *", tm(0, 0, 20, 6, 6)))
check("dow only: weekday must match", not matches("0 0 * * fri", tm(0, 0, 13, 6, 4)))

-- ---- rejects ----------------------------------------------------------------
local function rejects(desc, expr)
  local c2, err = cron.parse(expr)
  check("rejects " .. desc, c2 == nil and err ~= nil)
end
rejects("4 fields", "* * * *")
rejects("6 fields", "* * * * * *")
rejects("minute 60", "60 * * * *")
rejects("hour 24", "0 24 * * *")
rejects("dom 0", "0 0 0 * *")
rejects("dom 32", "0 0 32 * *")
rejects("month 13", "0 0 1 13 *")
rejects("dow 8", "0 0 * * 8")
rejects("step 0", "*/0 * * * *")
rejects("reversed range", "20-10 * * * *")
rejects("garbage token", "x * * * *")
rejects("unknown macro", "@fortnightly")
rejects("unknown name", "0 0 1 janu *")
rejects("bad duration", "@every fast")
rejects("zero duration", "@every 0s")
rejects("trailing junk in duration", "@every 5q")
rejects("non-string", 5)

print(("cron_selftest: %d passed, %d failed"):format(passed, failed))
if failed > 0 then error("cron selftest FAILED") end
