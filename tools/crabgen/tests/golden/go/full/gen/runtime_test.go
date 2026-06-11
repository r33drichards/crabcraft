// Conformance test for the crabcraft Go runtime codec (runtime.go) against
// the shared WIRE.md section-1 vectors (wit/vectors.json). This file is part
// of the crabgen Go template set and is copied verbatim into generated
// projects, so it must stay self-contained (stdlib only, package gen).
//
// CRAB_VECTORS must point at vectors.json. Each vector carries a JSON type
// descriptor, a JSON value (conventions documented in
// guest/crab-sdk/src/vectors.rs), and the expected lowercase-hex encoding.
// For every vector we assert:
//  1. encoding the JSON value per the descriptor yields exactly `hex`,
//  2. decoding `hex` consumes the whole buffer (Finish == nil),
//  3. re-encoding the decoded value yields exactly `hex` (byte round-trip),
//  4. for scalar/string types the decoded value equals the JSON value
//     (decimal-string convention for u64/s64 beyond 2^53).
package gen

import (
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"strconv"
	"strings"
	"testing"
)

// ---------------------------------------------------------------------------
// vectors.json model (test-only; interface{} trees are fine here)
// ---------------------------------------------------------------------------

type vector struct {
	Desc  string          `json:"desc"`
	Type  json.RawMessage `json:"type"`
	Value json.RawMessage `json:"value"`
	Hex   string          `json:"hex"`
}

// tdesc mirrors the JSON type descriptors used in vectors.json.
type tdesc struct {
	Kind    string `json:"kind"`
	Element *tdesc `json:"element"` // list
	Fields  []struct {
		Name string `json:"name"`
		Type *tdesc `json:"type"`
	} `json:"fields"` // record
	Members []*tdesc        `json:"members"` // tuple
	Cases   json.RawMessage `json:"cases"`   // enum: []string; variant: []{name,payload}
	Inner   *tdesc          `json:"inner"`   // option
	Ok      *tdesc          `json:"ok"`      // result
	Err     *tdesc          `json:"err"`     // result
	Count   int             `json:"count"`   // flags
}

func (t *tdesc) variantCases(tb testing.TB) []struct {
	Name    string `json:"name"`
	Payload *tdesc `json:"payload"`
} {
	var cases []struct {
		Name    string `json:"name"`
		Payload *tdesc `json:"payload"`
	}
	if err := json.Unmarshal(t.Cases, &cases); err != nil {
		tb.Fatalf("bad variant cases %s: %v", t.Cases, err)
	}
	return cases
}

func (t *tdesc) enumCount(tb testing.TB) uint32 {
	var names []string
	if err := json.Unmarshal(t.Cases, &names); err != nil {
		tb.Fatalf("bad enum cases %s: %v", t.Cases, err)
	}
	return uint32(len(names))
}

// numLit returns the raw JSON token as a bare literal: numbers verbatim,
// strings unquoted (the decimal-string convention for big u64/s64).
func numLit(tb testing.TB, raw json.RawMessage) string {
	s := strings.TrimSpace(string(raw))
	if strings.HasPrefix(s, "\"") {
		var unq string
		if err := json.Unmarshal(raw, &unq); err != nil {
			tb.Fatalf("bad numeric string %s: %v", raw, err)
		}
		return unq
	}
	return s
}

func jsonString(tb testing.TB, raw json.RawMessage) string {
	var s string
	if err := json.Unmarshal(raw, &s); err != nil {
		tb.Fatalf("expected JSON string, got %s: %v", raw, err)
	}
	return s
}

func isNull(raw json.RawMessage) bool {
	return strings.TrimSpace(string(raw)) == "null"
}

// ---------------------------------------------------------------------------
// JSON value -> wire bytes (drives the Encode* primitives)
// ---------------------------------------------------------------------------

