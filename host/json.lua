-- Minimal JSON for Lua 5.1 (decode + encode). Used for resolved-WIT schemas
-- and command-kind bodies. On CC, textutils exists, but this keeps every host
-- (Cobalt jar, CraftOS-PC, in-game, lua5.4) on identical behavior.
local M = {}

-- ---- decode -----------------------------------------------------------------
local function skip_ws(s, i)
  local _, j = s:find("^[ \t\r\n]*", i)
  return j + 1
end

local decode_value

local function decode_string(s, i) -- i at opening quote
  local buf, j = {}, i + 1
  while true do
    local c = s:sub(j, j)
    if c == "" then error("json: unterminated string") end
    if c == '"' then return table.concat(buf), j + 1 end
    if c == "\\" then
      local e = s:sub(j + 1, j + 1)
      if e == "u" then
        local hex = s:sub(j + 2, j + 5)
        local cp = tonumber(hex, 16) or error("json: bad \\u escape")
        j = j + 6
        -- fold UTF-16 surrogate pairs
        if cp >= 0xD800 and cp <= 0xDBFF and s:sub(j, j + 1) == "\\u" then
          local lo = tonumber(s:sub(j + 2, j + 5), 16)
          if lo and lo >= 0xDC00 and lo <= 0xDFFF then
            cp = 0x10000 + (cp - 0xD800) * 0x400 + (lo - 0xDC00)
            j = j + 6
          end
        end
        if cp < 0x80 then buf[#buf + 1] = string.char(cp)
        elseif cp < 0x800 then
          buf[#buf + 1] = string.char(192 + math.floor(cp / 64), 128 + cp % 64)
        elseif cp < 0x10000 then
          buf[#buf + 1] = string.char(224 + math.floor(cp / 4096),
            128 + math.floor(cp / 64) % 64, 128 + cp % 64)
        else
          buf[#buf + 1] = string.char(240 + math.floor(cp / 262144),
            128 + math.floor(cp / 4096) % 64, 128 + math.floor(cp / 64) % 64, 128 + cp % 64)
        end
      else
        local map = { n = "\n", t = "\t", r = "\r", b = "\b", f = "\f",
          ['"'] = '"', ["\\"] = "\\", ["/"] = "/" }
        buf[#buf + 1] = map[e] or error("json: bad escape \\" .. tostring(e))
        j = j + 2
      end
    else
      buf[#buf + 1] = c
      j = j + 1
    end
  end
end

decode_value = function(s, i)
  i = skip_ws(s, i)
  local c = s:sub(i, i)
  if c == "{" then
    local obj = {}
    i = skip_ws(s, i + 1)
    if s:sub(i, i) == "}" then return obj, i + 1 end
    while true do
      local k; k, i = decode_string(s, skip_ws(s, i))
      i = skip_ws(s, i)
      if s:sub(i, i) ~= ":" then error("json: expected ':'") end
      local v; v, i = decode_value(s, i + 1)
      obj[k] = v
      i = skip_ws(s, i)
      local d = s:sub(i, i)
      if d == "," then i = i + 1
      elseif d == "}" then return obj, i + 1
      else error("json: expected ',' or '}'") end
    end
  elseif c == "[" then
    local arr = {}
    i = skip_ws(s, i + 1)
    if s:sub(i, i) == "]" then return arr, i + 1 end
    while true do
      local v; v, i = decode_value(s, i)
      arr[#arr + 1] = v
      i = skip_ws(s, i)
      local d = s:sub(i, i)
      if d == "," then i = i + 1
      elseif d == "]" then return arr, i + 1
      else error("json: expected ',' or ']'") end
    end
  elseif c == '"' then
    return decode_string(s, i)
  elseif s:sub(i, i + 3) == "true" then return true, i + 4
  elseif s:sub(i, i + 4) == "false" then return false, i + 5
  elseif s:sub(i, i + 3) == "null" then return nil, i + 4
  else
    local num = s:match("^-?%d+%.?%d*[eE]?[+%-]?%d*", i)
    if not num or num == "" then error("json: unexpected '" .. c .. "' at " .. i) end
    return tonumber(num), i + #num
  end
end

function M.decode(s)
  local v, i = decode_value(s, 1)
  i = skip_ws(s, i)
  if i <= #s then error("json: trailing garbage at " .. i) end
  return v
end

-- ---- encode -----------------------------------------------------------------
local function is_array(t)
  local n = 0
  for k in pairs(t) do
    if type(k) ~= "number" then return false end
    n = n + 1
  end
  return n == #t
end

local function esc(s)
  return (s:gsub('[%c"\\]', function(c)
    local map = { ['"'] = '\\"', ["\\"] = "\\\\", ["\n"] = "\\n", ["\t"] = "\\t", ["\r"] = "\\r" }
    return map[c] or string.format("\\u%04x", c:byte())
  end))
end

function M.encode(v)
  local t = type(v)
  if v == nil then return "null"
  elseif t == "boolean" then return tostring(v)
  elseif t == "number" then
    if v ~= v or v == math.huge or v == -math.huge then error("json: non-finite number") end
    if v == math.floor(v) and math.abs(v) < 2^53 then return string.format("%.0f", v) end
    return string.format("%.17g", v)
  elseif t == "string" then return '"' .. esc(v) .. '"'
  elseif t == "table" then
    if is_array(v) then
      local parts = {}
      for i = 1, #v do parts[i] = M.encode(v[i]) end
      return "[" .. table.concat(parts, ",") .. "]"
    end
    local parts = {}
    for k, val in pairs(v) do
      parts[#parts + 1] = '"' .. esc(tostring(k)) .. '":' .. M.encode(val)
    end
    return "{" .. table.concat(parts, ",") .. "}"
  end
  error("json: cannot encode " .. t)
end

return M
