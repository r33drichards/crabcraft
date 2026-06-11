//! AssemblyScript backend: typed bindings + dispatch + mesh wrappers +
//! scaffold over the templates/ts runtime (asc wasm reactor lane).
//!
//! Layout note (the one backend whose sources do NOT live in gen/): asc
//! compiles from assembly/, so the driver-owned gen/ holds only schema.json +
//! MANIFEST and this backend additionally owns assembly/gen/ (wiped and
//! recreated on every generate — that is how mesh.ts disappears when the WIT
//! loses its imports) and assembly/index.ts (regenerated; precedent: the
//! Rust lane regenerates src/gen/ + src/lib.rs outside gen/). The
//! hand-written half is assembly/impl.ts (scaffold-once).
//!
//! Type mapping (WIT → AssemblyScript):
//!
//! | WIT              | AssemblyScript                                          |
//! |------------------|---------------------------------------------------------|
//! | bool             | bool                                                    |
//! | u8 u16 u32 u64   | u8 u16 u32 u64                                          |
//! | s8 s16 s32 s64   | i8 i16 i32 i64                                          |
//! | f32 f64          | f32 f64                                                 |
//! | char             | u32 (unicode scalar, validated on the wire)             |
//! | string           | string (UTF-16 in memory; UTF-8 validated on the wire)  |
//! | list<T>          | Array<T> (decode appends element-by-element: no blind   |
//! |                  | pre-allocation from the attacker-controlled count, the  |
//! |                  | analog of the sibling lanes' 4096 reserve clamp)        |
//! | option<T>        | `T | null` when T maps to a non-nullable REFERENCE type |
//! |                  | (string, Array, record/variant/flags/tuple/result       |
//! |                  | classes, option boxes); when T is a VALUE type          |
//! |                  | (numbers, bool, char, enum) or itself an option — AS    |
//! |                  | cannot express `valueType | null`, and nested nulls     |
//! |                  | would be ambiguous — a generated monomorphic box class  |
//! |                  | `Option<Token>` stands in: null = none,                 |
//! |                  | `new OptionBool(v)` = some(v)                           |
//! | tuple<A, B, ..>  | generated class `Tuple<N><TokA><TokB>..` with fields    |
//! |                  | f0..fN-1 (one class per distinct shape)                 |
//! | record           | class with camelCase fields, every field initialized    |
//! |                  | (construct with `new X()` and assign fields)            |
//! | variant          | class with `tag: i32` + one payload field per payload   |
//! |                  | case (camelCase; value payloads are plain fields, ref   |
//! |                  | payloads are nullable), `TAG_*` consts and `new<Case>`  |
//! |                  | static factories                                        |
//! | enum             | enum (i32-backed), cases in declaration order           |
//! | flags            | class { bits: u64 } + SCREAMING bit consts (max 64)     |
//! | result<T, E>     | value position: generated class `Result<TokT><TokE>`    |
//! |                  | (isErr selects the populated side; absent sides have no |
//! |                  | field). Function-RESULT position: see Res below.        |
//!
//! Function boundary (mirroring the Go/Rust/C++ lanes' Ret classification):
//! AS has no multi-returns and no exceptions across the boundary, so every
//! impl function returns a generated monomorphic Res class
//! (`Res<TokenOfV>`, `ResVoid` when there is no value) holding exactly one
//! of value/err — build with `ResX.ok(v)` / `ResX.fail(msg)`:
//! - no WIT result        → ResVoid;  non-null .err = status-1 reply
//! - plain value T        → Res<T>;   non-null .err = status-1 reply
//! - result<T, string> or result with absent err payload
//!                        → Res<T> (ResVoid when the ok side is absent); a
//!                          non-null .err is the WIRE result ERR CASE
//!                          (status stays 0)
//! - result<T, E> (other E) → Res<Result<TokT><TokE>>; .err = status-1
//!
//! Name casing: WIT kebab-case → PascalCase for types / generated shape
//! classes / factories ("echo-everything" → "EchoEverything", "a-u8" →
//! "AU8" — capitalize each dash segment, no acronym table), camelCase for
//! functions, params, fields and enum members (first segment lowercased),
//! SCREAMING_SNAKE for flags consts and variant TAG_* consts. Names hitting
//! an AS/TS keyword, an AS basic-type name, or a local the generated code
//! declares in the same scope get a trailing underscore.
//!
//! Declaration order: AS classes (like TS) may be referenced before their
//! declaration — field initializers run at `new` time, not module-load time
//! — so types are emitted in WIT declaration order (exports before imports)
//! with no dependency sorting (verified empirically against asc 0.28).
//!
//! Codec/error convention (matches templates/ts/assembly/gen/runtime.ts):
//! Decoder carries a sticky `err: string | null` — generated code checks
//! `d.err !== null` after every decode call (and inside every list loop, so
//! a hostile count cannot spin). Sink errors are also sticky and checked
//! ONCE after encoding; generated encode validators (enum range, flags
//! bits, variant tag/payload) set s.err and return early — bytes appended
//! after an error are discarded with the reply, never sent.
//!
//! Collisions: every identifier emitted at bindings.ts module scope is
//! checked against RESERVED (the names bindings.ts imports from runtime.ts/
//! mesh.ts/schema.ts, its own fixed declarations, and the AS stdlib names
//! generated code references) and each other; record fields, variant cases,
//! enum members, flags and params are checked within their own scopes.
//! Generated shape-class names (Option*/Tuple*/Result*/Res*) are claimed
//! after the WIT-derived names, so a WIT type landing on one fails loudly.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

use crate::backend::Backend;
use crate::emit::{iface_short, is_temp_shaped, project_name, trim_final, GENERATED_HEADER, W};
use crate::ir::{Func, Iface, Module, NamedTy, Ty};

const RUNTIME_TS: &str = include_str!("../templates/ts/assembly/gen/runtime.ts");
const MESH_TS: &str = include_str!("../templates/ts/assembly/gen/mesh.ts");
const PACKAGE_JSON: &str = include_str!("../templates/ts/package.json");
const PACKAGE_LOCK: &str = include_str!("../templates/ts/package-lock.json");

/// The placeholder project name in the package.json/package-lock.json
/// templates; scaffold() renames it to the project.
const TEMPLATE_PKG_NAME: &str = "crabcraft-guest";

/// Names visible at bindings.ts module scope besides the WIT-derived ones; a
/// WIT name mapping onto one of these fails generate(). Three groups:
/// what bindings.ts imports (the exported names of runtime.ts / mesh.ts /
/// schema.ts — grep their `export`s when the templates change), what
/// bindings.ts itself declares (`register`), and the AS stdlib globals the
/// generated code references (shadowing them would break the codecs).
const RESERVED: &[&str] = &[
    // runtime.ts exports
    "Sink",
    "Decoder",
    "HandlerResult",
    "Handler",
    "setSchema",
    "registerHandler",
    "unpinAlloc",
    "crab_alloc",
    "crab_schema",
    "crab_invoke",
    // mesh.ts exports
    "meshCall",
    "parseMeshReply",
    // schema.ts export
    "SCHEMA",
    // bindings.ts's own fixed declaration
    "register",
    // AS stdlib names generated code references
    "Array",
    "Uint8Array",
    "String",
];

/// TS/AS keywords and basic-type names: camelCase identifiers landing here
/// get a trailing underscore. PascalCase names can never collide (these are
/// all lowercase). `constructor` is included — a class field of that name
/// would collide with the constructor slot.
const AS_KEYWORDS: &[&str] = &[
    "abstract",
    "any",
    "as",
    "async",
    "await",
    "bool",
    "boolean",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "constructor",
    "continue",
    "debugger",
    "declare",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "f32",
    "f64",
    "false",
    "finally",
    "for",
    "from",
    "function",
    "get",
    "i16",
    "i32",
    "i64",
    "i8",
    "if",
    "implements",
    "import",
    "in",
    "instanceof",
    "interface",
    "is",
    "isize",
    "keyof",
    "let",
    "module",
    "namespace",
    "never",
    "new",
    "null",
    "number",
    "object",
    "of",
    "package",
    "private",
    "protected",
    "public",
    "readonly",
    "require",
    "return",
    "set",
    "static",
    "string",
    "super",
    "switch",
    "symbol",
    "this",
    "throw",
    "true",
    "try",
    "type",
    "typeof",
    "u16",
    "u32",
    "u64",
    "u8",
    "undefined",
    "unique",
    "usize",
    "v128",
    "var",
    "void",
    "while",
    "with",
    "yield",
];

/// Locals the generated code declares in scopes that also hold WIT params
/// (handler bodies, mesh wrappers); params landing here get a trailing
/// underscore. Codec-helper locals (`v`) live in functions with fixed
/// signatures that never see a param, so `v` is deliberately not listed.
const PARAM_AVOID: &[&str] = &["d", "s", "r", "fin", "workload"];

/// The temp-name prefixes this emitter generates (r0, x1, i2...).
const AS_TEMP_PREFIXES: &[&str] = &["r", "x", "i"];

pub struct AsBackend;

