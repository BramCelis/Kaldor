//! Waveform drawing: the zoomed scrolling view and the overview strip.
//!
//! Waveforms are drawn as one egui mesh per view (two triangles per pixel
//! column), which egui renders on the GPU through wgpu. A dedicated
//! instanced wgpu pipeline is planned if this misses the 2 ms budget.

use dj_analysis::{WavePoint, Waveform};
use dj_core::config::WaveformColorMode;
use egui::{Color32, Mesh, Pos2, Rect, Shape, Stroke, pos2};

/// Maps track frames to screen x for a zoomed view, aligned to whole screen
/// pixels.
///
/// The view has one waveform column per screen (physical) pixel; column `k`
/// shows the track frames `[k * fpp, (k + 1) * fpp)`. The first column is
/// the whole column nearest to `position - frames_visible / 2`, so from one
/// frame to the next the picture moves by whole screen pixels, and the
/// waveform, the grid lines and the markers (all placed with
/// [`ZoomMap::x_of`]) move together. Earlier versions slid the waveform by
/// sub-pixel offsets that the GPU rounded to pixels while the anti-aliased
/// lines moved smoothly, so the two wobbled against each other, and with
/// display scaling the columns were 1.25 or 1.5 pixels wide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZoomMap {
    /// Left edge in points, on a screen pixel boundary.
    pub left: f32,
    /// Screen pixel size in points (1 / pixels per point).
    pub px: f32,
    /// Track frames per screen pixel.
    pub fpp: f64,
    /// Index of the first column.
    pub k0: i64,
}

impl ZoomMap {
    pub fn new(rect: Rect, position: f64, frames_visible: f64, pixels_per_point: f32) -> Self {
        Self::with_playhead(rect, position, frames_visible, pixels_per_point, 0.5)
    }

    /// Like [`Self::new`], with the playhead at `at` of the width (0.5:
    /// the middle).
    pub fn with_playhead(
        rect: Rect,
        position: f64,
        frames_visible: f64,
        pixels_per_point: f32,
        at: f32,
    ) -> Self {
        let at = f64::from(at.clamp(0.1, 0.9));
        let ppp = pixels_per_point.max(0.25);
        let width_px = f64::from(rect.width().max(1.0) * ppp);
        let fpp = (frames_visible / width_px).max(f64::MIN_POSITIVE);
        Self {
            left: (rect.left() * ppp).round() / ppp,
            px: 1.0 / ppp,
            fpp,
            k0: ((position - frames_visible * at) / fpp).round() as i64,
        }
    }

    /// Track frame at the left edge.
    pub fn start(&self) -> f64 {
        self.k0 as f64 * self.fpp
    }

    /// Screen x (points) of track frame `f`.
    pub fn x_of(&self, f: f64) -> f32 {
        self.left + ((f - self.start()) / self.fpp) as f32 * self.px
    }
}

/// Smooth playhead for scrolling waveforms.
///
/// The engine reports a deck position per audio block, and the amount of
/// audio rendered between two screen frames varies (in exclusive mode the
/// device takes 3 ms periods, so 15 ms one frame and 18 ms the next). Drawing
/// that raw position makes the waveform judder by about a pixel. This clock
/// advances by real time at the deck's tempo and is pulled gently towards
/// the reported position; jumps (cue, seek, loop) snap at once.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollClock {
    shown: Option<f64>,
}

impl ScrollClock {
    /// Time constant of the pull towards the reported position.
    const TAU_SECONDS: f64 = 0.08;
    /// Differences larger than this are jumps, not jitter.
    const SNAP_SECONDS: f64 = 0.1;

    /// Next position to draw. `reported` is the engine's position in frames,
    /// `tempo` the deck speed, `rate` the sample rate and `dt` the seconds
    /// since the previous frame.
    pub fn update(&mut self, reported: f64, tempo: f64, rate: f64, dt: f64) -> f64 {
        self.update_with_tau(reported, tempo, rate, dt, Self::TAU_SECONDS)
    }

