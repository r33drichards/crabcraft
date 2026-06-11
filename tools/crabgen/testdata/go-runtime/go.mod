// Scratch module for running the crabgen Go runtime template against the
// WIRE conformance vectors on host go. The .go files here are COPIES of
// tools/crabgen/templates/go/*.go, refreshed by tests/go_vectors.rs before
// every run — edit the templates, not these copies.
module crabcraft.local/goruntime

go 1.24
