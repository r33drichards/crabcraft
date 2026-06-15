-- cardui_core: the dependency-injected kiosk state machine, shared by
-- demo/cardui.lua (real peripherals + mesh) and host/cardui_smoke.lua (fakes).
-- It bridges monitor taps and disk events to the `auth` workload and pushes
-- results back into the React UI via the engine's web_message export.
--
-- Bridge (must match web/cardui/CardApp.jsx exactly):
--   React -> host : console.log("\1CRB <json>") ; the host's stderr callback
--                   decodes the {op=...} commands into `cmds` during web_event.
--   host -> React : web_message(json) -> CardApp's __registerHostMsg handler.
--                   shapes: {ev="status",text} | {ev="granted",username,role}
--                         | {ev="denied",reason} | {ev="enrolled",user_id,username}
--
-- deps (d):
--   inst          web-engine wasm instance (web_event/web_message/web_malloc/
--                 web_free, .memory)
--   cmds          list the engine stderr callback fills with decoded commands;
--                 cleared at the start of every tap
--   auth          mesh proxy: auth.verify(tbl)->{is_err,ok,err},
--                 auth.register(tbl)->{is_err,ok,err}
--   sign          function(private_key, nonce) -> signature (errors on failure)
--   read_card     function(side) -> {user_id, private_key} | nil
--   write_card    function(side, card)
--   gen_nonce     function() -> string
--   read_username function() -> string  (real: terminal read(); test: canned)
--   door          function()            (pulse the door redstone)
--   json          {encode, decode}
--   store         sqlite workload name (default "sqlite")
return function(d)
  local store = d.store or "sqlite"
  local pending = nil   -- nil | "verify" | { kind="writecard", cred=…, username=… }

  local function wstr(s)
    local p = d.inst:call("web_malloc", #s + 1)
    d.inst.memory:storestr(p, s); d.inst.memory:set8(p + #s, 0)
    return p
  end
  local function msg(tbl)
    local p = wstr(d.json.encode(tbl))
    d.inst:call("web_message", p); d.inst:call("web_free", p)
  end

  local function do_verify(side)
    local card = d.read_card(side)
    if not (card and card.user_id and card.private_key) then
      msg({ ev = "denied", reason = "unrecognized card" }); pending = nil; return
    end
    local nonce = d.gen_nonce()
    local ok, sig = pcall(d.sign, card.private_key, nonce)
    if not ok then
      msg({ ev = "denied", reason = "sign error" }); pending = nil; return
    end
    local r = d.auth.verify({ store = store, ["user-id"] = card.user_id, nonce = nonce, signature = sig })
    if r.is_err then
      msg({ ev = "denied", reason = tostring(r.err) })
    else
      local acct = d.json.decode(r.ok)
      msg({ ev = "granted", username = acct.username, role = acct.meta and acct.meta.role })
      d.door()
    end
    pending = nil
  end

  local function handle(c)
    if c.op == "signin" then
      pending = "verify"; msg({ ev = "status", text = "tap your card…" })
    elseif c.op == "signup" then
      local name = d.read_username()
      msg({ ev = "status", text = "registering…" })
      local r = d.auth.register({ store = store, username = name, meta = "{}" })
      if r.is_err then
        msg({ ev = "denied", reason = tostring(r.err) }); pending = nil
      else
        pending = { kind = "writecard", cred = d.json.decode(r.ok), username = name }
        msg({ ev = "status", text = "insert a BLANK floppy…" })
      end
    elseif c.op == "opendoor" then
      d.door()
    elseif c.op == "lock" or c.op == "cancel" then
      pending = nil
    end
  end

  -- a monitor tap at 1-based cell (x,y): clear last commands, deliver the click
  -- (0-based to the engine), then act on whatever CardApp emitted.
  local function on_tap(x, y)
    for i = #d.cmds, 1, -1 do d.cmds[i] = nil end
    local tp = wstr("click"); d.inst:call("web_event", tp, x - 1, y - 1); d.inst:call("web_free", tp)
    for _, c in ipairs(d.cmds) do handle(c) end
  end

  -- a disk inserted on `side`: advance whatever the UI is waiting for.
  local function on_disk(side)
    if pending == "verify" then
      do_verify(side)
    elseif type(pending) == "table" and pending.kind == "writecard" then
      d.write_card(side, { user_id = pending.cred.user_id, private_key = pending.cred.private_key })
      msg({ ev = "enrolled", user_id = pending.cred.user_id, username = pending.username }); pending = nil
    end
  end

  return {
    on_tap = on_tap,
    on_disk = on_disk,
    handle = handle,
    msg = msg,
    pending = function() return pending end,
  }
end
