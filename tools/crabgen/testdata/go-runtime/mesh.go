// crabcraft service mesh client: the optional `crabcraft.call` host import
// (WIRE.md section 2). crabgen emits this template only when the module's
// WIT world has imports.
//
// Build compatibility: the //go:wasmimport declaration cannot compile on
// non-wasm targets (a bodiless Go function needs an implementation), so it
// lives in mesh_wasm.go behind `//go:build wasip1` and is wired up through
// the meshHostCall variable. On host go (tests) the variable stays nil and
// MeshCall returns a clear error instead.
package gen

import (
	"errors"
	"unsafe"
)

// meshHostCall is the raw crabcraft.call import; nil off-wasm. mesh_wasm.go
// assigns it in init() for wasip1 builds (TinyGo wasip1 sets GOOS=wasip1, so
// the tag covers both TinyGo and plain go 1.24+).
var meshHostCall func(wlPtr unsafe.Pointer, wlLen uint32,
	fnPtr unsafe.Pointer, fnLen uint32,
	parPtr unsafe.Pointer, parLen uint32) uint32

func slicePtr(b []byte) unsafe.Pointer {
	if len(b) == 0 {
		return nil
	}
	return unsafe.Pointer(&b[0])
}

// readLenbuf reads a host-returned LENBUF ([u32 LE length][payload]) out of
// linear memory; a null pointer reads as an empty payload.
func readLenbuf(ptr uint32) []byte {
	if ptr == 0 {
		return nil
	}
	hdr := unsafe.Slice((*byte)(unsafe.Pointer(uintptr(ptr))), 4)
	n := uint32(hdr[0]) | uint32(hdr[1])<<8 | uint32(hdr[2])<<16 | uint32(hdr[3])<<24
	if n == 0 {
		return nil
	}
	return unsafe.Slice((*byte)(unsafe.Pointer(uintptr(ptr)+4)), int(n))
}

// MeshCall invokes `fn` (a `<instance>#<function>` address) on the workload
// named `workload` through the host mesh, passing WIRE-encoded params, and
// returns the encoded result value. A status-1 reply decodes the error
// string. The host addresses services BY NAME; placement is its problem.
func MeshCall(workload, fn string, params []byte) ([]byte, error) {
	if meshHostCall == nil {
		return nil, errors.New("crabcraft.call import unavailable: not running under a crabcraft host")
	}
	wl := []byte(workload)
	fnb := []byte(fn)
	ptr := meshHostCall(slicePtr(wl), uint32(len(wl)),
		slicePtr(fnb), uint32(len(fnb)),
		slicePtr(params), uint32(len(params)))
	// The host wrote the reply via crab_alloc: copy out what we need, then
	// unpin so the buffer can be collected.
	defer delete(allocs, uintptr(ptr))
	return parseMeshReply(readLenbuf(ptr))
}

// parseMeshReply splits a [status][body] mesh reply: status 0 returns a copy
// of the body (the encoded result value); status 1 decodes the body as the
// error string, which must consume the body exactly; anything else is a
// protocol error.
func parseMeshReply(payload []byte) ([]byte, error) {
	if len(payload) == 0 {
		return nil, errors.New("mesh call: empty reply")
	}
	status, body := payload[0], payload[1:]
	switch status {
	case 0:
		out := make([]byte, len(body))
		copy(out, body)
		return out, nil
	case 1:
		d := NewDecoder(body)
		msg, err := d.String()
		if err != nil {
			return nil, errors.New("mesh call: malformed error reply")
		}
		if d.Remaining() != 0 {
			return nil, errors.New("mesh call: malformed error reply (trailing bytes)")
		}
		return nil, errors.New(msg)
	}
	return nil, errors.New("mesh call: invalid reply status")
}
