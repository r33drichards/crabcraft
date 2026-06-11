#!/usr/bin/env python3
"""crabgen end-to-end: the full developer workflow through the local simulator.

Per language lane (default: go; later tasks append rust/cpp/ts):

  Case A (scaffold):    crabgen new e2e-<lane> -> overwrite the starter WIT ->
                        regen -> write the 5-line impl -> build.sh -> deploy in
                        the sim -> invoke greet -> assert the reply.
  Case B (maintenance): edit the WIT (new func `shout`, new optional record
                        field `loud`) -> `crabgen check` FAILS naming the
                        project -> `crabgen regen` prints the missing Shout
                        signature -> add the impl -> rebuild -> `crabgen check`
                        passes -> redeploy -> greet (loud absent, back-compat)
                        AND shout both work in the same deployment.

The temporary project guest/e2e-<lane> and modules/e2e-<lane>.wasm are removed
in a finally block — no repo residue, even on failure.

Usage: python3 test/e2e_crabgen.py [lane ...]     (or LANES=go,rust ...)
Requires the local craftos-mcp sim (built on demand if missing):
  nix develop ~/craftos2 --command bash -c 'cd ~/craftos2/mcp && cargo build --release'
  env: CRAFTOS_ROM=~/craftos2-rom, DYLD_LIBRARY_PATH=~/craftos2/craftos2-lua/src
"""
import json
import os
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import e2e_sim as sim  # noqa: E402  (importing chdirs to the repo root)

ROOT = sim.ROOT
CRABGEN = os.path.join(ROOT, "target", "debug", "crabgen")

results = []  # (label, ok) accumulated across lanes/cases


def log(msg):
    print(f"[e2e_crabgen +{time.monotonic() - T0:6.1f}s] {msg}", flush=True)


def check(label, ok, detail=""):
    results.append((label, bool(ok)))
    print(("PASS " if ok else "FAIL ") + label + (f"  ({detail})" if detail and not ok else ""),
          flush=True)


def run(cmd, cwd=ROOT, expect=0):
    """Run cmd, capture output. expect=None skips the exit-code assertion."""
    p = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    if expect is not None and p.returncode != expect:
        sys.exit(f"command failed (rc={p.returncode}, wanted {expect}): {cmd}\n"
                 f"stdout:\n{p.stdout}\nstderr:\n{p.stderr}")
    return p


# ---- toolchain / sim availability -------------------------------------------------

def build_crabgen():
    log("building crabgen (cargo build -p crabgen via nix-shell)...")
    run(["nix-shell", "-p", "cargo", "rustc", "--run", "cargo build -p crabgen"])
    if not os.path.exists(CRABGEN):
        sys.exit(f"cargo build succeeded but {CRABGEN} is missing")
    log("crabgen built")


def ensure_sim_server():
    rom = os.path.expanduser("~/craftos2-rom")
    howto = ("craftos-mcp sim unavailable. To set it up:\n"
             "  nix develop ~/craftos2 --command bash -c "
             "'cd ~/craftos2/mcp && cargo build --release'\n"
             f"  binary: {sim.MCP_BIN}\n"
             f"  run env: CRAFTOS_ROM={rom} "
             "DYLD_LIBRARY_PATH=~/craftos2/craftos2-lua/src")
    if not os.path.isdir(rom):
        sys.exit(f"missing CraftOS ROM dir {rom}\n{howto}")
    if not os.path.exists(sim.MCP_BIN):
        log("craftos-mcp binary missing; building it once (incremental)...")
        p = subprocess.run(["nix", "develop", os.path.expanduser("~/craftos2"),
                            "--command", "bash", "-c",
                            "cd ~/craftos2/mcp && cargo build --release"])
        if p.returncode != 0 or not os.path.exists(sim.MCP_BIN):
            sys.exit(f"craftos-mcp build failed\n{howto}")
    log("sim server available")


# ---- sim plumbing ------------------------------------------------------------------

