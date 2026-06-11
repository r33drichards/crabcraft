//! WIRE conformance for the AssemblyScript runtime template (templates/ts).
//!
//! Mirrors cpp_vectors.rs: this test reads wit/vectors.json with serde_json
//! and GENERATES testdata/as-runtime/assembly/vectors.ts — tables of
//! straight-line encode / decode+re-encode functions, the exact
//! calling-convention shape the Task-5.2 bindings emitter will produce
//! (`const r0 = d.u32(); if (d.err !== null) return d.err;`) — then compiles
//! the scratch project with `asc` (pinned via package.json + package-lock)
//! and runs the wasm under node (run_vectors.mjs), which also drives the
//! section-2 ABI (crab_alloc/crab_schema/crab_invoke) and a fake
//! `crabcraft.call` mesh host from the host side.
//!
//! Finally the produced wasm is scanned with `wasm-objdump` for SIMD
//! (0xfd-prefixed) opcodes — the wasmcraft engine refuses them, and this is
//! the proof the AS toolchain meets that constraint.
//!
//! Runs in the normal suite (NOT #[ignore]d). Toolchain lookup: `nix shell
//! nixpkgs#nodejs` / `nixpkgs#wabt` if nix is on PATH (the project
//! convention), else bare `node`/`npm`/`npx`/`wasm-objdump`, else FAIL
//! loudly — never silently skip conformance.

use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn on_path(name: &str) -> bool {
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&paths).any(|dir| dir.join(name).is_file())
}

/// Command for a node-toolchain program (node/npm/npx): prefer nix (project
/// convention), fall back to the bare binary, and fail loudly if neither
/// exists — conformance must never be silently skipped.
fn node_cmd(prog: &str) -> Command {
    if on_path("nix") {
        let mut c = Command::new("nix");
        c.args(["shell", "nixpkgs#nodejs", "--command", prog]);
        c
    } else if on_path(prog) {
        Command::new(prog)
    } else {
        panic!(
            "neither `nix` nor `{prog}` is on PATH: cannot build the AssemblyScript \
             WIRE conformance vectors (install nix or node; do not skip this test)"
        );
    }
}

fn wasm_objdump_cmd() -> Command {
    if on_path("nix") {
        let mut c = Command::new("nix");
        c.args(["shell", "nixpkgs#wabt", "--command", "wasm-objdump"]);
        c
    } else if on_path("wasm-objdump") {
        Command::new("wasm-objdump")
    } else {
        panic!(
            "neither `nix` nor `wasm-objdump` is on PATH: cannot run the SIMD \
             tripwire on the AssemblyScript wasm (install nix or wabt; do not \
             skip this check)"
        );
    }
}