    /// Like [`Self::update`] with another pull time constant (shorter while
    /// scrubbing or scratching, so the waveform follows the hand closely but
    /// without the jog's bursts).
    pub fn update_with_tau(
        &mut self,
        reported: f64,
        tempo: f64,
        rate: f64,
        dt: f64,
        tau: f64,
    ) -> f64 {
        let dt = dt.clamp(0.0, 0.1);
        let shown = match self.shown {
            None => reported,
            Some(s) => {
                let predicted = s + tempo * rate * dt;
                let err = reported - predicted;
                if err.abs() > Self::SNAP_SECONDS * rate {
                    reported
                } else {
                    predicted + err * (1.0 - (-dt / tau.max(1e-3)).exp())
                }
            }
        };
        let shown = shown.max(0.0);
        self.shown = Some(shown);
        shown
    }

    /// Forget the history (paused, scrubbing, new track).
    pub fn reset(&mut self) {
        self.shown = None;
    }
}

/// Beatgrid colours: bar lines (beat 1) and other beats.
const GRID_BAR: Color32 = Color32::from_rgba_premultiplied(240, 240, 240, 240);
const GRID_BEAT: Color32 = Color32::from_rgba_premultiplied(150, 150, 150, 150);

/// Colour for one waveform column.
pub fn point_color(p: WavePoint, mode: WaveformColorMode) -> Color32 {
    match mode {
        WaveformColorMode::ThreeBand => {
            let (l, m, h) = (f32::from(p.low), f32::from(p.mid), f32::from(p.high));
            let max = l.max(m).max(h).max(1.0);
            let k = 255.0 / max;
            // Lows red, mids green, highs blue, normalised so the dominant
            // band is fully bright.
            Color32::from_rgb(
                (l * k).min(255.0) as u8,
                (m * k * 0.9).min(255.0) as u8,
                (h * k).min(255.0) as u8,
            )
        }
        WaveformColorMode::Monochrome => {
            let v = 140 + p.peak / 3;
            Color32::from_gray(v)
        }
        WaveformColorMode::Blue => {
            let h = f32::from(p.high) / 255.0;
            Color32::from_rgb(
                (40.0 + 200.0 * h) as u8,
                (110.0 + 140.0 * h) as u8,
                (230.0 + 25.0 * h) as u8,
            )
        }
    }
}

/// Adds a vertical bar per column to `mesh`.
pub(crate) fn push_bar(mesh: &mut Mesh, x0: f32, x1: f32, top: f32, bottom: f32, color: Color32) {
    let i = mesh.vertices.len() as u32;
    let uv = egui::epaint::WHITE_UV;
    for (x, y) in [(x0, top), (x1, top), (x1, bottom), (x0, bottom)] {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: pos2(x, y),
            uv,
            color,
        });
    }
    mesh.indices
        .extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
}

/// The waveform of one screen column covering seconds `[a, b)`. A column
/// as wide as a waveform point or wider shows the loudest point in it;
/// a narrower one (zoomed in far) shows the waveform blended between the
/// two nearest points at its middle, so the outline and the colours run
/// smoothly instead of in blocks a point wide.
pub fn column_point(wf: &Waveform, a: f64, b: f64) -> Option<WavePoint> {
    if b - a >= wf.point_seconds() {
        wf.span_seconds(a, b)
    } else {
        wf.at_seconds(0.5 * (a + b))
    }
}

/// What to draw in a zoomed view.
pub struct ZoomedParams<'a> {
    pub waveform: Option<&'a Waveform>,
    /// Playhead position in frames (estimated against the audio clock).
    pub position: f64,
    /// Frames visible across the full width.
    pub frames_visible: f64,
    pub length: f64,
    pub cue_point: Option<f64>,
    pub colors: WaveformColorMode,
    pub background: Color32,
    pub playhead: Color32,
    /// Beatgrid (times in seconds), drawn as lines with bars emphasised.
    pub grid: Option<dj_core::Beatgrid>,
    /// Engine sample rate: positions are frames at this rate.
    pub device_rate: f64,
    /// Hot cues, loop and slip position.
    pub markers: Markers,
    /// Bar numbers next to the bar lines (bar 1 is the bar BEAT 1 was set
    /// on, else the first downbeat of the track; the bar before it is -1).
    pub bar_numbers: bool,
    /// Beat and bar lines.
    pub grid_lines: bool,
    /// Where the playhead is across the width (0.5: the middle).
    pub playhead_at: f32,
}