def run_cluster(name, wasm_path, client_body):
    """Gateway + 1 worker (preloaded with the lane's wasm) + client running
    `client_body`. Returns the client node's collected emit() output.

    All three nodes collect output: a worker-side failure (e.g. the wasm
    refuses to load) never reaches the client's emits, so on any failed check
    the dump below is the diagnostic — mirror e2e_sim main()'s habit of
    printing the whole sim transcript."""
    with open(wasm_path, "rb") as f:
        wasm = f.read()
    spec = {
        "timeout_ms": 180000,
        "nodes": [
            {"label": "gateway", "position": [0, 0, 0], "collect": True,
             "program": sim.gateway_prog},
            {"label": "worker1", "position": [2, 0, 0], "collect": True,
             "program": sim.worker_prog("w1", {f"{name}.wasm": wasm})},
            {"label": "client", "position": [4, 0, 0], "collect": True,
             "program": sim.client_prog(client_body)},
        ],
    }
    text = sim.run_sim(spec)
    data = json.loads(text)
    out = {n["label"]: n["output"] for n in data["nodes"]}
    for label in ("gateway", "worker1", "client"):
        print(f"==== {label} output ====")
        print(out.get(label, "(no output)"))
    return out.get("client", "")


def client_body(name, schema_json, invokes):
    """The standard deploy-then-invoke client script. `invokes` is Lua emitting
    results from the proxy `W`."""
    return f"""
local __ok, __err = pcall(function()
local client = require('client')
local C = client.connect('gw', {{ attempts = 8 }})
emit('connected to gateway #' .. C.gw)

local function wait_running(n)
  for i = 1, 30 do
    local r = C:list()
    if r.ok then
      for _, w in ipairs(r.workloads or {{}}) do
        if w.name == n and w.state == 'running' then return true end
      end
    end
    sleep(2)
  end
  error('workload never ran: ' .. n)
end

local r = C:deploy({{ name = {sim.lua_str(name)}, wasm = {sim.lua_str("file:" + name + ".wasm")},
  kind = 'reactor', schema = {sim.lua_str(schema_json)} }})
emit('deploy: ' .. tostring(r.ok))
wait_running({sim.lua_str(name)})
local W = C:workload({sim.lua_str(name)})
{invokes}
end)
if not __ok then emit('CLIENT ERROR: ' .. tostring(__err)) end
done()
"""


# ---- go lane -----------------------------------------------------------------------

def go_write_impl(proj, name):
    """The 5-line scaffold-case impl: greet only (mirrors hello-go's logic)."""
    with open(os.path.join(proj, "impl.go"), "w") as f:
        f.write(f"""package main

import (
\t"crabcraft.local/{name}/gen"
)

type App struct{{}}

// greet(req: greet-request) -> string
func (App) Greet(req gen.GreetRequest) (string, error) {{
\tbang := "!"
\tif req.Excited != nil && *req.Excited {{
\t\tbang = "!!!"
\t}}
\treturn "Hello, " + req.Name + bang, nil
}}

func init() {{ gen.SetImpl(App{{}}) }}

func main() {{}}
""")


def go_add_shout(proj, name):
    """What a maintainer does after regen prints the missing Shout signature:
    add the import and append the method."""
    path = os.path.join(proj, "impl.go")
    with open(path) as f:
        src = f.read()
    gen_import = f'\t"crabcraft.local/{name}/gen"'
    if '"strings"' not in src:
        src = src.replace(gen_import, gen_import + '\n\t"strings"', 1)
    src += """
// shout(msg: string) -> string
func (App) Shout(msg string) (string, error) {
\treturn strings.ToUpper(msg) + "!", nil
}
"""
    with open(path, "w") as f:
        f.write(src)


# ---- rust lane ---------------------------------------------------------------------

def rust_trait(name):
    """world <name> -> the generated trait: E2eRustImpl for e2e-rust."""
    return "".join(seg.capitalize() for seg in name.split("-")) + "Impl"


def rust_write_impl(proj, name):
    """The scaffold-case impl: greet only (mirrors guest/hello's app.rs)."""
    trait = rust_trait(name)
    with open(os.path.join(proj, "src", "app.rs"), "w") as f:
        f.write(f"""//! e2e impl for the {name} guest.

use crate::gen::{{self, {trait}}};

pub struct App;

impl {trait} for App {{
    fn greet(&self, req: gen::GreetRequest) -> Result<String, String> {{
        let bang = if req.excited == Some(true) {{ "!!!" }} else {{ "!" }};
        Ok(format!("Hello, {{}}{{bang}}", req.name))
    }}
}}
""")


