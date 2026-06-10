#!/usr/bin/env python3
"""crabcraft e2e in the local CraftOS-PC simulator (~/craftos2 craftos-mcp).

Boots a cluster: gateway + 2 workers (1 dir-slot each) + client. The client
deploys workloads from YAML manifests (wasm preloaded on the workers as
file: urls), waits for the control loop to place them, then invokes through
the gateway via the schema-driven client factory. Asserts the replies.

Usage: python3 test/e2e_sim.py            (from the crabcraft repo root)
Requires: ~/craftos2/mcp/target/release/craftos-mcp built, ~/craftos2-rom.
"""
import base64, json, os, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
os.chdir(ROOT)

def slurp(p, binary=False):
    with open(p, "rb" if binary else "r") as f:
        return f.read()

def lua_str(s):
    return json.dumps(s, ensure_ascii=False)

# ---- files every node may need ------------------------------------------------
HOST_LIBS = {n: slurp(f"host/{n}.lua") for n in
             ["json", "cmval", "schema", "yaml", "runtime", "client", "worker", "gateway"]}
BUNDLE = slurp(os.path.expanduser("~/wasmcraft/dist/wasmcraft.lua"))
HELLO_WASM = slurp("modules/hello.wasm", binary=True)
HELLO_SCHEMA = slurp("wit/hello.json")
CALLER_WASM = slurp("modules/caller.wasm", binary=True) if os.path.exists("modules/caller.wasm") else None
CALLER_SCHEMA = slurp("wit/caller.json") if os.path.exists("wit/caller.json") else None
GO_WASM = slurp("modules/hello-go.wasm", binary=True) if os.path.exists("modules/hello-go.wasm") else None
GO_SCHEMA = slurp("wit/hello-go.json") if os.path.exists("wit/hello-go.json") else None
JS_WASM = slurp("modules/hello-js.wasm", binary=True) if os.path.exists("modules/hello-js.wasm") else None

B64 = '''
local function b64dec(data)
  local b='ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/'
  data = data:gsub('[^'..b..'=]', '')
  return (data:gsub('.', function(x)
    if x == '=' then return '' end
    local r,f='',(b:find(x)-1)
    for i=6,1,-1 do r=r..(f%2^i-f%2^(i-1)>0 and '1' or '0') end
    return r
  end):gsub('%d%d%d?%d?%d?%d?%d?%d?', function(x)
    if #x ~= 8 then return '' end
    local c=0
    for i=1,8 do c=c+(x:sub(i,i)=='1' and 2^(8-i) or 0) end
    return string.char(c)
  end))
end
'''

def write_files_lua(files, binaries):
    out = [B64]
    for name, content in files.items():
        out.append(f"do local h = fs.open({lua_str(name)}, 'wb') h.write({lua_str(content)}) h.close() end")
    for name, content in binaries.items():
        b64 = base64.b64encode(content).decode()
        out.append(f"do local h = fs.open({lua_str(name)}, 'wb') h.write(b64dec({lua_str(b64)})) h.close() end")
    return "\n".join(out)


PRELUDE = """
do
  local oldprint = print
  print = function(...)
    local p = {}
    for i = 1, select('#', ...) do p[i] = tostring(select(i, ...)) end
    pcall(emit, table.concat(p, ' '))
    oldprint(...)
  end
end
"""

def amalgamate(body, libs):
    """Inline lib sources with a local require shim; libs = {name: source}."""
    parts = [PRELUDE, "local preload, loaded = {}, {}",
             "local function require(n)",
             "  if loaded[n] ~= nil then return loaded[n] end",
             "  local f = preload[n] or error('module not bundled: '..n)",
             "  local m = f(); if m == nil then m = true end",
             "  loaded[n] = m; return m",
             "end"]
    for name, src in libs.items():
        parts.append(f"preload[{lua_str(name)}] = function(...)")
        parts.append(src)
        parts.append("end")
    parts.append(body)
    return "\n".join(parts)

