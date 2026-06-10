//! WIRE.md section 1 codec: component-model value encoding (subset).
//!
//! All multi-byte primitives little-endian; integers are LEB128.
//!
//! Decode rules (normative for the Lua host too):
//! - ULEB128: at most ceil(bits/7) bytes per width (u8: 2, u16: 3, u32: 5,
//!   u64: 10, char: 3 — chars cap at 0x10FFFF which is 21 bits).
//!   On the last permitted byte, any payload bits above the width must be 0,
//!   otherwise error "uleb overflow". A continuation bit on the last
//!   permitted byte is an error "uleb too long". Non-canonical encodings
//!   (zero-padded continuation bytes) are ACCEPTED within those limits.
//! - SLEB128: same max byte counts; the decoded value is range-checked
//!   against the target width (s8: -128..=127, ...). For s64 the 10th byte
//!   must be 0x00 or 0x7f.
//! - bool / option / result discriminant bytes must be exactly 0 or 1.
//! - variant/enum case index is decoded as a u32 ULEB and must be < the
//!   number of cases.
//! - flags: ceil(n/8) bytes; bits above flag n-1 in the last byte must be 0.

use crate::value::{Type, Value};

pub type Error = String;

// ---------------------------------------------------------------------------
// encode
// ---------------------------------------------------------------------------

