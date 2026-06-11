// NATIVE WIRE-conformance driver for the crabgen C++ runtime template
// (tools/crabgen/templates/cpp: crab.{hpp,cpp} + mesh.{hpp,cpp}).
//
// Built and run by tools/crabgen/tests/cpp_vectors.rs, which generates
// vectors.inc from wit/vectors.json with serde_json — a table of
// straight-line encode / decode+re-encode lambdas, so no JSON parser lives
// here and the generated code exercises the exact calling convention the
// Task-4.2 bindings emitter emits. For each vector we assert:
//   1. the encode lambda's bytes == the expected hex,
//   2. decoding consumes the whole buffer (Finish("value") empty),
//   3. the immediate re-encode of every decoded value == the hex,
//   4. top-level scalar values compare equal (checks generated into the
//      lambda, decimal-string convention for big u64/s64).
// Then the edge cases mirrored from templates/go/runtime_test.go (error
// message TEXTS asserted exactly — cross-language consistency) and the mesh
// reply-parser tests from mesh_test.go.
//
// This binary is compiled with plain `zig c++` (no -target wasm32-wasi):
// the templates' ABI/export sections are #if defined(__wasm__)-guarded, so
// the codec compiles and runs anywhere.

#include "crab.hpp"
#include "mesh.hpp"

#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>

struct Vec {
  const char* desc;
  const char* hex;
  void (*encode)(std::vector<uint8_t>&);
  std::string (*reenc)(crab::Decoder&, std::vector<uint8_t>&);
};

#include "vectors.inc"  // defines VECTORS[] + NVECTORS

static int FAILURES = 0;

static void fail(const std::string& what, const std::string& detail) {
  std::printf("FAIL %s: %s\n", what.c_str(), detail.c_str());
  FAILURES++;
}

static int hexNib(char c) {
  if (c >= '0' && c <= '9') return c - '0';
  if (c >= 'a' && c <= 'f') return c - 'a' + 10;
  if (c >= 'A' && c <= 'F') return c - 'A' + 10;
  return -1;
}

static std::vector<uint8_t> hexDecode(const char* s) {
  std::vector<uint8_t> out;
  for (size_t i = 0; s[i] && s[i + 1]; i += 2)
    out.push_back((uint8_t)((hexNib(s[i]) << 4) | hexNib(s[i + 1])));
  return out;
}

static std::string toHex(const std::vector<uint8_t>& b) {
  static const char* digits = "0123456789abcdef";
  std::string s;
  for (uint8_t c : b) {
    s.push_back(digits[c >> 4]);
    s.push_back(digits[c & 0xf]);
  }
  return s;
}

// expectExactErr: err must be present and TEXT-identical (the WIRE error
// strings are part of the cross-language contract).
static void expectExactErr(const char* what, const std::string& err, const char* want) {
  if (err.empty()) {
    fail(what, std::string("expected error \"") + want + "\", got success");
  } else if (err != want) {
    fail(what, "error text \"" + err + "\", want \"" + want + "\"");
  }
}

// ---------------------------------------------------------------------------
// shared vectors (the generated table)
// ---------------------------------------------------------------------------

static void runVectors() {
  for (size_t i = 0; i < NVECTORS; i++) {
    const Vec& v = VECTORS[i];
    std::vector<uint8_t> want = hexDecode(v.hex);

    // 1. JSON value -> bytes must equal hex.
    std::vector<uint8_t> enc;
    v.encode(enc);
    if (enc != want) {
      fail(v.desc, "encode: got " + toHex(enc) + ", want " + v.hex);
    }

    // 2. hex -> value must consume the whole buffer (+ scalar equality)...
    crab::Decoder d(want.data(), want.size());
    std::vector<uint8_t> re;
    std::string err = v.reenc(d, re);
    if (!err.empty()) {
      fail(v.desc, "decode: " + err);
      continue;
    }
    std::string fin = d.Finish("value");
    if (!fin.empty()) {
      fail(v.desc, "decode: " + fin);
      continue;
    }

    // 3. ...and re-encode byte-identically.
    if (re != want) {
      fail(v.desc, "re-encode: got " + toHex(re) + ", want " + v.hex);
    }
  }
}

