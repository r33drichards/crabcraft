// crabcraft C++ runtime implementation: WIRE.md section-1 codec (compiles
// everywhere) + section-2 guest ABI exports (wasm-only, guarded at the
// bottom). See crab.hpp for the calling convention and validation rules.

#include "crab.hpp"

#include <cstring>

namespace crab {

namespace {

template <class T>
Res<T> Ok(T v) {
  return Res<T>{std::move(v), {}};
}

// Hand-rolled UTF-8 validator: rejects overlong encodings, surrogates,
// scalars above U+10FFFF, stray/missing continuation bytes.
bool Utf8Valid(const uint8_t* p, size_t n) {
  size_t i = 0;
  while (i < n) {
    uint8_t c = p[i];
    if (c < 0x80) {
      i++;
      continue;
    }
    size_t len;
    uint32_t cp, min;
    if ((c & 0xe0) == 0xc0) {
      len = 2, cp = c & 0x1f, min = 0x80;
    } else if ((c & 0xf0) == 0xe0) {
      len = 3, cp = c & 0x0f, min = 0x800;
    } else if ((c & 0xf8) == 0xf0) {
      len = 4, cp = c & 0x07, min = 0x10000;
    } else {
      return false;  // continuation byte or invalid lead byte
    }
    if (i + len > n) return false;
    for (size_t j = 1; j < len; j++) {
      uint8_t cc = p[i + j];
      if ((cc & 0xc0) != 0x80) return false;
      cp = (cp << 6) | (cc & 0x3f);
    }
    if (cp < min || cp > 0x10FFFF || (cp >= 0xD800 && cp <= 0xDFFF)) return false;
    i += len;
  }
  return true;
}

}  // namespace

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

void EncodeUleb(std::vector<uint8_t>& out, uint64_t v) {
  for (;;) {
    uint8_t b = v & 0x7f;
    v >>= 7;
    if (v == 0) {
      out.push_back(b);
      return;
    }
    out.push_back(b | 0x80);
  }
}

void EncodeSleb(std::vector<uint8_t>& out, int64_t v) {
  for (;;) {
    uint8_t b = v & 0x7f;
    v >>= 7;  // arithmetic shift (sign-propagating) on every supported target
    bool sign = (b & 0x40) != 0;
    if ((v == 0 && !sign) || (v == -1 && sign)) {
      out.push_back(b);
      return;
    }
    out.push_back(b | 0x80);
  }
}

void EncodeBool(std::vector<uint8_t>& out, bool v) { out.push_back(v ? 1 : 0); }

void EncodeU8(std::vector<uint8_t>& out, uint8_t v) { EncodeUleb(out, v); }
void EncodeU16(std::vector<uint8_t>& out, uint16_t v) { EncodeUleb(out, v); }
void EncodeU32(std::vector<uint8_t>& out, uint32_t v) { EncodeUleb(out, v); }
void EncodeU64(std::vector<uint8_t>& out, uint64_t v) { EncodeUleb(out, v); }

void EncodeS8(std::vector<uint8_t>& out, int8_t v) { EncodeSleb(out, v); }
void EncodeS16(std::vector<uint8_t>& out, int16_t v) { EncodeSleb(out, v); }
void EncodeS32(std::vector<uint8_t>& out, int32_t v) { EncodeSleb(out, v); }
void EncodeS64(std::vector<uint8_t>& out, int64_t v) { EncodeSleb(out, v); }

void EncodeF32(std::vector<uint8_t>& out, float v) {
  uint32_t bits;
  std::memcpy(&bits, &v, sizeof bits);
  out.push_back(bits & 0xff);
  out.push_back((bits >> 8) & 0xff);
  out.push_back((bits >> 16) & 0xff);
  out.push_back((bits >> 24) & 0xff);
}

void EncodeF64(std::vector<uint8_t>& out, double v) {
  uint64_t bits;
  std::memcpy(&bits, &v, sizeof bits);
  for (int i = 0; i < 8; i++) out.push_back((bits >> (8 * i)) & 0xff);
}

void EncodeChar(std::vector<uint8_t>& out, uint32_t scalar) {
  EncodeUleb(out, scalar);
}

void EncodeString(std::vector<uint8_t>& out, std::string_view s) {
  EncodeUleb(out, s.size());
  out.insert(out.end(), s.begin(), s.end());
}

void EncodeListLen(std::vector<uint8_t>& out, size_t n) { EncodeUleb(out, n); }

void EncodeCase(std::vector<uint8_t>& out, uint32_t c) { EncodeUleb(out, c); }

void EncodeOptionTag(std::vector<uint8_t>& out, bool present) {
  EncodeBool(out, present);
}

void EncodeResultTag(std::vector<uint8_t>& out, bool is_err) {
  EncodeBool(out, is_err);
}

void EncodeFlags(std::vector<uint8_t>& out, const std::vector<bool>& bits) {
  size_t nbytes = (bits.size() + 7) / 8;
  size_t start = out.size();
  out.insert(out.end(), nbytes, 0);
  for (size_t i = 0; i < bits.size(); i++)
    if (bits[i]) out[start + i / 8] |= (uint8_t)(1u << (i % 8));
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

std::string Decoder::Finish(const char* what) const {
  size_t n = Remaining();
  if (n == 0) return std::string();
  return std::to_string(n) + " trailing byte(s) after " + what;
}

Res<uint8_t> Decoder::Byte() {
  if (pos_ >= len_) return Res<uint8_t>::fail("unexpected end of buffer");
  return Ok<uint8_t>(buf_[pos_++]);
}

Res<const uint8_t*> Decoder::Bytes(size_t n) {
  if (Remaining() < n) return Res<const uint8_t*>::fail("unexpected end of buffer");
  const uint8_t* p = buf_ + pos_;
  pos_ += n;
  return Ok(p);
}

// Uleb decodes an unsigned LEB128 capped at `bits` significant bits
// (max ceil(bits/7) bytes; payload bits above the width on the last
// permitted byte must be zero).
Res<uint64_t> Decoder::Uleb(unsigned bits) {
  unsigned max_bytes = (bits + 6) / 7;
  uint64_t result = 0;
  unsigned shift = 0;
  for (unsigned i = 0; i < max_bytes; i++) {
    auto b = Byte();
    if (!b.ok()) return Res<uint64_t>::fail(b.err);
    uint64_t payload = b.val & 0x7f;
    if (shift + 7 > bits && (payload >> (bits - shift)) != 0)
      return Res<uint64_t>::fail("uleb overflow");
    result |= payload << shift;
    if ((b.val & 0x80) == 0) return Ok(result);
    shift += 7;
  }
  return Res<uint64_t>::fail("uleb too long");
}

// Sleb decodes a signed LEB128 capped at `bits` (max ceil(bits/7) bytes),
// range-checked against the width; the 10th byte of an s64 must be 0x00 or
// 0x7f (only sign-extension patterns fit).
Res<int64_t> Decoder::Sleb(unsigned bits) {
  unsigned max_bytes = (bits + 6) / 7;
  int64_t result = 0;
  unsigned shift = 0;
  for (unsigned i = 0; i < max_bytes; i++) {
    auto b = Byte();
    if (!b.ok()) return Res<int64_t>::fail(b.err);
    uint8_t byte = b.val;
    if (shift == 63) {
      if (byte != 0x00 && byte != 0x7f) return Res<int64_t>::fail("sleb overflow");
      result |= (int64_t)((uint64_t)(byte & 1) << 63);
      return Ok(result);
    }
    result |= (int64_t)((uint64_t)(byte & 0x7f) << shift);
    shift += 7;
    if ((byte & 0x80) == 0) {
      if (shift < 64 && (byte & 0x40) != 0)
        result |= (int64_t)(~(uint64_t)0 << shift);  // sign-extend
      if (bits < 64) {
        int64_t min = -((int64_t)1 << (bits - 1));
        int64_t max = ((int64_t)1 << (bits - 1)) - 1;
        if (result < min || result > max) return Res<int64_t>::fail("sleb overflow");
      }
      return Ok(result);
    }
  }
  return Res<int64_t>::fail("sleb too long");
}

Res<bool> Decoder::PrefixBit(const char* what) {
  auto b = Byte();
  if (!b.ok()) return Res<bool>::fail(b.err);
  if (b.val == 0) return Ok(false);
  if (b.val == 1) return Ok(true);
  return Res<bool>::fail(std::string("invalid ") + what +
                         " byte: " + std::to_string(b.val));
}

Res<bool> Decoder::Bool() { return PrefixBit("bool"); }

Res<uint8_t> Decoder::U8() {
  auto v = Uleb(8);
  if (!v.ok()) return Res<uint8_t>::fail(std::move(v.err));
  return Ok<uint8_t>((uint8_t)v.val);
}

Res<uint16_t> Decoder::U16() {
  auto v = Uleb(16);
  if (!v.ok()) return Res<uint16_t>::fail(std::move(v.err));
  return Ok<uint16_t>((uint16_t)v.val);
}

Res<uint32_t> Decoder::U32() {
  auto v = Uleb(32);
  if (!v.ok()) return Res<uint32_t>::fail(std::move(v.err));
  return Ok<uint32_t>((uint32_t)v.val);
}

Res<uint64_t> Decoder::U64() { return Uleb(64); }

Res<int8_t> Decoder::S8() {
  auto v = Sleb(8);
  if (!v.ok()) return Res<int8_t>::fail(std::move(v.err));
  return Ok<int8_t>((int8_t)v.val);
}

Res<int16_t> Decoder::S16() {
  auto v = Sleb(16);
  if (!v.ok()) return Res<int16_t>::fail(std::move(v.err));
  return Ok<int16_t>((int16_t)v.val);
}

Res<int32_t> Decoder::S32() {
  auto v = Sleb(32);
  if (!v.ok()) return Res<int32_t>::fail(std::move(v.err));
  return Ok<int32_t>((int32_t)v.val);
}

Res<int64_t> Decoder::S64() { return Sleb(64); }

Res<float> Decoder::F32() {
  auto b = Bytes(4);
  if (!b.ok()) return Res<float>::fail(std::move(b.err));
  uint32_t bits = (uint32_t)b.val[0] | (uint32_t)b.val[1] << 8 |
                  (uint32_t)b.val[2] << 16 | (uint32_t)b.val[3] << 24;
  float f;
  std::memcpy(&f, &bits, sizeof f);
  return Ok(f);
}

Res<double> Decoder::F64() {
  auto b = Bytes(8);
  if (!b.ok()) return Res<double>::fail(std::move(b.err));
  uint64_t bits = 0;
  for (int i = 0; i < 8; i++) bits |= (uint64_t)b.val[i] << (8 * i);
  double f;
  std::memcpy(&f, &bits, sizeof f);
  return Ok(f);
}

Res<uint32_t> Decoder::Char() {
  auto v = Uleb(21);
  if (!v.ok()) return Res<uint32_t>::fail(std::move(v.err));
  uint64_t s = v.val;
  if (s > 0x10FFFF || (s >= 0xD800 && s <= 0xDFFF))
    return Res<uint32_t>::fail("invalid char scalar: " + std::to_string(s));
  return Ok<uint32_t>((uint32_t)s);
}

Res<std::string> Decoder::String() {
  auto n = Uleb(32);
  if (!n.ok()) return Res<std::string>::fail(std::move(n.err));
  auto b = Bytes((size_t)n.val);
  if (!b.ok()) return Res<std::string>::fail(std::move(b.err));
  if (!Utf8Valid(b.val, (size_t)n.val))
    return Res<std::string>::fail("invalid utf-8 in string");
  return Ok(std::string((const char*)b.val, (size_t)n.val));
}

Res<uint32_t> Decoder::ListLen() {
  auto v = Uleb(32);
  if (!v.ok()) return Res<uint32_t>::fail(std::move(v.err));
  return Ok<uint32_t>((uint32_t)v.val);
}

Res<bool> Decoder::OptionTag() { return PrefixBit("option"); }

Res<bool> Decoder::ResultTag() { return PrefixBit("result"); }

Res<uint32_t> Decoder::VariantCase(uint32_t num_cases) {
  auto v = Uleb(32);
  if (!v.ok()) return Res<uint32_t>::fail(std::move(v.err));
  if (v.val >= num_cases)
    return Res<uint32_t>::fail("variant case out of range: " + std::to_string(v.val));
  return Ok<uint32_t>((uint32_t)v.val);
}

Res<uint32_t> Decoder::EnumCase(uint32_t num_cases) {
  auto v = Uleb(32);
  if (!v.ok()) return Res<uint32_t>::fail(std::move(v.err));
  if (v.val >= num_cases)
    return Res<uint32_t>::fail("enum case out of range: " + std::to_string(v.val));
  return Ok<uint32_t>((uint32_t)v.val);
}

Res<std::vector<bool>> Decoder::Flags(size_t n) {
  size_t nbytes = (n + 7) / 8;
  auto b = Bytes(nbytes);
  if (!b.ok()) return Res<std::vector<bool>>::fail(std::move(b.err));
  if (n % 8 != 0 && (b.val[nbytes - 1] >> (n % 8)) != 0)
    return Res<std::vector<bool>>::fail("flags: unused high bits set");
  std::vector<bool> bits(n);
  for (size_t i = 0; i < n; i++) bits[i] = (b.val[i / 8] >> (i % 8)) & 1;
  return Ok(std::move(bits));
}

// ---------------------------------------------------------------------------
// guest ABI glue (target-independent half)
// ---------------------------------------------------------------------------

std::map<std::string, Handler>& Handlers() {
  static std::map<std::string, Handler> m;
  return m;
}

bool RegisterHandler(const char* name, Handler h) {
  Handlers()[name] = h;
  return true;
}

namespace detail {
std::map<uintptr_t, std::vector<uint8_t>>& Allocs() {
  static std::map<uintptr_t, std::vector<uint8_t>> m;
  return m;
}
}  // namespace detail

}  // namespace crab

// ---------------------------------------------------------------------------
// WIRE.md section 2: crab_alloc / crab_schema / crab_invoke (wasm-only).
// __attribute__((export_name)) is wasm-specific, hence the guard; the host
// half of every reply is a LENBUF = [u32 LE length][payload] that stays
// valid until the next crab_invoke/crab_schema call (the host copies
// immediately).
// ---------------------------------------------------------------------------
#if defined(__wasm__)

namespace {

// The current LENBUF reply (static so it outlives the call returning it).
std::vector<uint8_t>& Reply() {
  static std::vector<uint8_t> r;
  return r;
}

const uint8_t* Lenbuf(const uint8_t* payload, size_t n) {
  auto& r = Reply();
  r.clear();
  r.push_back(n & 0xff);
  r.push_back((n >> 8) & 0xff);
  r.push_back((n >> 16) & 0xff);
  r.push_back((n >> 24) & 0xff);
  r.insert(r.end(), payload, payload + n);
  return r.data();
}

// [status=0][encoded result]
const uint8_t* ReplyOK(const std::vector<uint8_t>& result) {
  std::vector<uint8_t> payload;
  payload.reserve(1 + result.size());
  payload.push_back(0);
  payload.insert(payload.end(), result.begin(), result.end());
  return Lenbuf(payload.data(), payload.size());
}

// [status=1][string message]
const uint8_t* ReplyErr(const std::string& msg) {
  std::vector<uint8_t> payload;
  payload.reserve(2 + msg.size());
  payload.push_back(1);
  crab::EncodeString(payload, msg);
  return Lenbuf(payload.data(), payload.size());
}

}  // namespace

extern "C" {

__attribute__((export_name("crab_alloc"))) uint8_t* crab_alloc(int32_t len) {
  if (len < 1) len = 1;
  std::vector<uint8_t> buf((size_t)len);
  uint8_t* p = buf.data();
  crab::detail::Allocs().emplace((uintptr_t)p, std::move(buf));
  return p;
}

__attribute__((export_name("crab_schema"))) const uint8_t* crab_schema() {
  std::string_view s = crab::SchemaJson();
  return Lenbuf((const uint8_t*)s.data(), s.size());
}

__attribute__((export_name("crab_invoke"))) const uint8_t* crab_invoke(
    const char* name_ptr, int32_t name_len, const uint8_t* arg_ptr,
    int32_t arg_len) {
  // Unpin the host-written name/args buffers when this invoke returns (warm
  // reactors would otherwise leak one pin per call). Safe because the name
  // is copied into a std::string, the handler has finished by the time the
  // destructor runs, and the reply lives in its own static buffer.
  struct Unpin {
    uintptr_t a, b;
    ~Unpin() {
      auto& m = crab::detail::Allocs();
      m.erase(a);
      m.erase(b);
    }
  } unpin{(uintptr_t)name_ptr, (uintptr_t)arg_ptr};

  std::string name(name_ptr, name_len > 0 ? (size_t)name_len : 0);
  auto& handlers = crab::Handlers();
  auto it = handlers.find(name);
  if (it == handlers.end()) return ReplyErr("unknown function: " + name);
  crab::Decoder d(arg_ptr, arg_len > 0 ? (size_t)arg_len : 0);
  auto r = it->second(d);
  if (!r.ok()) return ReplyErr(name + ": " + r.err);
  return ReplyOK(r.val);
}

}  // extern "C"

#endif  // defined(__wasm__)
