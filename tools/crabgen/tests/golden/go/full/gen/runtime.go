// crabcraft Go runtime: WIRE.md section-1 value codec + section-2 guest ABI.
//
// This file is a crabgen TEMPLATE shared by every generated Go guest; it
// contains no per-WIT content. Generated bindings (gen/bindings.go) call the
// Encode*/Decoder primitives in straight-line code — no reflection, no
// dynamic value tree — set Schema from a go:embed of schema.json, and
// populate handlers in init().
//
// Build compatibility: the //go:wasmexport directive is understood by both
// TinyGo (wasip1 reactor builds, `-buildmode=c-shared`) and plain go 1.24+
// (GOOS=wasip1). On non-wasm hosts plain go IGNORES the directive (verified
// on go 1.25 darwin: build + vet are clean), so this whole file compiles for
// host-go unit tests too — no build-tag split is needed here. The only
// wasm-only construct, the //go:wasmimport mesh declaration, lives in
// mesh_wasm.go behind a `//go:build wasip1` tag.
//
// Note on `go vet`: the explicit `unsafeptr` analyzer flags the
// integer-address -> unsafe.Pointer conversions in the ABI functions below.
// Those are inherent to the wasm ABI (the host passes raw linear-memory
// addresses) and are correct on wasm; `go test`'s default vet subset does
// not include unsafeptr, and a full vet pass is clean with
// `go vet -unsafeptr=false ./...`.
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
package gen

import (
	"errors"
	"math"
	"strconv"
	"unicode/utf8"
	"unsafe"
)

// ---------------------------------------------------------------------------
// WIRE.md section 1: encode (append-style primitives)
// ---------------------------------------------------------------------------

// EncodeUleb appends the unsigned LEB128 encoding of v.
func EncodeUleb(out []byte, v uint64) []byte {
	for {
		b := byte(v & 0x7f)
		v >>= 7
		if v == 0 {
			return append(out, b)
		}
		out = append(out, b|0x80)
	}
}

// EncodeSleb appends the signed LEB128 encoding of v.
func EncodeSleb(out []byte, v int64) []byte {
	for {
		b := byte(v & 0x7f)
		v >>= 7
		signBit := b&0x40 != 0
		if (v == 0 && !signBit) || (v == -1 && signBit) {
			return append(out, b)
		}
		out = append(out, b|0x80)
	}
}

// EncodeBool appends 1 byte: 0 or 1.
func EncodeBool(out []byte, v bool) []byte {
	if v {
		return append(out, 1)
	}
	return append(out, 0)
}

// EncodeU8 appends uleb(v).
func EncodeU8(out []byte, v uint8) []byte { return EncodeUleb(out, uint64(v)) }

// EncodeU16 appends uleb(v).
func EncodeU16(out []byte, v uint16) []byte { return EncodeUleb(out, uint64(v)) }

// EncodeU32 appends uleb(v).
func EncodeU32(out []byte, v uint32) []byte { return EncodeUleb(out, uint64(v)) }

// EncodeU64 appends uleb(v).
func EncodeU64(out []byte, v uint64) []byte { return EncodeUleb(out, v) }

// EncodeS8 appends sleb(v).
func EncodeS8(out []byte, v int8) []byte { return EncodeSleb(out, int64(v)) }

// EncodeS16 appends sleb(v).
func EncodeS16(out []byte, v int16) []byte { return EncodeSleb(out, int64(v)) }

// EncodeS32 appends sleb(v).
func EncodeS32(out []byte, v int32) []byte { return EncodeSleb(out, int64(v)) }

// EncodeS64 appends sleb(v).
func EncodeS64(out []byte, v int64) []byte { return EncodeSleb(out, v) }

// EncodeF32 appends 4 bytes IEEE-754 LE.
func EncodeF32(out []byte, v float32) []byte {
	bits := math.Float32bits(v)
	return append(out, byte(bits), byte(bits>>8), byte(bits>>16), byte(bits>>24))
}

// EncodeF64 appends 8 bytes IEEE-754 LE.
func EncodeF64(out []byte, v float64) []byte {
	bits := math.Float64bits(v)
	return append(out, byte(bits), byte(bits>>8), byte(bits>>16), byte(bits>>24),
		byte(bits>>32), byte(bits>>40), byte(bits>>48), byte(bits>>56))
}

// EncodeChar appends uleb(unicode scalar value).
func EncodeChar(out []byte, v rune) []byte { return EncodeUleb(out, uint64(uint32(v))) }

// EncodeString appends uleb(byte length) + UTF-8 bytes.
func EncodeString(out []byte, s string) []byte {
	out = EncodeUleb(out, uint64(len(s)))
	return append(out, s...)
}

