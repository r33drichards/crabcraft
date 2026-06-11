// crabcraft AssemblyScript runtime: WIRE.md section-1 value codec +
// section-2 guest ABI.
//
// This file is a crabgen TEMPLATE shared by every generated AssemblyScript
// guest; it contains no per-WIT content. Generated bindings
// (assembly/gen/bindings.ts) call the Sink/Decoder primitives in
// straight-line code — no closures, no reflection — call setSchema() with
// the resolved-WIT JSON, and registerHandler() for each exported function.
//
// Error / calling convention (no exceptions; recoverable errors only):
//   - Decoder carries `err: string | null`. Every decode method is a no-op
//     returning a zero value once err is set; generated code checks after
//     each call:
//         const r0 = d.u32();
//         if (d.err !== null) return HandlerResult.fail(d.err!);
//   - Sink (the encoder) carries the same `err: string | null`; the only
//     fallible encode is string() (lone UTF-16 surrogates have no UTF-8
//     form). Check s.err once after encoding.
//   - d.finish(what) returns `string | null`: null = whole buffer consumed,
//     otherwise the "N trailing byte(s) after <what>" message.
//   - Handlers have the signature `(d: Decoder) => HandlerResult` (a plain
//     function reference — AS first-class function, no closure state).
//
// Export strategy: AS only turns exports of the ENTRY file into wasm
// exports, so the generated assembly/index.ts must re-export the ABI:
//     export { crab_alloc, crab_schema, crab_invoke } from "./gen/runtime";
// Top-level statements (setSchema/registerHandler calls) compile into the
// module start function; build with `--exportStart _initialize` so it is
// exported as the reactor's optional _initialize (the host calls it once)
// instead of a wasm start section.
//
// Build flags (asc 0.28, pinned in package.json; the conformance build in
// tests/as_vectors.rs uses exactly these):
//   asc assembly/index.ts -o <module>.wasm \
//     --exportStart _initialize   (reactor: top-level init, no start section)
//     --use abort=                (no env.abort import; abort() traps)
//     --runtime incremental       (full GC; pinning map below keeps host
//                                  buffers alive)
//     --optimizeLevel 3 --shrinkLevel 1
// Memory is exported by default (the host requires it); SIMD is disabled by
// default in asc and the build must stay SIMD-free — the wasmcraft engine
// refuses 0xfd opcodes (run the wasm-objdump tripwire after every build).
//
// Memory/GC: asc --runtime incremental (the default). The GC is non-moving,
// but buffers handed to the host must stay REFERENCED: crab_alloc pins each
// allocation in a Map keyed by address, crab_invoke unpins its name/args
// buffers when it returns, and the current LENBUF reply stays referenced
// until the next crab_invoke/crab_schema call (the host copies
// immediately), per WIRE.md. Same semantics as the Go template.
//
// Strings: AS strings are UTF-16; WIRE strings are UTF-8. Decoding
// hand-validates the bytes (overlongs, surrogates, > U+10FFFF) before
// String.UTF8.decodeUnsafe; encoding rejects lone surrogates (AS would
// otherwise emit WTF-8) with the same "invalid utf-8 in string" error.
// u64/s64 are native AS u64/i64 — full precision, no float fallback.
//
// Error message TEXTS mirror guest/crab-sdk/src/codec.rs and the Go/C++
// templates exactly — cross-language consistency is part of the conformance
// contract ("uleb overflow", "invalid bool byte: 2",
// "N trailing byte(s) after params", ...).
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

// ---------------------------------------------------------------------------
// UTF-8 validation (shared by Decoder.string and the host-facing paths)
// ---------------------------------------------------------------------------

