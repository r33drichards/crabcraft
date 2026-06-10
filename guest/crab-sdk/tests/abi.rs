//! ABI-level tests: exercise crab_invoke_impl / crab_schema_impl natively
//! with real pointers, checking LENBUF framing and the status-byte reply
//! format of WIRE.md section 2.

use crab_sdk::abi::{crab_alloc_impl, crab_invoke_impl, crab_schema_impl, Registry};
use crab_sdk::codec::{encode, encode_to_vec};
use crab_sdk::value::{Type, Value};

fn setup(r: &mut Registry) {
    r.register("t:t/t@0.1.0#add", vec![Type::U32, Type::U32], |args| {
        let mut it = args.into_iter();
        let a: u32 = it.next().unwrap().try_into()?;
        let b: u32 = it.next().unwrap().try_into()?;
        Ok(Value::U32(a.wrapping_add(b)))
    });
    r.register("t:t/t@0.1.0#fail", vec![], |_| Err("nope".to_string()));
}

/// Read the LENBUF a returned pointer refers to.
unsafe fn read_lenbuf(p: *const u8) -> Vec<u8> {
    let len = u32::from_le_bytes(std::slice::from_raw_parts(p, 4).try_into().unwrap());
    std::slice::from_raw_parts(p.add(4), len as usize).to_vec()
}

fn invoke(name: &str, params: &[Value]) -> Vec<u8> {
    let mut args = Vec::new();
    for p in params {
        encode(p, &mut args);
    }
    unsafe {
        let ptr = crab_invoke_impl(setup, name.as_ptr(), name.len(), args.as_ptr(), args.len());
        read_lenbuf(ptr)
    }
}

#[test]
fn invoke_ok() {
    let reply = invoke("t:t/t@0.1.0#add", &[Value::U32(2), Value::U32(3)]);
    assert_eq!(reply[0], 0, "status ok");
    assert_eq!(&reply[1..], encode_to_vec(&Value::U32(5)).as_slice());
}

#[test]
fn invoke_unknown_function() {
    let reply = invoke("t:t/t@0.1.0#nope", &[]);
    assert_eq!(reply[0], 1, "status error");
    // body = string: uleb len + utf8
    let msg = "unknown function: t:t/t@0.1.0#nope";
    let expected = encode_to_vec(&Value::String(msg.into()));
    assert_eq!(&reply[1..], expected.as_slice());
}

#[test]
fn invoke_handler_error() {
    let reply = invoke("t:t/t@0.1.0#fail", &[]);
    assert_eq!(reply[0], 1);
    assert_eq!(
        &reply[1..],
        encode_to_vec(&Value::String("nope".into())).as_slice()
    );
}

#[test]
fn invoke_bad_params() {
    // add expects two u32s; send one
    let reply = invoke("t:t/t@0.1.0#add", &[Value::U32(2)]);
    assert_eq!(reply[0], 1);
}

#[test]
fn schema_lenbuf() {
    let ptr = crab_schema_impl("{\"hi\":1}");
    let payload = unsafe { read_lenbuf(ptr) };
    assert_eq!(payload, b"{\"hi\":1}");
}

#[test]
fn alloc_returns_writable_memory() {
    let ptr = crab_alloc_impl(16);
    assert!(!ptr.is_null());
    unsafe {
        std::ptr::write_bytes(ptr, 0xab, 16);
        assert_eq!(*ptr, 0xab);
    }
}
