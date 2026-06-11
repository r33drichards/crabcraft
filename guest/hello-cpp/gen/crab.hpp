// crabcraft C++ runtime: WIRE.md section-1 value codec + section-2 guest ABI.
//
// This file is a crabgen TEMPLATE shared by every generated C++ guest; it
// contains no per-WIT content. Generated bindings (gen/bindings.cpp) call
// the Encode*/Decoder primitives in straight-line code — no value trees, no
// RTTI — define SchemaJson(), and register their dispatch functions with
// RegisterHandler at static-init time.
//
// Error / calling convention (no exceptions; built with -fno-exceptions):
// every fallible decode returns Res<T> { val, err } where ok() == err.empty()
// (all runtime error messages are non-empty, so an empty err always means
// success). Generated code is straight-line:
//
//   auto r0 = d.U32();
//   if (!r0.ok()) return crab::Res<std::vector<uint8_t>>::fail(std::move(r0.err));
//   use(r0.val);
//
// The one fallible operation with no value, Decoder::Finish, returns a bare
// std::string: empty = success, otherwise the error message.
//
// Error message TEXTS mirror guest/crab-sdk/src/codec.rs and the Go template
// (templates/go/runtime.go) exactly — cross-language consistency is part of
// the conformance contract ("uleb overflow", "invalid bool byte: 2",
// "N trailing byte(s) after params", ...).
//
// Build compatibility: this header and crab.cpp compile both natively (host
// unit tests, the vectors conformance binary) and for wasm32-wasi. The
// wasm-only piece — the exported crab_alloc/crab_schema/crab_invoke ABI at
// the bottom of crab.cpp — is guarded by `#if defined(__wasm__)`; the codec
// and the handler registry are target-independent. The OPTIONAL mesh import
// (crabcraft.call) lives in mesh.{hpp,cpp}, a separate pair crabgen emits
// only when the module's WIT world has imports: an import declared in a
// compiled+referenced TU lands in the wasm import section whether or not it
// is ever called, and wasmcraft would then require the host to provide it —
// keeping it in a separate file is what keeps import-free modules
// import-free (same split as the Go template's mesh.go / mesh_wasm.go).
//
// Validation mirrors guest/crab-sdk/src/codec.rs exactly:
//   - ULEB128: at most ceil(bits/7) bytes per width; on the last permitted
//     byte any payload bits above the width must be 0 ("uleb overflow"); a
//     continuation bit on the last permitted byte is "uleb too long".
//     Non-canonical zero-padded encodings within those limits are accepted.
//   - SLEB128: same max byte counts; range-checked against the width; the
//     10th byte of an s64 must be 0x00 or 0x7f.
//   - bool / option / result discriminant bytes must be exactly 0 or 1.
//   - variant/enum case index: u32 ULEB, must be < the number of cases.
//   - char must be a unicode scalar value (<= 0x10FFFF, not a surrogate).
//   - string bytes must be valid UTF-8.
//   - flags: ceil(n/8) bytes; bits above flag n-1 in the last byte must be 0.

#pragma once

#include <cstdint>
#include <map>
#include <string>
#include <string_view>
#include <vector>

namespace crab {

// Res<T>: the no-exceptions error channel. Aggregate on purpose — generated
// code and the runtime use `return {value, {}}` for success and
// `Res<T>::fail(msg)` for errors. T must be default-constructible (emitter
// contract; matters for generated std::variant types, whose first
// alternative must therefore be default-constructible too).
template <class T>
struct Res {
  T val{};
  std::string err;  // empty = success; all runtime errors are non-empty
  bool ok() const { return err.empty(); }
  static Res fail(std::string e) {
    Res r;
    r.err = std::move(e);
    return r;
  }
};

// ---------------------------------------------------------------------------
// WIRE.md section 1: encode (append-style primitives over std::vector)
// ---------------------------------------------------------------------------

// EncodeUleb appends the unsigned LEB128 encoding of v.
void EncodeUleb(std::vector<uint8_t>& out, uint64_t v);
// EncodeSleb appends the signed LEB128 encoding of v.
void EncodeSleb(std::vector<uint8_t>& out, int64_t v);
// EncodeBool appends 1 byte: 0 or 1.
void EncodeBool(std::vector<uint8_t>& out, bool v);
// EncodeU8/U16/U32/U64 append uleb(v).
void EncodeU8(std::vector<uint8_t>& out, uint8_t v);
void EncodeU16(std::vector<uint8_t>& out, uint16_t v);
void EncodeU32(std::vector<uint8_t>& out, uint32_t v);
void EncodeU64(std::vector<uint8_t>& out, uint64_t v);
// EncodeS8/S16/S32/S64 append sleb(v).
void EncodeS8(std::vector<uint8_t>& out, int8_t v);
void EncodeS16(std::vector<uint8_t>& out, int16_t v);
void EncodeS32(std::vector<uint8_t>& out, int32_t v);
void EncodeS64(std::vector<uint8_t>& out, int64_t v);
// EncodeF32/F64 append 4/8 bytes IEEE-754 LE.
void EncodeF32(std::vector<uint8_t>& out, float v);
void EncodeF64(std::vector<uint8_t>& out, double v);
// EncodeChar appends uleb(unicode scalar value).
void EncodeChar(std::vector<uint8_t>& out, uint32_t scalar);
// EncodeString appends uleb(byte length) + UTF-8 bytes.
void EncodeString(std::vector<uint8_t>& out, std::string_view s);
// EncodeListLen appends uleb(count); the caller then appends each element.
void EncodeListLen(std::vector<uint8_t>& out, size_t n);
// EncodeCase appends uleb(case index) for variants and enums; for a variant
// the caller then appends the payload if the case has one. Records and
// tuples have no header at all: just encode each member in order.
void EncodeCase(std::vector<uint8_t>& out, uint32_t c);
// EncodeOptionTag appends the option discriminant (0 = none, 1 = some); the
// caller then appends the inner value when present.
void EncodeOptionTag(std::vector<uint8_t>& out, bool present);
// EncodeResultTag appends the result discriminant (0 = ok, 1 = err); the
// caller then appends the payload if that side has a type.
void EncodeResultTag(std::vector<uint8_t>& out, bool is_err);
// EncodeFlags appends ceil(bits.size()/8) bytes, bit i = flag i, LE byte
// order.
void EncodeFlags(std::vector<uint8_t>& out, const std::vector<bool>& bits);

// ---------------------------------------------------------------------------
// WIRE.md section 1: decode
// ---------------------------------------------------------------------------

// Decoder is a cursor over an encoded buffer (not owned; the buffer must
// outlive the Decoder).
class Decoder {
 public:
  Decoder(const uint8_t* buf, size_t len) : buf_(buf), len_(len), pos_(0) {}
  explicit Decoder(std::string_view buf)
      : Decoder(reinterpret_cast<const uint8_t*>(buf.data()), buf.size()) {}

