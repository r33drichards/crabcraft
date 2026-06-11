//! MANIFEST freshness, project discovery, and the check/regen/new drivers.
//!
//! CLI-level tests spawn the real binary (CARGO_BIN_EXE_crabgen) inside a
//! tempdir laid out like the repo: a `Cargo.toml` + `guest/` at the root
//! (that pair is how crabgen finds the repo root, walking up from cwd).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use crabgen::manifest::{sha256_hex, Manifest};
use crabgen::project;
use tempfile::TempDir;

/// A valid WIT that wit::load accepts (one exported interface).
/// Func names chosen so neither is a substring of regen's output headers.
const VALID_WIT: &str = "\
package crab:x@0.1.0;

interface api {
  greet: func(name: string) -> string;
  summon: func(count: u32) -> u32;
}

world x {
  export api;
}
";

fn make_repo() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    fs::write(tmp.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    fs::create_dir(tmp.path().join("guest")).unwrap();
    tmp
}

/// guest/<name>/<name>.wit + gen/MANIFEST with a matching hash.
fn add_project(root: &Path, name: &str, lang: &str, wit: &str) -> PathBuf {
    let dir = root.join("guest").join(name);
    fs::create_dir_all(dir.join("gen")).unwrap();
    fs::write(dir.join(format!("{name}.wit")), wit).unwrap();
    fs::write(
        dir.join("gen/MANIFEST"),
        Manifest::new(lang, wit.as_bytes()).render(),
    )
    .unwrap();
    dir
}

fn crabgen(root: &Path, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_crabgen"))
        .args(args)
        .current_dir(root)
        .output()
        .expect("spawn crabgen")
}

/// Like `crabgen`, with extra env vars set on the spawned binary.
fn crabgen_env(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_crabgen"))
        .args(args)
        .envs(env.iter().copied())
        .current_dir(root)
        .output()
        .expect("spawn crabgen")
}

