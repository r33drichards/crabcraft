// Edge cases beyond the shared vectors, mirroring
// templates/go/runtime_test.go and testdata/cpp-runtime/vectors_main.cpp
// (error message TEXTS asserted exactly — cross-language consistency), plus
// the AS-specific UTF-16 lone-surrogate rules and the mesh reply parser
// tests from templates/go/mesh_test.go. The live meshCall cases run against
// the fake `crabcraft.call` host in run_vectors.mjs: 1st call replies
// status-0 body 0x07, 2nd replies status-1 "boom", 3rd returns a null ptr.

import { Decoder, Sink } from "./gen/runtime";
import { meshCall, parseMeshReply } from "./gen/mesh";
import { expectExactErr, fail, hexDecode, toHex } from "./harness";

function testSlebSignExtension(): void {
  // s8 -1 encodes as a single 0x7f byte.
  const s = new Sink();
  s.s8(-1);
  if (toHex(s.bytes()) != "7f") fail("EncodeS8(-1)", "got " + toHex(s.bytes()) + ", want 7f");
  {
    const d = new Decoder(hexDecode("7f"));
    const v = d.s8();
    if (d.err !== null || v != -1) fail("S8(7f)", "want -1; err=" + (d.err !== null ? d.err! : ""));
  }
  // s64 min round-trips (10-byte sleb ending 0x7f) — full i64 precision.
  {
    const s2 = new Sink();
    s2.s64(i64.MIN_VALUE);
    const d = new Decoder(s2.bytes());
    const v = d.s64();
    if (d.err !== null || v != i64.MIN_VALUE || d.remaining() != 0) {
      fail("s64 min round-trip", "enc=" + toHex(s2.bytes()) + " err=" + (d.err !== null ? d.err! : ""));
    }
  }
  // 10th byte of an s64 may only be 0x00 or 0x7f.
  {
    const d = new Decoder(hexDecode("80808080808080808001"));
    d.s64();
    expectExactErr("s64 invalid 10th byte", d.err, "sleb overflow");
  }
  // s8 range check: 128 = 0x80 0x01 as sleb is out of range.
  {
    const d = new Decoder(hexDecode("8001"));
    d.s8();
    expectExactErr("s8 = 128", d.err, "sleb overflow");
  }
}

function testUlebOverflowBits(): void {
  // u8 max is 2 bytes; payload bits above bit 7 on byte 2 must be zero.
  {
    const d = new Decoder(hexDecode("ff03")); // would be 511
    d.u8();
    expectExactErr("u8 = 511", d.err, "uleb overflow");
  }
  // Continuation bit on the last permitted byte: too long.
  {
    const d = new Decoder(hexDecode("ff8100"));
    d.u8();
    expectExactErr("3-byte uleb for u8", d.err, "uleb too long");
  }
  // Non-canonical zero padding is accepted: 0x87 0x00 decodes to 7.
  {
    const d = new Decoder(hexDecode("8700"));
    const v = d.u8();
    if (d.err !== null || v != 7) fail("non-canonical uleb 8700", "want 7; err=" + (d.err !== null ? d.err! : ""));
  }
  // u64 10th byte may only contribute bit 63: 0x02 there overflows.
  {
    const d = new Decoder(hexDecode("ffffffffffffffffff02"));
    d.u64();
    expectExactErr("u64 with bit 64 set", d.err, "uleb overflow");
  }
  // u64 max round-trips with full integer precision (no float fallback).
  {
    const s = new Sink();
    s.u64(u64.MAX_VALUE);
    if (toHex(s.bytes()) != "ffffffffffffffffff01") fail("EncodeU64(max)", "got " + toHex(s.bytes()));
    const d = new Decoder(s.bytes());
    const v = d.u64();
    if (d.err !== null || v != u64.MAX_VALUE) fail("u64 max round-trip", "err=" + (d.err !== null ? d.err! : ""));
  }
}

function testCharValidation(): void {
  // Surrogate U+D800 (uleb 0x80 0xb0 0x03) is not a unicode scalar value.
  {
    const d = new Decoder(hexDecode("80b003"));
    d.char();
    expectExactErr("char U+D800", d.err, "invalid char scalar: 55296");
  }
  // Above U+10FFFF.
  {
    const s = new Sink();
    s.u32(0x110000);
    const d = new Decoder(s.bytes());
    d.char();
    expectExactErr("char U+110000", d.err, "invalid char scalar: 1114112");
  }
  // Max scalar U+10FFFF is fine.
  {
    const s = new Sink();
    s.u32(0x10ffff);
    const d = new Decoder(s.bytes());
    const v = d.char();
    if (d.err !== null || v != 0x10ffff) fail("char U+10FFFF", "err=" + (d.err !== null ? d.err! : ""));
  }
}

