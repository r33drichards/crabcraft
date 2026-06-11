//! Golden + compile tests for the Go backend.
//!
//! - `go_golden_full`: generate + scaffold tests/fixtures/full.wit into a
//!   tempdir and compare the produced tree (file SET and byte contents)
//!   against the snapshot at tests/golden/go/full/. Run with UPDATE_GOLDEN=1
//!   to re-record the snapshot — review the diff by hand, it's the contract.
//! - `go_full_project_compiles`: host `go build` + `go vet` + `gofmt -l` on
//!   the generated project INCLUDING the scaffolded impl.go stubs, proving
//!   the emitted code type-checks and is gofmt-clean. (Host go ignores
//!   //go:wasmexport; mesh_wasm.go is wasip1-tagged so the host build skips
//!   it — that's the point.)
//! - `go_missing_impls_*`: the substring scan over impl.go.
//! - `go_name_collision_is_an_error`: a WIT type whose Go name collides with
//!   a reserved bindings identifier fails generate() loudly.

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
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/go/full")
}

/// Generate + scaffold the full.wit project at <root>/guest/full, mirroring
/// the driver: gen/ pre-created, generate(), then scaffold(). Returns the
/// module and the project dir.
fn generate_full(root: &Path) -> (Module, PathBuf) {
    let dir = root.join("guest/full");
    fs::create_dir_all(dir.join("gen")).unwrap();
    fs::copy(fixture(), dir.join("full.wit")).unwrap();
    let module = wit::load(&dir.join("full.wit")).expect("load full.wit");
    let backend = backend_for("go").expect("go backend exists");
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
fn go_golden_full() {
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

// -- toolchain helpers (same convention as tests/go_vectors.rs) --------------

fn on_path(name: &str) -> bool {
    let Some(paths) = env::var_os("PATH") else {
        return false;
    };
    env::split_paths(&paths).any(|dir| dir.join(name).is_file())
}

/// `go <args>` (or gofmt) via nix when available, bare otherwise, FAIL if
/// neither — never silently skip the compile check.
fn go_cmd(tool: &str) -> Command {
    if on_path("nix") {
        let mut c = Command::new("nix");
        c.args(["shell", "nixpkgs#go", "--command", tool]);
        c
    } else if on_path(tool) {
        Command::new(tool)
    } else {
        panic!("neither `nix` nor `{tool}` is on PATH: cannot compile-check the Go backend output");
    }
}

fn run_in(dir: &Path, mut cmd: Command, what: &str) -> String {
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
    String::from_utf8_lossy(&out.stdout).to_string()
}

#[test]
fn go_full_project_compiles() {
    let tmp = tempfile::tempdir().unwrap();
    let (module, dir) = generate_full(tmp.path());
    // the driver writes gen/schema.json (schema.go embeds it); mirror that
    fs::write(dir.join("gen/schema.json"), &module.schema_json).unwrap();

    let mut build = go_cmd("go");
    build.args(["build", "./..."]);
    run_in(&dir, build, "go build");

    // runtime.go's integer->unsafe.Pointer ABI conversions are inherent to
    // wasm linear memory; unsafeptr is the documented exclusion.
    let mut vet = go_cmd("go");
    vet.args(["vet", "-unsafeptr=false", "./..."]);
    run_in(&dir, vet, "go vet");

    let mut fmt = go_cmd("gofmt");
    fmt.args(["-l", "."]);
    let unformatted = run_in(&dir, fmt, "gofmt -l");
    assert!(
        unformatted.trim().is_empty(),
        "generated files are not gofmt-clean:\n{unformatted}"
    );
}

#[test]
fn go_missing_impls_empty_after_scaffold() {
    let tmp = tempfile::tempdir().unwrap();
    let (module, dir) = generate_full(tmp.path());
    let backend = backend_for("go").unwrap();
    let missing = backend.missing_impls(&module, &dir).unwrap();
    assert!(
        missing.is_empty(),
        "scaffolded impl.go must satisfy every export, missing: {missing:#?}"
    );
}

#[test]
fn go_missing_impls_reports_typed_signatures() {
    let tmp = tempfile::tempdir().unwrap();
    let (module, dir) = generate_full(tmp.path());
    // wipe the stubs: every export is now missing
    fs::write(dir.join("impl.go"), "package main\n").unwrap();
    let backend = backend_for("go").unwrap();
    let missing = backend.missing_impls(&module, &dir).unwrap();
    assert_eq!(
        missing.len(),
        8,
        "full.wit exports 8 functions: {missing:#?}"
    );
    let all = missing.join("\n");
    assert!(
        all.contains("func (App) EchoEverything(e gen.Everything) (gen.Everything, error)"),
        "signatures must be fully typed:\n{all}"
    );
    assert!(
        all.contains("func (App) TryDivide(num float64, den float64) (float64, error)"),
        "result<f64, string> maps to (float64, error):\n{all}"
    );
    assert!(
        all.contains("func (App) NoResult(x uint32) error"),
        "no-result funcs map to a bare error return:\n{all}"
    );
    assert!(
        all.contains(
            "func (App) Retry(prev *gen.Result[uint32, gen.Color]) (gen.Result[uint32, gen.Color], error)"
        ),
        "typed-E results map to the generic Result[T, E]:\n{all}"
    );
}

#[test]
fn go_no_imports_project_compiles_without_mesh() {
    // `crabgen new` scaffolds a starter WIT with NO imports: mesh.go,
    // mesh_wasm.go and imports.go must not be emitted (TinyGo fails at
    // instantiation on unused wasm imports) and the project must still
    // build cleanly.
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
    let backend = backend_for("go").unwrap();
    backend.generate(&module, &dir).unwrap();
    backend.scaffold(&module, &dir).unwrap();
    fs::write(dir.join("gen/schema.json"), &module.schema_json).unwrap();

    for absent in [
        "gen/mesh.go",
        "gen/mesh_wasm.go",
        "gen/imports.go",
        "gen/mesh_test.go",
    ] {
        assert!(
            !dir.join(absent).exists(),
            "{absent} must only be emitted when the world has imports"
        );
    }

    let mut build = go_cmd("go");
    build.args(["build", "./..."]);
    run_in(&dir, build, "go build (no-imports project)");

    // The README tells users to run `go test ./...` — it must build and pass
    // even without the mesh files (mesh tests ship only with mesh.go).
    let vectors = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../wit/vectors.json")
        .canonicalize()
        .expect("wit/vectors.json must exist");
    let mut test = go_cmd("go");
    test.args(["test", "./..."]).env("CRAB_VECTORS", &vectors);
    run_in(&dir, test, "go test (no-imports project)");
    let mut fmt = go_cmd("gofmt");
    fmt.args(["-l", "."]);
    let unformatted = run_in(&dir, fmt, "gofmt -l (no-imports project)");
    assert!(
        unformatted.trim().is_empty(),
        "no-imports project is not gofmt-clean:\n{unformatted}"
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
    let backend = backend_for("go").unwrap();
    let res = backend.generate(&module, &dir);
    (module, dir, res)
}

#[test]
fn go_param_named_is_err_compiles() {
    // The mesh wrapper for a result-returning import declares `var isErr
    // bool` in the same scope as the function params, so a WIT param
    // `is-err` (→ lowerCamel `isErr`) must be mangled away from it.
    let tmp = tempfile::tempdir().unwrap();
    let wit = r#"package crab:meshy@0.1.0;

interface api {
  noop: func();
}

interface sender {
  send: func(is-err: bool) -> result<_, string>;
}

world meshy {
  import sender;
  export api;
}
"#;
    let (module, dir, res) = gen_inline(tmp.path(), "meshy", wit);
    res.expect("generate");
    let backend = backend_for("go").unwrap();
    backend.scaffold(&module, &dir).expect("scaffold");
    fs::write(dir.join("gen/schema.json"), &module.schema_json).unwrap();

    let mut build = go_cmd("go");
    build.args(["build", "./..."]);
    run_in(&dir, build, "go build (is-err param project)");
}

#[test]
fn go_record_field_collision_is_an_error() {
    // WIT words are lowercase OR all-caps acronyms: `a-b` and `AB` both
    // Go-case to field `AB`. Silently emitting a struct with duplicate
    // fields would be broken Go, so generate() must refuse.
    let tmp = tempfile::tempdir().unwrap();
    let wit = r#"package crab:dupfield@0.1.0;

interface api {
  record r {
    a-b: u32,
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
        res.expect_err("duplicate Go field names must fail generate()")
    );
    assert!(
        msg.contains("a-b") && msg.contains("AB"),
        "error must name both WIT fields and the colliding Go field: {msg}"
    );
}

#[test]
fn go_variant_case_named_tag_is_an_error() {
    // A payload case named `tag` would emit a `Tag *u32` field colliding
    // with the generated `Tag int` discriminant.
    let tmp = tempfile::tempdir().unwrap();
    let wit = r#"package crab:tagged@0.1.0;

interface api {
  variant v {
    tag(u32),
    other,
  }

  get-it: func() -> v;
}

world tagged {
  export api;
}
"#;
    let (_module, _dir, res) = gen_inline(tmp.path(), "tagged", wit);
    let msg = format!(
        "{:#}",
        res.expect_err("payload case `tag` must fail generate()")
    );
    assert!(
        msg.contains("Tag") && msg.contains("tag"),
        "error must name the case and the generated Tag field: {msg}"
    );
}

#[test]
fn go_name_collision_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("guest/clash");
    fs::create_dir_all(dir.join("gen")).unwrap();
    let wit = r#"package crab:clash@0.1.0;

interface api {
  record impl {
    x: u32,
  }

  get-it: func() -> impl;
}

world clash {
  export api;
}
"#;
    fs::write(dir.join("clash.wit"), wit).unwrap();
    let module = wit::load(&dir.join("clash.wit")).unwrap();
    let backend = backend_for("go").unwrap();
    let err = backend
        .generate(&module, &dir)
        .expect_err("type `impl` Go-cases to `Impl`, which the bindings reserve");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("Impl") && msg.contains("impl"),
        "error must name both the WIT type and the colliding Go identifier: {msg}"
    );
}
