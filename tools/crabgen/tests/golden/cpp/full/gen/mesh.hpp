// crabcraft service mesh client: the optional `crabcraft.call` host import
// (WIRE.md section 2). crabgen emits this mesh.{hpp,cpp} pair only when the
// module's WIT world has imports — the import declaration lives in mesh.cpp,
// and a wasm import declared in a compiled+referenced TU is emitted into the
// module's import section whether or not it is ever called at runtime, which
// would make wasmcraft require the host to provide it. Import-free modules
// must therefore not compile mesh.cpp at all (same split as the Go
// template's mesh.go / mesh_wasm.go).
//
// Build compatibility: mesh.cpp compiles natively too — off-wasm the import
// cannot exist, so MeshCall returns a clear "import unavailable" error and
// ParseMeshReply stays unit-testable.

#pragma once

#include "crab.hpp"

namespace crab {

// MeshCall invokes `fn` (a `<instance>#<function>` address) on the workload
// named `workload` through the host mesh, passing WIRE-encoded params, and
// returns the encoded result value. A status-1 reply decodes into the error
// string. The host addresses services BY NAME; placement is its problem.
Res<std::vector<uint8_t>> MeshCall(std::string_view workload, std::string_view fn,
                                   const std::vector<uint8_t>& params);

// ParseMeshReply splits a [status][body] mesh reply: status 0 returns a copy
// of the body (the encoded result value); status 1 decodes the body as the
// error string, which must consume the body exactly; anything else is a
// protocol error. Exposed for host-side tests.
Res<std::vector<uint8_t>> ParseMeshReply(const uint8_t* payload, size_t len);

}  // namespace crab
