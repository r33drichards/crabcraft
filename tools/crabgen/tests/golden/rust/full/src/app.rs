//! The application half of this guest: implement `gen::FullImpl` here.
//! crabgen scaffolds this file ONCE and never overwrites it; `crabgen regen`
//! prints any missing method signatures instead of editing it.

use crate::gen::{self, FullImpl};

/// App implements gen::FullImpl: one method per function exported by
/// crab:full/kitchen@0.1.0.
pub struct App;

impl FullImpl for App {
    /// Handles crab:full/kitchen@0.1.0#echo-everything.
    ///
    /// An Err return is a function-level failure (status-1 reply).
    fn echo_everything(&self, e: gen::Everything) -> Result<gen::Everything, String> {
        let _ = e;
        Err("unimplemented: echo-everything".into())
    }

    /// Handles crab:full/kitchen@0.1.0#pick-color.
    ///
    /// An Err return is a function-level failure (status-1 reply).
    fn pick_color(&self, c: gen::Color) -> Result<gen::Color, String> {
        let _ = c;
        Err("unimplemented: pick-color".into())
    }

    /// Handles crab:full/kitchen@0.1.0#set-perms.
    ///
    /// An Err return is a function-level failure (status-1 reply).
    fn set_perms(&self, p: gen::Perms) -> Result<gen::Perms, String> {
        let _ = p;
        Err("unimplemented: set-perms".into())
    }

    /// Handles crab:full/kitchen@0.1.0#classify.
    ///
    /// An Err return is a function-level failure (status-1 reply).
    fn classify(&self, s: gen::Shape) -> Result<String, String> {
        let _ = s;
        Err("unimplemented: classify".into())
    }

    /// Handles crab:full/kitchen@0.1.0#try-divide.
    ///
    /// An Err return encodes as the WIT result err case (a normal status-0 reply).
    fn try_divide(&self, num: f64, den: f64) -> Result<f64, String> {
        let _ = (num, den);
        Err("unimplemented: try-divide".into())
    }

    /// Handles crab:full/kitchen@0.1.0#maybe-list.
    ///
    /// An Err return is a function-level failure (status-1 reply).
    fn maybe_list(&self, xs: Option<Vec<u16>>) -> Result<Vec<Option<bool>>, String> {
        let _ = xs;
        Err("unimplemented: maybe-list".into())
    }

    /// Handles crab:full/kitchen@0.1.0#no-result.
    ///
    /// An Err return is a function-level failure (status-1 reply).
    fn no_result(&self, x: u32) -> Result<(), String> {
        let _ = x;
        Err("unimplemented: no-result".into())
    }

    /// Handles crab:full/kitchen@0.1.0#retry.
    ///
    /// The Ok value is the WIT result, returned whole; an Err return is a function-level failure (status-1 reply).
    fn retry(&self, prev: Option<Result<u32, gen::Color>>) -> Result<Result<u32, gen::Color>, String> {
        let _ = prev;
        Err("unimplemented: retry".into())
    }
}
