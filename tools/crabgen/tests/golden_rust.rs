//! Golden + compile tests for the Rust backend.
//!
//! - `rust_golden_full`: generate + scaffold tests/fixtures/full.wit into a
//!   tempdir and compare the produced tree (file SET and byte contents)
//!   against the snapshot at tests/golden/rust/full/. Run with
//!   UPDATE_GOLDEN=1 to re-record — review the diff by hand, it's the
//!   contract.
//! - `rust_full_project_compiles`: host `cargo check` on the generated
//!   project INCLUDING the scaffolded src/app.rs stubs, proving the emitted
//!   code type-checks (codec correctness is crab-sdk's already-tested job —
//!   it passes wit/vectors.json — so no new vectors test here). The tempdir
//!   project is detached from any workspace with an appended `[workspace]`
//!   table and its crab-sdk path dep rewritten to the REAL guest/crab-sdk
//!   (absolute path; the scaffolded relative ../crab-sdk only resolves in
//!   the repo layout).
//! - `rust_missing_impls_*`: the substring scan over src/app.rs.
//! - collision tests: record-field dup, variant-case dup, param dup, a WIT
//!   type landing on a reserved identifier — all fail generate() loudly.

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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/rust/full")
}

fn real_crab_sdk() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../guest/crab-sdk")
        .canonicalize()
        .expect("guest/crab-sdk must exist")
}

