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

# ---- node programs --------------------------------------------------------------
gateway_prog = f"""periphemu.create('back','modem',NET,true)
rednet.open('back')
os.setComputerLabel('gw')
{write_files_lua({"gateway": HOST_LIBS["gateway"]}, {})}
emit('gateway: starting')
shell.run('gateway')
"""

def worker_prog(label):
    files = {f"{n}.lua": HOST_LIBS[n] for n in ["json", "cmval", "schema", "runtime"]}
    files["worker"] = HOST_LIBS["worker"]
    files["wasmcraft"] = BUNDLE
    bins = {"hello.wasm": HELLO_WASM}
    if CALLER_WASM:
        bins["caller.wasm"] = CALLER_WASM
    return f"""periphemu.create('back','modem',NET,true)
rednet.open('back')
os.setComputerLabel('{label}')
{write_files_lua(files, bins)}
sleep(2)
emit('{label}: starting')
shell.run('worker', 'gw', '--slots', '1')
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
-- cluster state for the record
local l = C:list()
for _, w in ipairs(l.workloads or {{}}) do
  emit(('placed: %s on worker %s'):format(w.name, tostring(w.worker)))
end
end)
if not __ok then emit('CLIENT ERROR: ' .. tostring(__err)) end
done()
"""

client_prog = f"""periphemu.create('back','modem',NET,true)
rednet.open('back')
os.setComputerLabel('client')
{write_files_lua({f"{n}.lua": HOST_LIBS[n] for n in ["json", "cmval", "schema", "yaml", "client"]}, {})}
do local h = fs.open('ctest', 'w') h.write({lua_str(client_test_body)}) h.close() end
sleep(4)
emit('client: starting')
shell.run('ctest')
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

failed = [name for name, ok in checks if not ok]
for name, ok in checks:
    print(("PASS " if ok else "FAIL ") + name)
if failed:
    sys.exit("E2E FAILED: " + ", ".join(failed))
print("E2E ALL PASS")