// ---------------------------------------------------------------------------
// edge cases beyond the shared vectors (mirror templates/go/runtime_test.go)
// ---------------------------------------------------------------------------

static void testSlebSignExtension() {
  // s8 -1 encodes as a single 0x7f byte.
  std::vector<uint8_t> enc;
  crab::EncodeS8(enc, -1);
  if (toHex(enc) != "7f") fail("EncodeS8(-1)", "got " + toHex(enc) + ", want 7f");
  {
    auto b = hexDecode("7f");
    crab::Decoder d(b.data(), b.size());
    auto r = d.S8();
    if (!r.ok() || r.val != -1) fail("S8(7f)", "want -1; err=" + r.err);
  }
  // s64 min round-trips (10-byte sleb ending 0x7f).
  {
    std::vector<uint8_t> e2;
    crab::EncodeS64(e2, INT64_MIN);
    crab::Decoder d(e2.data(), e2.size());
    auto r = d.S64();
    if (!r.ok() || r.val != INT64_MIN || d.Remaining() != 0)
      fail("s64 min round-trip", "enc=" + toHex(e2) + " err=" + r.err);
  }
  // 10th byte of an s64 may only be 0x00 or 0x7f.
  {
    auto b = hexDecode("80808080808080808001");
    crab::Decoder d(b.data(), b.size());
    expectExactErr("s64 invalid 10th byte", d.S64().err, "sleb overflow");
  }
  // s8 range check: 128 = 0x80 0x01 as sleb is out of range.
  {
    auto b = hexDecode("8001");
    crab::Decoder d(b.data(), b.size());
    expectExactErr("s8 = 128", d.S8().err, "sleb overflow");
  }
}

static void testUlebOverflowBits() {
  // u8 max is 2 bytes; payload bits above bit 7 on byte 2 must be zero.
  {
    auto b = hexDecode("ff03");  // would be 511
    crab::Decoder d(b.data(), b.size());
    expectExactErr("u8 = 511", d.U8().err, "uleb overflow");
  }
  // Continuation bit on the last permitted byte: too long.
  {
    auto b = hexDecode("ff8100");
    crab::Decoder d(b.data(), b.size());
    expectExactErr("3-byte uleb for u8", d.U8().err, "uleb too long");
  }
  // Non-canonical zero padding is accepted: 0x87 0x00 decodes to 7.
  {
    auto b = hexDecode("8700");
    crab::Decoder d(b.data(), b.size());
    auto r = d.U8();
    if (!r.ok() || r.val != 7) fail("non-canonical uleb 8700", "want 7; err=" + r.err);
  }
  // u64 10th byte may only contribute bit 63: 0x02 there overflows.
  {
    auto b = hexDecode("ffffffffffffffffff02");
    crab::Decoder d(b.data(), b.size());
    expectExactErr("u64 with bit 64 set", d.U64().err, "uleb overflow");
  }
  // u64 max via the decimal-string convention round-trips.
  {
    std::vector<uint8_t> enc;
    crab::EncodeU64(enc, 18446744073709551615ULL);
    if (toHex(enc) != "ffffffffffffffffff01")
      fail("EncodeU64(max)", "got " + toHex(enc));
    crab::Decoder d(enc.data(), enc.size());
    auto r = d.U64();
    if (!r.ok() || r.val != 18446744073709551615ULL)
      fail("u64 max round-trip", "err=" + r.err);
  }
}