func encodeJSON(tb testing.TB, out []byte, t *tdesc, raw json.RawMessage) []byte {
	switch t.Kind {
	case "bool":
		var b bool
		if err := json.Unmarshal(raw, &b); err != nil {
			tb.Fatalf("bad bool %s: %v", raw, err)
		}
		return EncodeBool(out, b)
	case "u8", "u16", "u32", "u64":
		bits, _ := strconv.Atoi(t.Kind[1:])
		v, err := strconv.ParseUint(numLit(tb, raw), 10, bits)
		if err != nil {
			tb.Fatalf("bad %s %s: %v", t.Kind, raw, err)
		}
		switch t.Kind {
		case "u8":
			return EncodeU8(out, uint8(v))
		case "u16":
			return EncodeU16(out, uint16(v))
		case "u32":
			return EncodeU32(out, uint32(v))
		default:
			return EncodeU64(out, v)
		}
	case "s8", "s16", "s32", "s64":
		bits, _ := strconv.Atoi(t.Kind[1:])
		v, err := strconv.ParseInt(numLit(tb, raw), 10, bits)
		if err != nil {
			tb.Fatalf("bad %s %s: %v", t.Kind, raw, err)
		}
		switch t.Kind {
		case "s8":
			return EncodeS8(out, int8(v))
		case "s16":
			return EncodeS16(out, int16(v))
		case "s32":
			return EncodeS32(out, int32(v))
		default:
			return EncodeS64(out, v)
		}
	case "f32":
		v, err := strconv.ParseFloat(numLit(tb, raw), 32)
		if err != nil {
			tb.Fatalf("bad f32 %s: %v", raw, err)
		}
		return EncodeF32(out, float32(v))
	case "f64":
		v, err := strconv.ParseFloat(numLit(tb, raw), 64)
		if err != nil {
			tb.Fatalf("bad f64 %s: %v", raw, err)
		}
		return EncodeF64(out, v)
	case "char":
		runes := []rune(jsonString(tb, raw))
		if len(runes) != 1 {
			tb.Fatalf("char value must be one rune, got %s", raw)
		}
		return EncodeChar(out, runes[0])
	case "string":
		return EncodeString(out, jsonString(tb, raw))
	case "list":
		var items []json.RawMessage
		if err := json.Unmarshal(raw, &items); err != nil {
			tb.Fatalf("bad list %s: %v", raw, err)
		}
		out = EncodeListLen(out, len(items))
		for _, it := range items {
			out = encodeJSON(tb, out, t.Element, it)
		}
		return out
	case "record":
		var obj map[string]json.RawMessage
		if err := json.Unmarshal(raw, &obj); err != nil {
			tb.Fatalf("bad record %s: %v", raw, err)
		}
		for _, f := range t.Fields {
			fv, ok := obj[f.Name]
			if !ok {
				fv = json.RawMessage("null")
			}
			out = encodeJSON(tb, out, f.Type, fv)
		}
		return out
	case "tuple":
		var items []json.RawMessage
		if err := json.Unmarshal(raw, &items); err != nil {
			tb.Fatalf("bad tuple %s: %v", raw, err)
		}
		if len(items) != len(t.Members) {
			tb.Fatalf("tuple arity mismatch: %d values, %d members", len(items), len(t.Members))
		}
		for i, m := range t.Members {
			out = encodeJSON(tb, out, m, items[i])
		}
		return out
	case "variant":
		var v struct {
			Case    uint32          `json:"case"`
			Payload json.RawMessage `json:"payload"`
		}
		if err := json.Unmarshal(raw, &v); err != nil {
			tb.Fatalf("bad variant %s: %v", raw, err)
		}
		cases := t.variantCases(tb)
		out = EncodeCase(out, v.Case)
		if pt := cases[v.Case].Payload; pt != nil {
			out = encodeJSON(tb, out, pt, v.Payload)
		}
		return out
	case "enum":
		var c uint32
		if err := json.Unmarshal(raw, &c); err != nil {
			tb.Fatalf("bad enum %s: %v", raw, err)
		}
		return EncodeCase(out, c)
	case "option":
		if isNull(raw) {
			return EncodeOptionTag(out, false)
		}
		out = EncodeOptionTag(out, true)
		return encodeJSON(tb, out, t.Inner, raw)
	case "result":
		var v map[string]json.RawMessage
		if err := json.Unmarshal(raw, &v); err != nil {
			tb.Fatalf("bad result %s: %v", raw, err)
		}
		if pv, ok := v["ok"]; ok {
			out = EncodeResultTag(out, false)
			if t.Ok != nil {
				out = encodeJSON(tb, out, t.Ok, pv)
			}
			return out
		}
		out = EncodeResultTag(out, true)
		if t.Err != nil {
			out = encodeJSON(tb, out, t.Err, v["err"])
		}
		return out
	case "flags":
		var set []int
		if err := json.Unmarshal(raw, &set); err != nil {
			tb.Fatalf("bad flags %s: %v", raw, err)
		}
		bits := make([]bool, t.Count)
		for _, i := range set {
			bits[i] = true
		}
		return EncodeFlags(out, bits)
	}
	tb.Fatalf("unknown type kind %q", t.Kind)
	return nil
}