fn run_ok(mut cmd: Command, what: &str) -> String {
    let output = cmd.output().unwrap_or_else(|e| panic!("spawn {what}: {e}"));
    assert!(
        output.status.success(),
        "{what} failed ({}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn load_vectors(manifest: &Path) -> (PathBuf, Vec<Value>) {
    let path = manifest
        .join("../../wit/vectors.json")
        .canonicalize()
        .expect("wit/vectors.json must exist");
    let data = fs::read_to_string(&path).expect("read vectors.json");
    let vectors: Vec<Value> = serde_json::from_str(&data).expect("parse vectors.json");
    assert!(!vectors.is_empty(), "no vectors in {path:?}");
    (path, vectors)
}

// ---------------------------------------------------------------------------
// AS code generation from the JSON vectors (assembly/vectors.ts)
// ---------------------------------------------------------------------------

/// Escape a Rust string into an AS/TS double-quoted string literal. Non-ASCII
/// and control characters become \uXXXX escapes (UTF-16 code units, which is
/// exactly what AS strings hold — surrogate pairs included).
fn lit_str(s: &str) -> String {
    let mut out = String::from("\"");
    for u in s.encode_utf16() {
        match u {
            0x22 => out.push_str("\\\""),
            0x5c => out.push_str("\\\\"),
            0x20..=0x7e => out.push(u as u8 as char),
            _ => out.push_str(&format!("\\u{u:04x}")),
        }
    }
    out.push('"');
    out
}

/// The raw JSON token as a bare literal: numbers verbatim, strings unquoted
/// (the decimal-string convention for u64/s64 beyond Lua float precision).
/// AS integer literals are typed contextually, so a full-precision u64/i64
/// decimal literal is valid wherever a u64/i64 is expected.
fn num_lit(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// AS literal for an i64 (i64::MIN has no negative literal form).
fn s64_lit(v: i64) -> String {
    if v == i64::MIN {
        "i64.MIN_VALUE".to_string()
    } else {
        format!("{v}")
    }
}

/// Float literal that always parses (append .0 when integral).
fn float_lit(v: &Value) -> String {
    let s = num_lit(v);
    if s.contains('.') || s.contains('e') || s.contains('E') {
        s
    } else {
        format!("{s}.0")
    }
}

fn kind(ty: &Value) -> &str {
    ty["kind"].as_str().expect("type descriptor needs a kind")
}

/// Emit straight-line `s.<prim>(...)` Sink calls producing the vector's
/// value.
fn gen_encode(ty: &Value, val: &Value, code: &mut String) {
    let p = "  ";
    match kind(ty) {
        "bool" => {
            code.push_str(&format!(
                "{p}s.bool({});\n",
                val.as_bool().expect("bool value")
            ));
        }
        k @ ("u8" | "u16" | "u32" | "u64") => {
            let v: u64 = num_lit(val).parse().expect("unsigned value");
            code.push_str(&format!("{p}s.{k}({v});\n"));
        }
        k @ ("s8" | "s16" | "s32" | "s64") => {
            let v: i64 = num_lit(val).parse().expect("signed value");
            code.push_str(&format!("{p}s.{k}({});\n", s64_lit(v)));
        }
        "f32" => code.push_str(&format!("{p}s.f32(<f32>{});\n", float_lit(val))),
        "f64" => code.push_str(&format!("{p}s.f64({});\n", float_lit(val))),
        "char" => {
            let c = val
                .as_str()
                .and_then(|s| s.chars().next())
                .expect("one-char string") as u32;
            code.push_str(&format!("{p}s.char({c});\n"));
        }
        "string" => {
            let v = val.as_str().expect("string value");
            code.push_str(&format!("{p}s.string({});\n", lit_str(v)));
        }
        "list" => {
            let items = val.as_array().expect("list value");
            code.push_str(&format!("{p}s.listLen({});\n", items.len()));
            for it in items {
                gen_encode(&ty["element"], it, code);
            }
        }
        "record" => {
            for f in ty["fields"].as_array().expect("record fields") {
                let name = f["name"].as_str().expect("field name");
                // A field may only be omitted when it is an option (encodes
                // as none); anything else missing is a broken vector.
                let fv = match val.get(name) {
                    Some(v) => v,
                    None => {
                        assert_eq!(
                            kind(&f["type"]),
                            "option",
                            "record field {name:?} missing from value and not an option"
                        );
                        &Value::Null
                    }
                };
                gen_encode(&f["type"], fv, code);
            }
        }
        "tuple" => {
            let members = ty["members"].as_array().expect("tuple members");
            let items = val.as_array().expect("tuple value");
            assert_eq!(items.len(), members.len(), "tuple arity mismatch");
            for (m, it) in members.iter().zip(items) {
                gen_encode(m, it, code);
            }
        }
        "variant" => {
            let c = val["case"].as_u64().expect("variant case") as usize;
            code.push_str(&format!("{p}s.caseIdx({c});\n"));
            let payload_ty = &ty["cases"][c]["payload"];
            if !payload_ty.is_null() {
                gen_encode(payload_ty, &val["payload"], code);
            }
        }
        "enum" => {
            code.push_str(&format!(
                "{p}s.caseIdx({});\n",
                val.as_u64().expect("enum case")
            ));
        }
        "option" => {
            if val.is_null() {
                code.push_str(&format!("{p}s.optionTag(false);\n"));
            } else {
                code.push_str(&format!("{p}s.optionTag(true);\n"));
                gen_encode(&ty["inner"], val, code);
            }
        }
        "result" => {
            let obj = val.as_object().expect("result value");
            if let Some(okv) = obj.get("ok") {
                code.push_str(&format!("{p}s.resultTag(false);\n"));
                if !ty["ok"].is_null() {
                    gen_encode(&ty["ok"], okv, code);
                }
            } else {
                code.push_str(&format!("{p}s.resultTag(true);\n"));
                if !ty["err"].is_null() {
                    gen_encode(&ty["err"], &obj["err"], code);
                }
            }
        }
        "flags" => {
            let count = ty["count"].as_u64().expect("flags count") as usize;
            let mut bits = vec![false; count];
            for i in val.as_array().expect("flags value") {
                bits[i.as_u64().expect("flag index") as usize] = true;
            }
            let lits: Vec<&str> = bits.iter().map(|b| if *b { "true" } else { "false" }).collect();
            code.push_str(&format!("{p}s.flags([{}]);\n", lits.join(", ")));
        }
        k => panic!("unknown type kind {k:?}"),
    }
}

/// Emit straight-line decode + immediate re-encode code: each leaf decodes
/// off `d` (checking `d.err` after every call, the 5.2 emitter convention)
/// and re-encodes into `s`, so `s` must end byte-identical to the input.
/// `expect` (top-level scalars only, like the Go/C++ vectors tests) adds a
/// decoded-value equality check.
fn gen_reenc(ty: &Value, expect: Option<&Value>, n: &mut u32, code: &mut String) {
    let i = *n;
    *n += 1;
    let p = "  ";
    let check = format!("if (d.err !== null) return d.err;");
    match kind(ty) {
        "bool" => {
            code.push_str(&format!("{p}const r{i} = d.bool(); {check}\n"));
            if let Some(e) = expect {
                code.push_str(&format!(
                    "{p}if (r{i} != {}) return \"decoded bool mismatch\";\n",
                    e.as_bool().expect("bool value")
                ));
            }
            code.push_str(&format!("{p}s.bool(r{i});\n"));
        }
        k @ ("u8" | "u16" | "u32" | "u64") => {
            code.push_str(&format!("{p}const r{i} = d.{k}(); {check}\n"));
            if let Some(e) = expect {
                let v: u64 = num_lit(e).parse().expect("unsigned value");
                code.push_str(&format!(
                    "{p}if (r{i} != {v}) return \"decoded {k} mismatch\";\n"
                ));
            }
            code.push_str(&format!("{p}s.{k}(r{i});\n"));
        }
        k @ ("s8" | "s16" | "s32" | "s64") => {
            code.push_str(&format!("{p}const r{i} = d.{k}(); {check}\n"));
            if let Some(e) = expect {
                let v: i64 = num_lit(e).parse().expect("signed value");
                code.push_str(&format!(
                    "{p}if (r{i} != {}) return \"decoded {k} mismatch\";\n",
                    s64_lit(v)
                ));
            }
            code.push_str(&format!("{p}s.{k}(r{i});\n"));
        }
        "f32" => {
            code.push_str(&format!("{p}const r{i} = d.f32(); {check}\n"));
            if let Some(e) = expect {
                code.push_str(&format!(
                    "{p}if (r{i} != <f32>{}) return \"decoded f32 mismatch\";\n",
                    float_lit(e)
                ));
            }
            code.push_str(&format!("{p}s.f32(r{i});\n"));
        }
        "f64" => {
            code.push_str(&format!("{p}const r{i} = d.f64(); {check}\n"));
            if let Some(e) = expect {
                code.push_str(&format!(
                    "{p}if (r{i} != {}) return \"decoded f64 mismatch\";\n",
                    float_lit(e)
                ));
            }
            code.push_str(&format!("{p}s.f64(r{i});\n"));
        }
        "char" => {
            code.push_str(&format!("{p}const r{i} = d.char(); {check}\n"));
            if let Some(e) = expect {
                let c = e
                    .as_str()
                    .and_then(|s| s.chars().next())
                    .expect("one-char string") as u32;
                code.push_str(&format!(
                    "{p}if (r{i} != {c}) return \"decoded char mismatch\";\n"
                ));
            }
            code.push_str(&format!("{p}s.char(r{i});\n"));
        }
        "string" => {
            code.push_str(&format!("{p}const r{i} = d.string(); {check}\n"));
            if let Some(e) = expect {
                code.push_str(&format!(
                    "{p}if (r{i} != {}) return \"decoded string mismatch\";\n",
                    lit_str(e.as_str().expect("string value"))
                ));
            }
            code.push_str(&format!("{p}s.string(r{i});\n"));
        }
        "list" => {
            code.push_str(&format!("{p}const r{i} = d.listLen(); {check}\n"));
            code.push_str(&format!("{p}s.listLen(r{i});\n"));
            code.push_str(&format!(
                "{p}for (let j{i}: u32 = 0; j{i} < r{i}; j{i}++) {{\n"
            ));
            gen_reenc(&ty["element"], None, n, code);
            code.push_str(&format!("{p}}}\n"));
        }
        "record" => {
            for f in ty["fields"].as_array().expect("record fields") {
                gen_reenc(&f["type"], None, n, code);
            }
        }
        "tuple" => {
            for m in ty["members"].as_array().expect("tuple members") {
                gen_reenc(m, None, n, code);
            }
        }
        "variant" => {
            let cases = ty["cases"].as_array().expect("variant cases");
            code.push_str(&format!(
                "{p}const r{i} = d.variantCase({}); {check}\n",
                cases.len()
            ));
            code.push_str(&format!("{p}s.caseIdx(r{i});\n"));
            code.push_str(&format!("{p}switch (<i32>r{i}) {{\n"));
            for (c, case) in cases.iter().enumerate() {
                if case["payload"].is_null() {
                    code.push_str(&format!("{p}case {c}: break;\n"));
                } else {
                    code.push_str(&format!("{p}case {c}: {{\n"));
                    gen_reenc(&case["payload"], None, n, code);
                    code.push_str(&format!("{p}}} break;\n"));
                }
            }
            code.push_str(&format!("{p}default: break;\n{p}}}\n"));
        }
        "enum" => {
            let count = ty["cases"].as_array().expect("enum cases").len();
            code.push_str(&format!("{p}const r{i} = d.enumCase({count}); {check}\n"));
            code.push_str(&format!("{p}s.caseIdx(r{i});\n"));
        }
        "option" => {
            code.push_str(&format!("{p}const r{i} = d.optionTag(); {check}\n"));
            code.push_str(&format!("{p}s.optionTag(r{i});\n"));
            code.push_str(&format!("{p}if (r{i}) {{\n"));
            gen_reenc(&ty["inner"], None, n, code);
            code.push_str(&format!("{p}}}\n"));
        }
        "result" => {
            code.push_str(&format!("{p}const r{i} = d.resultTag(); {check}\n"));
            code.push_str(&format!("{p}s.resultTag(r{i});\n"));
            code.push_str(&format!("{p}if (r{i}) {{\n"));
            if !ty["err"].is_null() {
                gen_reenc(&ty["err"], None, n, code);
            }
            code.push_str(&format!("{p}}} else {{\n"));
            if !ty["ok"].is_null() {
                gen_reenc(&ty["ok"], None, n, code);
            }
            code.push_str(&format!("{p}}}\n"));
        }
        "flags" => {
            let count = ty["count"].as_u64().expect("flags count");
            code.push_str(&format!("{p}const r{i} = d.flags({count}); {check}\n"));
            code.push_str(&format!("{p}s.flags(r{i});\n"));
        }
        k => panic!("unknown type kind {k:?}"),
    }
}

fn is_scalar_kind(k: &str) -> bool {
    matches!(
        k,
        "bool" | "u8" | "u16" | "u32" | "u64" | "s8" | "s16" | "s32" | "s64" | "f32" | "f64"
            | "char" | "string"
    )
}

fn gen_vectors_ts(vectors: &[Value]) -> String {
    let mut s = String::new();
    s.push_str(
        "// generated by tools/crabgen/tests/as_vectors.rs from wit/vectors.json\n\
         // -- do not edit. One enc_/re_ function pair per conformance vector,\n\
         // straight-line, in the exact shape the Task-5.2 bindings emitter\n\
         // produces.\n\
         import { Decoder, Sink } from \"./gen/runtime\";\n\
         import { fail, hexDecode, toHex } from \"./harness\";\n\n",
    );
    let mut descs = Vec::new();
    let mut hexes = Vec::new();
    for (vi, v) in vectors.iter().enumerate() {
        let desc = v["desc"].as_str().expect("vector desc");
        let hex = v["hex"].as_str().expect("vector hex");
        descs.push(lit_str(desc));
        hexes.push(format!("\"{hex}\""));
        let ty = &v["type"];

        s.push_str(&format!("function enc_{vi}(s: Sink): void {{\n"));
        gen_encode(ty, &v["value"], &mut s);
        s.push_str("}\n\n");

        s.push_str(&format!(
            "function re_{vi}(d: Decoder, s: Sink): string | null {{\n"
        ));
        let mut n = 0u32;
        let expect = if is_scalar_kind(kind(ty)) {
            Some(&v["value"])
        } else {
            None
        };
        gen_reenc(ty, expect, &mut n, &mut s);
        s.push_str("  return null;\n}\n\n");
    }

    s.push_str(&format!(
        "const DESCS: string[] = [\n  {}\n];\n",
        descs.join(",\n  ")
    ));
    s.push_str(&format!(
        "const HEXES: string[] = [\n  {}\n];\n",
        hexes.join(",\n  ")
    ));
    let encs: Vec<String> = (0..vectors.len()).map(|i| format!("enc_{i}")).collect();
    let res: Vec<String> = (0..vectors.len()).map(|i| format!("re_{i}")).collect();
    s.push_str(&format!(
        "const ENCS: ((s: Sink) => void)[] = [{}];\n",
        encs.join(", ")
    ));
    s.push_str(&format!(
        "const RES: ((d: Decoder, s: Sink) => string | null)[] = [{}];\n\n",
        res.join(", ")
    ));
    s.push_str(
        "export function vectorCount(): i32 {\n  return DESCS.length;\n}\n\n\
         export function runVectors(): void {\n\
         \x20 for (let i = 0; i < DESCS.length; i++) {\n\
         \x20   const desc = DESCS[i];\n\
         \x20   const wantHex = HEXES[i];\n\
         \x20   // 1. value -> bytes must equal hex.\n\
         \x20   const s = new Sink();\n\
         \x20   ENCS[i](s);\n\
         \x20   if (s.err !== null) {\n\
         \x20     fail(desc, \"encode: \" + s.err!);\n\
         \x20     continue;\n\
         \x20   }\n\
         \x20   if (toHex(s.bytes()) != wantHex) {\n\
         \x20     fail(desc, \"encode: got \" + toHex(s.bytes()) + \", want \" + wantHex);\n\
         \x20   }\n\
         \x20   // 2. hex -> value must consume the whole buffer (+ scalar\n\
         \x20   // equality), and 3. re-encode byte-identically.\n\
         \x20   const d = new Decoder(hexDecode(wantHex));\n\
         \x20   const s2 = new Sink();\n\
         \x20   const err = RES[i](d, s2);\n\
         \x20   if (err !== null) {\n\
         \x20     fail(desc, \"decode: \" + err!);\n\
         \x20     continue;\n\
         \x20   }\n\
         \x20   const fin = d.finish(\"value\");\n\
         \x20   if (fin !== null) {\n\
         \x20     fail(desc, \"decode: \" + fin!);\n\
         \x20     continue;\n\
         \x20   }\n\
         \x20   if (s2.err !== null) {\n\
         \x20     fail(desc, \"re-encode: \" + s2.err!);\n\
         \x20     continue;\n\
         \x20   }\n\
         \x20   if (toHex(s2.bytes()) != wantHex) {\n\
         \x20     fail(desc, \"re-encode: got \" + toHex(s2.bytes()) + \", want \" + wantHex);\n\
         \x20   }\n\
         \x20 }\n\
         }\n",
    );
    s
}

// ---------------------------------------------------------------------------
// toolchain plumbing
// ---------------------------------------------------------------------------

/// Refresh testdata/as-runtime/assembly/gen from templates/ts/assembly/gen
/// so the copies can never go stale (the directory is wiped first so
/// deleted/renamed templates can't leave stale copies behind).
fn refresh_gen_copies(templates: &Path, scratch: &Path) {
    let src = templates.join("assembly/gen");
    let dst = scratch.join("assembly/gen");
    if dst.exists() {
        fs::remove_dir_all(&dst).expect("clear testdata/as-runtime/assembly/gen");
    }
    fs::create_dir_all(&dst).expect("create testdata/as-runtime/assembly/gen");
    let mut copied = 0;
    for entry in fs::read_dir(&src).expect("read templates/ts/assembly/gen") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "ts") {
            let dest = dst.join(path.file_name().unwrap());
            fs::copy(&path, &dest).unwrap_or_else(|e| panic!("copy {path:?} -> {dest:?}: {e}"));
            copied += 1;
        }
    }
    assert!(copied >= 2, "expected runtime.ts + mesh.ts in {src:?}");
}

