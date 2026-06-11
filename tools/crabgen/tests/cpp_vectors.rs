//! WIRE conformance for the C++ runtime template (templates/cpp).
//!
//! No JSON parser ships in C++: this test reads wit/vectors.json with
//! serde_json and generates `vectors.inc` — a table of straight-line
//! encode / decode+re-encode lambdas, the exact calling-convention shape the
//! Task-4.2 bindings emitter will produce — then compiles
//! testdata/cpp-runtime/vectors_main.cpp + crab.cpp + mesh.cpp NATIVELY with
//! `zig c++` and runs it. A second test compiles crab.cpp + mesh.cpp + a tiny
//! stub for wasm32-wasi (reactor, -mno-simd128, -fno-exceptions) to prove the
//! ABI path builds — the smoke for Task 4.2's build.sh.
//!
//! Runs in the normal suite (NOT #[ignore]d). Toolchain lookup: `nix shell
//! nixpkgs#zig` if nix is on PATH (the project convention), else a bare
//! `zig`, else FAIL loudly — never silently skip conformance.

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

/// `zig` invocation: prefer nix (project convention), fall back to a bare
/// zig, and fail loudly if neither exists — conformance must never be
/// silently skipped.
fn zig_cmd() -> Command {
    if on_path("nix") {
        let mut c = Command::new("nix");
        c.args(["shell", "nixpkgs#zig", "--command", "zig"]);
        c
    } else if on_path("zig") {
        Command::new("zig")
    } else {
        panic!(
            "neither `nix` nor `zig` is on PATH: cannot build the C++ WIRE \
             conformance vectors (install nix or zig; do not skip this test)"
        );
    }
}

/// Persistent zig cache under target/tmp so libc++ is not rebuilt per run.
fn zig_cache(cmd: &mut Command, tmp: &Path) {
    let cache = tmp.join("zig-cache");
    fs::create_dir_all(&cache).expect("create zig cache dir");
    cmd.env("ZIG_GLOBAL_CACHE_DIR", &cache)
        .env("ZIG_LOCAL_CACHE_DIR", &cache);
}

