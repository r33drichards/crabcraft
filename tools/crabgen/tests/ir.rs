use std::path::{Path, PathBuf};

use crabgen::ir::Ty;
use crabgen::wit;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn full_fixture_loads_into_ir() {
    let m = wit::load(&fixture("full.wit")).expect("load full.wit");

    assert_eq!(m.package, "crab:full@0.1.0");
    assert_eq!(m.world, "full");

    assert_eq!(m.exports.len(), 1, "exactly one exported interface");
    let kitchen = &m.exports[0];
    assert_eq!(kitchen.instance, "crab:full/kitchen@0.1.0");
    assert_eq!(kitchen.funcs.len(), 8);

    assert_eq!(m.imports.len(), 1, "one imported interface for mesh stubs");
    let telemetry = &m.imports[0];
    assert_eq!(telemetry.instance, "crab:full/telemetry@0.1.0");
    assert_eq!(telemetry.funcs.len(), 2);

    // record field names + declaration order
    let everything = kitchen
        .types
        .iter()
        .find(|t| t.wit_name == "everything")
        .expect("named type `everything`");
    let Ty::Record(fields) = &everything.ty else {
        panic!("`everything` should be a record, got {:?}", everything.ty);
    };
    let names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "a-bool", "a-u8", "a-u16", "a-u32", "a-u64", "a-s8", "a-s16", "a-s32", "a-s64",
            "a-f32", "a-f64", "a-char", "a-string", "a-list", "a-opt", "a-tuple",
        ]
    );

    // variant case order + payloads
    let shape = kitchen
        .types
        .iter()
        .find(|t| t.wit_name == "shape")
        .expect("named type `shape`");
    let Ty::Variant(cases) = &shape.ty else {
        panic!("`shape` should be a variant, got {:?}", shape.ty);
    };
    let case_names: Vec<&str> = cases.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(case_names, ["point", "circle", "rect", "boxed"]);
    assert_eq!(cases[0].1, None);
    assert_eq!(cases[1].1, Some(Ty::F32));
    // a named type used as a payload is referenced, not inlined
    assert_eq!(cases[3].1, Some(Ty::Named("everything".into())));

    // named types are referenced at use sites
    let echo = kitchen
        .funcs
        .iter()
        .find(|f| f.wit_name == "echo-everything")
        .expect("func echo-everything");
    assert_eq!(
        echo.params,
        vec![("e".into(), Ty::Named("everything".into()))]
    );
    assert_eq!(echo.result, Some(Ty::Named("everything".into())));

    // function with no result
    let no_result = kitchen
        .funcs
        .iter()
        .find(|f| f.wit_name == "no-result")
        .expect("func no-result");
    assert_eq!(no_result.result, None);

    // result<f64, string>
    let try_divide = kitchen
        .funcs
        .iter()
        .find(|f| f.wit_name == "try-divide")
        .expect("func try-divide");
    assert_eq!(
        try_divide.result,
        Some(Ty::Result(
            Some(Box::new(Ty::F64)),
            Some(Box::new(Ty::String))
        ))
    );

    // anonymous compounds are inlined structurally, not Named
    let maybe_list = kitchen
        .funcs
        .iter()
        .find(|f| f.wit_name == "maybe-list")
        .expect("func maybe-list");
    assert_eq!(
        maybe_list.params,
        vec![(
            "xs".into(),
            Ty::Option(Box::new(Ty::List(Box::new(Ty::U16))))
        )]
    );
    assert_eq!(
        maybe_list.result,
        Some(Ty::List(Box::new(Ty::Option(Box::new(Ty::Bool)))))
    );

    // import side: result<_, string> lowers with an absent ok type
    let report = telemetry
        .funcs
        .iter()
        .find(|f| f.wit_name == "report")
        .expect("func report");
    assert_eq!(
        report.result,
        Some(Ty::Result(None, Some(Box::new(Ty::String))))
    );

    // schema_json is real resolved-WIT JSON
    let v: serde_json::Value =
        serde_json::from_str(&m.schema_json).expect("schema_json parses as JSON");
    assert!(
        v.get("worlds").is_some(),
        "schema_json has a \"worlds\" key"
    );
}

#[test]
fn cross_interface_use_is_unsupported() {
    let err =
        wit::load(&fixture("crossuse.wit")).expect_err("`use` across interfaces must be rejected");
    assert!(
        format!("{err:#}").contains("unsupported"),
        "error should say unsupported, got: {err:#}"
    );
}

#[test]
fn resources_are_unsupported() {
    let err = wit::load(&fixture("resourceful.wit")).expect_err("resources must be rejected");
    assert!(
        format!("{err:#}").contains("unsupported"),
        "error should say unsupported, got: {err:#}"
    );
}

// If wit/hello.json is ever regenerated with a newer wasm-tools and this fails,
// per plan Task 1.2 step 4 the round-trip invariant becomes the assertion and
// gen/schema.json the canonical form.
//
// NOTE: wit/hello.{wit,json} is a self-consistent test fixture for this
// schema-fidelity check only. The deployed source of truth for the hello
// guest is guest/hello/gen/schema.json (crabgen-managed, freshness-gated
// by `crabgen check`); test/e2e_sim.py serves schemas from there.
#[test]
fn schema_json_matches_wasm_tools_json() {
    let root = repo_root();
    let m = wit::load(&root.join("wit/hello.wit")).expect("load wit/hello.wit");
    let ours: serde_json::Value = serde_json::from_str(&m.schema_json).unwrap();
    let canonical: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("wit/hello.json")).expect("read wit/hello.json"),
    )
    .unwrap();
    assert_eq!(
        ours, canonical,
        "serialized Resolve must structurally match wasm-tools component wit --json"
    );
}
