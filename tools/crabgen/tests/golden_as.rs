//! Golden + compile tests for the AssemblyScript backend.
//!
//! - `as_golden_full`: generate + scaffold tests/fixtures/full.wit into a
//!   tempdir and compare the produced tree (file SET and byte contents)
//!   against the snapshot at tests/golden/as/full/. Run with UPDATE_GOLDEN=1
//!   to re-record the snapshot — review the diff by hand, it's the contract.
//! - `as_full_project_compiles`: asc over the whole generated project
//!   (bindings + scaffolded stubs) with the exact build.sh flag set,
//!   asserting the crab_* exports land in the binary, that the
//!   crabcraft.call import appears exactly when an impl actually calls a
//!   mesh wrapper (asc only compiles entry-reachable code, so unused
//!   wrappers — and mesh.ts's @external — are tree-shaken, the same
//!   observable behavior as the C++ lane's linker GC), and that the SIMD
//!   tripwire stays clean.
//! - `as_no_imports_project_compiles_without_mesh`: an import-free WIT must
//!   produce no assembly/gen/mesh.ts and a wasm with NO crabcraft import.
//! - `as_missing_impls_*`: the substring scan over assembly/impl.ts.
//! - collision tests: duplicate record fields, a WIT type landing on a
//!   generated shared-shape class, and a reserved bindings identifier all
//!   fail generate() loudly.
//!
//! node_modules reuse: asc is NOT installed per test project. The committed
//! pin lives in testdata/as-runtime (kept warm by tests/as_vectors.rs); this
//! file npm-ci's it once if missing (Once-guarded — npm ci must not race
//! itself) and symlinks that node_modules into every temp project, so the
//! suite never grows a second toolchain tree. asc itself has no shared
//! mutable cache (unlike zig's libc++ build), so compiles are not
//! serialized.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

use crabgen::backend::backend_for;
use crabgen::ir::Module;
use crabgen::wit;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/full.wit")
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/as/full")
}

/// Generate + scaffold the full.wit project at <root>/guest/full, mirroring
/// the driver: gen/ pre-created, generate(), then scaffold(). Returns the
/// module and the project dir.
fn generate_full(root: &Path) -> (Module, PathBuf) {
    let dir = root.join("guest/full");
    fs::create_dir_all(dir.join("gen")).unwrap();
    fs::copy(fixture(), dir.join("full.wit")).unwrap();
    let module = wit::load(&dir.join("full.wit")).expect("load full.wit");
    let backend = backend_for("ts").expect("ts backend exists");
    backend.generate(&module, &dir).expect("generate");
    backend.scaffold(&module, &dir).expect("scaffold");
    (module, dir)
}

/// Relative paths of every file under base, sorted. Skips the WIT input
/// (test setup, not backend output).
fn tree(base: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().to_string();
                if rel != "full.wit" {
                    out.push(rel);
                }
            }
        }
    }
    out.sort();
    out
}

#[test]
fn as_golden_full() {
    let tmp = tempfile::tempdir().unwrap();
    let (_m, dir) = generate_full(tmp.path());

    // build.sh must be executable regardless of snapshot mode
    let mode = fs::metadata(dir.join("build.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "build.sh must carry the executable bit");

    let golden = golden_dir();
    if env::var_os("UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        if golden.exists() {
            fs::remove_dir_all(&golden).unwrap();
        }
        for rel in tree(&dir) {
            let dst = golden.join(&rel);
            fs::create_dir_all(dst.parent().unwrap()).unwrap();
            fs::copy(dir.join(&rel), dst).unwrap();
        }
        eprintln!("golden snapshot updated at {}", golden.display());
        return;
    }

    assert!(
        golden.is_dir(),
        "no golden snapshot at {} — record one with UPDATE_GOLDEN=1 and review it",
        golden.display()
    );
    let got = tree(&dir);
    let want = tree(&golden);
    assert_eq!(
        got, want,
        "generated file set differs from golden (UPDATE_GOLDEN=1 to re-record)"
    );
    for rel in &want {
        let got_bytes = fs::read_to_string(dir.join(rel)).unwrap();
        let want_bytes = fs::read_to_string(golden.join(rel)).unwrap();
        assert_eq!(
            got_bytes, want_bytes,
            "{rel} differs from golden (UPDATE_GOLDEN=1 to re-record)"
        );
    }
}

// -- toolchain helpers (same convention as tests/as_vectors.rs) ---------------

fn on_path(name: &str) -> bool {
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&paths).any(|dir| dir.join(name).is_file())
}