// EncodeListLen appends uleb(count); the caller then appends each element.
func EncodeListLen(out []byte, n int) []byte { return EncodeUleb(out, uint64(n)) }

// EncodeCase appends uleb(case index) for variants and enums; for a variant
// the caller then appends the payload if the case has one. Records and
// tuples have no header at all: just encode each member in order.
func EncodeCase(out []byte, c uint32) []byte { return EncodeUleb(out, uint64(c)) }

// EncodeOptionTag appends the option discriminant (0 = none, 1 = some); the
// caller then appends the inner value when present.
func EncodeOptionTag(out []byte, present bool) []byte { return EncodeBool(out, present) }

// EncodeResultTag appends the result discriminant (0 = ok, 1 = err); the
// caller then appends the payload if that side has a type.
func EncodeResultTag(out []byte, isErr bool) []byte { return EncodeBool(out, isErr) }

// EncodeFlags appends ceil(len(bits)/8) bytes, bit i = flag i, LE byte order.
func EncodeFlags(out []byte, bits []bool) []byte {
	nbytes := (len(bits) + 7) / 8
	start := len(out)
	for i := 0; i < nbytes; i++ {
		out = append(out, 0)
	}
	for i, set := range bits {
		if set {
			out[start+i/8] |= 1 << (i % 8)
		}
	}
	return out
}

// ---------------------------------------------------------------------------
// WIRE.md section 1: decode
// ---------------------------------------------------------------------------

var (
	errEOF         = errors.New("unexpected end of buffer")
	errUlebOver    = errors.New("uleb overflow")
	errUlebLong    = errors.New("uleb too long")
	errSlebOver    = errors.New("sleb overflow")
	errSlebLong    = errors.New("sleb too long")
	errBadUTF8     = errors.New("invalid utf-8 in string")
	errFlagsUnused = errors.New("flags: unused high bits set")
)

// Decoder is a cursor over an encoded buffer.
type Decoder struct {
	buf []byte
	pos int
}

// NewDecoder returns a Decoder positioned at the start of buf.
func NewDecoder(buf []byte) *Decoder { return &Decoder{buf: buf} }

// Remaining reports the number of undecoded bytes.
func (d *Decoder) Remaining() int { return len(d.buf) - d.pos }

// Finish errors unless the whole buffer was consumed; params decoding (and
// single-value decoding) must always end with a Finish check.
func (d *Decoder) Finish(what string) error {
	if n := d.Remaining(); n != 0 {
		return errors.New(strconv.Itoa(n) + " trailing byte(s) after " + what)
	}
	return nil
}

func (d *Decoder) byte() (byte, error) {
	if d.pos >= len(d.buf) {
		return 0, errEOF
	}
	b := d.buf[d.pos]
	d.pos++
	return b, nil
}

func (d *Decoder) bytes(n int) ([]byte, error) {
	if n < 0 || d.Remaining() < n {
		return nil, errEOF
	}
	s := d.buf[d.pos : d.pos+n]
	d.pos += n
	return s, nil
}

// Uleb decodes an unsigned LEB128 capped at `bits` significant bits
// (max ceil(bits/7) bytes; payload bits above the width on the last
// permitted byte must be zero).
func (d *Decoder) Uleb(bits uint) (uint64, error) {
	maxBytes := (bits + 6) / 7
	var result uint64
	var shift uint
	for i := uint(0); i < maxBytes; i++ {
		b, err := d.byte()
		if err != nil {
			return 0, err
		}
		payload := uint64(b & 0x7f)
		if shift+7 > bits && payload>>(bits-shift) != 0 {
			return 0, errUlebOver
		}
		result |= payload << shift
		if b&0x80 == 0 {
			return result, nil
		}
		shift += 7
	}
	return 0, errUlebLong
}

// Sleb decodes a signed LEB128 capped at `bits` (max ceil(bits/7) bytes),
// range-checked against the width; the 10th byte of an s64 must be 0x00 or
// 0x7f (only sign-extension patterns fit).
func (d *Decoder) Sleb(bits uint) (int64, error) {
	maxBytes := (bits + 6) / 7
	var result int64
	var shift uint
	for i := uint(0); i < maxBytes; i++ {
		b, err := d.byte()
		if err != nil {
			return 0, err
		}
		if shift == 63 {
			if b != 0x00 && b != 0x7f {
				return 0, errSlebOver
			}
			result |= int64(b&1) << 63
			return result, nil
		}
		result |= int64(b&0x7f) << shift
		shift += 7
		if b&0x80 == 0 {
			if shift < 64 && b&0x40 != 0 {
				result |= -1 << shift // sign-extend
			}
			if bits < 64 {
				min := -(int64(1) << (bits - 1))
				max := int64(1)<<(bits-1) - 1
				if result < min || result > max {
					return 0, errSlebOver
				}
			}
			return result, nil
		}
	}
	return 0, errSlebLong
}

