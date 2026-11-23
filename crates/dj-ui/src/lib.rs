//! egui user interface for Kaldor.
//!
//! [`KaldorApp`] holds all UI state and talks to the audio engine through an
//! [`dj_audio::EngineHandle`]. It runs either inside `eframe` (a real window)
//! or headless through [`headless::run_frames`], which the `--smoke-test`
//! flag and the tests use; headless mode never opens a window.

pub mod app;
pub mod audio;
pub mod brand;
pub mod browse;
pub mod controller;
pub mod controller_out;
mod deck_ext;
mod deck_view;
pub mod file_check;
pub mod files_panel;
pub mod frame_clock;
pub mod fx_panel;
mod fx_view;
mod general;
pub mod headless;
pub mod history_view;
pub mod interop_view;
mod keep_awake;
mod keyboard_setup;
pub mod layout;
pub mod library;
mod library_dialogs;
mod library_settings;
mod library_view;
pub mod library_wave;
pub mod loader;
mod mixer_setup;
mod mixer_view;
mod os_cursor;
pub mod perform;
mod perform_view;
mod record_view;
pub mod recorder;
pub mod rekordbox_sync;
pub mod relink;
mod sampler_view;
mod settings_ui;
mod setup;
pub mod smart_view;
pub mod theme;
pub mod uimap;
pub mod usb_export;
pub mod waveform_view;
pub mod widgets;

pub use app::{AppEvent, DropTarget, KaldorApp};
pub use setup::SetupTab;

/// The file name a recording gets from a name pattern (see Settings >
/// Recording).
pub fn record_view_name(pattern: &str, n: usize) -> String {
    record_view::recording_name(pattern, n)
}

/// Window title and app id.
pub const APP_NAME: &str = "Kaldor";

/// Runs the app in a native window until the user closes it.
pub fn run_native(app: KaldorApp) -> Result<(), eframe::Error> {
    // A real window: the Browse buttons may open the system's dialogs.
    browse::enable();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(APP_NAME)
        .with_app_id("kaldor")
        .with_inner_size([1600.0, 950.0])
        .with_min_inner_size([960.0, 600.0])
        // Full screen from the start when it was on last time.
        .with_fullscreen(app.config().ui.fullscreen);
    if let Some(icon) = brand::icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    // No screen saver or display sleep while the window is open: the
    // controller does not count as user input for the OS.
    let _awake = keep_awake::KeepAwake::new();
    eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |_cc| Ok(Box::new(EframeApp(app)))),
    )
}

struct EframeApp(KaldorApp);

impl eframe::App for EframeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
        // Waveforms scroll continuously against the audio clock, at most
        // at the frame rate set in the settings (0: every display frame).
        match self.0.config().ui.max_fps {
            0 => ui.ctx().request_repaint(),
            fps => ui
                .ctx()
                .request_repaint_after(std::time::Duration::from_secs_f64(
                    1.0 / f64::from(fps.max(10)),
                )),
        }
    }
}

/// A folder for unit tests in the workspace's `target` folder (tests stay
/// inside the project folder).
#[cfg(test)]
pub(crate) fn test_dir(name: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/tmp")
        .join(format!("{name}-{}", std::process::id()));
    std::fs::create_dir_all(&d).ok();
    d
}
