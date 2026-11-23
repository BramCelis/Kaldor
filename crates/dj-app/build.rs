//! Windows: puts the Kaldor icon (assets/kaldor.ico, made from the mark by
//! `cargo run -p dj-ui --example make_icons`)
//! into the .exe so Explorer, shortcuts and the taskbar show it. Other
//! targets: nothing. A missing resource compiler only gives a warning; the
//! window still gets its icon at run time.

fn main() {
    println!("cargo:rerun-if-changed=assets/kaldor.rc");
    println!("cargo:rerun-if-changed=assets/kaldor.ico");
    let result = embed_resource::compile(
        "assets/kaldor.rc",
        embed_resource::ParamsIncludeDirs(&["assets"]),
    );
    match result {
        embed_resource::CompilationResult::Ok | embed_resource::CompilationResult::NotWindows => {}
        other => println!("cargo:warning=no icon in the .exe: {other}"),
    }
}
