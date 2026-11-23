//! Screen geometry: where every region goes for a window size, deck layout,
//! mixer switch and divider position. Pure functions (no egui context), so
//! the layout can be tested at every size and scale.

use dj_core::config::DeckLayout;
use egui::{Rect, pos2, vec2};

/// Top bar height with one row / two rows.
pub const TOP_H: f32 = 34.0;
pub const TOP_H_TWO_ROWS: f32 = 60.0;
/// Below this width the top bar uses two rows.
pub const TOP_ONE_ROW_MIN_W: f32 = 900.0;
pub const DIVIDER_H: f32 = 6.0;
pub const LIBRARY_MIN_H: f32 = 110.0;
pub const STAGE_MIN_H: f32 = 150.0;
/// Space between regions.
pub const GAP: f32 = 4.0;
/// LAYER button row at the top of each side column.
pub const LAYER_ROW_H: f32 = 24.0;
/// Deck panel heights (full, compact, mini).
pub const PANEL_FULL_H: f32 = 216.0;
pub const PANEL_COMPACT_H: f32 = 100.0;
pub const PANEL_MINI_H: f32 = 28.0;
/// The mixer: height and the narrowest width it fits in.
pub const MIXER_H: f32 = 218.0;
pub const MIXER_MIN_W: f32 = 804.0;
/// Library toolbar row height; one row from this width up.
pub const TOOLBAR_ROW_H: f32 = 28.0;
pub const TOOLBAR_ONE_ROW_MIN_W: f32 = 1250.0;
/// Library height as a fraction of the window, allowed range.
pub const LIBRARY_FRACTION: (f32, f32) = (0.1, 0.8);
/// Extended layout: deck panel heights (full, compact), the narrowest full
/// panel, and the smallest waveform strip (zoomed only; the overview is in
/// the panel).
pub const EXT_PANEL_FULL_H: f32 = 160.0;
pub const EXT_PANEL_COMPACT_H: f32 = 96.0;
pub const EXT_FULL_MIN_W: f32 = 600.0;
pub const EXT_STRIP_MIN_H: f32 = 40.0;
/// The FX display: its width between the deck halves (Extended), its
/// height as a card there (at most; centred on the decks), and as one
/// line (the least it needs; the other layouts).
pub const FX_PANEL_W: f32 = 188.0;
pub const FX_CARD_H: f32 = 136.0;
pub const FX_PANEL_MIN_H: f32 = 34.0;
/// The widest the FX line gets in the other layouts (centred).
pub const FX_PANEL_STRIP_MAX_W: f32 = 480.0;

/// How much of a deck panel is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelSize {
    /// Everything: title, BPM, key, times, pitch fader and controls,
    /// bend, key lock, load field, CUE and PLAY.
    Full,
    /// Title, BPM, time, CUE and PLAY.
    Compact,
    /// One line: number, title, CUE, PLAY.
    Mini,
}

/// What the mixer area shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixerState {
    /// Under the waveforms.
    Open,
    /// Switched off by the user: bar only.
    Closed,
    /// Switched on but no room under the waveforms: bar only, with a button
    /// to show it in place of the waveforms.
    NoRoom,
    /// Shown in place of the waveforms (scrolls if needed).
    Swapped,
}