static void testCharValidation() {
  // Surrogate U+D800 (uleb 0x80 0xb0 0x03) is not a unicode scalar value.
  {
    auto b = hexDecode("80b003");
    crab::Decoder d(b.data(), b.size());
    expectExactErr("char U+D800", d.Char().err, "invalid char scalar: 55296");
  }
  // Above U+10FFFF.
  {
    std::vector<uint8_t> enc;
    crab::EncodeU32(enc, 0x110000);
    crab::Decoder d(enc.data(), enc.size());
    expectExactErr("char U+110000", d.Char().err, "invalid char scalar: 1114112");
  }
  // Max scalar U+10FFFF is fine.
  {
    std::vector<uint8_t> enc;
    crab::EncodeU32(enc, 0x10FFFF);
    crab::Decoder d(enc.data(), enc.size());
    auto r = d.Char();
    if (!r.ok() || r.val != 0x10FFFF) fail("char U+10FFFF", "err=" + r.err);
  }
}

static void testStrictBytes() {
  // bool / option / result discriminants must be exactly 0 or 1.
  {
    auto b = hexDecode("02");
    crab::Decoder d(b.data(), b.size());
    expectExactErr("bool byte 2", d.Bool().err, "invalid bool byte: 2");
  }
  {
    auto b = hexDecode("02");
    crab::Decoder d(b.data(), b.size());
    expectExactErr("option byte 2", d.OptionTag().err, "invalid option byte: 2");
  }
  {
    auto b = hexDecode("ff");
    crab::Decoder d(b.data(), b.size());
    expectExactErr("result byte 255", d.ResultTag().err, "invalid result byte: 255");
  }
  // invalid utf-8 in string.
  {
    auto b = hexDecode("02fffe");
    crab::Decoder d(b.data(), b.size());
    expectExactErr("invalid utf-8", d.String().err, "invalid utf-8 in string");
  }
  // string length past end of buffer.
  {
    auto b = hexDecode("056869");  // says 5 bytes, has 2
    crab::Decoder d(b.data(), b.size());
    expectExactErr("truncated string", d.String().err, "unexpected end of buffer");
  }
}

static void testResultNoPayloadTypes() {
  // result (no ok/err types): a bare status byte.
  auto b = hexDecode("00");
  crab::Decoder d(b.data(), b.size());
  auto r = d.ResultTag();
  if (!r.ok()) {
    fail("result no payload", "decode: " + r.err);
    return;
  }
  std::string fin = d.Finish("value");
  if (!fin.empty()) {
    fail("result no payload", "trailing: " + fin);
    return;
  }
  std::vector<uint8_t> re;
  crab::EncodeResultTag(re, r.val);
  if (toHex(re) != "00") fail("result no payload", "re-encode " + toHex(re));
}

static void testFlagsValidation() {
  // 10 flags = 2 bytes; bits 10..15 of byte 2 must be zero.
  {
    auto b = hexDecode("0004");  // bit 10 set
    crab::Decoder d(b.data(), b.size());
    expectExactErr("flags unused bit", d.Flags(10).err, "flags: unused high bits set");
  }
  // Exactly 8 flags: a full byte, no unused-bit check, bit 7 = 0x80.
  {
    auto b = hexDecode("80");
    crab::Decoder d(b.data(), b.size());
    auto r = d.Flags(8);
    if (!r.ok() || r.val.size() != 8 || !r.val[7] || r.val[0])
      fail("flags(8) of 80", "err=" + r.err);
  }
}

static void testVariantEnumRange() {
  {
    auto b = hexDecode("04");
    crab::Decoder d(b.data(), b.size());
    expectExactErr("enum case 4 of 4", d.EnumCase(4).err, "enum case out of range: 4");
  }
  {
    auto b = hexDecode("02");
    crab::Decoder d(b.data(), b.size());
    expectExactErr("variant case 2 of 2", d.VariantCase(2).err,
                   "variant case out of range: 2");
  }
}

static void testTrailingBytes() {
  auto b = hexDecode("0700");
  crab::Decoder d(b.data(), b.size());
  auto r = d.U32();
  if (!r.ok()) {
    fail("trailing bytes", "u32: " + r.err);
    return;
  }
  std::string fin = d.Finish("params");
  expectExactErr("Finish on trailing bytes", fin, "1 trailing byte(s) after params");
}

