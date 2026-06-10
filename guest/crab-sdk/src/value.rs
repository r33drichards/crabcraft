//! Dynamic value & type model mirroring the WIT types of WIRE.md section 1.

/// Wire type descriptor. The wire encoding is untagged, so decoding a buffer
/// requires one of these per parameter (registered alongside the handler).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
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
    List(Box<Type>),
    /// Field types in declaration order (names don't affect the wire).
    Record(Vec<Type>),
    Tuple(Vec<Type>),
    /// One entry per case, in declaration order; `None` = case has no payload.
    Variant(Vec<Option<Type>>),
    /// Number of cases.
    Enum(u32),
    Option(Box<Type>),
    Result {
        ok: Option<Box<Type>>,
        err: Option<Box<Type>>,
    },
    /// Number of flags.
    Flags(u32),
}

/// Dynamic, schema-driven value. Tagged in memory, untagged on the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    U8(u8),
    U16(u16),
    U32(u32),
    U64(u64),
    S8(i8),
    S16(i16),
    S32(i32),
    S64(i64),
    F32(f32),
    F64(f64),
    Char(char),
    String(String),
    List(Vec<Value>),
    /// Field values in declaration order.
    Record(Vec<Value>),
    Tuple(Vec<Value>),
    Variant {
        case: u32,
        payload: Option<Box<Value>>,
    },
    Enum(u32),
    Option(Option<Box<Value>>),
    Result(Result<Option<Box<Value>>, Option<Box<Value>>>),
    /// One bool per flag, in declaration order. Length = flag count.
    Flags(Vec<bool>),
}

impl Value {
    /// Convenience: `some(v)`.
    pub fn some(v: Value) -> Value {
        Value::Option(Some(Box::new(v)))
    }
    /// Convenience: `none`.
    pub fn none() -> Value {
        Value::Option(None)
    }
    /// Convenience: the empty result of a function with no result type
    /// (encodes to zero bytes).
    pub fn unit() -> Value {
        Value::Tuple(Vec::new())
    }
}

// ---- ergonomic conversions ------------------------------------------------

macro_rules! from_prim {
    ($($t:ty => $variant:ident),* $(,)?) => {$(
        impl From<$t> for Value {
            fn from(v: $t) -> Value { Value::$variant(v) }
        }
        impl TryFrom<Value> for $t {
            type Error = String;
            fn try_from(v: Value) -> Result<$t, String> {
                match v {
                    Value::$variant(x) => Ok(x),
                    other => Err(format!(
                        concat!("expected ", stringify!($variant), ", got {:?}"),
                        other
                    )),
                }
            }
        }
    )*};
}

from_prim! {
    bool => Bool,
    u8 => U8, u16 => U16, u32 => U32, u64 => U64,
    i8 => S8, i16 => S16, i32 => S32, i64 => S64,
    f32 => F32, f64 => F64,
    char => Char,
    String => String,
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::String(s.to_string())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(o: Option<T>) -> Value {
        Value::Option(o.map(|v| Box::new(v.into())))
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(items: Vec<T>) -> Value {
        Value::List(items.into_iter().map(Into::into).collect())
    }
}
