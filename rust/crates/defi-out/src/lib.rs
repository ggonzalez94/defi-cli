//! JSON/plain rendering and field selection.
//!
//! Mirrors `internal/out/render.go`. JSON uses 2-space indent with struct field
//! declaration order; plain output sorts map keys alphabetically; `--select`
//! projects named top-level fields (machine contract — spec §2.3).
#![allow(dead_code, unused)]
