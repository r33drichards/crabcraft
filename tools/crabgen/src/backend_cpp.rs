//! C++ backend: typed bindings + dispatch + mesh wrappers + scaffold over the
//! templates/cpp runtime (zig c++ wasm32-wasi reactor lane).
//!
//! Type mapping (WIT → C++):
//!
//! | WIT              | C++                                                     |
//! |------------------|---------------------------------------------------------|
//! | bool             | bool                                                    |
//! | u8 u16 u32 u64   | uint8_t uint16_t uint32_t uint64_t                      |
//! | s8 s16 s32 s64   | int8_t int16_t int32_t int64_t                          |
//! | f32 f64          | float double                                            |
//! | char             | uint32_t (unicode scalar, validated on the wire)        |
//! | string           | std::string (UTF-8, validated on the wire)              |
//! | list<T>          | std::vector<T> (decode pre-allocation clamped at 4096)  |
//! | option<T>        | std::optional<T>                                        |
//! | tuple<A, B, ..>  | std::tuple<A, B, ..>                                    |
//! | record           | struct with snake_case fields, every member `{}`-init   |
//! | variant          | std::variant alias, alternative i = WIT case i: a case  |
//! |                  | WITH a payload is a `struct <Variant><Case> {T value;}` |
//! |                  | alternative, a case WITHOUT one is std::monostate.      |
//! |                  | Construct payload cases from their structs              |
//! |                  | (`Shape s = ShapeCircle{2.0f};`); construct a non-first |
//! |                  | empty case with `std::in_place_index<i>` (two monostate |
//! |                  | alternatives make type-based construction ambiguous);   |
//! |                  | read with `v.index()` + `std::get<i>(v)`.               |
//! | enum             | enum class X : uint32_t, cases in declaration order     |
//! | flags            | struct { uint64_t bits; } + static constexpr uint64_t   |
//! |                  | per-flag bit consts (max 64 flags)                      |
//! | result<T, E>     | value position: gen::Result<T, E> (absent sides are    |
//! |                  | std::monostate). Function-RESULT position: see Ret.     |
//!
//! Function boundary (the impl:: functions, mirroring the Go/Rust lanes'
//! Ret classification): every impl function returns `crab::Res<V>` where
//! - no WIT result        → V = std::monostate; non-empty .err = status-1
//! - plain value T        → V = T;              non-empty .err = status-1
//! - result<T, string> or result with absent err payload
//!                        → V = T (or std::monostate); a non-empty .err is
//!                          the WIRE result ERR CASE (status stays 0)
//! - result<T, E> (other E) → V = gen::Result<T, E>; non-empty .err = status-1
//!
//! Name casing: WIT kebab-case → PascalCase for types / variant case structs
//! / enum cases / flags consts ("echo-everything" → "EchoEverything",
//! "a-u8" → "AU8" — capitalize each dash segment, no acronym table),
//! snake_case for functions, params and fields (every segment lowercased:
//! "AB" → "ab"). Names that hit a C++ keyword (or a local the generated
//! code declares in the same scope) get a trailing underscore.
//!
//! Declaration order: C++ requires definition before use by value, so named
//! types are emitted in dependency (topological) order, declaration order
//! breaking ties; WIT forbids recursive types, so a cycle is a hard error.
//!
//! Dispatch: gen/bindings.hpp DECLARES the `namespace impl` functions and
//! gen/bindings.cpp's handlers call them, so a missing definition in
//! impl.cpp is a LINK ERROR naming the symbol — enforcement beyond the
//! missing_impls substring scan.
//!
//! Collisions: every identifier emitted into namespace gen is checked
//! against the fixed generated names and each other; record fields, variant
//! cases, enum cases, flags and params are checked within their own scopes.
//! The crab runtime needs no reservation sweep (unlike the Go lane's
//! RESERVED table): everything the templates declare lives in namespace
//! crab (or is an extern "C" crab_* symbol), and generated code references
//! it crab::-qualified only — gen-namespace names cannot collide with it,
//! so there is no template-ident drift to guard against.

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};

use crate::backend::Backend;
use crate::emit::{iface_short, is_temp_shaped, project_name, trim_final, GENERATED_HEADER, W};
use crate::ir::{Func, Iface, Module, NamedTy, Ty};

const CRAB_HPP: &str = include_str!("../templates/cpp/crab.hpp");
const CRAB_CPP: &str = include_str!("../templates/cpp/crab.cpp");
const MESH_HPP: &str = include_str!("../templates/cpp/mesh.hpp");
const MESH_CPP: &str = include_str!("../templates/cpp/mesh.cpp");

/// Names the generated bindings declare in namespace gen besides the
/// WIT-derived ones; a WIT name mapping onto one of these fails generate().
/// (encode<P>/decode<P>/handle_<f> helpers are claimed per WIT name in
/// validate(); the crab:: runtime names never collide — see the header.)
const RESERVED: &[&str] = &["Result", "Registration", "registration"];

/// C++ keywords (through C++20): snake identifiers landing here get a
/// trailing underscore. PascalCase names can never collide (keywords are all
/// lowercase). Compound mesh-wrapper names are checked too (`co_await`,
/// `static_assert`... contain underscores).
const CPP_KEYWORDS: &[&str] = &[
    "alignas",
    "alignof",
    "and",
    "and_eq",
    "asm",
    "auto",
    "bitand",
    "bitor",
    "bool",
    "break",
    "case",
    "catch",
    "char",
    "char8_t",
    "char16_t",
    "char32_t",
    "class",
    "co_await",
    "co_return",
    "co_yield",
    "compl",
    "concept",
    "const",
    "const_cast",
    "consteval",
    "constexpr",
    "constinit",
    "continue",
    "decltype",
    "default",
    "delete",
    "do",
    "double",
    "dynamic_cast",
    "else",
    "enum",
    "explicit",
    "export",
    "extern",
    "false",
    "float",
    "for",
    "friend",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "mutable",
    "namespace",
    "new",
    "noexcept",
    "not",
    "not_eq",
    "nullptr",
    "operator",
    "or",
    "or_eq",
    "private",
    "protected",
    "public",
    "register",
    "reinterpret_cast",
    "requires",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "static_assert",
    "static_cast",
    "struct",
    "switch",
    "template",
    "this",
    "thread_local",
    "throw",
    "true",
    "try",
    "typedef",
    "typeid",
    "typename",
    "union",
    "unsigned",
    "using",
    "virtual",
    "void",
    "volatile",
    "wchar_t",
    "while",
    "xor",
    "xor_eq",
];

/// Locals the generated code declares in scopes that also hold WIT params
/// (handler bodies, mesh wrappers); params landing here get a trailing
/// underscore. Codec-helper locals (`v`, `bits`, `i`...) live in functions
/// with fixed signatures that never see a param, so they are deliberately
/// not listed.
const PARAM_AVOID: &[&str] = &["d", "out", "r", "fin", "body", "workload"];

/// The temp-name prefixes this emitter generates (r0, e1, i2, x3...).
const CPP_TEMP_PREFIXES: &[&str] = &["r", "e", "i", "x"];

pub struct CppBackend;

