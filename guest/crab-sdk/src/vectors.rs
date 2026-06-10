//! Golden encoding vectors: ground truth for the Lua host codec.
//!
//! Each vector carries a JSON type descriptor + JSON value representation
//! (as string literals, used verbatim in wit/vectors.json) alongside the
//! in-memory `Type`/`Value` pair and the expected hex encoding. Tests assert
//! `encode(value) == hex` and `decode(hex) == value`; the `gen-vectors` bin
//! emits wit/vectors.json from this table.
//!
//! JSON value conventions (the Lua test harness must mirror these):
//! - record: object keyed by field name (field order comes from the type)
//! - option: `null` for none, the inner value for some
//! - variant: `{"case": <index>, "payload": <value or null>}`
//! - enum: case index (number)
//! - result: `{"ok": <value or null>}` / `{"err": <value or null>}`
//! - flags: array of the SET flag indices
//! - u64/s64 beyond 2^53: decimal string (Lua number precision caveat)
//! - char: one-character string

use crate::value::{Type, Value};

pub struct Vector {
    pub desc: &'static str,
    /// JSON type descriptor, e.g. `{"kind":"u32"}`.
    pub type_json: &'static str,
    /// JSON representation of the value.
    pub value_json: &'static str,
    pub ty: Type,
    pub value: Value,
    /// Expected encoding, lowercase hex.
    pub hex: &'static str,
}

fn greet_request_type() -> Type {
    Type::Record(vec![Type::String, Type::Option(Box::new(Type::Bool))])
}

const GREET_REQUEST_TYPE_JSON: &str = r#"{"kind":"record","fields":[{"name":"name","type":{"kind":"string"}},{"name":"excited","type":{"kind":"option","inner":{"kind":"bool"}}}]}"#;

