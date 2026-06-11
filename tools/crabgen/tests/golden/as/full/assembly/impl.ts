// impl.ts — the application half of this guest: define the exported
// functions below (assembly/gen/bindings.ts imports them by name).
// crabgen scaffolds this file ONCE and never overwrites it; `crabgen
// regen` prints any missing function signatures instead of editing it.
import {
  Color,
  Everything,
  Perms,
  ResColor,
  ResEverything,
  ResF64,
  ResListOptionBool,
  ResPerms,
  ResResultU32Color,
  ResString,
  ResVoid,
  ResultU32Color,
  Shape,
} from "./gen/bindings";

// echoEverything handles crab:full/kitchen@0.1.0#echo-everything.
// A non-null .err is a function-level failure (status-1 reply).
export function echoEverything(e: Everything): ResEverything {
  return ResEverything.fail("unimplemented: echo-everything");
}

// pickColor handles crab:full/kitchen@0.1.0#pick-color.
// A non-null .err is a function-level failure (status-1 reply).
export function pickColor(c: Color): ResColor {
  return ResColor.fail("unimplemented: pick-color");
}

// setPerms handles crab:full/kitchen@0.1.0#set-perms.
// A non-null .err is a function-level failure (status-1 reply).
export function setPerms(p: Perms): ResPerms {
  return ResPerms.fail("unimplemented: set-perms");
}

// classify handles crab:full/kitchen@0.1.0#classify.
// A non-null .err is a function-level failure (status-1 reply).
export function classify(s_: Shape): ResString {
  return ResString.fail("unimplemented: classify");
}

// tryDivide handles crab:full/kitchen@0.1.0#try-divide.
// A non-null .err encodes as the WIT result err case (a normal status-0 reply).
export function tryDivide(num: f64, den: f64): ResF64 {
  return ResF64.fail("unimplemented: try-divide");
}

// maybeList handles crab:full/kitchen@0.1.0#maybe-list.
// A non-null .err is a function-level failure (status-1 reply).
export function maybeList(xs: Array<u16> | null): ResListOptionBool {
  return ResListOptionBool.fail("unimplemented: maybe-list");
}

// noResult handles crab:full/kitchen@0.1.0#no-result.
// A non-null .err is a function-level failure (status-1 reply).
export function noResult(x: u32): ResVoid {
  return ResVoid.fail("unimplemented: no-result");
}

// retry handles crab:full/kitchen@0.1.0#retry.
// A non-null .err is a function-level failure (status-1 reply).
export function retry(prev: ResultU32Color | null): ResResultU32Color {
  return ResResultU32Color.fail("unimplemented: retry");
}
