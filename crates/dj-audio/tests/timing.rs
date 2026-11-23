//! An engine test that depends on wall-clock time, in its own test program.
//!
//! Cargo runs test programs one after the other, so this test never shares
//! the CPU with the offline-render tests of the `engine` program. There it
//! failed once while the machine was busy (it needs 2,000 frames rendered
//! during a 200 ms sleep) and passed alone. The test is an unchanged copy
//! from `engine/null_and_devices.rs`.

use dj_audio::null_output::NullOutput;
use dj_audio::{AudioOutput, Engine, EngineConfig};
use std::time::Duration;

#[test]
fn null_output_runs_at_real_time_pace_and_stops() {
    let (engine, mut handle) = Engine::new(EngineConfig::default());
    let out = NullOutput::start(engine, 2, 256).unwrap();
    assert!(out.is_running());
    std::thread::sleep(Duration::from_millis(200));
    let engine = out.stop().expect("engine comes back");
    let frames = handle.update().frames_rendered;
    // 200 ms at 48 kHz is 9600 frames; allow generous scheduling slack.
    assert!(frames > 2_000, "rendered {frames}");
    assert!(frames < 48_000, "not faster than real time: {frames}");
    drop(engine);
}