function testStrictBytes(): void {
  // bool / option / result discriminants must be exactly 0 or 1.
  {
    const d = new Decoder(hexDecode("02"));
    d.bool();
    expectExactErr("bool byte 2", d.err, "invalid bool byte: 2");
  }
  {
    const d = new Decoder(hexDecode("02"));
    d.optionTag();
    expectExactErr("option byte 2", d.err, "invalid option byte: 2");
  }
  {
    const d = new Decoder(hexDecode("ff"));
    d.resultTag();
    expectExactErr("result byte 255", d.err, "invalid result byte: 255");
  }
  // invalid utf-8 in string.
  {
    const d = new Decoder(hexDecode("02fffe"));
    d.string();
    expectExactErr("invalid utf-8", d.err, "invalid utf-8 in string");
  }
  // WTF-8-encoded surrogate (ed a0 80 = U+D800) is NOT valid utf-8.
  {
    const d = new Decoder(hexDecode("03eda080"));
    d.string();
    expectExactErr("wtf-8 surrogate bytes", d.err, "invalid utf-8 in string");
  }
  // overlong encoding (c0 80 = overlong NUL) is rejected.
  {
    const d = new Decoder(hexDecode("02c080"));
    d.string();
    expectExactErr("overlong utf-8", d.err, "invalid utf-8 in string");
  }
  // string length past end of buffer.
  {
    const d = new Decoder(hexDecode("056869")); // says 5 bytes, has 2
    d.string();
    expectExactErr("truncated string", d.err, "unexpected end of buffer");
  }
}

function testLoneSurrogateEncode(): void {
  // AS strings are UTF-16: a lone surrogate has no UTF-8 form, and encoding
  // it must fail rather than silently emit WTF-8.
  {
    const s = new Sink();
    s.string(String.fromCharCode(0xd800)); // lone high surrogate
    expectExactErr("encode lone high surrogate", s.err, "invalid utf-8 in string");
  }
  {
    const s = new Sink();
    s.string(String.fromCharCode(0xdc00)); // lone low surrogate
    expectExactErr("encode lone low surrogate", s.err, "invalid utf-8 in string");
  }
  // A proper surrogate pair (U+1F980 crab) encodes fine.
  {
    const s = new Sink();
    s.string("🦀");
    if (s.err !== null) {
      fail("encode surrogate pair", "err=" + s.err!);
    } else if (toHex(s.bytes()) != "04f09fa680") {
      fail("encode surrogate pair", "got " + toHex(s.bytes()));
    }
  }
}

function testResultNoPayloadTypes(): void {
  // result (no ok/err types): a bare status byte.
  const d = new Decoder(hexDecode("00"));
  const isErr = d.resultTag();
  if (d.err !== null) {
    fail("result no payload", "decode: " + d.err!);
    return;
  }
  const fin = d.finish("value");
  if (fin !== null) {
    fail("result no payload", "trailing: " + fin!);
    return;
  }
  const s = new Sink();
  s.resultTag(isErr);
  if (toHex(s.bytes()) != "00") fail("result no payload", "re-encode " + toHex(s.bytes()));
}

function testFlagsValidation(): void {
  // 10 flags = 2 bytes; bits 10..15 of byte 2 must be zero.
  {
    const d = new Decoder(hexDecode("0004")); // bit 10 set
    d.flags(10);
    expectExactErr("flags unused bit", d.err, "flags: unused high bits set");
  }
  // Exactly 8 flags: a full byte, no unused-bit check, bit 7 = 0x80.
  {
    const d = new Decoder(hexDecode("80"));
    const bits = d.flags(8);
    if (d.err !== null || bits.length != 8 || !bits[7] || bits[0]) {
      fail("flags(8) of 80", "err=" + (d.err !== null ? d.err! : ""));
    }
  }
}

function testVariantEnumRange(): void {
  {
    const d = new Decoder(hexDecode("04"));
    d.enumCase(4);
    expectExactErr("enum case 4 of 4", d.err, "enum case out of range: 4");
  }
  {
    const d = new Decoder(hexDecode("02"));
    d.variantCase(2);
    expectExactErr("variant case 2 of 2", d.err, "variant case out of range: 2");
  }
}

