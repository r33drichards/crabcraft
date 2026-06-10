//! crabcraft caller module: implements crab:caller/relay@0.1.0
//! (wit/caller.wit). A wasm32-wasip1 reactor cdylib that exercises the
//! WIRE.md section 2 mesh import: greet-via(target, name) encodes a
//! greet-request and routes `crab:hello/greeter@0.1.0#greet` to the
//! workload named `target` via `crabcraft.call`.

use crab_sdk::{encode_to_vec, mesh_call, Registry, Type, Value};

/// Resolved-WIT JSON for this module, served verbatim via crab_schema.
const SCHEMA: &str = include_str!("../../../wit/caller.json");

const INSTANCE: &str = "crab:caller/relay@0.1.0";

/// Function address on the TARGET workload (the hello interface).
const GREET_FN: &str = "crab:hello/greeter@0.1.0#greet";

fn setup(r: &mut Registry) {
    r.register(
        &format!("{INSTANCE}#greet-via"),
        vec![Type::String, Type::String],
        greet_via,
    );
}

fn greet_via(args: Vec<Value>) -> Result<Value, String> {
    let mut args = args.into_iter();
    let target: String = args.next().ok_or("greet-via: missing 'target'")?.try_into()?;
    let name: String = args.next().ok_or("greet-via: missing 'name'")?.try_into()?;

    // greet's single param: greet-request record { name: string,
    // excited: option<bool> } — excited = none, encoded per section 1.
    let params = encode_to_vec(&Value::Record(vec![Value::String(name), Value::none()]));

    let reply = mesh_call(&target, GREET_FN, &params)?;

    // greet returns a string; decode the result bytes.
    let Value::String(greeting) = crab_sdk::decode(&Type::String, &reply)? else {
        return Err("greet-via: target returned a non-string result".into());
    };
    Ok(Value::String(format!("via mesh: {greeting}")))
}

crab_sdk::export_abi!(schema: SCHEMA, init: setup);