// ---------------------------------------------------------------------------
// wire bytes -> value tree (drives the Decoder), then back to bytes
// ---------------------------------------------------------------------------

// decoded is a test-only dynamic value: scalars hold the native Go type,
// composites hold the shapes built below. Re-encoding it must reproduce the
// input bytes exactly.
type dVariant struct {
	c       uint32
	payload interface{} // nil when the case has no payload type
	hasPay  bool
}
type dOption struct {
	some  bool
	inner interface{}
}
type dResult struct {
	isErr   bool
	payload interface{}
	hasPay  bool
}

func decodeValue(d *Decoder, t *tdesc, tb testing.TB) (interface{}, error) {
	switch t.Kind {
	case "bool":
		return d.Bool()
	case "u8":
		return d.U8()
	case "u16":
		return d.U16()
	case "u32":
		return d.U32()
	case "u64":
		return d.U64()
	case "s8":
		return d.S8()
	case "s16":
		return d.S16()
	case "s32":
		return d.S32()
	case "s64":
		return d.S64()
	case "f32":
		return d.F32()
	case "f64":
		return d.F64()
	case "char":
		return d.Char()
	case "string":
		return d.String()
	case "list":
		n, err := d.ListLen()
		if err != nil {
			return nil, err
		}
		items := make([]interface{}, 0, n)
		for i := 0; i < n; i++ {
			it, err := decodeValue(d, t.Element, tb)
			if err != nil {
				return nil, err
			}
			items = append(items, it)
		}
		return items, nil
	case "record":
		fields := make([]interface{}, 0, len(t.Fields))
		for _, f := range t.Fields {
			fv, err := decodeValue(d, f.Type, tb)
			if err != nil {
				return nil, err
			}
			fields = append(fields, fv)
		}
		return fields, nil
	case "tuple":
		members := make([]interface{}, 0, len(t.Members))
		for _, m := range t.Members {
			mv, err := decodeValue(d, m, tb)
			if err != nil {
				return nil, err
			}
			members = append(members, mv)
		}
		return members, nil
	case "variant":
		cases := t.variantCases(tb)
		c, err := d.VariantCase(uint32(len(cases)))
		if err != nil {
			return nil, err
		}
		v := dVariant{c: c}
		if pt := cases[c].Payload; pt != nil {
			p, err := decodeValue(d, pt, tb)
			if err != nil {
				return nil, err
			}
			v.payload, v.hasPay = p, true
		}
		return v, nil
	case "enum":
		return d.EnumCase(t.enumCount(tb))
	case "option":
		some, err := d.OptionTag()
		if err != nil {
			return nil, err
		}
		o := dOption{some: some}
		if some {
			inner, err := decodeValue(d, t.Inner, tb)
			if err != nil {
				return nil, err
			}
			o.inner = inner
		}
		return o, nil
	case "result":
		isErr, err := d.ResultTag()
		if err != nil {
			return nil, err
		}
		r := dResult{isErr: isErr}
		pt := t.Ok
		if isErr {
			pt = t.Err
		}
		if pt != nil {
			p, err := decodeValue(d, pt, tb)
			if err != nil {
				return nil, err
			}
			r.payload, r.hasPay = p, true
		}
		return r, nil
	case "flags":
		return d.Flags(t.Count)
	}
	tb.Fatalf("unknown type kind %q", t.Kind)
	return nil, nil
}

