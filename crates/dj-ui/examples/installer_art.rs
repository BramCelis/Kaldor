//! Makes the Windows installer's two images from the mark (24-bit BMPs, as
//! the WiX Toolset's dialogs want them):
//!
//! - the banner across the top of the inner pages, 493 x 58: white (the
//!   page title is drawn over its left part), the icon at the right;
//! - the picture of the welcome and finish pages, 493 x 312: a dark panel
//!   with the icon and the name on the left (164 px, where WiX leaves room
//!   for it), white where the text is drawn.
//!
//! ```sh
//! cargo run -p dj-ui --example installer_art -- MARK.svg BANNER.bmp DIALOG.bmp
//! ```
//!
//! Exits by itself.

use resvg::{tiny_skia, usvg};

/// The inside of an SVG document (between the root's tags).
fn inner(svg: &str) -> &str {
    let start = svg.find('>').map_or(0, |i| i + 1);
    let end = svg.rfind("</svg>").unwrap_or(svg.len());
    &svg[start..end]
}

/// Renders `svg` (a 0-100 square view box) into `pixmap` at `(x, y)`,
/// `size` pixels square.
fn draw(pixmap: &mut tiny_skia::Pixmap, svg: &str, x: f32, y: f32, size: f32) -> Option<()> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let s = size / tree.size().height();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(s, s).post_translate(x, y),
        &mut pixmap.as_mut(),
    );
    Some(())
}

/// The mark on the dark rounded square (as the app icon).
fn icon_svg(mark: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect width="100" height="100" rx="22" fill="#121418"/><svg x="14" y="14" width="72" height="72" viewBox="0 0 100 100">{}</svg></svg>"##,
        inner(mark)
    )
}

/// The mark alone.
fn mark_svg(mark: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100">{}</svg>"##,
        inner(mark)
    )
}

/// A 24-bit BMP (bottom-up rows, padded to 4 bytes) of an opaque pixmap.
fn bmp(p: &tiny_skia::Pixmap) -> Vec<u8> {
    let (w, h) = (p.width() as usize, p.height() as usize);
    let row = (w * 3).div_ceil(4) * 4;
    let size = 54 + row * h;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0; 4]); // no compression
    out.extend_from_slice(&((row * h) as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&[0; 8]);
    let px = p.data();
    for y in (0..h).rev() {
        let start = out.len();
        for x in 0..w {
            let i = (y * w + x) * 4;
            // Opaque pixmaps: premultiplied equals straight RGB.
            out.extend_from_slice(&[px[i + 2], px[i + 1], px[i]]);
        }
        out.resize(start + row, 0);
    }
    out
}

fn fill(p: &mut tiny_skia::Pixmap, x: f32, y: f32, w: f32, h: f32, rgb: [u8; 3]) {
    let mut paint = tiny_skia::Paint::default();
    paint.set_color_rgba8(rgb[0], rgb[1], rgb[2], 255);
    if let Some(r) = tiny_skia::Rect::from_xywh(x, y, w, h) {
        p.fill_rect(r, &paint, tiny_skia::Transform::identity(), None);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mark, out_banner, out_dialog] = args.as_slice() else {
        eprintln!("usage: installer_art MARK.svg BANNER.bmp DIALOG.bmp");
        return;
    };
    let Ok(mark) = std::fs::read_to_string(mark) else {
        eprintln!("cannot read {mark}");
        return;
    };
    const DARK: [u8; 3] = [0x12, 0x14, 0x18];
    const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];

    // Banner: white, the icon at the right, a thin line under it.
    let Some(mut banner) = tiny_skia::Pixmap::new(493, 58) else {
        return;
    };
    fill(&mut banner, 0.0, 0.0, 493.0, 58.0, WHITE);
    if draw(&mut banner, &icon_svg(&mark), 493.0 - 50.0, 7.0, 44.0).is_none() {
        eprintln!("could not render the icon");
        return;
    }

    // Welcome / finish picture: the dark panel with the mark and the name.
    let Some(mut dialog) = tiny_skia::Pixmap::new(493, 312) else {
        return;
    };
    fill(&mut dialog, 0.0, 0.0, 493.0, 312.0, WHITE);
    fill(&mut dialog, 0.0, 0.0, 164.0, 312.0, DARK);
    if draw(&mut dialog, &mark_svg(&mark), 22.0, 70.0, 120.0).is_none() {
        eprintln!("could not render the mark");
        return;
    }
    // A thin line in the brand purple along the panel's edge.
    fill(&mut dialog, 162.0, 0.0, 2.0, 312.0, [0x6B, 0x2F, 0xF0]);

    for (path, p) in [(out_banner, &banner), (out_dialog, &dialog)] {
        if let Err(e) = std::fs::write(path, bmp(p)) {
            eprintln!("{path}: {e}");
            return;
        }
        println!("{path}");
    }
}