/// Closest that thin grid lines (beats, and bars between the strong ones)
/// are drawn, in points.
const MIN_LINE_GAP: f32 = 20.0;

/// Every how many bars a strong bar line (with markers and number) is
/// drawn, for bars `px_per_bar` points apart: 1, 2, 4, 8 or 16, the
/// smallest that keeps them at least 96 points apart.
pub fn major_bar_step(px_per_bar: f32) -> i64 {
    [1, 2, 4, 8, 16]
        .into_iter()
        .find(|s| px_per_bar * *s as f32 >= 96.0)
        .unwrap_or(16)
}

/// Bar number of downbeat `n` of `g`: 1 is the bar BEAT 1 was set on (or
/// the first downbeat of the track), -1 the one before; counted with the grid (see
/// [`dj_core::Beatgrid::bar_number`]).
pub fn bar_number(g: &dj_core::Beatgrid, n: i64) -> i64 {
    g.bar_number(n)
}

/// Hot cues, the loop and the slip position (frames).
#[derive(Clone, Copy, Debug, Default)]
pub struct Markers {
    /// Hot cue position and colour per pad.
    pub hot_cues: [Option<(f64, Color32)>; 8],
    /// Loop start, end and whether it is active.
    pub loop_region: Option<(f64, f64, bool)>,
    /// A loop in point waiting for its out point.
    pub loop_in: Option<f64>,
    /// Where the deck would be without the slip action.
    pub slip: Option<f64>,
}

/// Loop colour (active; inactive loops are drawn fainter).
const LOOP_COLOR: Color32 = Color32::from_rgb(70, 200, 110);