function testTrailingBytes(): void {
  const d = new Decoder(hexDecode("0700"));
  d.u32();
  if (d.err !== null) {
    fail("trailing bytes", "u32: " + d.err!);
    return;
  }
  expectExactErr("finish on trailing bytes", d.finish("params"), "1 trailing byte(s) after params");
}

function testEmptyValues(): void {
  {
    const s = new Sink();
    s.string("");
    if (toHex(s.bytes()) != "00") fail("empty string encode", toHex(s.bytes()));
  }
  {
    const d = new Decoder(hexDecode("00"));
    const v = d.string();
    if (d.err !== null || v != "") fail("empty string decode", "err=" + (d.err !== null ? d.err! : ""));
  }
  {
    const s = new Sink();
    s.listLen(0);
    if (toHex(s.bytes()) != "00") fail("empty list encode", toHex(s.bytes()));
  }
}

function testEOFEverywhere(): void {
  {
    const d = new Decoder(new Uint8Array(0));
    d.bool();
    expectExactErr("bool on empty buffer", d.err, "unexpected end of buffer");
  }
  {
    const d = new Decoder(hexDecode("80")); // dangling continuation bit
    d.u32();
    expectExactErr("truncated uleb", d.err, "unexpected end of buffer");
  }
  {
    const d = new Decoder(hexDecode("000000")); // 3 bytes for an f32
    d.f32();
    expectExactErr("truncated f32", d.err, "unexpected end of buffer");
  }
}

// ---------------------------------------------------------------------------
// mesh reply parser (mirror templates/go/mesh_test.go) + live mesh calls
// ---------------------------------------------------------------------------

function testParseMeshReply(): void {
  // status 0: body returned verbatim.
  {
    const r = parseMeshReply(hexDecode("0007"));
    if (r.err !== null || toHex(r.bytes!) != "07") {
      fail("mesh status-0 reply", "err=" + (r.err !== null ? r.err! : "got " + toHex(r.bytes!)));
    }
  }
  // status 0 with empty body: ok, empty result.
  {
    const r = parseMeshReply(hexDecode("00"));
    if (r.err !== null || r.bytes!.length != 0) {
      fail("mesh status-0 empty reply", "err=" + (r.err !== null ? r.err! : ""));
    }
  }
  // status 1: the decoded error string.
  {
    const s = new Sink();
    s.byte(1);
    s.string("boom");
    expectExactErr("mesh status-1 reply", parseMeshReply(s.bytes()).err, "boom");
    // status 1 with trailing bytes after the error string: malformed.
    s.byte(0);
    expectExactErr(
      "mesh status-1 trailing bytes",
      parseMeshReply(s.bytes()).err,
      "mesh call: malformed error reply (trailing bytes)"
    );
  }
  // status 1 with an undecodable string: malformed.
  expectExactErr(
    "mesh status-1 truncated string",
    parseMeshReply(hexDecode("0105")).err,
    "mesh call: malformed error reply"
  );
  // invalid status byte.
  expectExactErr(
    "mesh status 2",
    parseMeshReply(hexDecode("02")).err,
    "mesh call: invalid reply status"
  );
  // empty payload.
  expectExactErr(
    "mesh empty payload",
    parseMeshReply(new Uint8Array(0)).err,
    "mesh call: empty reply"
  );
}

function testMeshLive(): void {
  // The node fake replies, in order: ok 0x07; err "boom"; null pointer. The
  // fake writes its replies into guest memory via re-entrant crab_alloc,
  // exercising the real import convention end to end.
  const params = new Sink();
  params.u32(7);
  {
    const r = meshCall("svc", "a:b/c@0.1.0#d", params.bytes());
    if (r.err !== null || toHex(r.bytes!) != "07") {
      fail("mesh live ok", "err=" + (r.err !== null ? r.err! : "got " + toHex(r.bytes!)));
    }
  }
  expectExactErr("mesh live err", meshCall("svc", "a:b/c@0.1.0#d", params.bytes()).err, "boom");
  expectExactErr(
    "mesh live null reply",
    meshCall("svc", "a:b/c@0.1.0#d", new Uint8Array(0)).err,
    "mesh call: empty reply"
  );
}

export function runEdgeCases(): void {
  testSlebSignExtension();
  testUlebOverflowBits();
  testCharValidation();
  testStrictBytes();
  testLoneSurrogateEncode();
  testResultNoPayloadTypes();
  testFlagsValidation();
  testVariantEnumRange();
  testTrailingBytes();
  testEmptyValues();
  testEOFEverywhere();
  testParseMeshReply();
  testMeshLive();
}
