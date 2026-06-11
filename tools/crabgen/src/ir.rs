//! Internal representation between wit-parser and the language backends.
//! One traversal of the resolved WIT produces this; every backend consumes it.

/// One guest module: a WIT world with exactly one exported interface (v1)
/// and any number of imported interfaces (mesh stubs).
#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    /// e.g. "crab:hello@0.1.0"
    pub package: String,
    pub world: String,
    /// v1: exactly one
    pub exports: Vec<Iface>,
    /// mesh stubs
    pub imports: Vec<Iface>,
    /// serde-serialized `wit_parser::Resolve` — what `crab_schema` serves
    pub schema_json: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Iface {
    /// e.g. "crab:hello/greeter@0.1.0"
    pub instance: String,
    pub funcs: Vec<Func>,
    pub types: Vec<NamedTy>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Func {
    pub wit_name: String,
    pub params: Vec<(String, Ty)>,
    pub result: Option<Ty>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NamedTy {
    pub wit_name: String,
    pub ty: Ty,
}

/// WIRE section-1 type tree. `Named` references a `NamedTy` declared in the
/// same `Iface` so backends can emit named declarations once and reference
/// them at use sites.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Bool,
    U8,
    U16,
    U32,
    U64,
    S8,
    S16,
    S32,
    S64,
    F32,
    F64,
    Char,
    String,
    List(Box<Ty>),
    Option(Box<Ty>),
    Tuple(Vec<Ty>),
    Record(Vec<(String, Ty)>),
    Variant(Vec<(String, Option<Ty>)>),
    Enum(Vec<String>),
    Flags(Vec<String>),
    Result(Option<Box<Ty>>, Option<Box<Ty>>),
    Named(String),
}
