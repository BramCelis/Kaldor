//! Runs the real binary with `--smoke-test` (headless, offline audio).

use std::process::Command;
use std::time::{Duration, Instant};

#[test]
fn smoke_test_flag_exits_cleanly() {
    let start = Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_kaldor"))
        .args(["--smoke-test", "--frames", "30"])
        .output()
        .expect("run kaldor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "exit {:?}\nstdout:\n{stdout}\nstderr:\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("smoke test: OK"), "{stdout}");
    assert!(start.elapsed() < Duration::from_secs(60));
}

#[test]
fn help_and_bad_arguments() {
    let help = Command::new(env!("CARGO_BIN_EXE_kaldor"))
        .arg("--help")
        .output()
        .expect("run kaldor");
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--smoke-test"));

    let bad = Command::new(env!("CARGO_BIN_EXE_kaldor"))
        .arg("--no-such-flag")
        .output()
        .expect("run kaldor");
    assert_eq!(bad.status.code(), Some(2));
}
