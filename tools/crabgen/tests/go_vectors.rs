//! WIRE conformance for the Go runtime template: copies templates/go/*.go
//! into testdata/go-runtime (a scratch host-go module) and runs `go test`
//! there with CRAB_VECTORS pointing at the shared wit/vectors.json.
//!
//! Runs in the normal suite (NOT #[ignore]d). Toolchain lookup: `nix shell
//! nixpkgs#go` if nix is on PATH (the project convention), else a bare `go`,
//! else FAIL loudly — never silently skip conformance.

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

/// Copy every .go file from templates/go into the scratch module so the
/// copies can never go stale (go.mod is the only file owned by testdata).
fn refresh_copies(templates: &Path, scratch: &Path) -> Vec<PathBuf> {
    fs::create_dir_all(scratch).expect("create testdata/go-runtime");
    let mut copied = Vec::new();
    for entry in fs::read_dir(templates).expect("read templates/go") {
        let path = entry.expect("dir entry").path();
        if path.extension().is_some_and(|e| e == "go") {
            let dest = scratch.join(path.file_name().unwrap());
            fs::copy(&path, &dest).unwrap_or_else(|e| panic!("copy {path:?} -> {dest:?}: {e}"));
            copied.push(dest);
        }
    }
    assert!(
        !copied.is_empty(),
        "no .go templates found in {templates:?}"
    );
    copied
}

#[test]
fn go_runtime_passes_wire_vectors() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let templates = manifest.join("templates/go");
    let scratch = manifest.join("testdata/go-runtime");
    refresh_copies(&templates, &scratch);

    let vectors = manifest
        .join("../../wit/vectors.json")
        .canonicalize()
        .expect("wit/vectors.json must exist");

    let mut cmd = if on_path("nix") {
        let mut c = Command::new("nix");
        c.args(["shell", "nixpkgs#go", "--command", "go"]);
        c
    } else if on_path("go") {
        Command::new("go")
    } else {
        panic!(
            "neither `nix` nor `go` is on PATH: cannot run the Go WIRE \
             conformance vectors (install nix or go; do not skip this test)"
        );
    };

    let output = cmd
        .args(["test", "./..."])
        .current_dir(&scratch)
        .env("CRAB_VECTORS", &vectors)
        .output()
        .expect("spawn go test");

    assert!(
        output.status.success(),
        "go test failed ({}):\n--- stdout ---\n{}\n--- stderr ---\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