/// stdout + stderr combined: tests assert on what the user sees, not channels.
fn all_output(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

// ---------------------------------------------------------------- manifest

#[test]
fn manifest_renders_exact_three_line_format() {
    let m = Manifest::new("go", b"hello");
    assert_eq!(
        m.render(),
        format!(
            "crabgen {}\nlang go\nwit-sha256 {}\n",
            env!("CARGO_PKG_VERSION"),
            // sha256("hello")
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        )
    );
}

#[test]
fn manifest_roundtrips_through_parse() {
    let m = Manifest::new("rust", b"some wit bytes");
    let parsed = Manifest::parse(&m.render()).expect("parse rendered manifest");
    assert_eq!(parsed, m);
    assert!(parsed.is_fresh(b"some wit bytes"));
    assert!(!parsed.is_fresh(b"mutated wit bytes"));
}

#[test]
fn manifest_parse_rejects_malformed_input() {
    for bad in [
        "",
        "crabgen 0.1.0\n",
        "crabgen 0.1.0\nlang go\n",
        "lang go\ncrabgen 0.1.0\nwit-sha256 abc\n",
        "crabgen 0.1.0\nlang go\nwit-sha256 abc\nextra line\n",
        "crabgen0.1.0\nlang go\nwit-sha256 abc\n",
    ] {
        assert!(
            Manifest::parse(bad).is_err(),
            "should reject malformed manifest {bad:?}"
        );
    }
}

// --------------------------------------------------------------- discovery

#[test]
fn discover_finds_managed_projects_and_ignores_strays() {
    let tmp = make_repo();
    let root = tmp.path();
    add_project(root, "x", "go", VALID_WIT);
    // strays like the real repo's guest/Untitled: no .wit, no MANIFEST
    fs::create_dir_all(root.join("guest/untitled")).unwrap();
    fs::write(root.join("guest/untitled/main.go"), "package main").unwrap();
    // hand-written guest: .wit but no MANIFEST → not crabgen-managed
    fs::create_dir_all(root.join("guest/handmade")).unwrap();
    fs::write(root.join("guest/handmade/handmade.wit"), VALID_WIT).unwrap();
    // gen/MANIFEST but the .wit was deleted → not a project either
    fs::create_dir_all(root.join("guest/witless/gen")).unwrap();
    fs::write(
        root.join("guest/witless/gen/MANIFEST"),
        Manifest::new("go", b"x").render(),
    )
    .unwrap();
    // a plain file directly under guest/ must not trip the walker
    fs::write(root.join("guest/notes.txt"), "hi").unwrap();

    let projects = project::discover(root).expect("discover");
    assert_eq!(projects.len(), 1, "exactly one managed project");
    let p = &projects[0];
    assert_eq!(p.dir, root.join("guest/x"));
    assert_eq!(p.rel, "guest/x");
    assert_eq!(p.wit_path, root.join("guest/x/x.wit"));
    assert_eq!(p.manifest.lang, "go");
}

#[test]
fn discover_errors_on_multiple_wit_files() {
    let tmp = make_repo();
    let root = tmp.path();
    let dir = add_project(root, "dup", "go", VALID_WIT);
    fs::write(dir.join("second.wit"), VALID_WIT).unwrap();

    let err = project::discover(root).expect_err("two .wit files must be ambiguous");
    let msg = format!("{err:#}");
    assert!(msg.contains("guest/dup"), "error names the project: {msg}");
}

// ------------------------------------------------------------------- check

#[test]
fn check_passes_when_manifest_matches_wit() {
    let tmp = make_repo();
    add_project(tmp.path(), "x", "go", VALID_WIT);

    let out = crabgen(tmp.path(), &["check"]);
    assert!(
        out.status.success(),
        "check should pass: {}",
        all_output(&out)
    );
}

#[test]
fn check_fails_listing_stale_project_and_suggests_regen() {
    let tmp = make_repo();
    let dir = add_project(tmp.path(), "x", "go", VALID_WIT);
    // mutate the WIT after the MANIFEST was written
    fs::write(
        dir.join("x.wit"),
        format!("{VALID_WIT}\n// a trailing comment\n"),
    )
    .unwrap();

    let out = crabgen(tmp.path(), &["check"]);
    assert!(!out.status.success(), "check must fail on a stale project");
    let text = all_output(&out);
    assert!(text.contains("guest/x"), "names the stale project: {text}");
    assert!(text.contains("crabgen regen"), "suggests regen: {text}");
}

#[test]
fn check_lists_every_stale_project() {
    let tmp = make_repo();
    for name in ["aaa", "bbb"] {
        let dir = add_project(tmp.path(), name, "go", VALID_WIT);
        fs::write(dir.join(format!("{name}.wit")), "// changed\n").unwrap();
    }
    add_project(tmp.path(), "fresh", "go", VALID_WIT);

    let out = crabgen(tmp.path(), &["check"]);
    assert!(!out.status.success());
    let text = all_output(&out);
    assert!(
        text.contains("guest/aaa") && text.contains("guest/bbb"),
        "{text}"
    );
    assert!(
        !text.contains("guest/fresh"),
        "fresh project not listed: {text}"
    );
}

#[test]
fn check_succeeds_quietly_when_no_projects_exist() {
    let tmp = make_repo();
    // only strays, like the repo's existing hand-written guests
    fs::create_dir_all(tmp.path().join("guest/hello-go")).unwrap();
    fs::write(tmp.path().join("guest/hello-go/main.go"), "package main").unwrap();

    let out = crabgen(tmp.path(), &["check"]);
    assert!(
        out.status.success(),
        "no projects → success: {}",
        all_output(&out)
    );
    assert!(
        out.stdout.is_empty() && out.stderr.is_empty(),
        "quietly means no output, got: {}",
        all_output(&out)
    );
}

// ------------------------------------------------------------------- regen

#[test]
fn regen_rewrites_gen_wholesale_and_freshens_manifest() {
    let tmp = make_repo();
    let dir = add_project(tmp.path(), "x", "test", VALID_WIT);
    // stale: WIT mutated; junk in gen/ that a wholesale rewrite must remove
    let new_wit = format!("{VALID_WIT}\n// v2\n");
    fs::write(dir.join("x.wit"), &new_wit).unwrap();
    fs::write(dir.join("gen/leftover.txt"), "junk from an old crabgen").unwrap();
    assert!(!crabgen(tmp.path(), &["check"]).status.success());

    let out = crabgen(tmp.path(), &["regen", "guest/x"]);
    assert!(out.status.success(), "regen failed: {}", all_output(&out));

    assert!(
        !dir.join("gen/leftover.txt").exists(),
        "gen/ rewritten wholesale"
    );
    assert!(
        dir.join("gen/GENERATED").exists(),
        "test backend marker written"
    );
    let schema: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.join("gen/schema.json")).unwrap())
            .expect("schema.json is valid JSON");
    assert!(
        schema.get("worlds").is_some(),
        "schema.json is a resolved WIT"
    );
    let m = Manifest::parse(&fs::read_to_string(dir.join("gen/MANIFEST")).unwrap()).unwrap();
    assert_eq!(m.lang, "test");
    assert_eq!(m.wit_sha256, sha256_hex(new_wit.as_bytes()));

    let out = crabgen(tmp.path(), &["check"]);
    assert!(
        out.status.success(),
        "check passes after regen: {}",
        all_output(&out)
    );
}

#[test]
fn regen_reports_missing_impls_without_touching_impl_file() {
    let tmp = make_repo();
    let dir = add_project(tmp.path(), "x", "test", VALID_WIT);
    // impl mentions greet but not summon
    fs::write(dir.join("impl.test"), "greet does things here\n").unwrap();

    let out = crabgen(tmp.path(), &["regen", "guest/x"]);
    assert!(out.status.success(), "{}", all_output(&out));
    let text = all_output(&out);
    assert!(text.contains("add these to impl.test:"), "{text}");
    assert!(text.contains("summon"), "missing func listed: {text}");
    assert!(
        !text.contains("greet"),
        "implemented func not listed: {text}"
    );
    assert_eq!(
        fs::read_to_string(dir.join("impl.test")).unwrap(),
        "greet does things here\n",
        "regen never edits the impl file"
    );
}

