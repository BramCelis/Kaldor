//! Makes the app icons from the mark: the mark on the dark rounded square,
//! as a 256 x 256 PNG (window and taskbar icon, `assets/kaldor-icon-256.png`)
//! and as a Windows .ico with 16, 24, 32, 48, 64, 128 and 256 px PNG
//! entries (`crates/dj-app/assets/kaldor.ico`, put into the .exe). Exits by
//! itself.
//!
//! ```sh
//! cargo run -p dj-ui --example make_icons -- MARK.svg OUT-256.png OUT.ico
//! ```

use resvg::{tiny_skia, usvg};

/// The inside of an SVG document (between the root's tags).
fn inner(svg: &str) -> &str {
    let start = svg.find('>').map_or(0, |i| i + 1);
    let end = svg.rfind("</svg>").unwrap_or(svg.len());
    &svg[start..end]
}

/// The mark on the dark rounded square (the mark at 72 % of it).
fn icon_svg(mark: &str) -> String {
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect width="100" height="100" rx="22" fill="#121418"/><svg x="14" y="14" width="72" height="72" viewBox="0 0 100 100">{}</svg></svg>"##,
        inner(mark)
    )
}

/// The icon as PNG bytes, `size` pixels square.
fn png(svg: &str, size: u32) -> Option<Vec<u8>> {
    let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(size, size)?;
    let s = size as f32 / tree.size().height();
    resvg::render(
        &tree,
        tiny_skia::Transform::from_scale(s, s),
        &mut pixmap.as_mut(),
    );
    pixmap.encode_png().ok()
}

/// A .ico holding PNG images (Windows Vista and later read these).
fn ico(images: &[(u32, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    let n = images.len() as u16;
    out.extend_from_slice(&[0, 0, 1, 0]);
    out.extend_from_slice(&n.to_le_bytes());
    let mut offset = 6 + 16 * u32::from(n);
    for (size, data) in images {
        // 256 is written as 0.
        let side = if *size >= 256 { 0 } else { *size as u8 };
        out.extend_from_slice(&[side, side, 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in images {
        out.extend_from_slice(data);
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [mark, out_png, out_ico] = args.as_slice() else {
        eprintln!("usage: make_icons MARK.svg OUT-256.png OUT.ico");
        return;
    };
    let Ok(mark) = std::fs::read_to_string(mark) else {
        eprintln!("cannot read {mark}");
        return;
    };
    let svg = icon_svg(&mark);
    let mut images = Vec::new();
    for size in [16, 24, 32, 48, 64, 128, 256] {
        match png(&svg, size) {
            Some(p) => images.push((size, p)),
            None => {
                eprintln!("could not render {size} px");
                return;
            }
        }
    }
    if let Some((_, p256)) = images.last()
        && let Err(e) = std::fs::write(out_png, p256)
    {
        eprintln!("{out_png}: {e}");
        return;
    }
    match std::fs::write(out_ico, ico(&images)) {
        Ok(()) => println!("{out_png}\n{out_ico}"),
        Err(e) => eprintln!("{out_ico}: {e}"),
    }
}
