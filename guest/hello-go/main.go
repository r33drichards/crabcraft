// crabcraft hello-go module: implements crab:hello-go/greeter@0.1.0
// (wit/hello-go.wit) as a wasm32-wasip1 REACTOR built with TinyGo:
//
//	tinygo build -o ../../modules/hello-go.wasm \
//	    -target=wasip1 -buildmode=c-shared -no-debug -scheduler=none .
//
// (-buildmode=c-shared makes a reactor: `_initialize` is exported, no
// `_start` is required at invoke time; see build.sh.)
//
// The WIRE.md section-1 codec subset needed here (uleb / bool / u32 /
// string / option / record) is hand-implemented below — small and explicit
// beats porting the whole Rust codec.
package main

import (
	_ "embed"
	"unicode/utf8"
	"unsafe"
)

// Resolved-WIT JSON (wasm-tools component wit --json wit/hello-go.wit),
// copied next to this file by build.sh and served verbatim via crab_schema.
//
//go:embed schema.json
var schema string

const instance = "crab:hello-go/greeter@0.1.0"

// ---------------------------------------------------------------------------
// WIRE.md section 1: value codec (subset: bool, u32, string, option, record)
// ---------------------------------------------------------------------------

// ulebAppend appends the unsigned LEB128 encoding of v.
func ulebAppend(out []byte, v uint64) []byte {
	for {
		b := byte(v & 0x7f)
		v >>= 7
		if v == 0 {
			return append(out, b)
		}
		out = append(out, b|0x80)
	}
}

// stringAppend appends uleb(len) + UTF-8 bytes.
func stringAppend(out []byte, s string) []byte {
	out = ulebAppend(out, uint64(len(s)))
	return append(out, s...)
}

type decoder struct {
	buf []byte
	pos int
}

func (d *decoder) remaining() int { return len(d.buf) - d.pos }

func (d *decoder) byte() (byte, bool) {
	if d.pos >= len(d.buf) {
		return 0, false
	}
	b := d.buf[d.pos]
	d.pos++
	return b, true
}

// uleb decodes an unsigned LEB128 capped at `bits` significant bits
// (same rules as the Rust SDK: max ceil(bits/7) bytes, payload bits above
// the width on the last permitted byte must be zero).
func (d *decoder) uleb(bits uint) (uint64, string) {
	maxBytes := (bits + 6) / 7
	var result uint64
	var shift uint
	for i := uint(0); i < maxBytes; i++ {
		b, ok := d.byte()
		if !ok {
			return 0, "unexpected end of buffer"
		}
		payload := uint64(b & 0x7f)
		if shift+7 > bits && payload>>(bits-shift) != 0 {
			return 0, "uleb overflow"
		}
		result |= payload << shift
		if b&0x80 == 0 {
			return result, ""
		}
		shift += 7
	}
	return 0, "uleb too long"
}

func (d *decoder) u32() (uint32, string) {
	v, e := d.uleb(32)
	return uint32(v), e
}

func (d *decoder) bool() (bool, string) {
	b, ok := d.byte()
	if !ok {
		return false, "unexpected end of buffer"
	}
	switch b {
	case 0:
		return false, ""
	case 1:
		return true, ""
	}
	return false, "invalid bool byte"
}

func (d *decoder) string() (string, string) {
	n, e := d.uleb(32)
	if e != "" {
		return "", e
	}
	if d.remaining() < int(n) {
		return "", "unexpected end of buffer"
	}
	s := string(d.buf[d.pos : d.pos+int(n)])
	d.pos += int(n)
	if !validUTF8(s) {
		return "", "invalid utf-8 in string"
	}
	return s, ""
}

// optionBool decodes option<bool>: 0 = none, 1 = some + bool byte.
func (d *decoder) optionBool() (present bool, val bool, err string) {
	b, ok := d.byte()
	if !ok {
		return false, false, "unexpected end of buffer"
	}
	switch b {
	case 0:
		return false, false, ""
	case 1:
		v, e := d.bool()
		return true, v, e
	}
	return false, false, "invalid option byte"
}

func validUTF8(s string) bool { return utf8.ValidString(s) }

// ---------------------------------------------------------------------------
// WIRE.md section 2: guest ABI (crab_alloc / crab_schema / crab_invoke)
// ---------------------------------------------------------------------------

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

func replyOK(result []byte) uint32 {
	payload := make([]byte, 0, 1+len(result))
	payload = append(payload, 0)
	payload = append(payload, result...)
	return lenbuf(payload)
}

func replyErr(msg string) uint32 {
	payload := make([]byte, 0, 2+len(msg))
	payload = append(payload, 1)
	payload = stringAppend(payload, msg)
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
	return lenbuf([]byte(schema))
}

//go:wasmexport crab_invoke
func crabInvoke(namePtr, nameLen, argPtr, argLen int32) uint32 {
	name := string(unsafe.Slice((*byte)(unsafe.Pointer(uintptr(uint32(namePtr)))), int(nameLen)))
	var args []byte
	if argLen > 0 {
		args = unsafe.Slice((*byte)(unsafe.Pointer(uintptr(uint32(argPtr)))), int(argLen))
	}

	switch name {
	case instance + "#greet":
		return invokeGreet(name, args)
	case instance + "#add":
		return invokeAdd(name, args)
	}
	return replyErr("unknown function: " + name)
}

// greet(req: greet-request{name: string, excited: option<bool>}) -> string
func invokeGreet(name string, args []byte) uint32 {
	d := &decoder{buf: args}
	who, e := d.string()
	if e != "" {
		return replyErr(name + ": bad params: " + e)
	}
	present, excited, e := d.optionBool()
	if e != "" {
		return replyErr(name + ": bad params: " + e)
	}
	if d.remaining() != 0 {
		return replyErr(name + ": bad params: trailing bytes after params")
	}
	bang := "!"
	if present && excited {
		bang = "!!!"
	}
	return replyOK(stringAppend(nil, "Hello from Go, "+who+bang))
}

// add(a: u32, b: u32) -> u32
func invokeAdd(name string, args []byte) uint32 {
	d := &decoder{buf: args}
	a, e := d.u32()
	if e != "" {
		return replyErr(name + ": bad params: " + e)
	}
	b, e := d.u32()
	if e != "" {
		return replyErr(name + ": bad params: " + e)
	}
	if d.remaining() != 0 {
		return replyErr(name + ": bad params: trailing bytes after params")
	}
	return replyOK(ulebAppend(nil, uint64(a+b)))
}

// main is required by package main; never run in -buildmode=c-shared
// (reactor) builds.
func main() {}
