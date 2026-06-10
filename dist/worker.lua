-- crabcraft worker (amalgamated; see host/)
-- self-bootstrap: the wasm engine (wasmcraft bundle) is fetched on first run
do
  local function exists(p)
    if type(fs) == "table" then return fs.exists(p) end
    local f = io.open(p, "r"); if f then f:close(); return true end
    return false
  end
  if type(fs) == "table" and fs.open and not exists("wasmcraft") then
    io.write("fetching wasmcraft engine ... ")
    local r = assert(http.get("https://github.com/r33drichards/wasmcraft/releases/latest/download/wasmcraft.lua", nil, true), "engine fetch failed")
    local h = fs.open("wasmcraft", "wb"); h.write(r.readAll()); h.close(); r.close()
    print("ok")
  end
end

local preload, loaded = {}, {}
local function require(n)
  if loaded[n] ~= nil then return loaded[n] end
  local f = preload[n] or error('module not bundled: '..n)
  local m = f(); if m == nil then m = true end
  loaded[n] = m; return m
end
preload["json"] = function(...)
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

end
preload["cmval"] = function(...)
-- Component-model value codec (docs/WIRE.md section 1), pure Lua 5.1.
-- Types are DESCRIPTORS: either a string primitive name ("u32", "string", ...)
-- or a table { kind=..., ... }:
--   { kind="list",    elem=T }
--   { kind="record",  fields={ {name=, type=T}, ... } }      (declaration order)
--   { kind="tuple",   members={T, ...} }
--   { kind="variant", cases={ {name=, type=T|nil}, ... } }
--   { kind="enum",    cases={ "a","b",... } }
--   { kind="option",  elem=T }
--   { kind="result",  ok=T|nil, err=T|nil }
--   { kind="flags",   names={ "a","b",... } }
-- Lua value mapping: bool<->boolean, numbers<->number, char = 1-char-ish string
-- by codepoint number (we use NUMBER codepoints), string<->string,
-- list<->array table, record<->{ [fieldname]=v }, tuple<->array,
-- variant<->{ case="name", value=v }, enum<->"name", option<->nil | value
-- (records wrap option fields by presence), result<->{ ok=v } | { err=v },
-- flags<->{ [name]=true }.
local M = {}

local floor = math.floor
local schar, sbyte = string.char, string.byte

-- math.frexp was removed in Lua 5.4; polyfill for the no-string.pack path
local frexp = math.frexp or function(x)
  if x == 0 or x ~= x or x == math.huge or x == -math.huge then return x, 0 end
  local e = floor(math.log(math.abs(x)) / math.log(2)) + 1
  local m = x / 2 ^ e
  while math.abs(m) >= 1 do m = m / 2; e = e + 1 end
  while math.abs(m) < 0.5 do m = m * 2; e = e - 1 end
  return m, e
end

-- decimal-string arithmetic for u64/s64 beyond 2^53 (values may be strings)
local function dstr_divmod128(d) -- decimal string -> quotient string, remainder
  local q, r = {}, 0
  for i = 1, #d do
    local cur = r * 10 + (sbyte(d, i) - 48)
    local digit = floor(cur / 128)
    if #q > 0 or digit > 0 then q[#q + 1] = digit end
    r = cur % 128
  end
  return table.concat(q == nil and {} or (function()
    local t = {}
    for i = 1, #q do t[i] = string.char(q[i] + 48) end
    return t
  end)()), r
end

local function dstr_is_zero(d) return d == "" or d:match("^0*$") ~= nil end

