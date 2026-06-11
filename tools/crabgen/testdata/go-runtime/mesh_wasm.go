//go:build wasip1

// wasip1-only half of the mesh client: the raw //go:wasmimport declaration
// (module "crabcraft", field "call" per WIRE.md section 2). Bodiless
// wasmimport functions do not compile on non-wasm targets, hence the build
// tag; mesh.go holds all testable logic. TinyGo's wasip1 target and plain
// go 1.24+ GOOS=wasip1 both set the wasip1 tag and both accept this
// declaration (unsafe.Pointer params keep the buffers alive across the
// call under Go's pointer rules).
package gen

import "unsafe"

//go:wasmimport crabcraft call
func crabcraftCall(wlPtr unsafe.Pointer, wlLen uint32,
	fnPtr unsafe.Pointer, fnLen uint32,
	parPtr unsafe.Pointer, parLen uint32) uint32

func init() {
	meshHostCall = crabcraftCall
}