/// Command for a node-toolchain program (node/npm/npx): prefer nix (project
/// convention), fall back to the bare binary, and fail loudly if neither
/// exists — the compile check must never be silently skipped.
fn node_cmd(prog: &str) -> Command {
    if on_path("nix") {
        let mut c = Command::new("nix");
        c.args(["shell", "nixpkgs#nodejs", "--command", prog]);
        c
    } else if on_path(prog) {
        Command::new(prog)
    } else {
        panic!(
            "neither `nix` nor `{prog}` is on PATH: cannot compile-check the \
             AssemblyScript backend output (install nix or node; do not skip this test)"
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
             tripwire (install nix or wabt; do not skip this check)"
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

/// The toolchain scratch dir whose node_modules every compile test reuses
/// (shared with tests/as_vectors.rs — one tree on disk, ever).
fn toolchain_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/as-runtime")
}

/// `npm ci` into testdata/as-runtime once per process if the pinned
/// assemblyscript isn't installed yet. Once-guarded: concurrent npm ci runs
/// into the same dir corrupt each other.
fn ensure_toolchain() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let scratch = toolchain_dir();
        let pkg: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(scratch.join("package.json")).unwrap())
                .unwrap();
        let pinned = pkg["devDependencies"]["assemblyscript"]
            .as_str()
            .expect("testdata/as-runtime/package.json pins assemblyscript");
        let installed = scratch.join("node_modules/assemblyscript/package.json");
        let up_to_date = fs::read_to_string(&installed)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .is_some_and(|v| v["version"].as_str() == Some(pinned));
        if up_to_date {
            return;
        }
        let mut cmd = node_cmd("npm");
        cmd.arg("ci").current_dir(&scratch);
        run_ok(cmd, "npm ci (assemblyscript toolchain)");
    });
}

/// Must stay in step with the asc invocation build_sh() emits
/// (src/backend_as.rs) — this test proves exactly the flags the scaffolded
/// script will use.
const ASC_FLAGS: &[&str] = &[
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
];

/// Compile the project's assembly/index.ts with the build.sh flag set,
/// reusing the shared toolchain via a node_modules symlink; returns the
/// wasm bytes for section scans.
fn compile_project(dir: &Path, what: &str) -> Vec<u8> {
    ensure_toolchain();
    let link = dir.join("node_modules");
    if !link.exists() {
        std::os::unix::fs::symlink(toolchain_dir().join("node_modules"), &link)
            .expect("symlink shared node_modules");
    }
    let wasm = dir.join("out.wasm");
    let mut cmd = node_cmd("npx");
    cmd.args(["asc", "assembly/index.ts", "-o"])
        .arg(&wasm)
        .args(ASC_FLAGS)
        .current_dir(dir);
    run_ok(cmd, &format!("asc ({what})"));
    let bytes = fs::read(&wasm).expect("read built wasm");
    assert!(!bytes.is_empty(), "{what}: built wasm is empty");

    // SIMD tripwire on every artifact (asc disables the feature by default;
    // prove it stays that way).
    let mut cmd = wasm_objdump_cmd();
    cmd.arg("-d").arg(&wasm);
    let disasm = run_ok(cmd, &format!("wasm-objdump -d ({what})"));
    assert_simd_free(&disasm, &wasm);
    bytes
}

