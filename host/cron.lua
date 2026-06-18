-- Cron schedules for crabcraft jobs (docs/WIRE.md section 6).
-- The gateway evaluates schedules against real-world UTC; crb uses the same
-- parser to reject bad schedules at deploy time. Grammar:
--   "@every <dur>"        intervals: 30s, 5m, 1h30m, 2d (gateway tick = ~2s)
--   "@hourly @daily @midnight @weekly @monthly @yearly @annually"
--   "min hour dom mon dow"  vixie-cron subset: * lists a,b,c ranges a-b
--                           steps */n a-b/n a/n  names jan-dec / sun-sat
--                           dow 0 and 7 = sunday
-- Vixie rule kept: when BOTH dom and dow are restricted, a time matches if
-- EITHER matches. No seconds field, no L/W/# extensions, no catch-up.
local M = {}

local MACROS = {
  ["@hourly"] = "0 * * * *", ["@daily"] = "0 0 * * *",
  ["@midnight"] = "0 0 * * *", ["@weekly"] = "0 0 * * 0",
  ["@monthly"] = "0 0 1 * *", ["@yearly"] = "0 0 1 1 *",
  ["@annually"] = "0 0 1 1 *",
}
local MON = { jan = 1, feb = 2, mar = 3, apr = 4, may = 5, jun = 6,
  jul = 7, aug = 8, sep = 9, oct = 10, nov = 11, dec = 12 }
local DOW = { sun = 0, mon = 1, tue = 2, wed = 3, thu = 4, fri = 5, sat = 6 }
local UNITS = { s = 1, m = 60, h = 3600, d = 86400 }

local function parse_every(s)
  local total, rest = 0, s
  while #rest > 0 do
    local n, u, tail = rest:match("^(%d+)([smhd])(.*)$")
    if not n then return nil, "bad duration '" .. s .. "' (use e.g. 30s, 5m, 1h30m)" end
    total = total + tonumber(n) * UNITS[u]
    rest = tail
  end
  if total < 1 then return nil, "@every duration must be at least 1s" end
  return total
end

local function field_value(tok, names, lo, hi)
  local v = tonumber(tok) or (names and names[tok:lower()])
  if type(v) ~= "number" or v % 1 ~= 0 or v < lo or v > hi then return nil end
  return v
end

-- one field -> set {n=true}, or nil for "*" (matches anything).
-- wrap: dow accepts 7 and stores it as 0 (both mean sunday).
local function parse_field(field, lo, hi, names, wrap)
  if field == "*" then return nil end
  local top = wrap and hi + 1 or hi
  local set = {}
  for part in (field .. ","):gmatch("([^,]*),") do
    local base, step = part:match("^(.-)/(%d+)$")
    base = base or part
    step = step and tonumber(step)
    if step and step < 1 then return nil, "step must be >= 1 in '" .. part .. "'" end
    local a, b
    if base == "*" then
      a, b = lo, hi
    else
      local x, y = base:match("^(.+)%-(.+)$")
      if x then
        a, b = field_value(x, names, lo, top), field_value(y, names, lo, top)
      else
        a = field_value(base, names, lo, top)
        b = step and top or a -- vixie: "n/step" runs n..top
      end
      if not a or not b or a > b then return nil, "bad value '" .. part .. "'" end
    end
    for v = a, b, step or 1 do
      set[wrap and v % (hi + 1) or v] = true
    end
  end
  return set
end

-- parse(expr) -> { every = seconds } | { min, hour, dom, mon, dow } | nil, err
-- (field sets are nil for "*"; match() treats nil as match-anything)
function M.parse(expr)
  if type(expr) ~= "string" then return nil, "schedule must be a string" end
  local s = expr:match("^%s*(.-)%s*$")
  local dur = s:match("^@every%s+(%S+)$")
  if dur then
    local secs, err = parse_every(dur)
    if not secs then return nil, err end
    return { every = secs }
  end
  s = MACROS[s:lower()] or s
  if s:sub(1, 1) == "@" then return nil, "unknown macro '" .. expr .. "'" end
  local f = {}
  for tok in s:gmatch("%S+") do f[#f + 1] = tok end
  if #f ~= 5 then
    return nil, "cron schedule needs 5 fields (min hour dom mon dow), " ..
      "@every <dur>, or a @macro - got '" .. expr .. "'"
  end
  local c, err = {}
  c.min, err = parse_field(f[1], 0, 59)
  if err then return nil, "minute: " .. err end
  c.hour, err = parse_field(f[2], 0, 23)
  if err then return nil, "hour: " .. err end
  c.dom, err = parse_field(f[3], 1, 31)
  if err then return nil, "day-of-month: " .. err end
  c.mon, err = parse_field(f[4], 1, 12, MON)
  if err then return nil, "month: " .. err end
  c.dow, err = parse_field(f[5], 0, 6, DOW, true)
  if err then return nil, "day-of-week: " .. err end
  return c
end

local function hit(set, v) return set == nil or set[v] == true end

-- match(parsed, tm): tm is an os.date("*t")-shaped table; pass os.date("!*t")
-- for the UTC semantics the gateway uses. Interval schedules never time-match.
function M.match(c, tm)
  if c.every then return false end
  if not (hit(c.min, tm.min) and hit(c.hour, tm.hour) and hit(c.mon, tm.month)) then
    return false
  end
  local dow = (tm.wday - 1) % 7 -- lua wday 1=sunday -> cron 0=sunday
  if c.dom and c.dow then return c.dom[tm.day] == true or c.dow[dow] == true end
  return hit(c.dom, tm.day) and hit(c.dow, dow)
end

return M