impl Backend for CppBackend {
    fn lang(&self) -> &'static str {
        "cpp"
    }

    fn impl_ext(&self) -> &'static str {
        "cpp"
    }

    fn generate(&self, m: &Module, dir: &Path) -> Result<()> {
        let g = Gen::new(m)?;
        let gen_dir = dir.join("gen");
        let write = |name: &str, content: &str| -> Result<()> {
            let path = gen_dir.join(name);
            fs::write(&path, content).with_context(|| format!("writing {}", path.display()))
        };
        write("crab.hpp", CRAB_HPP)?;
        write("crab.cpp", CRAB_CPP)?;
        if !m.imports.is_empty() {
            // The crabcraft.call import lands in the wasm import section as
            // soon as mesh.cpp is compiled in (build.sh globs gen/*.cpp), so
            // the pair is emitted ONLY when the world has imports — that is
            // what keeps import-free modules import-free.
            write("mesh.hpp", MESH_HPP)?;
            write("mesh.cpp", MESH_CPP)?;
        }
        write("bindings.hpp", &g.bindings_hpp()?)?;
        write("bindings.cpp", &g.bindings_cpp()?)?;
        write("schema_json.h", &schema_json_h(&m.schema_json))?;
        // README quotes WIT-derived content (the export instance), so it is
        // regenerated alongside gen/ — the canonical flow swaps the starter
        // WIT and regens, which must not leave stale docs. It lives at the
        // project root, not gen/.
        let readme_path = dir.join("README.md");
        fs::write(&readme_path, readme(&project_name(dir)?, m))
            .with_context(|| format!("writing {}", readme_path.display()))?;
        Ok(())
    }

    fn scaffold(&self, m: &Module, dir: &Path) -> Result<()> {
        let name = project_name(dir)?;
        let g = Gen::new(m)?;
        let impl_path = dir.join("impl.cpp");
        if !impl_path.exists() {
            fs::write(&impl_path, g.impl_cpp()?)
                .with_context(|| format!("writing {}", impl_path.display()))?;
        }
        let build = dir.join("build.sh");
        if !build.exists() {
            fs::write(&build, build_sh(&name))?;
            let mut perms = fs::metadata(&build)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&build, perms)?;
        }
        Ok(())
    }

    fn missing_impls(&self, m: &Module, dir: &Path) -> Result<Vec<String>> {
        let g = Gen::new(m)?;
        let src = fs::read_to_string(dir.join("impl.cpp")).unwrap_or_default();
        let mut missing = Vec::new();
        for iface in &m.exports {
            for f in &iface.funcs {
                let ident = cpp_ident(&f.wit_name);
                // A definition looks like `crab::Res<T> name(args) {` — the
                // name follows the return type's space (or a line break).
                let defined =
                    src.contains(&format!(" {ident}(")) || src.contains(&format!("\n{ident}("));
                if !defined {
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
fn cpp_pascal(kebab: &str) -> String {
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
fn cpp_snake(kebab: &str) -> String {
    kebab
        .split('-')
        .map(|seg| seg.to_ascii_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

/// snake identifier for fields/functions: keyword-mangled with a trailing
/// underscore (`delete` → `delete_`).
fn cpp_ident(kebab: &str) -> String {
    let mut s = cpp_snake(kebab);
    if CPP_KEYWORDS.contains(&s.as_str()) {
        s.push('_');
    }
    s
}

/// snake identifier for params: additionally mangled away from the locals
/// generated bodies declare and the `r<N>`/`e<N>`/`i<N>`/`x<N>` temp shape.
fn cpp_param(kebab: &str) -> String {
    let mut s = cpp_snake(kebab);
    if CPP_KEYWORDS.contains(&s.as_str())
        || PARAM_AVOID.contains(&s.as_str())
        || is_temp_shaped(&s, CPP_TEMP_PREFIXES)
    {
        s.push('_');
    }
    s
}

// ---------------------------------------------------------------------------
// type expressions
// ---------------------------------------------------------------------------

/// Scalar types: (Encode* function, Decoder method, C++ type).
fn scalar(ty: &Ty) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match ty {
        Ty::Bool => ("EncodeBool", "Bool", "bool"),
        Ty::U8 => ("EncodeU8", "U8", "uint8_t"),
        Ty::U16 => ("EncodeU16", "U16", "uint16_t"),
        Ty::U32 => ("EncodeU32", "U32", "uint32_t"),
        Ty::U64 => ("EncodeU64", "U64", "uint64_t"),
        Ty::S8 => ("EncodeS8", "S8", "int8_t"),
        Ty::S16 => ("EncodeS16", "S16", "int16_t"),
        Ty::S32 => ("EncodeS32", "S32", "int32_t"),
        Ty::S64 => ("EncodeS64", "S64", "int64_t"),
        Ty::F32 => ("EncodeF32", "F32", "float"),
        Ty::F64 => ("EncodeF64", "F64", "double"),
        Ty::Char => ("EncodeChar", "Char", "uint32_t"),
        Ty::String => ("EncodeString", "String", "std::string"),
        _ => return None,
    })
}

/// A type that encodes to zero bytes (empty tuple): encode/decode emit no
/// statements for it, so deref temps must not be created.
fn is_unit(ty: &Ty) -> bool {
    matches!(ty, Ty::Tuple(ts) if ts.is_empty())
}

/// C++ type expression for a value-position type. `qual` is "" inside
/// namespace gen, "gen::" in impl.cpp signatures.
fn cpp_ty(ty: &Ty, qual: &str) -> Result<String> {
    Ok(match ty {
        Ty::List(t) => format!("std::vector<{}>", cpp_ty(t, qual)?),
        Ty::Option(t) => format!("std::optional<{}>", cpp_ty(t, qual)?),
        Ty::Tuple(ts) => format!(
            "std::tuple<{}>",
            ts.iter()
                .map(|t| cpp_ty(t, qual))
                .collect::<Result<Vec<_>>>()?
                .join(", ")
        ),
        Ty::Result(ok, errt) => {
            let side = |t: &Option<Box<Ty>>| -> Result<String> {
                Ok(match t {
                    Some(t) => cpp_ty(t, qual)?,
                    None => "std::monostate".to_string(),
                })
            };
            format!("{qual}Result<{}, {}>", side(ok)?, side(errt)?)
        }
        Ty::Named(n) => format!("{qual}{}", cpp_pascal(n)),
        Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_) => {
            bail!("internal error: anonymous {ty:?} cannot appear in value position (WIT names these)")
        }
        _ => scalar(ty)
            .map(|(_, _, c)| c.to_string())
            .ok_or_else(|| anyhow!("unmapped type {ty:?}"))?,
    })
}

/// How a function's WIT result maps onto its `crab::Res<V>` impl signature
/// (mirrors the Go/Rust lanes' Ret classification).
enum Ret<'a> {
    /// no WIT result → V = std::monostate (non-empty .err = status-1 reply)
    None,
    /// plain value → V = T (non-empty .err = status-1 reply)
    Plain(&'a Ty),
    /// result<T, string> / result<T> / result<_,_> → V = T / std::monostate;
    /// a non-empty .err is the WIRE result ERR CASE (status stays 0).
    /// `has_msg` = the err side carries the string payload.
    ResStr { ok: Option<&'a Ty>, has_msg: bool },
    /// result with any other err type → V = gen::Result<T, E>
    /// (non-empty .err = status-1 reply)
    ResTyped(&'a Ty),
}

// ---------------------------------------------------------------------------
// encode / decode statement emission
// ---------------------------------------------------------------------------

/// Per-function emission state: fresh temp names and the error-return
/// statement pattern for the current context (`{}` = the error expression).
struct Cx {
    tmp: usize,
    fail_fmt: String,
}

impl Cx {
    fn new(fail_fmt: impl Into<String>) -> Self {
        Cx {
            tmp: 0,
            fail_fmt: fail_fmt.into(),
        }
    }

    fn fresh(&mut self, base: &str) -> String {
        let n = self.tmp;
        self.tmp += 1;
        format!("{base}{n}")
    }

    fn fail(&self, err_expr: &str) -> String {
        self.fail_fmt.replace("{}", err_expr)
    }
}

/// Statements appending the WIRE encoding of `expr` (type `ty`) to `out`.
fn emit_encode(w: &mut W, cx: &mut Cx, ty: &Ty, expr: &str) -> Result<()> {
    match ty {
        Ty::List(t) => {
            w.line(format!("crab::EncodeListLen(out, {expr}.size());"));
            let x = cx.fresh("x");
            w.open(format!("for (const auto& {x} : {expr}) {{"));
            emit_encode(w, cx, t, &x)?;
            w.close("}");
        }
        Ty::Option(t) => {
            w.line(format!("crab::EncodeOptionTag(out, {expr}.has_value());"));
            if !is_unit(t) {
                w.open(format!("if ({expr}.has_value()) {{"));
                let x = cx.fresh("x");
                w.line(format!("const auto& {x} = *{expr};"));
                emit_encode(w, cx, t, &x)?;
                w.close("}");
            }
        }
        Ty::Tuple(ts) => {
            for (i, t) in ts.iter().enumerate() {
                emit_encode(w, cx, t, &format!("std::get<{i}>({expr})"))?;
            }
        }
        Ty::Result(ok, errt) => {
            w.line(format!("crab::EncodeResultTag(out, {expr}.is_err);"));
            match (ok.as_deref(), errt.as_deref()) {
                (Some(okt), Some(et)) => {
                    w.open(format!("if ({expr}.is_err) {{"));
                    emit_encode(w, cx, et, &format!("{expr}.err"))?;
                    w.close("} else {");
                    w.ind += 1;
                    emit_encode(w, cx, okt, &format!("{expr}.ok"))?;
                    w.close("}");
                }
                (None, Some(et)) => {
                    w.open(format!("if ({expr}.is_err) {{"));
                    emit_encode(w, cx, et, &format!("{expr}.err"))?;
                    w.close("}");
                }
                (Some(okt), None) => {
                    w.open(format!("if (!{expr}.is_err) {{"));
                    emit_encode(w, cx, okt, &format!("{expr}.ok"))?;
                    w.close("}");
                }
                (None, None) => {}
            }
        }
        Ty::Named(n) => {
            let e = cx.fresh("e");
            w.line(format!("auto {e} = encode{}(out, {expr});", cpp_pascal(n)));
            w.line(format!(
                "if (!{e}.empty()) {}",
                cx.fail(&format!("std::move({e})"))
            ));
        }
        Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_) => {
            bail!("internal error: anonymous {ty:?} in encode position")
        }
        _ => {
            let (enc, _, _) = scalar(ty).ok_or_else(|| anyhow!("unmapped type {ty:?}"))?;
            w.line(format!("crab::{enc}(out, {expr});"));
        }
    }
    Ok(())
}

/// Statements decoding a value of `ty` from `d` into lvalue `dest` (already
/// declared and value-initialized by the caller).
fn emit_decode(w: &mut W, cx: &mut Cx, ty: &Ty, dest: &str) -> Result<()> {
    match ty {
        Ty::List(t) => {
            let r = cx.fresh("r");
            w.line(format!("auto {r} = d.ListLen();"));
            w.line(format!(
                "if (!{r}.ok()) {}",
                cx.fail(&format!("std::move({r}.err)"))
            ));
            // the count is attacker-controlled: clamp pre-allocation (the
            // sibling lanes cap initial capacity at 4096) and push_back
            w.line(format!(
                "{dest}.reserve({r}.val < 4096u ? {r}.val : 4096u);"
            ));
            let i = cx.fresh("i");
            w.open(format!("for (uint32_t {i} = 0; {i} < {r}.val; {i}++) {{"));
            let x = cx.fresh("x");
            w.line(format!("{} {x}{{}};", cpp_ty(t, "")?));
            emit_decode(w, cx, t, &x)?;
            w.line(format!("{dest}.push_back(std::move({x}));"));
            w.close("}");
        }
        Ty::Option(t) => {
            let r = cx.fresh("r");
            w.line(format!("auto {r} = d.OptionTag();"));
            w.line(format!(
                "if (!{r}.ok()) {}",
                cx.fail(&format!("std::move({r}.err)"))
            ));
            w.open(format!("if ({r}.val) {{"));
            let x = cx.fresh("x");
            w.line(format!("{} {x}{{}};", cpp_ty(t, "")?));
            emit_decode(w, cx, t, &x)?;
            w.line(format!("{dest} = std::move({x});"));
            w.close("}");
        }
        Ty::Tuple(ts) => {
            for (i, t) in ts.iter().enumerate() {
                emit_decode(w, cx, t, &format!("std::get<{i}>({dest})"))?;
            }
        }
        Ty::Result(ok, errt) => {
            let r = cx.fresh("r");
            w.line(format!("auto {r} = d.ResultTag();"));
            w.line(format!(
                "if (!{r}.ok()) {}",
                cx.fail(&format!("std::move({r}.err)"))
            ));
            w.line(format!("{dest}.is_err = {r}.val;"));
            match (ok.as_deref(), errt.as_deref()) {
                (Some(okt), Some(et)) => {
                    w.open(format!("if ({r}.val) {{"));
                    emit_decode(w, cx, et, &format!("{dest}.err"))?;
                    w.close("} else {");
                    w.ind += 1;
                    emit_decode(w, cx, okt, &format!("{dest}.ok"))?;
                    w.close("}");
                }
                (None, Some(et)) => {
                    w.open(format!("if ({r}.val) {{"));
                    emit_decode(w, cx, et, &format!("{dest}.err"))?;
                    w.close("}");
                }
                (Some(okt), None) => {
                    w.open(format!("if (!{r}.val) {{"));
                    emit_decode(w, cx, okt, &format!("{dest}.ok"))?;
                    w.close("}");
                }
                (None, None) => {}
            }
        }
        Ty::Named(n) => {
            let r = cx.fresh("r");
            w.line(format!("auto {r} = decode{}(d);", cpp_pascal(n)));
            w.line(format!(
                "if (!{r}.ok()) {}",
                cx.fail(&format!("std::move({r}.err)"))
            ));
            w.line(format!("{dest} = std::move({r}.val);"));
        }
        Ty::Record(_) | Ty::Variant(_) | Ty::Enum(_) | Ty::Flags(_) => {
            bail!("internal error: anonymous {ty:?} in decode position")
        }
        _ => {
            let (_, dec, _) = scalar(ty).ok_or_else(|| anyhow!("unmapped type {ty:?}"))?;
            let r = cx.fresh("r");
            w.line(format!("auto {r} = d.{dec}();"));
            w.line(format!(
                "if (!{r}.ok()) {}",
                cx.fail(&format!("std::move({r}.err)"))
            ));
            w.line(format!("{dest} = std::move({r}.val);"));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// the generator
// ---------------------------------------------------------------------------

struct Gen<'a> {
    m: &'a Module,
    /// WIT type name → definition, across every interface (one gen namespace).
    types: HashMap<&'a str, &'a Ty>,
    /// whether the gen::Result<T, E> declaration is needed
    need_result: bool,
}

impl<'a> Gen<'a> {
    fn new(m: &'a Module) -> Result<Self> {
        let mut g = Gen {
            m,
            types: HashMap::new(),
            need_result: false,
        };
        for iface in m.exports.iter().chain(&m.imports) {
            for t in &iface.types {
                if g.types.insert(t.wit_name.as_str(), &t.ty).is_some() {
                    bail!(
                        "type `{}` is declared in more than one interface; the C++ lane puts \
                         every type in one namespace — rename one of them",
                        t.wit_name
                    );
                }
            }
        }
        g.validate()?;
        g.need_result = g.scan_need_result()?;
        Ok(g)
    }

    /// Claim every identifier this module will emit into namespace gen;
    /// duplicate or reserved names fail loudly here, before anything is
    /// written.
    fn validate(&self) -> Result<()> {
        let mut taken: HashMap<String, String> = RESERVED
            .iter()
            .map(|s| (s.to_string(), "the generated bindings".to_string()))
            .collect();
        let mut claim = |ident: String, what: String| -> Result<()> {
            if let Some(owner) = taken.get(&ident) {
                bail!("{what} maps to C++ identifier `{ident}`, which collides with {owner}; rename it in the WIT");
            }
            taken.insert(ident, what);
            Ok(())
        };

        for iface in self.m.exports.iter().chain(&self.m.imports) {
            for t in &iface.types {
                let p = cpp_pascal(&t.wit_name);
                let what = format!("WIT type `{}` ({})", t.wit_name, iface.instance);
                claim(p.clone(), what.clone())?;
                claim(format!("encode{p}"), what.clone())?;
                claim(format!("decode{p}"), what.clone())?;
                match &t.ty {
                    // struct fields are their own (per-type) identifier scope
                    Ty::Record(fields) => {
                        let mut seen: HashMap<String, &str> = HashMap::new();
                        for (fname, _) in fields {
                            let fi = cpp_ident(fname);
                            if let Some(prev) = seen.insert(fi.clone(), fname) {
                                bail!(
                                    "{what}: fields `{prev}` and `{fname}` both map to C++ field `{fi}`; rename one in the WIT"
                                );
                            }
                        }
                    }
                    Ty::Enum(cases) => {
                        // enum class scope: cases collide only with each other
                        let mut seen: HashMap<String, &str> = HashMap::new();
                        for c in cases {
                            let cp = cpp_pascal(c);
                            if let Some(prev) = seen.insert(cp.clone(), c) {
                                bail!(
                                    "{what}: cases `{prev}` and `{c}` both map to C++ enumerator `{cp}`; rename one in the WIT"
                                );
                            }
                        }
                    }
                    Ty::Flags(flags) => {
                        if flags.len() > 64 {
                            bail!(
                                "flags `{}` has {} flags; the C++ lane (struct {p} {{ uint64_t bits; }}) supports at most 64",
                                t.wit_name,
                                flags.len()
                            );
                        }
                        // struct scope: consts collide only with each other
                        // (PascalCase can never hit the lowercase `bits`)
                        let mut seen: HashMap<String, &str> = HashMap::new();
                        for f in flags {
                            let fp = cpp_pascal(f);
                            if let Some(prev) = seen.insert(fp.clone(), f) {
                                bail!(
                                    "{what}: flags `{prev}` and `{f}` both map to C++ const `{fp}`; rename one in the WIT"
                                );
                            }
                        }
                    }
                    Ty::Variant(cases) => {
                        // every case must be distinct (alternative i = case
                        // i, and payload cases name a `<Variant><Case>`
                        // struct in namespace gen)
                        let mut seen: HashMap<String, &str> = HashMap::new();
                        for (c, payload) in cases {
                            let cp = cpp_pascal(c);
                            if let Some(prev) = seen.insert(cp.clone(), c) {
                                bail!(
                                    "{what}: cases `{prev}` and `{c}` both map to C++ case `{cp}`; rename one in the WIT"
                                );
                            }
                            if payload.is_some() {
                                claim(format!("{p}{cp}"), format!("{what} case `{c}`"))?;
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
                let ident = cpp_ident(&f.wit_name);
                let what = format!("WIT function `{}` ({})", f.wit_name, iface.instance);
                claim(format!("handle_{ident}"), what.clone())?;
                // namespace impl is its own scope: functions only collide
                // with each other there
                if let Some(prev) = funcs.insert(ident.clone(), f.wit_name.clone()) {
                    bail!(
                        "WIT functions `{prev}` and `{}` both map to C++ function `{ident}`; rename one",
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
        Ok(())
    }

    fn validate_params(&self, f: &Func, what: &str) -> Result<()> {
        let mut seen: HashMap<String, &str> = HashMap::new();
        for (name, _) in &f.params {
            if let Some(prev) = seen.insert(cpp_param(name), name) {
                bail!("{what}: params `{prev}` and `{name}` both map to the same C++ name");
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

    /// True if any value position needs gen::Result<T, E> (a result NOT
    /// mapped to the crab::Res channel at a function boundary).
    fn scan_need_result(&self) -> Result<bool> {
        fn walk(ty: &Ty) -> bool {
            match ty {
                Ty::Result(..) => true,
                Ty::List(t) | Ty::Option(t) => walk(t),
                Ty::Tuple(ts) => ts.iter().any(walk),
                Ty::Record(fs) => fs.iter().any(|(_, t)| walk(t)),
                Ty::Variant(cs) => cs.iter().any(|(_, t)| t.as_ref().is_some_and(walk)),
                _ => false,
            }
        }
        for iface in self.m.exports.iter().chain(&self.m.imports) {
            for t in &iface.types {
                if walk(&t.ty) {
                    return Ok(true);
                }
            }
            for f in &iface.funcs {
                if f.params.iter().any(|(_, t)| walk(t)) {
                    return Ok(true);
                }
                let needed = match self.classify(f)? {
                    Ret::None => false,
                    Ret::Plain(t) => walk(t),
                    Ret::ResStr { ok, .. } => ok.is_some_and(walk),
                    Ret::ResTyped(_) => true,
                };
                if needed {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    /// Named types in emission order: C++ needs definition before use, so
    /// dependencies come first (depth-first), declaration order (exports
    /// before imports) breaking ties. WIT forbids recursive types; a cycle
    /// is a hard error.
    fn emit_order(&self) -> Result<Vec<(&'a Iface, &'a NamedTy)>> {
        fn deps<'b>(ty: &'b Ty, out: &mut Vec<&'b str>) {
            match ty {
                Ty::Named(n) => out.push(n),
                Ty::List(t) | Ty::Option(t) => deps(t, out),
                Ty::Tuple(ts) => ts.iter().for_each(|t| deps(t, out)),
                Ty::Record(fs) => fs.iter().for_each(|(_, t)| deps(t, out)),
                Ty::Variant(cs) => cs.iter().for_each(|(_, t)| {
                    if let Some(t) = t {
                        deps(t, out)
                    }
                }),
                Ty::Result(a, b) => {
                    if let Some(t) = a {
                        deps(t, out);
                    }
                    if let Some(t) = b {
                        deps(t, out);
                    }
                }
                _ => {}
            }
        }

        let declared: Vec<(&Iface, &NamedTy)> = self
            .m
            .exports
            .iter()
            .chain(&self.m.imports)
            .flat_map(|i| i.types.iter().map(move |t| (i, t)))
            .collect();
        let index: HashMap<&str, usize> = declared
            .iter()
            .enumerate()
            .map(|(i, (_, t))| (t.wit_name.as_str(), i))
            .collect();
        // 0 = unvisited, 1 = in progress, 2 = done
        let mut state = vec![0u8; declared.len()];
        let mut order = Vec::new();
        fn visit<'b>(
            i: usize,
            declared: &[(&'b Iface, &'b NamedTy)],
            index: &HashMap<&str, usize>,
            state: &mut [u8],
            order: &mut Vec<(&'b Iface, &'b NamedTy)>,
        ) -> Result<()> {
            match state[i] {
                2 => return Ok(()),
                1 => bail!(
                    "recursive WIT type `{}` (C++ cannot declare it by value)",
                    declared[i].1.wit_name
                ),
                _ => {}
            }
            state[i] = 1;
            let mut ds = Vec::new();
            deps(&declared[i].1.ty, &mut ds);
            for d in ds {
                if let Some(&j) = index.get(d) {
                    visit(j, declared, index, state, order)?;
                }
            }
            state[i] = 2;
            order.push(declared[i]);
            Ok(())
        }
        for i in 0..declared.len() {
            visit(i, &declared, &index, &mut state, &mut order)?;
        }
        Ok(order)
    }

    /// The V in a function's `crab::Res<V>` boundary signature.
    fn ret_value(&self, f: &'a Func, qual: &str) -> Result<String> {
        Ok(match self.classify(f)? {
            Ret::None | Ret::ResStr { ok: None, .. } => "std::monostate".to_string(),
            Ret::Plain(t) | Ret::ResTyped(t) => cpp_ty(t, qual)?,
            Ret::ResStr { ok: Some(t), .. } => cpp_ty(t, qual)?,
        })
    }

    /// "crab::Res<std::string> greet(gen::GreetRequest req)" — shared by the
    /// bindings.hpp declarations, the scaffolded stubs and missing_impls.
    fn method_sig(&self, f: &'a Func, qual: &str) -> Result<String> {
        let params = f
            .params
            .iter()
            .map(|(n, t)| Ok(format!("{} {}", cpp_ty(t, qual)?, cpp_param(n))))
            .collect::<Result<Vec<_>>>()?
            .join(", ");
        Ok(format!(
            "crab::Res<{}> {}({params})",
            self.ret_value(f, qual)?,
            cpp_ident(&f.wit_name)
        ))
    }

    /// One doc line describing where a function's returned .err goes.
    fn err_doc(&self, f: &'a Func) -> Result<&'static str> {
        Ok(match self.classify(f)? {
            Ret::ResStr { has_msg: true, .. } => {
                "// A non-empty .err encodes as the WIT result err case (a normal status-0 reply)."
            }
            Ret::ResStr { has_msg: false, .. } => {
                "// A non-empty .err encodes as the WIT result err case (no payload: the message is dropped)."
            }
            _ => "// A non-empty .err is a function-level failure (status-1 reply).",
        })
    }

    // -- bindings.hpp ----------------------------------------------------------

    fn bindings_hpp(&self) -> Result<String> {
        let mut w = W::spaces2();
        w.line("namespace gen {");
        w.line("");
        if self.need_result {
            w.line("// Result mirrors a WIT result<T, E> in value position: is_err selects the");
            w.line("// populated side; sides with no payload use std::monostate.");
            w.line("template <class T, class E>");
            w.line("struct Result {");
            w.line("  bool is_err = false;");
            w.line("  T ok{};");
            w.line("  E err{};");
            w.line("};");
            w.line("");
        }
        for (iface, t) in self.emit_order()? {
            self.emit_type_decl(&mut w, iface, t)?;
        }
        if !self.m.imports.is_empty() {
            w.line("// Typed mesh wrappers (defined in bindings.cpp) for the world's imported");
            w.line("// interfaces: each WIRE-encodes its params, calls the named workload");
            w.line("// through the host mesh (crabcraft.call import), and decodes the reply.");
            w.line("// The caller names the target deployment — crabgen never bakes one in;");
            w.line("// placement is the host's problem.");
            for iface in &self.m.imports {
                for f in &iface.funcs {
                    w.line(format!("{};", self.mesh_wrapper_sig(iface, f, "")?));
                }
            }
            w.line("");
        }
        w.line("}  // namespace gen");
        w.line("");
        w.line("// The application implementation: define each function below in impl.cpp.");
        w.line("// crabgen scaffolds stubs once and never edits the file; a missing");
        w.line("// definition is a LINK ERROR naming the symbol (and `crabgen regen` prints");
        w.line("// the missing signatures).");
        w.line("namespace impl {");
        w.line("");
        let export = &self.m.exports[0];
        for f in &export.funcs {
            w.line(format!(
                "// {} handles {}#{}.",
                cpp_ident(&f.wit_name),
                export.instance,
                f.wit_name
            ));
            w.line(self.err_doc(f)?);
            w.line(format!("{};", self.method_sig(f, "gen::")?));
            w.line("");
        }
        w.line("}  // namespace impl");

        let mut out = String::new();
        out.push_str(GENERATED_HEADER);
        out.push_str("\n//\n");
        out.push_str(&format!(
            "// Typed bindings for WIT package {}, world {}: native type\n",
            self.m.package, self.m.world
        ));
        out.push_str("// declarations for every WIT type (namespace gen), the impl:: functions\n");
        out.push_str("// the application defines (impl.cpp), and typed mesh wrappers for\n");
        out.push_str("// imported interfaces.\n");
        out.push_str("//\n");
        out.push_str(TYPE_TABLE);
        out.push_str("#pragma once\n\n");
        out.push_str("#include <cstdint>\n");
        out.push_str("#include <optional>\n");
        out.push_str("#include <string>\n");
        out.push_str("#include <string_view>\n");
        out.push_str("#include <tuple>\n");
        out.push_str("#include <variant>\n");
        out.push_str("#include <vector>\n\n");
        out.push_str("#include \"crab.hpp\"\n\n");
        out.push_str(&w.buf);
        Ok(trim_final(&out))
    }

    fn emit_type_decl(&self, w: &mut W, iface: &Iface, t: &NamedTy) -> Result<()> {
        let p = cpp_pascal(&t.wit_name);
        match &t.ty {
            Ty::Record(fields) => {
                w.line(format!(
                    "// {p} mirrors the WIT record `{}` ({}).",
                    t.wit_name, iface.instance
                ));
                w.open(format!("struct {p} {{"));
                for (n, ft) in fields {
                    w.line(format!("{} {}{{}};", cpp_ty(ft, "")?, cpp_ident(n)));
                }
                w.close("};");
                w.line("");
            }
            Ty::Variant(cases) => {
                for (c, payload) in cases {
                    if let Some(pt) = payload {
                        w.line(format!(
                            "// {p}{}: payload of the `{c}` case of {p} (alias below).",
                            cpp_pascal(c)
                        ));
                        w.open(format!("struct {p}{} {{", cpp_pascal(c)));
                        w.line(format!("{} value{{}};", cpp_ty(pt, "")?));
                        w.close("};");
                        w.line("");
                    }
                }
                let alts = cases
                    .iter()
                    .map(|(c, payload)| match payload {
                        Some(_) => format!("{p}{}", cpp_pascal(c)),
                        None => "std::monostate".to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
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
                    "// alternative i = WIT case i ({case_list}); construct payload cases"
                ));
                w.line(format!(
                    "// from their structs, empty cases with std::in_place_index<i>, and"
                ));
                w.line("// read with v.index() + std::get<i>(v).");
                w.line(format!("using {p} = std::variant<{alts}>;"));
                w.line("");
            }
            Ty::Enum(cases) => {
                w.line(format!(
                    "// {p} mirrors the WIT enum `{}` ({}); cases in declaration order.",
                    t.wit_name, iface.instance
                ));
                w.open(format!("enum class {p} : uint32_t {{"));
                for c in cases {
                    w.line(format!("{},", cpp_pascal(c)));
                }
                w.close("};");
                w.line("");
            }
            Ty::Flags(flags) => {
                w.line(format!(
                    "// {p} mirrors the WIT flags `{}` ({}); bit i = flag i.",
                    t.wit_name, iface.instance
                ));
                w.line(format!(
                    "// Combine with |: {p}{{{p}::{} | ...}}.",
                    cpp_pascal(&flags[0])
                ));
                w.open(format!("struct {p} {{"));
                w.line("uint64_t bits = 0;");
                for (i, f) in flags.iter().enumerate() {
                    w.line(format!(
                        "static constexpr uint64_t {} = 1ULL << {i};",
                        cpp_pascal(f)
                    ));
                }
                w.close("};");
                w.line("");
            }
            other => {
                w.line(format!(
                    "// {p} aliases the WIT type `{}` ({}).",
                    t.wit_name, iface.instance
                ));
                w.line(format!("using {p} = {};", cpp_ty(other, "")?));
                w.line("");
            }
        }
        Ok(())
    }

    // -- bindings.cpp ------------------------------------------------------------

    fn bindings_cpp(&self) -> Result<String> {
        let mut w = W::spaces2();
        w.line("namespace gen {");
        w.line("namespace {");
        w.line("");
        for (_iface, t) in self.emit_order()? {
            self.emit_type_codecs(&mut w, t)?;
        }
        let export = &self.m.exports[0];
        for f in &export.funcs {
            self.emit_handler(&mut w, export, f)?;
        }
        if !export.funcs.is_empty() {
            w.line("// Registration: ONE static object whose constructor registers every");
            w.line("// exported function with the runtime's handler map, in declaration");
            w.line("// order. SIOF-safe: crab::Handlers() is a function-local static.");
            w.open("struct Registration {");
            w.open("Registration() {");
            for f in &export.funcs {
                w.line(format!(
                    "crab::RegisterHandler(\"{}#{}\", handle_{});",
                    export.instance,
                    f.wit_name,
                    cpp_ident(&f.wit_name)
                ));
            }
            w.close("}");
            w.close("};");
            w.line("Registration registration;");
            w.line("");
        }
        w.line("}  // namespace");
        if !self.m.imports.is_empty() {
            w.line("");
            for iface in &self.m.imports {
                for f in &iface.funcs {
                    self.emit_mesh_wrapper(&mut w, iface, f)?;
                }
            }
        }
        w.line("}  // namespace gen");
        w.line("");
        w.line("namespace crab {");
        w.line("");
        w.line("// SchemaJson serves gen/schema.json (embedded in schema_json.h) through");
        w.line("// the runtime's crab_schema export.");
        w.open("std::string_view SchemaJson() {");
        w.line("return std::string_view(kSchemaJson, sizeof(kSchemaJson) - 1);");
        w.close("}");
        w.line("");
        w.line("}  // namespace crab");

        let mut out = String::new();
        out.push_str(GENERATED_HEADER);
        out.push_str("\n//\n");
        out.push_str(&format!(
            "// WIRE codec helpers for every named type, the crab_invoke dispatch\n\
             // handlers + registration, the typed mesh wrappers, and the embedded\n\
             // schema (crab::SchemaJson) for WIT package {}, world {}.\n",
            self.m.package, self.m.world
        ));
        out.push_str("//\n");
        out.push_str(
            "// Error semantics (WIRE.md section 2): a handler maps a non-empty impl\n\
             // .err to a status-1 reply \"<function>: <message>\" — except for functions\n\
             // whose WIT result is result<T, string> (or a result with no err payload),\n\
             // where the impl's .err becomes the WIRE result ERR CASE: a normal\n\
             // status-0 reply carrying an encoded result value.\n",
        );
        out.push_str("\n#include \"bindings.hpp\"\n\n");
        out.push_str("#include <utility>\n\n");
        if !self.m.imports.is_empty() {
            out.push_str("#include \"mesh.hpp\"\n");
        }
        out.push_str("#include \"schema_json.h\"\n\n");
        out.push_str(&w.buf);
        Ok(trim_final(&out))
    }

    fn emit_type_codecs(&self, w: &mut W, t: &NamedTy) -> Result<()> {
        let p = cpp_pascal(&t.wit_name);
        let enc_fail = "return {};".to_string();
        let dec_fail = format!("return crab::Res<{p}>::fail({{}});");
        match &t.ty {
            Ty::Record(fields) => {
                w.line(format!(
                    "// encode{p} appends the WIRE encoding of v; \"\" = success."
                ));
                w.open(format!(
                    "std::string encode{p}(std::vector<uint8_t>& out, const {p}& v) {{"
                ));
                let mut cx = Cx::new(enc_fail);
                for (n, ft) in fields {
                    emit_encode(w, &mut cx, ft, &format!("v.{}", cpp_ident(n)))?;
                }
                w.line("return std::string();");
                w.close("}");
                w.line("");
                w.line(format!("// decode{p} decodes a {p} off d."));
                w.open(format!("crab::Res<{p}> decode{p}(crab::Decoder& d) {{"));
                w.line(format!("{p} v{{}};"));
                let mut cx = Cx::new(dec_fail);
                for (n, ft) in fields {
                    emit_decode(w, &mut cx, ft, &format!("v.{}", cpp_ident(n)))?;
                }
                w.line("return {std::move(v), {}};");
                w.close("}");
                w.line("");
            }
            Ty::Enum(cases) => {
                let n = cases.len();
                w.line(format!(
                    "// encode{p} appends the WIRE encoding of v; \"\" = success."
                ));
                w.open(format!(
                    "std::string encode{p}(std::vector<uint8_t>& out, const {p}& v) {{"
                ));
                w.line(format!(
                    "if ((uint32_t)v >= {n}u) return \"invalid {p}: \" + std::to_string((uint32_t)v);"
                ));
                w.line("crab::EncodeCase(out, (uint32_t)v);");
                w.line("return std::string();");
                w.close("}");
                w.line("");
                w.line(format!("// decode{p} decodes a {p} off d."));
                w.open(format!("crab::Res<{p}> decode{p}(crab::Decoder& d) {{"));
                w.line(format!("auto r0 = d.EnumCase({n});"));
                w.line(format!(
                    "if (!r0.ok()) return crab::Res<{p}>::fail(std::move(r0.err));"
                ));
                w.line(format!("return {{({p})r0.val, {{}}}};"));
                w.close("}");
                w.line("");
            }
            Ty::Flags(flags) => {
                let n = flags.len();
                w.line(format!(
                    "// encode{p} appends the WIRE encoding of v; \"\" = success."
                ));
                w.open(format!(
                    "std::string encode{p}(std::vector<uint8_t>& out, const {p}& v) {{"
                ));
                if n < 64 {
                    w.line(format!(
                        "if ((v.bits >> {n}) != 0) return \"invalid {p}: unknown bits\";"
                    ));
                }
                w.line(format!("std::vector<bool> bits({n});"));
                w.open(format!("for (size_t i = 0; i < {n}; i++) {{"));
                w.line("bits[i] = (v.bits >> i) & 1;");
                w.close("}");
                w.line("crab::EncodeFlags(out, bits);");
                w.line("return std::string();");
                w.close("}");
                w.line("");
                w.line(format!("// decode{p} decodes a {p} off d."));
                w.open(format!("crab::Res<{p}> decode{p}(crab::Decoder& d) {{"));
                w.line(format!("auto r0 = d.Flags({n});"));
                w.line(format!(
                    "if (!r0.ok()) return crab::Res<{p}>::fail(std::move(r0.err));"
                ));
                w.line(format!("{p} v{{}};"));
                w.open(format!("for (size_t i = 0; i < {n}; i++) {{"));
                w.line("if (r0.val[i]) v.bits |= 1ULL << i;");
                w.close("}");
                w.line("return {v, {}};");
                w.close("}");
                w.line("");
            }
            Ty::Variant(cases) => {
                w.line(format!(
                    "// encode{p} appends the WIRE encoding of v; \"\" = success."
                ));
                w.open(format!(
                    "std::string encode{p}(std::vector<uint8_t>& out, const {p}& v) {{"
                ));
                w.line("crab::EncodeCase(out, (uint32_t)v.index());");
                let mut cx = Cx::new(enc_fail);
                if cases.iter().any(|(_, payload)| payload.is_some()) {
                    w.open("switch (v.index()) {");
                    for (i, (_c, payload)) in cases.iter().enumerate() {
                        let Some(pt) = payload else { continue };
                        w.open(format!("case {i}: {{"));
                        emit_encode(w, &mut cx, pt, &format!("std::get<{i}>(v).value"))?;
                        w.line("break;");
                        w.close("}");
                    }
                    w.line("default:");
                    w.ind += 1;
                    w.line("break;");
                    w.ind -= 1;
                    w.close("}");
                }
                w.line("return std::string();");
                w.close("}");
                w.line("");

                w.line(format!("// decode{p} decodes a {p} off d."));
                w.open(format!("crab::Res<{p}> decode{p}(crab::Decoder& d) {{"));
                let mut cx = Cx::new(format!("return crab::Res<{p}>::fail({{}});"));
                let r = cx.fresh("r"); // r0, matching the temp scheme
                w.line(format!("auto {r} = d.VariantCase({});", cases.len()));
                w.line(format!(
                    "if (!{r}.ok()) return crab::Res<{p}>::fail(std::move({r}.err));"
                ));
                w.line(format!("{p} v{{}};"));
                w.open(format!("switch ({r}.val) {{"));
                for (i, (c, payload)) in cases.iter().enumerate() {
                    match payload {
                        None => {
                            w.line(format!("case {i}:"));
                            w.ind += 1;
                            w.line(format!("v = {p}{{std::in_place_index<{i}>}};"));
                            w.line("break;");
                            w.ind -= 1;
                        }
                        Some(pt) => {
                            w.open(format!("case {i}: {{"));
                            let cs = format!("{p}{}", cpp_pascal(c));
                            let x = cx.fresh("x");
                            w.line(format!("{cs} {x}{{}};"));
                            emit_decode(w, &mut cx, pt, &format!("{x}.value"))?;
                            w.line(format!(
                                "v = {p}{{std::in_place_index<{i}>, std::move({x})}};"
                            ));
                            w.line("break;");
                            w.close("}");
                        }
                    }
                }
                w.line("default:");
                w.ind += 1;
                w.line("break;  // unreachable: VariantCase bounds-checks");
                w.ind -= 1;
                w.close("}");
                w.line("return {std::move(v), {}};");
                w.close("}");
                w.line("");
            }
            other => {
                // alias: identical C++ type, helpers delegate to the structure
                w.line(format!(
                    "// encode{p} appends the WIRE encoding of v; \"\" = success."
                ));
                w.open(format!(
                    "std::string encode{p}(std::vector<uint8_t>& out, const {p}& v) {{"
                ));
                let mut cx = Cx::new(enc_fail);
                emit_encode(w, &mut cx, other, "v")?;
                w.line("return std::string();");
                w.close("}");
                w.line("");
                w.line(format!("// decode{p} decodes a {p} off d."));
                w.open(format!("crab::Res<{p}> decode{p}(crab::Decoder& d) {{"));
                w.line(format!("{p} v{{}};"));
                let mut cx = Cx::new(dec_fail);
                emit_decode(w, &mut cx, other, "v")?;
                w.line("return {std::move(v), {}};");
                w.close("}");
                w.line("");
            }
        }
        Ok(())
    }

    fn emit_handler(&self, w: &mut W, iface: &Iface, f: &'a Func) -> Result<()> {
        const RES: &str = "crab::Res<std::vector<uint8_t>>";
        let ident = cpp_ident(&f.wit_name);
        w.line(format!(
            "// handle_{ident} dispatches {}#{}; registered by Registration below.",
            iface.instance, f.wit_name
        ));
        w.open(format!("{RES} handle_{ident}(crab::Decoder& d) {{"));
        let mut cx = Cx::new(format!("return {RES}::fail(\"bad params: \" + {{}});"));
        for (n, t) in &f.params {
            let pn = cpp_param(n);
            w.line(format!("{} {pn}{{}};", cpp_ty(t, "")?));
            emit_decode(w, &mut cx, t, &pn)?;
        }
        w.line("std::string fin = d.Finish(\"params\");");
        w.line(format!(
            "if (!fin.empty()) return {RES}::fail(\"bad params: \" + fin);"
        ));
        let args = f
            .params
            .iter()
            .map(|(n, _)| format!("std::move({})", cpp_param(n)))
            .collect::<Vec<_>>()
            .join(", ");
        w.line(format!("auto r = impl::{ident}({args});"));
        cx.fail_fmt = format!("return {RES}::fail({{}});");
        match self.classify(f)? {
            Ret::None => {
                w.line(format!(
                    "if (!r.ok()) return {RES}::fail(std::move(r.err));"
                ));
                w.line("return {std::vector<uint8_t>{}, {}};");
            }
            Ret::Plain(t) | Ret::ResTyped(t) => {
                w.line(format!(
                    "if (!r.ok()) return {RES}::fail(std::move(r.err));"
                ));
                w.line("std::vector<uint8_t> out;");
                emit_encode(w, &mut cx, t, "r.val")?;
                w.line("return {std::move(out), {}};");
            }
            Ret::ResStr { ok, has_msg } => {
                w.line("std::vector<uint8_t> out;");
                w.open("if (!r.ok()) {");
                w.line("crab::EncodeResultTag(out, true);");
                if has_msg {
                    w.line("crab::EncodeString(out, r.err);");
                } else {
                    w.line("// the WIT err side has no payload: the message is dropped");
                }
                w.line("return {std::move(out), {}};");
                w.close("}");
                w.line("crab::EncodeResultTag(out, false);");
                if let Some(okt) = ok {
                    emit_encode(w, &mut cx, okt, "r.val")?;
                }
                w.line("return {std::move(out), {}};");
            }
        }
        w.close("}");
        w.line("");
        Ok(())
    }

    // -- mesh wrappers -----------------------------------------------------------

    fn mesh_wrapper_sig(&self, iface: &Iface, f: &'a Func, qual: &str) -> Result<String> {
        let mut params = vec!["std::string_view workload".to_string()];
        for (n, t) in &f.params {
            params.push(format!("{} {}", cpp_ty(t, qual)?, cpp_param(n)));
        }
        Ok(format!(
            "crab::Res<{}> {}({})",
            self.ret_value(f, qual)?,
            mesh_wrapper_name(iface, f),
            params.join(", ")
        ))
    }

    fn emit_mesh_wrapper(&self, w: &mut W, iface: &Iface, f: &'a Func) -> Result<()> {
        let name = mesh_wrapper_name(iface, f);
        let addr = format!("{}#{}", iface.instance, f.wit_name);
        let vty = self.ret_value(f, "")?;
        let res = format!("crab::Res<{vty}>");
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
        w.open(format!("{} {{", self.mesh_wrapper_sig(iface, f, "")?));
        let mut cx = Cx::new(format!("return {res}::fail({{}});"));
        w.line("std::vector<uint8_t> out;");
        for (n, t) in &f.params {
            emit_encode(w, &mut cx, t, &cpp_param(n))?;
        }
        w.line(format!(
            "auto body = crab::MeshCall(workload, \"{addr}\", out);"
        ));
        w.line(format!(
            "if (!body.ok()) return {res}::fail(std::move(body.err));"
        ));
        w.line("crab::Decoder d(body.val.data(), body.val.size());");
        let finish = |w: &mut W| {
            w.line("std::string fin = d.Finish(\"reply\");");
            w.line(format!(
                "if (!fin.empty()) return {res}::fail(std::move(fin));"
            ));
        };
        match ret {
            Ret::None => {
                finish(w);
                w.line("return {std::monostate{}, {}};");
            }
            Ret::Plain(t) | Ret::ResTyped(t) => {
                w.line(format!("{vty} r{{}};"));
                emit_decode(w, &mut cx, t, "r")?;
                finish(w);
                w.line("return {std::move(r), {}};");
            }
            Ret::ResStr { ok, has_msg } => {
                let r = cx.fresh("r");
                w.line(format!("auto {r} = d.ResultTag();"));
                w.line(format!(
                    "if (!{r}.ok()) return {res}::fail(std::move({r}.err));"
                ));
                w.open(format!("if ({r}.val) {{"));
                if has_msg {
                    let rm = cx.fresh("r");
                    w.line(format!("auto {rm} = d.String();"));
                    w.line(format!(
                        "if (!{rm}.ok()) return {res}::fail(std::move({rm}.err));"
                    ));
                    finish(w);
                    w.line(format!("return {res}::fail(std::move({rm}.val));"));
                } else {
                    finish(w);
                    w.line(format!(
                        "return {res}::fail(\"{}: err result (no payload)\");",
                        f.wit_name
                    ));
                }
                w.close("}");
                match ok {
                    Some(okt) => {
                        w.line(format!("{vty} r{{}};"));
                        emit_decode(w, &mut cx, okt, "r")?;
                        finish(w);
                        w.line("return {std::move(r), {}};");
                    }
                    None => {
                        finish(w);
                        w.line("return {std::monostate{}, {}};");
                    }
                }
            }
        }
        w.close("}");
        w.line("");
        Ok(())
    }

    // -- scaffold files ----------------------------------------------------------

    fn impl_cpp(&self) -> Result<String> {
        let export = &self.m.exports[0];
        let mut w = W::spaces2();
        w.line("// impl.cpp — the application half of this guest: define the impl:: functions");
        w.line("// declared in gen/bindings.hpp. crabgen scaffolds this file ONCE and never");
        w.line("// overwrites it; `crabgen regen` prints any missing function signatures");
        w.line("// instead of editing it (a missing definition is also a LINK ERROR naming");
        w.line("// the symbol when build.sh links the module).");
        w.line("#include \"gen/bindings.hpp\"");
        w.line("");
        w.line("namespace impl {");
        w.line("");
        for f in &export.funcs {
            w.line(format!(
                "// {} handles {}#{}.",
                cpp_ident(&f.wit_name),
                export.instance,
                f.wit_name
            ));
            w.line(self.err_doc(f)?);
            w.open(format!("{} {{", self.method_sig(f, "gen::")?));
            w.line(format!(
                "return crab::Res<{}>::fail(\"unimplemented: {}\");",
                self.ret_value(f, "gen::")?,
                f.wit_name
            ));
            w.close("}");
            w.line("");
        }
        w.line("}  // namespace impl");
        Ok(trim_final(&w.buf))
    }
}

/// `<iface>_<fn>` mesh wrapper name; the compound is keyword-mangled too
/// (`co` + `await` would otherwise spell `co_await`).
fn mesh_wrapper_name(iface: &Iface, f: &Func) -> String {
    let mut s = format!(
        "{}_{}",
        cpp_snake(iface_short(&iface.instance)),
        cpp_snake(&f.wit_name)
    );
    if CPP_KEYWORDS.contains(&s.as_str()) {
        s.push('_');
    }
    s
}

/// The shared type-mapping table comment (bindings.hpp header).
const TYPE_TABLE: &str = "\
// Type mapping (WIT -> C++):
//   bool/u*/s*/f32/f64 -> bool/uint*_t/int*_t/float/double
//   char               -> uint32_t (unicode scalar, validated on the wire)
//   string             -> std::string (UTF-8, validated on the wire)
//   list<T>            -> std::vector<T>
//   option<T>          -> std::optional<T>
//   tuple<A, B, ..>    -> std::tuple<A, B, ..>
//   record             -> struct with snake_case fields
//   variant            -> std::variant alias, alternative i = WIT case i:
//                         payload cases are `struct <Variant><Case> {T value;}`
//                         alternatives, empty cases are std::monostate
//   enum               -> enum class : uint32_t, cases in declaration order
//   flags              -> struct { uint64_t bits; } + per-flag bit consts
//   result<T, E>       -> value position: gen::Result<T, E> (absent sides
//                         are std::monostate); function-result position maps
//                         onto the impl's crab::Res channel (see below)
//
// Every impl:: function returns crab::Res<V> (ok() == .err.empty()):
//   - no WIT result:               V = std::monostate; .err = status-1 reply
//   - plain value T:               V = T;              .err = status-1 reply
//   - result<T, string> (or absent err payload): V = T (std::monostate when
//     the ok side is absent); a non-empty .err is the WIRE result ERR CASE —
//     a normal status-0 reply, NOT a function-level failure
//   - result<T, E> with any other E: V = gen::Result<T, E>; .err = status-1
//
// Name casing: WIT kebab-case -> PascalCase types/case structs/enum cases/
// flag consts (\"a-u8\" -> \"AU8\": capitalize each dash segment), snake_case
// functions/params/fields (every segment lowercased); names hitting a C++
// keyword or a generated local get a trailing underscore.
//
";

// ---------------------------------------------------------------------------
// non-Gen file contents
// ---------------------------------------------------------------------------

/// gen/schema_json.h: the resolved-WIT JSON as a raw string literal. The
/// delimiter is chosen defensively: lengthened until the schema cannot
/// possibly contain the closing `)DELIM"` sequence.
fn schema_json_h(schema: &str) -> String {
    let mut delim = "CRABGEN".to_string();
    let mut n = 0u32;
    while schema.contains(&format!("){delim}\"")) {
        n += 1;
        delim = format!("CRABGEN{n}");
    }
    format!(
        "{GENERATED_HEADER}\n//\n\
         // The resolved-WIT JSON this module serves from crab_schema — the same\n\
         // bytes as gen/schema.json, embedded so the wasm needs no filesystem.\n\
         // bindings.cpp's crab::SchemaJson() returns it.\n\
         #pragma once\n\n\
         static const char kSchemaJson[] = R\"{delim}({schema}){delim}\";\n"
    )
}

fn build_sh(name: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
# Build the {name} reactor module (scaffolded by crabgen; edit freely — this
# file is written once and never overwritten).
#
# -mexec-model=reactor = a WASI REACTOR: wasi-libc provides `_initialize`
# (run once by the host) and no `_start` is needed at invoke time.
# -mno-simd128 keeps SIMD opcodes out (the wasmcraft engine refuses them);
# -fno-exceptions/-fno-rtti match the runtime's crab::Res calling convention.
# gen/*.cpp is a glob ON PURPOSE: regen adds/removes gen/mesh.cpp as the WIT
# gains/loses imports, and this scaffold-once script must keep linking the
# right set without edits.
set -euo pipefail
cd "$(dirname "$0")"

nix shell nixpkgs#zig --command zig c++ \
  -target wasm32-wasi -mexec-model=reactor -mno-simd128 \
  -fno-exceptions -fno-rtti -std=c++17 -Oz -Wl,--export-memory \
  -o ../../modules/{name}.wasm \
  impl.cpp gen/*.cpp

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
        r#"# {name} — crabcraft guest (C++ lane)

<!-- generated by crabgen — edits will be overwritten on regen -->

Generated by crabgen. `{name}.wit` is the source of truth; `gen/` and this
README are GENERATED — never edit them, crabgen rewrites them wholesale on
every regen. Your code lives in `impl.cpp` (crabgen never touches it):
define every function declared in `gen/bindings.hpp`'s `namespace impl`.
A missing definition is a LINK ERROR naming the symbol, and
`crabgen regen` prints the missing signatures.

## Build

    ./build.sh

zig c++ (via nix) builds a wasm32-wasi REACTOR at `../../modules/{name}.wasm`
(`-mexec-model=reactor -mno-simd128 -fno-exceptions -fno-rtti -std=c++17`),
then the script fails hard if any SIMD (0xfd) opcodes snuck in — the
wasmcraft engine refuses them. It compiles `impl.cpp gen/*.cpp` (a glob, so
regen can add/remove `gen/mesh.cpp` without touching the script).

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
2. `crabgen regen guest/{name}` — rewrites `gen/` and prints typed
   signatures for any functions missing from `impl.cpp`.
3. Paste the stubs into `impl.cpp` and implement them.
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
        assert_eq!(cpp_pascal("echo-everything"), "EchoEverything");
        assert_eq!(cpp_pascal("a-u8"), "AU8");
        assert_eq!(cpp_pascal("e2e-cpp"), "E2eCpp");
        assert_eq!(cpp_pascal("x"), "X");
    }

    #[test]
    fn snake_lowercases_every_segment() {
        assert_eq!(cpp_snake("echo-everything"), "echo_everything");
        assert_eq!(cpp_snake("AB"), "ab");
        assert_eq!(cpp_snake("a-u8"), "a_u8");
    }

    #[test]
    fn idents_are_keyword_mangled() {
        assert_eq!(cpp_ident("delete"), "delete_");
        assert_eq!(cpp_ident("new"), "new_");
        assert_eq!(cpp_ident("template"), "template_");
        assert_eq!(cpp_ident("name"), "name");
    }

    #[test]
    fn params_avoid_generated_locals() {
        assert_eq!(cpp_param("d"), "d_");
        assert_eq!(cpp_param("out"), "out_");
        assert_eq!(cpp_param("r"), "r_");
        assert_eq!(cpp_param("fin"), "fin_");
        assert_eq!(cpp_param("body"), "body_");
        assert_eq!(cpp_param("workload"), "workload_");
        assert_eq!(cpp_param("r0"), "r0_"); // temp-shaped
        assert_eq!(cpp_param("e1"), "e1_"); // temp-shaped
        assert_eq!(cpp_param("x2"), "x2_"); // temp-shaped
        assert_eq!(cpp_param("i3"), "i3_"); // temp-shaped
        assert_eq!(cpp_param("e"), "e"); // bare prefixes are fine
        assert_eq!(cpp_param("xs"), "xs");
        assert_eq!(cpp_param("msg"), "msg");
        // codec-helper locals never share a scope with params: no mangling
        assert_eq!(cpp_param("v"), "v");
        assert_eq!(cpp_param("bits"), "bits");
    }

    #[test]
    fn schema_header_picks_a_safe_raw_delimiter() {
        let plain = schema_json_h("{\"a\": 1}");
        assert!(plain.contains("R\"CRABGEN({\"a\": 1})CRABGEN\""));
        // a schema containing the default closer forces a longer delimiter
        let hostile = schema_json_h("evil )CRABGEN\" body");
        assert!(hostile.contains("R\"CRABGEN1("));
        assert!(hostile.contains(")CRABGEN1\";"));
    }
}
