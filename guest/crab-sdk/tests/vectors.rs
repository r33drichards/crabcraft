//! Codec conformance: every golden vector round-trips, and decoding the
//! golden hex yields the expected value.

use crab_sdk::codec::{decode, decode_params, encode_to_vec, Decoder};
use crab_sdk::value::{Type, Value};
use crab_sdk::vectors::vectors;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn golden_encode() {
    for v in vectors() {
        assert_eq!(
            hex(&encode_to_vec(&v.value)),
            v.hex,
            "encode mismatch: {}",
            v.desc
        );
    }
}

#[test]
fn golden_decode() {
    for v in vectors() {
        let decoded = decode(&v.ty, &unhex(v.hex))
            .unwrap_or_else(|e| panic!("decode failed for {}: {e}", v.desc));
        assert_eq!(decoded, v.value, "decode mismatch: {}", v.desc);
    }
}

#[test]
fn round_trip() {
    for v in vectors() {
        let encoded = encode_to_vec(&v.value);
        let decoded = decode(&v.ty, &encoded)
            .unwrap_or_else(|e| panic!("round-trip decode failed for {}: {e}", v.desc));
        assert_eq!(decoded, v.value, "round-trip mismatch: {}", v.desc);
    }
}

#[test]
fn params_concatenation() {
    // add(a: u32, b: u32): params are concatenated in declaration order.
    let mut buf = Vec::new();
    crab_sdk::codec::encode(&Value::U32(624485), &mut buf);
    crab_sdk::codec::encode(&Value::U32(2), &mut buf);
    let params = decode_params(&[Type::U32, Type::U32], &buf).unwrap();
    assert_eq!(params, vec![Value::U32(624485), Value::U32(2)]);

    // trailing bytes are rejected
    buf.push(0);
    assert!(decode_params(&[Type::U32, Type::U32], &buf).is_err());
}

#[test]
fn leb_limits() {
    // u8: at most 2 bytes; payload bits above bit 7 must be zero
    assert!(decode(&Type::U8, &[0xff, 0x01]).is_ok());
    assert!(decode(&Type::U8, &[0xff, 0x02]).is_err()); // overflow
    assert!(decode(&Type::U8, &[0x80, 0x80, 0x00]).is_err()); // too long
    // non-canonical zero padding within the limit is accepted
    assert_eq!(decode(&Type::U8, &[0x85, 0x00]).unwrap(), Value::U8(5));

    // u64: at most 10 bytes; 10th byte <= 0x01
    let mut max = vec![0xff; 9];
    max.push(0x01);
    assert_eq!(decode(&Type::U64, &max).unwrap(), Value::U64(u64::MAX));
    let mut over = vec![0xff; 9];
    over.push(0x02);
    assert!(decode(&Type::U64, &over).is_err());
    let too_long = vec![0x80; 10];
    assert!(decode(&Type::U64, &too_long).is_err());

    // s64 min: 80 80 80 80 80 80 80 80 80 7f
    let mut s64min = vec![0x80; 9];
    s64min.push(0x7f);
    assert_eq!(decode(&Type::S64, &s64min).unwrap(), Value::S64(i64::MIN));
    // s8 range check
    assert_eq!(decode(&Type::S8, &[0x80, 0x7f]).unwrap(), Value::S8(-128));
    assert!(decode(&Type::S8, &[0xff, 0x7e]).is_err()); // -129
    assert!(decode(&Type::S8, &[0x80, 0x01]).is_err()); // 128
}

#[test]
fn strict_discriminants() {
    assert!(decode(&Type::Bool, &[0x02]).is_err());
    assert!(decode(&Type::Option(Box::new(Type::U8)), &[0x02, 0x00]).is_err());
    assert!(decode(&Type::Enum(4), &[0x04]).is_err());
    assert!(decode(&Type::Variant(vec![None]), &[0x01]).is_err());
    // flags: unused high bits in the last byte must be zero
    assert!(decode(&Type::Flags(10), &[0x00, 0x04]).is_err());
    assert!(decode(&Type::Flags(10), &[0x00, 0x03]).is_ok());
}

#[test]
fn invalid_payloads() {
    // string must be valid utf-8
    assert!(decode(&Type::String, &[0x01, 0xff]).is_err());
    // char must be a unicode scalar value (no surrogates, <= 0x10FFFF)
    assert!(decode(&Type::Char, &unhex("80f307")).is_ok());
    let mut surrogate = Vec::new();
    crab_sdk::codec::uleb_encode(0xD800, &mut surrogate);
    assert!(decode(&Type::Char, &surrogate).is_err());
    // truncated buffer
    assert!(decode(&Type::String, &[0x05, 0x68]).is_err());
    let mut d = Decoder::new(&[]);
    assert!(d.value(&Type::U32).is_err());
}
