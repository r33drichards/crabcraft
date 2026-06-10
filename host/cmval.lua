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