// utf8Valid hand-validates n bytes at ptr: rejects overlong encodings,
// surrogates, scalars above U+10FFFF, stray/missing continuation bytes.
function utf8Valid(ptr: usize, n: i32): bool {
  let i: i32 = 0;
  while (i < n) {
    const c = load<u8>(ptr + <usize>i);
    if (c < 0x80) {
      i++;
      continue;
    }
    let len: i32 = 0;
    let cp: u32 = 0;
    let min: u32 = 0;
    if ((c & 0xe0) == 0xc0) {
      len = 2;
      cp = c & 0x1f;
      min = 0x80;
    } else if ((c & 0xf0) == 0xe0) {
      len = 3;
      cp = c & 0x0f;
      min = 0x800;
    } else if ((c & 0xf8) == 0xf0) {
      len = 4;
      cp = c & 0x07;
      min = 0x10000;
    } else {
      return false; // continuation byte or invalid lead byte
    }
    if (i + len > n) return false;
    for (let j: i32 = 1; j < len; j++) {
      const cc = load<u8>(ptr + <usize>(i + j));
      if ((cc & 0xc0) != 0x80) return false;
      cp = (cp << 6) | <u32>(cc & 0x3f);
    }
    if (cp < min || cp > 0x10ffff || (cp >= 0xd800 && cp <= 0xdfff)) return false;
    i += len;
  }
  return true;
}

// utf16Wellformed reports whether an AS string has no lone surrogates
// (i.e. it has a valid UTF-8 form).
function utf16Wellformed(s: string): bool {
  for (let i = 0; i < s.length; i++) {
    const c = <u32>s.charCodeAt(i);
    if (c >= 0xd800 && c <= 0xdbff) {
      if (i + 1 >= s.length) return false;
      const c2 = <u32>s.charCodeAt(i + 1);
      if (c2 < 0xdc00 || c2 > 0xdfff) return false;
      i++; // valid pair: skip the low surrogate
    } else if (c >= 0xdc00 && c <= 0xdfff) {
      return false; // lone low surrogate
    }
  }
  return true;
}

// ---------------------------------------------------------------------------
// WIRE.md section 1: encode (Sink = append-style byte builder)
// ---------------------------------------------------------------------------

export class Sink {
  private data: Uint8Array = new Uint8Array(64);
  private len: i32 = 0;
  // err: null = ok. Only string() can fail (lone surrogates); generated
  // code checks once after encoding.
  err: string | null = null;

  private ensure(extra: i32): void {
    const need = this.len + extra;
    if (need <= this.data.length) return;
    let cap = this.data.length;
    while (cap < need) cap <<= 1;
    const nd = new Uint8Array(cap);
    nd.set(this.data.subarray(0, this.len));
    this.data = nd;
  }

  // bytes returns a copy of everything appended so far.
  bytes(): Uint8Array {
    return this.data.slice(0, this.len);
  }

  // byte appends one raw byte.
  byte(b: u8): void {
    this.ensure(1);
    this.data[this.len++] = b;
  }

  // raw appends raw bytes verbatim.
  raw(b: Uint8Array): void {
    this.ensure(b.length);
    this.data.set(b, this.len);
    this.len += b.length;
  }

  // uleb appends the unsigned LEB128 encoding of v.
  uleb(v: u64): void {
    for (;;) {
      const b = <u8>(v & 0x7f);
      v >>= 7;
      if (v == 0) {
        this.byte(b);
        return;
      }
      this.byte(b | 0x80);
    }
  }

  // sleb appends the signed LEB128 encoding of v.
  sleb(v: i64): void {
    for (;;) {
      const b = <u8>(v & 0x7f);
      v >>= 7; // arithmetic shift: i64 is signed
      const sign = (b & 0x40) != 0;
      if ((v == 0 && !sign) || (v == -1 && sign)) {
        this.byte(b);
        return;
      }
      this.byte(b | 0x80);
    }
  }

  // bool appends 1 byte: 0 or 1.
  bool(v: bool): void {
    this.byte(v ? 1 : 0);
  }

  // u8..u64 append uleb(v).
  u8(v: u8): void {
    this.uleb(<u64>v);
  }
  u16(v: u16): void {
    this.uleb(<u64>v);
  }
  u32(v: u32): void {
    this.uleb(<u64>v);
  }
  u64(v: u64): void {
    this.uleb(v);
  }