pub fn vectors() -> Vec<Vector> {
    vec![
        Vector {
            desc: "bool true",
            type_json: r#"{"kind":"bool"}"#,
            value_json: "true",
            ty: Type::Bool,
            value: Value::Bool(true),
            hex: "01",
        },
        Vector {
            desc: "bool false",
            type_json: r#"{"kind":"bool"}"#,
            value_json: "false",
            ty: Type::Bool,
            value: Value::Bool(false),
            hex: "00",
        },
        Vector {
            desc: "u8 0",
            type_json: r#"{"kind":"u8"}"#,
            value_json: "0",
            ty: Type::U8,
            value: Value::U8(0),
            hex: "00",
        },
        Vector {
            desc: "u8 255 (two-byte uleb)",
            type_json: r#"{"kind":"u8"}"#,
            value_json: "255",
            ty: Type::U8,
            value: Value::U8(255),
            hex: "ff01",
        },
        Vector {
            desc: "u32 7 (one-byte uleb)",
            type_json: r#"{"kind":"u32"}"#,
            value_json: "7",
            ty: Type::U32,
            value: Value::U32(7),
            hex: "07",
        },
        Vector {
            desc: "u32 624485 (multi-byte uleb)",
            type_json: r#"{"kind":"u32"}"#,
            value_json: "624485",
            ty: Type::U32,
            value: Value::U32(624485),
            hex: "e58e26",
        },
        Vector {
            desc: "u64 2^40-1",
            type_json: r#"{"kind":"u64"}"#,
            value_json: "1099511627775",
            ty: Type::U64,
            value: Value::U64(1099511627775),
            hex: "ffffffffff1f",
        },
        Vector {
            desc: "u64 max (value as decimal string: beyond Lua float precision)",
            type_json: r#"{"kind":"u64"}"#,
            value_json: r#""18446744073709551615""#,
            ty: Type::U64,
            value: Value::U64(u64::MAX),
            hex: "ffffffffffffffffff01",
        },
        Vector {
            desc: "s32 -1",
            type_json: r#"{"kind":"s32"}"#,
            value_json: "-1",
            ty: Type::S32,
            value: Value::S32(-1),
            hex: "7f",
        },
        Vector {
            desc: "s32 -624485",
            type_json: r#"{"kind":"s32"}"#,
            value_json: "-624485",
            ty: Type::S32,
            value: Value::S32(-624485),
            hex: "9bf159",
        },
        Vector {
            desc: "f32 1.5",
            type_json: r#"{"kind":"f32"}"#,
            value_json: "1.5",
            ty: Type::F32,
            value: Value::F32(1.5),
            hex: "0000c03f",
        },
        Vector {
            desc: "f64 -2.75",
            type_json: r#"{"kind":"f64"}"#,
            value_json: "-2.75",
            ty: Type::F64,
            value: Value::F64(-2.75),
            hex: "00000000000006c0",
        },
        Vector {
            desc: "char 'A'",
            type_json: r#"{"kind":"char"}"#,
            value_json: r#""A""#,
            ty: Type::Char,
            value: Value::Char('A'),
            hex: "41",
        },
        Vector {
            desc: "char U+1F980 (crab)",
            type_json: r#"{"kind":"char"}"#,
            value_json: "\"\u{1F980}\"",
            ty: Type::Char,
            value: Value::Char('\u{1F980}'),
            hex: "80f307",
        },
        Vector {
            desc: "string 'hi'",
            type_json: r#"{"kind":"string"}"#,
            value_json: r#""hi""#,
            ty: Type::String,
            value: Value::String("hi".into()),
            hex: "026869",
        },
        Vector {
            desc: "string 'h\u{e9}llo' (utf-8, length in bytes)",
            type_json: r#"{"kind":"string"}"#,
            value_json: "\"h\u{e9}llo\"",
            ty: Type::String,
            value: Value::String("h\u{e9}llo".into()),
            hex: "0668c3a96c6c6f",
        },
        Vector {
            desc: "list<u32> [1,2,3]",
            type_json: r#"{"kind":"list","element":{"kind":"u32"}}"#,
            value_json: "[1,2,3]",
            ty: Type::List(Box::new(Type::U32)),
            value: Value::List(vec![Value::U32(1), Value::U32(2), Value::U32(3)]),
            hex: "03010203",
        },
        Vector {
            desc: "list<u32> empty",
            type_json: r#"{"kind":"list","element":{"kind":"u32"}}"#,
            value_json: "[]",
            ty: Type::List(Box::new(Type::U32)),
            value: Value::List(vec![]),
            hex: "00",
        },
        Vector {
            desc: "greet-request name=steve excited=none",
            type_json: GREET_REQUEST_TYPE_JSON,
            value_json: r#"{"name":"steve","excited":null}"#,
            ty: greet_request_type(),
            value: Value::Record(vec![Value::String("steve".into()), Value::none()]),
            hex: "05737465766500",
        },
        Vector {
            desc: "greet-request name=steve excited=some(true)",
            type_json: GREET_REQUEST_TYPE_JSON,
            value_json: r#"{"name":"steve","excited":true}"#,
            ty: greet_request_type(),
            value: Value::Record(vec![
                Value::String("steve".into()),
                Value::some(Value::Bool(true)),
            ]),
            hex: "0573746576650101",
        },
        Vector {
            desc: "tuple<u32,string> (7,'ok')",
            type_json: r#"{"kind":"tuple","members":[{"kind":"u32"},{"kind":"string"}]}"#,
            value_json: r#"[7,"ok"]"#,
            ty: Type::Tuple(vec![Type::U32, Type::String]),
            value: Value::Tuple(vec![Value::U32(7), Value::String("ok".into())]),
            hex: "07026f6b",
        },
        Vector {
            desc: "enum case 2 of 4",
            type_json: r#"{"kind":"enum","cases":["north","south","east","west"]}"#,
            value_json: "2",
            ty: Type::Enum(4),
            value: Value::Enum(2),
            hex: "02",
        },
        Vector {
            desc: "variant case 1 with u32 payload",
            type_json: r#"{"kind":"variant","cases":[{"name":"empty","payload":null},{"name":"num","payload":{"kind":"u32"}}]}"#,
            value_json: r#"{"case":1,"payload":42}"#,
            ty: Type::Variant(vec![None, Some(Type::U32)]),
            value: Value::Variant {
                case: 1,
                payload: Some(Box::new(Value::U32(42))),
            },
            hex: "012a",
        },
        Vector {
            desc: "variant case 0 without payload",
            type_json: r#"{"kind":"variant","cases":[{"name":"empty","payload":null},{"name":"num","payload":{"kind":"u32"}}]}"#,
            value_json: r#"{"case":0,"payload":null}"#,
            ty: Type::Variant(vec![None, Some(Type::U32)]),
            value: Value::Variant {
                case: 0,
                payload: None,
            },
            hex: "00",
        },
        Vector {
            desc: "option<string> none",
            type_json: r#"{"kind":"option","inner":{"kind":"string"}}"#,
            value_json: "null",
            ty: Type::Option(Box::new(Type::String)),
            value: Value::none(),
            hex: "00",
        },
        Vector {
            desc: "option<string> some('yo')",
            type_json: r#"{"kind":"option","inner":{"kind":"string"}}"#,
            value_json: r#""yo""#,
            ty: Type::Option(Box::new(Type::String)),
            value: Value::some(Value::String("yo".into())),
            hex: "0102796f",
        },
        Vector {
            desc: "result<u32,string> ok(7)",
            type_json: r#"{"kind":"result","ok":{"kind":"u32"},"err":{"kind":"string"}}"#,
            value_json: r#"{"ok":7}"#,
            ty: Type::Result {
                ok: Some(Box::new(Type::U32)),
                err: Some(Box::new(Type::String)),
            },
            value: Value::Result(Ok(Some(Box::new(Value::U32(7))))),
            hex: "0007",
        },
        Vector {
            desc: "result<u32,string> err('boom')",
            type_json: r#"{"kind":"result","ok":{"kind":"u32"},"err":{"kind":"string"}}"#,
            value_json: r#"{"err":"boom"}"#,
            ty: Type::Result {
                ok: Some(Box::new(Type::U32)),
                err: Some(Box::new(Type::String)),
            },
            value: Value::Result(Err(Some(Box::new(Value::String("boom".into()))))),
            hex: "0104626f6f6d",
        },
        Vector {
            desc: "flags 10 defined, {0,3,9} set (2 bytes, LE bit/byte order)",
            type_json: r#"{"kind":"flags","count":10}"#,
            value_json: "[0,3,9]",
            ty: Type::Flags(10),
            value: Value::Flags(
                (0..10).map(|i| i == 0 || i == 3 || i == 9).collect(),
            ),
            hex: "0902",
        },
    ]
}