func reencodeValue(out []byte, t *tdesc, v interface{}, tb testing.TB) []byte {
	switch t.Kind {
	case "bool":
		return EncodeBool(out, v.(bool))
	case "u8":
		return EncodeU8(out, v.(uint8))
	case "u16":
		return EncodeU16(out, v.(uint16))
	case "u32":
		return EncodeU32(out, v.(uint32))
	case "u64":
		return EncodeU64(out, v.(uint64))
	case "s8":
		return EncodeS8(out, v.(int8))
	case "s16":
		return EncodeS16(out, v.(int16))
	case "s32":
		return EncodeS32(out, v.(int32))
	case "s64":
		return EncodeS64(out, v.(int64))
	case "f32":
		return EncodeF32(out, v.(float32))
	case "f64":
		return EncodeF64(out, v.(float64))
	case "char":
		return EncodeChar(out, v.(rune))
	case "string":
		return EncodeString(out, v.(string))
	case "list":
		items := v.([]interface{})
		out = EncodeListLen(out, len(items))
		for _, it := range items {
			out = reencodeValue(out, t.Element, it, tb)
		}
		return out
	case "record":
		fields := v.([]interface{})
		for i, f := range t.Fields {
			out = reencodeValue(out, f.Type, fields[i], tb)
		}
		return out
	case "tuple":
		members := v.([]interface{})
		for i, m := range t.Members {
			out = reencodeValue(out, m, members[i], tb)
		}
		return out
	case "variant":
		vv := v.(dVariant)
		cases := t.variantCases(tb)
		out = EncodeCase(out, vv.c)
		if vv.hasPay {
			out = reencodeValue(out, cases[vv.c].Payload, vv.payload, tb)
		}
		return out
	case "enum":
		return EncodeCase(out, v.(uint32))
	case "option":
		o := v.(dOption)
		out = EncodeOptionTag(out, o.some)
		if o.some {
			out = reencodeValue(out, t.Inner, o.inner, tb)
		}
		return out
	case "result":
		r := v.(dResult)
		out = EncodeResultTag(out, r.isErr)
		if r.hasPay {
			pt := t.Ok
			if r.isErr {
				pt = t.Err
			}
			out = reencodeValue(out, pt, r.payload, tb)
		}
		return out
	case "flags":
		return EncodeFlags(out, v.([]bool))
	}
	tb.Fatalf("unknown type kind %q", t.Kind)
	return nil
}

// expectScalar checks decoded-value equality for scalar/string kinds, per
// the JSON value conventions (decimal strings for big u64/s64, one-rune
// strings for char). Composite kinds are covered byte-exactly by the
// encode/re-encode assertions.
func expectScalar(tb testing.TB, t *tdesc, raw json.RawMessage, got interface{}) {
	switch t.Kind {
	case "bool":
		var want bool
		if err := json.Unmarshal(raw, &want); err != nil {
			tb.Fatalf("bad bool %s: %v", raw, err)
		}
		if got.(bool) != want {
			tb.Fatalf("decoded %v, want %v", got, want)
		}
	case "u8", "u16", "u32", "u64":
		want, err := strconv.ParseUint(numLit(tb, raw), 10, 64)
		if err != nil {
			tb.Fatalf("bad %s %s: %v", t.Kind, raw, err)
		}
		var g uint64
		switch x := got.(type) {
		case uint8:
			g = uint64(x)
		case uint16:
			g = uint64(x)
		case uint32:
			g = uint64(x)
		case uint64:
			g = x
		}
		if g != want {
			tb.Fatalf("decoded %d, want %d", g, want)
		}
	case "s8", "s16", "s32", "s64":
		want, err := strconv.ParseInt(numLit(tb, raw), 10, 64)
		if err != nil {
			tb.Fatalf("bad %s %s: %v", t.Kind, raw, err)
		}
		var g int64
		switch x := got.(type) {
		case int8:
			g = int64(x)
		case int16:
			g = int64(x)
		case int32:
			g = int64(x)
		case int64:
			g = x
		}
		if g != want {
			tb.Fatalf("decoded %d, want %d", g, want)
		}
	case "f32", "f64":
		want, err := strconv.ParseFloat(numLit(tb, raw), 64)
		if err != nil {
			tb.Fatalf("bad %s %s: %v", t.Kind, raw, err)
		}
		var g float64
		switch x := got.(type) {
		case float32:
			g = float64(x)
		case float64:
			g = x
		}
		if g != want {
			tb.Fatalf("decoded %v, want %v", g, want)
		}
	case "char":
		want := []rune(jsonString(tb, raw))
		if got.(rune) != want[0] {
			tb.Fatalf("decoded %q, want %q", got.(rune), want[0])
		}
	case "string":
		want := jsonString(tb, raw)
		if got.(string) != want {
			tb.Fatalf("decoded %q, want %q", got, want)
		}
	}
}