/// Generate + scaffold the full.wit project at <root>/guest/full, mirroring
/// the driver: gen/ pre-created, generate(), then scaffold(). scaffold()
/// edits the root workspace members, so a minimal <root>/Cargo.toml is laid
/// down first.
fn generate_full(root: &Path) -> (Module, PathBuf) {
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nresolver = \"2\"\nmembers = [\"guest/crab-sdk\"]\n",
    )
    .unwrap();
    let dir = root.join("guest/full");
    fs::create_dir_all(dir.join("gen")).unwrap();
    fs::copy(fixture(), dir.join("full.wit")).unwrap();
    let module = wit::load(&dir.join("full.wit")).expect("load full.wit");
    let backend = backend_for("rust").expect("rust backend exists");
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
fn rust_golden_full() {
    let tmp = tempfile::tempdir().unwrap();
    let (_m, dir) = generate_full(tmp.path());

    // build.sh must be executable regardless of snapshot mode
    let mode = fs::metadata(dir.join("build.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "build.sh must carry the executable bit");

    // the workspace edit must have registered the project
    let root_cargo = fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap();
    assert!(
        root_cargo.contains("\"guest/full\""),
        "scaffold must add the crate to the root workspace members: {root_cargo}"
    );

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

// -- toolchain helpers --------------------------------------------------------

fn on_path(name: &str) -> bool {
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&paths).any(|dir| dir.join(name).is_file())
}

/// The cargo running this test suite (env CARGO), falling back to PATH —
/// never silently skip the compile check.
fn cargo_cmd() -> Command {
    if let Some(c) = env::var_os("CARGO") {
        Command::new(c)
    } else if on_path("cargo") {
        Command::new("cargo")
    } else {
        panic!("neither $CARGO nor a PATH cargo is available: cannot compile-check the Rust backend output");
    }
}

fn run_in(dir: &Path, mut cmd: Command, what: &str) -> (String, String) {
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
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

/// Detach the tempdir project from any workspace and point its crab-sdk
/// path dep at the real one, then write the schema the driver would have.
fn make_checkable(module: &Module, dir: &Path) {
    fs::write(dir.join("gen/schema.json"), &module.schema_json).unwrap();
    let cargo = dir.join("Cargo.toml");
    let src = fs::read_to_string(&cargo).unwrap();
    let src = src.replace(
        "\"../crab-sdk\"",
        &format!("\"{}\"", real_crab_sdk().display()),
    );
    fs::write(&cargo, format!("{src}\n[workspace]\n")).unwrap();
}

#[test]
fn rust_full_project_compiles() {
    let tmp = tempfile::tempdir().unwrap();
    let (module, dir) = generate_full(tmp.path());
    make_checkable(&module, &dir);

    let mut check = cargo_cmd();
    check.arg("check");
    let (_, stderr) = run_in(&dir, check, "cargo check");
    assert!(
        !stderr.contains("warning"),
        "generated project must compile warning-free:\n{stderr}"
    );
}

#[test]
fn rust_missing_impls_empty_after_scaffold() {
    let tmp = tempfile::tempdir().unwrap();
    let (module, dir) = generate_full(tmp.path());
    let backend = backend_for("rust").unwrap();
    let missing = backend.missing_impls(&module, &dir).unwrap();
    assert!(
        missing.is_empty(),
        "scaffolded src/app.rs must satisfy every export, missing: {missing:#?}"
    );
}

#[test]
fn rust_missing_impls_reports_typed_signatures() {
    let tmp = tempfile::tempdir().unwrap();
    let (module, dir) = generate_full(tmp.path());
    // wipe the stubs: every export is now missing
    fs::write(dir.join("src/app.rs"), "pub struct App;\n").unwrap();
    let backend = backend_for("rust").unwrap();
    let missing = backend.missing_impls(&module, &dir).unwrap();
    assert_eq!(missing.len(), 8, "full.wit exports 8 functions: {missing:#?}");
    let all = missing.join("\n");
    assert!(
        all.contains(
            "fn echo_everything(&self, e: gen::Everything) -> Result<gen::Everything, String>"
        ),
        "signatures must be fully typed:\n{all}"
    );
    assert!(
        all.contains("fn try_divide(&self, num: f64, den: f64) -> Result<f64, String>"),
        "result<f64, string> maps to the method's own Result<f64, String>:\n{all}"
    );
    assert!(
        all.contains("fn no_result(&self, x: u32) -> Result<(), String>"),
        "no-result funcs map to Result<(), String>:\n{all}"
    );
    assert!(
        all.contains(
            "fn retry(&self, prev: Option<Result<u32, gen::Color>>) -> Result<Result<u32, gen::Color>, String>"
        ),
        "typed-E results nest: Result<Result<T, E>, String>:\n{all}"
    );
}

/// Generate (no scaffold) a project from inline WIT at <root>/guest/<name>;
/// returns generate()'s result so error-path tests can inspect it.
fn gen_inline(
    root: &Path,
    name: &str,
    wit_src: &str,
) -> (crabgen::ir::Module, PathBuf, anyhow::Result<()>) {
    let dir = root.join("guest").join(name);
    fs::create_dir_all(dir.join("gen")).unwrap();
    fs::write(dir.join(format!("{name}.wit")), wit_src).unwrap();
    let module = wit::load(&dir.join(format!("{name}.wit"))).unwrap();
    let backend = backend_for("rust").unwrap();
    let res = backend.generate(&module, &dir);
    (module, dir, res)
}

#[test]
fn rust_record_field_collision_is_an_error() {
    // WIT words are lowercase OR all-caps acronyms: `ab` and `AB` both
    // snake_case to field `ab`. Silently emitting a struct with duplicate
    // fields would be broken Rust, so generate() must refuse.
    let tmp = tempfile::tempdir().unwrap();
    let wit_src = r#"package crab:dupfield@0.1.0;

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
    let (_module, _dir, res) = gen_inline(tmp.path(), "dupfield", wit_src);
    let msg = format!(
        "{:#}",
        res.expect_err("duplicate Rust field names must fail generate()")
    );
    assert!(
        msg.contains("ab") && msg.contains("AB"),
        "error must name both WIT fields and the colliding Rust field: {msg}"
    );
}

#[test]
fn rust_variant_case_collision_is_an_error() {
    // `a-b` and `%AB` both PascalCase to variant `AB`.
    let tmp = tempfile::tempdir().unwrap();
    let wit_src = r#"package crab:dupcase@0.1.0;

interface api {
  variant v {
    a-b(u32),
    %AB,
  }

  get-it: func() -> v;
}

world dupcase {
  export api;
}
"#;
    let (_module, _dir, res) = gen_inline(tmp.path(), "dupcase", wit_src);
    let msg = format!(
        "{:#}",
        res.expect_err("duplicate Rust variant names must fail generate()")
    );
    assert!(
        msg.contains("a-b") && msg.contains("AB"),
        "error must name both WIT cases and the colliding Rust variant: {msg}"
    );
}

#[test]
fn rust_param_collision_is_an_error() {
    // `ab` and `%AB` both snake_case to param `ab`.
    let tmp = tempfile::tempdir().unwrap();
    let wit_src = r#"package crab:dupparam@0.1.0;

interface api {
  f: func(ab: u32, %AB: u32);
}

world dupparam {
  export api;
}
"#;
    let (_module, _dir, res) = gen_inline(tmp.path(), "dupparam", wit_src);
    let msg = format!(
        "{:#}",
        res.expect_err("duplicate Rust param names must fail generate()")
    );
    assert!(
        msg.contains("ab") && msg.contains("AB"),
        "error must name both WIT params: {msg}"
    );
}

#[test]
fn rust_reserved_name_collision_is_an_error() {
    // A WIT type named `value` Pascals to `Value`, which the bindings import
    // from crab-sdk.
    let tmp = tempfile::tempdir().unwrap();
    let wit_src = r#"package crab:clash@0.1.0;

interface api {
  record value {
    x: u32,
  }

  get-it: func() -> value;
}

world clash {
  export api;
}
"#;
    let (_module, _dir, res) = gen_inline(tmp.path(), "clash", wit_src);
    let msg = format!(
        "{:#}",
        res.expect_err("type `value` maps to `Value`, which the bindings reserve")
    );
    assert!(
        msg.contains("Value") && msg.contains("value"),
        "error must name both the WIT type and the colliding Rust identifier: {msg}"
    );
}

#[test]
fn rust_param_named_workload_is_mangled_in_mesh_wrappers() {
    // The mesh wrapper's first parameter is `workload: &str`, sharing a
    // scope with the WIT params — a WIT param `workload` must be mangled.
    let tmp = tempfile::tempdir().unwrap();
    let wit_src = r#"package crab:meshy@0.1.0;

interface api {
  noop: func();
}

interface sender {
  send: func(workload: string) -> result<_, string>;
}

world meshy {
  import sender;
  export api;
}
"#;
    let (_module, dir, res) = gen_inline(tmp.path(), "meshy", wit_src);
    res.expect("generate");
    let gen_src = fs::read_to_string(dir.join("src/gen/mod.rs")).unwrap();
    assert!(
        gen_src.contains("pub fn sender_send(workload: &str, workload_: String)"),
        "WIT param `workload` must be mangled away from the wrapper's own param:\n{gen_src}"
    );
}

#[test]
fn rust_regen_with_imports_demands_mesh_feature() {
    // generate() must refuse to emit mesh wrappers when the scaffolded
    // Cargo.toml (written before the WIT gained imports) lacks the crab-sdk
    // `mesh` feature — emitted code wouldn't compile.
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("guest/late");
    fs::create_dir_all(dir.join("gen")).unwrap();
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"late\"\n\n[dependencies]\ncrab-sdk = { path = \"../crab-sdk\" }\n",
    )
    .unwrap();
    let wit_src = r#"package crab:late@0.1.0;

interface api {
  noop: func();
}

interface sender {
  send: func(msg: string);
}

world late {
  import sender;
  export api;
}
"#;
    fs::write(dir.join("late.wit"), wit_src).unwrap();
    let module = wit::load(&dir.join("late.wit")).unwrap();
    let backend = backend_for("rust").unwrap();
    let msg = format!(
        "{:#}",
        backend
            .generate(&module, &dir)
            .expect_err("imports without the mesh feature must fail generate()")
    );
    assert!(
        msg.contains("mesh"),
        "error must say how to enable the mesh feature: {msg}"
    );
}