/// Draws the zoomed waveform with a fixed playhead in the middle.
pub fn paint_zoomed(painter: &egui::Painter, rect: Rect, p: &ZoomedParams<'_>) {
    painter.rect_filled(rect, 2.0, p.background);
    let map = ZoomMap::with_playhead(
        rect,
        p.position,
        p.frames_visible,
        painter.pixels_per_point(),
        p.playhead_at,
    );
    let (fpp, px) = (map.fpp, map.px);
    let start = map.start();
    let mid_y = rect.center().y;
    let half_h = rect.height() / 2.0 - 1.0;
    let sr = p.device_rate.max(1.0);

    if let Some(wf) = p.waveform {
        let mut mesh = Mesh::default();
        let cols = ((rect.right() - map.left) / px).ceil().max(0.0) as usize;
        mesh.vertices.reserve(cols * 12);
        mesh.indices.reserve(cols * 18);
        // The centre line on a screen pixel boundary, so the solid part of
        // each column ends on whole pixels.
        let mid = (mid_y / px).round() * px;
        // Columns are fixed slices of the track, one screen pixel wide, so a
        // column shows the same peak in every frame (flicker).
        for c in 0..cols {
            let a = (map.k0 + c as i64) as f64 * fpp;
            let b = a + fpp;
            if b <= 0.0 || a >= p.length {
                continue;
            }
            if let Some(pt) = column_point(wf, a / sr, b / sr) {
                let h = f32::from(pt.peak) / 255.0 * half_h;
                if h < 0.5 {
                    continue;
                }
                let x0 = map.left + c as f32 * px;
                let (x0, x1) = (x0.max(rect.left()), (x0 + px).min(rect.right()));
                if x1 <= x0 {
                    continue;
                }
                // Whole screen pixels solid, and the part of a pixel the
                // height reaches into as a faint edge (anti-aliased), so a
                // slow decay shows no pixel steps.
                let color = point_color(pt, p.colors);
                let solid = (h / px).floor() * px;
                let edge = h / px - (h / px).floor();
                push_bar(&mut mesh, x0, x1, mid - solid, mid + solid, color);
                if edge > 0.02 {
                    let faint = color.gamma_multiply(edge);
                    push_bar(&mut mesh, x0, x1, mid - solid - px, mid - solid, faint);
                    push_bar(&mut mesh, x0, x1, mid + solid, mid + solid + px, faint);
                }
            }
        }
        painter.add(Shape::mesh(mesh));
    }

    // Beatgrid on top of the waveform (behind it, a full-height waveform hid
    // it). Each line has a dark outline so it shows on bright and dark parts
    // alike; bar lines are stronger and get markers at the top and bottom.
    if let Some(g) = p.grid.filter(|_| p.grid_lines || p.bar_numbers) {
        let t0 = start / sr;
        let t1 = (start + p.frames_visible) / sr;
        // Beat lines (and thin bar lines) only while they are at least
        // MIN_LINE_GAP apart: closer they are a comb that hides the waveform.
        let px_per_beat = (g.period() * sr / fpp) as f32 * px;
        let outline = |a: u8| Color32::from_rgba_unmultiplied(0, 0, 0, a);
        // Zoomed out, only every 2nd, 4th, ... bar (from bar 1: phrases)
        // is a strong line with markers and a number; the others are thin
        // lines while they are not too close.
        let step = major_bar_step(px_per_beat * 4.0);
        let px_per_major = px_per_beat * 4.0 * step as f32;
        // Bar numbers only where a bar is wide enough for its label.
        let numbers = p.bar_numbers && px_per_major >= 28.0 && rect.height() >= 24.0;
        for (n, t, bar_line) in g.beats_between(t0, t1) {
            let major = bar_line && g.bar_index(n).rem_euclid(step) == 0;
            // A bar line between the major ones is drawn like a beat.
            let down = major;
            if bar_line && !major && px_per_beat * 4.0 < MIN_LINE_GAP {
                continue;
            }
            if numbers && down {
                let x = map.x_of(t * sr);
                if x >= rect.left() && x + 4.0 < rect.right() {
                    let label = bar_number(&g, n).to_string();
                    let font = egui::FontId::proportional(10.5);
                    let at = pos2(x + 5.0, rect.top() + 1.0);
                    painter.text(
                        at + egui::vec2(1.0, 1.0),
                        egui::Align2::LEFT_TOP,
                        &label,
                        font.clone(),
                        outline(200),
                    );
                    painter.text(at, egui::Align2::LEFT_TOP, &label, font, GRID_BAR);
                }
            }
            if !p.grid_lines || (!bar_line && px_per_beat < MIN_LINE_GAP) {
                continue;
            }
            let x = map.x_of(t * sr);
            if x < rect.left() || x > rect.right() {
                continue;
            }
            let line = [pos2(x, rect.top()), pos2(x, rect.bottom())];
            if down {
                painter.line_segment(line, Stroke::new(3.5, outline(150)));
                painter.line_segment(line, Stroke::new(1.5, GRID_BAR));
                let m = 5.0;
                for (y, dir) in [(rect.top(), 1.0), (rect.bottom(), -1.0)] {
                    painter.add(Shape::convex_polygon(
                        vec![pos2(x - m, y), pos2(x + m, y), pos2(x, y + dir * m * 1.2)],
                        GRID_BAR,
                        Stroke::new(1.0, outline(150)),
                    ));
                }
            } else {
                painter.line_segment(line, Stroke::new(2.5, outline(90)));
                painter.line_segment(line, Stroke::new(1.0, GRID_BEAT));
            }
        }
    }

    // Track start and end.
    for edge in [0.0, p.length] {
        let x = map.x_of(edge);
        if x >= rect.left() && x <= rect.right() {
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(1.0, Color32::from_gray(90)),
            );
        }
    }
    if let Some(cue) = p.cue_point {
        let x = map.x_of(cue);
        if x >= rect.left() && x <= rect.right() {
            let c = Color32::from_rgb(255, 170, 0);
            let line = [pos2(x, rect.top()), pos2(x, rect.bottom())];
            painter.line_segment(line, Stroke::new(4.0, Color32::from_black_alpha(170)));
            painter.line_segment(line, Stroke::new(2.0, c));
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(x - 5.0, rect.top()),
                    pos2(x + 5.0, rect.top()),
                    pos2(x, rect.top() + 6.0),
                ],
                c,
                Stroke::NONE,
            ));
        }
    }
    paint_markers(painter, rect, &p.markers, |f| map.x_of(f), true);
    let cx = map.x_of(p.position);
    painter.line_segment(
        [pos2(cx, rect.top()), pos2(cx, rect.bottom())],
        Stroke::new(2.0, p.playhead),
    );
}