func isScalarKind(kind string) bool {
	switch kind {
	case "bool", "u8", "u16", "u32", "u64", "s8", "s16", "s32", "s64",
		"f32", "f64", "char", "string":
		return true
	}
	return false
}

// ---------------------------------------------------------------------------
// the vectors test
// ---------------------------------------------------------------------------

func loadVectors(t *testing.T) []vector {
	path := os.Getenv("CRAB_VECTORS")
	if path == "" {
		// Generated projects live at <repo>/guest/<name> and this test runs
		// with cwd = gen/, so the repo's shared vectors are three levels up
		// (../../wit/vectors.json relative to the project root). Set
		// CRAB_VECTORS to run from anywhere else.
		path = "../../../wit/vectors.json"
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatalf("reading vectors (set CRAB_VECTORS to override the path): %v", err)
	}
	var vecs []vector
	if err := json.Unmarshal(data, &vecs); err != nil {
		t.Fatalf("parsing %s: %v", path, err)
	}
	if len(vecs) == 0 {
		t.Fatalf("no vectors in %s", path)
	}
	return vecs
}

func TestWireVectors(t *testing.T) {
	vecs := loadVectors(t)
	t.Logf("running %d conformance vectors", len(vecs))
	for _, v := range vecs {
		v := v
		t.Run(v.Desc, func(t *testing.T) {
			var ty tdesc
			if err := json.Unmarshal(v.Type, &ty); err != nil {
				t.Fatalf("bad type descriptor %s: %v", v.Type, err)
			}
			want, err := hex.DecodeString(v.Hex)
			if err != nil {
				t.Fatalf("bad hex %q: %v", v.Hex, err)
			}

			// 1. JSON value -> bytes must equal hex.
			enc := encodeJSON(t, nil, &ty, v.Value)
			if fmt.Sprintf("%x", enc) != v.Hex {
				t.Errorf("encode: got %x, want %s", enc, v.Hex)
			}

			// 2. hex -> value must consume the whole buffer...
			d := NewDecoder(want)
			got, err := decodeValue(d, &ty, t)
			if err != nil {
				t.Fatalf("decode: %v", err)
			}
			if err := d.Finish("value"); err != nil {
				t.Fatalf("decode: %v", err)
			}

			// 3. ...and re-encode byte-identically.
			re := reencodeValue(nil, &ty, got, t)
			if fmt.Sprintf("%x", re) != v.Hex {
				t.Errorf("re-encode: got %x, want %s", re, v.Hex)
			}

			// 4. scalar/string decoded-value equality.
			if isScalarKind(ty.Kind) {
				expectScalar(t, &ty, v.Value, got)
			}
		})
	}
}

// ---------------------------------------------------------------------------
// edge cases beyond the shared vectors (mirror crab-sdk validation rules)
// ---------------------------------------------------------------------------

func mustHex(t *testing.T, s string) []byte {
	b, err := hex.DecodeString(s)
	if err != nil {
		t.Fatalf("bad hex %q: %v", s, err)
	}
	return b
}

func TestSlebSignExtension(t *testing.T) {
	// s8 -1 encodes as a single 0x7f byte.
	if got := EncodeS8(nil, -1); fmt.Sprintf("%x", got) != "7f" {
		t.Errorf("EncodeS8(-1) = %x, want 7f", got)
	}
	d := NewDecoder(mustHex(t, "7f"))
	if v, err := d.S8(); err != nil || v != -1 {
		t.Errorf("S8(7f) = %d, %v; want -1", v, err)
	}
	// s64 min round-trips (10-byte sleb ending 0x7f).
	enc := EncodeS64(nil, -9223372036854775808)
	d = NewDecoder(enc)
	v, err := d.S64()
	if err != nil || v != -9223372036854775808 {
		t.Errorf("s64 min round-trip = %d, %v (enc %x)", v, err, enc)
	}
	if d.Remaining() != 0 {
		t.Errorf("s64 min: %d bytes left", d.Remaining())
	}
	// 10th byte of an s64 may only be 0x00 or 0x7f.
	d = NewDecoder(mustHex(t, "80808080808080808001"))
	if _, err := d.S64(); err == nil {
		t.Error("s64 with invalid 10th byte 0x01 must error")
	}
	// s8 range check: 0xc0 0x7f sign-extends to -64... use a clearly
	// out-of-range value: 128 = 0x80 0x01 as sleb.
	d = NewDecoder(mustHex(t, "8001"))
	if _, err := d.S8(); err == nil {
		t.Error("s8 = 128 must overflow")
	}
}

