//! Export tests.
//!
//! One test program per crate instead of one per file: each program is a
//! separate link, and linking was most of a full test run. Each module is
//! one of the former test files (run one with its name as a filter, e.g.
//! `cargo test -p <crate> --test export <module>`).

mod golden_stick;
mod rekordbox_xml;
mod usb_export;
