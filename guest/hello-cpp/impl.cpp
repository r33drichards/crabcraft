// impl.cpp — the application half of this guest: define the impl:: functions
// declared in gen/bindings.hpp. crabgen scaffolds this file ONCE and never
// overwrites it; `crabgen regen` prints any missing function signatures
// instead of editing it (a missing definition is also a LINK ERROR naming
// the symbol when build.sh links the module).
#include "gen/bindings.hpp"

namespace impl {

// greet handles crab:hello-cpp/greeter@0.1.0#greet.
// A non-empty .err is a function-level failure (status-1 reply).
crab::Res<std::string> greet(gen::GreetRequest req) {
  const char* bang = (req.excited.has_value() && *req.excited) ? "!!!" : "!";
  return crab::Res<std::string>{"Hello from C++, " + req.name + bang, {}};
}

// add handles crab:hello-cpp/greeter@0.1.0#add.
// add(a: u32, b: u32) -> u32 (unsigned arithmetic wraps naturally)
crab::Res<uint32_t> add(uint32_t a, uint32_t b) {
  return crab::Res<uint32_t>{a + b, {}};
}

}  // namespace impl