func (d *Decoder) prefixBit(what string) (bool, error) {
	b, err := d.byte()
	if err != nil {
		return false, err
	}
	switch b {
	case 0:
		return false, nil
	case 1:
		return true, nil
	}
	return false, errors.New("invalid " + what + " byte: " + strconv.Itoa(int(b)))
}

// Bool decodes a strict 0/1 byte.
func (d *Decoder) Bool() (bool, error) { return d.prefixBit("bool") }

// U8 decodes a uleb capped at 8 bits.
func (d *Decoder) U8() (uint8, error) {
	v, err := d.Uleb(8)
	return uint8(v), err
}

// U16 decodes a uleb capped at 16 bits.
func (d *Decoder) U16() (uint16, error) {
	v, err := d.Uleb(16)
	return uint16(v), err
}

// U32 decodes a uleb capped at 32 bits.
func (d *Decoder) U32() (uint32, error) {
	v, err := d.Uleb(32)
	return uint32(v), err
}

// U64 decodes a uleb capped at 64 bits.
func (d *Decoder) U64() (uint64, error) { return d.Uleb(64) }

// S8 decodes a sleb range-checked to 8 bits.
func (d *Decoder) S8() (int8, error) {
	v, err := d.Sleb(8)
	return int8(v), err
}

// S16 decodes a sleb range-checked to 16 bits.
func (d *Decoder) S16() (int16, error) {
	v, err := d.Sleb(16)
	return int16(v), err
}

// S32 decodes a sleb range-checked to 32 bits.
func (d *Decoder) S32() (int32, error) {
	v, err := d.Sleb(32)
	return int32(v), err
}

// S64 decodes a sleb capped at 64 bits.
func (d *Decoder) S64() (int64, error) { return d.Sleb(64) }

// F32 decodes 4 bytes IEEE-754 LE.
func (d *Decoder) F32() (float32, error) {
	b, err := d.bytes(4)
	if err != nil {
		return 0, err
	}
	bits := uint32(b[0]) | uint32(b[1])<<8 | uint32(b[2])<<16 | uint32(b[3])<<24
	return math.Float32frombits(bits), nil
}

// F64 decodes 8 bytes IEEE-754 LE.
func (d *Decoder) F64() (float64, error) {
	b, err := d.bytes(8)
	if err != nil {
		return 0, err
	}
	bits := uint64(b[0]) | uint64(b[1])<<8 | uint64(b[2])<<16 | uint64(b[3])<<24 |
		uint64(b[4])<<32 | uint64(b[5])<<40 | uint64(b[6])<<48 | uint64(b[7])<<56
	return math.Float64frombits(bits), nil
}

// Char decodes a uleb scalar (21-bit cap) and validates it is a unicode
// scalar value (<= 0x10FFFF and not a surrogate).
func (d *Decoder) Char() (rune, error) {
	v, err := d.Uleb(21)
	if err != nil {
		return 0, err
	}
	r := rune(v)
	if !utf8.ValidRune(r) {
		return 0, errors.New("invalid char scalar: " + strconv.FormatUint(v, 10))
	}
	return r, nil
}

// String decodes uleb(byte length) + UTF-8 bytes, validating the UTF-8.
func (d *Decoder) String() (string, error) {
	n, err := d.Uleb(32)
	if err != nil {
		return "", err
	}
	b, err := d.bytes(int(n))
	if err != nil {
		return "", err
	}
	if !utf8.Valid(b) {
		return "", errBadUTF8
	}
	return string(b), nil
}

// ListLen decodes uleb(count); the caller then decodes each element. The
// count is attacker-controlled, so callers must clamp pre-allocation
// (crab-sdk caps initial capacity at 4096) and let element decoding fail
// naturally on short buffers.
func (d *Decoder) ListLen() (int, error) {
	v, err := d.Uleb(32)
	return int(v), err
}

// OptionTag decodes the option discriminant (false = none, true = some);
// the caller then decodes the inner value when present.
func (d *Decoder) OptionTag() (bool, error) { return d.prefixBit("option") }

// ResultTag decodes the result discriminant (false = ok, true = err); the
// caller then decodes the payload if that side has a type.
func (d *Decoder) ResultTag() (bool, error) { return d.prefixBit("result") }