/// Draws the whole-track overview with playhead and cue marker.
#[allow(clippy::too_many_arguments)]
pub fn paint_overview(
    painter: &egui::Painter,
    rect: Rect,
    waveform: Option<&Waveform>,
    length: f64,
    ready: f64,
    position: f64,
    cue_point: Option<f64>,
    colors: WaveformColorMode,
    background: Color32,
    playhead: Color32,
    device_rate: f64,
    markers: &Markers,
    triangles: bool,
) {
    painter.rect_filled(rect, 2.0, background);
    if length <= 0.0 {
        return;
    }
    let width = rect.width().max(1.0);
    let fpp = length / f64::from(width);
    let device_rate = device_rate.max(1.0);
    let mid_y = rect.center().y;
    let half_h = rect.height() / 2.0 - 1.0;
    if let Some(wf) = waveform {
        let mut mesh = Mesh::default();
        for c in 0..width.ceil() as usize {
            let a = c as f64 * fpp;
            if let Some(pt) = wf.span_seconds(a / device_rate, (a + fpp) / device_rate) {
                let h = (f32::from(pt.peak) / 255.0 * half_h).max(0.5);
                let x0 = rect.left() + c as f32;
                let mut col = point_color(pt, colors);
                if a > position {
                    col = col.gamma_multiply(0.55);
                }
                push_bar(&mut mesh, x0, x0 + 1.0, mid_y - h, mid_y + h, col);
            }
        }
        painter.add(Shape::mesh(mesh));
    } else if ready < length {
        // Loading: show decode progress.
        let x = rect.left() + (ready / length) as f32 * width;
        painter.rect_filled(
            Rect::from_min_max(rect.min, Pos2::new(x, rect.max.y)),
            2.0,
            Color32::from_gray(70),
        );
    }
    let x_of = |f: f64| rect.left() + (f / length) as f32 * width;
    if triangles {
        // Deck panel overview (Extended): small triangles on the edges, no
        // line through the waveform. Loop and slip as usual.
        paint_markers(
            painter,
            rect,
            &Markers {
                hot_cues: [None; 8],
                ..*markers
            },
            x_of,
            false,
        );
        let outline = Stroke::new(1.0, Color32::from_black_alpha(200));
        let (hw, h) = (5.5, (rect.height() * 0.45).clamp(5.0, 9.0));
        for (f, color) in markers.hot_cues.iter().flatten() {
            let x = x_of(*f);
            if x >= rect.left() - hw && x <= rect.right() + hw {
                painter.add(Shape::convex_polygon(
                    vec![
                        pos2(x - hw, rect.bottom()),
                        pos2(x, rect.bottom() - h),
                        pos2(x + hw, rect.bottom()),
                    ],
                    *color,
                    outline,
                ));
            }
        }
        if let Some(cue) = cue_point {
            let x = x_of(cue);
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(x - hw, rect.top()),
                    pos2(x + hw, rect.top()),
                    pos2(x, rect.top() + h),
                ],
                Color32::WHITE,
                outline,
            ));
        }
    } else {
        if let Some(cue) = cue_point {
            let x = x_of(cue);
            let line = [pos2(x, rect.top()), pos2(x, rect.bottom())];
            painter.line_segment(line, Stroke::new(4.0, Color32::from_black_alpha(170)));
            painter.line_segment(line, Stroke::new(2.0, Color32::from_rgb(255, 170, 0)));
        }
        paint_markers(painter, rect, markers, x_of, false);
    }
    let x = rect.left() + (position / length).clamp(0.0, 1.0) as f32 * width;
    painter.line_segment(
        [pos2(x, rect.top()), pos2(x, rect.bottom())],
        Stroke::new(2.0, playhead),
    );
}