func TestUlebOverflowBits(t *testing.T) {
	// u8 max is 2 bytes; payload bits above bit 7 on byte 2 must be zero.
	d := NewDecoder(mustHex(t, "ff03")) // would be 511
	if _, err := d.U8(); err == nil {
		t.Error("u8 = 511 must overflow")
	}
	// Continuation bit on the last permitted byte: too long.
	d = NewDecoder(mustHex(t, "ff8100"))
	if _, err := d.U8(); err == nil {
		t.Error("3-byte uleb for u8 must error (too long)")
	}
	// Non-canonical zero padding is accepted: 0x87 0x00 decodes to 7.
	d = NewDecoder(mustHex(t, "8700"))
	if v, err := d.U8(); err != nil || v != 7 {
		t.Errorf("non-canonical uleb 8700 = %d, %v; want 7", v, err)
	}
	// u64 10th byte may only contribute bit 63: 0x02 there overflows.
	d = NewDecoder(mustHex(t, "ffffffffffffffffff02"))
	if _, err := d.U64(); err == nil {
		t.Error("u64 with bit 64 set must overflow")
	}
}

func TestCharValidation(t *testing.T) {
	// Surrogate U+D800 (uleb 0x80 0xb0 0x03) is not a unicode scalar value.
	d := NewDecoder(mustHex(t, "80b003"))
	if _, err := d.Char(); err == nil {
		t.Error("char U+D800 (surrogate) must be rejected")
	}
	// Above U+10FFFF.
	enc := EncodeU32(nil, 0x110000)
	d = NewDecoder(enc)
	if _, err := d.Char(); err == nil {
		t.Error("char U+110000 must be rejected")
	}
	// Max scalar U+10FFFF is fine.
	enc = EncodeU32(nil, 0x10FFFF)
	d = NewDecoder(enc)
	if v, err := d.Char(); err != nil || v != 0x10FFFF {
		t.Errorf("char U+10FFFF = %x, %v", v, err)
	}
}

func TestStrictBytes(t *testing.T) {
	// bool must be exactly 0 or 1.
	d := NewDecoder(mustHex(t, "02"))
	if _, err := d.Bool(); err == nil {
		t.Error("bool byte 2 must error")
	}
	// option tag must be exactly 0 or 1.
	d = NewDecoder(mustHex(t, "02"))
	if _, err := d.OptionTag(); err == nil {
		t.Error("option byte 2 must error")
	}
	// result tag must be exactly 0 or 1.
	d = NewDecoder(mustHex(t, "ff"))
	if _, err := d.ResultTag(); err == nil {
		t.Error("result byte 255 must error")
	}
	// invalid utf-8 in string.
	d = NewDecoder(mustHex(t, "02fffe"))
	if _, err := d.String(); err == nil {
		t.Error("invalid utf-8 must be rejected")
	}
	// string length past end of buffer.
	d = NewDecoder(mustHex(t, "056869")) // says 5 bytes, has 2
	if _, err := d.String(); err == nil {
		t.Error("truncated string must error")
	}
}

func TestResultNoPayloadTypes(t *testing.T) {
	// result (no ok/err types): a bare status byte.
	ty := &tdesc{Kind: "result"}
	d := NewDecoder(mustHex(t, "00"))
	v, err := decodeValue(d, ty, t)
	if err != nil {
		t.Fatalf("decode result ok: %v", err)
	}
	if err := d.Finish("value"); err != nil {
		t.Fatalf("trailing: %v", err)
	}
	if re := reencodeValue(nil, ty, v, t); fmt.Sprintf("%x", re) != "00" {
		t.Errorf("re-encode = %x, want 00", re)
	}
}

