//! WIRE.md section 2 optional import: the service mesh (`crabcraft.call`).
//!
//! Compiled only with the `mesh` cargo feature so the import is OPT-IN:
//! modules that never call [`mesh_call`] (e.g. `hello`) keep building
//! without an import of (module `"crabcraft"`, field `"call"`). Only crates
//! that enable the feature *and* call [`mesh_call`] link the import.

use crate::value::{Type, Value};

#[link(wasm_import_module = "crabcraft")]
extern "C" {
    /// `call(wl_ptr, wl_len, fn_ptr, fn_len, par_ptr, par_len) -> ptr` to a
    /// LENBUF reply the host wrote into guest memory via `crab_alloc`.
    #[link_name = "call"]
    fn crabcraft_call(
        wl_ptr: *const u8,
        wl_len: i32,
        fn_ptr: *const u8,
        fn_len: i32,
        par_ptr: *const u8,
        par_len: i32,
    ) -> *const u8;
}

/// Invoke `func` (a `<instance>#<function>` address) on the workload named
/// `workload`, passing section-1 encoded `params`. The host routes by NAME
/// through the gateway — placement is the mesh's problem, not ours.
///
/// Returns the encoded result value bytes on status 0, or the error string
/// on status 1 (also used for transport-level failures).
pub fn mesh_call(workload: &str, func: &str, params: &[u8]) -> Result<Vec<u8>, String> {
    let reply = unsafe {
        crabcraft_call(
            workload.as_ptr(),
            workload.len() as i32,
            func.as_ptr(),
            func.len() as i32,
            params.as_ptr(),
            params.len() as i32,
        )
    };
    if reply.is_null() {
        return Err("mesh: host returned null reply".into());
    }

    // LENBUF: 4-byte u32 LE length, then that many payload bytes.
    let len_bytes: [u8; 4] = unsafe { std::slice::from_raw_parts(reply, 4) }
        .try_into()
        .unwrap();
    let len = u32::from_le_bytes(len_bytes) as usize;
    let payload = unsafe { std::slice::from_raw_parts(reply.add(4), len) };

    // Payload: [status: 1 byte][body].
    match payload.split_first() {
        Some((0, body)) => Ok(body.to_vec()),
        Some((1, body)) => {
            // body = string (uleb len + utf8 message)
            let msg = match crate::codec::decode(&Type::String, body) {
                Ok(Value::String(s)) => s,
                _ => "mesh: malformed error string in reply".into(),
            };
            Err(msg)
        }
        Some((s, _)) => Err(format!("mesh: invalid reply status byte: {s}")),
        None => Err("mesh: empty reply payload".into()),
    }
}
