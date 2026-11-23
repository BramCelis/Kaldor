//! Keeps the screen on and the computer awake while the app's window is
//! open.
//!
//! A DJ may not touch the mouse or keyboard for a long time while playing
//! (the controller is not "user input" to the OS), so without this the
//! display went to sleep in the middle of a set. On Windows this asks for
//! `ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED` with `SetThreadExecutionState`;
//! the request belongs to the calling thread and lasts until the guard is
//! dropped (or the process ends). Elsewhere it does nothing yet.

/// Holds the "stay awake" request; dropping it gives the decision back to
/// the OS's power settings.
pub struct KeepAwake {
    _private: (),
}

impl KeepAwake {
    /// Asks the OS to keep the display and the system on. Must be created and
    /// dropped on the same thread (the UI thread), which outlives the window.
    pub fn new() -> Self {
        imp::acquire();
        Self { _private: () }
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        imp::release();
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };

    pub fn acquire() {
        // SAFETY: plain flags in, previous state out; no pointers involved.
        let prev = unsafe {
            SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED)
        };
        if prev.0 == 0 {
            tracing::warn!("could not keep the screen awake (SetThreadExecutionState failed)");
        }
    }

    pub fn release() {
        // SAFETY: as above.
        unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn acquire() {}
    pub fn release() {}
}
