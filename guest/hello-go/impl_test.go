// Behavior of the ported hello-go logic (host-go test; excluded from the
// TinyGo wasm build). The wire codec/dispatch around it is covered by the
// generated gen/ tests.
package main

import (
	"testing"

	"crabcraft.local/hello-go/gen"
)

func TestGreet(t *testing.T) {
	yes, no := true, false
	cases := []struct {
		name    string
		excited *bool
		want    string
	}{
		{"steve", nil, "Hello from Go, steve!"},
		{"steve", &no, "Hello from Go, steve!"},
		{"steve", &yes, "Hello from Go, steve!!!"},
		{"", nil, "Hello from Go, !"},
	}
	for _, c := range cases {
		got, err := App{}.Greet(gen.GreetRequest{Name: c.name, Excited: c.excited})
		if err != nil || got != c.want {
			t.Errorf("Greet(%q, %v) = %q, %v; want %q", c.name, c.excited, got, err, c.want)
		}
	}
}

func TestAddWraps(t *testing.T) {
	got, err := App{}.Add(4_000_000_000, 1_000_000_000)
	if err != nil || got != 705_032_704 {
		t.Errorf("Add wrap = %d, %v; want 705032704", got, err)
	}
	got, err = App{}.Add(2, 3)
	if err != nil || got != 5 {
		t.Errorf("Add = %d, %v; want 5", got, err)
	}
}