fn run_ok(mut cmd: Command, what: &str) {
    let output = cmd.output().unwrap_or_else(|e| panic!("spawn {what}: {e}"));
    assert!(
        output.status.success(),
        "{what} failed ({}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
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
// C++ code generation from the JSON vectors (the .inc)
// ---------------------------------------------------------------------------

/// Escape arbitrary bytes into a C++ string literal. Octal escapes (always
/// 3 digits) are unambiguous regardless of what character follows — unlike
/// \xNN, which would swallow subsequent hex digits.
fn lit_str(bytes: &[u8]) -> String {
    let mut s = String::from("\"");
    for &b in bytes {
        match b {
            b'"' => s.push_str("\\\""),
            b'\\' => s.push_str("\\\\"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\{b:03o}")),
        }
    }
    s.push('"');
    s
}

/// The raw JSON token as a bare literal: numbers verbatim, strings unquoted
/// (the decimal-string convention for u64/s64 beyond Lua float precision).
fn num_lit(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// C++ literal for an i64 (i64::MIN has no negative literal form in C++).
fn s64_lit(v: i64) -> String {
    if v == i64::MIN {
        "(-9223372036854775807LL - 1)".to_string()
    } else {
        format!("{v}LL")
    }
}

/// Float literal that is always valid C++ (append .0 when integral).
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

fn cpp_int_type(k: &str) -> &'static str {
    match k {
        "u8" => "uint8_t",
        "u16" => "uint16_t",
        "u32" => "uint32_t",
        "u64" => "uint64_t",
        "s8" => "int8_t",
        "s16" => "int16_t",
        "s32" => "int32_t",
        "s64" => "int64_t",
        _ => unreachable!(),
    }
}

/// Emit straight-line `crab::Encode*` calls producing the vector's value.
fn gen_encode(ty: &Value, val: &Value, code: &mut String) {
    let p = "    ";
    match kind(ty) {
        "bool" => {
            code.push_str(&format!(
                "{p}crab::EncodeBool(out, {});\n",
                val.as_bool().expect("bool value")
            ));
        }
        k @ ("u8" | "u16" | "u32" | "u64") => {
            let v: u64 = num_lit(val).parse().expect("unsigned value");
            code.push_str(&format!(
                "{p}crab::Encode{}(out, ({}){v}ULL);\n",
                k.to_uppercase(),
                cpp_int_type(k)
            ));
        }
        k @ ("s8" | "s16" | "s32" | "s64") => {
            let v: i64 = num_lit(val).parse().expect("signed value");
            code.push_str(&format!(
                "{p}crab::Encode{}(out, ({}){});\n",
                k.to_uppercase(),
                cpp_int_type(k),
                s64_lit(v)
            ));
        }
        "f32" => code.push_str(&format!("{p}crab::EncodeF32(out, {}f);\n", float_lit(val))),
        "f64" => code.push_str(&format!("{p}crab::EncodeF64(out, {});\n", float_lit(val))),
        "char" => {
            let c = val
                .as_str()
                .and_then(|s| s.chars().next())
                .expect("one-char string") as u32;
            code.push_str(&format!("{p}crab::EncodeChar(out, {c}u);\n"));
        }
        "string" => {
            let s = val.as_str().expect("string value");
            code.push_str(&format!(
                "{p}crab::EncodeString(out, std::string_view({}, {}));\n",
                lit_str(s.as_bytes()),
                s.len()
            ));
        }
        "list" => {
            let items = val.as_array().expect("list value");
            code.push_str(&format!("{p}crab::EncodeListLen(out, {});\n", items.len()));
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
            code.push_str(&format!("{p}crab::EncodeCase(out, {c}u);\n"));
            let payload_ty = &ty["cases"][c]["payload"];
            if !payload_ty.is_null() {
                gen_encode(payload_ty, &val["payload"], code);
            }
        }
        "enum" => {
            code.push_str(&format!(
                "{p}crab::EncodeCase(out, {}u);\n",
                val.as_u64().expect("enum case")
            ));
        }
        "option" => {
            if val.is_null() {
                code.push_str(&format!("{p}crab::EncodeOptionTag(out, false);\n"));
            } else {
                code.push_str(&format!("{p}crab::EncodeOptionTag(out, true);\n"));
                gen_encode(&ty["inner"], val, code);
            }
        }
        "result" => {
            let obj = val.as_object().expect("result value");
            if let Some(okv) = obj.get("ok") {
                code.push_str(&format!("{p}crab::EncodeResultTag(out, false);\n"));
                if !ty["ok"].is_null() {
                    gen_encode(&ty["ok"], okv, code);
                }
            } else {
                code.push_str(&format!("{p}crab::EncodeResultTag(out, true);\n"));
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
            code.push_str(&format!(
                "{p}crab::EncodeFlags(out, std::vector<bool>{{{}}});\n",
                lits.join(", ")
            ));
        }
        k => panic!("unknown type kind {k:?}"),
    }
}

/// Emit straight-line decode + immediate re-encode code: each leaf decodes
/// off `d` (propagating errors) and re-encodes into `out`, so `out` must end
/// byte-identical to the input. `expect` (top-level scalars only, like the
/// Go vectors test) adds a decoded-value equality check.
fn gen_reenc(ty: &Value, expect: Option<&Value>, n: &mut u32, code: &mut String) {
    let i = *n;
    *n += 1;
    let p = "    ";
    match kind(ty) {
        "bool" => {
            code.push_str(&format!(
                "{p}auto r{i} = d.Bool(); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            if let Some(e) = expect {
                code.push_str(&format!(
                    "{p}if (r{i}.val != {}) return \"decoded bool mismatch\";\n",
                    e.as_bool().expect("bool value")
                ));
            }
            code.push_str(&format!("{p}crab::EncodeBool(out, r{i}.val);\n"));
        }
        k @ ("u8" | "u16" | "u32" | "u64") => {
            let m = k.to_uppercase();
            code.push_str(&format!(
                "{p}auto r{i} = d.{m}(); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            if let Some(e) = expect {
                let v: u64 = num_lit(e).parse().expect("unsigned value");
                code.push_str(&format!(
                    "{p}if ((uint64_t)r{i}.val != {v}ULL) return \"decoded {k} mismatch\";\n"
                ));
            }
            code.push_str(&format!("{p}crab::Encode{m}(out, r{i}.val);\n"));
        }
        k @ ("s8" | "s16" | "s32" | "s64") => {
            let m = k.to_uppercase();
            code.push_str(&format!(
                "{p}auto r{i} = d.{m}(); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            if let Some(e) = expect {
                let v: i64 = num_lit(e).parse().expect("signed value");
                code.push_str(&format!(
                    "{p}if ((int64_t)r{i}.val != {}) return \"decoded {k} mismatch\";\n",
                    s64_lit(v)
                ));
            }
            code.push_str(&format!("{p}crab::Encode{m}(out, r{i}.val);\n"));
        }
        "f32" => {
            code.push_str(&format!(
                "{p}auto r{i} = d.F32(); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            if let Some(e) = expect {
                code.push_str(&format!(
                    "{p}if (r{i}.val != {}f) return \"decoded f32 mismatch\";\n",
                    float_lit(e)
                ));
            }
            code.push_str(&format!("{p}crab::EncodeF32(out, r{i}.val);\n"));
        }
        "f64" => {
            code.push_str(&format!(
                "{p}auto r{i} = d.F64(); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            if let Some(e) = expect {
                code.push_str(&format!(
                    "{p}if (r{i}.val != {}) return \"decoded f64 mismatch\";\n",
                    float_lit(e)
                ));
            }
            code.push_str(&format!("{p}crab::EncodeF64(out, r{i}.val);\n"));
        }
        "char" => {
            code.push_str(&format!(
                "{p}auto r{i} = d.Char(); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            if let Some(e) = expect {
                let c = e
                    .as_str()
                    .and_then(|s| s.chars().next())
                    .expect("one-char string") as u32;
                code.push_str(&format!(
                    "{p}if (r{i}.val != {c}u) return \"decoded char mismatch\";\n"
                ));
            }
            code.push_str(&format!("{p}crab::EncodeChar(out, r{i}.val);\n"));
        }
        "string" => {
            code.push_str(&format!(
                "{p}auto r{i} = d.String(); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            if let Some(e) = expect {
                let s = e.as_str().expect("string value");
                code.push_str(&format!(
                    "{p}if (r{i}.val != std::string({}, {})) return \"decoded string mismatch\";\n",
                    lit_str(s.as_bytes()),
                    s.len()
                ));
            }
            code.push_str(&format!("{p}crab::EncodeString(out, r{i}.val);\n"));
        }
        "list" => {
            code.push_str(&format!(
                "{p}auto r{i} = d.ListLen(); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            code.push_str(&format!("{p}crab::EncodeListLen(out, r{i}.val);\n"));
            code.push_str(&format!(
                "{p}for (uint32_t j{i} = 0; j{i} < r{i}.val; j{i}++) {{\n"
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
                "{p}auto r{i} = d.VariantCase({}); if (!r{i}.ok()) return r{i}.err;\n",
                cases.len()
            ));
            code.push_str(&format!("{p}crab::EncodeCase(out, r{i}.val);\n"));
            code.push_str(&format!("{p}switch (r{i}.val) {{\n"));
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
            code.push_str(&format!(
                "{p}auto r{i} = d.EnumCase({count}); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            code.push_str(&format!("{p}crab::EncodeCase(out, r{i}.val);\n"));
        }
        "option" => {
            code.push_str(&format!(
                "{p}auto r{i} = d.OptionTag(); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            code.push_str(&format!("{p}crab::EncodeOptionTag(out, r{i}.val);\n"));
            code.push_str(&format!("{p}if (r{i}.val) {{\n"));
            gen_reenc(&ty["inner"], None, n, code);
            code.push_str(&format!("{p}}}\n"));
        }
        "result" => {
            code.push_str(&format!(
                "{p}auto r{i} = d.ResultTag(); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            code.push_str(&format!("{p}crab::EncodeResultTag(out, r{i}.val);\n"));
            code.push_str(&format!("{p}if (r{i}.val) {{\n"));
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
            code.push_str(&format!(
                "{p}auto r{i} = d.Flags({count}); if (!r{i}.ok()) return r{i}.err;\n"
            ));
            code.push_str(&format!("{p}crab::EncodeFlags(out, r{i}.val);\n"));
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

fn gen_inc(vectors: &[Value]) -> String {
    let mut s = String::new();
    s.push_str("// generated by tools/crabgen/tests/cpp_vectors.rs from wit/vectors.json\n");
    s.push_str("// -- do not edit. One entry per conformance vector.\n");
    s.push_str("static const Vec VECTORS[] = {\n");
    for v in vectors {
        let desc = v["desc"].as_str().expect("vector desc");
        let hex = v["hex"].as_str().expect("vector hex");
        let ty = &v["type"];
        let mut enc = String::new();
        gen_encode(ty, &v["value"], &mut enc);
        let mut re = String::new();
        let mut n = 0u32;
        let expect = if is_scalar_kind(kind(ty)) {
            Some(&v["value"])
        } else {
            None
        };
        gen_reenc(ty, expect, &mut n, &mut re);
        s.push_str(&format!(
            "  {{{}, \"{hex}\",\n   [](std::vector<uint8_t>& out) {{\n{enc}   }},\n   \
             [](crab::Decoder& d, std::vector<uint8_t>& out) -> std::string {{\n{re}    \
             return std::string();\n   }}}},\n",
            lit_str(desc.as_bytes())
        ));
    }
    s.push_str("};\n");
    s.push_str("static const size_t NVECTORS = sizeof(VECTORS) / sizeof(VECTORS[0]);\n");
    s
}

// ---------------------------------------------------------------------------
// the tests
// ---------------------------------------------------------------------------

#[test]
fn cpp_runtime_passes_wire_vectors() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let templates = manifest.join("templates/cpp");
    let driver = manifest.join("testdata/cpp-runtime/vectors_main.cpp");
    let (_, vectors) = load_vectors(manifest);

    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cpp-vectors");
    fs::create_dir_all(&tmp).expect("create tmp dir");
    fs::write(tmp.join("vectors.inc"), gen_inc(&vectors)).expect("write vectors.inc");

    let bin = tmp.join("vectors_main");
    let mut cmd = zig_cmd();
    zig_cache(&mut cmd, &tmp);
    cmd.args(["c++", "-std=c++17", "-fno-exceptions", "-fno-rtti", "-O1"])
        .arg("-I")
        .arg(&templates)
        .arg("-I")
        .arg(&tmp)
        .arg("-o")
        .arg(&bin)
        .arg(&driver)
        .arg(templates.join("crab.cpp"))
        .arg(templates.join("mesh.cpp"));
    run_ok(cmd, "zig c++ (native vectors build)");

    let output = Command::new(&bin).output().expect("run vectors_main");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "vectors_main failed ({}):\n--- stdout ---\n{stdout}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    // The driver prints how many table vectors it ran: pin it to the JSON
    // count so a silently-empty .inc can never pass.
    let want = format!("ok: {} vectors", vectors.len());
    assert!(
        stdout.contains(&want),
        "vectors_main stdout missing {want:?}:\n{stdout}"
    );
}

/// The wasm32-wasi reactor build of the ABI half must compile and link: this
/// is the smoke for Task 4.2's build.sh. A stub provides what generated
/// bindings normally do (SchemaJson + one registered handler that also
/// touches MeshCall so the crabcraft.call import path links).
#[test]
fn cpp_runtime_abi_compiles_for_wasm() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let templates = manifest.join("templates/cpp");

    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR")).join("cpp-wasm-smoke");
    fs::create_dir_all(&tmp).expect("create tmp dir");
    let stub = tmp.join("stub.cpp");
    fs::write(
        &stub,
        r#"// wasm smoke stub: stands in for generated bindings (Task 4.2).
#include "crab.hpp"
#include "mesh.hpp"

namespace crab {
std::string_view SchemaJson() { return "{}"; }
}  // namespace crab

static crab::Res<std::vector<uint8_t>> ping(crab::Decoder& d) {
  std::string fin = d.Finish("params");
  if (!fin.empty()) return crab::Res<std::vector<uint8_t>>::fail(fin);
  std::vector<uint8_t> out;
  crab::EncodeString(out, "pong");
  // Reference the mesh client so the crabcraft.call import links too.
  auto r = crab::MeshCall("self", "test:x/y@0.1.0#ping", out);
  (void)r;
  return {out, {}};
}

static bool registered [[maybe_unused]] =
    crab::RegisterHandler("test:x/y@0.1.0#ping", ping);
"#,
    )
    .expect("write stub.cpp");

    let wasm = tmp.join("smoke.wasm");
    let mut cmd = zig_cmd();
    zig_cache(&mut cmd, &tmp);
    cmd.args([
        "c++",
        "-target",
        "wasm32-wasi",
        "-mexec-model=reactor",
        "-mno-simd128",
        "-fno-exceptions",
        "-fno-rtti",
        "-std=c++17",
        "-Oz",
        "-Wl,--export-memory",
    ])
    .arg("-I")
    .arg(&templates)
    .arg("-o")
    .arg(&wasm)
    .arg(templates.join("crab.cpp"))
    .arg(templates.join("mesh.cpp"))
    .arg(&stub);
    run_ok(cmd, "zig c++ (wasm32-wasi reactor smoke)");

    let bytes = fs::read(&wasm).expect("read smoke.wasm");
    assert!(!bytes.is_empty(), "smoke.wasm is empty");
    // The export names and the mesh import module must appear in the binary
    // (name bytes in the export/import sections).
    for needle in ["crab_alloc", "crab_schema", "crab_invoke", "crabcraft"] {
        assert!(
            bytes
                .windows(needle.len())
                .any(|w| w == needle.as_bytes()),
            "smoke.wasm missing {needle:?} (export/import section)"
        );
    }
}
