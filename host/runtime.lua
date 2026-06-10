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
  local host = wc.wasi.make({
    fs = (wc.hostfs and wc.hostfs(opts.root or ".")) or nil,
    root = opts.root or ".",
    args = { opts.name or "workload" },
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
