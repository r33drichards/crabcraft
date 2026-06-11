// impl.ts — the application half of this guest: define the exported
// functions below (assembly/gen/bindings.ts imports them by name).
// crabgen scaffolds this file ONCE and never overwrites it; `crabgen
// regen` prints any missing function signatures instead of editing it.
import {
  GreetRequest,
  ResString,
  ResU32,
} from "./gen/bindings";

// greet handles crab:hello-ts/greeter@0.1.0#greet.
// A non-null .err is a function-level failure (status-1 reply).
export function greet(req: GreetRequest): ResString {
  const bang = req.excited !== null && req.excited!.value ? "!!!" : "!";
  return ResString.ok("Hello from TS, " + req.name + bang);
}

// add handles crab:hello-ts/greeter@0.1.0#add.
// add(a: u32, b: u32) -> u32 (u32 arithmetic wraps naturally)
export function add(a: u32, b: u32): ResU32 {
  return ResU32.ok(a + b);
}