func TestFlagsValidation(t *testing.T) {
	// 10 flags = 2 bytes; bits 10..15 of byte 2 must be zero.
	d := NewDecoder(mustHex(t, "0004")) // bit 10 set
	if _, err := d.Flags(10); err == nil {
		t.Error("flags with unused high bit set must error")
	}
	// Exactly 8 flags: a full byte, no unused-bit check, bit 7 = 0x80.
	d = NewDecoder(mustHex(t, "80"))
	bits, err := d.Flags(8)
	if err != nil || !bits[7] || bits[0] {
		t.Errorf("flags(8) of 80 = %v, %v", bits, err)
	}
}

func TestVariantEnumRange(t *testing.T) {
	d := NewDecoder(mustHex(t, "04"))
	if _, err := d.EnumCase(4); err == nil {
		t.Error("enum case 4 of 4 must be out of range")
	}
	d = NewDecoder(mustHex(t, "02"))
	if _, err := d.VariantCase(2); err == nil {
		t.Error("variant case 2 of 2 must be out of range")
	}
}

func TestTrailingBytes(t *testing.T) {
	d := NewDecoder(mustHex(t, "0700"))
	if _, err := d.U32(); err != nil {
		t.Fatalf("u32: %v", err)
	}
	err := d.Finish("params")
	if err == nil {
		t.Fatal("Finish must error on trailing bytes")
	}
	if !strings.Contains(err.Error(), "trailing byte(s) after params") {
		t.Errorf("unexpected error: %v", err)
	}
}

func TestEmptyValues(t *testing.T) {
	if got := EncodeString(nil, ""); fmt.Sprintf("%x", got) != "00" {
		t.Errorf("empty string = %x, want 00", got)
	}
	d := NewDecoder(mustHex(t, "00"))
	if s, err := d.String(); err != nil || s != "" {
		t.Errorf("decode empty string = %q, %v", s, err)
	}
	if got := EncodeListLen(nil, 0); fmt.Sprintf("%x", got) != "00" {
		t.Errorf("empty list = %x, want 00", got)
	}
}

func TestParseMeshReply(t *testing.T) {
	// status 0: body returned verbatim.
	body, err := parseMeshReply(mustHex(t, "0007"))
	if err != nil || fmt.Sprintf("%x", body) != "07" {
		t.Errorf("status-0 reply = %x, %v; want 07", body, err)
	}
	// status 0 with empty body: ok, empty result.
	body, err = parseMeshReply(mustHex(t, "00"))
	if err != nil || len(body) != 0 {
		t.Errorf("status-0 empty reply = %x, %v; want empty", body, err)
	}
	// status 1: the decoded error string.
	payload := append([]byte{1}, EncodeString(nil, "boom")...)
	if _, err = parseMeshReply(payload); err == nil || err.Error() != "boom" {
		t.Errorf("status-1 reply err = %v; want boom", err)
	}
	// status 1 with trailing bytes after the error string: malformed.
	if _, err = parseMeshReply(append(payload, 0)); err == nil ||
		!strings.Contains(err.Error(), "malformed error reply") {
		t.Errorf("status-1 trailing bytes err = %v; want malformed", err)
	}
	// status 1 with an undecodable string: malformed.
	if _, err = parseMeshReply(mustHex(t, "0105")); err == nil ||
		!strings.Contains(err.Error(), "malformed error reply") {
		t.Errorf("status-1 truncated string err = %v; want malformed", err)
	}
	// invalid status byte.
	if _, err = parseMeshReply(mustHex(t, "02")); err == nil ||
		!strings.Contains(err.Error(), "invalid reply status") {
		t.Errorf("status-2 err = %v; want invalid reply status", err)
	}
	// empty payload.
	if _, err = parseMeshReply(nil); err == nil ||
		!strings.Contains(err.Error(), "empty reply") {
		t.Errorf("empty payload err = %v; want empty reply", err)
	}
}

func TestEOFEverywhere(t *testing.T) {
	d := NewDecoder(nil)
	if _, err := d.Bool(); err == nil {
		t.Error("bool on empty buffer must error")
	}
	d = NewDecoder(mustHex(t, "80")) // dangling continuation bit
	if _, err := d.U32(); err == nil {
		t.Error("truncated uleb must error")
	}
	d = NewDecoder(mustHex(t, "000000")) // 3 bytes for an f32
	if _, err := d.F32(); err == nil {
		t.Error("truncated f32 must error")
	}
}
