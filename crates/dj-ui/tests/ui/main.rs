//! UI tests (headless: no window, no audio device, no MIDI port).
//!
//! One test program per crate instead of one per file: each program is a
//! separate link, and linking was most of a full test run. Each module is
//! one of the former test files (run one with its name as a filter, e.g.
//! `cargo test -p <crate> --test ui <module>`).

mod audio_switch;
mod controller;
mod crate_tree;
mod fx;
mod headless;
mod history;
mod import_from;
mod layout_geometry;
mod library_files;
mod library_list;
mod library_ui;
mod load_file;
mod performance;
mod recording;
mod rekordbox_tab;
mod settings_options;
mod smart_crates;
mod ui_functions;
