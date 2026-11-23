//! Renders logo candidates side by side as they will be seen: the app icon
//! (the mark on the dark rounded square) at 256, 64, 48, 32 and 16 px, and
//! the mark at top-bar height (22 px) on the dark and the light top bar.
//! Exits by itself.
//!
//! ```sh
//! cargo run -p dj-ui --example logo_sheet -- OUT.png A.svg [B.svg ...]
//! ```
//!
//! Each SVG uses a 100 x 100 view box; `#FFFFFF` is the ink that turns dark
//! on the light theme (as in `src/brand.rs`).

use resvg::{tiny_skia, usvg};

/// The inside of an SVG document (between the root's tags).
fn inner(svg: &str) -> &str {
    let start = svg.find('>').map_or(0, |i| i + 1);
    let end = svg.rfind("</svg>").unwrap_or(svg.len());
    &svg[start..end]
}

/// The app icon: the mark on the dark rounded square.
fn icon_svg(mark: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect width="100" height="100" rx="22" fill="#121418"/><svg x="14" y="14" width="72" height="72" viewBox="0 0 100 100">{}</svg></svg>"##,
        inner(mark)
    )
}

fn draw(sheet: &mut tiny_skia::Pixmap, svg: &str, x: f32, y: f32, size: f32) {
    let Ok(tree) = usvg::Tree::from_str(svg, &usvg::Options::default()) else {
        eprintln!("bad svg");
        return;
    };
    let s = size / tree.size().height();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_row(s, 0.0, 0.0, s, x, y),
        &mut sheet.as_mut(),
    );
}

fn fill(sheet: &mut tiny_skia::Pixmap, x: f32, y: f32, w: f32, h: f32, rgb: (u8, u8, u8)) {
    if let Some(r) = tiny_skia::Rect::from_xywh(x, y, w, h) {
        let mut p = tiny_skia::Paint::default();
        p.set_color_rgba8(rgb.0, rgb.1, rgb.2, 255);
        sheet.fill_rect(r, &p, tiny_skia::Transform::identity(), None);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((out, files)) = args.split_first() else {
        eprintln!("usage: logo_sheet OUT.png A.svg [B.svg ...]");
        return;
    };
    let row = 300.0;
    let (w, h) = (1000u32, (row as u32) * files.len().max(1) as u32);
    let Some(mut sheet) = tiny_skia::Pixmap::new(w, h) else {
        return;
    };
    fill(&mut sheet, 0.0, 0.0, w as f32, h as f32, (0x2A, 0x2D, 0x33));
    for (k, f) in files.iter().enumerate() {
        let Ok(mark) = std::fs::read_to_string(f) else {
            eprintln!("cannot read {f}");
            continue;
        };
        let y = k as f32 * row;
        let icon = icon_svg(&mark);
        draw(&mut sheet, &icon, 20.0, y + 22.0, 256.0);
        let mut x = 300.0;
        for size in [64.0, 48.0, 32.0, 16.0] {
            draw(&mut sheet, &icon, x, y + 22.0, size);
            x += size + 24.0;
        }
        // The top bar, dark and light, the mark 22 px high (as in the app).
        fill(
            &mut sheet,
            300.0,
            y + 130.0,
            660.0,
            34.0,
            (0x1A, 0x1D, 0x23),
        );
        draw(&mut sheet, &mark, 320.0, y + 136.0, 22.0);
        fill(
            &mut sheet,
            300.0,
            y + 180.0,
            660.0,
            34.0,
            (0xF7, 0xF8, 0xFA),
        );
        draw(
            &mut sheet,
            &mark.replace("#FFFFFF", "#1D2229"),
            320.0,
            y + 186.0,
            22.0,
        );
        // Large, to judge the shapes.
        draw(&mut sheet, &mark, 420.0, y + 120.0, 160.0);
    }
    match sheet.save_png(out) {
        Ok(()) => println!("{out}"),
        Err(e) => eprintln!("{e}"),
    }
}
