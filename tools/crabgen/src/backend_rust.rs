//! Rust backend: typed bindings over guest/crab-sdk (the Rust lane's runtime
//! — unlike Go there is no template runtime here, crab-sdk already implements
//! the WIRE codec + ABI and passes wit/vectors.json).
//!
//! Per project (guest/<name>/):
//! - src/gen/mod.rs — GENERATED every regen: native types with conversions,
//!   the `<World>Impl` trait, `setup(&mut Registry)` adapter registration,
//!   typed mesh wrappers.
//! - src/lib.rs — GENERATED every regen (thin: mod gen; mod app; the
//!   crab_sdk::export_abi! invocation); like README.md it lives outside
//!   gen/ but is crabgen-owned.
//! - src/app.rs — SCAFFOLD-ONCE: `pub struct App` + unimplemented stubs.
//! - Cargo.toml — SCAFFOLD-ONCE: cdylib, crab-sdk path dep (+ the "mesh"
//!   feature only when the world has imports).
//! - build.sh — SCAFFOLD-ONCE: rustup-from-nix wasip1 build + SIMD tripwire.
//!
//! `new --lang rust` also inserts the crate into the root workspace members
//! (idempotent; regen never needs it).
//!
//! Type mapping (WIT → Rust):
//!
//! | WIT              | Rust                                                  |
//! |------------------|-------------------------------------------------------|
//! | bool             | bool                                                  |
//! | u8 u16 u32 u64   | u8 u16 u32 u64                                        |
//! | s8 s16 s32 s64   | i8 i16 i32 i64                                        |
//! | f32 f64          | f32 f64                                               |
//! | char             | char                                                  |
//! | string           | String                                                |
//! | list<T>          | Vec<T>                                                |
//! | option<T>        | Option<T>                                             |
//! | tuple<A, B, ..>  | (A, B, ..)  (empty tuple = ())                        |
//! | record           | struct with pub snake_case fields                     |
//! | variant          | enum with payload tuple variants                      |
//! | enum             | fieldless enum, cases in declaration order            |
//! | flags            | u64 newtype + SCREAMING bit consts (max 64 flags)     |
//! | result<T, E>     | value position: Result<T, E> (absent sides are ()).   |
//! |                  | Function-RESULT position with E = string or absent:   |
//! |                  | the method's own Result<T, String> — an Err encodes   |
//! |                  | as the WIRE result err case (status stays 0); any     |
//! |                  | other E nests: Result<Result<T, E>, String> where the |
//! |                  | OUTER Err means status 1.                             |
//!
//! Conversions: every nominal type (record/variant/enum/flags) gets
//! `TryFrom<Value> for X` (wire → native) and `TryFrom<X> for Value`
//! (native → wire). The native→wire direction is TryFrom rather than From
//! UNIFORMLY because flags can carry unknown bits the wire cannot (silently
//! dropping them would corrupt data); one fallible direction everywhere
//! beats a per-type split. Aliases are `pub type` declarations converted
//! structurally at use sites.
//!
//! Name casing: WIT kebab-case → PascalCase for types/variant cases/the
//! trait ("echo-everything" → "EchoEverything", "a-u8" → "AU8" — capitalize
//! each dash segment, no acronym table), snake_case for functions, params
//! and fields (every segment lowercased: "AB" → "ab"), SCREAMING_SNAKE for
//! flags consts. Names that hit a Rust keyword (or a local the generated
//! code declares in the same scope: `args`, `params`, `reply`, `workload`,
//! `v`) get a trailing underscore (`type` → `type_`) — mangling over
//! `r#`-raw idents so signatures stay grep-able and `self`/`Self`/`crate`
//! (not raw-able) need no special case.
//!
//! Error semantics (WIRE.md section 2): crab-sdk's Registry maps a handler
//! `Err(msg)` to a status-1 reply "<message>". Generated trait methods
//! return `Result<_, String>`; for a WIT function whose result is
//! result<T, string> (or result with no err payload) the adapter converts
//! the method's Err into the WIRE result ERR CASE (a normal status-0 reply)
//! instead — mirroring the Go lane's Ret classification.
//!
//! Collisions: every emitted module-scope identifier is checked against the
//! fixed generated names + the preludes the generated code references;
//! params, record fields and variant/enum cases are checked within their
//! scopes. Any WIT name that lands on a taken identifier fails generate()
//! with both names in the message instead of emitting broken code.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

use crate::backend::Backend;
use crate::emit::{
    iface_short, is_temp_shaped, pad, project_name, trim_final, GENERATED_HEADER, W,
};
use crate::ir::{Func, Iface, Module, NamedTy, Ty};

/// Module-scope identifiers the generated src/gen/mod.rs declares or
/// references unqualified; a WIT name mapping onto one of these fails
/// generate(). (Lowercase primitive names like `bool`/`u8` are unreachable:
/// PascalCasing always uppercases the first letter.)
const RESERVED: &[&str] = &[
    // declared by the generated module
    "SCHEMA", "setup", // crab-sdk imports
    "Registry", "Type", "Value",
    // prelude names the generated code uses unqualified (a same-named local
    // struct would shadow them inside the module)
    "String", "Vec", "Box", "Option", "Result", "Some", "None", "Ok", "Err", "TryFrom", "From",
    "Into", // not even r#-able
    "Self",
];

/// Rust keywords (strict + reserved, through edition 2024's `gen`): snake
/// identifiers landing here get a trailing underscore.
const RUST_KEYWORDS: &[&str] = &[
    "as", "abstract", "async", "await", "become", "box", "break", "const", "continue", "crate",
    "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "union", "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// Locals the generated code declares in scopes that also hold WIT params
/// (handler bodies, mesh wrappers); params landing here get a trailing
/// underscore. Lift/lower internals (`x`, `xs`, `p`, `r`, `x0`…) live in
/// nested blocks that never reference params, except the `x<N>` tuple temps
/// which are mangled by shape below.
const PARAM_AVOID: &[&str] = &["args", "params", "reply", "workload", "v"];

pub struct RustBackend;

impl Backend for RustBackend {
    fn lang(&self) -> &'static str {
        "rust"
    }

    fn impl_ext(&self) -> &'static str {
        "rs"
    }

    fn impl_file(&self) -> String {
        "src/app.rs".to_string()
    }

    fn generate(&self, m: &Module, dir: &Path) -> Result<()> {
        let name = project_name(dir)?;
        let g = Gen::new(m, &name)?;
        // Validate before writing anything: a scaffolded Cargo.toml that
        // doesn't enable crab-sdk's "mesh" feature cannot compile the mesh
        // wrappers this generate() is about to emit. (On `new` the file
        // doesn't exist yet; scaffold() writes it correctly right after.)
        let cargo_toml = dir.join("Cargo.toml");
        if !m.imports.is_empty() && cargo_toml.exists() {
            let src = fs::read_to_string(&cargo_toml)
                .with_context(|| format!("reading {}", cargo_toml.display()))?;
            // Look for "mesh" in NON-COMMENT content only: the scaffolded
            // header comment itself mentions `features = ["mesh"]`, so a
            // whole-file substring scan would never fire. Stripping each
            // line at its first '#' is exact for the scaffolded shape (no
            // TOML string in it contains '#').
            let has_mesh = src
                .lines()
                .any(|l| l.split('#').next().unwrap_or("").contains("mesh"));
            if !has_mesh {
                bail!(
                    "the WIT now imports interfaces (mesh wrappers need crab-sdk's `mesh` \
                     feature) but {} doesn't enable it; change the dependency line to:\n  \
                     crab-sdk = {{ path = \"../crab-sdk\", features = [\"mesh\"] }}",
                    cargo_toml.display()
                );
            }
        }
        let src_gen = dir.join("src/gen");
        fs::create_dir_all(&src_gen).with_context(|| format!("creating {}", src_gen.display()))?;
        fs::write(src_gen.join("mod.rs"), g.mod_rs()?)?;
        fs::write(dir.join("src/lib.rs"), g.lib_rs())?;
        fs::write(dir.join("README.md"), readme(&name, m))?;
        Ok(())
    }

    fn scaffold(&self, m: &Module, dir: &Path) -> Result<()> {
        let name = project_name(dir)?;
        let g = Gen::new(m, &name)?;
        let write_once = |path: &Path, content: &str| -> Result<()> {
            if !path.exists() {
                fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
            }
            Ok(())
        };
        fs::create_dir_all(dir.join("src"))?;
        write_once(&dir.join("src/app.rs"), &g.app_rs()?)?;
        write_once(&dir.join("Cargo.toml"), &g.cargo_toml())?;
        let build = dir.join("build.sh");
        if !build.exists() {
            fs::write(&build, build_sh(&name))?;
            let mut perms = fs::metadata(&build)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&build, perms)?;
        }
        // dir is <repo_root>/guest/<name> (backend invariant): the workspace
        // root manifest is two levels up.
        let root_cargo = dir
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("Cargo.toml"))
            .ok_or_else(|| anyhow!("{} has no repo root two levels up", dir.display()))?;
        add_workspace_member(&root_cargo, &format!("guest/{name}"))?;
        Ok(())
    }

    fn missing_impls(&self, m: &Module, dir: &Path) -> Result<Vec<String>> {
        let g = Gen::new(m, &project_name(dir)?)?;
        let src = fs::read_to_string(dir.join("src/app.rs")).unwrap_or_default();
        let mut missing = Vec::new();
        for iface in &m.exports {
            for f in &iface.funcs {
                if !src.contains(&format!("fn {}(", rust_ident(&f.wit_name))) {
                    missing.push(g.method_sig(f, "gen::")?);
                }
            }
        }
        Ok(missing)
    }
}