  // s8..s64 append sleb(v).
  s8(v: i8): void {
    this.sleb(<i64>v);
  }
  s16(v: i16): void {
    this.sleb(<i64>v);
  }
  s32(v: i32): void {
    this.sleb(<i64>v);
  }
  s64(v: i64): void {
    this.sleb(v);
  }

  // f32/f64 append 4/8 bytes IEEE-754 LE.
  f32(v: f32): void {
    const bits = reinterpret<u32>(v);
    this.byte(<u8>(bits & 0xff));
    this.byte(<u8>((bits >> 8) & 0xff));
    this.byte(<u8>((bits >> 16) & 0xff));
    this.byte(<u8>((bits >> 24) & 0xff));
  }
  f64(v: f64): void {
    const bits = reinterpret<u64>(v);
    for (let i = 0; i < 8; i++) this.byte(<u8>((bits >> <u64>(8 * i)) & 0xff));
  }

  // char appends uleb(unicode scalar value).
  char(scalar: u32): void {
    this.uleb(<u64>scalar);
  }

  // string appends uleb(byte length) + UTF-8 bytes. Sets err (and appends
  // nothing) when the string has a lone surrogate: it has no UTF-8 form,
  // and silently emitting WTF-8 would violate the wire contract.
  string(s: string): void {
    if (this.err !== null) return;
    if (!utf16Wellformed(s)) {
      this.err = "invalid utf-8 in string";
      return;
    }
    const b = Uint8Array.wrap(String.UTF8.encode(s));
    this.uleb(<u64>b.length);
    this.raw(b);
  }

  // listLen appends uleb(count); the caller then appends each element.
  listLen(n: u32): void {
    this.uleb(<u64>n);
  }

  // caseIdx appends uleb(case index) for variants and enums; for a variant
  // the caller then appends the payload if the case has one. Records and
  // tuples have no header at all: just encode each member in order.
  caseIdx(c: u32): void {
    this.uleb(<u64>c);
  }

  // optionTag appends the option discriminant (0 = none, 1 = some); the
  // caller then appends the inner value when present.
  optionTag(present: bool): void {
    this.bool(present);
  }

  // resultTag appends the result discriminant (0 = ok, 1 = err); the caller
  // then appends the payload if that side has a type.
  resultTag(isErr: bool): void {
    this.bool(isErr);
  }

  // flags appends ceil(bits.length/8) bytes, bit i = flag i, LE byte order.
  flags(bits: Array<bool>): void {
    const nbytes = (bits.length + 7) / 8;
    const start = this.len;
    for (let i = 0; i < nbytes; i++) this.byte(0);
    for (let i = 0; i < bits.length; i++) {
      if (bits[i]) {
        const at = start + i / 8;
        this.data[at] = <u8>(<i32>this.data[at] | (1 << (i % 8)));
      }
    }
  }
}

// ---------------------------------------------------------------------------
// WIRE.md section 1: decode
// ---------------------------------------------------------------------------

// Decoder is a cursor over an encoded buffer. Methods return zero values
// once err is set; check `d.err !== null` after each call.
export class Decoder {
  private buf: Uint8Array;
  private pos: i32 = 0;
  err: string | null = null;

  constructor(buf: Uint8Array) {
    this.buf = buf;
  }

  // remaining reports the number of undecoded bytes.
  remaining(): i32 {
    return this.buf.length - this.pos;
  }

  // finish returns null when the whole buffer was consumed, otherwise
  // "N trailing byte(s) after <what>". Params decoding (and single-value
  // decoding) must always end with a finish check.
  finish(what: string): string | null {
    const n = this.remaining();
    if (n == 0) return null;
    return n.toString() + " trailing byte(s) after " + what;
  }

  private byteRead(): u8 {
    if (this.pos >= this.buf.length) {
      this.err = "unexpected end of buffer";
      return 0;
    }
    return this.buf[this.pos++];
  }

