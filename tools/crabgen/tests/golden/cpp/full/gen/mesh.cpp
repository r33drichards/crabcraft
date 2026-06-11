// Mesh client implementation: ParseMeshReply (target-independent) + the raw
// crabcraft.call import and MeshCall plumbing (wasm-only). See mesh.hpp for
// why this is a separate, conditionally-emitted file.

#include "mesh.hpp"

namespace crab {

Res<std::vector<uint8_t>> ParseMeshReply(const uint8_t* payload, size_t len) {
  using R = Res<std::vector<uint8_t>>;
  if (len == 0) return R::fail("mesh call: empty reply");
  uint8_t status = payload[0];
  if (status == 0) return R{std::vector<uint8_t>(payload + 1, payload + len), {}};
  if (status == 1) {
    Decoder d(payload + 1, len - 1);
    auto msg = d.String();
    if (!msg.ok()) return R::fail("mesh call: malformed error reply");
    if (d.Remaining() != 0)
      return R::fail("mesh call: malformed error reply (trailing bytes)");
    return R::fail(std::move(msg.val));
  }
  return R::fail("mesh call: invalid reply status");
}

}  // namespace crab

#if defined(__wasm__)

extern "C" {
// WIRE.md section 2 optional import (module "crabcraft", field "call"):
// returns a pointer to a LENBUF reply the host wrote into guest memory via
// crab_alloc.
__attribute__((import_module("crabcraft"), import_name("call"))) uint32_t
crabcraft_call(const void* wl_ptr, uint32_t wl_len, const void* fn_ptr,
               uint32_t fn_len, const void* par_ptr, uint32_t par_len);
}

namespace crab {

Res<std::vector<uint8_t>> MeshCall(std::string_view workload, std::string_view fn,
                                   const std::vector<uint8_t>& params) {
  uint32_t ptr = crabcraft_call(workload.data(), (uint32_t)workload.size(),
                                fn.data(), (uint32_t)fn.size(), params.data(),
                                (uint32_t)params.size());
  // Read the LENBUF ([u32 LE length][payload]); a null pointer reads as an
  // empty payload. Copy out what we need, then unpin the host-allocated
  // reply so its memory can be reclaimed.
  std::vector<uint8_t> payload;
  if (ptr != 0) {
    const uint8_t* p = reinterpret_cast<const uint8_t*>((uintptr_t)ptr);
    uint32_t n = (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 |
                 (uint32_t)p[3] << 24;
    payload.assign(p + 4, p + 4 + n);
    detail::Allocs().erase((uintptr_t)ptr);
  }
  return ParseMeshReply(payload.data(), payload.size());
}

}  // namespace crab

#else  // !defined(__wasm__)

namespace crab {

Res<std::vector<uint8_t>> MeshCall(std::string_view, std::string_view,
                                   const std::vector<uint8_t>&) {
  return Res<std::vector<uint8_t>>::fail(
      "crabcraft.call import unavailable: not running under a crabcraft host");
}

}  // namespace crab

#endif  // defined(__wasm__)
