//! WIRE.md section 2 guest ABI, implemented once here.
//!
//! The application crate (a wasm32-wasip1 cdylib reactor) calls
//! [`export_abi!`] exactly once, supplying its schema JSON and a registry
//! init function; the macro emits the three required `#[no_mangle]` exports
//! (`crab_alloc`, `crab_schema`, `crab_invoke`) which forward here.
//!
//! Returned buffers are LENBUF (4-byte u32 LE length + payload) and live in
//! a thread_local Vec, so they stay valid until the next call.

use std::cell::RefCell;
use std::sync::OnceLock;

use crate::codec::{decode_params, encode, uleb_encode};
use crate::value::{Type, Value};

/// A handler takes the decoded params and returns the result value
/// (use [`Value::unit()`] for functions with no result) or an error string.
pub type Handler = fn(Vec<Value>) -> Result<Value, String>;

/// Maps fully-qualified function addresses (`<instance>#<function>`,
/// e.g. `crab:hello/greeter@0.1.0#greet`) to param types + handler.
#[derive(Default)]
pub struct Registry {
    funcs: Vec<(String, Vec<Type>, Handler)>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `handler` for `instance_func` (e.g.
    /// `"crab:hello/greeter@0.1.0#add"`). `params` are the WIT parameter
    /// types in declaration order — the wire is untagged, so decoding the
    /// incoming params buffer requires them.
    pub fn register(&mut self, instance_func: &str, params: Vec<Type>, handler: Handler) {
        self.funcs
            .push((instance_func.to_string(), params, handler));
    }

    fn lookup(&self, name: &str) -> Option<&(String, Vec<Type>, Handler)> {
        self.funcs.iter().find(|(n, _, _)| n == name)
    }
}

static REGISTRY: OnceLock<Registry> = OnceLock::new();

thread_local! {
    /// Reply buffer: valid until the next crab_invoke / crab_schema call.
    static REPLY: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Build the LENBUF reply in the static buffer and return its pointer.
fn lenbuf(payload: &[u8]) -> *const u8 {
    REPLY.with(|r| {
        let mut buf = r.borrow_mut();
        buf.clear();
        buf.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        buf.extend_from_slice(payload);
        buf.as_ptr()
    })
}

/// `crab_alloc(len) -> ptr`: plain malloc-style allocation the host uses to
/// place the name/params buffers before calling `crab_invoke`.
pub fn crab_alloc_impl(len: usize) -> *mut u8 {
    let size = len.max(1);
    let layout = std::alloc::Layout::from_size_align(size, 8).unwrap();
    unsafe { std::alloc::alloc(layout) }
}

/// `crab_schema() -> ptr`: LENBUF wrapping the module's resolved-WIT JSON.
pub fn crab_schema_impl(schema: &str) -> *const u8 {
    lenbuf(schema.as_bytes())
}

fn reply_ok(result_bytes: &[u8]) -> *const u8 {
    let mut payload = Vec::with_capacity(1 + result_bytes.len());
    payload.push(0u8);
    payload.extend_from_slice(result_bytes);
    lenbuf(&payload)
}

fn reply_err(msg: &str) -> *const u8 {
    let mut payload = Vec::with_capacity(2 + msg.len());
    payload.push(1u8);
    uleb_encode(msg.len() as u64, &mut payload);
    payload.extend_from_slice(msg.as_bytes());
    lenbuf(&payload)
}

/// `crab_invoke(name_ptr, name_len, arg_ptr, arg_len) -> ptr`.
///
/// # Safety
/// `name_ptr..name_ptr+name_len` and `arg_ptr..arg_ptr+arg_len` must be
/// valid readable ranges in linear memory (the host wrote them via
/// `crab_alloc`).
pub unsafe fn crab_invoke_impl(
    init: fn(&mut Registry),
    name_ptr: *const u8,
    name_len: usize,
    arg_ptr: *const u8,
    arg_len: usize,
) -> *const u8 {
    let registry = REGISTRY.get_or_init(|| {
        let mut r = Registry::new();
        init(&mut r);
        r
    });

    let name_bytes = std::slice::from_raw_parts(name_ptr, name_len);
    let name = match std::str::from_utf8(name_bytes) {
        Ok(s) => s,
        Err(_) => return reply_err("function name is not valid utf-8"),
    };

    let Some((_, param_types, handler)) = registry.lookup(name) else {
        return reply_err(&format!("unknown function: {name}"));
    };

    let args = std::slice::from_raw_parts(arg_ptr, arg_len);
    let params = match decode_params(param_types, args) {
        Ok(p) => p,
        Err(e) => return reply_err(&format!("{name}: bad params: {e}")),
    };

    match handler(params) {
        Ok(result) => {
            let mut bytes = Vec::new();
            encode(&result, &mut bytes);
            reply_ok(&bytes)
        }
        Err(e) => reply_err(&e),
    }
}

/// Emit the three WIRE.md section 2 exports. Call once in the app crate:
///
/// ```ignore
/// crab_sdk::export_abi!(schema: include_str!("../wit.json"), init: setup);
///
/// fn setup(r: &mut crab_sdk::Registry) {
///     r.register("pkg:iface/inst@0.1.0#fn", vec![Type::U32], my_handler);
/// }
/// ```
#[macro_export]
macro_rules! export_abi {
    (schema: $schema:expr, init: $init:expr) => {
        #[no_mangle]
        pub extern "C" fn crab_alloc(len: i32) -> i32 {
            $crate::abi::crab_alloc_impl(len.max(0) as u32 as usize) as i32
        }

        #[no_mangle]
        pub extern "C" fn crab_schema() -> i32 {
            $crate::abi::crab_schema_impl($schema) as i32
        }

        #[no_mangle]
        pub extern "C" fn crab_invoke(
            name_ptr: i32,
            name_len: i32,
            arg_ptr: i32,
            arg_len: i32,
        ) -> i32 {
            unsafe {
                $crate::abi::crab_invoke_impl(
                    $init,
                    name_ptr as u32 as usize as *const u8,
                    name_len as u32 as usize,
                    arg_ptr as u32 as usize as *const u8,
                    arg_len as u32 as usize,
                ) as i32
            }
        }
    };
}
