//! The Kaldor logo: the window icon and the mark in the top bar. The mark
//! ("dual orbit, crystal": a faceted record with two decks
//! orbiting it) is an SVG (`assets/kaldor-mark.svg`) rendered at the exact
//! pixel size it is shown at, so it stays sharp at any scaling (see
//! `crates/dj-ui/assets/README.md`).

use egui::{Color32, ColorImage, TextureHandle, TextureOptions};

/// The app icon (the mark on its dark rounded square), 256 x 256.
const ICON_PNG: &[u8] = include_bytes!("../assets/kaldor-icon-256.png");
/// The mark, vector.
const MARK_SVG: &str = include_str!("../assets/kaldor-mark.svg");

/// Width / height of the mark, from its SVG (so a new SVG of any shape is
/// drawn undistorted).
pub fn mark_aspect() -> f32 {
    static ASPECT: std::sync::OnceLock<f32> = std::sync::OnceLock::new();
    *ASPECT.get_or_init(|| {
        resvg::usvg::Tree::from_str(MARK_SVG, &resvg::usvg::Options::default())
            .map(|t| t.size().width() / t.size().height().max(1.0))
            .unwrap_or(1.0)
    })
}

/// The logo's purple (the record's middle tone).
pub const PURPLE: Color32 = Color32::from_rgb(0x6B, 0x2F, 0xF0);

/// In the mark: the white parts' lit side (`#FFFFFF`) and shaded side
/// (`#C9CED6`); on the light theme they take the ink and a lighter ink.
const MARK_WHITE: &str = "#FFFFFF";
const MARK_WHITE_SHADE: &str = "#C9CED6";

/// Decodes an 8-bit RGB(A) PNG into (width, height, RGBA bytes).
fn decode(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = dec.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return None;
    }
    buf.truncate(info.buffer_size());
    Some((info.width, info.height, buf))
}

/// The window and taskbar icon.
pub fn icon() -> Option<egui::IconData> {
    let (width, height, rgba) = decode(ICON_PNG)?;
    Some(egui::IconData {
        rgba,
        width,
        height,
    })
}

/// The mark rendered from its SVG, `height` pixels high: as drawn (white
/// ring and deck) on dark themes; on light themes the white parts take
/// `ink`, their shaded sides a lighter ink (the purple stays).
pub fn mark_image(ink: Option<Color32>, height: u32) -> Option<ColorImage> {
    use resvg::{tiny_skia, usvg};
    let hex = |c: Color32| format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
    let svg = match ink {
        Some(c) => {
            // The shade: the ink 45 % of the way to white.
            let lift = |v: u8| (f32::from(v) + (255.0 - f32::from(v)) * 0.45) as u8;
            let shade = Color32::from_rgb(lift(c.r()), lift(c.g()), lift(c.b()));
            MARK_SVG
                .replace(MARK_WHITE, &hex(c))
                .replace(MARK_WHITE_SHADE, &hex(shade))
        }
        None => MARK_SVG.to_string(),
    };
    let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).ok()?;
    let h = height.clamp(1, 1024);
    let scale = h as f32 / tree.size().height();
    let w = (tree.size().width() * scale).ceil().max(1.0) as u32;
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    Some(ColorImage::from_rgba_premultiplied(
        [w as usize, h as usize],
        pixmap.data(),
    ))
}

/// The mark as a texture `height` pixels high, made once per theme and
/// size and kept in the context.
pub(crate) fn mark_texture(
    ctx: &egui::Context,
    dark: bool,
    ink: Color32,
    height: u32,
) -> Option<TextureHandle> {
    let id = egui::Id::unique(("kaldor_mark", dark, height));
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return Some(t);
    }
    let img = mark_image((!dark).then_some(ink), height)?;
    let t = ctx.load_texture("kaldor_mark", img, TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logo_images_decode() {
        let icon = icon().expect("icon");
        assert_eq!((icon.width, icon.height), (256, 256));
        // Rendered at the height asked for, with the SVG's own proportions.
        let mark = mark_image(None, 128).expect("mark");
        assert_eq!(mark.size[1], 128);
        assert_eq!(mark.size[0], (128.0 * mark_aspect()).ceil() as usize);
        assert_eq!(mark_image(None, 40).expect("small").size[1], 40);
        // The logo purple is in it; the light-theme version still renders.
        let purple = |c: &Color32| c.a() == 255 && c.b() > 200 && c.r() < 140 && c.g() < 80;
        assert!(mark.pixels.iter().any(purple), "purple");
        let ink = Color32::from_rgb(0x1D, 0x22, 0x29);
        let light = mark_image(Some(ink), 128).expect("light mark");
        assert!(light.pixels.iter().any(|c| c.a() == 255), "drawn");
    }
}