impl Backend for AsBackend {
    fn lang(&self) -> &'static str {
        "ts"
    }

    fn impl_ext(&self) -> &'static str {
        "ts"
    }

    fn impl_file(&self) -> String {
        "assembly/impl.ts".to_string()
    }

    fn generate(&self, m: &Module, dir: &Path) -> Result<()> {
        let g = Gen::new(m)?;
        let asm_gen = dir.join("assembly/gen");
        // assembly/gen is backend-owned the way the driver owns gen/: wiped
        // wholesale so a WIT losing its imports also loses mesh.ts.
        if asm_gen.exists() {
            fs::remove_dir_all(&asm_gen)
                .with_context(|| format!("clearing {}", asm_gen.display()))?;
        }
        fs::create_dir_all(&asm_gen).with_context(|| format!("creating {}", asm_gen.display()))?;
        let write = |name: &str, content: &str| -> Result<()> {
            let path = asm_gen.join(name);
            fs::write(&path, content).with_context(|| format!("writing {}", path.display()))
        };
        write("runtime.ts", RUNTIME_TS)?;
        if !m.imports.is_empty() {
            // mesh.ts (and bindings.ts's `import { meshCall }`) exists only
            // when the world has imports. asc tree-shakes entry-unreachable
            // code, so the crabcraft.call @external only lands in the wasm
            // import section once an impl actually calls a mesh wrapper
            // (proven by tests/golden_as.rs) — the conditional emission
            // keeps import-free WORLDS free of mesh code entirely.
            write("mesh.ts", MESH_TS)?;
        }
        write("bindings.ts", &g.bindings_ts()?)?;
        write("schema.ts", &schema_ts(&m.schema_json))?;
        // index.ts is the asc entry file (only ITS exports become wasm
        // exports); regenerated every time, like src/lib.rs in the Rust lane.
        fs::write(dir.join("assembly/index.ts"), index_ts())?;
        // README quotes WIT-derived content (the export instance), so it is
        // regenerated alongside the bindings.
        let readme_path = dir.join("README.md");
        fs::write(&readme_path, readme(&project_name(dir)?, m))
            .with_context(|| format!("writing {}", readme_path.display()))?;
        Ok(())
    }

    fn scaffold(&self, m: &Module, dir: &Path) -> Result<()> {
        let name = project_name(dir)?;
        let g = Gen::new(m)?;
        fs::create_dir_all(dir.join("assembly"))?;
        let write_once = |rel: &str, content: &str| -> Result<bool> {
            let path = dir.join(rel);
            if path.exists() {
                return Ok(false);
            }
            fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
            Ok(true)
        };
        write_once("assembly/impl.ts", &g.impl_ts()?)?;
        // package.json + the committed lock pin the asc toolchain; only the
        // project name is templated (npm tolerates a lock/name mismatch, but
        // a self-consistent project is what we ship).
        let renamed = |s: &str| s.replace(TEMPLATE_PKG_NAME, &name);
        write_once("package.json", &renamed(PACKAGE_JSON))?;
        write_once("package-lock.json", &renamed(PACKAGE_LOCK))?;
        if write_once("build.sh", &build_sh(&name))? {
            let build = dir.join("build.sh");
            let mut perms = fs::metadata(&build)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&build, perms)?;
        }
        Ok(())
    }

    fn missing_impls(&self, m: &Module, dir: &Path) -> Result<Vec<String>> {
        let g = Gen::new(m)?;
        let src = fs::read_to_string(dir.join("assembly/impl.ts")).unwrap_or_default();
        let mut missing = Vec::new();
        for iface in &m.exports {
            for f in &iface.funcs {
                if !src.contains(&format!("export function {}(", as_ident(&f.wit_name))) {
                    missing.push(g.method_sig(f)?);
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
fn as_pascal(kebab: &str) -> String {
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

/// kebab-case → camelCase: first segment lowercased whole, the rest
/// capitalized on their first char ("a-u8" → "aU8", "AB" → "ab").
fn as_camel(kebab: &str) -> String {
    let mut out = String::new();
    for (i, seg) in kebab.split('-').enumerate() {
        if i == 0 {
            out.push_str(&seg.to_ascii_lowercase());
        } else {
            let mut c = seg.chars();
            if let Some(f) = c.next() {
                out.push(f.to_ascii_uppercase());
                out.push_str(c.as_str());
            }
        }
    }
    out
}

/// kebab-case → SCREAMING_SNAKE ("read-only" → "READ_ONLY"). Keywords are
/// all lowercase, so no mangling is needed.
fn as_screaming(kebab: &str) -> String {
    kebab
        .split('-')
        .map(|seg| seg.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// camel identifier for fields/functions/enum members: keyword-mangled with
/// a trailing underscore (`new` → `new_`, `u32` → `u32_`).
fn as_ident(kebab: &str) -> String {
    let mut s = as_camel(kebab);
    if AS_KEYWORDS.contains(&s.as_str()) {
        s.push('_');
    }
    s
}

/// camel identifier for params: additionally mangled away from the locals
/// generated bodies declare and the `r<N>`/`x<N>`/`i<N>` temp shape.
fn as_param(kebab: &str) -> String {
    let mut s = as_camel(kebab);
    if AS_KEYWORDS.contains(&s.as_str())
        || PARAM_AVOID.contains(&s.as_str())
        || is_temp_shaped(&s, AS_TEMP_PREFIXES)
    {
        s.push('_');
    }
    s
}

// ---------------------------------------------------------------------------
// type expressions
// ---------------------------------------------------------------------------

/// Scalar types: (Sink/Decoder method — same name both directions, AS type).
fn scalar(ty: &Ty) -> Option<(&'static str, &'static str)> {
    Some(match ty {
        Ty::Bool => ("bool", "bool"),
        Ty::U8 => ("u8", "u8"),
        Ty::U16 => ("u16", "u16"),
        Ty::U32 => ("u32", "u32"),
        Ty::U64 => ("u64", "u64"),
        Ty::S8 => ("s8", "i8"),
        Ty::S16 => ("s16", "i16"),
        Ty::S32 => ("s32", "i32"),
        Ty::S64 => ("s64", "i64"),
        Ty::F32 => ("f32", "f32"),
        Ty::F64 => ("f64", "f64"),
        Ty::Char => ("char", "u32"),
        Ty::String => ("string", "string"),
        _ => return None,
    })
}

/// How a function's WIT result maps onto its Res<V> impl signature (mirrors
/// the Go/Rust/C++ lanes' Ret classification).
enum Ret<'a> {
    /// no WIT result → ResVoid (non-null .err = status-1 reply)
    None,
    /// plain value → Res<T> (non-null .err = status-1 reply)
    Plain(&'a Ty),
    /// result<T, string> / result<T> / result<_,_> → Res<T> / ResVoid; a
    /// non-null .err is the WIRE result ERR CASE (status stays 0).
    /// `has_msg` = the err side carries the string payload.
    ResStr { ok: Option<&'a Ty>, has_msg: bool },
    /// result with any other err type → Res<Result<TokT><TokE>>
    /// (non-null .err = status-1 reply)
    ResTyped(&'a Ty),
}

// ---------------------------------------------------------------------------
// encode / decode statement emission
// ---------------------------------------------------------------------------

/// Per-function emission state: fresh temp names and the statement emitted
/// after a `d.err !== null` check (decode failure). Encode never checks
/// inline — Sink errors are sticky and checked once at the end.
struct Cx {
    tmp: usize,
    fail_stmt: String,
}

impl Cx {
    fn new(fail_stmt: impl Into<String>) -> Self {
        Cx {
            tmp: 0,
            fail_stmt: fail_stmt.into(),
        }
    }

    fn fresh(&mut self, base: &str) -> String {
        let n = self.tmp;
        self.tmp += 1;
        format!("{base}{n}")
    }
}

// ---------------------------------------------------------------------------
// the generator
// ---------------------------------------------------------------------------

/// A generated shared-shape class (emitted once per distinct shape).
enum Shape {
    /// `Option<Tok>`: box for an option whose payload AS cannot null.
    OptionBox(Ty),
    /// `Tuple<N><Toks>`: fields f0..fN-1.
    Tuple(Vec<Ty>),
    /// `Result<TokOk><TokErr>`: isErr + present sides.
    ResultVal(Option<Ty>, Option<Ty>),
    /// `Res<Tok>` / `ResVoid`: the impl/mesh function-return wrapper.
    Res(Option<Ty>),
}

struct Gen<'a> {
    m: &'a Module,
    /// WIT type name → definition, across every interface (one module scope).
    types: HashMap<&'a str, &'a Ty>,
    /// Shared shape classes in first-need order, keyed by class name.
    shapes: Vec<(String, Shape)>,
}

impl<'a> Gen<'a> {
    fn new(m: &'a Module) -> Result<Self> {
        let mut g = Gen {
            m,
            types: HashMap::new(),
            shapes: Vec::new(),
        };
        for iface in m.exports.iter().chain(&m.imports) {
            for t in &iface.types {
                if g.types.insert(t.wit_name.as_str(), &t.ty).is_some() {
                    bail!(
                        "type `{}` is declared in more than one interface; the AssemblyScript \
                         lane puts every type in one module — rename one of them",
                        t.wit_name
                    );
                }
            }
        }
        g.collect_shapes()?;
        g.validate()?;
        Ok(g)
    }

    /// Follow Named references to the defining type (alias chains too).
    fn resolve(&self, ty: &'a Ty) -> Result<&'a Ty> {
        Ok(self.resolve_named(ty)?.1)
    }

    /// Like resolve, but also reports the LAST WIT name on the alias chain
    /// (None when `ty` is structural) — needed to construct/reference the
    /// defining class or enum (`new` and enum members do not work through
    /// type aliases).
    fn resolve_named(&self, ty: &'a Ty) -> Result<(Option<&'a str>, &'a Ty)> {
        let mut name = None;
        let mut t = ty;
        for _ in 0..64 {
            match t {
                Ty::Named(n) => {
                    name = Some(n.as_str());
                    t = self
                        .types
                        .get(n.as_str())
                        .copied()
                        .ok_or_else(|| anyhow!("unresolved type reference `{n}`"))?;
                }
                _ => return Ok((name, t)),
            }
        }
        bail!("type alias cycle while resolving `{ty:?}`");
    }

    /// Does option<t> need a box class? AS cannot express `T | null` for a
    /// value-typed T, and an option whose payload is itself nullable would
    /// make null ambiguous.
    fn needs_box(&self, t: &'a Ty) -> Result<bool> {
        Ok(matches!(
            self.resolve(t)?,
            Ty::Bool
                | Ty::U8
                | Ty::U16
                | Ty::U32
                | Ty::U64
                | Ty::S8
                | Ty::S16
                | Ty::S32
                | Ty::S64
                | Ty::F32
                | Ty::F64
                | Ty::Char
                | Ty::Enum(_)
                | Ty::Option(_)
        ))
    }

    /// PascalCase token naming a type shape (shared-shape class names are
    /// built from these): Option<Tok>, Tuple<N><Toks>, Result<Tok><Tok>
    /// (absent sides = Void), Named → its Pascal name.
    fn ty_token(&self, ty: &Ty) -> Result<String> {
        Ok(match ty {
            Ty::List(t) => format!("List{}", self.ty_token(t)?),
            Ty::Option(t) => format!("Option{}", self.ty_token(t)?),
            Ty::Tuple(ts) => {
                let mut s = format!("Tuple{}", ts.len());
                for t in ts {
                    s.push_str(&self.ty_token(t)?);
                }
                s
            }
            Ty::Result(ok, errt) => {
                let side = |t: &Option<Box<Ty>>| -> Result<String> {
                    Ok(match t {
                        Some(t) => self.ty_token(t)?,
                        None => "Void".to_string(),
                    })
                };
                format!("Result{}{}", side(ok)?, side(errt)?)
            }
            Ty::Named(n) => as_pascal(n),
            Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_) => {
                bail!("internal error: anonymous {ty:?} cannot appear in value position (WIT names these)")
            }
            _ => scalar(ty)
                .map(|(m, _)| {
                    let mut c = m.chars();
                    c.next().map_or(String::new(), |f| {
                        f.to_ascii_uppercase().to_string() + c.as_str()
                    })
                })
                .ok_or_else(|| anyhow!("unmapped type {ty:?}"))?,
        })
    }

    /// AS type expression for a value-position type.
    fn as_ty(&self, ty: &'a Ty) -> Result<String> {
        Ok(match ty {
            Ty::List(t) => format!("Array<{}>", self.as_ty(t)?),
            Ty::Option(t) => {
                if self.needs_box(t)? {
                    format!("{} | null", self.ty_token(ty)?)
                } else {
                    format!("{} | null", self.as_ty(t)?)
                }
            }
            Ty::Tuple(_) | Ty::Result(..) => self.ty_token(ty)?,
            Ty::Named(n) => as_pascal(n),
            Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_) => {
                bail!("internal error: anonymous {ty:?} cannot appear in value position (WIT names these)")
            }
            _ => scalar(ty)
                .map(|(_, t)| t.to_string())
                .ok_or_else(|| anyhow!("unmapped type {ty:?}"))?,
        })
    }

    /// Default (zero) value expression for a type — every generated field
    /// and local is definitely initialized with one of these.
    fn default_expr(&self, ty: &'a Ty) -> Result<String> {
        Ok(match ty {
            Ty::Bool => "false".to_string(),
            Ty::U8
            | Ty::U16
            | Ty::U32
            | Ty::U64
            | Ty::S8
            | Ty::S16
            | Ty::S32
            | Ty::S64
            | Ty::F32
            | Ty::F64
            | Ty::Char => "0".to_string(),
            Ty::String => "\"\"".to_string(),
            Ty::List(t) => format!("new Array<{}>()", self.as_ty(t)?),
            Ty::Option(_) => "null".to_string(),
            Ty::Tuple(_) | Ty::Result(..) => format!("new {}()", self.ty_token(ty)?),
            Ty::Named(_) => {
                let (name, resolved) = self.resolve_named(ty)?;
                let name = name.expect("Named resolves through names");
                match resolved {
                    Ty::Enum(cases) => {
                        format!("{}.{}", as_pascal(name), as_ident(&cases[0]))
                    }
                    Ty::Record(_) | Ty::Variant(_) | Ty::Flags(_) => {
                        format!("new {}()", as_pascal(name))
                    }
                    // alias to a structural type: its structural default
                    other => self.default_expr(other)?,
                }
            }
            Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_) => {
                bail!("internal error: anonymous {ty:?} in default position")
            }
        })
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

    /// The Res wrapper's value type for a function (None = ResVoid).
    fn res_value(&self, f: &'a Func) -> Result<Option<&'a Ty>> {
        Ok(match self.classify(f)? {
            Ret::None | Ret::ResStr { ok: None, .. } => None,
            Ret::Plain(t) | Ret::ResTyped(t) => Some(t),
            Ret::ResStr { ok: Some(t), .. } => Some(t),
        })
    }

    /// The Res wrapper class name for a function.
    fn res_name(&self, f: &'a Func) -> Result<String> {
        Ok(match self.res_value(f)? {
            None => "ResVoid".to_string(),
            Some(t) => format!("Res{}", self.ty_token(t)?),
        })
    }

    // -- shared shape collection -------------------------------------------------

    fn collect_shapes(&mut self) -> Result<()> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut shapes: Vec<(String, Shape)> = Vec::new();
        // walk in deterministic order: per iface (exports then imports),
        // types in declaration order, then funcs (params, then result)
        let ifaces: Vec<&Iface> = self.m.exports.iter().chain(&self.m.imports).collect();
        for iface in &ifaces {
            for t in &iface.types {
                self.scan_ty(&t.ty, &mut seen, &mut shapes)?;
            }
        }
        for iface in &ifaces {
            for f in &iface.funcs {
                for (_, t) in &f.params {
                    self.scan_ty(t, &mut seen, &mut shapes)?;
                }
                match self.classify(f)? {
                    Ret::None => {}
                    Ret::Plain(t) | Ret::ResTyped(t) => self.scan_ty(t, &mut seen, &mut shapes)?,
                    Ret::ResStr { ok, .. } => {
                        if let Some(t) = ok {
                            self.scan_ty(t, &mut seen, &mut shapes)?;
                        }
                    }
                }
                let name = self.res_name(f)?;
                if seen.insert(name.clone()) {
                    shapes.push((name, Shape::Res(self.res_value(f)?.cloned())));
                }
            }
        }
        self.shapes = shapes;
        Ok(())
    }

    fn scan_ty(
        &self,
        ty: &'a Ty,
        seen: &mut HashSet<String>,
        shapes: &mut Vec<(String, Shape)>,
    ) -> Result<()> {
        match ty {
            Ty::List(t) => self.scan_ty(t, seen, shapes)?,
            Ty::Option(t) => {
                if self.needs_box(t)? {
                    let name = self.ty_token(ty)?;
                    if seen.insert(name.clone()) {
                        shapes.push((name, Shape::OptionBox((**t).clone())));
                    }
                }
                self.scan_ty(t, seen, shapes)?;
            }
            Ty::Tuple(ts) => {
                let name = self.ty_token(ty)?;
                if seen.insert(name.clone()) {
                    shapes.push((name, Shape::Tuple(ts.clone())));
                }
                for t in ts {
                    self.scan_ty(t, seen, shapes)?;
                }
            }
            Ty::Result(ok, errt) => {
                let name = self.ty_token(ty)?;
                if seen.insert(name.clone()) {
                    shapes.push((
                        name,
                        Shape::ResultVal(ok.as_deref().cloned(), errt.as_deref().cloned()),
                    ));
                }
                if let Some(t) = ok.as_deref() {
                    self.scan_ty(t, seen, shapes)?;
                }
                if let Some(t) = errt.as_deref() {
                    self.scan_ty(t, seen, shapes)?;
                }
            }
            Ty::Record(fs) => {
                for (_, t) in fs {
                    self.scan_ty(t, seen, shapes)?;
                }
            }
            Ty::Variant(cs) => {
                for (_, t) in cs {
                    if let Some(t) = t {
                        self.scan_ty(t, seen, shapes)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    // -- validation ----------------------------------------------------------------

    /// Claim every identifier this module will emit at bindings.ts module
    /// scope; duplicate or reserved names fail loudly here, before anything
    /// is written.
    fn validate(&self) -> Result<()> {
        let mut taken: HashMap<String, String> = RESERVED
            .iter()
            .map(|s| (s.to_string(), "the generated bindings".to_string()))
            .collect();
        let mut claim = |ident: String, what: String| -> Result<()> {
            if let Some(owner) = taken.get(&ident) {
                bail!("{what} maps to AssemblyScript identifier `{ident}`, which collides with {owner}; rename it in the WIT");
            }
            taken.insert(ident, what);
            Ok(())
        };

        for iface in self.m.exports.iter().chain(&self.m.imports) {
            for t in &iface.types {
                let p = as_pascal(&t.wit_name);
                let what = format!("WIT type `{}` ({})", t.wit_name, iface.instance);
                claim(p.clone(), what.clone())?;
                claim(format!("encode{p}"), what.clone())?;
                claim(format!("decode{p}"), what.clone())?;
                match &t.ty {
                    Ty::Record(fields) => {
                        // class fields are their own (per-type) scope
                        let mut seen: HashMap<String, &str> = HashMap::new();
                        for (fname, _) in fields {
                            let fi = as_ident(fname);
                            if let Some(prev) = seen.insert(fi.clone(), fname) {
                                bail!(
                                    "{what}: fields `{prev}` and `{fname}` both map to field `{fi}`; rename one in the WIT"
                                );
                            }
                        }
                    }
                    Ty::Enum(cases) => {
                        let mut seen: HashMap<String, &str> = HashMap::new();
                        for c in cases {
                            let ci = as_ident(c);
                            if let Some(prev) = seen.insert(ci.clone(), c) {
                                bail!(
                                    "{what}: cases `{prev}` and `{c}` both map to enum member `{ci}`; rename one in the WIT"
                                );
                            }
                        }
                    }
                    Ty::Flags(flags) => {
                        if flags.len() > 64 {
                            bail!(
                                "flags `{}` has {} flags; the AssemblyScript lane (class {p} {{ bits: u64 }}) supports at most 64",
                                t.wit_name,
                                flags.len()
                            );
                        }
                        let mut seen: HashMap<String, &str> = HashMap::new();
                        for f in flags {
                            let fc = as_screaming(f);
                            if let Some(prev) = seen.insert(fc.clone(), f) {
                                bail!(
                                    "{what}: flags `{prev}` and `{f}` both map to const `{fc}`; rename one in the WIT"
                                );
                            }
                        }
                    }
                    Ty::Variant(cases) => {
                        // instance scope: payload fields + the fixed `tag`;
                        // static scope: TAG_* consts + new<Case> factories
                        let mut fields: HashMap<String, &str> =
                            HashMap::from([("tag".to_string(), "the generated `tag` field")]);
                        let mut statics: HashMap<String, &str> = HashMap::new();
                        for (c, _) in cases {
                            let ci = as_ident(c);
                            if let Some(prev) = fields.insert(ci.clone(), c) {
                                bail!(
                                    "{what}: case `{c}` maps to field `{ci}`, which collides with {prev}; rename it in the WIT"
                                );
                            }
                            for s in [
                                format!("TAG_{}", as_screaming(c)),
                                format!("new{}", as_pascal(c)),
                            ] {
                                if let Some(prev) = statics.insert(s.clone(), c) {
                                    bail!(
                                        "{what}: cases `{prev}` and `{c}` both map to static `{s}`; rename one in the WIT"
                                    );
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        for iface in &self.m.exports {
            let mut funcs: HashMap<String, String> = HashMap::new();
            for f in &iface.funcs {
                let ident = as_ident(&f.wit_name);
                let what = format!("WIT function `{}` ({})", f.wit_name, iface.instance);
                claim(format!("handle{}", as_pascal(&f.wit_name)), what.clone())?;
                claim(format!("impl{}", as_pascal(&f.wit_name)), what.clone())?;
                // assembly/impl.ts is its own scope: its exported functions
                // only collide with each other there
                if let Some(prev) = funcs.insert(ident.clone(), f.wit_name.clone()) {
                    bail!(
                        "WIT functions `{prev}` and `{}` both map to AssemblyScript function `{ident}`; rename one",
                        f.wit_name
                    );
                }
                self.validate_params(f, &what)?;
            }
        }
        for iface in &self.m.imports {
            for f in &iface.funcs {
                let what = format!("WIT import `{}` ({})", f.wit_name, iface.instance);
                claim(mesh_wrapper_name(iface, f), what.clone())?;
                self.validate_params(f, &what)?;
            }
        }
        // shared-shape class names are derived, so they are claimed LAST: a
        // WIT type landing on one (e.g. a record named `option-bool` next to
        // a use of option<bool>) is reported against the WIT name.
        for (name, _) in &self.shapes {
            claim(
                name.clone(),
                format!("the generated shared-shape class `{name}`"),
            )?;
        }
        Ok(())
    }

    fn validate_params(&self, f: &Func, what: &str) -> Result<()> {
        let mut seen: HashMap<String, &str> = HashMap::new();
        for (name, _) in &f.params {
            if let Some(prev) = seen.insert(as_param(name), name) {
                bail!(
                    "{what}: params `{prev}` and `{name}` both map to the same AssemblyScript name"
                );
            }
        }
        Ok(())
    }

    // -- encode / decode emission ----------------------------------------------

    /// Statements appending the WIRE encoding of `expr` (type `ty`) to Sink
    /// `s`. Sink errors are sticky; no inline checks (one check at the end
    /// of the enclosing context).
    fn emit_encode(&self, w: &mut W, cx: &mut Cx, ty: &'a Ty, expr: &str) -> Result<()> {
        match ty {
            Ty::List(t) => {
                w.line(format!("s.listLen(<u32>{expr}.length);"));
                let i = cx.fresh("i");
                w.open(format!("for (let {i} = 0; {i} < {expr}.length; {i}++) {{"));
                self.emit_encode(w, cx, t, &format!("{expr}[{i}]"))?;
                w.close("}");
            }
            Ty::Option(t) => {
                w.line(format!("s.optionTag({expr} !== null);"));
                w.open(format!("if ({expr} !== null) {{"));
                let x = cx.fresh("x");
                if self.needs_box(t)? {
                    w.line(format!("const {x} = {expr}!.value;"));
                } else {
                    w.line(format!("const {x} = {expr}!;"));
                }
                self.emit_encode(w, cx, t, &x)?;
                w.close("}");
            }
            Ty::Tuple(ts) => {
                for (i, t) in ts.iter().enumerate() {
                    self.emit_encode(w, cx, t, &format!("{expr}.f{i}"))?;
                }
            }
            Ty::Result(ok, errt) => {
                w.line(format!("s.resultTag({expr}.isErr);"));
                match (ok.as_deref(), errt.as_deref()) {
                    (Some(okt), Some(et)) => {
                        w.open(format!("if ({expr}.isErr) {{"));
                        self.emit_encode(w, cx, et, &format!("{expr}.err"))?;
                        w.close("} else {");
                        w.ind += 1;
                        self.emit_encode(w, cx, okt, &format!("{expr}.ok"))?;
                        w.close("}");
                    }
                    (None, Some(et)) => {
                        w.open(format!("if ({expr}.isErr) {{"));
                        self.emit_encode(w, cx, et, &format!("{expr}.err"))?;
                        w.close("}");
                    }
                    (Some(okt), None) => {
                        w.open(format!("if (!{expr}.isErr) {{"));
                        self.emit_encode(w, cx, okt, &format!("{expr}.ok"))?;
                        w.close("}");
                    }
                    (None, None) => {}
                }
            }
            Ty::Named(n) => {
                w.line(format!("encode{}(s, {expr});", as_pascal(n)));
            }
            Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_) => {
                bail!("internal error: anonymous {ty:?} in encode position")
            }
            _ => {
                let (m, _) = scalar(ty).ok_or_else(|| anyhow!("unmapped type {ty:?}"))?;
                w.line(format!("s.{m}({expr});"));
            }
        }
        Ok(())
    }

    /// Statements decoding a value of `ty` from Decoder `d` into lvalue
    /// `dest` (already declared and default-initialized by the caller).
    /// `d.err` is checked after every decode call; on failure `cx.fail_stmt`
    /// runs.
    fn emit_decode(&self, w: &mut W, cx: &mut Cx, ty: &'a Ty, dest: &str) -> Result<()> {
        let fail = cx.fail_stmt.clone();
        match ty {
            Ty::List(t) => {
                let r = cx.fresh("r");
                w.line(format!("const {r} = d.listLen();"));
                w.line(format!("if (d.err !== null) {fail}"));
                // The count is attacker-controlled: append element-by-element
                // (no blind pre-allocation), and the in-loop err check stops
                // a hostile count at the first failing element.
                let i = cx.fresh("i");
                w.open(format!("for (let {i}: u32 = 0; {i} < {r}; {i}++) {{"));
                let x = cx.fresh("x");
                w.line(format!(
                    "let {x}: {} = {};",
                    self.as_ty(t)?,
                    self.default_expr(t)?
                ));
                self.emit_decode(w, cx, t, &x)?;
                w.line(format!("{dest}.push({x});"));
                w.close("}");
            }
            Ty::Option(t) => {
                let r = cx.fresh("r");
                w.line(format!("const {r} = d.optionTag();"));
                w.line(format!("if (d.err !== null) {fail}"));
                w.open(format!("if ({r}) {{"));
                let x = cx.fresh("x");
                w.line(format!(
                    "let {x}: {} = {};",
                    self.as_ty(t)?,
                    self.default_expr(t)?
                ));
                self.emit_decode(w, cx, t, &x)?;
                if self.needs_box(t)? {
                    w.line(format!("{dest} = new {}({x});", self.ty_token(ty)?));
                } else {
                    w.line(format!("{dest} = {x};"));
                }
                w.close("}");
            }
            Ty::Tuple(ts) => {
                for (i, t) in ts.iter().enumerate() {
                    self.emit_decode(w, cx, t, &format!("{dest}.f{i}"))?;
                }
            }
            Ty::Result(ok, errt) => {
                let r = cx.fresh("r");
                w.line(format!("const {r} = d.resultTag();"));
                w.line(format!("if (d.err !== null) {fail}"));
                w.line(format!("{dest}.isErr = {r};"));
                match (ok.as_deref(), errt.as_deref()) {
                    (Some(okt), Some(et)) => {
                        w.open(format!("if ({r}) {{"));
                        self.emit_decode(w, cx, et, &format!("{dest}.err"))?;
                        w.close("} else {");
                        w.ind += 1;
                        self.emit_decode(w, cx, okt, &format!("{dest}.ok"))?;
                        w.close("}");
                    }
                    (None, Some(et)) => {
                        w.open(format!("if ({r}) {{"));
                        self.emit_decode(w, cx, et, &format!("{dest}.err"))?;
                        w.close("}");
                    }
                    (Some(okt), None) => {
                        w.open(format!("if (!{r}) {{"));
                        self.emit_decode(w, cx, okt, &format!("{dest}.ok"))?;
                        w.close("}");
                    }
                    (None, None) => {}
                }
            }
            Ty::Named(n) => {
                w.line(format!("{dest} = decode{}(d);", as_pascal(n)));
                w.line(format!("if (d.err !== null) {fail}"));
            }
            Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_) => {
                bail!("internal error: anonymous {ty:?} in decode position")
            }
            _ => {
                let (m, _) = scalar(ty).ok_or_else(|| anyhow!("unmapped type {ty:?}"))?;
                w.line(format!("{dest} = d.{m}();"));
                w.line(format!("if (d.err !== null) {fail}"));
            }
        }
        Ok(())
    }

    // -- bindings.ts -------------------------------------------------------------

    fn bindings_ts(&self) -> Result<String> {
        let mut w = W::spaces2();
        // type declarations: WIT declaration order (exports then imports) —
        // AS classes may be referenced before declaration, no sorting needed
        for iface in self.m.exports.iter().chain(&self.m.imports) {
            for t in &iface.types {
                self.emit_type_decl(&mut w, iface, t)?;
            }
        }
        for (name, shape) in &self.shapes {
            self.emit_shape_decl(&mut w, name, shape)?;
        }
        for iface in self.m.exports.iter().chain(&self.m.imports) {
            for t in &iface.types {
                self.emit_type_codecs(&mut w, t)?;
            }
        }
        let export = &self.m.exports[0];
        for f in &export.funcs {
            self.emit_handler(&mut w, export, f)?;
        }
        w.line("// register installs the schema and every exported handler; the generated");
        w.line("// assembly/index.ts calls it at top level, which compiles into the module");
        w.line("// start function (exported as the reactor's _initialize by build.sh).");
        w.open("export function register(): void {");
        w.line("setSchema(SCHEMA);");
        for f in &export.funcs {
            w.line(format!(
                "registerHandler(\"{}#{}\", handle{});",
                export.instance,
                f.wit_name,
                as_pascal(&f.wit_name)
            ));
        }
        w.close("}");
        if !self.m.imports.is_empty() {
            w.line("");
            w.line("// Typed mesh wrappers for the world's imported interfaces: each");
            w.line("// WIRE-encodes its params, calls the named workload through the host");
            w.line("// mesh (crabcraft.call import), and decodes the reply. The caller names");
            w.line("// the target deployment — crabgen never bakes one in; placement is the");
            w.line("// host's problem.");
            for iface in &self.m.imports {
                for f in &iface.funcs {
                    self.emit_mesh_wrapper(&mut w, iface, f)?;
                }
            }
        }

        let mut out = String::new();
        out.push_str(GENERATED_HEADER);
        out.push_str("\n//\n");
        out.push_str(&format!(
            "// Typed bindings for WIT package {}, world {}: native classes\n",
            self.m.package, self.m.world
        ));
        out.push_str(
            "// for every WIT type, WIRE codecs, the crab_invoke dispatch handlers +\n\
             // register(), and typed mesh wrappers for imported interfaces. The\n\
             // application half lives in assembly/impl.ts (imported below); crabgen\n\
             // scaffolds it once and `crabgen regen` prints any missing signatures.\n",
        );
        out.push_str("//\n");
        out.push_str(TYPE_TABLE);
        out.push_str(
            "// Error semantics (WIRE.md section 2): a handler maps a non-null impl\n\
             // .err to a status-1 reply \"<function>: <message>\" — except for functions\n\
             // whose WIT result is result<T, string> (or a result with no err payload),\n\
             // where the impl's .err becomes the WIRE result ERR CASE: a normal\n\
             // status-0 reply carrying an encoded result value.\n\n",
        );
        out.push_str("import {\n  Decoder,\n  HandlerResult,\n  registerHandler,\n  setSchema,\n  Sink,\n} from \"./runtime\";\n");
        if !self.m.imports.is_empty() {
            out.push_str("import { meshCall } from \"./mesh\";\n");
        }
        out.push_str("import { SCHEMA } from \"./schema\";\n");
        if !export.funcs.is_empty() {
            out.push_str("import {\n");
            for f in &export.funcs {
                out.push_str(&format!(
                    "  {} as impl{},\n",
                    as_ident(&f.wit_name),
                    as_pascal(&f.wit_name)
                ));
            }
            out.push_str("} from \"../impl\";\n");
        }
        out.push('\n');
        out.push_str(&w.buf);
        Ok(trim_final(&out))
    }

    fn emit_type_decl(&self, w: &mut W, iface: &Iface, t: &'a NamedTy) -> Result<()> {
        let p = as_pascal(&t.wit_name);
        match &t.ty {
            Ty::Record(fields) => {
                w.line(format!(
                    "// {p} mirrors the WIT record `{}` ({}); construct with `new {p}()`",
                    t.wit_name, iface.instance
                ));
                w.line("// and assign fields (every field is default-initialized).");
                w.open(format!("export class {p} {{"));
                for (n, ft) in fields {
                    w.line(format!(
                        "{}: {} = {};",
                        as_ident(n),
                        self.as_ty(ft)?,
                        self.default_expr(ft)?
                    ));
                }
                w.close("}");
                w.line("");
            }
            Ty::Variant(cases) => {
                let case_list = cases
                    .iter()
                    .map(|(c, _)| c.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                w.line(format!(
                    "// {p} mirrors the WIT variant `{}` ({}):",
                    t.wit_name, iface.instance
                ));
                w.line(format!(
                    "// `tag` selects the case ({case_list}; TAG_* consts) and exactly the"
                ));
                w.line("// matching payload field is meaningful. Build values with the static");
                w.line("// new<Case>() factories; read by switching on `tag`.");
                w.open(format!("export class {p} {{"));
                w.line("tag: i32 = 0;");
                for (c, payload) in cases {
                    if let Some(pt) = payload {
                        w.line(format!(
                            "{}: {} = {};",
                            as_ident(c),
                            self.payload_field_ty(pt)?,
                            self.payload_field_default(pt)?
                        ));
                    }
                }
                for (i, (c, _)) in cases.iter().enumerate() {
                    w.line(format!(
                        "static readonly TAG_{}: i32 = {i};",
                        as_screaming(c)
                    ));
                }
                for (i, (c, payload)) in cases.iter().enumerate() {
                    match payload {
                        None => {
                            w.open(format!("static new{}(): {p} {{", as_pascal(c)));
                            w.line(format!("const v = new {p}();"));
                            w.line(format!("v.tag = {i};"));
                            w.line("return v;");
                            w.close("}");
                        }
                        Some(pt) => {
                            w.open(format!(
                                "static new{}(value: {}): {p} {{",
                                as_pascal(c),
                                self.as_ty(pt)?
                            ));
                            w.line(format!("const v = new {p}();"));
                            w.line(format!("v.tag = {i};"));
                            w.line(format!("v.{} = value;", as_ident(c)));
                            w.line("return v;");
                            w.close("}");
                        }
                    }
                }
                w.close("}");
                w.line("");
            }
            Ty::Enum(cases) => {
                w.line(format!(
                    "// {p} mirrors the WIT enum `{}` ({}); cases in declaration order.",
                    t.wit_name, iface.instance
                ));
                w.open(format!("export enum {p} {{"));
                for c in cases {
                    w.line(format!("{},", as_ident(c)));
                }
                w.close("}");
                w.line("");
            }
            Ty::Flags(flags) => {
                w.line(format!(
                    "// {p} mirrors the WIT flags `{}` ({}); bit i = flag i.",
                    t.wit_name, iface.instance
                ));
                w.line(format!(
                    "// Combine with |: v.bits = {p}.{} | ...",
                    as_screaming(&flags[0])
                ));
                w.open(format!("export class {p} {{"));
                w.line("bits: u64 = 0;");
                for (i, f) in flags.iter().enumerate() {
                    w.line(format!(
                        "static readonly {}: u64 = {};",
                        as_screaming(f),
                        1u64 << i
                    ));
                }
                w.close("}");
                w.line("");
            }
            other => {
                w.line(format!(
                    "// {p} aliases the WIT type `{}` ({}).",
                    t.wit_name, iface.instance
                ));
                w.line(format!("export type {p} = {};", self.as_ty(other)?));
                w.line("");
            }
        }
        Ok(())
    }

    /// Variant payload FIELD type: value payloads are plain fields, option
    /// payloads are their (nullable) type, any other reference payload is
    /// stored nullable so the unselected cases need no dummy instances.
    fn payload_field_ty(&self, pt: &'a Ty) -> Result<String> {
        let resolved = self.resolve(pt)?;
        if is_value_kind(resolved) || matches!(resolved, Ty::Option(_)) {
            self.as_ty(pt)
        } else {
            Ok(format!("{} | null", self.as_ty(pt)?))
        }
    }

    fn payload_field_default(&self, pt: &'a Ty) -> Result<String> {
        let resolved = self.resolve(pt)?;
        if is_value_kind(resolved) || matches!(resolved, Ty::Option(_)) {
            self.default_expr(pt)
        } else {
            Ok("null".to_string())
        }
    }

    fn emit_shape_decl(&self, w: &mut W, name: &str, shape: &Shape) -> Result<()> {
        match shape {
            Shape::OptionBox(t) => {
                w.line(format!(
                    "// {name} boxes the payload of option<{}>: AS cannot express",
                    self.as_ty(t)?
                ));
                w.line(format!(
                    "// that type | null, so `{name} | null` stands in — null = none,"
                ));
                w.line(format!("// `new {name}(v)` = some(v)."));
                w.open(format!("export class {name} {{"));
                w.line(format!("value: {};", self.as_ty(t)?));
                w.open(format!("constructor(value: {}) {{", self.as_ty(t)?));
                w.line("this.value = value;");
                w.close("}");
                w.close("}");
                w.line("");
            }
            Shape::Tuple(ts) => {
                let members = ts
                    .iter()
                    .map(|t| self.as_ty(t))
                    .collect::<Result<Vec<_>>>()?
                    .join(", ");
                w.line(format!(
                    "// {name} mirrors tuple<{members}>: fields f0..f{} in order.",
                    ts.len().max(1) - 1
                ));
                w.open(format!("export class {name} {{"));
                for (i, t) in ts.iter().enumerate() {
                    w.line(format!(
                        "f{i}: {} = {};",
                        self.as_ty(t)?,
                        self.default_expr(t)?
                    ));
                }
                w.close("}");
                w.line("");
            }
            Shape::ResultVal(ok, errt) => {
                w.line(format!(
                    "// {name} mirrors a WIT result in value position: isErr selects the"
                ));
                w.line("// populated side; absent sides have no field.");
                w.open(format!("export class {name} {{"));
                w.line("isErr: bool = false;");
                if let Some(t) = ok {
                    w.line(format!(
                        "ok: {} = {};",
                        self.as_ty(t)?,
                        self.default_expr(t)?
                    ));
                }
                if let Some(t) = errt {
                    w.line(format!(
                        "err: {} = {};",
                        self.as_ty(t)?,
                        self.default_expr(t)?
                    ));
                }
                w.close("}");
                w.line("");
            }
            Shape::Res(value) => {
                w.line(format!(
                    "// {name} is what an impl function (or mesh wrapper) returns: exactly"
                ));
                w.line("// one of value/err is meaningful — build with ok()/fail().");
                w.open(format!("export class {name} {{"));
                if let Some(t) = value {
                    w.line(format!(
                        "value: {} = {};",
                        self.as_ty(t)?,
                        self.default_expr(t)?
                    ));
                }
                w.line("err: string | null = null;");
                match value {
                    Some(t) => {
                        w.open(format!("static ok(value: {}): {name} {{", self.as_ty(t)?));
                        w.line(format!("const r = new {name}();"));
                        w.line("r.value = value;");
                        w.line("return r;");
                        w.close("}");
                    }
                    None => {
                        w.open(format!("static ok(): {name} {{"));
                        w.line(format!("return new {name}();"));
                        w.close("}");
                    }
                }
                w.open(format!("static fail(err: string): {name} {{"));
                w.line(format!("const r = new {name}();"));
                w.line("r.err = err;");
                w.line("return r;");
                w.close("}");
                w.close("}");
                w.line("");
            }
        }
        Ok(())
    }

    // -- codecs ------------------------------------------------------------------

    fn emit_type_codecs(&self, w: &mut W, t: &'a NamedTy) -> Result<()> {
        let p = as_pascal(&t.wit_name);
        match &t.ty {
            Ty::Record(fields) => {
                w.line(format!(
                    "// encode{p} appends the WIRE encoding of v (errors via s.err)."
                ));
                w.open(format!("function encode{p}(s: Sink, v: {p}): void {{"));
                let mut cx = Cx::new("return;");
                for (n, ft) in fields {
                    self.emit_encode(w, &mut cx, ft, &format!("v.{}", as_ident(n)))?;
                }
                w.close("}");
                w.line("");
                w.line(format!(
                    "// decode{p} decodes a {p} off d (on failure d.err is set and the"
                ));
                w.line("// partial value must be discarded).");
                w.open(format!("function decode{p}(d: Decoder): {p} {{"));
                w.line(format!("const v = new {p}();"));
                let mut cx = Cx::new("return v;");
                for (n, ft) in fields {
                    self.emit_decode(w, &mut cx, ft, &format!("v.{}", as_ident(n)))?;
                }
                w.line("return v;");
                w.close("}");
                w.line("");
            }
            Ty::Enum(cases) => {
                let n = cases.len();
                w.line(format!(
                    "// encode{p} appends the WIRE encoding of v (errors via s.err)."
                ));
                w.open(format!("function encode{p}(s: Sink, v: {p}): void {{"));
                w.open(format!("if (<u32>v >= {n}) {{"));
                w.line(format!("s.err = \"invalid {p}: \" + (<u32>v).toString();"));
                w.line("return;");
                w.close("}");
                w.line("s.caseIdx(<u32>v);");
                w.close("}");
                w.line("");
                w.line(format!("// decode{p} decodes a {p} off d."));
                w.open(format!("function decode{p}(d: Decoder): {p} {{"));
                w.line(format!("const r0 = d.enumCase({n});"));
                w.line(format!("return <{p}>(<i32>r0);"));
                w.close("}");
                w.line("");
            }
            Ty::Flags(flags) => {
                let n = flags.len();
                w.line(format!(
                    "// encode{p} appends the WIRE encoding of v (errors via s.err)."
                ));
                w.open(format!("function encode{p}(s: Sink, v: {p}): void {{"));
                if n < 64 {
                    w.open(format!("if ((v.bits >> {n}) != 0) {{"));
                    w.line(format!("s.err = \"invalid {p}: unknown bits\";"));
                    w.line("return;");
                    w.close("}");
                }
                w.line(format!("const x0 = new Array<bool>({n});"));
                w.open(format!("for (let i1 = 0; i1 < {n}; i1++) {{"));
                w.line("x0[i1] = ((v.bits >> <u64>i1) & 1) != 0;");
                w.close("}");
                w.line("s.flags(x0);");
                w.close("}");
                w.line("");
                w.line(format!("// decode{p} decodes a {p} off d."));
                w.open(format!("function decode{p}(d: Decoder): {p} {{"));
                w.line(format!("const v = new {p}();"));
                w.line(format!("const r0 = d.flags({n});"));
                w.line("if (d.err !== null) return v;");
                w.open(format!("for (let i1 = 0; i1 < {n}; i1++) {{"));
                w.line("if (r0[i1]) v.bits |= <u64>1 << <u64>i1;");
                w.close("}");
                w.line("return v;");
                w.close("}");
                w.line("");
            }
            Ty::Variant(cases) => {
                let n = cases.len();
                w.line(format!(
                    "// encode{p} appends the WIRE encoding of v (errors via s.err)."
                ));
                w.open(format!("function encode{p}(s: Sink, v: {p}): void {{"));
                w.open(format!("if (v.tag < 0 || v.tag >= {n}) {{"));
                w.line(format!("s.err = \"invalid {p} tag: \" + v.tag.toString();"));
                w.line("return;");
                w.close("}");
                w.line("s.caseIdx(<u32>v.tag);");
                let mut cx = Cx::new("return;");
                if cases.iter().any(|(_, payload)| payload.is_some()) {
                    w.open("switch (v.tag) {");
                    for (i, (c, payload)) in cases.iter().enumerate() {
                        let Some(pt) = payload else { continue };
                        let field = format!("v.{}", as_ident(c));
                        w.open(format!("case {i}: {{"));
                        let resolved = self.resolve(pt)?;
                        if is_value_kind(resolved) || matches!(resolved, Ty::Option(_)) {
                            self.emit_encode(w, &mut cx, pt, &field)?;
                        } else {
                            // ref payload stored nullable: a missing payload
                            // under its own tag is a caller bug, reported as
                            // an encode error rather than a trap
                            w.open(format!("if ({field} === null) {{"));
                            w.line(format!("s.err = \"invalid {p}: missing `{c}` payload\";"));
                            w.line("return;");
                            w.close("}");
                            let x = cx.fresh("x");
                            w.line(format!("const {x} = {field}!;"));
                            self.emit_encode(w, &mut cx, pt, &x)?;
                        }
                        w.line("break;");
                        w.close("}");
                    }
                    w.line("default:");
                    w.ind += 1;
                    w.line("break;");
                    w.ind -= 1;
                    w.close("}");
                }
                w.close("}");
                w.line("");

                w.line(format!("// decode{p} decodes a {p} off d."));
                w.open(format!("function decode{p}(d: Decoder): {p} {{"));
                w.line(format!("const v = new {p}();"));
                let mut cx = Cx::new("return v;");
                let r = cx.fresh("r"); // r0, matching the temp scheme
                w.line(format!("const {r} = d.variantCase({n});"));
                w.line("if (d.err !== null) return v;");
                w.line(format!("v.tag = <i32>{r};"));
                if cases.iter().any(|(_, payload)| payload.is_some()) {
                    w.open(format!("switch (<i32>{r}) {{"));
                    for (i, (c, payload)) in cases.iter().enumerate() {
                        let Some(pt) = payload else { continue };
                        let field = format!("v.{}", as_ident(c));
                        w.open(format!("case {i}: {{"));
                        let resolved = self.resolve(pt)?;
                        if is_value_kind(resolved) || matches!(resolved, Ty::Option(_)) {
                            self.emit_decode(w, &mut cx, pt, &field)?;
                        } else {
                            let x = cx.fresh("x");
                            w.line(format!(
                                "let {x}: {} = {};",
                                self.as_ty(pt)?,
                                self.default_expr(pt)?
                            ));
                            self.emit_decode(w, &mut cx, pt, &x)?;
                            w.line(format!("{field} = {x};"));
                        }
                        w.line("break;");
                        w.close("}");
                    }
                    w.line("default:");
                    w.ind += 1;
                    w.line("break; // unreachable: variantCase bounds-checks");
                    w.ind -= 1;
                    w.close("}");
                }
                w.line("return v;");
                w.close("}");
                w.line("");
            }
            other => {
                // alias: identical AS type, helpers delegate to the structure
                w.line(format!(
                    "// encode{p} appends the WIRE encoding of v (errors via s.err)."
                ));
                w.open(format!("function encode{p}(s: Sink, v: {p}): void {{"));
                let mut cx = Cx::new("return;");
                self.emit_encode(w, &mut cx, other, "v")?;
                w.close("}");
                w.line("");
                w.line(format!("// decode{p} decodes a {p} off d."));
                w.open(format!("function decode{p}(d: Decoder): {p} {{"));
                w.line(format!("let v: {p} = {};", self.default_expr(other)?));
                let mut cx = Cx::new("return v;");
                self.emit_decode(w, &mut cx, other, "v")?;
                w.line("return v;");
                w.close("}");
                w.line("");
            }
        }
        Ok(())
    }

    // -- handlers ------------------------------------------------------------------

    fn emit_handler(&self, w: &mut W, iface: &Iface, f: &'a Func) -> Result<()> {
        let p = as_pascal(&f.wit_name);
        w.line(format!(
            "// handle{p} dispatches {}#{}; registered by register() below.",
            iface.instance, f.wit_name
        ));
        w.open(format!("function handle{p}(d: Decoder): HandlerResult {{"));
        let mut cx = Cx::new("return HandlerResult.fail(\"bad params: \" + d.err!);".to_string());
        for (n, t) in &f.params {
            let pn = as_param(n);
            w.line(format!(
                "let {pn}: {} = {};",
                self.as_ty(t)?,
                self.default_expr(t)?
            ));
            self.emit_decode(w, &mut cx, t, &pn)?;
        }
        w.line("const fin = d.finish(\"params\");");
        w.line("if (fin !== null) return HandlerResult.fail(\"bad params: \" + fin!);");
        let args = f
            .params
            .iter()
            .map(|(n, _)| as_param(n))
            .collect::<Vec<_>>()
            .join(", ");
        w.line(format!("const r = impl{p}({args});"));
        match self.classify(f)? {
            Ret::None => {
                w.line("if (r.err !== null) return HandlerResult.fail(r.err!);");
                w.line("return HandlerResult.pass(new Uint8Array(0));");
            }
            Ret::Plain(t) | Ret::ResTyped(t) => {
                w.line("if (r.err !== null) return HandlerResult.fail(r.err!);");
                w.line("const s = new Sink();");
                self.emit_encode(w, &mut cx, t, "r.value")?;
                w.line("if (s.err !== null) return HandlerResult.fail(s.err!);");
                w.line("return HandlerResult.pass(s.bytes());");
            }
            Ret::ResStr { ok, has_msg } => {
                w.line("const s = new Sink();");
                w.open("if (r.err !== null) {");
                w.line("s.resultTag(true);");
                if has_msg {
                    w.line("s.string(r.err!);");
                } else {
                    w.line("// the WIT err side has no payload: the message is dropped");
                }
                w.line("if (s.err !== null) return HandlerResult.fail(s.err!);");
                w.line("return HandlerResult.pass(s.bytes());");
                w.close("}");
                w.line("s.resultTag(false);");
                if let Some(okt) = ok {
                    self.emit_encode(w, &mut cx, okt, "r.value")?;
                }
                w.line("if (s.err !== null) return HandlerResult.fail(s.err!);");
                w.line("return HandlerResult.pass(s.bytes());");
            }
        }
        w.close("}");
        w.line("");
        Ok(())
    }

    // -- mesh wrappers -----------------------------------------------------------

    fn mesh_wrapper_sig(&self, iface: &Iface, f: &'a Func) -> Result<String> {
        let mut params = vec!["workload: string".to_string()];
        for (n, t) in &f.params {
            params.push(format!("{}: {}", as_param(n), self.as_ty(t)?));
        }
        Ok(format!(
            "export function {}({}): {}",
            mesh_wrapper_name(iface, f),
            params.join(", "),
            self.res_name(f)?
        ))
    }

    fn emit_mesh_wrapper(&self, w: &mut W, iface: &Iface, f: &'a Func) -> Result<()> {
        let name = mesh_wrapper_name(iface, f);
        let addr = format!("{}#{}", iface.instance, f.wit_name);
        let res = self.res_name(f)?;
        let ret = self.classify(f)?;
        w.line(format!("// {name} calls {addr} on the workload named"));
        w.line("// `workload` through the host mesh. The returned .err covers transport");
        let res_note = match ret {
            Ret::ResStr { .. } => {
                "// failures, remote status-1 failures, AND the WIT result err case."
            }
            _ => "// failures and remote status-1 failures.",
        };
        w.line(res_note);
        w.open(format!("{} {{", self.mesh_wrapper_sig(iface, f)?));
        let mut cx = Cx::new(format!("return {res}.fail(d.err!);"));
        w.line("const s = new Sink();");
        for (n, t) in &f.params {
            self.emit_encode(w, &mut cx, t, &as_param(n))?;
        }
        w.line(format!("if (s.err !== null) return {res}.fail(s.err!);"));
        w.line(format!(
            "const r = meshCall(workload, \"{addr}\", s.bytes());"
        ));
        w.line(format!("if (r.err !== null) return {res}.fail(r.err!);"));
        w.line("const d = new Decoder(r.bytes!);");
        let finish = |w: &mut W| {
            w.line("const fin = d.finish(\"reply\");");
            w.line(format!("if (fin !== null) return {res}.fail(fin!);"));
        };
        match ret {
            Ret::None => {
                finish(w);
                w.line(format!("return {res}.ok();"));
            }
            Ret::Plain(t) | Ret::ResTyped(t) => {
                let x = cx.fresh("x");
                w.line(format!(
                    "let {x}: {} = {};",
                    self.as_ty(t)?,
                    self.default_expr(t)?
                ));
                self.emit_decode(w, &mut cx, t, &x)?;
                finish(w);
                w.line(format!("return {res}.ok({x});"));
            }
            Ret::ResStr { ok, has_msg } => {
                let r = cx.fresh("r");
                w.line(format!("const {r} = d.resultTag();"));
                w.line(format!("if (d.err !== null) return {res}.fail(d.err!);"));
                w.open(format!("if ({r}) {{"));
                if has_msg {
                    let rm = cx.fresh("r");
                    w.line(format!("const {rm} = d.string();"));
                    w.line(format!("if (d.err !== null) return {res}.fail(d.err!);"));
                    finish(w);
                    w.line(format!("return {res}.fail({rm});"));
                } else {
                    finish(w);
                    w.line(format!(
                        "return {res}.fail(\"{}: err result (no payload)\");",
                        f.wit_name
                    ));
                }
                w.close("}");
                match ok {
                    Some(okt) => {
                        let x = cx.fresh("x");
                        w.line(format!(
                            "let {x}: {} = {};",
                            self.as_ty(okt)?,
                            self.default_expr(okt)?
                        ));
                        self.emit_decode(w, &mut cx, okt, &x)?;
                        finish(w);
                        w.line(format!("return {res}.ok({x});"));
                    }
                    None => {
                        finish(w);
                        w.line(format!("return {res}.ok();"));
                    }
                }
            }
        }
        w.close("}");
        w.line("");
        Ok(())
    }

    // -- scaffold files ----------------------------------------------------------

    /// "export function greet(req: GreetRequest): ResString" — shared by the
    /// scaffolded stubs and missing_impls.
    fn method_sig(&self, f: &'a Func) -> Result<String> {
        let params = f
            .params
            .iter()
            .map(|(n, t)| Ok(format!("{}: {}", as_param(n), self.as_ty(t)?)))
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        Ok(format!(
            "export function {}({params}): {}",
            as_ident(&f.wit_name),
            self.res_name(f)?
        ))
    }

    /// One doc line describing where a function's returned .err goes.
    fn err_doc(&self, f: &'a Func) -> Result<&'static str> {
        Ok(match self.classify(f)? {
            Ret::ResStr { has_msg: true, .. } => {
                "// A non-null .err encodes as the WIT result err case (a normal status-0 reply)."
            }
            Ret::ResStr { has_msg: false, .. } => {
                "// A non-null .err encodes as the WIT result err case (no payload: the message is dropped)."
            }
            _ => "// A non-null .err is a function-level failure (status-1 reply).",
        })
    }

    /// Bindings names a signature references (for impl.ts's import list).
    fn sig_idents(&self, ty: &'a Ty, out: &mut HashSet<String>) -> Result<()> {
        match ty {
            Ty::List(t) => self.sig_idents(t, out)?,
            Ty::Option(t) => {
                if self.needs_box(t)? {
                    out.insert(self.ty_token(ty)?);
                } else {
                    self.sig_idents(t, out)?;
                }
            }
            Ty::Tuple(_) | Ty::Result(..) => {
                out.insert(self.ty_token(ty)?);
            }
            Ty::Named(n) => {
                out.insert(as_pascal(n));
            }
            _ => {}
        }
        Ok(())
    }

    fn impl_ts(&self) -> Result<String> {
        let export = &self.m.exports[0];
        let mut idents: HashSet<String> = HashSet::new();
        for f in &export.funcs {
            for (_, t) in &f.params {
                self.sig_idents(t, &mut idents)?;
            }
            idents.insert(self.res_name(f)?);
        }
        let mut imports: Vec<String> = idents.into_iter().collect();
        imports.sort();

        let mut w = W::spaces2();
        w.line("// impl.ts — the application half of this guest: define the exported");
        w.line("// functions below (assembly/gen/bindings.ts imports them by name).");
        w.line("// crabgen scaffolds this file ONCE and never overwrites it; `crabgen");
        w.line("// regen` prints any missing function signatures instead of editing it.");
        if !imports.is_empty() {
            w.open("import {");
            for i in &imports {
                w.line(format!("{i},"));
            }
            w.close("} from \"./gen/bindings\";");
        }
        w.line("");
        for f in &export.funcs {
            w.line(format!(
                "// {} handles {}#{}.",
                as_ident(&f.wit_name),
                export.instance,
                f.wit_name
            ));
            w.line(self.err_doc(f)?);
            w.open(format!("{} {{", self.method_sig(f)?));
            w.line(format!(
                "return {}.fail(\"unimplemented: {}\");",
                self.res_name(f)?,
                f.wit_name
            ));
            w.close("}");
            w.line("");
        }
        Ok(trim_final(&w.buf))
    }
}

/// A type whose AS mapping is a value type (not a reference): numbers, bool,
/// char and enums. Callers pass the RESOLVED type.
fn is_value_kind(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Bool
            | Ty::U8
            | Ty::U16
            | Ty::U32
            | Ty::U64
            | Ty::S8
            | Ty::S16
            | Ty::S32
            | Ty::S64
            | Ty::F32
            | Ty::F64
            | Ty::Char
            | Ty::Enum(_)
    )
}

/// `<iface><Fn>` camelCase mesh wrapper name ("telemetry" + "report" →
/// "telemetryReport"); keyword-mangled like any camel ident.
fn mesh_wrapper_name(iface: &Iface, f: &Func) -> String {
    as_ident(&format!("{}-{}", iface_short(&iface.instance), f.wit_name))
}

/// The shared type-mapping table comment (bindings.ts header).
const TYPE_TABLE: &str = "\
// Type mapping (WIT -> AssemblyScript):
//   bool/u*/s*/f32/f64 -> bool/u8..u64/i8..i64/f32/f64
//   char               -> u32 (unicode scalar, validated on the wire)
//   string             -> string (UTF-16 here; UTF-8 validated on the wire)
//   list<T>            -> Array<T>
//   option<T>          -> `T | null` when T maps to a non-nullable reference
//                         type; a generated box class (`OptionBool | null`,
//                         null = none, new OptionBool(v) = some) when T is a
//                         value type (numbers, bool, char, enum) or an option
//   tuple<A, B, ..>    -> generated class Tuple<N><A><B>.. (fields f0..fN-1)
//   record             -> class with camelCase fields (new X() + assign)
//   variant            -> class with `tag: i32`, one payload field per
//                         payload case, TAG_* consts, new<Case>() factories
//   enum               -> enum (i32-backed), cases in declaration order
//   flags              -> class { bits: u64 } + SCREAMING bit consts (<= 64)
//   result<T, E>       -> value position: generated class Result<T><E>
//                         (isErr selects the side); function-result position
//                         maps onto the impl's Res channel (see below)
//
// Every impl function returns a generated monomorphic Res class (ResVoid
// when there is no value); exactly one of value/err is meaningful — build
// with Res<T>.ok(v) / Res<T>.fail(msg):
//   - no WIT result:               ResVoid;  .err = status-1 reply
//   - plain value T:               Res<T>;   .err = status-1 reply
//   - result<T, string> (or absent err payload): Res<T> (ResVoid when the
//     ok side is absent); a non-null .err is the WIRE result ERR CASE —
//     a normal status-0 reply, NOT a function-level failure
//   - result<T, E> with any other E: Res<Result<T><E>>; .err = status-1
//
// Name casing: WIT kebab-case -> PascalCase types/shape classes/factories
// (\"a-u8\" -> \"AU8\": capitalize each dash segment), camelCase functions/
// params/fields/enum members, SCREAMING_SNAKE flag and TAG_* consts; names
// hitting an AS/TS keyword, a basic-type name, or a generated local get a
// trailing underscore.
//
";

// ---------------------------------------------------------------------------
// non-Gen file contents
// ---------------------------------------------------------------------------

/// Escape a string into an AS double-quoted literal. Non-ASCII and control
/// characters become \uXXXX escapes (UTF-16 code units — exactly what AS
/// strings hold, surrogate pairs included).
fn ts_str(s: &str) -> String {
    let mut out = String::from("\"");
    for u in s.encode_utf16() {
        match u {
            0x22 => out.push_str("\\\""),
            0x5c => out.push_str("\\\\"),
            0x20..=0x7e => out.push(u as u8 as char),
            _ => out.push_str(&format!("\\u{u:04x}")),
        }
    }
    out.push('"');
    out
}

/// assembly/gen/schema.ts: the resolved-WIT JSON as one escaped string
/// constant (AS has no include_str; the JSON is ASCII-escaped into a
/// literal, the same bytes as gen/schema.json).
fn schema_ts(schema: &str) -> String {
    format!(
        "{GENERATED_HEADER}\n//\n\
         // The resolved-WIT JSON this module serves from crab_schema — the same\n\
         // bytes as gen/schema.json, embedded because the wasm has no filesystem.\n\
         // gen/bindings.ts passes it to setSchema() inside register().\n\
         export const SCHEMA: string = {};\n",
        ts_str(schema)
    )
}

/// assembly/index.ts: the asc entry file (only ITS exports become wasm
/// exports). Regenerated every regen.
fn index_ts() -> String {
    format!(
        "{GENERATED_HEADER}\n//\n\
         // Entry file: asc only turns exports of the ENTRY file into wasm exports,\n\
         // so the WIRE ABI is re-exported here. The register() call below is a\n\
         // top-level statement: it compiles into the module start function, which\n\
         // build.sh exports as the reactor's `_initialize` (--exportStart) — the\n\
         // host calls it once before invoking.\n\
         export {{ crab_alloc, crab_schema, crab_invoke }} from \"./gen/runtime\";\n\
         import {{ register }} from \"./gen/bindings\";\n\
         register();\n"
    )
}

/// The pinned assemblyscript version from the template package.json (single
/// source of truth; build.sh bakes it into its staleness check).
fn pinned_as_version() -> String {
    let pkg: serde_json::Value =
        serde_json::from_str(PACKAGE_JSON).expect("templates/ts/package.json parses");
    pkg["devDependencies"]["assemblyscript"]
        .as_str()
        .expect("templates/ts/package.json pins assemblyscript")
        .to_string()
}

/// The asc flag set below must stay in step with ASC_FLAGS in
/// tests/golden_as.rs — the compile test proves exactly these flags build a
/// working reactor.
fn build_sh(name: &str) -> String {
    let ver = pinned_as_version();
    format!(
        r#"#!/usr/bin/env bash
# Build the {name} reactor module (scaffolded by crabgen; edit freely — this
# file is written once and never overwritten).
#
# The module is an AssemblyScript REACTOR: the top-level register() call in
# assembly/index.ts compiles into the module start function, which
# --exportStart turns into the exported `_initialize` (run once by the host)
# instead of a wasm start section. --use abort= removes the env.abort import
# (abort() traps); --runtime incremental is the full AS GC (the runtime
# template pins host-visible buffers). asc is pinned by package.json +
# package-lock.json; npm ci restores it when node_modules is missing or
# carries a different version.
set -euo pipefail
cd "$(dirname "$0")"

if ! grep -qs '"version": "{ver}"' node_modules/assemblyscript/package.json; then
  nix shell nixpkgs#nodejs --command npm ci --no-audit --no-fund
fi

# SIMD note: asc disables the wasm simd feature by default (0.28 has no
# `--disable simd`, only an opt-in `--enable simd` — never add it); the
# tripwire below proves the artifact stays SIMD-free.
nix shell nixpkgs#nodejs --command npx asc assembly/index.ts \
  -o ../../modules/{name}.wasm \
  --exportStart _initialize --use abort= --runtime incremental \
  --optimizeLevel 3 --shrinkLevel 1

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
    let ver = pinned_as_version();
    format!(
        r#"# {name} — crabcraft guest (AssemblyScript lane)

<!-- generated by crabgen — edits will be overwritten on regen -->

Generated by crabgen. `{name}.wit` is the source of truth; `gen/`,
`assembly/gen/`, `assembly/index.ts` and this README are GENERATED — never
edit them, crabgen rewrites them wholesale on every regen. Your code lives
in `assembly/impl.ts` (crabgen never touches it): define every function the
generated `assembly/gen/bindings.ts` imports from it. `crabgen regen`
prints the missing signatures.

## Build

    ./build.sh

npm ci restores the pinned toolchain (assemblyscript {ver}, via
package.json + package-lock.json) into `node_modules/` when missing, then
asc (via nix's nodejs) builds a wasm REACTOR at `../../modules/{name}.wasm`
(`--exportStart _initialize --use abort= --runtime incremental
--optimizeLevel 3 --shrinkLevel 1`; flags live in build.sh — no
asconfig.json), and the script fails hard if any SIMD (0xfd) opcodes snuck
in — the wasmcraft engine refuses them.

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
   typed signatures for any functions missing from `assembly/impl.ts`.
3. Paste the stubs into `assembly/impl.ts` and implement them.
4. `./build.sh`, redeploy, invoke.

`crabgen check` (run it in CI/pre-commit) fails while `gen/` is stale.

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
        assert_eq!(as_pascal("echo-everything"), "EchoEverything");
        assert_eq!(as_pascal("a-u8"), "AU8");
        assert_eq!(as_pascal("e2e-ts"), "E2eTs");
        assert_eq!(as_pascal("x"), "X");
    }

    #[test]
    fn camel_lowercases_first_segment() {
        assert_eq!(as_camel("echo-everything"), "echoEverything");
        assert_eq!(as_camel("a-u8"), "aU8");
        assert_eq!(as_camel("AB"), "ab");
        assert_eq!(as_camel("x"), "x");
    }

    #[test]
    fn screaming_uppercases_segments() {
        assert_eq!(as_screaming("read-only"), "READ_ONLY");
        assert_eq!(as_screaming("exec"), "EXEC");
    }

    #[test]
    fn idents_are_keyword_mangled() {
        assert_eq!(as_ident("new"), "new_");
        assert_eq!(as_ident("delete"), "delete_");
        assert_eq!(as_ident("constructor"), "constructor_");
        assert_eq!(as_ident("u32"), "u32_"); // AS basic type names too
        assert_eq!(as_ident("string"), "string_");
        assert_eq!(as_ident("name"), "name");
    }

    #[test]
    fn params_avoid_generated_locals() {
        assert_eq!(as_param("d"), "d_");
        assert_eq!(as_param("s"), "s_");
        assert_eq!(as_param("r"), "r_");
        assert_eq!(as_param("fin"), "fin_");
        assert_eq!(as_param("workload"), "workload_");
        assert_eq!(as_param("r0"), "r0_"); // temp-shaped
        assert_eq!(as_param("x1"), "x1_"); // temp-shaped
        assert_eq!(as_param("i2"), "i2_"); // temp-shaped
        assert_eq!(as_param("e"), "e"); // not a generated prefix
        assert_eq!(as_param("xs"), "xs");
        assert_eq!(as_param("msg"), "msg");
        // codec-helper locals never share a scope with params: no mangling
        assert_eq!(as_param("v"), "v");
    }

    #[test]
    fn ts_str_escapes_utf16() {
        assert_eq!(ts_str("ab"), "\"ab\"");
        assert_eq!(ts_str("a\"b\\c"), "\"a\\\"b\\\\c\"");
        assert_eq!(ts_str("\n"), "\"\\u000a\"");
        // astral chars become surrogate pairs (what AS strings hold)
        assert_eq!(ts_str("🦀"), "\"\\ud83e\\udd80\"");
    }

    #[test]
    fn template_pin_is_extractable() {
        // build.sh and the README bake this in; a template change that
        // breaks extraction must fail loudly.
        let v = pinned_as_version();
        assert!(!v.is_empty() && v.chars().next().unwrap().is_ascii_digit());
    }
}
