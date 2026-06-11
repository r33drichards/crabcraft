// impl.cpp — the application half of this guest: define the impl:: functions
// declared in gen/bindings.hpp. crabgen scaffolds this file ONCE and never
// overwrites it; `crabgen regen` prints any missing function signatures
// instead of editing it (a missing definition is also a LINK ERROR naming
// the symbol when build.sh links the module).
#include "gen/bindings.hpp"

namespace impl {

// echo_everything handles crab:full/kitchen@0.1.0#echo-everything.
// A non-empty .err is a function-level failure (status-1 reply).
crab::Res<gen::Everything> echo_everything(gen::Everything e) {
  return crab::Res<gen::Everything>::fail("unimplemented: echo-everything");
}

// pick_color handles crab:full/kitchen@0.1.0#pick-color.
// A non-empty .err is a function-level failure (status-1 reply).
crab::Res<gen::Color> pick_color(gen::Color c) {
  return crab::Res<gen::Color>::fail("unimplemented: pick-color");
}

// set_perms handles crab:full/kitchen@0.1.0#set-perms.
// A non-empty .err is a function-level failure (status-1 reply).
crab::Res<gen::Perms> set_perms(gen::Perms p) {
  return crab::Res<gen::Perms>::fail("unimplemented: set-perms");
}

// classify handles crab:full/kitchen@0.1.0#classify.
// A non-empty .err is a function-level failure (status-1 reply).
crab::Res<std::string> classify(gen::Shape s) {
  return crab::Res<std::string>::fail("unimplemented: classify");
}

// try_divide handles crab:full/kitchen@0.1.0#try-divide.
// A non-empty .err encodes as the WIT result err case (a normal status-0 reply).
crab::Res<double> try_divide(double num, double den) {
  return crab::Res<double>::fail("unimplemented: try-divide");
}

// maybe_list handles crab:full/kitchen@0.1.0#maybe-list.
// A non-empty .err is a function-level failure (status-1 reply).
crab::Res<std::vector<std::optional<bool>>> maybe_list(std::optional<std::vector<uint16_t>> xs) {
  return crab::Res<std::vector<std::optional<bool>>>::fail("unimplemented: maybe-list");
}

// no_result handles crab:full/kitchen@0.1.0#no-result.
// A non-empty .err is a function-level failure (status-1 reply).
crab::Res<std::monostate> no_result(uint32_t x) {
  return crab::Res<std::monostate>::fail("unimplemented: no-result");
}

// retry handles crab:full/kitchen@0.1.0#retry.
// A non-empty .err is a function-level failure (status-1 reply).
crab::Res<gen::Result<uint32_t, gen::Color>> retry(std::optional<gen::Result<uint32_t, gen::Color>> prev) {
  return crab::Res<gen::Result<uint32_t, gen::Color>>::fail("unimplemented: retry");
}

}  // namespace impl