# ---- node programs --------------------------------------------------------------
gateway_prog = f"""periphemu.create('back','modem',NET,true)
rednet.open('back')
os.setComputerLabel('gw')
do local h = fs.open('gateway', 'w') h.write({lua_str(PRELUDE + HOST_LIBS["gateway"])}) h.close() end
emit('gateway: starting')
local fn, lerr = loadfile('gateway')
if not fn then emit('gateway PARSE: ' .. tostring(lerr)) return end
if setfenv then setfenv(fn, getfenv(1)) end
local ok, err = pcall(fn)
emit('gateway EXITED: ' .. tostring(ok) .. ' ' .. tostring(err))
"""

def worker_prog(label):
    worker_amalg = amalgamate(HOST_LIBS["worker"],
        {n: HOST_LIBS[n] for n in ["json", "cmval", "schema", "runtime"]})
    files = {"worker": worker_amalg, "wasmcraft": BUNDLE}
    bins = {"hello.wasm": HELLO_WASM}
    if CALLER_WASM and label == "w2":
        bins["caller.wasm"] = CALLER_WASM
    if GO_WASM and label == "w1":
        bins["hello-go.wasm"] = GO_WASM
    if JS_WASM and label == "w2":
        bins["hello-js.wasm"] = JS_WASM
    return f"""periphemu.create('back','modem',NET,true)
rednet.open('back')
os.setComputerLabel('{label}')
{write_files_lua(files, bins)}
sleep(2)
emit('{label}: starting')
local fn, lerr = loadfile('worker')
if not fn then emit('{label} PARSE: ' .. tostring(lerr)) return end
if setfenv then setfenv(fn, getfenv(1)) end
local ok, err = pcall(fn, 'gw', '--slots', '2')
emit('{label} EXITED: ' .. tostring(ok) .. ' ' .. tostring(err))
"""

caller_test = ""
if CALLER_WASM:
    caller_test = f"""
-- deploy the mesh caller to the second slot
do local h = fs.open('caller.yml','w') h.write("name: caller\\nwasm: file:caller.wasm\\nkind: reactor\\nschema: caller.json\\n") h.close() end
do local h = fs.open('caller.json','w') h.write({lua_str(CALLER_SCHEMA)}) h.close() end
r = C:deploy({{ name = 'caller', wasm = 'file:caller.wasm', kind = 'reactor',
  schema = {lua_str(CALLER_SCHEMA)} }})
emit('deploy caller: ' .. tostring(r.ok))
wait_running('caller')
-- THE MESH TEST: caller (one worker) calls hello (other worker) through the gateway
local relay = C:workload('caller')
local viamesh = relay['greet-via']({{ target = 'hello', name = 'mesh' }})
emit('mesh: ' .. tostring(viamesh))
"""

go_test = ""
if GO_WASM:
    go_test = f"""
-- ANY-LANGUAGE, SAME ABI: the TinyGo reactor (file only on w1)
r = C:deploy({{ name = 'hello-go', wasm = 'file:hello-go.wasm', kind = 'reactor',
  schema = {lua_str(GO_SCHEMA)} }})
emit('deploy hello-go: ' .. tostring(r.ok))
wait_running('hello-go')
local hgo = C:workload('hello-go')
emit('go greet: ' .. tostring(hgo.greet({{ name = 'gopher', excited = true }})))
"""

js_test = ""
if JS_WASM:
    js_test = f"""
-- COMMAND KIND: Javy-compiled JS, JSON in -> JSON out (file only on w2)
r = C:deploy({{ name = 'hello-js', wasm = 'file:hello-js.wasm', kind = 'command' }})
emit('deploy hello-js: ' .. tostring(r.ok))
wait_running('hello-js')
local hjs = C:workload('hello-js')
local jr = hjs({{ fn = 'greet', name = 'quickjs' }})
emit('js greet: ' .. tostring(type(jr) == 'table' and jr.result or jr))
"""