  // Remaining reports the number of undecoded bytes.
  size_t Remaining() const { return len_ - pos_; }

  // Finish returns "" when the whole buffer was consumed, otherwise
  // "N trailing byte(s) after <what>". Params decoding (and single-value
  // decoding) must always end with a Finish check.
  std::string Finish(const char* what) const;

  // Bool decodes a strict 0/1 byte.
  Res<bool> Bool();
  // U8..U64 decode a uleb capped at the type's bit width.
  Res<uint8_t> U8();
  Res<uint16_t> U16();
  Res<uint32_t> U32();
  Res<uint64_t> U64();
  // S8..S64 decode a sleb range-checked to the type's bit width.
  Res<int8_t> S8();
  Res<int16_t> S16();
  Res<int32_t> S32();
  Res<int64_t> S64();
  // F32/F64 decode 4/8 bytes IEEE-754 LE.
  Res<float> F32();
  Res<double> F64();
  // Char decodes a uleb scalar (21-bit cap) and validates it is a unicode
  // scalar value (<= 0x10FFFF and not a surrogate).
  Res<uint32_t> Char();
  // String decodes uleb(byte length) + UTF-8 bytes, validating the UTF-8.
  Res<std::string> String();
  // ListLen decodes uleb(count); the caller then decodes each element. The
  // count is attacker-controlled, so do not pre-allocate from it blindly —
  // let element decoding fail naturally on short buffers.
  Res<uint32_t> ListLen();
  // OptionTag decodes the option discriminant (false = none, true = some);
  // the caller then decodes the inner value when present.
  Res<bool> OptionTag();
  // ResultTag decodes the result discriminant (false = ok, true = err); the
  // caller then decodes the payload if that side has a type.
  Res<bool> ResultTag();
  // VariantCase/EnumCase decode a u32-uleb case index and bounds-check it;
  // for a variant the caller then decodes the payload if the case has one.
  Res<uint32_t> VariantCase(uint32_t num_cases);
  Res<uint32_t> EnumCase(uint32_t num_cases);
  // Flags decodes ceil(n/8) bytes into one bool per flag (bit i = flag i,
  // LE byte order); bits above flag n-1 in the last byte must be zero.
  Res<std::vector<bool>> Flags(size_t n);

  // Raw uleb/sleb with explicit bit caps (the typed helpers above use these).
  Res<uint64_t> Uleb(unsigned bits);
  Res<int64_t> Sleb(unsigned bits);

 private:
  Res<uint8_t> Byte();
  Res<const uint8_t*> Bytes(size_t n);
  Res<bool> PrefixBit(const char* what);

  const uint8_t* buf_;
  size_t len_;
  size_t pos_;
};

// ---------------------------------------------------------------------------
// WIRE.md section 2: guest ABI glue
// (the exported crab_alloc/crab_schema/crab_invoke live in crab.cpp under
//  #if defined(__wasm__); the registry below is target-independent so
//  generated bindings compile natively for unit tests too)
// ---------------------------------------------------------------------------

// Handler decodes its params from the Decoder (including the trailing-bytes
// Finish check) and returns the encoded result value.
using Handler = Res<std::vector<uint8_t>> (*)(Decoder&);

// Handlers maps function addresses (`<instance>#<function>`) to handlers.
// It is a function-local static so registration from any TU's static
// initializers is safe regardless of static-init order across TUs.
std::map<std::string, Handler>& Handlers();

// RegisterHandler is what generated bindings call at static-init time:
//   static bool reg = crab::RegisterHandler("crab:hello/greeter@0.1.0#greet",
//                                           greet_dispatch);
// (the bool return exists only to allow that initializer form)
bool RegisterHandler(const char* name, Handler h);

// SchemaJson is DEFINED by the generated bindings (gen/bindings.cpp): the
// resolved-WIT JSON this module serves from crab_schema.
std::string_view SchemaJson();

namespace detail {
// Allocs pins buffers whose addresses were handed to the host (crab_alloc
// allocations, mesh replies) until explicitly unpinned; keyed by address.
// crab_invoke unpins its name/args buffers when it returns; MeshCall unpins
// the host reply after copying it out.
std::map<uintptr_t, std::vector<uint8_t>>& Allocs();
}  // namespace detail

}  // namespace crab