pub fn uleb_encode(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

pub fn sleb_encode(mut v: i64, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        let sign_bit = b & 0x40 != 0;
        if (v == 0 && !sign_bit) || (v == -1 && sign_bit) {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// Encode `v` per WIRE.md section 1, appending to `out`.
pub fn encode(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Bool(b) => out.push(*b as u8),
        Value::U8(n) => uleb_encode(*n as u64, out),
        Value::U16(n) => uleb_encode(*n as u64, out),
        Value::U32(n) => uleb_encode(*n as u64, out),
        Value::U64(n) => uleb_encode(*n, out),
        Value::S8(n) => sleb_encode(*n as i64, out),
        Value::S16(n) => sleb_encode(*n as i64, out),
        Value::S32(n) => sleb_encode(*n as i64, out),
        Value::S64(n) => sleb_encode(*n, out),
        Value::F32(f) => out.extend_from_slice(&f.to_le_bytes()),
        Value::F64(f) => out.extend_from_slice(&f.to_le_bytes()),
        Value::Char(c) => uleb_encode(*c as u64, out),
        Value::String(s) => {
            uleb_encode(s.len() as u64, out);
            out.extend_from_slice(s.as_bytes());
        }
        Value::List(items) => {
            uleb_encode(items.len() as u64, out);
            for it in items {
                encode(it, out);
            }
        }
        Value::Record(fields) | Value::Tuple(fields) => {
            for f in fields {
                encode(f, out);
            }
        }
        Value::Variant { case, payload } => {
            uleb_encode(*case as u64, out);
            if let Some(p) = payload {
                encode(p, out);
            }
        }
        Value::Enum(case) => uleb_encode(*case as u64, out),
        Value::Option(o) => match o {
            None => out.push(0),
            Some(v) => {
                out.push(1);
                encode(v, out);
            }
        },
        Value::Result(r) => match r {
            Ok(v) => {
                out.push(0);
                if let Some(v) = v {
                    encode(v, out);
                }
            }
            Err(e) => {
                out.push(1);
                if let Some(e) = e {
                    encode(e, out);
                }
            }
        },
        Value::Flags(bits) => {
            let nbytes = (bits.len() + 7) / 8;
            let mut bytes = vec![0u8; nbytes];
            for (i, set) in bits.iter().enumerate() {
                if *set {
                    bytes[i / 8] |= 1 << (i % 8);
                }
            }
            out.extend_from_slice(&bytes);
        }
    }
}

/// Encode to a fresh Vec.
pub fn encode_to_vec(v: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    encode(v, &mut out);
    out
}

// ---------------------------------------------------------------------------
// decode
// ---------------------------------------------------------------------------

/// Cursor over an encoded buffer.
pub struct Decoder<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Decoder { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn byte(&mut self) -> Result<u8, Error> {
        let b = *self
            .buf
            .get(self.pos)
            .ok_or_else(|| "unexpected end of buffer".to_string())?;
        self.pos += 1;
        Ok(b)
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if self.remaining() < n {
            return Err("unexpected end of buffer".into());
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    /// ULEB128 capped at `bits` significant bits (max ceil(bits/7) bytes).
    pub fn uleb(&mut self, bits: u32) -> Result<u64, Error> {
        let max_bytes = (bits + 6) / 7;
        let mut result: u64 = 0;
        let mut shift: u32 = 0;
        for _ in 0..max_bytes {
            let b = self.byte()?;
            let payload = (b & 0x7f) as u64;
            if shift + 7 > bits && (payload >> (bits - shift)) != 0 {
                return Err("uleb overflow".into());
            }
            result |= payload << shift;
            if b & 0x80 == 0 {
                return Ok(result);
            }
            shift += 7;
        }
        Err("uleb too long".into())
    }

    /// SLEB128 capped at `bits` (max ceil(bits/7) bytes), range-checked.
    pub fn sleb(&mut self, bits: u32) -> Result<i64, Error> {
        let max_bytes = (bits + 6) / 7;
        let mut result: i64 = 0;
        let mut shift: u32 = 0;
        for _ in 0..max_bytes {
            let b = self.byte()?;
            if shift == 63 {
                // 10th byte of an s64: only sign-extension patterns fit.
                if b != 0x00 && b != 0x7f {
                    return Err("sleb overflow".into());
                }
                result |= ((b & 1) as i64) << 63;
                return Ok(result);
            }
            result |= ((b & 0x7f) as i64) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                if shift < 64 && b & 0x40 != 0 {
                    result |= -1i64 << shift; // sign-extend
                }
                if bits < 64 {
                    let min = -(1i64 << (bits - 1));
                    let max = (1i64 << (bits - 1)) - 1;
                    if result < min || result > max {
                        return Err("sleb overflow".into());
                    }
                }
                return Ok(result);
            }
        }
        Err("sleb too long".into())
    }

    fn prefix_bit(&mut self, what: &str) -> Result<bool, Error> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            b => Err(format!("invalid {what} byte: {b}")),
        }
    }

    /// Decode one value of type `ty` at the cursor.
    pub fn value(&mut self, ty: &Type) -> Result<Value, Error> {
        Ok(match ty {
            Type::Bool => Value::Bool(self.prefix_bit("bool")?),
            Type::U8 => Value::U8(self.uleb(8)? as u8),
            Type::U16 => Value::U16(self.uleb(16)? as u16),
            Type::U32 => Value::U32(self.uleb(32)? as u32),
            Type::U64 => Value::U64(self.uleb(64)?),
            Type::S8 => Value::S8(self.sleb(8)? as i8),
            Type::S16 => Value::S16(self.sleb(16)? as i16),
            Type::S32 => Value::S32(self.sleb(32)? as i32),
            Type::S64 => Value::S64(self.sleb(64)?),
            Type::F32 => {
                let b: [u8; 4] = self.bytes(4)?.try_into().unwrap();
                Value::F32(f32::from_le_bytes(b))
            }
            Type::F64 => {
                let b: [u8; 8] = self.bytes(8)?.try_into().unwrap();
                Value::F64(f64::from_le_bytes(b))
            }
            Type::Char => {
                let scalar = self.uleb(21)? as u32;
                Value::Char(
                    char::from_u32(scalar)
                        .ok_or_else(|| format!("invalid char scalar: {scalar}"))?,
                )
            }
            Type::String => {
                let len = self.uleb(32)? as usize;
                let bytes = self.bytes(len)?;
                Value::String(
                    std::str::from_utf8(bytes)
                        .map_err(|e| format!("invalid utf-8 in string: {e}"))?
                        .to_string(),
                )
            }
            Type::List(elem) => {
                let count = self.uleb(32)? as usize;
                let mut items = Vec::with_capacity(count.min(4096));
                for _ in 0..count {
                    items.push(self.value(elem)?);
                }
                Value::List(items)
            }
            Type::Record(fields) => {
                let mut vals = Vec::with_capacity(fields.len());
                for f in fields {
                    vals.push(self.value(f)?);
                }
                Value::Record(vals)
            }
            Type::Tuple(members) => {
                let mut vals = Vec::with_capacity(members.len());
                for m in members {
                    vals.push(self.value(m)?);
                }
                Value::Tuple(vals)
            }
            Type::Variant(cases) => {
                let case = self.uleb(32)? as u32;
                let payload_ty = cases
                    .get(case as usize)
                    .ok_or_else(|| format!("variant case out of range: {case}"))?;
                let payload = match payload_ty {
                    Some(t) => Some(Box::new(self.value(t)?)),
                    None => None,
                };
                Value::Variant { case, payload }
            }
            Type::Enum(n) => {
                let case = self.uleb(32)? as u32;
                if case >= *n {
                    return Err(format!("enum case out of range: {case}"));
                }
                Value::Enum(case)
            }
            Type::Option(inner) => {
                if self.prefix_bit("option")? {
                    Value::Option(Some(Box::new(self.value(inner)?)))
                } else {
                    Value::Option(None)
                }
            }
            Type::Result { ok, err } => {
                if self.prefix_bit("result")? {
                    let e = match err {
                        Some(t) => Some(Box::new(self.value(t)?)),
                        None => None,
                    };
                    Value::Result(Err(e))
                } else {
                    let o = match ok {
                        Some(t) => Some(Box::new(self.value(t)?)),
                        None => None,
                    };
                    Value::Result(Ok(o))
                }
            }
            Type::Flags(n) => {
                let n = *n as usize;
                let nbytes = (n + 7) / 8;
                let bytes = self.bytes(nbytes)?;
                if n % 8 != 0 {
                    let unused = bytes[nbytes - 1] >> (n % 8);
                    if unused != 0 {
                        return Err("flags: unused high bits set".into());
                    }
                }
                let mut bits = Vec::with_capacity(n);
                for i in 0..n {
                    bits.push(bytes[i / 8] & (1 << (i % 8)) != 0);
                }
                Value::Flags(bits)
            }
        })
    }
}

/// Decode exactly one `ty` from `buf`; errors on trailing bytes.
pub fn decode(ty: &Type, buf: &[u8]) -> Result<Value, Error> {
    let mut d = Decoder::new(buf);
    let v = d.value(ty)?;
    if d.remaining() != 0 {
        return Err(format!("{} trailing byte(s) after value", d.remaining()));
    }
    Ok(v)
}

/// Decode a params buffer: each parameter in declaration order, concatenated.
/// Errors on trailing bytes.
pub fn decode_params(tys: &[Type], buf: &[u8]) -> Result<Vec<Value>, Error> {
    let mut d = Decoder::new(buf);
    let mut vals = Vec::with_capacity(tys.len());
    for (i, ty) in tys.iter().enumerate() {
        vals.push(
            d.value(ty)
                .map_err(|e| format!("param {i}: {e}"))?,
        );
    }
    if d.remaining() != 0 {
        return Err(format!("{} trailing byte(s) after params", d.remaining()));
    }
    Ok(vals)
}