client_test_body = f"""
local __ok, __err = pcall(function()
local client = require('client')
local C = client.connect('gw', {{ attempts = 8 }})
emit('connected to gateway #' .. C.gw)

local function wait_running(name)
  for i = 1, 30 do
    local r = C:list()
    if r.ok then
      for _, w in ipairs(r.workloads or {{}}) do
        if w.name == name and w.state == 'running' then return true end
      end
    end
    sleep(2)
  end
  error('workload never ran: ' .. name)
end

-- declarative deploy (manifest semantics; schema inline like crb does)
local r = C:deploy({{ name = 'hello', wasm = 'file:hello.wasm', kind = 'reactor',
  schema = {lua_str(HELLO_SCHEMA)} }})
emit('deploy hello: ' .. tostring(r.ok))
wait_running('hello')
emit('hello placed + running')

-- THE FACTORY: schema-driven proxy, typed call through gateway -> worker
local hello = C:workload('hello')
emit('greet: ' .. tostring(hello.greet({{ name = 'crab', excited = true }})))
emit('add: ' .. tostring(hello.add({{ a = 40, b = 2 }})))
{caller_test}
{go_test}
{js_test}
-- cluster state for the record
local l = C:list()
for _, w in ipairs(l.workloads or {{}}) do
  emit(('placed: %s on worker %s'):format(w.name, tostring(w.worker)))
end
end)
if not __ok then emit('CLIENT ERROR: ' .. tostring(__err)) end
done()
"""

ctest_amalg = amalgamate(client_test_body,
    {n: HOST_LIBS[n] for n in ["json", "cmval", "schema", "yaml", "client"]})
client_prog = f"""periphemu.create('back','modem',NET,true)
rednet.open('back')
os.setComputerLabel('client')
do local h = fs.open('ctest', 'w') h.write({lua_str(ctest_amalg)}) h.close() end
sleep(4)
emit('client: starting')
local fn, lerr = loadfile('ctest')
if not fn then emit('CTEST PARSE ERROR: ' .. tostring(lerr)) done() return end
if setfenv then setfenv(fn, getfenv(1)) end
local ok, err = pcall(fn)
if not ok then emit('CTEST RUNTIME ERROR: ' .. tostring(err)) end
done()
"""

spec = {
    "timeout_ms": 180000,
    "nodes": [
        {"label": "gateway", "position": [0, 0, 0], "program": gateway_prog},
        {"label": "worker1", "position": [2, 0, 0], "program": worker_prog("w1")},
        {"label": "worker2", "position": [4, 0, 0], "program": worker_prog("w2")},
        {"label": "client", "position": [6, 0, 0], "collect": True, "program": client_prog},
    ],
}

# ---- drive the MCP server over stdio --------------------------------------------
env = dict(os.environ)
env["CRAFTOS_ROM"] = os.path.expanduser("~/craftos2-rom")
env["DYLD_LIBRARY_PATH"] = os.path.expanduser("~/craftos2/craftos2-lua/src")
proc = subprocess.Popen(
    [os.path.expanduser("~/craftos2/mcp/target/release/craftos-mcp")],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
    env=env, text=True, bufsize=1)

def send(o):
    proc.stdin.write(json.dumps(o) + "\n"); proc.stdin.flush()

def recv(want):
    while True:
        line = proc.stdout.readline()
        if not line: sys.exit("craftos-mcp closed unexpectedly")
        try: m = json.loads(line)
        except Exception: continue
        if m.get("id") == want: return m

send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params":
      {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "e2e", "version": "0"}}})
recv(1)
send({"jsonrpc": "2.0", "method": "notifications/initialized"})
send({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params":
      {"name": "run_simulation", "arguments": spec}})
resp = recv(2)
proc.kill()

text = "".join(c.get("text", "") for c in resp.get("result", {}).get("content", []))
print(text)
data = json.loads(text)
out = {n["label"]: n["output"] for n in data["nodes"]}
client_out = out.get("client", "")
print("==== client output ====")
print(client_out)

checks = [
    ("connected", "connected to gateway" in client_out),
    ("hello deployed", "deploy hello: true" in client_out),
    ("hello running", "hello placed + running" in client_out),
    ("greet via mesh routing", "greet: Hello, crab!!!" in client_out),
    ("add via mesh routing", "add: 42" in client_out),
]
if CALLER_WASM:
    checks.append(("cross-module mesh call", "mesh: via mesh: Hello, mesh!" in client_out))
if GO_WASM:
    checks.append(("Go reactor lane", "go greet: Hello from Go, gopher!!!" in client_out))
if JS_WASM:
    checks.append(("JS command lane", "js greet: Hello from JS, quickjs!" in client_out))

failed = [name for name, ok in checks if not ok]
for name, ok in checks:
    print(("PASS " if ok else "FAIL ") + name)
if failed:
    sys.exit("E2E FAILED: " + ", ".join(failed))
print("E2E ALL PASS")
