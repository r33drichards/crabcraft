// impl.go — the application half of this guest: implement gen.Impl here.
// crabgen scaffolds this file ONCE and never overwrites it; `crabgen regen`
// prints any missing method signatures instead of editing it.
package main

import (
	"crabcraft.local/hello-go/gen"
)

// App implements gen.Impl: one method per function exported by
// crab:hello-go/greeter@0.1.0.
type App struct{}

// Greet handles crab:hello-go/greeter@0.1.0#greet.
// greet(req: greet-request{name: string, excited: option<bool>}) -> string
func (App) Greet(req gen.GreetRequest) (string, error) {
	bang := "!"
	if req.Excited != nil && *req.Excited {
		bang = "!!!"
	}
	return "Hello from Go, " + req.Name + bang, nil
}

// Add handles crab:hello-go/greeter@0.1.0#add.
// add(a: u32, b: u32) -> u32 (wrapping, as in the original hand-rolled guest)
func (App) Add(a uint32, b uint32) (uint32, error) {
	return a + b, nil
}

func init() { gen.SetImpl(App{}) }

// main never runs: the module builds as a wasip1 reactor (the host calls
// _initialize once, then crab_invoke per request).
func main() {}