// ---------------------------------------------------------------------------
// naming
// ---------------------------------------------------------------------------

/// kebab-case → PascalCase: capitalize each dash segment ("a-u8" → "AU8").
fn rust_pascal(kebab: &str) -> String {
    kebab
        .split('-')
        .map(|seg| {
            let mut c = seg.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// kebab-case → snake_case, every segment lowercased ("AB" → "ab").
fn rust_snake(kebab: &str) -> String {
    kebab
        .split('-')
        .map(|seg| seg.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// snake identifier for fields/functions: keyword-mangled with a trailing
/// underscore (`type` → `type_`).
fn rust_ident(kebab: &str) -> String {
    let mut s = rust_snake(kebab);
    if RUST_KEYWORDS.contains(&s.as_str()) {
        s.push('_');
    }
    s
}

/// snake identifier for params: additionally mangled away from the locals
/// generated bodies declare and the `x<N>` tuple-temp shape.
fn rust_param(kebab: &str) -> String {
    let mut s = rust_snake(kebab);
    if RUST_KEYWORDS.contains(&s.as_str())
        || PARAM_AVOID.contains(&s.as_str())
        || is_temp_shaped(&s, RUST_TEMP_PREFIXES)
    {
        s.push('_');
    }
    s
}

/// kebab-case → SCREAMING_SNAKE for flags consts.
fn rust_screaming(kebab: &str) -> String {
    rust_snake(kebab).to_ascii_uppercase()
}

/// The temp-name prefixes this emitter generates (x0, x1, … tuple temps).
const RUST_TEMP_PREFIXES: &[&str] = &["x"];

// ---------------------------------------------------------------------------
// emission plumbing (the shared W from emit.rs, 4-space-indented)
// ---------------------------------------------------------------------------

/// Scalar types: (Rust type, Value variant).
fn scalar(ty: &Ty) -> Option<(&'static str, &'static str)> {
    Some(match ty {
        Ty::Bool => ("bool", "Bool"),
        Ty::U8 => ("u8", "U8"),
        Ty::U16 => ("u16", "U16"),
        Ty::U32 => ("u32", "U32"),
        Ty::U64 => ("u64", "U64"),
        Ty::S8 => ("i8", "S8"),
        Ty::S16 => ("i16", "S16"),
        Ty::S32 => ("i32", "S32"),
        Ty::S64 => ("i64", "S64"),
        Ty::F32 => ("f32", "F32"),
        Ty::F64 => ("f64", "F64"),
        Ty::Char => ("char", "Char"),
        Ty::String => ("String", "String"),
        _ => return None,
    })
}

/// How a function's WIT result maps onto its Rust signature (mirrors the Go
/// lane's Ret classification).
enum Ret<'a> {
    /// no WIT result → `Result<(), String>` (Err = status-1 reply)
    None,
    /// plain value → `Result<T, String>` (Err = status-1 reply)
    Plain(&'a Ty),
    /// result<T, string> / result<T> / result<_,_> → `Result<T, String>` /
    /// `Result<(), String>`; an Err IS the WIRE result ERR CASE (status
    /// stays 0). `has_msg` = the err side carries the string payload.
    ResStr { ok: Option<&'a Ty>, has_msg: bool },
    /// result with any other err type → `Result<Result<T, E>, String>`
    /// (the outer Err = status-1 reply)
    ResTyped(&'a Ty),
}

// ---------------------------------------------------------------------------
// type expressions (free functions: they never consult Gen state)
// ---------------------------------------------------------------------------

/// Rust type expression for a value-position type. `qual` is "" inside
/// the gen module, "gen::" in app.rs signatures.
fn rust_ty(ty: &Ty, qual: &str) -> Result<String> {
    Ok(match ty {
        Ty::List(t) => format!("Vec<{}>", rust_ty(t, qual)?),
        Ty::Option(t) => format!("Option<{}>", rust_ty(t, qual)?),
        Ty::Tuple(ts) => match ts.len() {
            0 => "()".to_string(),
            1 => format!("({},)", rust_ty(&ts[0], qual)?),
            _ => format!(
                "({})",
                ts.iter()
                    .map(|t| rust_ty(t, qual))
                    .collect::<Result<Vec<_>>>()?
                    .join(", ")
            ),
        },
        Ty::Result(ok, errt) => {
            let side = |t: &Option<Box<Ty>>| -> Result<String> {
                Ok(match t {
                    Some(t) => rust_ty(t, qual)?,
                    None => "()".to_string(),
                })
            };
            format!("Result<{}, {}>", side(ok)?, side(errt)?)
        }
        Ty::Named(n) => format!("{qual}{}", rust_pascal(n)),
        Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_) => {
            bail!("internal error: anonymous {ty:?} cannot appear in value position (WIT names these)")
        }
        _ => scalar(ty)
            .map(|(rt, _)| rt.to_string())
            .ok_or_else(|| anyhow!("unmapped type {ty:?}"))?,
    })
}

/// `crab_sdk::Type` tree expression for the registry / mesh codec.
fn type_tree(ty: &Ty, ind: usize) -> Result<String> {
    let p0 = pad(ind);
    let p1 = pad(ind + 1);
    Ok(match ty {
        Ty::List(t) => format!("Type::List(Box::new({}))", type_tree(t, ind)?),
        Ty::Option(t) => format!("Type::Option(Box::new({}))", type_tree(t, ind)?),
        Ty::Tuple(ts) if ts.is_empty() => "Type::Tuple(Vec::new())".to_string(),
        Ty::Tuple(ts) => {
            let items = ts
                .iter()
                .map(|t| Ok(format!("{p1}{},\n", type_tree(t, ind + 1)?)))
                .collect::<Result<String>>()?;
            format!("Type::Tuple(vec![\n{items}{p0}])")
        }
        Ty::Record(fields) => {
            let items = fields
                .iter()
                .map(|(_, t)| Ok(format!("{p1}{},\n", type_tree(t, ind + 1)?)))
                .collect::<Result<String>>()?;
            format!("Type::Record(vec![\n{items}{p0}])")
        }
        Ty::Variant(cases) => {
            let items = cases
                .iter()
                .map(|(_, t)| {
                    Ok(match t {
                        Some(t) => {
                            format!("{p1}Some({}),\n", type_tree(t, ind + 1)?)
                        }
                        None => format!("{p1}None,\n"),
                    })
                })
                .collect::<Result<String>>()?;
            format!("Type::Variant(vec![\n{items}{p0}])")
        }
        Ty::Enum(cases) => format!("Type::Enum({})", cases.len()),
        Ty::Flags(flags) => format!("Type::Flags({})", flags.len()),
        Ty::Result(ok, errt) => {
            let side = |t: &Option<Box<Ty>>| -> Result<String> {
                Ok(match t {
                    Some(t) => format!("Some(Box::new({}))", type_tree(t, ind + 1)?),
                    None => "None".to_string(),
                })
            };
            format!(
                "Type::Result {{\n{p1}ok: {},\n{p1}err: {},\n{p0}}}",
                side(ok)?,
                side(errt)?
            )
        }
        Ty::Named(n) => format!("ty_{}()", rust_snake(n)),
        _ => {
            let (_, var) = scalar(ty).ok_or_else(|| anyhow!("unmapped type {ty:?}"))?;
            format!("Type::{var}")
        }
    })
}

// ---------------------------------------------------------------------------
// the generator
// ---------------------------------------------------------------------------

struct Gen<'a> {
    m: &'a Module,
    project: String,
    /// WIT type name → definition, across every interface (one Rust module).
    types: HashMap<&'a str, &'a Ty>,
    trait_name: String,
}

impl<'a> Gen<'a> {
    fn new(m: &'a Module, project: &str) -> Result<Self> {
        let mut g = Gen {
            m,
            project: project.to_string(),
            types: HashMap::new(),
            trait_name: format!("{}Impl", rust_pascal(&m.world)),
        };
        for iface in m.exports.iter().chain(&m.imports) {
            for t in &iface.types {
                if g.types.insert(t.wit_name.as_str(), &t.ty).is_some() {
                    bail!(
                        "type `{}` is declared in more than one interface; the Rust lane puts \
                         every type in one module — rename one of them",
                        t.wit_name
                    );
                }
            }
        }
        g.validate()?;
        Ok(g)
    }

    /// Claim every module-scope identifier this module will emit; duplicate
    /// or reserved names fail loudly here, before anything is written.
    fn validate(&self) -> Result<()> {
        let mut taken: HashMap<String, String> = RESERVED
            .iter()
            .map(|s| (s.to_string(), "the generated bindings".to_string()))
            .collect();
        taken.insert(
            self.trait_name.clone(),
            format!("the generated trait for world `{}`", self.m.world),
        );
        let mut claim = |ident: String, what: String| -> Result<()> {
            if let Some(owner) = taken.get(&ident) {
                bail!("{what} maps to Rust identifier `{ident}`, which collides with {owner}; rename it in the WIT");
            }
            taken.insert(ident, what);
            Ok(())
        };

        for iface in self.m.exports.iter().chain(&self.m.imports) {
            for t in &iface.types {
                let p = rust_pascal(&t.wit_name);
                let what = format!("WIT type `{}` ({})", t.wit_name, iface.instance);
                claim(p.clone(), what.clone())?;
                claim(format!("ty_{}", rust_snake(&t.wit_name)), what.clone())?;
                match &t.ty {
                    Ty::Record(fields) => {
                        let mut seen: HashMap<String, &str> = HashMap::new();
                        for (fname, _) in fields {
                            let fi = rust_ident(fname);
                            if let Some(prev) = seen.insert(fi.clone(), fname) {
                                bail!(
                                    "{what}: fields `{prev}` and `{fname}` both map to Rust field `{fi}`; rename one in the WIT"
                                );
                            }
                        }
                    }
                    Ty::Variant(cases) => {
                        let mut seen: HashMap<String, &str> = HashMap::new();
                        for (c, _) in cases {
                            let cp = rust_pascal(c);
                            if cp == "Self" {
                                bail!("{what}: case `{c}` maps to `Self`, which Rust forbids as a variant name; rename it in the WIT");
                            }
                            if let Some(prev) = seen.insert(cp.clone(), c) {
                                bail!(
                                    "{what}: cases `{prev}` and `{c}` both map to Rust variant `{cp}`; rename one in the WIT"
                                );
                            }
                        }
                    }
                    Ty::Enum(cases) => {
                        let mut seen: HashMap<String, &str> = HashMap::new();
                        for c in cases {
                            let cp = rust_pascal(c);
                            if cp == "Self" {
                                bail!("{what}: case `{c}` maps to `Self`, which Rust forbids as a variant name; rename it in the WIT");
                            }
                            if let Some(prev) = seen.insert(cp.clone(), c.as_str()) {
                                bail!(
                                    "{what}: cases `{prev}` and `{c}` both map to Rust variant `{cp}`; rename one in the WIT"
                                );
                            }
                        }
                    }
                    Ty::Flags(flags) => {
                        if flags.len() > 64 {
                            bail!(
                                "flags `{}` has {} flags; the Rust lane (u64 newtype {p}) supports at most 64",
                                t.wit_name,
                                flags.len()
                            );
                        }
                        let mut seen: HashMap<String, &str> = HashMap::new();
                        for f in flags {
                            let fc = rust_screaming(f);
                            if let Some(prev) = seen.insert(fc.clone(), f) {
                                bail!(
                                    "{what}: flags `{prev}` and `{f}` both map to Rust const `{fc}`; rename one in the WIT"
                                );
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        for iface in &self.m.exports {
            let mut methods: HashMap<String, String> = HashMap::new();
            for f in &iface.funcs {
                let s = rust_ident(&f.wit_name);
                let what = format!("WIT function `{}` ({})", f.wit_name, iface.instance);
                claim(format!("handle_{}", rust_snake(&f.wit_name)), what.clone())?;
                if let Some(prev) = methods.insert(s.clone(), f.wit_name.clone()) {
                    bail!(
                        "WIT functions `{prev}` and `{}` both map to Rust method `{s}`; rename one",
                        f.wit_name
                    );
                }
                self.validate_params(f, &what)?;
            }
        }
        for iface in &self.m.imports {
            let is = rust_snake(iface_short(&iface.instance));
            for f in &iface.funcs {
                let what = format!("WIT import `{}` ({})", f.wit_name, iface.instance);
                claim(format!("{is}_{}", rust_snake(&f.wit_name)), what.clone())?;
                self.validate_params(f, &what)?;
            }
        }
        Ok(())
    }

    fn validate_params(&self, f: &Func, what: &str) -> Result<()> {
        let mut seen: HashMap<String, &str> = HashMap::new();
        for (name, _) in &f.params {
            if let Some(prev) = seen.insert(rust_param(name), name) {
                bail!("{what}: params `{prev}` and `{name}` both map to the same Rust name");
            }
        }
        Ok(())
    }

    /// Follow Named references to the defining type (alias chains too).
    fn resolve(&self, ty: &'a Ty) -> Result<&'a Ty> {
        let mut t = ty;
        for _ in 0..64 {
            match t {
                Ty::Named(n) => {
                    t = self
                        .types
                        .get(n.as_str())
                        .copied()
                        .ok_or_else(|| anyhow!("unresolved type reference `{n}`"))?;
                }
                _ => return Ok(t),
            }
        }
        bail!("type alias cycle while resolving `{ty:?}`");
    }

    /// Does the named type (after alias chains) declare a nominal Rust type
    /// (struct/enum/newtype with TryFrom impls)?
    fn is_nominal(&self, name: &str) -> Result<bool> {
        let ty = self
            .types
            .get(name)
            .copied()
            .ok_or_else(|| anyhow!("unresolved type reference `{name}`"))?;
        Ok(matches!(
            self.resolve(ty)?,
            Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_)
        ))
    }

    fn classify(&self, f: &'a Func) -> Result<Ret<'a>> {
        let Some(rty) = &f.result else {
            return Ok(Ret::None);
        };
        if let Ty::Result(ok, errt) = self.resolve(rty)? {
            let ok = ok.as_deref();
            match errt.as_deref() {
                None => return Ok(Ret::ResStr { ok, has_msg: false }),
                Some(e) if matches!(self.resolve(e)?, Ty::String) => {
                    return Ok(Ret::ResStr { ok, has_msg: true });
                }
                Some(_) => return Ok(Ret::ResTyped(rty)),
            }
        }
        Ok(Ret::Plain(rty))
    }

    // -- Value <-> native conversion expressions --------------------------------

    /// Expression converting `expr` (a `Value`) into the native type, using
    /// `?` / `return Err(...)` — every enclosing scope returns
    /// `Result<_, String>`. `ind` is the indent level of the line the
    /// expression starts on.
    fn lift(&self, ty: &Ty, expr: &str, ind: usize) -> Result<String> {
        let p0 = pad(ind);
        let p1 = pad(ind + 1);
        let p2 = pad(ind + 2);
        Ok(match ty {
            Ty::Named(n) if self.is_nominal(n)? => {
                format!("{}::try_from({expr})?", rust_pascal(n))
            }
            Ty::Named(n) => {
                let under = self
                    .types
                    .get(n.as_str())
                    .copied()
                    .ok_or_else(|| anyhow!("unresolved type reference `{n}`"))?;
                self.lift(under, expr, ind)?
            }
            Ty::List(t) => format!(
                "match {expr} {{\n\
                 {p1}Value::List(xs) => xs\n\
                 {p2}.into_iter()\n\
                 {p2}.map(|x| Ok({}))\n\
                 {p2}.collect::<Result<Vec<_>, String>>()?,\n\
                 {p1}other => return Err(format!(\"expected list, got {{other:?}}\")),\n\
                 {p0}}}",
                self.lift(t, "x", ind + 2)?
            ),
            Ty::Option(t) => format!(
                "match {expr} {{\n\
                 {p1}Value::Option(Some(x)) => Some({}),\n\
                 {p1}Value::Option(None) => None,\n\
                 {p1}other => return Err(format!(\"expected option, got {{other:?}}\")),\n\
                 {p0}}}",
                self.lift(t, "*x", ind + 1)?
            ),
            Ty::Tuple(ts) if ts.is_empty() => format!(
                "match {expr} {{\n\
                 {p1}Value::Tuple(xs) if xs.is_empty() => (),\n\
                 {p1}other => return Err(format!(\"expected 0-tuple, got {{other:?}}\")),\n\
                 {p0}}}"
            ),
            Ty::Tuple(ts) => {
                let n = ts.len();
                let items = ts
                    .iter()
                    .map(|t| {
                        Ok(format!(
                            "{}{},\n",
                            pad(ind + 3),
                            self.lift(t, "xs.next().unwrap()", ind + 3)?
                        ))
                    })
                    .collect::<Result<String>>()?;
                format!(
                    "match {expr} {{\n\
                     {p1}Value::Tuple(xs) if xs.len() == {n} => {{\n\
                     {p2}let mut xs = xs.into_iter();\n\
                     {p2}(\n\
                     {items}\
                     {p2})\n\
                     {p1}}}\n\
                     {p1}other => return Err(format!(\"expected {n}-tuple, got {{other:?}}\")),\n\
                     {p0}}}"
                )
            }
            Ty::Result(ok, errt) => {
                let rty = rust_ty(ty, "")?;
                let side = |t: &Option<Box<Ty>>| -> Result<String> {
                    Ok(match t {
                        Some(t) => format!(
                            "match p {{\n\
                             {p3}Some(x) => {},\n\
                             {p3}None => return Err(\"missing result payload\".to_string()),\n\
                             {p2}}}",
                            self.lift(t, "*x", ind + 3)?,
                            p3 = pad(ind + 3),
                            p2 = pad(ind + 2),
                        ),
                        None => format!(
                            "match p {{\n\
                             {p3}None => (),\n\
                             {p3}Some(x) => return Err(format!(\"unexpected result payload {{x:?}}\")),\n\
                             {p2}}}",
                            p3 = pad(ind + 3),
                            p2 = pad(ind + 2),
                        ),
                    })
                };
                format!(
                    "{{\n\
                     {p1}let r: {rty} = match {expr} {{\n\
                     {p2}Value::Result(Ok(p)) => Ok({}),\n\
                     {p2}Value::Result(Err(p)) => Err({}),\n\
                     {p2}other => return Err(format!(\"expected result, got {{other:?}}\")),\n\
                     {p1}}};\n\
                     {p1}r\n\
                     {p0}}}",
                    side(ok)?,
                    side(errt)?
                )
            }
            Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_) => {
                bail!("internal error: anonymous {ty:?} in lift position")
            }
            _ => {
                let (rt, _) = scalar(ty).ok_or_else(|| anyhow!("unmapped type {ty:?}"))?;
                format!("{rt}::try_from({expr})?")
            }
        })
    }

    /// True if lowering this type can fail (it reaches a nominal type, whose
    /// native→wire conversion is TryFrom — see the header).
    fn lower_fallible(&self, ty: &Ty) -> Result<bool> {
        Ok(match ty {
            Ty::Named(n) => {
                if self.is_nominal(n)? {
                    true
                } else {
                    let under = self.types.get(n.as_str()).copied().unwrap();
                    self.lower_fallible(under)?
                }
            }
            Ty::List(t) | Ty::Option(t) => self.lower_fallible(t)?,
            Ty::Tuple(ts) => ts
                .iter()
                .map(|t| self.lower_fallible(t))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .any(|b| b),
            Ty::Result(ok, errt) => {
                ok.as_deref()
                    .map_or(Ok(false), |t| self.lower_fallible(t))?
                    || errt
                        .as_deref()
                        .map_or(Ok(false), |t| self.lower_fallible(t))?
            }
            _ => false,
        })
    }

    /// Expression converting native `expr` into a `Value`, using `?` when
    /// nominal conversions are involved.
    fn lower(&self, ty: &Ty, expr: &str, ind: usize) -> Result<String> {
        let p0 = pad(ind);
        let p1 = pad(ind + 1);
        Ok(match ty {
            Ty::Named(n) if self.is_nominal(n)? => format!("Value::try_from({expr})?"),
            Ty::Named(n) => {
                let under = self
                    .types
                    .get(n.as_str())
                    .copied()
                    .ok_or_else(|| anyhow!("unresolved type reference `{n}`"))?;
                self.lower(under, expr, ind)?
            }
            Ty::List(t) => {
                if self.lower_fallible(t)? {
                    format!(
                        "Value::List(\n\
                         {p1}{expr}.into_iter()\n\
                         {p2}.map(|x| Ok({}))\n\
                         {p2}.collect::<Result<Vec<_>, String>>()?,\n\
                         {p0})",
                        self.lower(t, "x", ind + 2)?,
                        p2 = pad(ind + 2),
                    )
                } else {
                    format!(
                        "Value::List({expr}.into_iter().map(|x| {}).collect())",
                        self.lower(t, "x", ind)?
                    )
                }
            }
            Ty::Option(t) => {
                if self.lower_fallible(t)? {
                    format!(
                        "match {expr} {{\n\
                         {p1}Some(x) => Value::Option(Some(Box::new({}))),\n\
                         {p1}None => Value::Option(None),\n\
                         {p0}}}",
                        self.lower(t, "x", ind + 1)?
                    )
                } else {
                    format!(
                        "Value::Option({expr}.map(|x| Box::new({})))",
                        self.lower(t, "x", ind)?
                    )
                }
            }
            Ty::Tuple(ts) if ts.is_empty() => format!(
                "{{\n\
                 {p1}let () = {expr};\n\
                 {p1}Value::Tuple(Vec::new())\n\
                 {p0}}}"
            ),
            Ty::Tuple(ts) => {
                let names: Vec<String> = (0..ts.len()).map(|i| format!("x{i}")).collect();
                let pattern = if ts.len() == 1 {
                    format!("({},)", names[0])
                } else {
                    format!("({})", names.join(", "))
                };
                let items = ts
                    .iter()
                    .zip(&names)
                    .map(|(t, n)| {
                        Ok(format!(
                            "{p2}{},\n",
                            self.lower(t, n, ind + 2)?,
                            p2 = pad(ind + 2)
                        ))
                    })
                    .collect::<Result<String>>()?;
                format!(
                    "{{\n\
                     {p1}let {pattern} = {expr};\n\
                     {p1}Value::Tuple(vec![\n\
                     {items}\
                     {p1}])\n\
                     {p0}}}"
                )
            }
            Ty::Result(ok, errt) => {
                let arm = |variant: &str, t: &Option<Box<Ty>>| -> Result<String> {
                    Ok(match t {
                        Some(t) => format!(
                            "{variant}(x) => Value::Result({variant}(Some(Box::new({})))),",
                            self.lower(t, "x", ind + 1)?
                        ),
                        None => format!("{variant}(()) => Value::Result({variant}(None)),"),
                    })
                };
                format!(
                    "match {expr} {{\n\
                     {p1}{}\n\
                     {p1}{}\n\
                     {p0}}}",
                    arm("Ok", ok)?,
                    arm("Err", errt)?
                )
            }
            Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_) => {
                bail!("internal error: anonymous {ty:?} in lower position")
            }
            _ => {
                let (_, var) = scalar(ty).ok_or_else(|| anyhow!("unmapped type {ty:?}"))?;
                format!("Value::{var}({expr})")
            }
        })
    }

    // -- named type declarations -------------------------------------------------

    fn emit_named_ty(&self, w: &mut W, iface: &Iface, t: &NamedTy) -> Result<()> {
        let p = rust_pascal(&t.wit_name);
        match &t.ty {
            Ty::Record(fields) => self.emit_record(w, iface, t, &p, fields)?,
            Ty::Variant(cases) => self.emit_variant(w, iface, t, &p, cases)?,
            Ty::Enum(cases) => self.emit_enum(w, iface, t, &p, cases),
            Ty::Flags(flags) => self.emit_flags(w, iface, t, &p, flags),
            other => {
                w.line(format!(
                    "/// Aliases the WIT type `{}` ({}); converted structurally at use sites.",
                    t.wit_name, iface.instance
                ));
                w.line(format!("pub type {p} = {};", rust_ty(other, "")?));
                w.line("");
            }
        }
        // WIRE type tree, registered with the runtime / used by mesh calls.
        w.line(format!("/// WIRE type tree for `{}`.", t.wit_name));
        w.open(format!("fn ty_{}() -> Type {{", rust_snake(&t.wit_name)));
        w.line(type_tree(&t.ty, w.ind)?);
        w.close("}");
        w.line("");
        Ok(())
    }

    fn emit_record(
        &self,
        w: &mut W,
        iface: &Iface,
        t: &NamedTy,
        p: &str,
        fields: &[(String, Ty)],
    ) -> Result<()> {
        w.line(format!(
            "/// Mirrors the WIT record `{}` ({}).",
            t.wit_name, iface.instance
        ));
        w.line("#[derive(Debug, Clone, PartialEq)]");
        w.open(format!("pub struct {p} {{"));
        for (n, ft) in fields {
            w.line(format!("pub {}: {},", rust_ident(n), rust_ty(ft, "")?));
        }
        w.close("}");
        w.line("");

        w.open(format!("impl TryFrom<Value> for {p} {{"));
        w.line("type Error = String;");
        w.line("");
        w.open("fn try_from(v: Value) -> Result<Self, String> {");
        w.open("let fields = match v {");
        w.line("Value::Record(fields) => fields,");
        w.line(format!(
            "other => return Err(format!(\"{}: expected record, got {{other:?}}\")),",
            t.wit_name
        ));
        w.close("};");
        w.open(format!("if fields.len() != {} {{", fields.len()));
        w.line(format!(
            "return Err(format!(\"{}: expected {} fields, got {{}}\", fields.len()));",
            t.wit_name,
            fields.len()
        ));
        w.close("}");
        w.line("let mut fields = fields.into_iter();");
        w.open(format!("Ok({p} {{"));
        for (n, ft) in fields {
            w.line(format!(
                "{}: {},",
                rust_ident(n),
                self.lift(ft, "fields.next().unwrap()", w.ind)?
            ));
        }
        w.close("})");
        w.close("}");
        w.close("}");
        w.line("");

        w.open(format!("impl TryFrom<{p}> for Value {{"));
        w.line("type Error = String;");
        w.line("");
        w.open(format!("fn try_from(v: {p}) -> Result<Value, String> {{"));
        w.open("Ok(Value::Record(vec![");
        for (n, ft) in fields {
            w.line(format!(
                "{},",
                self.lower(ft, &format!("v.{}", rust_ident(n)), w.ind)?
            ));
        }
        w.close("]))");
        w.close("}");
        w.close("}");
        w.line("");
        Ok(())
    }

    fn emit_variant(
        &self,
        w: &mut W,
        iface: &Iface,
        t: &NamedTy,
        p: &str,
        cases: &[(String, Option<Ty>)],
    ) -> Result<()> {
        w.line(format!(
            "/// Mirrors the WIT variant `{}` ({}); cases in declaration order.",
            t.wit_name, iface.instance
        ));
        w.line("#[derive(Debug, Clone, PartialEq)]");
        w.open(format!("pub enum {p} {{"));
        for (c, payload) in cases {
            match payload {
                None => w.line(format!("{},", rust_pascal(c))),
                Some(pt) => w.line(format!("{}({}),", rust_pascal(c), rust_ty(pt, "")?)),
            }
        }
        w.close("}");
        w.line("");

        w.open(format!("impl TryFrom<Value> for {p} {{"));
        w.line("type Error = String;");
        w.line("");
        w.open("fn try_from(v: Value) -> Result<Self, String> {");
        w.open("match v {");
        for (i, (c, payload)) in cases.iter().enumerate() {
            let cp = rust_pascal(c);
            match payload {
                None => w.line(format!(
                    "Value::Variant {{ case: {i}, payload: None }} => Ok({p}::{cp}),"
                )),
                Some(pt) => {
                    let lifted = self.lift(pt, "*x", w.ind)?;
                    w.line(format!(
                        "Value::Variant {{ case: {i}, payload: Some(x) }} => Ok({p}::{cp}({lifted})),"
                    ));
                }
            }
        }
        w.line(format!(
            "other => Err(format!(\"{}: invalid variant value {{other:?}}\")),",
            t.wit_name
        ));
        w.close("}");
        w.close("}");
        w.close("}");
        w.line("");

        w.open(format!("impl TryFrom<{p}> for Value {{"));
        w.line("type Error = String;");
        w.line("");
        w.open(format!("fn try_from(v: {p}) -> Result<Value, String> {{"));
        w.open("Ok(match v {");
        for (i, (c, payload)) in cases.iter().enumerate() {
            let cp = rust_pascal(c);
            match payload {
                None => w.line(format!(
                    "{p}::{cp} => Value::Variant {{ case: {i}, payload: None }},"
                )),
                Some(pt) => {
                    w.open(format!("{p}::{cp}(x) => Value::Variant {{"));
                    w.line(format!("case: {i},"));
                    w.line(format!(
                        "payload: Some(Box::new({})),",
                        self.lower(pt, "x", w.ind)?
                    ));
                    w.close("},");
                }
            }
        }
        w.close("})");
        w.close("}");
        w.close("}");
        w.line("");
        Ok(())
    }

    fn emit_enum(&self, w: &mut W, iface: &Iface, t: &NamedTy, p: &str, cases: &[String]) {
        w.line(format!(
            "/// Mirrors the WIT enum `{}` ({}); cases in declaration order.",
            t.wit_name, iface.instance
        ));
        w.line("#[derive(Debug, Clone, Copy, PartialEq, Eq)]");
        w.open(format!("pub enum {p} {{"));
        for c in cases {
            w.line(format!("{},", rust_pascal(c)));
        }
        w.close("}");
        w.line("");

        w.open(format!("impl TryFrom<Value> for {p} {{"));
        w.line("type Error = String;");
        w.line("");
        w.open("fn try_from(v: Value) -> Result<Self, String> {");
        w.open("match v {");
        for (i, c) in cases.iter().enumerate() {
            w.line(format!("Value::Enum({i}) => Ok({p}::{}),", rust_pascal(c)));
        }
        w.line(format!(
            "other => Err(format!(\"{}: invalid enum value {{other:?}}\")),",
            t.wit_name
        ));
        w.close("}");
        w.close("}");
        w.close("}");
        w.line("");

        w.open(format!("impl TryFrom<{p}> for Value {{"));
        w.line("type Error = String;");
        w.line("");
        w.open(format!("fn try_from(v: {p}) -> Result<Value, String> {{"));
        w.open("Ok(Value::Enum(match v {");
        for (i, c) in cases.iter().enumerate() {
            w.line(format!("{p}::{} => {i},", rust_pascal(c)));
        }
        w.close("}))");
        w.close("}");
        w.close("}");
        w.line("");
    }

    fn emit_flags(&self, w: &mut W, iface: &Iface, t: &NamedTy, p: &str, flags: &[String]) {
        let n = flags.len();
        w.line(format!(
            "/// Mirrors the WIT flags `{}` ({}); bit i = flag i. Combine with `|`,",
            t.wit_name, iface.instance
        ));
        w.line("/// test with `contains`.");
        w.line("#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]");
        w.line(format!("pub struct {p}(pub u64);"));
        w.line("");
        w.open(format!("impl {p} {{"));
        for (i, f) in flags.iter().enumerate() {
            w.line(format!(
                "pub const {}: {p} = {p}(1 << {i});",
                rust_screaming(f)
            ));
        }
        w.line("");
        w.line("/// True if every flag set in `other` is also set in `self`.");
        w.open(format!("pub fn contains(self, other: {p}) -> bool {{"));
        w.line("self.0 & other.0 == other.0");
        w.close("}");
        w.close("}");
        w.line("");
        w.open(format!("impl std::ops::BitOr for {p} {{"));
        w.line(format!("type Output = {p};"));
        w.line("");
        w.open(format!("fn bitor(self, rhs: {p}) -> {p} {{"));
        w.line(format!("{p}(self.0 | rhs.0)"));
        w.close("}");
        w.close("}");
        w.line("");

        w.open(format!("impl TryFrom<Value> for {p} {{"));
        w.line("type Error = String;");
        w.line("");
        w.open("fn try_from(v: Value) -> Result<Self, String> {");
        w.open("let bits = match v {");
        w.line("Value::Flags(bits) => bits,");
        w.line(format!(
            "other => return Err(format!(\"{}: expected flags, got {{other:?}}\")),",
            t.wit_name
        ));
        w.close("};");
        w.open(format!("if bits.len() != {n} {{"));
        w.line(format!(
            "return Err(format!(\"{}: expected {n} flags, got {{}}\", bits.len()));",
            t.wit_name
        ));
        w.close("}");
        w.line(format!("let mut v = {p}(0);"));
        w.open("for (i, set) in bits.into_iter().enumerate() {");
        w.open("if set {");
        w.line("v.0 |= 1 << i;");
        w.close("}");
        w.close("}");
        w.line("Ok(v)");
        w.close("}");
        w.close("}");
        w.line("");

        w.open(format!("impl TryFrom<{p}> for Value {{"));
        w.line("type Error = String;");
        w.line("");
        w.line(format!(
            "/// Fails on bits beyond the {n} declared flags — the wire cannot carry them."
        ));
        w.open(format!("fn try_from(v: {p}) -> Result<Value, String> {{"));
        if n < 64 {
            w.open(format!("if v.0 >> {n} != 0 {{"));
            w.line(format!(
                "return Err(format!(\"{}: unknown bits in {{:#x}}\", v.0));",
                t.wit_name
            ));
            w.close("}");
        }
        w.line(format!(
            "Ok(Value::Flags((0..{n}).map(|i| v.0 & (1 << i) != 0).collect()))"
        ));
        w.close("}");
        w.close("}");
        w.line("");
    }

    // -- function signatures -------------------------------------------------------

    /// "fn try_divide(&self, num: f64, den: f64) -> Result<f64, String>" —
    /// shared by the trait, the scaffolded stubs, and missing_impls output.
    fn method_sig(&self, f: &Func, qual: &str) -> Result<String> {
        let mut params = vec!["&self".to_string()];
        for (n, t) in &f.params {
            params.push(format!("{}: {}", rust_param(n), rust_ty(t, qual)?));
        }
        let ret = match self.classify(f)? {
            Ret::None => "Result<(), String>".to_string(),
            Ret::Plain(t) => format!("Result<{}, String>", rust_ty(t, qual)?),
            Ret::ResStr { ok, .. } => match ok {
                Some(t) => format!("Result<{}, String>", rust_ty(t, qual)?),
                None => "Result<(), String>".to_string(),
            },
            Ret::ResTyped(t) => format!("Result<{}, String>", rust_ty(t, qual)?),
        };
        Ok(format!(
            "fn {}({}) -> {ret}",
            rust_ident(&f.wit_name),
            params.join(", ")
        ))
    }

    /// One doc line describing where a method's returned Err goes.
    fn err_doc(&self, f: &Func) -> Result<&'static str> {
        Ok(match self.classify(f)? {
            Ret::ResStr { has_msg: true, .. } => {
                "/// An Err return encodes as the WIT result err case (a normal status-0 reply)."
            }
            Ret::ResStr { has_msg: false, .. } => {
                "/// An Err return encodes as the WIT result err case (no payload: the message is dropped)."
            }
            Ret::ResTyped(_) => {
                "/// The Ok value is the WIT result, returned whole; an Err return is a function-level failure (status-1 reply)."
            }
            _ => "/// An Err return is a function-level failure (status-1 reply).",
        })
    }

    // -- src/gen/mod.rs --------------------------------------------------------------

    fn mod_rs(&self) -> Result<String> {
        let mut w = W::spaces4();

        for iface in self.m.exports.iter().chain(&self.m.imports) {
            for t in &iface.types {
                self.emit_named_ty(&mut w, iface, t)?;
            }
        }

        let export = &self.m.exports[0];
        w.line("/// The application interface: one method per function exported by");
        w.line(format!(
            "/// {}. src/app.rs implements it on `App`.",
            export.instance
        ));
        w.open(format!("pub trait {} {{", self.trait_name));
        for (i, f) in export.funcs.iter().enumerate() {
            if i > 0 {
                w.line("");
            }
            w.line(format!("/// Handles {}#{}.", export.instance, f.wit_name));
            w.line("///");
            w.line(self.err_doc(f)?);
            w.line(format!("{};", self.method_sig(f, "")?));
        }
        w.close("}");
        w.line("");

        w.line("/// Registers every exported function: the WIRE type trees of its params");
        w.line("/// plus the adapter that lifts `Vec<Value>` into native types, calls");
        w.line("/// `crate::app::App`, and lowers the result.");
        let r_name = if export.funcs.is_empty() { "_r" } else { "r" };
        w.open(format!("pub fn setup({r_name}: &mut Registry) {{"));
        for f in &export.funcs {
            w.open("r.register(");
            w.line(format!("\"{}#{}\",", export.instance, f.wit_name));
            if f.params.is_empty() {
                w.line("Vec::new(),");
            } else {
                w.open("vec![");
                for (_, t) in &f.params {
                    w.line(format!("{},", type_tree(t, w.ind)?));
                }
                w.close("],");
            }
            w.line(format!("handle_{},", rust_snake(&f.wit_name)));
            w.close(");");
        }
        w.close("}");
        w.line("");

        for f in &export.funcs {
            self.emit_handler(&mut w, export, f)?;
        }

        for iface in &self.m.imports {
            let is = rust_snake(iface_short(&iface.instance));
            for f in &iface.funcs {
                self.emit_mesh_wrapper(&mut w, iface, &is, f)?;
            }
        }

        let mut out = String::new();
        out.push_str(GENERATED_HEADER);
        out.push_str("\n//\n");
        out.push_str(&format!(
            "// Typed bindings for WIT package {}, world {}: native declarations\n",
            self.m.package, self.m.world
        ));
        out.push_str("// for every WIT type with Value conversions, the application trait\n");
        out.push_str(&format!(
            "// (`{}`, implemented in src/app.rs), and setup() wiring the\n",
            self.trait_name
        ));
        out.push_str("// adapters into crab-sdk's Registry.\n");
        out.push_str("//\n");
        out.push_str("// Error semantics (WIRE.md): a method for a WIT function returning\n");
        out.push_str("// result<T, string> (or result with no err payload) maps an Err to the\n");
        out.push_str("// WIRE result ERR CASE — a normal status-0 reply carrying an encoded\n");
        out.push_str("// result value. Every other method's Err is a function-level failure:\n");
        out.push_str("// the runtime replies status 1 with the message.\n");
        out.push('\n');
        out.push_str("// The bindings are a surface API: the app may use any subset of the\n");
        out.push_str("// generated types/consts/wrappers — and generated code is exempt from\n");
        out.push_str("// style lints.\n");
        out.push_str("#![allow(dead_code)]\n");
        out.push_str("#![allow(clippy::all)]\n\n");
        if w.buf.contains("Type::") || w.buf.contains("-> Type") {
            out.push_str("use crab_sdk::{Registry, Type, Value};\n\n");
        } else {
            out.push_str("use crab_sdk::{Registry, Value};\n\n");
        }
        out.push_str("/// Resolved-WIT JSON (gen/schema.json), served verbatim via crab_schema.\n");
        out.push_str("pub const SCHEMA: &str = include_str!(\"../../gen/schema.json\");\n\n");
        out.push_str(&w.buf);
        Ok(trim_final(&out))
    }

    fn emit_handler(&self, w: &mut W, iface: &Iface, f: &Func) -> Result<()> {
        let s = rust_snake(&f.wit_name);
        w.line(format!(
            "/// Adapter for {}#{}; registered in setup().",
            iface.instance, f.wit_name
        ));
        let args_name = if f.params.is_empty() { "_args" } else { "args" };
        w.open(format!(
            "fn handle_{s}({args_name}: Vec<Value>) -> Result<Value, String> {{"
        ));
        if !f.params.is_empty() {
            w.line("let mut args = args.into_iter();");
            for (n, t) in &f.params {
                let pn = rust_param(n);
                let expr = format!("args.next().ok_or(\"missing param `{n}`\")?");
                w.line(format!("let {pn} = {};", self.lift(t, &expr, w.ind)?));
            }
        }
        let call = format!(
            "crate::app::App.{}({})",
            rust_ident(&f.wit_name),
            f.params
                .iter()
                .map(|(n, _)| rust_param(n))
                .collect::<Vec<_>>()
                .join(", ")
        );
        match self.classify(f)? {
            Ret::None => {
                w.line(format!("{call}?;"));
                w.line("Ok(Value::unit())");
            }
            Ret::Plain(t) | Ret::ResTyped(t) => {
                w.line(format!("let r = {call}?;"));
                w.line(format!("Ok({})", self.lower(t, "r", w.ind)?));
            }
            Ret::ResStr { ok, has_msg } => {
                w.open(format!("match {call} {{"));
                match ok {
                    Some(t) => w.line(format!(
                        "Ok(r) => Ok(Value::Result(Ok(Some(Box::new({}))))),",
                        self.lower(t, "r", w.ind)?
                    )),
                    None => w.line("Ok(()) => Ok(Value::Result(Ok(None))),"),
                }
                if has_msg {
                    w.line("Err(e) => Ok(Value::Result(Err(Some(Box::new(Value::String(e)))))),");
                } else {
                    w.line("// the WIT err side has no payload: the message is dropped");
                    w.line("Err(_) => Ok(Value::Result(Err(None))),");
                }
                w.close("}");
            }
        }
        w.close("}");
        w.line("");
        Ok(())
    }

    fn emit_mesh_wrapper(&self, w: &mut W, iface: &Iface, is: &str, f: &Func) -> Result<()> {
        let name = format!("{is}_{}", rust_snake(&f.wit_name));
        let addr = format!("{}#{}", iface.instance, f.wit_name);
        let ret = self.classify(f)?;
        let mut sig_params = vec!["workload: &str".to_string()];
        for (n, t) in &f.params {
            sig_params.push(format!("{}: {}", rust_param(n), rust_ty(t, "")?));
        }
        let sig_ret = match &ret {
            Ret::None => "Result<(), String>".to_string(),
            Ret::Plain(t) | Ret::ResTyped(t) => {
                format!("Result<{}, String>", rust_ty(t, "")?)
            }
            Ret::ResStr { ok, .. } => match ok {
                Some(t) => format!("Result<{}, String>", rust_ty(t, "")?),
                None => "Result<(), String>".to_string(),
            },
        };
        w.line(format!("/// Calls {addr} on the workload named"));
        w.line("/// `workload` through the host mesh (crabcraft.call). The Err covers");
        match ret {
            Ret::ResStr { .. } => {
                w.line("/// transport failures, remote status-1 failures, AND the WIT result");
                w.line("/// err case.");
            }
            _ => w.line("/// transport failures and remote status-1 failures."),
        }
        w.open(format!(
            "pub fn {name}({}) -> {sig_ret} {{",
            sig_params.join(", ")
        ));
        let params_expr = if f.params.is_empty() {
            "&[]".to_string()
        } else {
            w.line("let mut params = Vec::new();");
            for (n, t) in &f.params {
                w.line(format!(
                    "crab_sdk::codec::encode(&{}, &mut params);",
                    self.lower(t, &rust_param(n), w.ind)?
                ));
            }
            "&params".to_string()
        };
        w.line(format!(
            "let reply = crab_sdk::mesh_call(workload, \"{addr}\", {params_expr})?;"
        ));
        match ret {
            Ret::None => {
                w.line("crab_sdk::decode(&Type::Tuple(Vec::new()), &reply)?;");
                w.line("Ok(())");
            }
            Ret::Plain(t) | Ret::ResTyped(t) => {
                w.line(format!(
                    "let v = crab_sdk::decode(&{}, &reply)?;",
                    type_tree(t, w.ind)?
                ));
                w.line(format!("Ok({})", self.lift(t, "v", w.ind)?));
            }
            Ret::ResStr { ok, has_msg } => {
                let rty = f.result.as_ref().expect("ResStr has a result type");
                w.line(format!(
                    "let v = crab_sdk::decode(&{}, &reply)?;",
                    type_tree(rty, w.ind)?
                ));
                w.open("match v {");
                match ok {
                    Some(t) => {
                        w.line(format!(
                            "Value::Result(Ok(Some(x))) => Ok({}),",
                            self.lift(t, "*x", w.ind)?
                        ));
                        w.line(
                            "Value::Result(Ok(None)) => Err(\"missing result payload\".to_string()),",
                        );
                    }
                    None => {
                        w.line("Value::Result(Ok(None)) => Ok(()),");
                        w.line(
                            "Value::Result(Ok(Some(x))) => Err(format!(\"unexpected result payload {x:?}\")),",
                        );
                    }
                }
                if has_msg {
                    w.line("Value::Result(Err(Some(x))) => Err(String::try_from(*x)?),");
                    w.line("Value::Result(Err(None)) => Err(\"missing err payload\".to_string()),");
                } else {
                    w.line(format!(
                        "Value::Result(Err(_)) => Err(\"{}: err result (no payload)\".to_string()),",
                        f.wit_name
                    ));
                }
                w.line("other => Err(format!(\"expected result, got {other:?}\")),");
                w.close("}");
            }
        }
        w.close("}");
        w.line("");
        Ok(())
    }

    // -- src/lib.rs --------------------------------------------------------------

    fn lib_rs(&self) -> String {
        let export = &self.m.exports[0];
        format!(
            r#"{GENERATED_HEADER}
//
// {name}: crabcraft guest module implementing {instance}
// (see {name}.wit). A wasm32-wasip1 reactor cdylib: this file only wires the
// generated bindings (src/gen/) and the application (src/app.rs) into the
// WIRE.md section 2 ABI. Regenerated on every `crabgen regen` — your code
// belongs in src/app.rs.

mod app;
mod gen;

crab_sdk::export_abi!(schema: gen::SCHEMA, init: gen::setup);
"#,
            name = self.project,
            instance = export.instance,
        )
    }

    // -- scaffold files ------------------------------------------------------------

    fn app_rs(&self) -> Result<String> {
        let export = &self.m.exports[0];
        let mut w = W::spaces4();
        w.line(format!(
            "//! The application half of this guest: implement `gen::{}` here.",
            self.trait_name
        ));
        w.line("//! crabgen scaffolds this file ONCE and never overwrites it; `crabgen regen`");
        w.line("//! prints any missing method signatures instead of editing it.");
        w.line("");
        // Only pull in `gen::` types when a signature references them.
        let mut sigs = Vec::new();
        for f in &export.funcs {
            sigs.push(self.method_sig(f, "gen::")?);
        }
        if sigs.iter().any(|s| s.contains("gen::")) {
            w.line(format!("use crate::gen::{{self, {}}};", self.trait_name));
        } else {
            w.line(format!("use crate::gen::{};", self.trait_name));
        }
        w.line("");
        w.line(format!(
            "/// App implements gen::{}: one method per function exported by",
            self.trait_name
        ));
        w.line(format!("/// {}.", export.instance));
        w.line("pub struct App;");
        w.line("");
        if export.funcs.is_empty() {
            w.line(format!("impl {} for App {{}}", self.trait_name));
            return Ok(trim_final(&w.buf));
        }
        w.open(format!("impl {} for App {{", self.trait_name));
        for (i, f) in export.funcs.iter().enumerate() {
            if i > 0 {
                w.line("");
            }
            w.line(format!("/// Handles {}#{}.", export.instance, f.wit_name));
            w.line("///");
            w.line(self.err_doc(f)?);
            w.open(format!("{} {{", sigs[i]));
            if !f.params.is_empty() {
                let names: Vec<String> = f.params.iter().map(|(n, _)| rust_param(n)).collect();
                if names.len() == 1 {
                    w.line(format!("let _ = {};", names[0]));
                } else {
                    w.line(format!("let _ = ({});", names.join(", ")));
                }
            }
            w.line(format!("Err(\"unimplemented: {}\".into())", f.wit_name));
            w.close("}");
        }
        w.close("}");
        Ok(trim_final(&w.buf))
    }

    fn cargo_toml(&self) -> String {
        let dep = if self.m.imports.is_empty() {
            "crab-sdk = { path = \"../crab-sdk\" }".to_string()
        } else {
            // the world imports interfaces: the generated mesh wrappers call
            // crab_sdk::mesh_call, which is behind the opt-in "mesh" feature
            "crab-sdk = { path = \"../crab-sdk\", features = [\"mesh\"] }".to_string()
        };
        format!(
            r#"# crabcraft guest crate (Rust lane), scaffolded by crabgen. Written once and
# never overwritten — but note: if the WIT later GAINS imports, add
# `features = ["mesh"]` to the crab-sdk dependency (regen will remind you).
[package]
name = "{name}"
version = "0.1.0"
edition = "2021"
description = "crabcraft guest module: {instance}"

[lib]
crate-type = ["cdylib"]

[dependencies]
{dep}
"#,
            name = self.project,
            instance = self.m.exports[0].instance,
        )
    }
}

// ---------------------------------------------------------------------------
// workspace membership
// ---------------------------------------------------------------------------

/// Insert `member` into the root manifest's `members = [...]` array if
/// absent (at the position where it sorts relative to the existing entries,
/// preserving their order and the array's one-line/multi-line style).
/// Idempotent: a present member is left untouched.
fn add_workspace_member(root_cargo: &Path, member: &str) -> Result<()> {
    let src = fs::read_to_string(root_cargo)
        .with_context(|| format!("reading {}", root_cargo.display()))?;
    // Line-anchored `members = [` match: a bare substring search could land
    // inside a comment or `default-members`, making the NEXT '[' (e.g. the
    // one in `[workspace]`) the array and corrupting the manifest.
    let mut off = 0;
    let mut anchor = None;
    for line in src.split_inclusive('\n') {
        let t = line.trim_start();
        if !t.starts_with('#') {
            if let Some(rest) = t.strip_prefix("members") {
                if rest.trim_start().starts_with('=') {
                    anchor = Some(off);
                    break;
                }
            }
        }
        off += line.len();
    }
    let start =
        anchor.ok_or_else(|| anyhow!("{}: no workspace members array", root_cargo.display()))?;
    let open = src[start..]
        .find('[')
        .map(|i| start + i)
        .ok_or_else(|| anyhow!("{}: malformed members array", root_cargo.display()))?;
    let close = src[open..]
        .find(']')
        .map(|i| open + i)
        .ok_or_else(|| anyhow!("{}: malformed members array", root_cargo.display()))?;
    let body = &src[open + 1..close];
    if body.contains('#') {
        bail!(
            "{}: comments inside the members array are not supported by crabgen's editor; \
             add \"{member}\" to it by hand",
            root_cargo.display()
        );
    }
    let items: Vec<String> = body
        .split(',')
        .map(|s| s.trim().trim_matches('"').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if items.iter().any(|i| i == member) {
        return Ok(());
    }
    let mut new_items = items.clone();
    let pos = new_items
        .iter()
        .position(|i| i.as_str() > member)
        .unwrap_or(new_items.len());
    new_items.insert(pos, member.to_string());
    let rendered = if body.contains('\n') {
        let indent = "    ";
        let inner = new_items
            .iter()
            .map(|i| format!("{indent}\"{i}\",\n"))
            .collect::<String>();
        format!("\n{inner}")
    } else {
        new_items
            .iter()
            .map(|i| format!("\"{i}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let out = format!("{}[{}]{}", &src[..open], rendered, &src[close + 1..]);
    fs::write(root_cargo, out).with_context(|| format!("writing {}", root_cargo.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// scaffold templates
// ---------------------------------------------------------------------------

fn build_sh(name: &str) -> String {
    let underscored = name.replace('-', "_");
    format!(
        r#"#!/usr/bin/env bash
# Build the {name} reactor module (scaffolded by crabgen; edit freely — this
# file is written once and never overwritten).
#
# cdylib + wasm32-wasip1 = a REACTOR: the wasm exports the crab_* ABI and
# needs no `_start` at invoke time. rustup-from-nix because nixpkgs' plain
# rustc ships no wasm32-wasip1 std; if the stable toolchain lacks the
# target, run `rustup target add wasm32-wasip1` once.
set -euo pipefail
cd "$(dirname "$0")"

nix shell nixpkgs#rustup --command cargo build --release --target wasm32-wasip1 -p {name}
cp ../../target/wasm32-wasip1/release/{underscored}.wasm ../../modules/{name}.wasm

# SIMD tripwire: the wasmcraft engine refuses 0xfd-prefixed (SIMD) opcodes.
# wasm-objdump prints opcode bytes as "fd 0c" (no 0x prefix) and SIMD
# mnemonics as v128.*/i8x16.*/... after the "|" column; anchor the mnemonic
# check to that column so a symbol name containing e.g. "i32x4" can't
# false-positive. The byte check can in rare cases match a wrapped non-SIMD
# instruction's continuation byte (loud + debuggable, accepted).
disasm="$(nix shell nixpkgs#wabt --command wasm-objdump -d ../../modules/{name}.wasm)"
if grep -qE '\| +(v128|i8x16|i16x8|i32x4|i64x2|f32x4|f64x2|f16x8)\.' <<<"$disasm" ||
   grep -qE '^ *[0-9a-f]+: fd' <<<"$disasm"; then
  echo 'FATAL: SIMD opcodes in output wasm' >&2
  exit 1
fi

ls -la ../../modules/{name}.wasm
"#
    )
}

fn readme(name: &str, m: &Module) -> String {
    let instance = &m.exports[0].instance;
    format!(
        r#"# {name} — crabcraft guest (Rust lane)

<!-- generated by crabgen — edits will be overwritten on regen -->

Generated by crabgen. `{name}.wit` is the source of truth; `gen/`,
`src/gen/` and `src/lib.rs` (and this README) are GENERATED — never edit
them, crabgen rewrites them wholesale on every regen. Your code lives in
`src/app.rs` (crabgen never touches it). `Cargo.toml` and `build.sh` are
scaffolded once and then yours.

## Build

    ./build.sh

cargo (rustup via nix) builds a wasm32-wasip1 reactor cdylib, copied to
`../../modules/{name}.wasm`, then the script fails hard if any SIMD (0xfd)
opcodes snuck in — the wasmcraft engine refuses them. Host-side checks run
with plain cargo: `cargo check -p {name}`.

## Deploy

Add a manifest and apply it in-game with `crb apply` (exported interface:
`{instance}`):

```yaml
name: {name}
wasm: <URL serving modules/{name}.wasm>
kind: reactor
schema: <URL serving the resolved-WIT JSON>   # serve a copy of gen/schema.json
```

## Maintenance loop

1. Edit `{name}.wit`.
2. `crabgen regen guest/{name}` — rewrites the generated files and prints
   typed signatures for any methods missing from `src/app.rs`.
3. Paste the stubs into `src/app.rs` and implement them.
4. `./build.sh`, redeploy, invoke.

`crabgen check` (run it in CI/pre-commit) fails while gen/ is stale.

If the WIT gains `import`ed interfaces later, enable crab-sdk's mesh
feature in Cargo.toml (`crabgen regen` reminds you):
`crab-sdk = {{ path = "../crab-sdk", features = ["mesh"] }}`.

## WIT versioning

Backwards-compatible evolution: add new functions freely, and add new
inputs as `option<T>` (old callers simply omit them). Anything else is a
breaking change: bump the package version (`@0.1.0` → `@0.2.0`) and the
daemon serves both versions side by side.
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pascal_titles_each_dash_segment() {
        assert_eq!(rust_pascal("echo-everything"), "EchoEverything");
        assert_eq!(rust_pascal("a-u8"), "AU8");
        assert_eq!(rust_pascal("e2e-rust"), "E2eRust");
        assert_eq!(rust_pascal("x"), "X");
    }

    #[test]
    fn snake_lowercases_every_segment() {
        assert_eq!(rust_snake("echo-everything"), "echo_everything");
        assert_eq!(rust_snake("AB"), "ab");
        assert_eq!(rust_snake("a-u8"), "a_u8");
    }

    #[test]
    fn idents_are_keyword_mangled() {
        assert_eq!(rust_ident("type"), "type_");
        assert_eq!(rust_ident("self"), "self_");
        assert_eq!(rust_ident("match"), "match_");
        assert_eq!(rust_ident("name"), "name");
    }

    #[test]
    fn params_avoid_generated_locals() {
        assert_eq!(rust_param("args"), "args_");
        assert_eq!(rust_param("workload"), "workload_");
        assert_eq!(rust_param("params"), "params_");
        assert_eq!(rust_param("reply"), "reply_");
        assert_eq!(rust_param("v"), "v_");
        assert_eq!(rust_param("x0"), "x0_"); // tuple-temp shaped
        assert_eq!(rust_param("xs"), "xs"); // lift internals never share scope
        assert_eq!(rust_param("loop"), "loop_"); // keyword
        assert_eq!(rust_param("num"), "num");
    }

    #[test]
    fn screaming_for_flags_consts() {
        assert_eq!(rust_screaming("read"), "READ");
        assert_eq!(rust_screaming("read-write"), "READ_WRITE");
    }

    #[test]
    fn workspace_member_insertion_is_sorted_and_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Cargo.toml");
        fs::write(
            &path,
            "[workspace]\nresolver = \"2\"\nmembers = [\"guest/crab-sdk\", \"guest/hello\", \"tools/crabgen\"]\n",
        )
        .unwrap();
        add_workspace_member(&path, "guest/full").unwrap();
        let got = fs::read_to_string(&path).unwrap();
        assert_eq!(
            got,
            "[workspace]\nresolver = \"2\"\nmembers = [\"guest/crab-sdk\", \"guest/full\", \"guest/hello\", \"tools/crabgen\"]\n"
        );
        // idempotent
        add_workspace_member(&path, "guest/full").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), got);
    }

    #[test]
    fn workspace_member_anchor_ignores_comments_mentioning_members() {
        // A comment above [workspace] containing the word "members" must not
        // become the anchor (the first '[' after it would be [workspace]'s,
        // corrupting the manifest).
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Cargo.toml");
        fs::write(
            &path,
            "# workspace members live below; keep default-members unset\n\
             [workspace]\nmembers = [\"guest/a\"]\n",
        )
        .unwrap();
        add_workspace_member(&path, "guest/b").unwrap();
        let got = fs::read_to_string(&path).unwrap();
        assert_eq!(
            got,
            "# workspace members live below; keep default-members unset\n\
             [workspace]\nmembers = [\"guest/a\", \"guest/b\"]\n"
        );
    }

    #[test]
    fn workspace_member_multiline_style_preserved() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("Cargo.toml");
        fs::write(
            &path,
            "[workspace]\nmembers = [\n    \"guest/a\",\n    \"guest/z\",\n]\n",
        )
        .unwrap();
        add_workspace_member(&path, "guest/m").unwrap();
        let got = fs::read_to_string(&path).unwrap();
        assert_eq!(
            got,
            "[workspace]\nmembers = [\n    \"guest/a\",\n    \"guest/m\",\n    \"guest/z\",\n]\n"
        );
    }
}