/// `npm ci` into testdata/as-runtime/node_modules unless the pinned
/// assemblyscript version is already installed (node_modules is gitignored
/// and cached between runs).
fn ensure_node_modules(scratch: &Path) {
    let pkg: Value = serde_json::from_str(
        &fs::read_to_string(scratch.join("package.json")).expect("read package.json"),
    )
    .expect("parse package.json");
    let pinned = pkg["devDependencies"]["assemblyscript"]
        .as_str()
        .expect("package.json must pin assemblyscript");

    let installed = scratch.join("node_modules/assemblyscript/package.json");
    let up_to_date = fs::read_to_string(&installed)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .is_some_and(|v| v["version"].as_str() == Some(pinned));
    if up_to_date {
        return;
    }
    let mut cmd = node_cmd("npm");
    cmd.arg("ci").current_dir(scratch);
    run_ok(cmd, "npm ci (assemblyscript toolchain)");
}

/// SIMD tripwire: the wasmcraft engine refuses 0xfd-prefixed (SIMD)
/// opcodes. Same two anchored checks as the Go/C++ lanes' build.sh:
/// wasm-objdump prints opcode bytes as "fd 0c" (no 0x prefix) right after
/// the offset column, and SIMD mnemonics as v128.*/i8x16.*/... after the
/// "|" column — anchoring to those columns keeps symbol names containing
/// e.g. "i32x4" from false-positiving.
fn assert_simd_free(disasm: &str, wasm: &Path) {
    const MNEMONICS: [&str; 8] = [
        "v128.", "i8x16.", "i16x8.", "i32x4.", "i64x2.", "f32x4.", "f64x2.", "f16x8.",
    ];
    for line in disasm.lines() {
        if let Some(idx) = line.find('|') {
            let mn = line[idx + 1..].trim_start();
            if MNEMONICS.iter().any(|m| mn.starts_with(m)) {
                panic!("SIMD opcode in {wasm:?}: {line}");
            }
        }
        let t = line.trim_start();
        if let Some((addr, rest)) = t.split_once(':') {
            if !addr.is_empty() && addr.chars().all(|c| c.is_ascii_hexdigit()) {
                let bytes = rest.trim_start();
                if bytes.starts_with("fd ") {
                    panic!("0xfd-prefixed (SIMD) opcode in {wasm:?}: {line}");
                }
            }
        }
    }
}