local function dstr_mul128_add(d, add) -- decimal string * 128 + add -> decimal string
  local out, carry = {}, add
  for i = #d, 1, -1 do
    local v = (sbyte(d, i) - 48) * 128 + carry
    out[#out + 1] = v % 10
    carry = floor(v / 10)
  end
  while carry > 0 do
    out[#out + 1] = carry % 10
    carry = floor(carry / 10)
  end
  local t = {}
  for i = #out, 1, -1 do t[#t + 1] = string.char(out[i] + 48) end
  local r = table.concat(t)
  return r == "" and "0" or r
end

-- utf8: first codepoint of a string (for char values given as strings)
local function utf8_cp(str)
  local b1 = sbyte(str, 1) or error("char: empty string")
  if b1 < 0x80 then return b1 end
  if b1 < 0xE0 then return (b1 - 192) * 64 + (sbyte(str, 2) - 128) end
  if b1 < 0xF0 then
    return (b1 - 224) * 4096 + (sbyte(str, 2) - 128) * 64 + (sbyte(str, 3) - 128)
  end
  return (b1 - 240) * 262144 + (sbyte(str, 2) - 128) * 4096 +
    (sbyte(str, 3) - 128) * 64 + (sbyte(str, 4) - 128)
end

-- ---- LEB128 -----------------------------------------------------------------
local function uleb_enc_str(d, out) -- unsigned decimal string
  repeat
    local q, r = dstr_divmod128(d)
    d = q
    local done = dstr_is_zero(d)
    out[#out + 1] = schar(done and r or r + 128)
  until done
end

local function uleb_enc(n, out)
  if type(n) == "string" then return uleb_enc_str(n:gsub("^0+(%d)", "%1"), out) end
  assert(n >= 0, "uleb: negative")
  repeat
    local b = n % 128
    n = floor(n / 128)
    if n > 0 then b = b + 128 end
    out[#out + 1] = schar(b)
  until n == 0
end

local function uleb_dec(s, pos)
  -- shift starts as a FLOAT: on Lua 5.4 integer arithmetic would silently wrap
  -- past 2^63; float accumulation degrades gracefully and the byte-count check
  -- below switches to exact decimal-string reconstruction
  local n, shift = 0, 1.0
  local bytes = {}
  for _ = 1, 10 do
    local b = sbyte(s, pos)
    if not b then error("uleb: truncated") end
    pos = pos + 1
    bytes[#bytes + 1] = b % 128
    n = n + (b % 128) * shift
    if b < 128 then
      if n >= 2 ^ 53 then
        -- exceeds exact float range: rebuild as a decimal string
        local d = "0"
        for i = #bytes, 1, -1 do d = dstr_mul128_add(d, bytes[i]) end
        return d, pos
      end
      return n, pos
    end
    shift = shift * 128
  end
  error("uleb: too long")
end

local function sleb_enc(n, out)
  repeat
    local b = n % 128
    n = floor(n / 128)
    -- arithmetic shift semantics: for negatives Lua floor-div already gives
    -- -1 as the fixpoint, matching sign extension
    local done = (n == 0 and b < 64) or (n == -1 and b >= 64)
    if not done then b = b + 128 end
    out[#out + 1] = schar(b)
  until done
end

local function sleb_dec(s, pos)
  local n, shift, b = 0, 1.0, nil
  for _ = 1, 10 do
    b = sbyte(s, pos)
    if not b then error("sleb: truncated") end
    pos = pos + 1
    n = n + (b % 128) * shift
    shift = shift * 128
    if b < 128 then
      if b >= 64 then n = n - shift end -- sign extend
      return n, pos
    end
  end
  error("sleb: too long")
end

-- ---- IEEE-754 little-endian (no string.pack on plain 5.1; handcoded) --------
local function f64_enc(x, out)
  if string.pack then out[#out + 1] = string.pack("<d", x) return end
  -- portable encode
  local sign = 0
  if x < 0 or (x == 0 and 1 / x < 0) then sign = 1; x = -x end
  local mant, expo
  if x ~= x then out[#out + 1] = schar(0, 0, 0, 0, 0, 0, 248, 127) return
  elseif x == math.huge then mant, expo = 0, 2047
  elseif x == 0 then mant, expo = 0, 0
  else
    local m, e = frexp(x)
    expo = e + 1022
    if expo <= 0 then -- subnormal
      mant = m * 2 ^ (52 + expo)
      expo = 0
    else
      mant = (m * 2 - 1) * 2 ^ 52
    end
  end
  local bytes = {}
  for i = 1, 6 do
    bytes[i] = mant % 256
    mant = floor(mant / 256)
  end
  bytes[7] = mant + (expo % 16) * 16
  bytes[8] = floor(expo / 16) + sign * 128
  out[#out + 1] = schar(bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8])
end

local function f64_dec(s, pos)
  if string.unpack then return string.unpack("<d", s, pos) end
  local b = { sbyte(s, pos, pos + 7) }
  if #b < 8 then error("f64: truncated") end
  local sign = b[8] >= 128 and -1 or 1
  local expo = (b[8] % 128) * 16 + floor(b[7] / 16)
  local mant = b[7] % 16
  for i = 6, 1, -1 do mant = mant * 256 + b[i] end
  local x
  if expo == 2047 then x = mant == 0 and math.huge or (0 / 0)
  elseif expo == 0 then x = mant * 2 ^ -1074
  else x = (mant + 2 ^ 52) * 2 ^ (expo - 1075) end
  return sign * x, pos + 8
end

local function f32_enc(x, out)
  if string.pack then out[#out + 1] = string.pack("<f", x) return end
  -- encode via f64 then narrow: do a manual single-precision encode
  local sign = 0
  if x < 0 or (x == 0 and 1 / x < 0) then sign = 1; x = -x end
  local mant, expo
  if x ~= x then out[#out + 1] = schar(0, 0, 192, 127) return
  elseif x == math.huge then mant, expo = 0, 255
  elseif x == 0 then mant, expo = 0, 0
  else
    local m, e = frexp(x)
    expo = e + 126
    if expo <= 0 then mant = m * 2 ^ (23 + expo); expo = 0
    else mant = (m * 2 - 1) * 2 ^ 23 end
    mant = floor(mant + 0.5) -- round to nearest
    if mant >= 2 ^ 23 and expo > 0 then mant = 0; expo = expo + 1 end
    if expo >= 255 then mant, expo = 0, 255 end
  end
  local b1 = mant % 256
  local b2 = floor(mant / 256) % 256
  local b3 = floor(mant / 65536) + (expo % 2) * 128
  local b4 = floor(expo / 2) + sign * 64
  out[#out + 1] = schar(b1, b2, b3, b4 + (sign == 1 and 64 or 0))
end

local function f32_dec(s, pos)
  if string.unpack then return string.unpack("<f", s, pos) end
  local b1, b2, b3, b4 = sbyte(s, pos, pos + 3)
  if not b4 then error("f32: truncated") end
  local sign = b4 >= 128 and -1 or 1
  local expo = (b4 % 128) * 2 + floor(b3 / 128)
  local mant = (b3 % 128) * 65536 + b2 * 256 + b1
  local x
  if expo == 255 then x = mant == 0 and math.huge or (0 / 0)
  elseif expo == 0 then x = mant * 2 ^ -149
  else x = (mant + 2 ^ 23) * 2 ^ (expo - 150) end
  return sign * x, pos + 4
end

-- ---- main codec ---------------------------------------------------------------
local UINTS = { u8 = true, u16 = true, u32 = true, u64 = true }
local SINTS = { s8 = true, s16 = true, s32 = true, s64 = true }

local function enc(ty, v, out)
  if type(ty) == "string" then
    if ty == "bool" then out[#out + 1] = schar(v and 1 or 0)
    elseif UINTS[ty] then uleb_enc(v, out)
    elseif SINTS[ty] then sleb_enc(v, out)
    elseif ty == "f32" then f32_enc(v, out)
    elseif ty == "f64" then f64_enc(v, out)
    elseif ty == "char" then
      uleb_enc(type(v) == "string" and utf8_cp(v) or v, out)
    elseif ty == "string" then
      uleb_enc(#v, out)
      out[#out + 1] = v
    else error("cmval: unknown primitive " .. ty) end
    return
  end
  local k = ty.kind
  if k == "list" then
    uleb_enc(#v, out)
    for i = 1, #v do enc(ty.elem, v[i], out) end
  elseif k == "record" then
    for _, f in ipairs(ty.fields) do
      local fv = v[f.name]
      local fty = f.type
      if type(fty) == "table" and fty.kind == "option" then
        -- option fields accept plain absence
        if fv == nil then out[#out + 1] = schar(0)
        else out[#out + 1] = schar(1); enc(fty.elem, fv, out) end
      else
        assert(fv ~= nil, "cmval: missing record field " .. f.name)
        enc(fty, fv, out)
      end
    end
  elseif k == "tuple" then
    for i, mty in ipairs(ty.members) do enc(mty, v[i], out) end
  elseif k == "variant" then
    for i, c in ipairs(ty.cases) do
      if c.name == v.case then
        uleb_enc(i - 1, out)
        if c.type then enc(c.type, v.value, out) end
        return
      end
    end
    error("cmval: unknown variant case " .. tostring(v.case))
  elseif k == "enum" then
    for i, name in ipairs(ty.cases) do
      if name == v then uleb_enc(i - 1, out) return end
    end
    error("cmval: unknown enum case " .. tostring(v))
  elseif k == "option" then
    if v == nil then out[#out + 1] = schar(0)
    else out[#out + 1] = schar(1); enc(ty.elem, v, out) end
  elseif k == "result" then
    if v.ok ~= nil or (v.err == nil and v.ok == nil and v.is_ok) then
      out[#out + 1] = schar(0)
      if ty.ok then enc(ty.ok, v.ok, out) end
    elseif v.err ~= nil or v.is_err then
      out[#out + 1] = schar(1)
      if ty.err then enc(ty.err, v.err, out) end
    else error("cmval: result needs ok or err") end
  elseif k == "flags" then
    local nbytes = math.ceil(#ty.names / 8)
    local bytes = {}
    for i = 1, nbytes do bytes[i] = 0 end
    for i, name in ipairs(ty.names) do
      if v[name] then
        local byi = floor((i - 1) / 8) + 1
        bytes[byi] = bytes[byi] + 2 ^ ((i - 1) % 8)
      end
    end
    for i = 1, nbytes do out[#out + 1] = schar(bytes[i]) end
  else error("cmval: unknown kind " .. tostring(k)) end
end

local function dec(ty, s, pos)
  if type(ty) == "string" then
    if ty == "bool" then
      local b = sbyte(s, pos) or error("bool: truncated")
      return b ~= 0, pos + 1
    elseif UINTS[ty] or ty == "char" then return uleb_dec(s, pos)
    elseif SINTS[ty] then return sleb_dec(s, pos)
    elseif ty == "f32" then return f32_dec(s, pos)
    elseif ty == "f64" then return f64_dec(s, pos)
    elseif ty == "string" then
      local len; len, pos = uleb_dec(s, pos)
      if pos + len - 1 > #s then error("string: truncated") end
      return s:sub(pos, pos + len - 1), pos + len
    else error("cmval: unknown primitive " .. ty) end
  end
  local k = ty.kind
  if k == "list" then
    local n; n, pos = uleb_dec(s, pos)
    local arr = {}
    for i = 1, n do arr[i], pos = dec(ty.elem, s, pos) end
    return arr, pos
  elseif k == "record" then
    local rec = {}
    for _, f in ipairs(ty.fields) do rec[f.name], pos = dec(f.type, s, pos) end
    return rec, pos
  elseif k == "tuple" then
    local t = {}
    for i, mty in ipairs(ty.members) do t[i], pos = dec(mty, s, pos) end
    return t, pos
  elseif k == "variant" then
    local idx; idx, pos = uleb_dec(s, pos)
    local c = ty.cases[idx + 1] or error("variant: bad discriminant " .. idx)
    local v = { case = c.name }
    if c.type then v.value, pos = dec(c.type, s, pos) end
    return v, pos
  elseif k == "enum" then
    local idx; idx, pos = uleb_dec(s, pos)
    return ty.cases[idx + 1] or error("enum: bad discriminant " .. idx), pos
  elseif k == "option" then
    local b = sbyte(s, pos) or error("option: truncated")
    pos = pos + 1
    if b == 0 then return nil, pos end
    return dec(ty.elem, s, pos)
  elseif k == "result" then
    local b = sbyte(s, pos) or error("result: truncated")
    pos = pos + 1
    if b == 0 then
      local v = { is_ok = true }
      if ty.ok then v.ok, pos = dec(ty.ok, s, pos) end
      return v, pos
    end
    local v = { is_err = true }
    if ty.err then v.err, pos = dec(ty.err, s, pos) end
    return v, pos
  elseif k == "flags" then
    local nbytes = math.ceil(#ty.names / 8)
    local set = {}
    for i, name in ipairs(ty.names) do
      local byi = floor((i - 1) / 8)
      local b = sbyte(s, pos + byi) or error("flags: truncated")
      if floor(b / 2 ^ ((i - 1) % 8)) % 2 == 1 then set[name] = true end
    end
    return set, pos + nbytes
  else error("cmval: unknown kind " .. tostring(k)) end
end

function M.encode(ty, v)
  local out = {}
  enc(ty, v, out)
  return table.concat(out)
end

function M.decode(ty, s, pos)
  return dec(ty, s, pos or 1)
end

-- encode/decode a function's param list: types = array of descriptors
function M.encode_params(types, values)
  local out = {}
  for i, ty in ipairs(types) do enc(ty, values[i], out) end
  return table.concat(out)
end

function M.decode_params(types, s)
  local vals, pos = {}, 1
  for i, ty in ipairs(types) do vals[i], pos = dec(ty, s, pos) end
  return vals, pos
end

return M

end
preload["schema"] = function(...)
-- Resolved-WIT JSON (wasm-tools component wit --json) -> cmval descriptors.
-- This is what makes clients DATA-DRIVEN: given a schema, we can encode params
-- and decode results for any function with zero codegen.
--   local sc = schema.load(json_string_or_table)
--   local fn = sc.functions["crab:hello/greeter@0.1.0#greet"]
--   fn.params  -> { {name=, type=<cmval descriptor>}, ... }
--   fn.result  -> <cmval descriptor> | nil
--   sc.list()  -> sorted function addresses
local json = require("json")
local M = {}

local function conv_type(resolved, t)
  if type(t) == "string" then return t end -- primitive name
  -- numeric index into the top-level types array (0-based)
  if type(t) == "number" then
    local entry = resolved.types[t + 1]
      or error("schema: dangling type index " .. tostring(t))
    return conv_type(resolved, entry.kind)
  end
  -- kind table forms
  if t.record then
    local fields = {}
    for i, f in ipairs(t.record.fields) do
      fields[i] = { name = f.name, type = conv_type(resolved, f.type) }
    end
    return { kind = "record", fields = fields }
  elseif t.option then
    return { kind = "option", elem = conv_type(resolved, t.option) }
  elseif t.list then
    return { kind = "list", elem = conv_type(resolved, t.list) }
  elseif t.tuple then
    local m = {}
    for i, mt in ipairs(t.tuple.types) do m[i] = conv_type(resolved, mt) end
    return { kind = "tuple", members = m }
  elseif t.variant then
    local cases = {}
    for i, c in ipairs(t.variant.cases) do
      cases[i] = { name = c.name, type = c.type and conv_type(resolved, c.type) or nil }
    end
    return { kind = "variant", cases = cases }
  elseif t.enum then
    local cases = {}
    for i, c in ipairs(t.enum.cases) do
      cases[i] = type(c) == "table" and c.name or c
    end
    return { kind = "enum", cases = cases }
  elseif t.result then
    return { kind = "result",
      ok = t.result.ok and conv_type(resolved, t.result.ok) or nil,
      err = t.result.err and conv_type(resolved, t.result.err) or nil }
  elseif t.flags then
    local names = {}
    for i, fl in ipairs(t.flags.flags) do
      names[i] = type(fl) == "table" and fl.name or fl
    end
    return { kind = "flags", names = names }
  elseif t.type then -- type alias
    return conv_type(resolved, t.type)
  end
  error("schema: unsupported type kind: " .. json.encode(t))
end

function M.load(src)
  local resolved = type(src) == "string" and json.decode(src) or src
  local sc = { functions = {}, raw = resolved }
  for ifidx, iface in ipairs(resolved.interfaces or {}) do
    -- instance address: package name + / + interface name (pkg carries @version)
    local pkg = resolved.packages[(iface.package or 0) + 1]
    local pkgname = pkg and pkg.name or "unknown:pkg"
    -- wasm-tools may render the version inside the package name already
    local base, ver = pkgname:match("^(.-)@(.+)$")
    local instance
    if base then instance = base .. "/" .. iface.name .. "@" .. ver
    else instance = pkgname .. "/" .. iface.name end
    for fname, f in pairs(iface.functions or {}) do
      local params = {}
      for i, p in ipairs(f.params or {}) do
        params[i] = { name = p.name, type = conv_type(resolved, p.type) }
      end
      local result = nil
      if f.result ~= nil then result = conv_type(resolved, f.result) end
      local addr = instance .. "#" .. fname
      sc.functions[addr] = {
        addr = addr, instance = instance, name = fname,
        params = params, result = result,
      }
    end
  end
  function sc.list()
    local out = {}
    for addr in pairs(sc.functions) do out[#out + 1] = addr end
    table.sort(out)
    return out
  end
  -- param types as a plain array (for cmval.encode_params)
  function sc.param_types(addr)
    local f = sc.functions[addr] or error("schema: no function " .. addr)
    local t = {}
    for i, p in ipairs(f.params) do t[i] = p.type end
    return t
  end
  return sc
end

return M

end
preload["runtime"] = function(...)
-- crabcraft guest runtime: load and invoke wasm workloads on wasmcraft.
-- Implements WIRE.md section 2 (reactor crab ABI) and the command kind.
--   local rt = require("runtime")
--   rt.engine_path = "wasmcraft"            -- the wasmcraft bundle file
--   local w = rt.load_reactor(bytes, { mode = "transpile", root = ".",
--                                      call = function(name, func, params) ... end })
--   w.schema_json                            -- the module's resolved-WIT JSON
--   local reply = w:invoke("crab:hello/greeter@0.1.0#greet", <param bytes>)
--   -- reply = { ok = true, result = <bytes> } | { ok = false, err = "msg" }
-- Command kind:
--   local out = rt.run_command(bytes, body_json, { mode = ..., root = ... })
local M = { engine_path = nil }

local wasmcraft -- lazily loaded bundle

local function find(c)
  for _, p in ipairs(c) do
    local f = io.open(p, "rb")
    if f then f:close(); return p end
  end
end

local function engine()
  if wasmcraft then return wasmcraft end
  local path = M.engine_path or find({ "wasmcraft", "../wasmcraft/dist/wasmcraft.lua",
    "/Users/robertwendt/wasmcraft/dist/wasmcraft.lua" })
  assert(path, "wasmcraft bundle not found (set runtime.engine_path)")
  wasmcraft = assert(loadfile(path))()
  return wasmcraft
end
function M.engine() return engine() end

local function u32le(s, i)
  local b1, b2, b3, b4 = s:byte(i, i + 3)
  return b1 + b2 * 256 + b3 * 65536 + b4 * 16777216
end

-- read a LENBUF (4-byte u32 LE length + payload) out of guest memory
local function read_lenbuf(mem, ptr)
  local hdr = mem:loadstr(ptr, 4)
  local len = u32le(hdr, 1)
  return mem:loadstr(ptr + 4, len)
end

-- ---- reactor kind ------------------------------------------------------------
-- opts.call: optional host import implementing crabcraft.call (service mesh):
--   call(workload_name, func_addr, param_bytes) -> ok(boolean), bytes_or_err
function M.load_reactor(bytes, opts)
  opts = opts or {}
  local wc = engine()
  local module = opts.module or wc.load(bytes)
  local hostfs = (wc.hostfs and wc.hostfs(opts.root or ".")) or nil
  local host = wc.wasi.make({
    fs = hostfs, root = opts.root or ".",
    args = { opts.name or "workload" },
    write = opts.write or function() end,
    writeerr = opts.writeerr or function() end,
  })
  local imports = { wasi_snapshot_preview1 = host }
  local pending_call_reply -- LENBUF bytes the next crabcraft.call returns
  local inst
  imports.crabcraft = {
    -- wasmcraft host-import convention: fn(args_array, inst) -> results_array.
    -- crab_call(wl_ptr, wl_len, fn_ptr, fn_len, par_ptr, par_len) -> lenbuf ptr
    call = function(a)
      local wl_ptr, wl_len, fn_ptr, fn_len, par_ptr, par_len =
        a[1], a[2], a[3], a[4], a[5], a[6]
      local mem = inst.memory
      local wl = mem:loadstr(wl_ptr, wl_len)
      local fn = mem:loadstr(fn_ptr, fn_len)
      local par = mem:loadstr(par_ptr, par_len)
      local reply
      if not opts.call then
        reply = string.char(1) .. require("cmval").encode("string",
          "no mesh: crabcraft.call not wired on this host")
      else
        local ok, body = opts.call(wl, fn, par)
        if ok then reply = string.char(0) .. body
        else reply = string.char(1) .. require("cmval").encode("string", tostring(body)) end
      end
      -- hand it to the guest via its own allocator
      local ptr = inst:call("crab_alloc", #reply + 4)
      local len = #reply
      inst.memory:storestr(ptr, string.char(len % 256, math.floor(len / 256) % 256,
        math.floor(len / 65536) % 256, math.floor(len / 16777216) % 256) .. reply)
      return { ptr }
    end,
  }
  inst = wc.instantiate(module, imports,
    { mode = opts.mode or "transpile", chunk_cache = opts.chunk_cache })
  -- reactor init if present
  pcall(function() inst:call("_initialize") end)

  local w = { inst = inst, mode = inst.mode }
  local sptr = inst:call("crab_schema")
  w.schema_json = read_lenbuf(inst.memory, sptr)

  function w:invoke(func_addr, param_bytes)
    param_bytes = param_bytes or ""
    local mem = self.inst.memory
    local name_ptr = self.inst:call("crab_alloc", #func_addr)
    mem:storestr(name_ptr, func_addr)
    local arg_ptr = 0
    if #param_bytes > 0 then
      arg_ptr = self.inst:call("crab_alloc", #param_bytes)
      mem:storestr(arg_ptr, param_bytes)
    end
    local ok, rptr = pcall(function()
      return self.inst:call("crab_invoke", name_ptr, #func_addr, arg_ptr, #param_bytes)
    end)
    if not ok then return { ok = false, err = "guest trap: " .. tostring(rptr) } end
    local payload = read_lenbuf(mem, rptr)
    local status = payload:byte(1)
    if status == 0 then
      return { ok = true, result = payload:sub(2) }
    end
    -- error body = encoded string
    local msg = require("cmval").decode("string", payload, 2)
    return { ok = false, err = msg }
  end
  return w
end

-- ---- command kind ------------------------------------------------------------
-- Each invoke is a fresh _start run: body on stdin, stdout is the reply.
function M.run_command(bytes, body, opts)
  opts = opts or {}
  local wc = engine()
  local module = opts.module or wc.load(bytes)
  local out = {}
  local input, pos = (body or "") .. "\n", 1
  local argt = { opts.name or "workload" }
  for _, a in ipairs(opts.argv or {}) do argt[#argt + 1] = tostring(a) end
  local host = wc.wasi.make({
    fs = (wc.hostfs and wc.hostfs(opts.root or ".")) or nil,
    root = opts.root or ".",
    args = argt,
    stdin = function(maxlen)
      if pos > #input then return "" end
      local c = input:sub(pos, pos + maxlen - 1)
      pos = pos + #c
      return c
    end,
    write = function(s) out[#out + 1] = s end,
    writeerr = function(s) out[#out + 1] = s end,
  })
  local inst = wc.instantiate(module, { wasi_snapshot_preview1 = host },
    { mode = opts.mode or "transpile", chunk_cache = opts.chunk_cache })
  local ok, err = pcall(function() inst:call("_start") end)
  if not ok and not (type(err) == "table" and err[wc.wasi.EXIT]) then
    return nil, "guest trap: " .. tostring(err)
  end
  return table.concat(out)
end

return M

end
-- crabcraft worker: the data plane (docs/WIRE.md section 3).
-- A computer with disks; one workload per disk (the disk = the volume).
--   worker [gatewayName]      (default: first crabcraft host found)
-- Needs beside it: runtime.lua, cmval.lua, json.lua, and the wasmcraft bundle.
local PROTO = "crabcraft"
local CRAB_VERSION = "0.2.8" -- stamped by tools/amalgamate.py
local WORKER_URL = "https://github.com/r33drichards/crabcraft/releases/latest/download/worker.lua"
local args = { ... }
if args[1] == "--install" and type(fs) == "table" then
  table.remove(args, 1)
  local rest = ""
  for _, a in ipairs(args) do rest = rest .. ', "' .. a .. '"' end
  local h = fs.open("startup.lua", "w")
  h.write('shell.run("worker"' .. rest .. ')\n')
  h.close()
  print("worker: installed to startup.lua")
end
local gwname = args[1]
if gwname == "--slots" then gwname = nil end

if package and package.path then package.path = "host/?.lua;./?.lua;" .. package.path end
local rt = require("runtime")
local cm = require("cmval")
local json = require("json")

local label = (os.getComputerLabel and os.getComputerLabel())
  or ("worker-" .. tostring(os.getComputerID and os.getComputerID() or 0))

local opened = false
if type(peripheral) == "table" and peripheral.find then
  peripheral.find("modem", function(n) rednet.open(n); opened = true end)
end
if not opened then print("worker: no modem attached."); return end

-- ---- find the gateway ---------------------------------------------------------
-- picatd lesson: busy computers answer dns lookups too slowly for the window;
-- cache the id after first contact and ping it directly on later boots.
local gw
do
  local f = io.open(".crab_gateway", "r")
  local cached = f and tonumber(f:read("*a")); if f then f:close() end
  if cached then
    rednet.send(cached, { type = "ping", id = "wkr:ping" }, PROTO)
    local t = os.clock()
    while os.clock() - t < 10 do
      local s, r = rednet.receive(PROTO, 10 - (os.clock() - t))
      if s == cached and type(r) == "table" and r.id == "wkr:ping" then gw = cached; break end
    end
    if not gw then print("worker: cached gateway #" .. cached .. " silent - rediscovering") end
  end
end
if not gw then
  for attempt = 1, 6 do
    if gwname then gw = rednet.lookup(PROTO, gwname, 5)
    else local hosts = { rednet.lookup(PROTO, nil, 5) }; gw = hosts[1] end
    if gw then break end
    print("worker: no gateway answered lookup " .. attempt .. "/6")
  end
end
if not gw then print("worker: no gateway on the network."); return end
do local f = io.open(".crab_gateway", "w"); if f then f:write(tostring(gw)); f:close() end end
print("worker: gateway is #" .. gw)

-- ---- slots: one workload per disk ----------------------------------------------
-- Real CC: every mounted disk drive is a slot. Fallback (sim / no drives):
-- local directories slot1..slotN (N = 1, or --slots n).
local function fexists(p)
  if type(fs) == "table" then return fs.exists(p) end
  local f = io.open(p, "r"); if f then f:close(); return true end
  local d = io.open(p .. "/.keep", "r"); if d then d:close(); return true end
  return false
end
local function mkdir(p)
  if type(fs) == "table" then fs.makeDir(p)
  elseif os.execute then os.execute("mkdir -p '" .. p .. "'") end
end
local function readfile(p)
  if type(fs) == "table" and fs.open then
    if not fs.exists(p) then return nil end
    local h = fs.open(p, "rb"); local d = h.readAll(); h.close(); return d
  end
  local f = io.open(p, "rb"); if not f then return nil end
  local d = f:read("*a"); f:close(); return d
end
local function writefile(p, d)
  if type(fs) == "table" and fs.open then
    local h = fs.open(p, "wb"); h.write(d); h.close(); return
  end
  local f = assert(io.open(p, "wb")); f:write(d); f:close()
end
local function delfile(p)
  if type(fs) == "table" then pcall(fs.delete, p) else pcall(os.remove, p) end
end

local slots = {} -- slotname -> { dir, meta = {name,kind,url}|nil, w = reactor|nil, module = decoded|nil, queue = {} }
do
  local dirs = {}
  if type(peripheral) == "table" and peripheral.getNames then
    for _, pname in ipairs(peripheral.getNames()) do
      if peripheral.getType(pname) == "drive" then
        local mp = peripheral.call(pname, "getMountPath")
        if mp then dirs[#dirs + 1] = "/" .. mp end
      end
    end
  end
  if #dirs == 0 then
    local n = 1
    for i = 1, #args do if args[i] == "--slots" then n = tonumber(args[i + 1]) or 1 end end
    for i = 1, n do
      dirs[#dirs + 1] = "slot" .. i
      mkdir("slot" .. i)
    end
  end
  for _, d in ipairs(dirs) do
    slots[d] = { dir = d, queue = {} }
    local meta = readfile(d .. "/crab-meta.json")
    if meta then
      local ok, m = pcall(json.decode, meta)
      if ok then slots[d].meta = m end
    end
  end
end

local function slot_used(dir)
  if type(fs) == "table" and fs.getCapacity then
    local ok, cap = pcall(fs.getCapacity, dir)
    local ok2, free = pcall(fs.getFreeSpace, dir)
    if ok and ok2 and cap and free then return cap - free end
  end
  local total = 0
  if type(fs) == "table" and fs.list then
    local function walk(p)
      for _, f in ipairs(fs.list(p) or {}) do
        local fp = fs.combine(p, f)
        if fs.isDir(fp) then walk(fp) else total = total + (fs.getSize(fp) or 0) end
      end
    end
    pcall(walk, dir)
  end
  return total
end

local function slot_list()
  local out = {}
  for sname, s in pairs(slots) do
    out[#out + 1] = { disk = sname, workload = s.meta and s.meta.name or nil,
      used = slot_used(s.dir),
      state = s.meta and ((s.w or s.sessions or (s.meta.kind == "command" and s.module)) and "running" or "loading") or nil }
  end
  return out
end

-- ---- mesh: guests calling other workloads through the gateway -------------------
local mesh_seq = 0
local mesh_replies = {}
local function mesh_call(target, func, params)
  mesh_seq = mesh_seq + 1
  local id = "mesh:" .. tostring(os.getComputerID and os.getComputerID() or 0) .. ":" .. mesh_seq
  rednet.send(gw, { type = "invoke", id = id, name = target, func = func, params = params }, PROTO)
  local deadline = os.clock() + 60
  while os.clock() < deadline do
    if mesh_replies[id] ~= nil then
      local r = mesh_replies[id]
      mesh_replies[id] = nil
      if r.ok then return true, r.result or "" end
      return false, r.err or "mesh call failed"
    end
    os.pullEvent("crab_mesh")
  end
  return false, "mesh call timed out: " .. target
end

-- ---- picat session support (kind = "session") -------------------------------------
-- Warm interpreter sessions: boot once at assign, each invoke loads + runs the
-- program in the live REPL (no per-call runtime boot). v0: Picat only.
local PICATLIB_URL = "https://github.com/r33drichards/wasmcraft/releases/latest/download/picat.lua"
local picatlib
local function get_picatlib()
  if picatlib then return picatlib end
  if not fexists("picatlib") then
    if not http then error("session kind needs http to fetch picat.lua") end
    local r = assert(http.get(PICATLIB_URL), "cannot fetch picat.lua")
    writefile("picatlib", r.readAll())
    r.close()
  end
  picatlib = dofile("picatlib")
  return picatlib
end

-- ---- workload lifecycle ----------------------------------------------------------
local function start_slot(sname)
  local s = slots[sname]
  if not s.meta then return end
  local wasm = readfile(s.dir .. "/workload.wasm")
  if not wasm then print("worker: slot " .. sname .. " missing workload.wasm"); s.meta = nil; return end
  -- TRANSPILE AT DEPLOY: the whole module is compiled here, once; every
  -- request (and every command-kind re-instantiate) is served from the cache.
  -- warm: false in the manifest skips it (functions then compile on first use).
  local wc = rt.engine()
  if s.meta.kind ~= "session" then s.module = wc.load(wasm) end
  s.cache = {}
  if s.meta.kind ~= "session" and s.meta.warm ~= false and wc.precompile_cache then
    local t0 = os.clock()
    local cache, n, fb = wc.precompile_cache(s.module, "transpile")
    s.cache = cache
    print(("worker: '%s' transpiled %d fns%s in %s (%.1fs)"):format(s.meta.name, n,
      fb > 0 and (" (" .. fb .. " interp)") or "", sname, os.clock() - t0))
  end
  if s.meta.kind == "session" then
    local t0 = os.clock()
    local plib = get_picatlib()
    s.wasmbytes = wasm
    s.sessions = { main = plib.session({ module = wasm, root = s.dir, mode = "transpile" }) }
    print(("worker: '%s' (session) warm in %s (%.1fs)"):format(s.meta.name, sname, os.clock() - t0))
  elseif s.meta.kind == "command" then
    print(("worker: '%s' (command) ready in %s"):format(s.meta.name, sname))
  else
    local t0 = os.clock()
    s.w = rt.load_reactor(wasm, { mode = "transpile", root = s.dir, name = s.meta.name,
      call = mesh_call, chunk_cache = s.cache, module = s.module })
    print(("worker: '%s' (reactor) warm in %s (%.1fs)"):format(s.meta.name, sname, os.clock() - t0))
  end
end

local function assign(msg)
  local s = slots[msg.slot]
  if not s then return { ok = false, err = "no slot " .. tostring(msg.slot) } end
  local wasm
  local file = msg.url:match("^file:(.+)$")
  if file then
    wasm = readfile(file)
    if not wasm then return { ok = false, err = "file not found: " .. file } end
  else
    if not http then return { ok = false, err = "no http on this worker" } end
    local r, herr = http.get(msg.url, nil, true) -- binary
    if not r then return { ok = false, err = "fetch failed: " .. tostring(herr) } end
    wasm = r.readAll()
    r.close()
  end
  writefile(s.dir .. "/workload.wasm", wasm)
  s.meta = { name = msg.name, kind = msg.kind or "reactor", url = msg.url, warm = msg.warm,
    args = msg.args, body_file = msg.body_file }
  s.cache = {} -- new workload, fresh transpile cache
  writefile(s.dir .. "/crab-meta.json", json.encode(s.meta))
  s.w, s.module = nil, nil
  local ok, err = pcall(start_slot, msg.slot)
  if not ok then
    -- clear the slot completely: a phantom meta would be advertised by
    -- heartbeats and wrongly adopted by the gateway
    s.meta, s.w, s.module, s.sessions, s.wasmbytes = nil, nil, nil, nil, nil
    delfile(s.dir .. "/crab-meta.json")
    delfile(s.dir .. "/workload.wasm")
    return { ok = false, err = "start failed: " .. tostring(err) }
  end
  return { ok = true }
end

local function drain(msg)
  local s = slots[msg.slot]
  if s then
    if s.sessions then
      for _, sess in pairs(s.sessions) do pcall(function() sess:close() end) end
    end
    s.meta, s.w, s.module, s.sessions, s.wasmbytes, s.sessq = nil, nil, nil, nil, nil, nil
    os.queueEvent("crab_work") -- wake session tasks so they notice and exit
    delfile(s.dir .. "/crab-meta.json")
    delfile(s.dir .. "/workload.wasm")
    print("worker: drained " .. msg.slot)
  end
end

local function find_workload(name)
  for sname, s in pairs(slots) do
    if s.meta and s.meta.name == name then return s end
  end
end

local function do_invoke(msg)
  local s = find_workload(msg.name)
  if not s then return { ok = false, err = "workload not here: " .. tostring(msg.name) } end
  if s.meta.kind == "session" then
    -- handled by per-session tasks (route_session_invoke); only reached if
    -- the slot is still booting its main session
    return { ok = false, err = "session still booting" }
  end
  if s.meta.kind == "command" then
    local body = msg.body or ""
    local stdin_body = body
    if s.meta.body_file then
      -- custom-runtime pattern: the body becomes a file on the volume and the
      -- module runs with manifest argv (e.g. an interpreter running a program)
      writefile(s.dir .. "/" .. s.meta.body_file, body)
      stdin_body = ""
    end
    local out, err = rt.run_command(nil, stdin_body, { module = s.module, root = s.dir,
      name = s.meta.name, mode = "transpile", chunk_cache = s.cache, argv = s.meta.args })
    if not out then return { ok = false, err = err } end
    return { ok = true, result = out }
  end
  if not s.w then return { ok = false, err = "workload still loading" } end
  local r = s.w:invoke(msg.func, msg.params or "")
  return { ok = r.ok, result = r.result, err = r.err }
end

-- ---- tasks ----------------------------------------------------------------------
local tasks = {}
local function spawn(fn) tasks[#tasks + 1] = { co = coroutine.create(fn) } end

-- ---- shared execution for session workloads (picatd's model) -----------------
-- Each named session gets its own queue + coroutine: a long solve in session
-- 'a' never blocks session 'b' or the slot's control messages. Sessions boot
-- lazily inside their own task (a multi-minute boot doesn't stall the slot).
local function session_task(s, sname2)
  return function()
    while true do
      local q = s.sessq and s.sessq[sname2]
      if not q then return end -- drained
      if #q.jobs == 0 then
        os.pullEvent("crab_work")
      else
        local job = table.remove(q.jobs, 1)
        local m = job.msg
        local reply
        if m.reset then
          local old = s.sessions and s.sessions[sname2]
          if old then pcall(function() old:close() end) end
          if s.sessions then s.sessions[sname2] = nil end
          s.sessq[sname2] = nil
          reply = { ok = true, result = "session '" .. sname2 .. "' reset" }
          rednet.send(job.sender, { type = "invoke-reply", id = m.id,
            ok = reply.ok, result = reply.result, err = reply.err }, PROTO)
          return -- task ends; a fresh invoke re-creates queue+task+session
        end
        if s.sessions and not s.sessions[sname2] then
          print(("worker: booting session '%s' for '%s'"):format(sname2, s.meta and s.meta.name or "?"))
          local plib = get_picatlib()
          local okb, sess = pcall(plib.session,
            { module = s.wasmbytes, root = s.dir, mode = "transpile" })
          if okb then s.sessions[sname2] = sess
          else reply = { ok = false, err = "session boot failed: " .. tostring(sess) } end
        end
        if not reply then
          if not (s.sessions and s.sessions[sname2]) then
            reply = { ok = false, err = "workload drained" }
          else
            local ok, out = pcall(function()
              return s.sessions[sname2]:run(m.body or "", "req_" .. sname2 .. ".pi")
            end)
            reply = ok and { ok = true, result = out }
                       or { ok = false, err = "session error: " .. tostring(out) }
          end
        end
        rednet.send(job.sender, { type = "invoke-reply", id = m.id,
          ok = reply.ok, result = reply.result, err = reply.err }, PROTO)
      end
    end
  end
end

local function route_session_invoke(s, job)
  local sname2 = job.msg.session or "main"
  s.sessq = s.sessq or {}
  if not s.sessq[sname2] then
    s.sessq[sname2] = { jobs = {} }
    spawn(session_task(s, sname2))
  end
  local q = s.sessq[sname2].jobs
  q[#q + 1] = job
  os.queueEvent("crab_work")
end

-- PER-SLOT queues + coroutines (the picatd lesson): a workload blocked on a
-- mesh call must not starve other workloads on the same computer - otherwise
-- two co-located services calling each other deadlock.
local function find_slot_for(msg)
  if msg.type == "assign" or msg.type == "drain" then return slots[msg.slot] end
  for _, s in pairs(slots) do
    if s.meta and s.meta.name == msg.name then return s end
  end
end

spawn(function() -- receiver
  while true do
    local sender, msg = rednet.receive(PROTO)
    if type(msg) == "table" then
      if msg.type == "update" and sender == gw then
        -- gateway-ordered rollout: refetch self and reboot (startup relaunches)
        print("worker: updating from " .. (msg.url or WORKER_URL))
        local r = http and http.get(msg.url or WORKER_URL, nil, true)
        if r then
          local me = (shell and shell.getRunningProgram and shell.getRunningProgram()) or "worker"
          local h = fs.open(me, "wb") h.write(r.readAll()) h.close() r.close()
          rednet.send(sender, { ok = true, id = msg.id }, PROTO)
          print("worker: updated - rebooting")
          os.sleep(0.5)
          os.reboot()
        else
          rednet.send(sender, { ok = false, err = "fetch failed", id = msg.id }, PROTO)
        end
      elseif msg.type == "invoke-reply" or (msg.id and tostring(msg.id):match("^mesh:") and msg.ok ~= nil) then
        mesh_replies[msg.id] = msg
        os.queueEvent("crab_mesh")
      elseif msg.type == "assign" or msg.type == "invoke" or msg.type == "drain" then
        local s = find_slot_for(msg)
        if s then
          s.queue[#s.queue + 1] = { sender = sender, msg = msg }
          os.queueEvent("crab_work")
        elseif msg.type == "invoke" then
          rednet.send(sender, { type = "invoke-reply", id = msg.id,
            ok = false, err = "workload not here: " .. tostring(msg.name) }, PROTO)
        elseif msg.id then
          rednet.send(sender, { ok = false, err = "no slot " .. tostring(msg.slot), id = msg.id }, PROTO)
        end
      end
    end
  end
end)

local function slot_task(sname, s)
  return function()
    while true do
      if #s.queue == 0 then
        os.pullEvent("crab_work")
      else
        local job = table.remove(s.queue, 1)
        local m = job.msg
        if m.type == "assign" then
          local r = assign(m)
          r.id = m.id
          rednet.send(job.sender, r, PROTO)
          rednet.send(gw, { type = "heartbeat", worker = label, slots = slot_list(), version = CRAB_VERSION }, PROTO)
        elseif m.type == "drain" then
          drain(m)
          rednet.send(gw, { type = "heartbeat", worker = label, slots = slot_list(), version = CRAB_VERSION }, PROTO)
        elseif m.type == "invoke" then
          if s.meta and s.meta.kind == "session" and s.sessions then
            route_session_invoke(s, job)
          else
            local ok, r = pcall(do_invoke, m)
            if not ok then r = { ok = false, err = "worker error: " .. tostring(r) } end
            rednet.send(job.sender, { type = "invoke-reply", id = m.id,
              ok = r.ok, result = r.result, err = r.err }, PROTO)
          end
        end
      end
    end
  end
end

spawn(function() -- boot: recover disks, register, then idle
  for sname, s in pairs(slots) do
    if s.meta then
      local ok, err = pcall(start_slot, sname)
      if not ok then print("worker: recover " .. sname .. " failed: " .. tostring(err)) end
    end
  end
  rednet.send(gw, { type = "register", worker = label, slots = slot_list(), version = CRAB_VERSION, id = "reg" }, PROTO)
end)

for sname, s in pairs(slots) do
  spawn(slot_task(sname, s))
end

spawn(function() -- heartbeat
  while true do
    local timer = os.startTimer(5)
    repeat local _, p = os.pullEvent("timer") until p == timer
    rednet.send(gw, { type = "heartbeat", worker = label, slots = slot_list(), version = CRAB_VERSION }, PROTO)
  end
end)

local nslots = 0
for _ in pairs(slots) do nslots = nslots + 1 end
print(("worker '%s' up: %d slot(s), gateway #%d"):format(label, nslots, gw))
local ev = {}
while true do
  for i = #tasks, 1, -1 do
    local t = tasks[i]
    if t.filter == nil or t.filter == ev[1] or ev[1] == "terminate" then
      local ok, f = coroutine.resume(t.co, (table.unpack or unpack)(ev))
      if not ok then print("worker task error: " .. tostring(f)); table.remove(tasks, i)
      elseif coroutine.status(t.co) == "dead" then table.remove(tasks, i)
      else t.filter = f end
    end
  end
  ev = { os.pullEventRaw() }
  if ev[1] == "terminate" then print("worker: stopped.") return end
end
