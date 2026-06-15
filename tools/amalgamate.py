#!/usr/bin/env python3
"""Produce dist/: self-contained in-game programs (no separate lib files).

dist/gateway.lua  - standalone control plane (no deps)
dist/worker.lua   - worker + inlined libs + wasmcraft-bundle self-bootstrap
dist/crb.lua      - CLI + inlined libs (client factory, codec, yaml, json)
"""
import json, os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)
WASMCRAFT_URL = "https://github.com/r33drichards/wasmcraft/releases/latest/download/wasmcraft.lua"

def slurp(p):
    with open(p) as f:
        return f.read()

def lua_str(s):
    return json.dumps(s, ensure_ascii=False)

VERSION = slurp("VERSION").strip()

def stamp(src):
    return src.replace('local CRAB_VERSION = "dev"', f'local CRAB_VERSION = "{VERSION}"')

LIBS = {n: slurp(f"host/{n}.lua") for n in
        ["json", "cmval", "schema", "yaml", "runtime", "client"]}

def amalgamate(body, libnames, header):
    parts = [header,
             "local preload, loaded = {}, {}",
             "local function require(n)",
             "  if loaded[n] ~= nil then return loaded[n] end",
             "  local f = preload[n] or error('module not bundled: '..n)",
             "  local m = f(); if m == nil then m = true end",
             "  loaded[n] = m; return m",
             "end"]
    for name in libnames:
        parts.append(f"preload[{lua_str(name)}] = function(...)")
        parts.append(LIBS[name])
        parts.append("end")
    parts.append(body)
    return "\n".join(parts)

BOOTSTRAP = f"""-- self-bootstrap: the wasm engine (wasmcraft bundle) is fetched on first run
do
  local function exists(p)
    if type(fs) == "table" then return fs.exists(p) end
    local f = io.open(p, "r"); if f then f:close(); return true end
    return false
  end
  if type(fs) == "table" and fs.open and not exists("wasmcraft") then
    io.write("fetching wasmcraft engine ... ")
    local r = assert(http.get({lua_str(WASMCRAFT_URL)}, nil, true), "engine fetch failed")
    local h = fs.open("wasmcraft", "wb"); h.write(r.readAll()); h.close(); r.close()
    print("ok")
  end
end
"""

os.makedirs("dist", exist_ok=True)

# gateway: standalone already
with open("dist/gateway.lua", "w") as f:
    f.write("-- crabcraft gateway (amalgamated; see host/gateway.lua)\n" + stamp(slurp("host/gateway.lua")))

# worker: libs + engine bootstrap
with open("dist/worker.lua", "w") as f:
    f.write(amalgamate(stamp(slurp("host/worker.lua")),
                       ["json", "cmval", "schema", "runtime"],
                       "-- crabcraft worker (amalgamated; see host/)\n" + BOOTSTRAP))

# crb: libs only (no engine needed on the client)
with open("dist/crb.lua", "w") as f:
    f.write(amalgamate(slurp("host/crb.lua"),
                       ["json", "cmval", "schema", "yaml", "client"],
                       "-- crb: the crabcraft CLI (amalgamated; see host/)\n"))

# crblib: the client runtime as a requireable module (generated clients
# bootstrap this from the release)
with open("dist/crblib.lua", "w") as f:
    f.write(amalgamate(
        'return { client = require("client"), json = require("json"), '
        'cmval = require("cmval"), schema = require("schema"), yaml = require("yaml") }',
        ["json", "cmval", "schema", "yaml", "client"],
        "-- crblib: crabcraft client runtime (amalgamated; see host/)\n"))

# cardlib: client + LOCAL wasm runtime, for the card-reader turtle. It verifies
# over the mesh (client) AND signs the login challenge locally (runtime), so the
# floppy's private key never leaves the turtle. Bundles the engine bootstrap.
with open("dist/cardlib.lua", "w") as f:
    f.write(amalgamate(
        'return { client = require("client"), runtime = require("runtime"), '
        'json = require("json"), cmval = require("cmval"), schema = require("schema") }',
        ["json", "cmval", "schema", "runtime", "client"],
        "-- cardlib: crabcraft card-reader runtime (amalgamated; see host/)\n" + BOOTSTRAP))

for p in ["dist/gateway.lua", "dist/worker.lua", "dist/crb.lua", "dist/crblib.lua",
          "dist/cardlib.lua"]:
    print(p, os.path.getsize(p), "bytes")