  // ulebBits decodes an unsigned LEB128 capped at `bits` significant bits
  // (max ceil(bits/7) bytes; payload bits above the width on the last
  // permitted byte must be zero).
  ulebBits(bits: u32): u64 {
    if (this.err !== null) return 0;
    const maxBytes = (bits + 6) / 7;
    let result: u64 = 0;
    let shift: u32 = 0;
    for (let i: u32 = 0; i < maxBytes; i++) {
      const b = this.byteRead();
      if (this.err !== null) return 0;
      const payload = <u64>(b & 0x7f);
      if (shift + 7 > bits && payload >> <u64>(bits - shift) != 0) {
        this.err = "uleb overflow";
        return 0;
      }
      result |= payload << <u64>shift;
      if ((b & 0x80) == 0) return result;
      shift += 7;
    }
    this.err = "uleb too long";
    return 0;
  }

  // slebBits decodes a signed LEB128 capped at `bits` (max ceil(bits/7)
  // bytes), range-checked against the width; the 10th byte of an s64 must
  // be 0x00 or 0x7f (only sign-extension patterns fit).
  slebBits(bits: u32): i64 {
    if (this.err !== null) return 0;
    const maxBytes = (bits + 6) / 7;
    let result: i64 = 0;
    let shift: u32 = 0;
    for (let i: u32 = 0; i < maxBytes; i++) {
      const b = this.byteRead();
      if (this.err !== null) return 0;
      if (shift == 63) {
        if (b != 0x00 && b != 0x7f) {
          this.err = "sleb overflow";
          return 0;
        }
        result |= <i64>(<u64>(b & 1) << 63);
        return result;
      }
      result |= <i64>(<u64>(b & 0x7f) << <u64>shift);
      shift += 7;
      if ((b & 0x80) == 0) {
        if (shift < 64 && (b & 0x40) != 0) {
          result |= <i64>(~(<u64>0) << <u64>shift); // sign-extend
        }
        if (bits < 64) {
          const min: i64 = -(<i64>1 << <i64>(bits - 1));
          const max: i64 = (<i64>1 << <i64>(bits - 1)) - 1;
          if (result < min || result > max) {
            this.err = "sleb overflow";
            return 0;
          }
        }
        return result;
      }
    }
    this.err = "sleb too long";
    return 0;
  }

  private prefixBit(what: string): bool {
    if (this.err !== null) return false;
    const b = this.byteRead();
    if (this.err !== null) return false;
    if (b == 0) return false;
    if (b == 1) return true;
    this.err = "invalid " + what + " byte: " + b.toString();
    return false;
  }

  // bool decodes a strict 0/1 byte.
  bool(): bool {
    return this.prefixBit("bool");
  }

  // u8..u64 decode a uleb capped at the type's bit width.
  u8(): u8 {
    return <u8>this.ulebBits(8);
  }
  u16(): u16 {
    return <u16>this.ulebBits(16);
  }
  u32(): u32 {
    return <u32>this.ulebBits(32);
  }
  u64(): u64 {
    return this.ulebBits(64);
  }

  // s8..s64 decode a sleb range-checked to the type's bit width.
  s8(): i8 {
    return <i8>this.slebBits(8);
  }
  s16(): i16 {
    return <i16>this.slebBits(16);
  }
  s32(): i32 {
    return <i32>this.slebBits(32);
  }
  s64(): i64 {
    return this.slebBits(64);
  }

  // f32/f64 decode 4/8 bytes IEEE-754 LE.
  f32(): f32 {
    if (this.err !== null) return 0;
    if (this.remaining() < 4) {
      this.err = "unexpected end of buffer";
      return 0;
    }
    let bits: u32 = 0;
    for (let i = 0; i < 4; i++) {
      bits |= <u32>this.buf[this.pos + i] << (8 * i);
    }
    this.pos += 4;
    return reinterpret<f32>(bits);
  }
  f64(): f64 {
    if (this.err !== null) return 0;
    if (this.remaining() < 8) {
      this.err = "unexpected end of buffer";
      return 0;
    }
    let bits: u64 = 0;
    for (let i = 0; i < 8; i++) {
      bits |= <u64>this.buf[this.pos + i] << <u64>(8 * i);
    }
    this.pos += 8;
    return reinterpret<f64>(bits);
  }

