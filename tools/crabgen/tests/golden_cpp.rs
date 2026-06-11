//! Golden + compile tests for the C++ backend.
//!
//! - `cpp_golden_full`: generate + scaffold tests/fixtures/full.wit into a
//!   tempdir and compare the produced tree (file SET and byte contents)
//!   against the snapshot at tests/golden/cpp/full/. Run with UPDATE_GOLDEN=1
//!   to re-record the snapshot — review the diff by hand, it's the contract.
//! - `cpp_full_project_compiles`: NATIVE `zig c++ -c` over every TU (fast
//!   type-check of bindings + scaffolded stubs), then the full wasm32-wasi
//!   REACTOR link with the build.sh flag set, asserting the crab_* export
//!   names land in the binary — and that the crabcraft import appears
//!   exactly when an impl actually calls a mesh wrapper (wasm-ld
//!   garbage-collects unused wrappers, keeping stub modules import-free).
//! - `cpp_no_imports_project_compiles_without_mesh`: an import-free WIT must
//!   produce no mesh.{hpp,cpp} and a wasm with NO crabcraft import.
//! - `cpp_missing_impls_*`: the substring scan over impl.cpp.
//! - collision tests: duplicate record fields, a variant case struct
//!   colliding with another type, and a reserved bindings identifier all
//!   fail generate() loudly.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crabgen::backend::backend_for;
use crabgen::ir::Module;
use crabgen::wit;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/full.wit")
}

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/cpp/full")
}