/// Same anchored checks as the build.sh tripwire / tests/as_vectors.rs.
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
            if !addr.is_empty()
                && addr.chars().all(|c| c.is_ascii_hexdigit())
                && rest.trim_start().starts_with("fd ")
            {
                panic!("0xfd-prefixed (SIMD) opcode in {wasm:?}: {line}");
            }
        }
    }
}

fn has_bytes(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[test]
fn as_full_project_compiles() {
    let tmp = tempfile::tempdir().unwrap();
    let (_module, dir) = generate_full(tmp.path());
    let bytes = compile_project(&dir, "full.wit project");
    // ABI export names must land in the export section.
    for needle in ["crab_alloc", "crab_schema", "crab_invoke"] {
        assert!(
            has_bytes(&bytes, needle),
            "full.wit wasm missing {needle:?} (export section)"
        );
    }
    // asc compiles only what is reachable from the entry file: the
    // scaffolded stubs never call a mesh wrapper, so the wrappers — and
    // mesh.ts's @external crabcraft.call declaration — are tree-shaken
    // away. A module is import-free until its impl actually uses the mesh
    // (verified here; same observable behavior as the C++ lane's linker GC).
    assert!(
        !has_bytes(&bytes, "crabcraft"),
        "stub impls must not pull in the crabcraft.call import"
    );

    // Once an impl calls a wrapper, the import must appear.
    let impl_path = dir.join("assembly/impl.ts");
    let stub = fs::read_to_string(&impl_path).unwrap();
    let needle = "return ResVoid.fail(\"unimplemented: no-result\");";
    assert!(stub.contains(needle), "scaffold drifted:\n{stub}");
    let meshy = stub
        .replace(
            needle,
            "const pong = telemetryPing(\"self\");\n  \
             if (pong.err !== null) return ResVoid.fail(pong.err!);\n  \
             return ResVoid.ok();",
        )
        .replace(
            "} from \"./gen/bindings\";",
            "  telemetryPing,\n} from \"./gen/bindings\";",
        );
    fs::write(&impl_path, meshy).unwrap();
    let bytes = compile_project(&dir, "full.wit project, mesh-calling impl");
    assert!(
        has_bytes(&bytes, "crabcraft"),
        "an impl calling a mesh wrapper must carry the crabcraft.call import"
    );
}

#[test]
fn as_no_imports_project_compiles_without_mesh() {
    // `crabgen new` scaffolds a starter WIT with NO imports: mesh.ts must
    // not be emitted (its @external declaration would land in the import
    // section and make the host require crabcraft.call) and the project
    // must still build.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("guest/solo");
    fs::create_dir_all(dir.join("gen")).unwrap();
    let wit = r#"package crab:solo@0.1.0;

interface api {
  greet: func(name: string) -> string;
}

world solo {
  export api;
}
"#;
    fs::write(dir.join("solo.wit"), wit).unwrap();
    let module = wit::load(&dir.join("solo.wit")).unwrap();
    let backend = backend_for("ts").unwrap();
    backend.generate(&module, &dir).unwrap();
    backend.scaffold(&module, &dir).unwrap();

    assert!(
        !dir.join("assembly/gen/mesh.ts").exists(),
        "assembly/gen/mesh.ts must only be emitted when the world has imports"
    );

    let bytes = compile_project(&dir, "no-imports project");
    for needle in ["crab_alloc", "crab_schema", "crab_invoke"] {
        assert!(
            has_bytes(&bytes, needle),
            "no-imports wasm missing {needle:?}"
        );
    }
    assert!(
        !has_bytes(&bytes, "crabcraft"),
        "import-free module must NOT carry the crabcraft.call import"
    );
}

#[test]
fn as_mesh_ts_removed_when_imports_dropped() {
    // assembly/gen is backend-owned and wiped on regen: a WIT edit that
    // drops the imports must also drop mesh.ts (and the index/bindings must
    // still compile).
    let tmp = tempfile::tempdir().unwrap();
    let (_module, dir) = generate_full(tmp.path());
    assert!(dir.join("assembly/gen/mesh.ts").exists());

    let no_imports = fs::read_to_string(fixture())
        .unwrap()
        .replace("import telemetry;\n", "");
    fs::write(dir.join("full.wit"), no_imports).unwrap();
    let module = wit::load(&dir.join("full.wit")).unwrap();
    let backend = backend_for("ts").unwrap();
    backend.generate(&module, &dir).unwrap();
    assert!(
        !dir.join("assembly/gen/mesh.ts").exists(),
        "mesh.ts must disappear when the WIT loses its imports"
    );
    let bytes = compile_project(&dir, "imports-dropped project");
    assert!(!has_bytes(&bytes, "crabcraft"));
}

#[test]
fn as_missing_impls_empty_after_scaffold() {
    let tmp = tempfile::tempdir().unwrap();
    let (module, dir) = generate_full(tmp.path());
    let backend = backend_for("ts").unwrap();
    let missing = backend.missing_impls(&module, &dir).unwrap();
    assert!(
        missing.is_empty(),
        "scaffolded impl.ts must satisfy every export, missing: {missing:#?}"
    );
}

#[test]
fn as_missing_impls_reports_typed_signatures() {
    let tmp = tempfile::tempdir().unwrap();
    let (module, dir) = generate_full(tmp.path());
    // wipe the stubs: every export is now missing
    fs::write(dir.join("assembly/impl.ts"), "// empty\n").unwrap();
    let backend = backend_for("ts").unwrap();
    let missing = backend.missing_impls(&module, &dir).unwrap();
    assert_eq!(
        missing.len(),
        8,
        "full.wit exports 8 functions: {missing:#?}"
    );
    let all = missing.join("\n");
    assert!(
        all.contains("export function echoEverything(e: Everything): ResEverything"),
        "signatures must be fully typed:\n{all}"
    );
    assert!(
        all.contains("export function tryDivide(num: f64, den: f64): ResF64"),
        "result<f64, string> maps to ResF64:\n{all}"
    );
    assert!(
        all.contains("export function noResult(x: u32): ResVoid"),
        "no-result funcs map to ResVoid:\n{all}"
    );
    assert!(
        all.contains("export function retry(prev: ResultU32Color | null): ResResultU32Color"),
        "typed-E results map to Res<Result<T><E>>:\n{all}"
    );
    assert!(
        all.contains("export function maybeList(xs: Array<u16> | null): ResListOptionBool"),
        "option<list> is nullable, list<option<bool>> uses the OptionBool box:\n{all}"
    );
}

/// Generate (no scaffold) a project from inline WIT at <root>/guest/<name>;
/// returns the result of generate() so error-path tests can inspect it.
fn gen_inline(
    root: &Path,
    name: &str,
    wit: &str,
) -> (crabgen::ir::Module, PathBuf, anyhow::Result<()>) {
    let dir = root.join("guest").join(name);
    fs::create_dir_all(dir.join("gen")).unwrap();
    fs::write(dir.join(format!("{name}.wit")), wit).unwrap();
    let module = wit::load(&dir.join(format!("{name}.wit"))).unwrap();
    let backend = backend_for("ts").unwrap();
    let res = backend.generate(&module, &dir);
    (module, dir, res)
}

#[test]
fn as_record_field_collision_is_an_error() {
    // camelCase lowercases the first segment: `ab` and `%AB` both map to
    // field `ab`. Silently emitting a class with duplicate fields would be
    // broken AS, so generate() must refuse.
    let tmp = tempfile::tempdir().unwrap();
    let wit = r#"package crab:dupfield@0.1.0;

interface api {
  record r {
    ab: u32,
    %AB: u32,
  }

  get-it: func() -> r;
}

world dupfield {
  export api;
}
"#;
    let (_module, _dir, res) = gen_inline(tmp.path(), "dupfield", wit);
    let msg = format!(
        "{:#}",
        res.expect_err("duplicate AS field names must fail generate()")
    );
    assert!(
        msg.contains("ab") && msg.contains("AB"),
        "error must name both WIT fields and the colliding AS field: {msg}"
    );
}

#[test]
fn as_shape_class_collision_is_an_error() {
    // A record named `option-bool` PascalCases to OptionBool — the very
    // class the bindings generate to box option<bool>.
    let tmp = tempfile::tempdir().unwrap();
    let wit = r#"package crab:boxclash@0.1.0;

interface api {
  record option-bool {
    x: u32,
  }

  get-it: func(v: option<bool>) -> option-bool;
}

world boxclash {
  export api;
}
"#;
    let (_module, _dir, res) = gen_inline(tmp.path(), "boxclash", wit);
    let msg = format!(
        "{:#}",
        res.expect_err("WIT type OptionBool must collide with the generated box class")
    );
    assert!(
        msg.contains("OptionBool"),
        "error must name the colliding identifier: {msg}"
    );
}

#[test]
fn as_shape_token_ambiguity_is_an_error() {
    // The shape-class name mangling is not injective: tuple<a-b, c> and
    // tuple<a, b-c> both spell Tuple2ABC. Two DIFFERENT shapes landing on
    // one generated class name must fail generate(), never silently merge
    // (one of the two would get a class with the wrong fields).
    let tmp = tempfile::tempdir().unwrap();
    let wit = r#"package crab:tokclash@0.1.0;

interface api {
  record a-b {
    x: u32,
  }

  record c {
    x: u32,
  }

  record a {
    x: u32,
  }

  record b-c {
    x: u32,
  }

  first: func(v: tuple<a-b, c>) -> u32;
  second: func(v: tuple<a, b-c>) -> u32;
}

world tokclash {
  export api;
}
"#;
    let (_module, _dir, res) = gen_inline(tmp.path(), "tokclash", wit);
    let msg = format!(
        "{:#}",
        res.expect_err("two distinct tuple shapes named Tuple2ABC must fail generate()")
    );
    assert!(
        msg.contains("Tuple2ABC") && msg.contains("rename one"),
        "error must name the ambiguous generated class: {msg}"
    );
}

#[test]
fn as_reserved_name_collision_is_an_error() {
    // `sink` PascalCases to `Sink`, which bindings.ts imports from the
    // runtime template.
    let tmp = tempfile::tempdir().unwrap();
    let wit = r#"package crab:clash@0.1.0;

interface api {
  record sink {
    x: u32,
  }

  get-it: func() -> sink;
}

world clash {
  export api;
}
"#;
    let (_module, _dir, res) = gen_inline(tmp.path(), "clash", wit);
    let msg = format!(
        "{:#}",
        res.expect_err("type `sink` collides with the runtime's Sink")
    );
    assert!(
        msg.contains("Sink") && msg.contains("sink"),
        "error must name both the WIT type and the colliding identifier: {msg}"
    );
}

#[test]
fn as_keyword_param_project_compiles() {
    // Params/fields hitting AS/TS keywords (incl. `constructor` and basic
    // type names like `u32`) get a trailing underscore, and a type ALIAS
    // (absent from full.wit) gets an `export type` + delegating codec
    // helpers; the generated project must actually compile.
    let tmp = tempfile::tempdir().unwrap();
    let wit = r#"package crab:kw@0.1.0;

interface api {
  type ids = list<u32>;

  record opts {
    new: bool,
    %constructor: u32,
    delete: option<string>,
  }

  configure: func(template: opts, class: u32, %u32: string) -> result<string, string>;
  lookup: func(xs: ids) -> ids;
}

world kw {
  export api;
}
"#;
    let dir = tmp.path().join("guest/kw");
    fs::create_dir_all(dir.join("gen")).unwrap();
    fs::write(dir.join("kw.wit"), wit).unwrap();
    let module = wit::load(&dir.join("kw.wit")).unwrap();
    let backend = backend_for("ts").unwrap();
    backend.generate(&module, &dir).expect("generate");
    backend.scaffold(&module, &dir).expect("scaffold");
    compile_project(&dir, "keyword-name project");
}