#[test]
fn failed_generate_must_not_leave_a_fresh_manifest() {
    let tmp = make_repo();
    let dir = add_project(tmp.path(), "x", "test", VALID_WIT);
    // stale: WIT mutated after MANIFEST was written
    fs::write(dir.join("x.wit"), format!("{VALID_WIT}\n// v2\n")).unwrap();

    let out = crabgen_env(
        tmp.path(),
        &["regen", "guest/x"],
        &[("CRABGEN_FAIL_GENERATE", "1")],
    );
    assert!(
        !out.status.success(),
        "regen must fail: {}",
        all_output(&out)
    );

    // The freshness stamp must only exist over gen/ that generate completed.
    // A fresh MANIFEST here would make `check` pass on garbage forever.
    assert!(
        !dir.join("gen/MANIFEST").exists(),
        "gen/MANIFEST must not exist after a failed generate"
    );
}

#[test]
fn regen_all_regenerates_every_project() {
    let tmp = make_repo();
    let dirs: Vec<_> = ["one", "two"]
        .iter()
        .map(|n| add_project(tmp.path(), n, "test", VALID_WIT))
        .collect();

    let out = crabgen(tmp.path(), &["regen", "--all"]);
    assert!(out.status.success(), "{}", all_output(&out));
    for d in &dirs {
        assert!(
            d.join("gen/GENERATED").exists(),
            "{} regenerated",
            d.display()
        );
    }
    assert!(crabgen(tmp.path(), &["check"]).status.success());
}

#[test]
fn regen_errors_for_langs_without_backends() {
    let tmp = make_repo();
    add_project(tmp.path(), "x", "go", VALID_WIT);

    let out = crabgen(tmp.path(), &["regen", "guest/x"]);
    assert!(!out.status.success());
    assert!(
        all_output(&out).contains("no backend for lang go yet"),
        "{}",
        all_output(&out)
    );
}

#[test]
fn regen_errors_on_unmanaged_path() {
    let tmp = make_repo();
    fs::create_dir_all(tmp.path().join("guest/handmade")).unwrap();
    fs::write(tmp.path().join("guest/handmade/handmade.wit"), VALID_WIT).unwrap();

    let out = crabgen(tmp.path(), &["regen", "guest/handmade"]);
    assert!(!out.status.success());
    assert!(
        all_output(&out).contains("MANIFEST"),
        "explains what's missing: {}",
        all_output(&out)
    );
}

// --------------------------------------------------------------------- new

#[test]
fn new_scaffolds_a_working_project() {
    let tmp = make_repo();

    let out = crabgen(tmp.path(), &["new", "shiny", "--lang", "test"]);
    assert!(out.status.success(), "{}", all_output(&out));

    let dir = tmp.path().join("guest/shiny");
    let wit = fs::read_to_string(dir.join("shiny.wit")).expect("starter WIT written");
    assert!(wit.contains("package crab:shiny@0.1.0;"), "{wit}");
    assert!(
        wit.contains("option<T>"),
        "starter WIT carries the versioning guidance comment: {wit}"
    );
    assert!(dir.join("gen/MANIFEST").exists());
    assert!(dir.join("gen/schema.json").exists());
    assert!(dir.join("gen/GENERATED").exists(), "generate ran");
    assert!(dir.join("impl.test").exists(), "scaffold ran");
    // stub impl is empty → new prints the signatures to fill in
    assert!(
        all_output(&out).contains("add these to impl.test:"),
        "{}",
        all_output(&out)
    );

    assert!(
        crabgen(tmp.path(), &["check"]).status.success(),
        "fresh after new"
    );
}

#[test]
fn new_cleans_up_after_itself_when_backend_is_missing() {
    let tmp = make_repo();

    let out = crabgen(tmp.path(), &["new", "doomed", "--lang", "go"]);
    assert!(!out.status.success());
    assert!(
        all_output(&out).contains("no backend for lang go yet"),
        "{}",
        all_output(&out)
    );
    assert!(
        !tmp.path().join("guest/doomed").exists(),
        "failed `new` must not leave a half-created project behind"
    );
}

#[test]
fn new_refuses_to_overwrite_an_existing_dir() {
    let tmp = make_repo();
    let dir = add_project(tmp.path(), "x", "test", VALID_WIT);

    let out = crabgen(tmp.path(), &["new", "x", "--lang", "test"]);
    assert!(!out.status.success(), "must refuse: {}", all_output(&out));
    assert!(all_output(&out).contains("guest/x"), "{}", all_output(&out));
    assert!(
        dir.join("x.wit").exists(),
        "pre-existing project untouched (cleanup must not delete it)"
    );
    assert_eq!(fs::read_to_string(dir.join("x.wit")).unwrap(), VALID_WIT);
}
