//! Mouse position from the operating system, for files dropped from the
//! file manager.
//!
//! winit 0.30 reports a dropped file without a position, and on Windows the
//! window receives no mouse moves while a file is dragged in from Explorer,
//! so egui's pointer position is stale or unknown at the moment of the drop
//! (every dropped file landed on deck 1). On Windows the cursor is
//! read with `GetCursorPos`; elsewhere this returns `None` and the caller
//! uses egui's pointer position.

use egui::{Pos2, Rect};

/// Converts a cursor position in physical screen pixels to window-local ui
/// points. `inner` is the window's client area in screen points and
/// `pixels_per_point` the scale it was measured with.
pub fn screen_px_to_local(px: (i32, i32), inner: Rect, pixels_per_point: f32) -> Pos2 {
    let ppp = pixels_per_point.max(0.01);
    Pos2::new(
        px.0 as f32 / ppp - inner.min.x,
        px.1 as f32 / ppp - inner.min.y,
    )
}

/// The mouse position in window-local ui points, if the OS can tell.
pub fn cursor_in_window(ctx: &egui::Context) -> Option<Pos2> {
    // egui-winit computes `inner_rect` with egui's own scale (native scale
    // times the UI zoom), so the same scale converts the cursor.
    let (inner, ppp) = ctx.input(|i| (i.viewport().inner_rect, i.pixels_per_point()));
    let inner = inner?;
    screen_cursor_px().map(|px| screen_px_to_local(px, inner, ppp))
}

#[cfg(windows)]
fn screen_cursor_px() -> Option<(i32, i32)> {
    let mut p = windows::Win32::Foundation::POINT::default();
    // SAFETY: `p` is a valid, writable POINT for the duration of the call.
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) }.ok()?;
    Some((p.x, p.y))
}

#[cfg(not(windows))]
fn screen_cursor_px() -> Option<(i32, i32)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_screen_pixels_to_window_points() {
        // Window client area at (100, 50) points on a 150 % display.
        let inner = Rect::from_min_size(Pos2::new(100.0, 50.0), egui::vec2(800.0, 600.0));
        let p = screen_px_to_local((450, 300), inner, 1.5);
        assert_eq!(p, Pos2::new(200.0, 150.0));
        let p = screen_px_to_local((100, 50), inner, 1.0);
        assert_eq!(p, Pos2::new(0.0, 0.0));
    }
}
