//! Project discovery and the check/regen/new drivers.
//!
//! A "project" is `guest/<name>/` containing BOTH a single `*.wit` file and
//! `gen/MANIFEST`. Dirs missing either (the repo's hand-written guests,
//! strays like guest/Untitled) are silently ignored.
//!
//! Repo root = the nearest ancestor of cwd containing both `guest/` and
//! `Cargo.toml`; in practice crabgen runs from the repo root.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::backend::{backend_for, Backend};
use crate::ir::Module;
use crate::manifest::Manifest;
use crate::wit;

#[derive(Debug, Clone)]
pub struct Project {
    /// Absolute project dir: `<repo_root>/guest/<name>`
    pub dir: PathBuf,
    /// Repo-relative dir for display: `guest/<name>`
    pub rel: String,
    pub wit_path: PathBuf,
    pub manifest: Manifest,
}

impl Project {
    /// Does gen/MANIFEST still match the WIT bytes on disk?
    pub fn is_fresh(&self) -> Result<bool> {
        let bytes = fs::read(&self.wit_path)
            .with_context(|| format!("reading {}", self.wit_path.display()))?;
        Ok(self.manifest.is_fresh(&bytes))
    }
}

/// What regen/new report back for the CLI to print.
pub struct Outcome {
    /// `impl.<ext>` for the project's lang
    pub impl_file: String,
    /// typed signatures missing from the impl file
    pub missing_impls: Vec<String>,
}

/// Walk up from cwd to the first dir containing both `guest/` and `Cargo.toml`.
pub fn find_repo_root() -> Result<PathBuf> {
    let cwd = std::env::current_dir().context("getting cwd")?;
    for dir in cwd.ancestors() {
        if dir.join("guest").is_dir() && dir.join("Cargo.toml").is_file() {
            return Ok(dir.to_path_buf());
        }
    }
    bail!(
        "not inside a crabcraft repo: no ancestor of {} contains both guest/ and Cargo.toml",
        cwd.display()
    )
}

/// Every crabgen-managed project under `guest/`, sorted by name.
pub fn discover(repo_root: &Path) -> Result<Vec<Project>> {
    let guest = repo_root.join("guest");
    let mut projects = Vec::new();
    if !guest.is_dir() {
        return Ok(projects);
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(&guest)
        .with_context(|| format!("reading {}", guest.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs {
        if let Some(p) = load_managed(repo_root, &dir)? {
            projects.push(p);
        }
    }
    Ok(projects)
}

/// Load the project at a user-supplied path (for `regen <path>`), with loud
/// errors where `discover` would silently skip. Relative paths resolve
/// against the repo root.
pub fn load_at(repo_root: &Path, path: &Path) -> Result<Project> {
    let dir = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    };
    if !dir.is_dir() {
        bail!("{} is not a directory", dir.display());
    }
    if !dir.join("gen/MANIFEST").is_file() {
        bail!(
            "{} has no gen/MANIFEST — not a crabgen-managed project (scaffold one with `crabgen new`)",
            dir.display()
        );
    }
    load_managed(repo_root, &dir)?.with_context(|| format!("{} has no .wit file", dir.display()))
}

/// `Some(Project)` if `dir` has both gen/MANIFEST and exactly one `*.wit`;
/// `None` if it's not crabgen-managed; `Err` on an ambiguous/broken project.
fn load_managed(repo_root: &Path, dir: &Path) -> Result<Option<Project>> {
    let rel = match dir.strip_prefix(repo_root) {
        Ok(r) => r.to_string_lossy().replace('\\', "/"),
        Err(_) => dir.display().to_string(),
    };
    let manifest_path = dir.join("gen/MANIFEST");
    if !manifest_path.is_file() {
        return Ok(None);
    }
    let mut wits: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("reading {rel}"))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "wit"))
        .collect();
    wits.sort();
    let wit_path = match wits.len() {
        0 => return Ok(None), // gen/ left behind after the WIT was removed
        1 => wits.remove(0),
        n => bail!("{rel} contains {n} .wit files; a crabgen project has exactly one"),
    };
    let manifest = Manifest::parse(
        &fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {rel}/gen/MANIFEST"))?,
    )
    .with_context(|| format!("parsing {rel}/gen/MANIFEST"))?;
    Ok(Some(Project {
        dir: dir.to_path_buf(),
        rel,
        wit_path,
        manifest,
    }))
}

