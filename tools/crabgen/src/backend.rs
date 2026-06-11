//! Language backends. Both `new` and `regen` drive this trait; the driver
//! (project.rs) owns the lifecycle — wholesale gen/ rewrite, schema.json,
//! MANIFEST — and backends fill in the language-specific rest.
//!
//! `dir` is always the project directory (`guest/<name>`); generated code
//! goes under `dir/gen/`, scaffold files at the project root.

use std::fs;
use std::path::Path;

use anyhow::{bail, Result};

use crate::backend_cpp::CppBackend;
use crate::backend_go::GoBackend;
use crate::backend_rust::RustBackend;
use crate::ir::Module;

/// Invariant: every `dir` passed in is `<repo_root>/guest/<name>` — backends
/// may rely on `dir.parent().parent()` being the workspace root (the Rust
/// lane needs it to edit the root Cargo.toml members list).
pub trait Backend {
    fn lang(&self) -> &'static str;
    /// Extension of the hand-written impl file (`impl.<ext>`).
    fn impl_ext(&self) -> &'static str;
    /// Project-relative path of the hand-written impl file, as shown to the
    /// user ("add these to <impl_file>:"). Defaults to `impl.<ext>`; the
    /// Rust lane overrides it (src/app.rs).
    fn impl_file(&self) -> String {
        format!("impl.{}", self.impl_ext())
    }
    /// Emit gen/ contents beyond schema.json + MANIFEST (the driver writes
    /// those), plus the project README (WIT-derived, so regenerated — at the
    /// project root, not gen/).
    fn generate(&self, m: &Module, dir: &Path) -> Result<()>;
    /// Impl stub, build.sh, go.mod-style language files — written ONLY if absent.
    fn scaffold(&self, m: &Module, dir: &Path) -> Result<()>;
    /// Typed signatures of exported functions not found in the impl file.
    /// Printed by the driver; the impl file is NEVER edited by crabgen.
    fn missing_impls(&self, m: &Module, dir: &Path) -> Result<Vec<String>>;
}

pub fn backend_for(lang: &str) -> Result<Box<dyn Backend>> {
    match lang {
        // placeholder lane used by crabgen's own tests
        "test" => Ok(Box::new(TestBackend)),
        "go" => Ok(Box::new(GoBackend)),
        "rust" => Ok(Box::new(RustBackend)),
        "cpp" => Ok(Box::new(CppBackend)),
        // the last lane lands in phase 5
        "ts" => bail!("no backend for lang {lang} yet"),
        other => bail!("unknown lang `{other}` (expected one of: rust, go, cpp, ts)"),
    }
}

/// Minimal backend for exercising the driver in tests: `generate` writes a
/// gen/GENERATED marker, `scaffold` an empty impl.test, and `missing_impls`
/// is a substring scan of impl.test for each exported function's WIT name.
struct TestBackend;

impl Backend for TestBackend {
    fn lang(&self) -> &'static str {
        "test"
    }

    fn impl_ext(&self) -> &'static str {
        "test"
    }

    fn generate(&self, m: &Module, dir: &Path) -> Result<()> {
        // Failure injection for the driver's regression tests.
        if std::env::var_os("CRABGEN_FAIL_GENERATE").is_some_and(|v| v == "1") {
            bail!("CRABGEN_FAIL_GENERATE=1: injected generate failure");
        }
        let mut marker = format!("world {}\n", m.world);
        for f in m.exports.iter().flat_map(|i| &i.funcs) {
            marker.push_str(&f.wit_name);
            marker.push('\n');
        }
        fs::write(dir.join("gen/GENERATED"), marker)?;
        Ok(())
    }

    fn scaffold(&self, _m: &Module, dir: &Path) -> Result<()> {
        let impl_path = dir.join("impl.test");
        if !impl_path.exists() {
            fs::write(
                impl_path,
                "# write the functions listed in gen/GENERATED here\n",
            )?;
        }
        Ok(())
    }

    fn missing_impls(&self, m: &Module, dir: &Path) -> Result<Vec<String>> {
        let impl_src = fs::read_to_string(dir.join("impl.test")).unwrap_or_default();
        Ok(m.exports
            .iter()
            .flat_map(|i| &i.funcs)
            .filter(|f| !impl_src.contains(&f.wit_name))
            .map(|f| f.wit_name.clone())
            .collect())
    }
}