/// Generate + scaffold the full.wit project at <root>/guest/full, mirroring
/// the driver: gen/ pre-created, generate(), then scaffold(). Returns the
/// module and the project dir.
fn generate_full(root: &Path) -> (Module, PathBuf) {
    let dir = root.join("guest/full");
    fs::create_dir_all(dir.join("gen")).unwrap();
    fs::copy(fixture(), dir.join("full.wit")).unwrap();
    let module = wit::load(&dir.join("full.wit")).expect("load full.wit");
    let backend = backend_for("cpp").expect("cpp backend exists");
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
fn cpp_golden_full() {
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

// -- toolchain helpers (same convention as tests/cpp_vectors.rs) --------------

fn on_path(name: &str) -> bool {
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&paths).any(|dir| dir.join(name).is_file())
}

/// `zig` via nix when available, bare otherwise, FAIL if neither — never
/// silently skip the compile check.
fn zig_cmd() -> Command {
    if on_path("nix") {
        let mut c = Command::new("nix");
        c.args(["shell", "nixpkgs#zig", "--command", "zig"]);
        c
    } else if on_path("zig") {
        Command::new("zig")
    } else {
        panic!("neither `nix` nor `zig` is on PATH: cannot compile-check the C++ backend output");
    }
}

/// Persistent zig cache under target/tmp so libc++ is not rebuilt per run
/// (shared with tests/cpp_vectors.rs's wasm smoke).
fn zig_cache(cmd: &mut Command) {
    let cache = Path::new(env!("CARGO_TARGET_TMPDIR")).join("zig-cache");
    fs::create_dir_all(&cache).expect("create zig cache dir");
    cmd.env("ZIG_GLOBAL_CACHE_DIR", &cache)
        .env("ZIG_LOCAL_CACHE_DIR", &cache);
}

fn run_in(dir: &Path, mut cmd: Command, what: &str) {
    let out = cmd
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("spawn {what}: {e}"));
    assert!(
        out.status.success(),
        "{what} failed ({}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Must stay in step with the zig invocation build_sh() emits
/// (src/backend_cpp.rs) — this test proves exactly the flags the scaffolded
/// script will use.
const WASM_FLAGS: &[&str] = &[
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
];

/// Every .cpp TU of a generated project (impl.cpp + gen/*.cpp, like build.sh
/// globs them), project-relative.
fn project_tus(dir: &Path) -> Vec<String> {
    let mut tus = vec!["impl.cpp".to_string()];
    let mut gens: Vec<String> = fs::read_dir(dir.join("gen"))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
        .filter(|n| n.ends_with(".cpp"))
        .map(|n| format!("gen/{n}"))
        .collect();
    gens.sort();
    tus.extend(gens);
    tus
}

/// Native fast type-check + the full wasm reactor link; returns the wasm
/// bytes for section scans.
///
/// Serialized: concurrent `zig c++` invocations against ONE shared cache
/// deadlock while racing to build libc++ (observed: all parked at 0% CPU
/// indefinitely), and per-test caches would rebuild libc++ N times. One
/// compile at a time warms the shared cache once and stays warm.
fn compile_project(dir: &Path, what: &str) -> Vec<u8> {
    static ZIG_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _serial = ZIG_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    let tus = project_tus(dir);

    // 1. native object compile: catches type errors fast, per TU, without
    //    needing a main() to link against (`zig c++ -fsyntax-only` is broken
    //    upstream — it reports FileNotFound for every input — so -c it is).
    //    Generated code AND the crab/mesh templates are held to
    //    -Wall -Wextra -Werror; the scaffolded impl.cpp is exempt (its stubs
    //    carry named-but-unused params on purpose — the file is user-owned
    //    and the stub bodies are transient).
    let (impl_tus, gen_tus): (Vec<_>, Vec<_>) = tus.iter().partition(|t| !t.starts_with("gen/"));
    let mut strict = zig_cmd();
    zig_cache(&mut strict);
    strict
        .args(["c++", "-std=c++17", "-fno-exceptions", "-fno-rtti"])
        .args(["-Wall", "-Wextra", "-Werror", "-c"])
        .args(&gen_tus);
    run_in(
        dir,
        strict,
        &format!("zig c++ -c native strict gen/ ({what})"),
    );
    let mut check = zig_cmd();
    zig_cache(&mut check);
    check
        .args(["c++", "-std=c++17", "-fno-exceptions", "-fno-rtti", "-c"])
        .args(&impl_tus);
    run_in(dir, check, &format!("zig c++ -c native impl ({what})"));

    // 2. the wasm reactor build with the exact build.sh flag set (full link:
    //    a missing impl:: definition fails HERE, naming the symbol)
    let wasm = dir.join("out.wasm");
    let mut build = zig_cmd();
    zig_cache(&mut build);
    build.args(WASM_FLAGS).arg("-o").arg(&wasm).args(&tus);
    run_in(dir, build, &format!("zig c++ wasm reactor ({what})"));
    let bytes = fs::read(&wasm).expect("read built wasm");
    assert!(!bytes.is_empty(), "{what}: built wasm is empty");
    bytes
}

fn has_bytes(haystack: &[u8], needle: &str) -> bool {
    haystack
        .windows(needle.len())
        .any(|w| w == needle.as_bytes())
}

#[test]
fn cpp_full_project_compiles() {
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
    // The scaffolded stubs never call the mesh wrappers, so wasm-ld
    // garbage-collects them AND the crabcraft.call import: a module is
    // import-free until its impl actually uses the mesh (the host then
    // doesn't have to provide the import).
    assert!(
        !has_bytes(&bytes, "crabcraft"),
        "stub impls must not pull in the crabcraft.call import"
    );

    // Once an impl calls a wrapper, the import must appear.
    let impl_path = dir.join("impl.cpp");
    let stub = fs::read_to_string(&impl_path).unwrap();
    let needle = "return crab::Res<std::monostate>::fail(\"unimplemented: no-result\");";
    assert!(stub.contains(needle), "scaffold drifted:\n{stub}");
    let meshy = stub.replace(
        needle,
        "auto pong = gen::telemetry_ping(\"self\");\n  \
         if (!pong.ok()) return crab::Res<std::monostate>::fail(std::move(pong.err));\n  \
         return {std::monostate{}, {}};",
    );
    fs::write(&impl_path, meshy).unwrap();
    let bytes = compile_project(&dir, "full.wit project, mesh-calling impl");
    assert!(
        has_bytes(&bytes, "crabcraft"),
        "an impl calling a mesh wrapper must carry the crabcraft.call import"
    );
}

#[test]
fn cpp_no_imports_project_compiles_without_mesh() {
    // `crabgen new` scaffolds a starter WIT with NO imports: mesh.hpp and
    // mesh.cpp must not be emitted (a compiled-in crabcraft.call import
    // would make the host require it) and the project must still build.
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
    let backend = backend_for("cpp").unwrap();
    backend.generate(&module, &dir).unwrap();
    backend.scaffold(&module, &dir).unwrap();

    for absent in ["gen/mesh.hpp", "gen/mesh.cpp"] {
        assert!(
            !dir.join(absent).exists(),
            "{absent} must only be emitted when the world has imports"
        );
    }

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
fn cpp_missing_impls_empty_after_scaffold() {
    let tmp = tempfile::tempdir().unwrap();
    let (module, dir) = generate_full(tmp.path());
    let backend = backend_for("cpp").unwrap();
    let missing = backend.missing_impls(&module, &dir).unwrap();
    assert!(
        missing.is_empty(),
        "scaffolded impl.cpp must satisfy every export, missing: {missing:#?}"
    );
}

#[test]
fn cpp_missing_impls_reports_typed_signatures() {
    let tmp = tempfile::tempdir().unwrap();
    let (module, dir) = generate_full(tmp.path());
    // wipe the stubs: every export is now missing
    fs::write(dir.join("impl.cpp"), "#include \"gen/bindings.hpp\"\n").unwrap();
    let backend = backend_for("cpp").unwrap();
    let missing = backend.missing_impls(&module, &dir).unwrap();
    assert_eq!(
        missing.len(),
        8,
        "full.wit exports 8 functions: {missing:#?}"
    );
    let all = missing.join("\n");
    assert!(
        all.contains("crab::Res<gen::Everything> echo_everything(gen::Everything e)"),
        "signatures must be fully typed:\n{all}"
    );
    assert!(
        all.contains("crab::Res<double> try_divide(double num, double den)"),
        "result<f64, string> maps to crab::Res<double>:\n{all}"
    );
    assert!(
        all.contains("crab::Res<std::monostate> no_result(uint32_t x)"),
        "no-result funcs map to crab::Res<std::monostate>:\n{all}"
    );
    assert!(
        all.contains(
            "crab::Res<gen::Result<uint32_t, gen::Color>> retry(std::optional<gen::Result<uint32_t, gen::Color>> prev)"
        ),
        "typed-E results map to gen::Result<T, E>:\n{all}"
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
    let backend = backend_for("cpp").unwrap();
    let res = backend.generate(&module, &dir);
    (module, dir, res)
}

#[test]
fn cpp_readme_refreshes_on_regen_after_wit_swap() {
    // The canonical flow is `new` (starter WIT) -> replace the WIT -> regen.
    // README.md quotes the export instance, so it must be REGENERATED on
    // every generate(), not written once.
    let v1 = r#"package crab:swap@0.1.0;

interface api {
  greet: func(name: string) -> string;
}

world swap {
  export api;
}
"#;
    let tmp = tempfile::tempdir().unwrap();
    let (module, dir, res) = gen_inline(tmp.path(), "swap", v1);
    res.unwrap();
    let backend = backend_for("cpp").unwrap();
    backend.scaffold(&module, &dir).unwrap();
    let readme = fs::read_to_string(dir.join("README.md")).unwrap();
    assert!(
        readme.contains("crab:swap/api@0.1.0"),
        "README names the initial instance:\n{readme}"
    );

    let v2 = v1.replace("api", "greeter");
    fs::write(dir.join("swap.wit"), &v2).unwrap();
    let module = wit::load(&dir.join("swap.wit")).unwrap();
    backend.generate(&module, &dir).unwrap();
    let readme = fs::read_to_string(dir.join("README.md")).unwrap();
    assert!(
        readme.contains("crab:swap/greeter@0.1.0") && !readme.contains("crab:swap/api@0.1.0"),
        "README is stale after a WIT swap + regen:\n{readme}"
    );
}

#[test]
fn cpp_record_field_collision_is_an_error() {
    // snake_case lowercases every segment: `ab` and `%AB` both map to field
    // `ab`. Silently emitting a struct with duplicate members would be
    // broken C++, so generate() must refuse.
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
        res.expect_err("duplicate C++ field names must fail generate()")
    );
    assert!(
        msg.contains("ab") && msg.contains("AB"),
        "error must name both WIT fields and the colliding C++ field: {msg}"
    );
}

#[test]
fn cpp_variant_case_struct_collision_is_an_error() {
    // A payload case `circle` of variant `v` emits `struct VCircle`; a
    // sibling type `v-circle` PascalCases to the same name.
    let tmp = tempfile::tempdir().unwrap();
    let wit = r#"package crab:vclash@0.1.0;

interface api {
  record v-circle {
    radius: f32,
  }

  variant v {
    circle(u32),
    other,
  }

  get-it: func() -> v;
}

world vclash {
  export api;
}
"#;
    let (_module, _dir, res) = gen_inline(tmp.path(), "vclash", wit);
    let msg = format!(
        "{:#}",
        res.expect_err("case struct VCircle must collide with type v-circle")
    );
    assert!(
        msg.contains("VCircle"),
        "error must name the colliding C++ identifier: {msg}"
    );
}

#[test]
fn cpp_reserved_name_collision_is_an_error() {
    // `registration` PascalCases to `Registration`, which bindings.cpp
    // declares for the handler-registering static object.
    let tmp = tempfile::tempdir().unwrap();
    let wit = r#"package crab:clash@0.1.0;

interface api {
  record registration {
    x: u32,
  }

  get-it: func() -> registration;
}

world clash {
  export api;
}
"#;
    let (_module, _dir, res) = gen_inline(tmp.path(), "clash", wit);
    let msg = format!(
        "{:#}",
        res.expect_err("type `registration` collides with the generated Registration")
    );
    assert!(
        msg.contains("Registration") && msg.contains("registration"),
        "error must name both the WIT type and the colliding C++ identifier: {msg}"
    );
}

#[test]
fn cpp_keyword_param_project_compiles() {
    // Params/fields hitting C++ keywords get a trailing underscore, and a
    // type ALIAS (absent from full.wit) gets a `using` + delegating codec
    // helpers; the generated project must actually compile.
    let tmp = tempfile::tempdir().unwrap();
    let wit = r#"package crab:kw@0.1.0;

interface api {
  type ids = list<u32>;

  record opts {
    new: bool,
    delete: option<string>,
  }

  configure: func(template: opts, class: u32) -> result<string, string>;
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
    let backend = backend_for("cpp").unwrap();
    backend.generate(&module, &dir).expect("generate");
    backend.scaffold(&module, &dir).expect("scaffold");
    compile_project(&dir, "keyword-name project");
}
