//! The application half of this guest: implement `gen::HelloImpl` here.
//! crabgen scaffolds this file ONCE and never overwrites it; `crabgen regen`
//! prints any missing method signatures instead of editing it.

use crate::gen::{self, HelloImpl};

/// App implements gen::HelloImpl: one method per function exported by
/// crab:hello/greeter@0.1.0.
pub struct App;

impl HelloImpl for App {
    /// Handles crab:hello/greeter@0.1.0#greet.
    ///
    /// An Err return is a function-level failure (status-1 reply).
    fn greet(&self, req: gen::GreetRequest) -> Result<String, String> {
        let bang = if req.excited == Some(true) { "!!!" } else { "!" };
        Ok(format!("Hello, {}{bang}", req.name))
    }

    /// Handles crab:hello/greeter@0.1.0#add.
    ///
    /// An Err return is a function-level failure (status-1 reply).
    fn add(&self, a: u32, b: u32) -> Result<u32, String> {
        Ok(a.wrapping_add(b))
    }
}
