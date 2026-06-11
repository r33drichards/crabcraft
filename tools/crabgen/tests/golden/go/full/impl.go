// impl.go — the application half of this guest: implement gen.Impl here.
// crabgen scaffolds this file ONCE and never overwrites it; `crabgen regen`
// prints any missing method signatures instead of editing it.
package main

import (
	"fmt"

	"crabcraft.local/full/gen"
)

// App implements gen.Impl: one method per function exported by
// crab:full/kitchen@0.1.0.
type App struct{}

// EchoEverything handles crab:full/kitchen@0.1.0#echo-everything.
// A non-nil error is a function-level failure (status-1 reply).
func (App) EchoEverything(e gen.Everything) (gen.Everything, error) {
	return gen.Everything{}, fmt.Errorf("unimplemented: echo-everything")
}

// PickColor handles crab:full/kitchen@0.1.0#pick-color.
// A non-nil error is a function-level failure (status-1 reply).
func (App) PickColor(c_ gen.Color) (gen.Color, error) {
	return 0, fmt.Errorf("unimplemented: pick-color")
}

// SetPerms handles crab:full/kitchen@0.1.0#set-perms.
// A non-nil error is a function-level failure (status-1 reply).
func (App) SetPerms(p gen.Perms) (gen.Perms, error) {
	return 0, fmt.Errorf("unimplemented: set-perms")
}

// Classify handles crab:full/kitchen@0.1.0#classify.
// A non-nil error is a function-level failure (status-1 reply).
func (App) Classify(s gen.Shape) (string, error) {
	return "", fmt.Errorf("unimplemented: classify")
}

// TryDivide handles crab:full/kitchen@0.1.0#try-divide.
// A non-nil error encodes as the WIT result err case (a normal status-0 reply).
func (App) TryDivide(num float64, den float64) (float64, error) {
	return 0, fmt.Errorf("unimplemented: try-divide")
}

// MaybeList handles crab:full/kitchen@0.1.0#maybe-list.
// A non-nil error is a function-level failure (status-1 reply).
func (App) MaybeList(xs *[]uint16) ([]*bool, error) {
	return nil, fmt.Errorf("unimplemented: maybe-list")
}

// NoResult handles crab:full/kitchen@0.1.0#no-result.
// A non-nil error is a function-level failure (status-1 reply).
func (App) NoResult(x uint32) error {
	return fmt.Errorf("unimplemented: no-result")
}

func init() { gen.SetImpl(App{}) }

// main never runs: the module builds as a wasip1 reactor (the host calls
// _initialize once, then crab_invoke per request).
func main() {}