/// Draws the loop region, loop in point, hot cues (numbered flags in the
/// zoomed view, ticks in the overview) and the slip ghost playhead. `x_of`
/// maps a frame to an x position.
pub fn paint_markers(
    painter: &egui::Painter,
    rect: Rect,
    m: &Markers,
    x_of: impl Fn(f64) -> f32,
    zoomed: bool,
) {
    let clip = |x: f32| x.clamp(rect.left(), rect.right());
    // The loop: an overlay in the loop colour with thin solid edges (the
    // first version was too faint, bands and thick edges too heavy).
    // An exited loop is drawn fainter.
    if let Some((a, b, active)) = m.loop_region {
        let (xa, xb) = (x_of(a), x_of(b));
        if xb >= rect.left() && xa <= rect.right() {
            let span =
                Rect::from_min_max(pos2(clip(xa), rect.top()), pos2(clip(xb), rect.bottom()));
            painter.rect_filled(
                span,
                0.0,
                LOOP_COLOR.gamma_multiply(if active { 0.34 } else { 0.12 }),
            );
            let edge = Stroke::new(
                if active { 2.0 } else { 1.0 },
                LOOP_COLOR.gamma_multiply(if active { 1.0 } else { 0.5 }),
            );
            for x in [xa, xb] {
                if x >= rect.left() && x <= rect.right() {
                    painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], edge);
                }
            }
        }
    }
    if let Some(a) = m.loop_in {
        let x = x_of(a);
        if x >= rect.left() && x <= rect.right() {
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(2.0, LOOP_COLOR),
            );
        }
    }
    // Hot cues (thin lines got lost in the waveform and next to the
    // bar lines). Each cue is a thick line with a dark outline, plus a
    // numbered flag at the top in the zoomed view or a tab in the overview.
    let outline = Color32::from_rgba_unmultiplied(0, 0, 0, 170);
    for (k, c) in m.hot_cues.iter().enumerate() {
        let Some((f, color)) = c else {
            continue;
        };
        let x = x_of(*f);
        let flag_w = if zoomed { 15.0 } else { 9.0 };
        if x < rect.left() - flag_w || x > rect.right() {
            continue;
        }
        if x >= rect.left() {
            let line = [pos2(x, rect.top()), pos2(x, rect.bottom())];
            let w = if zoomed { 2.5 } else { 2.0 };
            painter.line_segment(line, Stroke::new(w + 2.0, outline));
            painter.line_segment(line, Stroke::new(w, *color));
        }
        let flag_h = if zoomed { 15.0 } else { 9.0 };
        if rect.height() < flag_h + 6.0 {
            continue;
        }
        // Flag right of the line, at the top.
        let flag = Rect::from_min_size(pos2(x, rect.top()), egui::vec2(flag_w, flag_h));
        let flag = flag.intersect(rect);
        if flag.width() <= 2.0 {
            continue;
        }
        painter.rect_filled(flag.expand(1.0).intersect(rect), 2.0, outline);
        painter.rect_filled(flag, 2.0, *color);
        if zoomed {
            painter.text(
                flag.center(),
                egui::Align2::CENTER_CENTER,
                format!("{}", k + 1),
                crate::theme::prop(11.5),
                crate::theme::Palette::on(*color),
            );
        }
    }
    if let Some(s) = m.slip {
        let x = x_of(s);
        if x >= rect.left() && x <= rect.right() {
            painter.line_segment(
                [pos2(x, rect.top()), pos2(x, rect.bottom())],
                Stroke::new(2.0, Color32::from_rgba_unmultiplied(255, 255, 255, 110)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The view moves by whole screen pixels: at any position, any track
    /// frame lands at the same sub-pixel offset as before (so the waveform
    /// columns, the grid lines and the markers keep their places relative
    /// to each other), with display scaling too.
    #[test]
    fn zoom_map_moves_by_whole_screen_pixels() {
        let rect = Rect::from_min_size(pos2(100.3, 10.0), egui::vec2(900.0, 120.0));
        for ppp in [1.0f32, 1.25, 1.5, 2.0] {
            let frames_visible = 4.0 * 48_000.0;
            let a = ZoomMap::new(rect, 1_000_000.0, frames_visible, ppp);
            // The left edge is on a screen pixel boundary.
            assert!(((a.left * ppp) - (a.left * ppp).round()).abs() < 1e-3);
            // One column per screen pixel.
            assert!((a.fpp - frames_visible / f64::from(900.0 * ppp)).abs() < 1e-9);
            let beat = 1_000_000.0 + 12_345.6;
            let x0 = a.x_of(beat);
            for step in [0.3, 7.7, 1_234.5, 48_000.0 / 60.0] {
                let b = ZoomMap::new(rect, 1_000_000.0 + step, frames_visible, ppp);
                let moved = (x0 - b.x_of(beat)) * ppp;
                assert!(
                    (moved - moved.round()).abs() < 1e-2,
                    "ppp {ppp}, step {step}: moved {moved} screen pixels"
                );
                // The playhead (the view centre) is within half a column.
                let centre = b.x_of(1_000_000.0 + step);
                assert!((centre - rect.center().x).abs() * ppp <= 1.0, "{centre}");
            }
        }
    }

    fn dj_audio_free_track(sr: u32, samples: &[f32]) -> std::sync::Arc<dj_decode::TrackAudio> {
        dj_decode::TrackAudio::from_interleaved(sr, samples)
    }

    /// The column heights over `seconds` on `cols` columns: the largest
    /// jump between neighbours and how many neighbours are equal.
    fn steps(
        wf: &Waveform,
        seconds: (f64, f64),
        cols: usize,
        f: fn(&Waveform, f64, f64) -> Option<WavePoint>,
    ) -> (i32, usize) {
        let w = (seconds.1 - seconds.0) / cols as f64;
        let peaks: Vec<i32> = (0..cols)
            .filter_map(|c| {
                let a = seconds.0 + c as f64 * w;
                f(wf, a, a + w).map(|p| i32::from(p.peak))
            })
            .collect();
        let jump = peaks
            .windows(2)
            .map(|p| (p[1] - p[0]).abs())
            .max()
            .unwrap_or(0);
        let flat = peaks.windows(2).filter(|p| p[0] == p[1]).count();
        (jump, flat)
    }

    /// Zoomed in to 1 beat across 1200 pixels, the 200/s waveform drawn as
    /// before is blocks 12 pixels wide: flat, then a jump. Blended (and from
    /// the detail waveform) the outline of a swelling tone changes a little
    /// at every pixel instead.
    #[test]
    fn zoomed_in_waveforms_have_no_blocks() {
        // A 1 kHz tone whose level swells and fades three times a second.
        let sr = 44_100u32;
        let samples: Vec<f32> = (0..sr as usize * 2)
            .flat_map(|i| {
                let t = i as f64 / f64::from(sr);
                let amp = 0.5 + 0.4 * (2.0 * std::f64::consts::PI * 3.0 * t).sin();
                let v = (amp * (2.0 * std::f64::consts::PI * 1000.0 * t).sin()) as f32;
                [v, v]
            })
            .collect();
        let track = dj_audio_free_track(sr, &samples);
        let base = Waveform::build(&track);
        let detail = Waveform::build_detail(&track);
        let span = (0.5, 1.0);
        let (old_jump, old_flat) = steps(&base, span, 1200, |w, a, b| w.span_seconds(a, b));
        assert!(
            old_jump >= 6 && old_flat > 1000,
            "the old drawing should show blocks: jumps of {old_jump}, {old_flat} flat"
        );
        // (Neighbours can still be equal: heights are whole numbers, and a
        // slow swell changes by less than one per pixel.)
        for (name, wf) in [("blended 200/s", &base), ("detail", &detail)] {
            let (jump, flat) = steps(wf, span, 1200, column_point);
            assert!(
                jump * 3 <= old_jump,
                "{name}: jumps of {jump} (was {old_jump}), {flat} flat (was {old_flat})"
            );
        }
        // Zoomed out (32 beats on 1200 pixels) nothing changes: the loudest
        // point per column, as before.
        for c in 0..1200 {
            let a = c as f64 * 16.0 / 1200.0;
            let b = a + 16.0 / 1200.0;
            assert_eq!(column_point(&base, a, b), base.span_seconds(a, b));
        }
    }

    /// Zoomed out, strong bar lines thin out to every 2nd, 4th, ... bar,
    /// at least 96 points apart; zoomed in every bar is strong.
    #[test]
    fn bar_lines_thin_out_when_zoomed_out() {
        // 1900 points wide: 8 beats (default), 32, 128 beats.
        for (beats, want) in [(8.0f32, 1), (32.0, 1), (64.0, 1), (128.0, 2)] {
            let px_per_bar = 1900.0 / beats * 4.0;
            let step = major_bar_step(px_per_bar);
            assert_eq!(step, want, "{beats} beats");
            assert!(px_per_bar * step as f32 >= 96.0);
        }
        assert_eq!(major_bar_step(1.0), 16, "at most every 16th bar");
    }

    /// Reported positions arrive in uneven steps (3 ms device periods
    /// against 60 Hz frames); the drawn position must advance evenly.
    #[test]
    fn scroll_clock_smooths_uneven_reports() {
        let rate = 44_100.0;
        let dt = 1.0 / 60.0;
        let mut clock = ScrollClock::default();
        let mut true_pos = 100_000.0;
        let mut reported = true_pos;
        let mut shown = Vec::new();
        for i in 0..600 {
            true_pos += rate * dt;
            // Engine has rendered whole 3 ms periods, alternating 5 and 6.
            let periods = if i % 2 == 0 { 5.0 } else { 6.0 };
            reported += periods * 0.003 * rate;
            // Keep the report near the truth like a real engine would.
            reported += (true_pos - reported) * 0.5;
            shown.push(clock.update(reported, 1.0, rate, dt));
        }
        let steps: Vec<f64> = shown.windows(2).skip(120).map(|w| w[1] - w[0]).collect();
        let ideal = rate * dt;
        let worst = steps.iter().map(|s| (s - ideal).abs()).fold(0.0, f64::max);
        let raw_jitter = 0.003 * rate / 2.0;
        assert!(
            worst < raw_jitter / 4.0,
            "worst step error {worst:.1} frames"
        );
        let last = *shown.last().unwrap_or(&0.0);
        assert!(
            (last - true_pos).abs() < 0.01 * rate,
            "drifted {:.0} frames",
            last - true_pos
        );
    }

    #[test]
    fn scroll_clock_snaps_on_jumps_and_starts_at_the_report() {
        let rate = 48_000.0;
        let mut clock = ScrollClock::default();
        assert_eq!(clock.update(5_000.0, 1.0, rate, 0.0), 5_000.0);
        clock.update(5_800.0, 1.0, rate, 1.0 / 60.0);
        // Cue jump back to 1 000: shown at once, not glided.
        assert_eq!(clock.update(1_000.0, 1.0, rate, 1.0 / 60.0), 1_000.0);
        clock.reset();
        assert_eq!(clock.update(42.0, 1.0, rate, 0.5), 42.0);
    }
}