/// All discovered projects whose gen/ no longer matches their WIT.
pub fn stale_projects(repo_root: &Path) -> Result<Vec<Project>> {
    let mut stale = Vec::new();
    for p in discover(repo_root)? {
        if !p.is_fresh()? {
            stale.push(p);
        }
    }
    Ok(stale)
}

/// Re-emit gen/ for one project: load the WIT, rewrite gen/ wholesale,
/// report which impls are missing. NEVER touches the impl file.
pub fn regen(project: &Project) -> Result<Outcome> {
    let backend = backend_for(&project.manifest.lang)?;
    let module = wit::load(&project.wit_path)?;
    rewrite_gen(&module, backend.as_ref(), &project.dir, &project.wit_path)?;
    finish(&module, backend.as_ref(), &project.dir)
}

/// Scaffold guest/<name>/ from a starter WIT, then run the same generate
/// path as regen plus the once-only scaffold. On ANY failure the
/// freshly-created dir is removed — a failed `new` leaves nothing behind.
pub fn new_project(repo_root: &Path, name: &str, lang: &str) -> Result<Outcome> {
    validate_name(name)?;
    let dir = repo_root.join("guest").join(name);
    if dir.exists() {
        bail!("guest/{name} already exists; edit its WIT and run `crabgen regen guest/{name}`");
    }
    let result = (|| {
        fs::create_dir_all(&dir)?;
        let wit_path = dir.join(format!("{name}.wit"));
        fs::write(&wit_path, starter_wit(name))?;
        let backend = backend_for(lang)?;
        let module = wit::load(&wit_path)?;
        rewrite_gen(&module, backend.as_ref(), &dir, &wit_path)?;
        backend.scaffold(&module, &dir)?;
        finish(&module, backend.as_ref(), &dir)
    })();
    if result.is_err() {
        // best-effort cleanup of the dir we just created
        let _ = fs::remove_dir_all(&dir);
    }
    result
}

/// Delete gen/ and recreate it: schema.json + MANIFEST (driver-owned),
/// then whatever the backend emits.
fn rewrite_gen(module: &Module, backend: &dyn Backend, dir: &Path, wit_path: &Path) -> Result<()> {
    let wit_bytes =
        fs::read(wit_path).with_context(|| format!("reading {}", wit_path.display()))?;
    let gen = dir.join("gen");
    if gen.exists() {
        fs::remove_dir_all(&gen).with_context(|| format!("clearing {}", gen.display()))?;
    }
    fs::create_dir_all(&gen)?;
    fs::write(gen.join("schema.json"), &module.schema_json)?;
    fs::write(
        gen.join("MANIFEST"),
        Manifest::new(backend.lang(), &wit_bytes).render(),
    )?;
    backend.generate(module, dir)
}

fn finish(module: &Module, backend: &dyn Backend, dir: &Path) -> Result<Outcome> {
    Ok(Outcome {
        impl_file: format!("impl.{}", backend.impl_ext()),
        missing_impls: backend.missing_impls(module, dir)?,
    })
}

fn validate_name(name: &str) -> Result<()> {
    let kebab = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !kebab {
        bail!(
            "project name `{name}` must be a WIT identifier: lowercase kebab-case (e.g. my-guest)"
        );
    }
    Ok(())
}

/// Starter WIT modeled on wit/hello.wit, versioning guidance included.
fn starter_wit(name: &str) -> String {
    format!(
        r#"// {name} — crabcraft guest interface. This file is the source of truth:
// edit it, then run `crabgen regen guest/{name}` to refresh gen/.
//
// Versioned package: backwards-compatible evolution = add functions freely;
// new inputs as option<T>; breaking changes bump the version and the daemon
// serves both side by side.
package crab:{name}@0.1.0;

interface api {{
  record greet-request {{
    name: string,
    /// optional flourish; older clients simply omit it (option = back-compat)
    excited: option<bool>,
  }}

  greet: func(req: greet-request) -> string;
}}

world {name} {{
  export api;
}}
"#
    )
}