/// Inputs to [`compute`].
#[derive(Clone, Copy, Debug)]
pub struct LayoutInput {
    /// Window size in points.
    pub size: egui::Vec2,
    pub layout: DeckLayout,
    /// Show only the two LAYER decks (any layout; `TwoDecks` implies it).
    pub two_decks: bool,
    /// LAYER deck of the left and right side (0-3).
    pub layer: [usize; 2],
    pub mixer_open: bool,
    /// The user asked to see the mixer in place of the waveforms.
    pub mixer_swap: bool,
    /// Library height as a fraction of the window height.
    pub library_fraction: f32,
    /// The FX panel is open (FX in the top bar).
    pub fx_panel: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub deck: usize,
    pub rect: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelSlot {
    pub deck: usize,
    pub rect: Rect,
    pub size: PanelSize,
}

/// Where everything goes.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    pub screen: Rect,
    pub top: Rect,
    pub top_two_rows: bool,
    /// Side columns (decks 1/3 left, 2/4 right), including the LAYER row;
    /// in the Extended layout the two halves of the deck grid.
    pub left: Rect,
    pub right: Rect,
    /// LAYER button rows (none in the Extended layout: the deck badges
    /// switch LAYER there).
    pub layer_rows: Option<[Rect; 2]>,
    pub panels: Vec<PanelSlot>,
    /// Between the side columns: waveforms and mixer.
    pub centre: Rect,
    /// The FX panel when open: between the deck halves (Extended), else a
    /// strip at the top of the centre.
    pub fx: Option<Rect>,
    pub strips: Vec<Slot>,
    pub mixer_state: MixerState,
    /// The MIXER / PADS / FX bar (none in the Extended layout).
    pub mixer_bar: Option<Rect>,
    /// Mixer area when open or swapped.
    pub mixer: Option<Rect>,
    pub divider: Rect,
    /// Library / Setup area.
    pub bottom: Rect,
    pub toolbar_rows: u8,
}

impl Frame {
    /// Moves every rectangle by `v` (the window's content origin).
    pub fn translate(&mut self, v: egui::Vec2) {
        if v == egui::Vec2::ZERO {
            return;
        }
        for r in [
            &mut self.screen,
            &mut self.top,
            &mut self.left,
            &mut self.right,
            &mut self.centre,
            &mut self.divider,
            &mut self.bottom,
        ] {
            *r = r.translate(v);
        }
        if let Some(b) = &mut self.mixer_bar {
            *b = b.translate(v);
        }
        if let Some(f) = &mut self.fx {
            *f = f.translate(v);
        }
        for r in self.layer_rows.iter_mut().flatten() {
            *r = r.translate(v);
        }
        for p in &mut self.panels {
            p.rect = p.rect.translate(v);
        }
        for s in &mut self.strips {
            s.rect = s.rect.translate(v);
        }
        if let Some(m) = &mut self.mixer {
            *m = m.translate(v);
        }
    }
}

/// Decks shown on the left and right side, top to bottom.
pub fn side_decks(two_decks: bool, layer: [usize; 2]) -> [Vec<usize>; 2] {
    if two_decks {
        [vec![layer[0]], vec![layer[1]]]
    } else {
        [vec![0, 2], vec![1, 3]]
    }
}

/// Library height for a window of height `h` (points).
pub fn library_height(h: f32, top_h: f32, fraction: f32) -> f32 {
    let avail = (h - top_h - DIVIDER_H).max(0.0);
    let f = fraction.clamp(LIBRARY_FRACTION.0, LIBRARY_FRACTION.1);
    if avail < LIBRARY_MIN_H + STAGE_MIN_H {
        // Not enough room for both minimums: share it.
        return (avail * 0.4).floor();
    }
    (h * f).clamp(LIBRARY_MIN_H, avail - STAGE_MIN_H).floor()
}

/// The library fraction that puts the divider at `divider_y` (points).
pub fn fraction_for_divider(h: f32, divider_y: f32) -> f32 {
    let lib = h - divider_y - DIVIDER_H / 2.0;
    (lib / h.max(1.0)).clamp(LIBRARY_FRACTION.0, LIBRARY_FRACTION.1)
}

fn side_width(w: f32) -> f32 {
    (w * 0.165).clamp(190.0, 300.0).floor()
}

