//! crab-sdk: guest-side SDK for crabcraft wasm modules.
//!
//! Implements docs/WIRE.md exactly:
//! - section 1: the component-model value codec ([`codec`], [`Value`], [`Type`])
//! - section 2: the guest ABI (`crab_alloc` / `crab_schema` / `crab_invoke`),
//!   emitted into the final cdylib by [`export_abi!`].

pub mod abi;
pub mod codec;
#[cfg(all(feature = "mesh", target_arch = "wasm32"))]
pub mod mesh;
pub mod value;
pub mod vectors;

pub use abi::Registry;
#[cfg(all(feature = "mesh", target_arch = "wasm32"))]
pub use mesh::mesh_call;
pub use codec::{decode, decode_params, encode, encode_to_vec, Decoder};
pub use value::{Type, Value};
