//! Colours, sizes and the egui style of the app (our own palette).

use dj_core::DeckId;
use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, vec2};

/// Colours for one theme.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    /// Window background behind the panels.
    pub bg: Color32,
    /// Top bar, library and mixer background.
    pub panel: Color32,
    /// Deck panels and raised areas.
    pub raised: Color32,
    /// Panel borders and separators.
    pub border: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    /// Selection and active toggles (the Kaldor purple).
    pub accent: Color32,
    /// Accent for thin lines and outlines (lighter on dark themes so a
    /// 1-2 px line stays visible).
    pub accent_line: Color32,
    /// Background of waveforms, meters and text fields.
    pub well: Color32,
    /// Playhead on the waveforms.
    pub playhead: Color32,
    pub cue: Color32,
    pub play: Color32,
    pub warn: Color32,
    pub error: Color32,
    pub knob: Color32,
    pub knob_track: Color32,
}

/// Cue marker and CUE button colour (amber).
pub const CUE: Color32 = Color32::from_rgb(255, 170, 0);

impl Palette {
    pub fn get(dark: bool) -> Self {
        if dark {
            Self {
                dark,
                bg: Color32::from_rgb(0x12, 0x14, 0x18),
                panel: Color32::from_rgb(0x1A, 0x1D, 0x23),
                raised: Color32::from_rgb(0x22, 0x26, 0x2E),
                border: Color32::from_rgb(0x30, 0x35, 0x3F),
                text: Color32::from_rgb(0xE4, 0xE7, 0xEC),
                text_weak: Color32::from_rgb(0x8A, 0x93, 0xA0),
                accent: crate::brand::PURPLE,
                accent_line: Color32::from_rgb(0x97, 0x6D, 0xF4),
                well: Color32::from_gray(18),
                playhead: Color32::WHITE,
                cue: CUE,
                play: Color32::from_rgb(0x4C, 0xC7, 0x7A),
                warn: Color32::from_rgb(230, 200, 60),
                error: Color32::from_rgb(230, 90, 90),
                knob: Color32::from_rgb(0x3A, 0x40, 0x4B),
                knob_track: Color32::from_rgb(0x2A, 0x2F, 0x38),
            }
        } else {
            Self {
                dark,
                bg: Color32::from_rgb(0xEC, 0xEE, 0xF1),
                panel: Color32::from_rgb(0xF7, 0xF8, 0xFA),
                raised: Color32::WHITE,
                border: Color32::from_rgb(0xCF, 0xD4, 0xDB),
                text: Color32::from_rgb(0x1D, 0x22, 0x29),
                text_weak: Color32::from_rgb(0x5D, 0x66, 0x72),
                accent: crate::brand::PURPLE,
                accent_line: crate::brand::PURPLE,
                well: Color32::from_gray(235),
                playhead: Color32::BLACK,
                cue: Color32::from_rgb(0xE0, 0x90, 0x00),
                play: Color32::from_rgb(0x2E, 0xA0, 0x5A),
                warn: Color32::from_rgb(0xB8, 0x90, 0x10),
                error: Color32::from_rgb(0xC8, 0x3C, 0x3C),
                knob: Color32::from_rgb(0xDD, 0xE1, 0xE7),
                knob_track: Color32::from_rgb(0xC4, 0xCA, 0xD2),
            }
        }
    }

    /// Text colour on top of `fill` (black on light fills, white on dark).
    pub fn on(fill: Color32) -> Color32 {
        let l =
            0.299 * f32::from(fill.r()) + 0.587 * f32::from(fill.g()) + 0.114 * f32::from(fill.b());
        if l > 140.0 {
            Color32::from_gray(15)
        } else {
            Color32::WHITE
        }
    }
}

/// A colour stored as `0xRRGGBB` (cue and track colours).
pub fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// Deck accent colours.
pub fn deck_color(id: DeckId) -> Color32 {
    match id.index() {
        0 => Color32::from_rgb(70, 150, 255),
        1 => Color32::from_rgb(255, 140, 50),
        2 => Color32::from_rgb(80, 210, 120),
        _ => Color32::from_rgb(200, 110, 255),
    }
}

/// Monospace font for big numbers (BPM, time).
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}

