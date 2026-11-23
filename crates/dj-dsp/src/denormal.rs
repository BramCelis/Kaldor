//! Flush-to-zero for denormal floats on the audio thread.
//!
//! Denormal numbers (tiny values near zero, common in decaying filter
//! states) are very slow on most CPUs. [`DenormalGuard`] sets the CPU's
//! flush-to-zero (and on x86 also denormals-are-zero) mode for the current
//! thread and restores the previous mode when dropped.

/// Sets flush-to-zero mode on the current thread while alive.
#[must_use = "the mode is restored when the guard is dropped"]
pub struct DenormalGuard {
    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    previous: u64,
}

impl DenormalGuard {
    /// Enables flush-to-zero on this thread.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            let mut old: u32 = 0;
            // SAFETY: `stmxcsr` stores the current thread's MXCSR register to
            // the given, valid and aligned `u32` location. It has no other
            // effect.
            unsafe {
                core::arch::asm!("stmxcsr [{}]", in(reg) &mut old as *mut u32, options(nostack, preserves_flags));
            }
            // Bit 15 = FTZ (flush to zero), bit 6 = DAZ (denormals are zero).
            let new: u32 = old | 0x8040;
            // SAFETY: `ldmxcsr` loads MXCSR for the current thread from a
            // valid `u32`. We only set the FTZ and DAZ bits, which change how
            // denormals are rounded and never cause exceptions, because the
            // exception mask bits are copied unchanged from the old value.
            unsafe {
                core::arch::asm!("ldmxcsr [{}]", in(reg) &new as *const u32, options(nostack, readonly, preserves_flags));
            }
            Self {
                previous: u64::from(old),
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            let old: u64;
            // SAFETY: reading FPCR has no side effects.
            unsafe {
                core::arch::asm!("mrs {}, fpcr", out(reg) old, options(nomem, nostack, preserves_flags));
            }
            // Bit 24 = FZ (flush to zero).
            let new = old | (1 << 24);
            // SAFETY: setting the FZ bit only changes denormal handling for
            // the current thread; all other control bits are preserved.
            unsafe {
                core::arch::asm!("msr fpcr, {}", in(reg) new, options(nomem, nostack, preserves_flags));
            }
            Self { previous: old }
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Self {}
        }
    }
}

impl Drop for DenormalGuard {
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        {
            let old = self.previous as u32;
            // SAFETY: restores the MXCSR value read in `new` on this thread.
            unsafe {
                core::arch::asm!("ldmxcsr [{}]", in(reg) &old as *const u32, options(nostack, readonly, preserves_flags));
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: restores the FPCR value read in `new` on this thread.
            unsafe {
                core::arch::asm!("msr fpcr, {}", in(reg) self.previous, options(nomem, nostack, preserves_flags));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hint::black_box;

    #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
    #[test]
    fn denormals_flush_to_zero_inside_guard_only() {
        let tiny = black_box(f32::MIN_POSITIVE);
        let before = black_box(tiny) * black_box(0.5);
        assert!(before > 0.0, "denormal exists without the guard");
        {
            let _g = DenormalGuard::new();
            let inside = black_box(tiny) * black_box(0.5);
            assert_eq!(inside, 0.0, "flushed inside the guard");
        }
        let after = black_box(tiny) * black_box(0.5);
        assert!(after > 0.0, "previous mode restored");
    }
}
