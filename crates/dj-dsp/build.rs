//! Compiles Signalsmith Stretch (header-only C++, vendored in `third_party/`)
//! behind a small C interface (`cpp/signalsmith_wrapper.cpp`).
//!
//! Needs only the platform C++ compiler (MSVC, clang or gcc), no bindgen:
//! the Rust declarations are written by hand in `src/signalsmith.rs`.

fn main() {
    println!("cargo:rerun-if-changed=cpp/signalsmith_wrapper.cpp");
    println!("cargo:rerun-if-changed=third_party");
    cc::Build::new()
        .cpp(true)
        .std("c++14")
        .file("cpp/signalsmith_wrapper.cpp")
        .include("third_party")
        // The algorithm is number-crunching; unoptimised builds are up to 10x
        // slower (library README), which would break real-time playback and
        // slow the tests. Optimise even in debug builds.
        .opt_level(2)
        .define("NDEBUG", None)
        .warnings(false)
        .compile("kaldor_signalsmith");
}