/// Proportional font.
pub fn prop(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

/// The display face (the signature look): Archivo
/// Expanded ExtraBold, as the logo and the FX display. For names and
/// captions in capitals (CUE, SYNC, the effect's name), never for running
/// text. It is wider than the text face, so a caption at `size` takes
/// about the room of the text face at `size / DISPLAY_SCALE`.
pub fn display(ctx: &egui::Context, size: f32) -> FontId {
    logo_font(ctx, size)
}

/// Display captions are drawn at this share of the size the text face
/// would have, so they fit the same buttons.
pub const DISPLAY_SCALE: f32 = 0.8;

/// A caption that takes the display face: it has letters and none of them
/// is lower case ("CUE", "LOOP 4", "1/2"; not "New crate", not "÷2").
pub fn is_caption(text: &str) -> bool {
    text.chars().any(|c| c.is_alphabetic()) && !text.chars().any(|c| c.is_lowercase())
}

/// A small label in capitals in the weak colour, as "FX" and "COLOR" on
/// the FX display: the name of what follows ("BPM", "COLOR", a settings
/// section). Returns the painted rectangle.
pub fn caps_label(
    painter: &egui::Painter,
    pos: egui::Pos2,
    align: egui::Align2,
    text: &str,
    pal: &Palette,
) -> egui::Rect {
    painter.text(pos, align, text.to_uppercase(), prop(10.0), pal.text_weak)
}

/// Font family of the Kaldor name in the top bar.
pub const LOGO_FONT: &str = "Archivo";

/// Archivo (variable font; Copyright 2020 The Archivo Project Authors, SIL
/// Open Font License 1.1, see `assets/fonts/`), bundled for the name.
const ARCHIVO: &[u8] = include_bytes!("../assets/fonts/Archivo-VariableFont_wdth,wght.ttf");

/// The name in Archivo Expanded ExtraBold: weight 800, width 125.
const LOGO_WEIGHT: f32 = 800.0;
const LOGO_WIDTH: f32 = 125.0;
/// Tracking of the name, in thousandths of the font size.
pub const LOGO_TRACKING: f32 = 140.0 / 1000.0;

fn logo_font_id() -> egui::Id {
    egui::Id::unique("kaldor_logo_font_loaded")
}

/// Adds Archivo (Expanded ExtraBold) as the font family of the Kaldor
/// name, once per egui context.
pub fn install_logo_font(ctx: &egui::Context) {
    if ctx.data(|d| d.get_temp::<bool>(logo_font_id())) == Some(true) {
        return;
    }
    let mut data = egui::FontData::from_static(ARCHIVO);
    data.tweak.coords =
        egui::epaint::text::VariationCoords::new([(b"wght", LOGO_WEIGHT), (b"wdth", LOGO_WIDTH)]);
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert(LOGO_FONT.to_owned(), std::sync::Arc::new(data));
    fonts.families.insert(
        FontFamily::Name(LOGO_FONT.into()),
        vec![LOGO_FONT.to_owned()],
    );
    ctx.set_fonts(fonts);
    ctx.data_mut(|d| d.insert_temp(logo_font_id(), true));
}

/// The font of the Kaldor name: Archivo once it is installed in the
/// context, else the app's proportional font.
pub fn logo_font(ctx: &egui::Context, size: f32) -> FontId {
    if ctx.data(|d| d.get_temp::<bool>(logo_font_id())) == Some(true) {
        FontId::new(size, FontFamily::Name(LOGO_FONT.into()))
    } else {
        prop(size)
    }
}

/// Applies the palette and sizes to egui.
pub fn apply(ctx: &egui::Context, dark: bool) {
    let p = Palette::get(dark);
    let mut v = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    v.panel_fill = p.panel;
    v.window_fill = p.raised;
    v.window_stroke = Stroke::new(1.0, p.border);
    v.extreme_bg_color = p.well;
    v.faint_bg_color = p.raised;
    v.selection.bg_fill = p.accent.gamma_multiply(if dark { 0.55 } else { 0.35 });
    v.selection.stroke = Stroke::new(1.0, p.accent_line);
    v.hyperlink_color = p.accent_line;
    v.warn_fg_color = p.warn;
    v.error_fg_color = p.error;
    let r = CornerRadius::same(4);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(6);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.panel;
    w.noninteractive.weak_bg_fill = p.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.text);
    w.noninteractive.corner_radius = r;
    for (wv, lift) in [
        (&mut w.inactive, 0.0f32),
        (&mut w.hovered, 0.08),
        (&mut w.active, 0.14),
        (&mut w.open, 0.08),
    ] {
        let base = if dark {
            lighten(p.raised, 0.05 + lift)
        } else {
            darken(p.raised, 0.04 + lift)
        };
        wv.bg_fill = base;
        wv.weak_bg_fill = base;
        wv.corner_radius = r;
        wv.fg_stroke = Stroke::new(1.0, p.text);
        wv.bg_stroke = if lift > 0.0 {
            Stroke::new(1.0, p.accent_line.gamma_multiply(0.8))
        } else {
            Stroke::new(1.0, p.border)
        };
    }
    ctx.set_visuals(v);
    ctx.all_styles_mut(|s| {
        s.text_styles = [
            (TextStyle::Small, prop(10.5)),
            (TextStyle::Body, prop(13.5)),
            (TextStyle::Button, prop(13.5)),
            (TextStyle::Heading, prop(17.0)),
            (TextStyle::Monospace, mono(13.0)),
        ]
        .into();
        s.spacing.item_spacing = vec2(6.0, 4.0);
        s.spacing.button_padding = vec2(7.0, 2.0);
        s.spacing.interact_size.y = 20.0;
    });
}

fn lighten(c: Color32, k: f32) -> Color32 {
    let f = |x: u8| (f32::from(x) + (255.0 - f32::from(x)) * k).round() as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

fn darken(c: Color32, k: f32) -> Color32 {
    let f = |x: u8| (f32::from(x) * (1.0 - k)).round() as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captions_in_capitals_take_the_display_face() {
        for c in ["CUE", "PAUSE", "LOOP 4", "1/2 BEAT", "REC"] {
            assert!(is_caption(c), "{c}");
        }
        for c in ["New crate", "Files", "÷2", "4", "", "+6 dB"] {
            assert!(!is_caption(c), "{c}");
        }
    }
}