/// Splits a side column's panel area among its decks.
fn side_panels(area: Rect, decks: &[usize], layer: usize) -> Vec<PanelSlot> {
    let h = area.height();
    let mut out = Vec::new();
    match decks {
        [one] => {
            let size = if h >= PANEL_FULL_H {
                PanelSize::Full
            } else if h >= PANEL_COMPACT_H {
                PanelSize::Compact
            } else {
                PanelSize::Mini
            };
            let rect = if size == PanelSize::Mini {
                Rect::from_min_size(area.min, vec2(area.width(), PANEL_MINI_H.min(h)))
            } else {
                area
            };
            out.push(PanelSlot {
                deck: *one,
                rect,
                size,
            });
        }
        [a, b] => {
            let sizes = if h >= 2.0 * PANEL_FULL_H + GAP {
                [(PanelSize::Full, (h - GAP) / 2.0); 2]
            } else if h >= PANEL_FULL_H + PANEL_COMPACT_H + GAP {
                let full = (PanelSize::Full, h - GAP - PANEL_COMPACT_H);
                let compact = (PanelSize::Compact, PANEL_COMPACT_H);
                if layer == *a {
                    [full, compact]
                } else {
                    [compact, full]
                }
            } else if h >= 2.0 * PANEL_COMPACT_H + GAP {
                [(PanelSize::Compact, (h - GAP) / 2.0); 2]
            } else {
                [(PanelSize::Mini, PANEL_MINI_H.min((h - GAP) / 2.0)); 2]
            };
            let mut y = area.top();
            for (deck, (size, ph)) in [*a, *b].into_iter().zip(sizes) {
                let ph = ph.floor();
                out.push(PanelSlot {
                    deck,
                    rect: Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), ph)),
                    size,
                });
                y += ph + GAP;
            }
        }
        _ => {}
    }
    out
}

/// Strip rows (top to bottom) and columns for the waveform area.
fn strip_grid(layout: DeckLayout, two_decks: bool, layer: [usize; 2]) -> Vec<Vec<usize>> {
    match (layout, two_decks) {
        (DeckLayout::FourGrid, true) => vec![vec![layer[0], layer[1]]],
        (_, true) => vec![vec![layer[0]], vec![layer[1]]],
        (DeckLayout::FourGrid, false) => vec![vec![0, 1], vec![2, 3]],
        (_, false) => vec![vec![0], vec![1], vec![2], vec![3]],
    }
}

/// Height of an Extended layout deck panel.
fn ext_panel_h(size: PanelSize) -> f32 {
    match size {
        PanelSize::Full => EXT_PANEL_FULL_H,
        PanelSize::Compact => EXT_PANEL_COMPACT_H,
        PanelSize::Mini => PANEL_MINI_H,
    }
}

/// The Extended layout: the deck panels in a grid over the full width (1
/// and 3 on the left, 2 and 4 on the right; with 2 decks one row of the
/// LAYER decks), and full-width waveform strips under them. No mixer, pads
/// or FX area. Panels are full when there is
/// room, else compact, else one line. With `fx` the FX panel sits between
/// the two halves.
fn extended(stage: Rect, decks_per_side: &[Vec<usize>; 2], fx: bool) -> ExtParts {
    let fx_w = if fx { FX_PANEL_W + 2.0 * GAP } else { 0.0 };
    let half_w = ((stage.width() - 2.0 * GAP - fx_w) / 2.0).floor();
    let sizes: &[PanelSize] = if half_w >= EXT_FULL_MIN_W {
        &[PanelSize::Full, PanelSize::Compact, PanelSize::Mini]
    } else {
        &[PanelSize::Compact, PanelSize::Mini]
    };
    let rows = decks_per_side[0].len().max(1) as f32;
    let strips = 2.0 * rows;
    let room = |s: PanelSize| {
        let decks = rows * ext_panel_h(s) + (rows - 1.0) * GAP;
        decks + GAP + strips * EXT_STRIP_MIN_H + (strips - 1.0) * GAP <= stage.height()
    };
    let size = sizes
        .iter()
        .copied()
        .find(|s| room(*s))
        .unwrap_or(PanelSize::Mini);
    let ph = ext_panel_h(size).min(((stage.height() - GAP) / rows).floor().max(1.0));
    let decks = Rect::from_min_size(
        stage.min,
        vec2(stage.width(), rows * ph + (rows - 1.0) * GAP),
    );
    let left = Rect::from_min_size(decks.min, vec2(half_w, decks.height()));
    let right = Rect::from_min_max(pos2(decks.right() - half_w, decks.top()), decks.max);
    let mut panels = Vec::new();
    for (side, col) in [left, right].into_iter().enumerate() {
        for (row, deck) in decks_per_side[side].iter().copied().enumerate() {
            panels.push(PanelSlot {
                deck,
                rect: Rect::from_min_size(
                    pos2(col.left(), col.top() + row as f32 * (ph + GAP)),
                    vec2(col.width(), ph),
                ),
                size,
            });
        }
    }
    // The FX display: in the gap between the halves, a card centred on the
    // decks (one line when they are lower than that).
    let fx = fx.then(|| {
        let h = decks
            .height()
            .clamp(FX_PANEL_MIN_H, FX_CARD_H)
            .min(stage.height());
        let top = decks.top() + ((decks.height() - h) / 2.0).max(0.0).floor();
        Rect::from_min_max(
            pos2(left.right() + 2.0 * GAP, top),
            pos2(right.left() - 2.0 * GAP, top + h),
        )
    });
    let top = fx.map_or(decks.bottom(), |f| f.bottom().max(decks.bottom()));
    let centre = Rect::from_min_max(
        pos2(stage.left(), (top + GAP).min(stage.bottom())),
        pos2(stage.right(), stage.bottom()),
    );
    ExtParts {
        left,
        right,
        panels,
        centre,
        fx,
    }
}

