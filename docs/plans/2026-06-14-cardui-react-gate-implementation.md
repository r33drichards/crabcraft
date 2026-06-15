# cardui — React Monitor Gate Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Ship an integrated kiosk station (`demo/cardui.lua`) whose CC monitor renders a React sign-in/sign-up UI gated on the existing `auth` workload, with the card private key never leaving the machine.

**Architecture:** One Lua program owns a single event loop and three runtimes: `web.wasm` (React render, via the wasmcraft browser substrate), `auth.wasm` (local Ed25519 signing, via `cardlib.runtime`), and a `cardlib` mesh client (`register`/`verify` on the `auth` workload). React→host commands ride `console.log` (sentinel lines on the engine's stderr); host→React results ride the new `web_message(json)` export. The auth mechanics (signer, floppy I/O, nonce, door pulse) are lifted from `demo/cardlock.lua`.

**Tech Stack:** Lua (CC:Tweaked), React 18 + react-reconciler (esbuild bundle, no react-dom), the wasmcraft web engine, crab ABI / cmval, Ed25519 auth workload.

**Design:** `docs/plans/2026-06-14-cardui-react-gate-design.md`
**Dependency:** the wasmcraft `web_message` engine change (`wasmcraft/docs/plans/2026-06-14-web-message-channel-implementation.md`) **must be built first** — the bridge needs it.

**Conventions:**
- Run Lua/tests via nix (`.claude` rule). crabcraft's gate is `test/check.sh` (Rust/crabgen); Lua smokes run on Cobalt like `host/auth_smoke.lua` (`../wasmcraft/tools/cobalt host/<x>.lua`).
- Reuse, don't reinvent: `demo/cardlock.lua` already implements the signer, `find_disk`/`wait_for_card`/`read_card`/`write_card`, `gen_nonce`, and the door pulse. Lift them.
- Engine binaries are fetched from releases in-game; for **local tests** point at a `web.wasm` built with `web_message` (the wasmcraft worktree's `wasm/web.wasm`).

---

### Task 0: Make the `web_message` engine available locally

The bridge and the kiosk smoke test need a `web.wasm` that exports `web_message`.

**Step 1:** Confirm the wasmcraft change is built (its plan Task 2 done):
```bash
ls -l ../../../wasmcraft/.worktrees/web-message-channel/wasm/web.wasm
```
**Step 2:** Stage a copy for crabcraft tests + the browser helpers:
```bash
mkdir -p test/vendor
cp ../../../wasmcraft/.worktrees/web-message-channel/wasm/web.wasm                test/vendor/web.wasm
cp ../../../wasmcraft/.worktrees/web-message-channel/dist/browser.lua             test/vendor/browser.lua
cp ../../../wasmcraft/.worktrees/web-message-channel/dist/webrender.lua           test/vendor/webrender.lua
cp ../../../wasmcraft/.worktrees/web-message-channel/dist/wasmcraft.lua           test/vendor/wasmcraft.lua
```
**Step 3:** Ignore the vendored binaries (regenerable, not crabcraft source):
```bash
echo "test/vendor/" >> .gitignore
git add .gitignore && git commit -m "chore: ignore test/vendor (staged wasmcraft engine for cardui tests)"
```
Expected: `test/vendor/web.wasm` exports `web_message` (verified indirectly by Task 3).

---

### Task 1: The React app — `CardApp.jsx` + `main.jsx` + `cardui.html`

**Files:**
- Create: `web/cardui/CardApp.jsx`
- Create: `web/cardui/main.jsx` (reconciler host-config + hooks + mount; adapted from wasmcraft `web/app/main.jsx`)
- Create: `web/cardui/cardui.html`
- Create: `tools/build-cardui` (esbuild bundler; mirrors wasmcraft `tools/build-web`)

**Step 1: `web/cardui/CardApp.jsx`** — the state machine (screens per the design):
```jsx
import React, { useState, useEffect } from "react";

const send = (cmd) => console.log("\x01CRB " + JSON.stringify(cmd));
const btn = (bg) => ({ background: bg, color: "white" });

export function CardApp() {
  const [s, setS] = useState({ screen: "locked", note: "" });
  useEffect(() => { globalThis.__registerHostMsg((m) => {
    if (m.ev === "status")   setS((p) => ({ ...p, note: m.text }));
    else if (m.ev === "granted")  setS({ screen: "unlocked", user: m.username, role: m.role });
    else if (m.ev === "denied")   setS({ screen: "denied", note: m.reason });
    else if (m.ev === "enrolled") setS({ screen: "enrolled", user: m.username, id: m.user_id });
    else if (m.ev === "locked")   setS({ screen: "locked", note: "" });
  }); }, []);

  switch (s.screen) {
    case "awaitcard": return Prompt("Tap your card", s.note);
    case "enrolling": return Prompt("Enrolling…", s.note);
    case "unlocked": return (
      <div>
        <h1 style={{ color: "lime", textAlign: "center" }}>UNLOCKED</h1>
        <p>Welcome, <strong style={{ color: "cyan" }}>{s.user}</strong>{s.role ? " (" + s.role + ")" : ""}</p>
        <button onClick={() => send({ op: "opendoor" })} style={btn("green")}>[ Open Door ]</button>
        <button onClick={() => { send({ op: "lock" }); }} style={btn("gray")}>[ Lock ]</button>
      </div>);
    case "denied": return (
      <div>
        <h1 style={{ color: "red", textAlign: "center" }}>DENIED</h1>
        <p style={{ color: "red" }}>{s.note}</p>
        <button onClick={() => send({ op: "lock" })} style={btn("gray")}>[ Back ]</button>
      </div>);
    case "enrolled": return (
      <div>
        <h1 style={{ color: "lime", textAlign: "center" }}>ENROLLED</h1>
        <p>{s.user} — user_id <strong style={{ color: "yellow" }}>{s.id}</strong></p>
        <p style={{ color: "gray" }}>Card written. Keep the floppy safe.</p>
        <button onClick={() => send({ op: "lock" })} style={btn("gray")}>[ Back ]</button>
      </div>);
    default: return ( // locked
      <div>
        <h1 style={{ color: "white", textAlign: "center" }}>🔒 LOCKED</h1>
        <button onClick={() => { send({ op: "signin" }); setS({ screen: "awaitcard", note: "" }); }} style={btn("blue")}>[ Sign in ]</button>
        <button onClick={() => { send({ op: "signup" }); setS({ screen: "enrolling", note: "" }); }} style={btn("green")}>[ Sign up ]</button>
      </div>);
  }
}
function Prompt(title, note) {
  return (
    <div>
      <h1 style={{ color: "yellow", textAlign: "center" }}>{title}</h1>
      <p style={{ color: "gray" }}>{note || "…"}</p>
      <button onClick={() => { (0, eval)('1'); require; }} style={btn("gray")}>[ Cancel ]</button>
    </div>);
}
```
> Note: replace the placeholder `Cancel` onClick with `() => { const {send}=arguments; }` — actually keep it simple: make `Prompt` a component receiving `onCancel`; wire `onClick={onCancel}` and pass `() => { send({op:"cancel"}); setS({screen:"locked"}); }`. (Implement Prompt as a proper component; the inline above is shorthand for the plan.)

**Step 2: `web/cardui/main.jsx`** — copy wasmcraft `web/app/main.jsx` verbatim, then: (a) replace the `import { Counter }` with `import { CardApp } from "./CardApp.jsx"` and mount `CardApp`; (b) append the `__registerHostMsg`/`__wasmcraft_message` hook block from the wasmcraft plan Task 3.

**Step 3: `web/cardui/cardui.html`**:
```html
<body><div id="root"></div><script src="cardapp.js"></script></body>
```

**Step 4: `tools/build-cardui`** — copy wasmcraft `tools/build-web`, change defaults to `ENTRY=web/cardui/main.jsx`, `OUT=web/site/cardapp.js` (create `web/site/`), keep the react@18 + react-reconciler@0.29 + esbuild install.

**Step 5: Build the bundle**
Run: `nix-shell --run "tools/build-cardui"`
Expected: writes `web/site/cardapp.js`; `grep -c CardApp web/site/cardapp.js` ≥ 1.

**Step 6: Commit**
```bash
git add web/cardui tools/build-cardui web/site/cardapp.js
git commit -m "feat(cardui): React sign-in/up gate app + bundler"
```

---

### Task 2: The kiosk program — `demo/cardui.lua`

Fuses the browser render loop (`start_engine`/`wstr`/`web_init`/`web_event` from wasmcraft `dist/browser.lua`) with cardlock's auth. Owns one `os.pullEvent` loop.

**Files:**
- Create: `demo/cardui.lua`

**Step 1:** Write `demo/cardui.lua`. Structure (full helpers lifted from `demo/cardlock.lua`):

```lua
-- cardui: a React monitor gated on public-key cardlock auth. One computer with a
-- monitor + disk drive + wireless modem renders web/cardui via the wasmcraft web
-- engine; taps drive sign-in/sign-up over a console.log/web_message bridge.
--   cardui init     -- create the users table (once)
--   cardui          -- kiosk mode
local ENGINE_URL = "https://github.com/r33drichards/wasmcraft/releases/latest/download/web.wasm"
local BROWSER_URL= "https://github.com/r33drichards/wasmcraft/releases/latest/download/browser.lua"
local RENDER_URL = "https://github.com/r33drichards/wasmcraft/releases/latest/download/webrender.lua"
local PAGE_URL   = "https://github.com/r33drichards/crabcraft/releases/latest/download/cardui.html"
local APPJS_URL  = "https://github.com/r33drichards/crabcraft/releases/latest/download/cardapp.js"
local LIBURL     = "https://github.com/r33drichards/crabcraft/releases/latest/download/cardlib.lua"
local AUTHWASM_URL = "https://github.com/r33drichards/crabcraft/releases/latest/download/auth.wasm"

local STORE, AUTH = "sqlite", "auth"
local CARDFILE, DOOR_SIDE = "card.json", "back"
local A = "crab:auth/accounts@0.1.0#"

-- (fetch helpers, cardlib load, client connect, make_signer, find_disk,
--  wait_for_card, read_card, write_card, gen_nonce — COPY VERBATIM from
--  demo/cardlock.lua; they are unchanged.)

-- ---- web engine: load web.wasm + render CardApp on the monitor -------------
-- Adapt dist/browser.lua's start_engine/wstr/render. errfn parses \x01CRB lines.
local cmds = {}
local function errfn(line)
  local body = line:match("^\1CRB (.+)$")
  if body then cmds[#cmds + 1] = lib.json.decode(body) end
end
local function web_message(inst, tbl)
  local p = wstr(inst, lib.json.encode(tbl)); inst:call("web_message", p); inst:call("web_free", p)
end
local function web_event_click(inst, x, y)
  cmds = {}
  local tp = wstr(inst, "click"); inst:call("web_event", tp, x - 1, y - 1); inst:call("web_free", tp)
  return cmds            -- commands React emitted during this tap
end

-- ---- subcommands ----------------------------------------------------------
local cmd = ({ ... })[1]
if cmd == "init" then
  local r = auth["init"]({ store = STORE })
  if r.is_err then error("init failed: " .. tostring(r.err), 0) end
  print("users table ready on '" .. STORE .. "'"); return
end

-- kiosk mode: render + event loop
-- pending: nil | "verify" | { kind="writecard", cred=… }
local inst = start_web_engine()    -- loads web.wasm, web_init("cardui.html"), paints
local sign = make_signer()         -- local auth.wasm signer
local pending = nil

local function do_verify(side)
  local card = read_card(side)
  if not (card and card.user_id and card.private_key) then
    web_message(inst, { ev = "denied", reason = "unrecognized card" }); pending = nil; return
  end
  local nonce = gen_nonce()
  local ok, sig = pcall(sign, card.private_key, nonce)
  if not ok then web_message(inst, { ev = "denied", reason = "sign error" }); pending = nil; return end
  local r = auth["verify"]({ store = STORE, ["user-id"] = card.user_id, nonce = nonce, signature = sig })
  if r.is_err then web_message(inst, { ev = "denied", reason = tostring(r.err) })
  else
    local acct = lib.json.decode(r.ok)
    web_message(inst, { ev = "granted", username = acct.username, role = (acct.meta and acct.meta.role) })
    if rs and rs.setOutput then rs.setOutput(DOOR_SIDE, true); sleep(2); rs.setOutput(DOOR_SIDE, false) end
  end
  pending = nil
end

local function handle(c)
  if c.op == "signin" then pending = "verify"; web_message(inst, { ev = "status", text = "tap your card…" })
  elseif c.op == "signup" then
    web.setCursorBlink and nil
    write("username: "); local name = read()           -- terminal entry (v1)
    web_message(inst, { ev = "status", text = "registering…" })
    local r = auth["register"]({ store = STORE, username = name, meta = "{}" })
    if r.is_err then web_message(inst, { ev = "denied", reason = tostring(r.err) }); return end
    pending = { kind = "writecard", cred = lib.json.decode(r.ok) }
    web_message(inst, { ev = "status", text = "insert a BLANK floppy…" })
  elseif c.op == "opendoor" then
    if rs and rs.setOutput then rs.setOutput(DOOR_SIDE, true); sleep(2); rs.setOutput(DOOR_SIDE, false) end
  elseif c.op == "lock" or c.op == "cancel" then pending = nil
  end
end

while true do
  local ev = { os.pullEvent() }
  local e = ev[1]
  if e == "monitor_touch" or e == "mouse_click" then
    for _, c in ipairs(web_event_click(inst, ev[3], ev[4])) do handle(c) end
  elseif e == "disk" then
    local side = find_disk()
    if side and pending == "verify" then do_verify(side)
    elseif side and type(pending) == "table" and pending.kind == "writecard" then
      write_card(side, { user_id = pending.cred.user_id, private_key = pending.cred.private_key })
      pcall(disk.setLabel, side, "card")
      web_message(inst, { ev = "enrolled", user_id = pending.cred.user_id, username = "card" }); pending = nil
    end
  elseif (e == "char" and ev[2] == "q") or e == "terminate" then break end
end
```
> The `start_web_engine`/`wstr`/`render` bodies are adapted from `dist/browser.lua` (Task 0 vendored it). Keep stdout→draw-protocol and stderr→`errfn` strictly separate, exactly as `browser.lua` does. `auth` is the cardlib client proxy `C:workload(AUTH)` from cardlock.

**Step 2: Commit**
```bash
git add demo/cardui.lua
git commit -m "feat(cardui): kiosk program — web engine + auth + bridge loop"
```

---

### Task 3: Headless integration smoke — `host/cardui_smoke.lua`

Drive the real engine + a **fake mesh client** + synthetic taps/disk; assert the bridge produces the right frames. Mirrors `host/auth_smoke.lua`.

**Files:**
- Create: `host/cardui_smoke.lua`

**Step 1:** Write a harness that:
1. Loads `test/vendor/web.wasm` (has `web_message`) with `wasi.make` capturing stdout (frames) and stderr (`\x01CRB` commands), root mounted at a temp dir containing `cardui.html` + `cardapp.js` (built in Task 1).
2. `web_init("cardui.html", 51)` → assert frame contains "LOCKED".
3. Find the `[ Sign in ]` button row from the draw protocol (like `web_event_test.lua` finds `increment`), `web_event("click", …)` it → assert a `{op:"signin"}` command was captured on stderr.
4. Simulate the host: call `web_message('{"ev":"status","text":"tap your card…"}')` → frame shows the prompt; then `web_message('{"ev":"granted","username":"alice","role":"admin"}')` → frame shows "Welcome, alice".
5. `web_message('{"ev":"denied","reason":"access denied"}')` path → frame shows "DENIED".
6. `web_message('{"ev":"enrolled","user_id":"3f9a","username":"alice"}')` → frame shows "3f9a".

Use `T` from a small local assert helper (or copy `auth_smoke.lua`'s `check`/`passed`/`failed` pattern; print `ALL_PASS`/`FAILED`).

**Step 2: Run it**
Run: `nix-shell --run "../wasmcraft/.worktrees/web-message-channel/tools/cobalt host/cardui_smoke.lua jit"`
Expected: each `ok` line, final `ALL_PASS`. (React first render is slow; allow time. If too slow for CI, gate behind an env flag like `auth_smoke` does and document it.)

**Step 3: Commit**
```bash
git add host/cardui_smoke.lua
git commit -m "test(cardui): headless bridge smoke (taps + web_message frames)"
```

---

### Task 4: Release wiring + docs

**Files:**
- Modify: `.github/workflows` release amalgamation (add `cardui.lua`, `cardui.html`, `cardapp.js` to the published assets) — confirm the exact workflow first: `cat .github/workflows/*.yml` and find where `amalgamate.py`/asset list lives.
- Modify: `README.md` (add a "Card kiosk (React monitor gate)" subsection with the in-game quickstart from the design doc).
- Create: `manifests/` already has `auth.yml`/`sqlite.yml` — no change.

**Step 1:** Add `demo/cardui.lua`, `web/cardui/cardui.html`, `web/site/cardapp.js` to the release asset set (wherever `cardlock.lua`/`cardlib.lua` are listed).
**Step 2:** Document the station (computer + monitor + disk drive + modem) and the `cardui init` / `cardui` flow in `README.md`.
**Step 3: Commit**
```bash
git add .github README.md
git commit -m "build(cardui): publish cardui assets + README quickstart"
```

---

### Task 5: Full in-game validation (manual)

Integrated station vs. a live gateway/worker with `sqlite` + `auth` deployed:
1. `crb deploy manifests/sqlite.yml && crb deploy manifests/auth.yml`; `cardui init`.
2. `cardui` → monitor shows 🔒 LOCKED.
3. Tap **Sign up** → type a username on the keyboard → insert blank floppy → monitor shows the new `user_id`. Pocket the floppy (the card).
4. Tap **Sign in** → tap the card on the drive → **Welcome, <name>** + door pulses on `back`.
5. Negative: a blank/foreign floppy → **DENIED**.
Capture each screen with `cc-screenshot` for the PR.

---

## Done when

- `web/cardui` app bundles to `cardapp.js`; `demo/cardui.lua` runs the kiosk loop.
- `host/cardui_smoke.lua` is `ALL_PASS` against the `web_message` engine.
- `host/auth_smoke.lua` and `test/check.sh` stay green (no regressions to the auth workload).
- Release publishes `cardui.lua` + `cardui.html` + `cardapp.js`; README documents the station.
- In-game: sign-up writes a card, sign-in unlocks + pulses the door, bad card is denied.
```