  // char decodes a uleb scalar (21-bit cap) and validates it is a unicode
  // scalar value (<= 0x10FFFF and not a surrogate).
  char(): u32 {
    if (this.err !== null) return 0;
    const v = this.ulebBits(21);
    if (this.err !== null) return 0;
    if (v > 0x10ffff || (v >= 0xd800 && v <= 0xdfff)) {
      this.err = "invalid char scalar: " + v.toString();
      return 0;
    }
    return <u32>v;
  }

  // string decodes uleb(byte length) + UTF-8 bytes, validating the UTF-8.
  string(): string {
    if (this.err !== null) return "";
    const n = <u32>this.ulebBits(32);
    if (this.err !== null) return "";
    if (<u32>this.remaining() < n) {
      this.err = "unexpected end of buffer";
      return "";
    }
    const ptr = this.buf.dataStart + <usize>this.pos;
    if (!utf8Valid(ptr, <i32>n)) {
      this.err = "invalid utf-8 in string";
      return "";
    }
    const s = String.UTF8.decodeUnsafe(ptr, <usize>n);
    this.pos += <i32>n;
    return s;
  }

  // listLen decodes uleb(count); the caller then decodes each element. The
  // count is attacker-controlled, so do not pre-allocate from it blindly —
  // let element decoding fail naturally on short buffers.
  listLen(): u32 {
    return <u32>this.ulebBits(32);
  }

  // optionTag decodes the option discriminant (false = none, true = some);
  // the caller then decodes the inner value when present.
  optionTag(): bool {
    return this.prefixBit("option");
  }

  // resultTag decodes the result discriminant (false = ok, true = err); the
  // caller then decodes the payload if that side has a type.
  resultTag(): bool {
    return this.prefixBit("result");
  }

  // variantCase decodes a u32-uleb case index and bounds-checks it; the
  // caller then decodes the payload if the case has one.
  variantCase(numCases: u32): u32 {
    if (this.err !== null) return 0;
    const v = this.ulebBits(32);
    if (this.err !== null) return 0;
    if (<u32>v >= numCases) {
      this.err = "variant case out of range: " + v.toString();
      return 0;
    }
    return <u32>v;
  }

  // enumCase decodes a u32-uleb case index and bounds-checks it.
  enumCase(numCases: u32): u32 {
    if (this.err !== null) return 0;
    const v = this.ulebBits(32);
    if (this.err !== null) return 0;
    if (<u32>v >= numCases) {
      this.err = "enum case out of range: " + v.toString();
      return 0;
    }
    return <u32>v;
  }

  // flags decodes ceil(n/8) bytes into one bool per flag (bit i = flag i,
  // LE byte order); bits above flag n-1 in the last byte must be zero.
  flags(n: i32): Array<bool> {
    const empty = new Array<bool>(0);
    if (this.err !== null) return empty;
    const nbytes = (n + 7) / 8;
    if (this.remaining() < nbytes) {
      this.err = "unexpected end of buffer";
      return empty;
    }
    if (n % 8 != 0 && <i32>this.buf[this.pos + nbytes - 1] >> (n % 8) != 0) {
      this.err = "flags: unused high bits set";
      return empty;
    }
    const bits = new Array<bool>(n);
    for (let i = 0; i < n; i++) {
      bits[i] = ((<i32>this.buf[this.pos + i / 8] >> (i % 8)) & 1) != 0;
    }
    this.pos += nbytes;
    return bits;
  }
}

// ---------------------------------------------------------------------------
// WIRE.md section 2: guest ABI (crab_alloc / crab_schema / crab_invoke)
// ---------------------------------------------------------------------------

// HandlerResult is what a handler (and meshCall) returns: exactly one of
// bytes (success: the encoded result value) or err is set.
export class HandlerResult {
  bytes: Uint8Array | null = null;
  err: string | null = null;

  static pass(bytes: Uint8Array): HandlerResult {
    const r = new HandlerResult();
    r.bytes = bytes;
    return r;
  }

