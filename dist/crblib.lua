-- crblib: crabcraft client runtime (amalgamated; see host/)

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
preload["yaml"] = function(...)
-- Minimal YAML subset for crabcraft manifests (docs/WIRE.md section 4):
-- nested maps by 2-space indentation, lists of scalars ("- item"), scalars
-- (plain / 'single' / "double" quoted, numbers, true/false), comments (#).
-- No anchors, no multi-doc, no block scalars, no flow collections.
local M = {}

local function parse_scalar(s)
  s = s:match("^%s*(.-)%s*$")
  local q = s:match('^"(.*)"$') or s:match("^'(.*)'$")
  if q then return q end
  if s == "true" then return true end
  if s == "false" then return false end
  if s == "null" or s == "~" or s == "" then return nil end
  local n = tonumber(s)
  if n then return n end
  return s
end

function M.decode(text)
  local lines = {}
  for line in (text .. "\n"):gmatch("(.-)\n") do
    local stripped = line:gsub("#.*$", "")
    if stripped:match("%S") then
      local indent = #(stripped:match("^( *)"))
      lines[#lines + 1] = { indent = indent, body = stripped:sub(indent + 1) }
    end
  end

  local pos = 0
  local function parse_block(indent)
    local node
    while pos < #lines do
      local ln = lines[pos + 1]
      if ln.indent < indent then break end
      if ln.indent > indent then error("yaml: bad indentation at '" .. ln.body .. "'") end
      if ln.body:match("^%- ") or ln.body == "-" then
        node = node or {}
        if type(node) ~= "table" then error("yaml: mixed list/map") end
        pos = pos + 1
        local item = ln.body:sub(3)
        if item:match("%S") then
          node[#node + 1] = parse_scalar(item)
        else
          node[#node + 1] = parse_block(indent + 2)
        end
      else
        local key, rest = ln.body:match("^([%w%-%._/]+):%s*(.*)$")
        if not key then error("yaml: cannot parse line '" .. ln.body .. "'") end
        node = node or {}
        pos = pos + 1
        if rest:match("%S") then
          node[key] = parse_scalar(rest)
        else
          -- nested block (or empty value)
          local nxt = lines[pos + 1]
          if nxt and nxt.indent > indent then
            node[key] = parse_block(nxt.indent)
          else
            node[key] = nil
          end
        end
      end
    end
    return node or {}
  end

  return parse_block(lines[1] and lines[1].indent or 0)
end

return M

end
preload["client"] = function(...)
-- crabcraft client library: THE FACTORY (docs/WIRE.md section 5).
-- Connect to a gateway, then get schema-driven proxies for any workload:
--   local crab = require("client").connect()         -- or .connect("gateway")
--   local hello = crab:workload("hello")
--   print(hello.greet({ name = "steve", excited = true }))  -- typed mesh call
-- Proxies are generated from the workload's resolved-WIT schema at runtime:
-- params are plain Lua tables validated/encoded per the schema; results are
-- decoded back to Lua values. No codegen.
local PROTO = "crabcraft"
package.path = "host/?.lua;./?.lua;" .. package.path
local cm = require("cmval")
local json = require("json")
local schema_mod = require("schema")

local M = {}

local function open_modems()
  local ok = false
  if type(peripheral) == "table" and peripheral.find then
    peripheral.find("modem", function(n) rednet.open(n); ok = true end)
  end
  return ok
end

function M.connect(gwname, opts)
  opts = opts or {}
  assert(open_modems(), "client: no modem attached")
  local gw
  -- cached id + direct ping first: busy gateways miss dns lookup windows
  local function readcache()
    local f = io.open(".crab_gateway", "r")
    if not f then return nil end
    local v = tonumber(f:read("*a")); f:close(); return v
  end
  local cached = readcache()
  if cached and not gwname then
    rednet.send(cached, { type = "ping", id = "cli:ping" }, PROTO)
    local t = os.clock()
    while os.clock() - t < 5 do
      local s, r = rednet.receive(PROTO, 5 - (os.clock() - t))
      if s == cached and type(r) == "table" and r.id == "cli:ping" then gw = cached; break end
    end
  end
  if not gw then
    for _ = 1, opts.attempts or 4 do
      if gwname then gw = rednet.lookup(PROTO, gwname, 5)
      else local hosts = { rednet.lookup(PROTO, nil, 5) }; gw = hosts[1] end
      if gw then break end
    end
  end
  assert(gw, "client: no crabcraft gateway on the network")
  local f = io.open(".crab_gateway", "w")
  if f then f:write(tostring(gw)); f:close() end

  local C = { gw = gw, seq = 0 }

  function C:request(msg, timeout)
    self.seq = self.seq + 1
    msg.id = ("c%d:%d"):format(os.getComputerID and os.getComputerID() or 0, self.seq)
    rednet.send(self.gw, msg, PROTO)
    local deadline = os.clock() + (timeout or 60)
    while os.clock() < deadline do
      local s, r = rednet.receive(PROTO, math.max(0.1, deadline - os.clock()))
      if s == self.gw and type(r) == "table" and r.id == msg.id then
        if r.status then print("(" .. tostring(r.status) .. ")")
        else return r end
      end
    end
    return { ok = false, err = "timed out waiting for gateway" }
  end

  -- raw operations
  function C:list() return self:request({ type = "list" }) end
  function C:deploy(spec) return self:request({ type = "deploy", name = spec.name,
    url = spec.wasm or spec.url, kind = spec.kind, schema = spec.schema, warm = spec.warm }) end
  function C:remove(name) return self:request({ type = "remove", name = name }) end
  function C:schema(name)
    local r = self:request({ type = "schema", name = name })
    if not r.ok then return nil, r.err end
    return r.schema, nil, r.kind
  end

  -- the factory from a schema you already have (generated clients embed it)
  function C:workload_from_schema(name, sjson, kind)
    return self:_proxy(name, sjson, kind)
  end

  -- the factory: a proxy whose methods are the workload's WIT functions
  function C:workload(name)
    local sjson, err, kind = self:schema(name)
    if sjson == nil and kind ~= "command" then
      error("no schema for workload '" .. name .. "': " .. tostring(err), 0)
    end
    return self:_proxy(name, sjson, kind)
  end

  function C:_proxy(name, sjson, kind)
    if kind == "command" then
      -- command kind: one callable taking/returning JSON-able tables
      return setmetatable({}, { __call = function(_, body)
        local r = self:request({ type = "invoke", name = name,
          body = type(body) == "string" and body or json.encode(body) }, 120)
        if not r.ok then error("invoke failed: " .. tostring(r.err), 0) end
        local ok, decoded = pcall(json.decode, r.result)
        return ok and decoded or r.result
      end })
    end
    local sc = schema_mod.load(sjson)
    local proxy = { __name = name, __schema = sc }
    -- short name -> full address (unambiguous short names only)
    local short = {}
    for addr in pairs(sc.functions) do
      local fname = addr:match("#(.+)$")
      short[fname] = short[fname] == nil and addr or false
    end
    setmetatable(proxy, { __index = function(_, fname)
      local addr = sc.functions[fname] and fname or short[fname]
      if addr == false then error("ambiguous function name '" .. fname .. "' - use the full address", 0) end
      if not addr then error("no function '" .. fname .. "' on workload '" .. name .. "'", 0) end
      local f = sc.functions[addr]
      return function(argtbl)
        argtbl = argtbl or {}
        local values = {}
        -- sugar: a function with ONE record param accepts the record's fields
        -- directly: hello.greet{ name = "x" } instead of { req = { name = "x" } }
        if #f.params == 1 and argtbl[f.params[1].name] == nil and argtbl[1] == nil then
          values[1] = argtbl
        else
          -- positional or named: named keys win, else array order
          for i, p in ipairs(f.params) do
            if argtbl[p.name] ~= nil then values[i] = argtbl[p.name]
            else values[i] = argtbl[i] end
          end
        end
        local bytes = cm.encode_params(sc.param_types(addr), values)
        local r = self:request({ type = "invoke", name = name, func = addr, params = bytes }, 120)
        if not r.ok then error("invoke failed: " .. tostring(r.err), 0) end
        if f.result then return (cm.decode(f.result, r.result or "")) end
        return true
      end
    end })
    return proxy
  end

  return C
end

return M

end
return { client = require("client"), json = require("json"), cmval = require("cmval"), schema = require("schema"), yaml = require("yaml") }