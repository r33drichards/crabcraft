# Design: cardui — a React monitor UI gated on cardlock auth

Date: 2026-06-14
Repo: crabcraft (app half)
Companion design: wasmcraft `docs/plans/2026-06-14-web-message-host-channel-design.md`

## Summary

An integrated kiosk station — one CC computer with a **monitor**, a **disk
drive**, and a **wireless modem** — renders a real React UI via the wasmcraft web
engine. The UI is a public-key auth gate on top of the existing `auth` workload:

- **Locked** screen with `[ Sign in ]` / `[ Sign up ]`.
- **Sign up** runs the enrollment flow (register over the mesh → write a fresh
  Ed25519 private key to a blank floppy = the card) and shows the new `user_id`.
- **Sign in** asks for a card; the station signs a fresh nonce **locally** with
  the floppy's key (so the key never leaves the machine), verifies it over the
  mesh against the stored public key, and on success flips React to **Unlocked**
  (Welcome + `[ Open Door ]`) and pulses redstone.

This reuses the auth security model unchanged (private key only on the floppy,
challenge/response verify); it adds a React presentation + interaction layer.

## Architecture

```
            ┌──────────────────────── cardui.lua (one Lua program) ───────────────────────┐
 monitor ◄──┤  web.wasm  (render)          taps ──► web_event("click")                      │
   taps  ──►│  auth.wasm (local signer)    host ──► web_message({...}) ──► React setState     │
 drive   ──►│  cardlib client (mesh)       React ─► console.log("\x01CRB {...}") ─► host cmds  │
 modem   ──►│  redstone "back" (door)                                                          │
            └──────────────────────────────────────────────────────────────────────────────┘
                       │ rednet                          │ mesh: register / verify
                       ▼                                  ▼
                   gateway ───────────────────────► auth workload ──► sqlite workload
```

One computer, **one event loop**, **two wasm reactors** (`web.wasm` for render,
`auth.wasm` for local signing) plus the cardlib mesh client (for `register` /
`verify`, which run on the `auth` workload over rednet and touch `sqlite`).

## The bridge protocol

Depends on the wasmcraft `web_message` export (companion design). Two channels:

### React → host (commands), over `console.log`

The engine routes `console.log` to the host's stderr callback (`errfn`), kept
separate from the draw protocol. React emits a sentinel-prefixed JSON line:

```js
const send = (cmd) => console.log("\x01CRB " + JSON.stringify(cmd));
// {op:"signin"} {op:"signup"} {op:"opendoor"} {op:"lock"} {op:"cancel"}
```

Because the `console.log` fires *synchronously inside* the `onClick` (during the
`web_event` call), the host has the command the instant `web_event` returns.

### Host → React (results), over `web_message(json)`

After doing the privileged work, the host pushes one JSON result:

```lua
msg{ ev="status",   text="Tap your card…" }
msg{ ev="granted",  username="alice", role="admin" }     -- + door pulse
msg{ ev="denied",   reason="access denied" }
msg{ ev="enrolled", user_id="3f9a…", username="alice" }
```

React's registered `__registerHostMsg` handler switches on `ev` and `setState`.

## The React app (`web/cardui/CardApp.jsx`)

A single state machine; screens are styled block-level `div`/`button` trees
(block buttons so hit-testing lands; 16 CC colors). States:

| screen      | shows                                  | from                          |
|-------------|----------------------------------------|-------------------------------|
| `locked`    | 🔒 LOCKED, `[Sign in]` `[Sign up]`     | initial; back from any        |
| `awaitcard` | "Tap your card…" + status note, Cancel | tap Sign in                   |
| `enrolling` | "Enrolling…" + status note, Cancel     | tap Sign up                   |
| `unlocked`  | Welcome <user> (<role>), `[Open Door]` `[Lock]` | host `granted`       |
| `denied`    | DENIED + reason, Back                  | host `denied`                 |
| `enrolled`  | new `user_id`, "write your card", Back | host `enrolled`               |

Mounted with the same custom react-reconciler as `Counter.jsx`; registers its
host-message handler via `globalThis.__registerHostMsg(setFromHost)`.

## The kiosk program (`demo/cardui.lua`)

Fuses `browser.lua`'s render substrate with `cardlock.lua`'s auth. Owns one
event loop:

```
load   web.wasm  (start_engine, from wasmcraft release/disk)
       auth.wasm (cardlib runtime.load_reactor — local signer)
       cardlib   client.connect()  (mesh: auth workload)
render frame 1 (cardui.html → CardApp, "locked")

loop os.pullEvent():
  monitor_touch → cmds={}; web_event("click",x,y)   -- errfn collects \x01CRB lines
                  for cmd in cmds: handle(cmd)
  disk          → if pending step, advance it (card present)
  char "q"/terminate → quit

handle {op=signin}  → pending="verify"; msg(status "Tap your card…")
handle {op=signup}  → name = prompt on terminal (read());  r = client:register(name)
                      msg(status "Insert a BLANK floppy…"); pending="writecard"; stash cred
handle {op=opendoor}→ rs.setOutput("back",true); sleep(2); rs.setOutput("back",false)
handle {op=lock}    → pending=nil; (CardApp already set locked)
handle {op=cancel}  → pending=nil

disk & pending=verify   → card=read_floppy; nonce=gen_nonce()
                          sig = sign(card.private_key, nonce)        -- LOCAL auth.wasm
                          acct = client:verify(card.user_id,nonce,sig)
                          ok → msg(granted username,role) + door pulse
                          err→ msg(denied reason)
disk & pending=writecard→ write_card(side, {user_id, private_key})
                          msg(enrolled user_id, username); pending=nil
```

The signer, nonce gen, floppy read/write, door pulse, and re-lock-on-eject are
lifted from `demo/cardlock.lua` (single source of the auth mechanics).

`errfn` parser: lines starting `\x01CRB ` → decode JSON command; all other
`console.log` output is ignored (or echoed to a debug log).

**Username entry (v1):** the monitor delivers only taps, so sign-up prompts for
the username via the computer keyboard (`read()`), monitor shows "→ type the
username on the keyboard." An on-screen tap-keyboard in React is a future option
(YAGNI for v1).

## Repos, build & assets

| Piece                                    | Repo      |
|------------------------------------------|-----------|
| `web_message` export + `main.jsx` hook + rebuilt `web.wasm` + release | wasmcraft |
| `web/cardui/CardApp.jsx` + `tools/build-cardui` (esbuild) → `cardapp.js` | crabcraft |
| `demo/cardui.lua` (kiosk loop)           | crabcraft |
| `cardui.html` (mounts `#root`, loads `cardapp.js`) | crabcraft |
| `manifests/auth.yml` (exists), `manifests/sqlite.yml` (exists) | crabcraft |

`cardui.lua` fetches `web.wasm` + `browser`/`webrender` helpers from the
**wasmcraft** release and `auth.wasm` + `cardlib` from the **crabcraft** release.
`cardapp.js` and `cardui.html` ship as crabcraft release assets.

Build order: **wasmcraft first** (engine change → `build-fixtures` → release),
then crabcraft (`build-cardui` bundle → release).

## E2E testing (cheapest first)

1. **Engine unit (wasmcraft):** `test/web_message_test.lua` — companion design.
2. **Auth logic (crabcraft):** existing `host/auth_smoke.lua` — register→sign→
   verify on the real engine with a fake sqlite. Unchanged; still the crypto gate.
3. **Kiosk integration (crabcraft):** new `host/cardui_smoke.lua` — feed
   synthetic `monitor_touch` taps + a fake card disk into `cardui.lua`'s loop
   with a fake mesh client; assert the command/`web_message` handshake yields
   `granted` / `denied` / `enrolled` frames. Headless (Cobalt / `cc-screenshot`).
4. **Full in-game:** integrated station vs. a live gateway/worker with `sqlite`
   + `auth` deployed. `[Sign up]` → type name → write floppy → `[Sign in]` →
   tap card → Welcome + door pulse. Negative: blank floppy → DENIED.

`cc-screenshot` produces a color HTML capture of each screen for review.

## Deploy (in-game quickstart, once released)

```
-- gateway + worker(s) up; deploy the workloads:
crb deploy manifests/sqlite.yml
crb deploy manifests/auth.yml

-- on the kiosk station (computer + monitor + disk drive + modem):
wget https://github.com/r33drichards/crabcraft/releases/latest/download/cardui.lua cardui
cardui init        -- create users table (once)
cardui             -- kiosk mode: locked screen on the monitor
```

## Out of scope (YAGNI for v1)

- On-screen tap-keyboard for username entry (terminal `read()` for now).
- Role-aware admin panels / user lists on the monitor (verify returns role; the
  unlocked screen can branch later).
- Multi-card / session timeout policies beyond re-lock-on-eject + `[Lock]`.