// VariantCase decodes a u32-uleb case index and bounds-checks it; the
// caller then decodes the payload if the case has one.
func (d *Decoder) VariantCase(numCases uint32) (uint32, error) {
	v, err := d.Uleb(32)
	if err != nil {
		return 0, err
	}
	c := uint32(v)
	if c >= numCases {
		return 0, errors.New("variant case out of range: " + strconv.FormatUint(v, 10))
	}
	return c, nil
}

// EnumCase decodes a u32-uleb case index and bounds-checks it.
func (d *Decoder) EnumCase(numCases uint32) (uint32, error) {
	v, err := d.Uleb(32)
	if err != nil {
		return 0, err
	}
	c := uint32(v)
	if c >= numCases {
		return 0, errors.New("enum case out of range: " + strconv.FormatUint(v, 10))
	}
	return c, nil
}

// Flags decodes ceil(n/8) bytes into one bool per flag (bit i = flag i, LE
// byte order); bits above flag n-1 in the last byte must be zero.
func (d *Decoder) Flags(n int) ([]bool, error) {
	nbytes := (n + 7) / 8
	b, err := d.bytes(nbytes)
	if err != nil {
		return nil, err
	}
	if n%8 != 0 && b[nbytes-1]>>(n%8) != 0 {
		return nil, errFlagsUnused
	}
	bits := make([]bool, n)
	for i := 0; i < n; i++ {
		bits[i] = b[i/8]&(1<<(i%8)) != 0
	}
	return bits, nil
}

// ---------------------------------------------------------------------------
// WIRE.md section 2: guest ABI (crab_alloc / crab_schema / crab_invoke)
// ---------------------------------------------------------------------------

// Schema holds the resolved-WIT JSON served by crab_schema; the generated
// bindings set it from a go:embed of gen/schema.json.
var Schema string

// handlers maps function addresses (`<instance>#<function>`) to handlers; a
// handler decodes its params from the Decoder (including the trailing-bytes
// Finish check) and returns the encoded result value. Generated bindings
// populate the map in init().
var handlers = map[string]func(*Decoder) ([]byte, error){}

// allocs pins host-written buffers: TinyGo's GC is non-moving, but buffers
// must stay REFERENCED or they may be collected out from under the host.
var allocs = map[uintptr][]byte{}

// reply holds the current LENBUF; valid until the next crab_invoke /
// crab_schema call (the host copies immediately), per WIRE.md.
var reply []byte

// lenbuf builds [u32 LE length][payload] in the static reply buffer and
// returns its address.
func lenbuf(payload []byte) uint32 {
	reply = reply[:0]
	n := uint32(len(payload))
	reply = append(reply, byte(n), byte(n>>8), byte(n>>16), byte(n>>24))
	reply = append(reply, payload...)
	return uint32(uintptr(unsafe.Pointer(&reply[0])))
}

// replyOK builds the [status=0][encoded result] reply.
func replyOK(result []byte) uint32 {
	payload := make([]byte, 0, 1+len(result))
	payload = append(payload, 0)
	payload = append(payload, result...)
	return lenbuf(payload)
}

// replyErr builds the [status=1][string message] reply.
func replyErr(msg string) uint32 {
	payload := make([]byte, 0, 2+len(msg))
	payload = append(payload, 1)
	payload = EncodeString(payload, msg)
	return lenbuf(payload)
}

//go:wasmexport crab_alloc
func crabAlloc(length int32) uint32 {
	if length < 1 {
		length = 1
	}
	buf := make([]byte, length)
	p := uintptr(unsafe.Pointer(&buf[0]))
	allocs[p] = buf
	return uint32(p)
}

//go:wasmexport crab_schema
func crabSchema() uint32 {
	return lenbuf([]byte(Schema))
}

//go:wasmexport crab_invoke
func crabInvoke(namePtr, nameLen, argPtr, argLen int32) uint32 {
	// Unpin the host-written name/args buffers when this invoke returns:
	// warm reactors would otherwise leak one pin per call. Safe because the
	// name is copied into a string, the handler has finished by the time the
	// deferred deletes run, and the reply lives in its own buffer.
	defer delete(allocs, uintptr(uint32(namePtr)))
	defer delete(allocs, uintptr(uint32(argPtr)))
	name := string(unsafe.Slice((*byte)(unsafe.Pointer(uintptr(uint32(namePtr)))), int(nameLen)))
	h, ok := handlers[name]
	if !ok {
		return replyErr("unknown function: " + name)
	}
	var args []byte
	if argLen > 0 {
		args = unsafe.Slice((*byte)(unsafe.Pointer(uintptr(uint32(argPtr)))), int(argLen))
	}
	result, err := h(NewDecoder(args))
	if err != nil {
		return replyErr(name + ": " + err.Error())
	}
	return replyOK(result)
}