def rust_add_shout(proj, name):
    """What a maintainer does after regen prints the missing shout signature:
    paste the method inside the impl block (before its closing brace)."""
    path = os.path.join(proj, "src", "app.rs")
    with open(path) as f:
        src = f.read()
    method = '''
    fn shout(&self, msg: String) -> Result<String, String> {
        Ok(msg.to_uppercase() + "!")
    }
'''
    i = src.rfind("}")
    src = src[:i] + method + src[i:]
    with open(path, "w") as f:
        f.write(src)


# The per-lane seam. Behavioral contract every lane must honor:
#   write_impl(proj, name) — greet returns "Hello, <name>" + ("!!!" if excited else "!")
#   add_shout(proj, name)  — shout returns uppercase(msg) + "!"
#   shout_sig              — substring `crabgen regen` must print for the missing shout impl
# Optional: cleanup(proj, name) — invoked in run_lane's finally for lane-specific
# residue outside guest/<name> and modules/<name>.wasm (root Cargo.toml is
# already snapshot/restored for everyone — rust's `new` edits its members).
LANE = {
    "go": {
        "write_impl": go_write_impl,
        "add_shout": go_add_shout,
        "shout_sig": "func (App) Shout(",
    },
    "rust": {
        # `new --lang rust` also edits the root Cargo.toml members — already
        # snapshot/restored lane-agnostically in run_lane, so no cleanup hook.
        "write_impl": rust_write_impl,
        "add_shout": rust_add_shout,
        "shout_sig": "fn shout(&self, msg: String) -> Result<String, String>",
    },
}


# ---- the WIT files -----------------------------------------------------------------

def wit_v1(name):
    return f"""package crab:{name}@0.1.0;

interface api {{
  record greet-request {{
    name: string,
    excited: option<bool>,
  }}

  greet: func(req: greet-request) -> string;
}}

world {name} {{
  export api;
}}
"""


def wit_v2(name):
    return f"""package crab:{name}@0.1.0;

interface api {{
  record greet-request {{
    name: string,
    excited: option<bool>,
    loud: option<bool>,
  }}

  greet: func(req: greet-request) -> string;
  shout: func(msg: string) -> string;
}}

world {name} {{
  export api;
}}
"""


# ---- the two cases -----------------------------------------------------------------

def case_a(lane, name, proj, wasm_out):
    log(f"[{lane}] case A: crabgen new {name} --lang {lane}")
    p = run([CRABGEN, "new", name, "--lang", lane])
    check(f"{lane} A: new scaffolds", f"created guest/{name}" in p.stdout, p.stdout)

    log(f"[{lane}] case A: swap in the e2e WIT + regen")
    with open(os.path.join(proj, f"{name}.wit"), "w") as f:
        f.write(wit_v1(name))
    run([CRABGEN, "regen", f"guest/{name}"])

    log(f"[{lane}] case A: write the impl")
    LANE[lane]["write_impl"](proj, name)

    log(f"[{lane}] case A: build.sh (slow: toolchain via nix)...")
    run(["./build.sh"], cwd=proj)
    check(f"{lane} A: build produced wasm", os.path.exists(wasm_out))

    with open(os.path.join(proj, "gen", "schema.json")) as f:
        schema = f.read()
    log(f"[{lane}] case A: sim deploy + invoke")
    out = run_cluster(name, wasm_out, client_body(name, schema, """
emit('greet: ' .. tostring(W.greet({ name = 'steve', excited = true })))
"""))
    check(f"{lane} A: deployed", "deploy: true" in out)
    check(f"{lane} A: greet via sim", "greet: Hello, steve!!!" in out)