struct ExtParts {
    left: Rect,
    right: Rect,
    panels: Vec<PanelSlot>,
    centre: Rect,
    fx: Option<Rect>,
}

fn strips_in(area: Rect, grid: &[Vec<usize>]) -> Vec<Slot> {
    let rows = grid.len().max(1) as f32;
    let row_h = ((area.height() - GAP * (rows - 1.0)) / rows).floor();
    let mut out = Vec::new();
    for (r, row) in grid.iter().enumerate() {
        let cols = row.len().max(1) as f32;
        let col_w = ((area.width() - 2.0 * GAP * (cols - 1.0)) / cols).floor();
        for (c, &deck) in row.iter().enumerate() {
            let x = area.left() + c as f32 * (col_w + 2.0 * GAP);
            let y = area.top() + r as f32 * (row_h + GAP);
            out.push(Slot {
                deck,
                rect: Rect::from_min_size(pos2(x, y), vec2(col_w, row_h)),
            });
        }
    }
    out
}

/// Computes the layout.
pub fn compute(input: &LayoutInput) -> Frame {
    let (w, h) = (input.size.x.max(1.0), input.size.y.max(1.0));
    let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));
    let top_two_rows = w < TOP_ONE_ROW_MIN_W;
    let top_h = if top_two_rows { TOP_H_TWO_ROWS } else { TOP_H };
    let top = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, top_h));
    let lib_h = library_height(h, top_h, input.library_fraction);
    let bottom = Rect::from_min_max(pos2(0.0, h - lib_h), pos2(w, h));
    let divider = Rect::from_min_max(pos2(0.0, bottom.top() - DIVIDER_H), pos2(w, bottom.top()));
    let stage = Rect::from_min_max(
        pos2(GAP, top.bottom() + GAP),
        pos2(w - GAP, divider.top() - GAP),
    );

    // The old "2 decks" layout is Stacked with 2 decks.
    let (layout, two_decks) = match input.layout {
        DeckLayout::TwoDecks => (DeckLayout::FourStacked, true),
        l => (l, input.two_decks),
    };
    let grid = strip_grid(layout, two_decks, input.layer);
    let rows = grid.len() as f32;
    let decks_per_side = side_decks(two_decks, input.layer);
    let extended_layout = layout == DeckLayout::Extended;
    let (left, right, layer_rows, panels, mut centre, mixer_state, mut fx) = if extended_layout {
        let e = extended(stage, &decks_per_side, input.fx_panel);
        (
            e.left,
            e.right,
            None,
            e.panels,
            e.centre,
            MixerState::Closed,
            e.fx,
        )
    } else {
        let side_w = side_width(w);
        let left = Rect::from_min_size(stage.min, vec2(side_w, stage.height()));
        let right = Rect::from_min_size(
            pos2(stage.right() - side_w, stage.top()),
            vec2(side_w, stage.height()),
        );
        let layer_rows = [left, right]
            .map(|c| Rect::from_min_size(c.min, vec2(c.width(), LAYER_ROW_H.min(c.height()))));
        let mut panels = Vec::new();
        for (side, col) in [left, right].into_iter().enumerate() {
            let area =
                Rect::from_min_max(pos2(col.left(), layer_rows[side].bottom() + GAP), col.max);
            if area.height() > 0.0 {
                panels.extend(side_panels(area, &decks_per_side[side], input.layer[side]));
            }
        }
        let centre = Rect::from_min_max(
            pos2(left.right() + 2.0 * GAP, stage.top()),
            pos2(right.left() - 2.0 * GAP, stage.bottom()),
        );
        (
            left,
            right,
            Some(layer_rows),
            panels,
            centre,
            MixerState::Closed,
            None,
        )
    };
    // Other layouts: the FX panel is a strip at the top of the centre,
    // between the side columns.
    if input.fx_panel && !extended_layout && centre.height() > FX_PANEL_MIN_H + GAP {
        let w = centre.width().min(FX_PANEL_STRIP_MAX_W);
        fx = Some(Rect::from_min_size(
            pos2(centre.center().x - w / 2.0, centre.top()),
            vec2(w, FX_PANEL_MIN_H),
        ));
        centre.min.y += FX_PANEL_MIN_H + GAP;
    }
    // No MIXER / PADS / FX bar or area in any layout: the
    // waveforms take the whole centre.
    let (mixer_bar, mixer) = (None, None);
    let strips = if centre.height() >= rows * 12.0 && centre.width() > 40.0 {
        strips_in(centre, &grid)
    } else {
        Vec::new()
    };

    Frame {
        screen,
        top,
        top_two_rows,
        left,
        right,
        layer_rows,
        panels,
        centre,
        fx,
        strips,
        mixer_state,
        mixer_bar,
        mixer,
        divider,
        bottom,
        toolbar_rows: if w >= TOOLBAR_ONE_ROW_MIN_W { 1 } else { 2 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) const SIZES: [(f32, f32); 3] = [(1366.0, 768.0), (1920.0, 1080.0), (3840.0, 2160.0)];
    pub(crate) const SCALES: [f32; 3] = [1.0, 1.5, 2.0];
    pub(crate) const LAYOUTS: [DeckLayout; 4] = [
        DeckLayout::TwoDecks,
        DeckLayout::FourStacked,
        DeckLayout::FourGrid,
        DeckLayout::Extended,
    ];

    fn input(w: f32, h: f32, layout: DeckLayout) -> LayoutInput {
        LayoutInput {
            size: vec2(w, h),
            layout,
            two_decks: false,
            layer: [0, 1],
            mixer_open: true,
            mixer_swap: false,
            library_fraction: 0.30,
            fx_panel: false,
        }
    }

    fn all_rects(f: &Frame) -> Vec<(String, Rect)> {
        let mut v = vec![
            ("top".to_string(), f.top),
            ("divider".into(), f.divider),
            ("bottom".into(), f.bottom),
        ];
        if let Some(b) = f.mixer_bar {
            v.push(("mixer_bar".into(), b));
        }
        for (i, l) in f.layer_rows.iter().flatten().enumerate() {
            v.push((format!("layer row {i}"), *l));
        }
        for p in &f.panels {
            v.push((format!("panel {}", p.deck + 1), p.rect));
        }
        for s in &f.strips {
            v.push((format!("strip {}", s.deck + 1), s.rect));
        }
        if let Some(m) = f.mixer {
            v.push(("mixer".into(), m));
        }
        if let Some(x) = f.fx {
            v.push(("fx".into(), x));
        }
        v
    }

    /// Every size, scale and layout: regions inside the window, none empty,
    /// none overlapping another.
    #[test]
    fn regions_fit_and_never_overlap() {
        for (pw, ph) in SIZES {
            for s in SCALES {
                for layout in LAYOUTS {
                    for (open, swap, fx) in [
                        (true, false, false),
                        (false, false, false),
                        (true, true, false),
                        (false, false, true),
                    ] {
                        let mut inp = input(pw / s, ph / s, layout);
                        inp.mixer_open = open;
                        inp.mixer_swap = swap;
                        inp.fx_panel = fx;
                        let f = compute(&inp);
                        let what =
                            format!("{pw}x{ph}@{s} {layout:?} open {open} swap {swap} fx {fx}");
                        if fx {
                            let r = f.fx.unwrap_or_else(|| panic!("{what}: no FX panel"));
                            assert!(r.height() >= FX_PANEL_MIN_H, "{what}: {r:?}");
                        }
                        let rects = all_rects(&f);
                        for (name, r) in &rects {
                            assert!(
                                r.width() >= 1.0 && r.height() >= 1.0,
                                "{what}: {name} empty {r:?}"
                            );
                            assert!(f.screen.contains_rect(*r), "{what}: {name} outside {r:?}");
                        }
                        for (i, (a, ra)) in rects.iter().enumerate() {
                            for (b, rb) in &rects[i + 1..] {
                                let o = ra.intersect(*rb);
                                assert!(
                                    o.width() <= 0.0 || o.height() <= 0.0,
                                    "{what}: {a} {ra:?} overlaps {b} {rb:?}"
                                );
                            }
                        }
                        // Every deck of the layout has a panel.
                        let want = if layout == DeckLayout::TwoDecks { 2 } else { 4 };
                        assert_eq!(f.panels.len(), want, "{what}");
                        // Strips: one per deck unless the mixer replaced them.
                        if f.mixer_state != MixerState::Swapped {
                            assert_eq!(f.strips.len(), want, "{what}");
                        }
                        for p in &f.panels {
                            let need = match (layout, p.size) {
                                (_, PanelSize::Mini) => 10.0,
                                (DeckLayout::Extended, s) => ext_panel_h(s),
                                (_, PanelSize::Full) => PANEL_FULL_H,
                                (_, PanelSize::Compact) => PANEL_COMPACT_H,
                            };
                            assert!(p.rect.height() >= need, "{what}: panel {p:?}");
                            if layout == DeckLayout::Extended && p.size == PanelSize::Full {
                                assert!(p.rect.width() >= EXT_FULL_MIN_W, "{what}: {p:?}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn playheads_line_up_in_the_stacked_layouts() {
        for layout in [
            DeckLayout::TwoDecks,
            DeckLayout::FourStacked,
            DeckLayout::Extended,
        ] {
            let f = compute(&input(1920.0, 1080.0, layout));
            let x: Vec<f32> = f.strips.iter().map(|s| s.rect.center().x).collect();
            assert!(x.windows(2).all(|w| (w[0] - w[1]).abs() < 0.01), "{x:?}");
            let widths: Vec<f32> = f.strips.iter().map(|s| s.rect.width()).collect();
            assert!(widths.windows(2).all(|w| w[0] == w[1]));
        }
    }

    /// Extended: the 2x2 deck grid (1 and 3 left, 2 and 4 right) over four
    /// full-width strips with their playheads lined up; full panels at
    /// 1920x1080 and 1536x864 (1920x1080 at 125 %), also with the mixer open.
    #[test]
    fn extended_grid_over_full_width_strips() {
        for (w, h) in [(1920.0, 1080.0), (1536.0, 864.0)] {
            for open in [false, true] {
                let mut inp = input(w, h, DeckLayout::Extended);
                inp.mixer_open = open;
                let f = compute(&inp);
                let what = format!("{w}x{h} open {open}");
                assert!(f.layer_rows.is_none());
                let p = |d: usize| f.panels.iter().find(|p| p.deck == d).expect("panel").rect;
                assert!(
                    p(0).left() < p(1).left() && p(0).top() < p(2).top(),
                    "{what}"
                );
                assert_eq!(p(0).left(), p(2).left());
                assert_eq!(p(1).left(), p(3).left());
                assert_eq!(p(0).top(), p(1).top());
                let strips_top = f
                    .strips
                    .iter()
                    .map(|s| s.rect.top())
                    .fold(f32::MAX, f32::min);
                assert!(
                    strips_top > p(3).bottom(),
                    "{what}: strips under the panels"
                );
                let order: Vec<usize> = f.strips.iter().map(|s| s.deck).collect();
                assert_eq!(order, [0, 1, 2, 3]);
                for s in &f.strips {
                    assert!(s.rect.width() > w - 20.0, "{what}: full width {s:?}");
                }
                assert!(
                    f.panels.iter().all(|p| p.size == PanelSize::Full),
                    "{what}: {:?}",
                    f.panels
                );
                // No mixer, pads or FX area, whatever the mixer switch says:
                // the strips reach down to the divider.
                assert!(f.mixer_bar.is_none() && f.mixer.is_none(), "{what}");
                assert_eq!(f.mixer_state, MixerState::Closed, "{what}");
                let strips_bottom = f.strips.iter().map(|s| s.rect.bottom()).fold(0.0, f32::max);
                assert!(f.divider.top() - strips_bottom <= GAP + 1.0, "{what}");
            }
        }
        // Narrow halves: compact panels.
        let f = compute(&input(1366.0 / 1.5, 768.0 / 1.5, DeckLayout::Extended));
        assert!(f.panels.iter().all(|p| p.size != PanelSize::Full));
    }

    #[test]
    fn full_panels_and_no_mixer_area_at_common_sizes() {
        for (w, h) in [(1366.0, 768.0), (1920.0, 1080.0)] {
            for layout in LAYOUTS.into_iter().filter(|l| *l != DeckLayout::Extended) {
                let f = compute(&input(w, h, layout));
                // No MIXER / PADS / FX bar or area in any layout, even with
                // the mixer switch on.
                assert!(
                    f.mixer_bar.is_none() && f.mixer.is_none(),
                    "{w}x{h} {layout:?}"
                );
                assert_eq!(f.mixer_state, MixerState::Closed, "{w}x{h} {layout:?}");
                assert!(
                    f.panels.iter().all(|p| p.size == PanelSize::Full),
                    "{w}x{h} {layout:?}: {:?}",
                    f.panels
                );
            }
        }
    }

    #[test]
    fn panels_collapse_by_layer_when_short() {
        // A side with two decks and room for one full panel only: the LAYER
        // deck keeps the full panel.
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(250.0, 330.0));
        let p = side_panels(area, &[0, 2], 2);
        assert_eq!(p[0].size, PanelSize::Compact);
        assert_eq!(p[1].size, PanelSize::Full);
        let p = side_panels(area, &[0, 2], 0);
        assert_eq!(p[0].size, PanelSize::Full);
        assert_eq!(p[1].size, PanelSize::Compact);
    }

    #[test]
    fn divider_fraction_round_trip() {
        let h = 1000.0;
        let f = fraction_for_divider(h, 600.0);
        let lib = library_height(h, TOP_H, f);
        assert!((h - lib - DIVIDER_H / 2.0 - 600.0).abs() <= 1.0, "{lib}");
        // Clamped so the stage keeps its minimum.
        assert!(library_height(h, TOP_H, 0.99) <= h - TOP_H - DIVIDER_H - STAGE_MIN_H);
    }

    #[test]
    fn small_windows_use_two_rows() {
        let f = compute(&input(683.0, 384.0, DeckLayout::FourStacked));
        assert!(f.top_two_rows);
        assert_eq!(f.toolbar_rows, 2);
    }
}