/// The tripwire must catch real SIMD disasm by mnemonic (format verified
/// against wabt: ` 000057: fd 11 | i32x4.splat`).
#[test]
#[should_panic(expected = "SIMD opcode")]
fn simd_tripwire_catches_mnemonics() {
    assert_simd_free(
        " 000057: fd 11                      | i32x4.splat",
        Path::new("x.wasm"),
    );
}

/// ...and by the raw 0xfd byte column even if the mnemonic were missing.
#[test]
#[should_panic(expected = "0xfd-prefixed")]
fn simd_tripwire_catches_fd_bytes() {
    assert_simd_free(
        " 00005f: fd ae 01                   | something.unknown",
        Path::new("x.wasm"),
    );
}

/// Symbol names containing SIMD-ish substrings must not false-positive
/// (the checks are anchored to the byte/mnemonic columns).
#[test]
fn simd_tripwire_ignores_simd_like_symbols() {
    assert_simd_free(
        " 000010: 10 80 01                   | call 128 <foo_i32x4.bar>\n \
         000053: 02 7b                      | local[1..2] type=v128",
        Path::new("x.wasm"),
    );
}

// ---------------------------------------------------------------------------
// the test
// ---------------------------------------------------------------------------

#[test]
fn as_runtime_passes_wire_vectors() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let templates = manifest.join("templates/ts");
    let scratch = manifest.join("testdata/as-runtime");
    refresh_gen_copies(&templates, &scratch);

    let (_, vectors) = load_vectors(manifest);
    fs::write(
        scratch.join("assembly/vectors.ts"),
        gen_vectors_ts(&vectors),
    )
    .expect("write assembly/vectors.ts");

    ensure_node_modules(&scratch);

    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join("as-vectors");
    fs::create_dir_all(&tmp).expect("create tmp dir");
    let wasm = tmp.join("vectors.wasm");

    // Compile with the production flag set (see templates/ts: reactor shape
    // via --exportStart _initialize, no env.abort import via --use abort=,
    // incremental GC runtime). SIMD is disabled by default in asc — the
    // tripwire below is the proof.
    let mut cmd = node_cmd("npx");
    cmd.args(["asc", "assembly/main.ts", "-o"])
        .arg(&wasm)
        .args([
            "--exportStart",
            "_initialize",
            "--use",
            "abort=",
            "--runtime",
            "incremental",
            "--optimizeLevel",
            "3",
            "--shrinkLevel",
            "1",
        ])
        .current_dir(&scratch);
    run_ok(cmd, "asc (AssemblyScript vectors build)");

    // Run the conformance suite under node: host-side ABI smoke + fake mesh
    // host + the in-wasm vectors table and edge cases.
    let mut cmd = node_cmd("node");
    cmd.arg("run_vectors.mjs").arg(&wasm).current_dir(&scratch);
    let stdout = run_ok(cmd, "node run_vectors.mjs");
    // The runner prints how many table vectors it ran: pin it to the JSON
    // count so a silently-empty table can never pass.
    let want = format!("ok: {} vectors", vectors.len());
    assert!(
        stdout.contains(&want),
        "run_vectors.mjs stdout missing {want:?}:\n{stdout}"
    );

    // SIMD tripwire on the artifact the toolchain actually produced.
    let mut cmd = wasm_objdump_cmd();
    cmd.arg("-d").arg(&wasm);
    let disasm = run_ok(cmd, "wasm-objdump -d (SIMD tripwire)");
    assert_simd_free(&disasm, &wasm);
}