def case_b(lane, name, proj, wasm_out):
    log(f"[{lane}] case B: edit the WIT (add shout + loud field)")
    with open(os.path.join(proj, f"{name}.wit"), "w") as f:
        f.write(wit_v2(name))

    # NOTE: `crabgen check` is repo-wide — these two assertions (fails here,
    # passes later) also depend on every OTHER guest project being fresh. An
    # unrelated stale project fails them spuriously (deliberate: keeps the
    # whole repo honest).
    p = run([CRABGEN, "check"], expect=None)
    check(f"{lane} B: check fails on stale WIT",
          p.returncode != 0 and f"guest/{name}" in p.stderr,
          f"rc={p.returncode} stderr={p.stderr!r}")

    p = run([CRABGEN, "regen", f"guest/{name}"])
    check(f"{lane} B: regen prints missing shout sig",
          LANE[lane]["shout_sig"] in p.stdout, p.stdout)

    log(f"[{lane}] case B: add the shout impl + rebuild")
    LANE[lane]["add_shout"](proj, name)
    run(["./build.sh"], cwd=proj)

    p = run([CRABGEN, "check"], expect=None)
    check(f"{lane} B: check passes after regen+rebuild", p.returncode == 0,
          f"rc={p.returncode} stderr={p.stderr!r}")

    with open(os.path.join(proj, "gen", "schema.json")) as f:
        schema = f.read()
    log(f"[{lane}] case B: sim redeploy + invoke old func (loud absent) + new func")
    out = run_cluster(name, wasm_out, client_body(name, schema, """
-- back-compat: client re-encodes against the NEW schema; loud is absent so the
-- record gains an option-none byte and the old behavior must hold
emit('greet: ' .. tostring(W.greet({ name = 'steve', excited = true })))
emit('shout: ' .. tostring(W.shout({ msg = 'hi' })))
"""))
    check(f"{lane} B: deployed", "deploy: true" in out)
    check(f"{lane} B: old func greet still works (loud absent)",
          "greet: Hello, steve!!!" in out)
    check(f"{lane} B: new func shout works", "shout: HI!" in out)


# ---- driver ------------------------------------------------------------------------

def run_lane(lane):
    if lane not in LANE:
        sys.exit(f"lane '{lane}' not implemented yet (have: {', '.join(LANE)})")
    name = f"e2e-{lane}"
    proj = os.path.join(ROOT, "guest", name)
    wasm_out = os.path.join(ROOT, "modules", f"{name}.wasm")
    if os.path.exists(proj):
        sys.exit(f"{proj} already exists; remove it before running")
    prior_wasm = None
    if os.path.exists(wasm_out):  # shouldn't happen, but restore if it does
        with open(wasm_out, "rb") as f:
            prior_wasm = f.read()
    # `new --lang rust` appends the project crate to the root workspace
    # members, and the lane's cargo builds then record it in Cargo.lock —
    # without restoring both, a run leaves a dangling members entry (breaks
    # every later cargo invocation) or lockfile churn. Snapshot/restore is
    # lane-agnostic and harmless for lanes that never touch them.
    snapshots = {}
    for fname in ("Cargo.toml", "Cargo.lock"):
        path = os.path.join(ROOT, fname)
        with open(path) as f:
            snapshots[path] = f.read()
    try:
        case_a(lane, name, proj, wasm_out)
        case_b(lane, name, proj, wasm_out)
    finally:
        shutil.rmtree(proj, ignore_errors=True)
        if prior_wasm is not None:
            with open(wasm_out, "wb") as f:
                f.write(prior_wasm)
        elif os.path.exists(wasm_out):
            os.remove(wasm_out)
        for path, content in snapshots.items():
            with open(path, "w") as f:
                f.write(content)
        cleanup = LANE[lane].get("cleanup")
        if cleanup:
            cleanup(proj, name)
        log(f"[{lane}] cleaned up guest/{name} and modules/{name}.wasm")


T0 = time.monotonic()


def main():
    lanes = sys.argv[1:] or os.environ.get("LANES", "go").split(",")
    build_crabgen()
    ensure_sim_server()
    for lane in lanes:
        run_lane(lane)
    print("\n==== summary ====")
    failed = [label for label, ok in results if not ok]
    for label, ok in results:
        print(("PASS " if ok else "FAIL ") + label)
    if failed:
        sys.exit("E2E CRABGEN FAILED: " + ", ".join(failed))
    print("E2E CRABGEN ALL PASS")


if __name__ == "__main__":
    main()
