//! Tests that need real audio hardware (`#[ignore]`). The null output's
//! real-time test is in the `timing` test program.

/// Needs a real audio device. Run with `cargo test -p dj-audio -- --ignored`.
#[test]
#[ignore = "touches real audio hardware"]
fn list_real_output_devices() {
    let devices = dj_audio::device::list_output_devices(None).unwrap();
    for d in &devices {
        println!(
            "{} | {} | {} ch | {}-{} Hz{}",
            d.host,
            d.name,
            d.max_output_channels,
            d.min_sample_rate,
            d.max_sample_rate,
            if d.is_default { " (default)" } else { "" }
        );
    }
    assert!(!devices.is_empty());
}