static void testEmptyValues() {
  {
    std::vector<uint8_t> enc;
    crab::EncodeString(enc, "");
    if (toHex(enc) != "00") fail("empty string encode", toHex(enc));
  }
  {
    auto b = hexDecode("00");
    crab::Decoder d(b.data(), b.size());
    auto r = d.String();
    if (!r.ok() || !r.val.empty()) fail("empty string decode", "err=" + r.err);
  }
  {
    std::vector<uint8_t> enc;
    crab::EncodeListLen(enc, 0);
    if (toHex(enc) != "00") fail("empty list encode", toHex(enc));
  }
}

static void testEOFEverywhere() {
  {
    crab::Decoder d(nullptr, 0);
    expectExactErr("bool on empty buffer", d.Bool().err, "unexpected end of buffer");
  }
  {
    auto b = hexDecode("80");  // dangling continuation bit
    crab::Decoder d(b.data(), b.size());
    expectExactErr("truncated uleb", d.U32().err, "unexpected end of buffer");
  }
  {
    auto b = hexDecode("000000");  // 3 bytes for an f32
    crab::Decoder d(b.data(), b.size());
    expectExactErr("truncated f32", d.F32().err, "unexpected end of buffer");
  }
}

// ---------------------------------------------------------------------------
// mesh reply parser (mirror templates/go/mesh_test.go)
// ---------------------------------------------------------------------------

static void testParseMeshReply() {
  // status 0: body returned verbatim.
  {
    auto p = hexDecode("0007");
    auto r = crab::ParseMeshReply(p.data(), p.size());
    if (!r.ok() || toHex(r.val) != "07")
      fail("mesh status-0 reply", "got " + toHex(r.val) + " err=" + r.err);
  }
  // status 0 with empty body: ok, empty result.
  {
    auto p = hexDecode("00");
    auto r = crab::ParseMeshReply(p.data(), p.size());
    if (!r.ok() || !r.val.empty())
      fail("mesh status-0 empty reply", "err=" + r.err);
  }
  // status 1: the decoded error string.
  std::vector<uint8_t> payload{1};
  crab::EncodeString(payload, "boom");
  expectExactErr("mesh status-1 reply",
                 crab::ParseMeshReply(payload.data(), payload.size()).err, "boom");
  // status 1 with trailing bytes after the error string: malformed.
  {
    auto trailing = payload;
    trailing.push_back(0);
    expectExactErr("mesh status-1 trailing bytes",
                   crab::ParseMeshReply(trailing.data(), trailing.size()).err,
                   "mesh call: malformed error reply (trailing bytes)");
  }
  // status 1 with an undecodable string: malformed.
  {
    auto p = hexDecode("0105");
    expectExactErr("mesh status-1 truncated string",
                   crab::ParseMeshReply(p.data(), p.size()).err,
                   "mesh call: malformed error reply");
  }
  // invalid status byte.
  {
    auto p = hexDecode("02");
    expectExactErr("mesh status 2", crab::ParseMeshReply(p.data(), p.size()).err,
                   "mesh call: invalid reply status");
  }
  // empty payload.
  expectExactErr("mesh empty payload", crab::ParseMeshReply(nullptr, 0).err,
                 "mesh call: empty reply");
  // Off-wasm MeshCall reports the import as unavailable.
  expectExactErr("MeshCall off-wasm",
                 crab::MeshCall("svc", "a:b/c@0.1.0#d", std::vector<uint8_t>{}).err,
                 "crabcraft.call import unavailable: not running under a crabcraft host");
}

int main() {
  runVectors();
  testSlebSignExtension();
  testUlebOverflowBits();
  testCharValidation();
  testStrictBytes();
  testResultNoPayloadTypes();
  testFlagsValidation();
  testVariantEnumRange();
  testTrailingBytes();
  testEmptyValues();
  testEOFEverywhere();
  testParseMeshReply();
  if (FAILURES != 0) {
    std::printf("%d failure(s)\n", FAILURES);
    return 1;
  }
  std::printf("ok: %zu vectors + edge cases\n", NVECTORS);
  return 0;
}
