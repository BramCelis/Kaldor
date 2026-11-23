//! Engine tests on the offline renderer and null outputs. `no_alloc` (its
//! own global allocator), `soak` (long, timing-based) and `timing` (a
//! wall-clock test that must not share the CPU) stay separate programs.
//!
//! One test program per crate instead of one per file: each program is a
//! separate link, and linking was most of a full test run. Each module is
//! one of the former test files (run one with its name as a filter, e.g.
//! `cargo test -p <crate> --test engine <module>`).

mod controller;
mod deck;
mod fx;
mod mixer;
mod null_and_devices;
mod performance;
mod routing;
mod settings;