  static fail(msg: string): HandlerResult {
    const r = new HandlerResult();
    r.err = msg;
    return r;
  }
}

// Handler decodes its params from the Decoder (including the trailing-bytes
// finish check) and returns the encoded result value. Plain function
// reference: AS function values without closure state.
export type Handler = (d: Decoder) => HandlerResult;

// schemaJson is the resolved-WIT JSON served by crab_schema; the generated
// bindings call setSchema at top level (runs in _initialize).
let schemaJson: string = "{}";

export function setSchema(s: string): void {
  schemaJson = s;
}

// handlers maps function addresses (`<instance>#<function>`) to handlers.
const handlers = new Map<string, Handler>();

export function registerHandler(name: string, fn: Handler): void {
  handlers.set(name, fn);
}

// pinned keeps buffers whose addresses were handed to the host REFERENCED
// (the AS GC is non-moving, but unreferenced objects are collected):
// crab_alloc allocations and mesh replies, keyed by data address.
// crab_invoke unpins its name/args buffers when it returns; meshCall unpins
// the host reply after copying it out.
const pinned = new Map<usize, Uint8Array>();

// unpinAlloc releases a pinned crab_alloc buffer (used by the mesh client
// after copying a host reply out).
export function unpinAlloc(ptr: usize): void {
  if (pinned.has(ptr)) pinned.delete(ptr);
}

// reply holds the current LENBUF; valid until the next crab_invoke /
// crab_schema call (the host copies immediately), per WIRE.md.
let reply: Uint8Array = new Uint8Array(0);

// lenbuf builds [u32 LE length][payload], keeps it referenced, and returns
// its address.
function lenbuf(payload: Uint8Array): usize {
  const n = payload.length;
  const out = new Uint8Array(4 + n);
  out[0] = <u8>(n & 0xff);
  out[1] = <u8>((n >> 8) & 0xff);
  out[2] = <u8>((n >> 16) & 0xff);
  out[3] = <u8>((n >> 24) & 0xff);
  out.set(payload, 4);
  reply = out;
  return out.dataStart;
}

// replyOK builds the [status=0][encoded result] reply.
function replyOK(result: Uint8Array): usize {
  const payload = new Uint8Array(1 + result.length);
  payload[0] = 0;
  payload.set(result, 1);
  return lenbuf(payload);
}

// replyErr builds the [status=1][string message] reply.
function replyErr(msg: string): usize {
  const s = new Sink();
  s.byte(1);
  s.string(msg);
  if (s.err !== null) {
    // The message itself was unencodable (lone surrogate): send a safe
    // constant instead of WTF-8.
    const s2 = new Sink();
    s2.byte(1);
    s2.string("invalid reply message");
    return lenbuf(s2.bytes());
  }
  return lenbuf(s.bytes());
}

export function crab_alloc(len: i32): usize {
  const n = len < 1 ? 1 : len;
  const buf = new Uint8Array(n);
  pinned.set(buf.dataStart, buf);
  return buf.dataStart;
}

export function crab_schema(): usize {
  return lenbuf(Uint8Array.wrap(String.UTF8.encode(schemaJson)));
}

export function crab_invoke(
  namePtr: usize,
  nameLen: i32,
  argPtr: usize,
  argLen: i32
): usize {
  // Copy the name and args out of the host-written buffers, then unpin them
  // (warm reactors would otherwise leak one pin per call).
  const name =
    nameLen > 0 ? String.UTF8.decodeUnsafe(namePtr, <usize>nameLen) : "";
  const args = new Uint8Array(argLen > 0 ? argLen : 0);
  if (argLen > 0) memory.copy(args.dataStart, argPtr, <usize>argLen);
  unpinAlloc(namePtr);
  unpinAlloc(argPtr);

  if (!handlers.has(name)) return replyErr("unknown function: " + name);
  const h = handlers.get(name);
  const r = h(new Decoder(args));
  if (r.err !== null) return replyErr(name + ": " + r.err!);
  const bytes = r.bytes;
  return replyOK(bytes !== null ? bytes : new Uint8Array(0));
}
