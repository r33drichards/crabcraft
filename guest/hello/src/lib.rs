//! crabcraft hello module: implements crab:hello/greeter@0.1.0
//! (wit/hello.wit). Built as a wasm32-wasip1 reactor cdylib.

use crab_sdk::{Registry, Type, Value};

/// Resolved-WIT JSON for this module, served verbatim via crab_schema.
const SCHEMA: &str = include_str!("../../../wit/hello.json");

const INSTANCE: &str = "crab:hello/greeter@0.1.0";

fn setup(r: &mut Registry) {
    r.register(
        &format!("{INSTANCE}#greet"),
        // greet(req: greet-request); record fields in declaration order:
        // { name: string, excited: option<bool> }
        vec![Type::Record(vec![
            Type::String,
            Type::Option(Box::new(Type::Bool)),
        ])],
        greet,
    );
    r.register(&format!("{INSTANCE}#add"), vec![Type::U32, Type::U32], add);
}

fn greet(args: Vec<Value>) -> Result<Value, String> {
    let mut args = args.into_iter();
    let Some(Value::Record(fields)) = args.next() else {
        return Err("greet: expected greet-request record".into());
    };
    let mut fields = fields.into_iter();
    let name: String = fields
        .next()
        .ok_or("greet: missing field 'name'")?
        .try_into()?;
    let excited = matches!(
        fields.next(),
        Some(Value::Option(Some(b))) if *b == Value::Bool(true)
    );
    let bang = if excited { "!!!" } else { "!" };
    Ok(Value::String(format!("Hello, {name}{bang}")))
}

fn add(args: Vec<Value>) -> Result<Value, String> {
    let mut args = args.into_iter();
    let a: u32 = args.next().ok_or("add: missing 'a'")?.try_into()?;
    let b: u32 = args.next().ok_or("add: missing 'b'")?.try_into()?;
    Ok(Value::U32(a.wrapping_add(b)))
}

crab_sdk::export_abi!(schema: SCHEMA, init: setup);
