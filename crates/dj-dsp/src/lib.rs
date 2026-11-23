//! DSP building blocks for Kaldor.
//!
//! Everything in this crate that is called from the audio thread is
//! allocation-free after construction and never panics on valid input.

pub mod biquad;
pub mod color_fx;
pub mod curves;
pub mod denormal;
pub mod fx;
pub mod interp;
pub mod limiter;
pub mod oversample;
pub mod signalsmith;
pub mod smoothing;
pub mod stretch;
pub mod svf;

pub use denormal::DenormalGuard;
pub use interp::SincResampler;
pub use limiter::MasterLimiter;
pub use signalsmith::SignalsmithStretch;
pub use smoothing::LinearSmoother;
pub use stretch::{FrameSource, TimeStretcher, Wsola};

/// A zeroed buffer whose memory is committed now. `vec![0.0; n]` gets
/// lazily zeroed pages from the OS: the first write to each 4 KB page
/// later (on the audio thread, for delay lines and record buffers) is a
/// page fault, which can cost a dropout. Writing one value per page here
/// moves those faults to the thread that creates the buffer.
pub fn prefaulted_zeros(n: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; n];
    for chunk in v.chunks_mut(1024) {
        if let Some(x) = chunk.first_mut() {
            // `black_box` so the compiler cannot drop the store of a zero
            // into memory it knows is zero.
            *x = std::hint::black_box(0.0);
        }
    }
    v
}

/// One-pole smoothing coefficient for a time constant of `ms` milliseconds:
/// `y += (x - y) * (1 - coeff)` reaches 63 % of a step after `ms`.
pub fn time_constant_coeff(sample_rate: u32, ms: f32) -> f32 {
    (-1.0 / (ms.max(0.001) * 0.001 * sample_rate.max(1) as f32)).exp()
}
