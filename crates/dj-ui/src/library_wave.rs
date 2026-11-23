//! The small waveforms in the track list (the Waveform column, as in
//! rekordbox's track list): the whole track with its hot cues in their
//! colours. Built from the waveform the analysis stored in the library,
//! a few tracks per frame (only rows on screen), and kept in memory.

use crate::theme::rgb;
use crate::waveform_view::{point_color, push_bar};
use dj_analysis::waveform::{WavePoint, Waveform};
use dj_core::config::WaveformColorMode;
use dj_library::{Cue, CueKind, Library, TrackId};
use egui::{Color32, Mesh, Rect, Shape, Stroke, pos2};
use std::collections::{HashMap, VecDeque};

/// Points across a small waveform (it is drawn up to this many pixels wide
/// without repeating).
pub const POINTS: usize = 400;
/// Small waveforms kept in memory (the oldest go first).
const KEEP: usize = 3000;
/// Small waveforms read from the library per frame (each is one database
/// read), so scrolling stays smooth.
pub const LOADS_PER_FRAME: usize = 6;

/// One track's small waveform.
#[derive(Clone, Debug, PartialEq)]
pub struct MiniWave {
    pub points: Vec<WavePoint>,
    /// Hot cues: where (0 to 1 across the track) and their colour.
    pub hot_cues: Vec<(f32, Color32)>,
}

impl MiniWave {
    /// From the stored waveform and the track's cues (seconds).
    pub fn build(wf: &Waveform, cues: &[Cue]) -> Self {
        let sr = f64::from(wf.sample_rate.max(1));
        let len = wf.levels.first().map_or(0, Vec::len) as f64 * wf.frames_per_point as f64 / sr;
        let points = if len > 0.0 {
            (0..POINTS)
                .map(|i| {
                    let a = len * i as f64 / POINTS as f64;
                    let b = len * (i + 1) as f64 / POINTS as f64;
                    wf.span_seconds(a, b).unwrap_or_default()
                })
                .collect()
        } else {
            Vec::new()
        };
        let hot_cues = cues
            .iter()
            .filter_map(|c| match c.kind {
                CueKind::Hot(slot) if len > 0.0 => {
                    let color = c
                        .color
                        .unwrap_or(crate::perform::PAD_COLORS[usize::from(slot) % 8]);
                    Some(((c.position / len).clamp(0.0, 1.0) as f32, rgb(color)))
                }
                _ => None,
            })
            .collect();
        Self { points, hot_cues }
    }
}

/// The small waveforms read so far. `None` in the map: the track has no
/// stored waveform (not analysed yet).
#[derive(Default)]
pub struct MiniWaves {
    map: HashMap<TrackId, Option<MiniWave>>,
    order: VecDeque<TrackId>,
}

impl MiniWaves {
    /// The small waveform of a track: `None` when it was not read yet,
    /// `Some(None)` when the track has no waveform.
    pub fn get(&self, id: TrackId) -> Option<Option<&MiniWave>> {
        self.map.get(&id).map(Option::as_ref)
    }

    /// Reads it again next time (new analysis, cues changed).
    pub fn forget(&mut self, id: TrackId) {
        self.map.remove(&id);
    }

    /// Reads up to [`LOADS_PER_FRAME`] of `ids` that are not read yet.
    /// Returns true when some are still waiting.
    pub fn load(&mut self, lib: &Library, ids: &[TrackId]) -> bool {
        let mut loaded = 0;
        for id in ids {
            if self.map.contains_key(id) {
                continue;
            }
            if loaded == LOADS_PER_FRAME {
                return true;
            }
            loaded += 1;
            let mini = lib.waveform(*id).ok().flatten().map(|wf| {
                let cues = lib.cues(*id).unwrap_or_default();
                MiniWave::build(&wf, &cues)
            });
            self.map.insert(*id, mini);
            self.order.push_back(*id);
            while self.order.len() > KEEP {
                if let Some(old) = self.order.pop_front() {
                    self.map.remove(&old);
                }
            }
        }
        false
    }
}

/// Draws a small waveform in `rect` (a cell of the track list): the bars,
/// then the hot cues as coloured lines with a small flag at the top. A
/// track that was not analysed gets a thin line; one not read yet nothing.
pub fn paint(
    painter: &egui::Painter,
    rect: Rect,
    mini: Option<Option<&MiniWave>>,
    mode: WaveformColorMode,
    weak: Color32,
) {
    let mid = rect.center().y;
    let Some(m) = mini.flatten().filter(|m| !m.points.is_empty()) else {
        if mini.is_some() {
            // Read, but not analysed yet.
            painter.line_segment(
                [pos2(rect.left(), mid), pos2(rect.right(), mid)],
                Stroke::new(1.0, weak.gamma_multiply(0.5)),
            );
        }
        return;
    };
    let w = rect.width().max(1.0);
    let half = (rect.height() / 2.0 - 1.0).max(1.0);
    let mut mesh = Mesh::default();
    let n = m.points.len();
    for c in 0..w.ceil() as usize {
        let i = (c * n / w.ceil() as usize).min(n - 1);
        let pt = m.points[i];
        let h = (f32::from(pt.peak) / 255.0 * half).max(0.5);
        let x0 = rect.left() + c as f32;
        push_bar(
            &mut mesh,
            x0,
            x0 + 1.0,
            mid - h,
            mid + h,
            point_color(pt, mode),
        );
    }
    painter.add(Shape::mesh(mesh));
    let outline = Stroke::new(1.0, Color32::from_black_alpha(180));
    for (f, color) in &m.hot_cues {
        let x = rect.left() + f * w;
        painter.line_segment(
            [pos2(x, rect.top()), pos2(x, rect.bottom())],
            Stroke::new(1.5, *color),
        );
        let s = 3.5;
        painter.add(Shape::convex_polygon(
            vec![
                pos2(x - s, rect.top()),
                pos2(x + s, rect.top()),
                pos2(x, rect.top() + s * 1.3),
            ],
            *color,
            outline,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(points: usize) -> Waveform {
        Waveform {
            points_per_second: 100.0,
            frames_per_point: 480,
            sample_rate: 48_000,
            levels: vec![
                (0..points)
                    .map(|i| WavePoint {
                        peak: (i % 256) as u8,
                        low: 10,
                        mid: 20,
                        high: 30,
                    })
                    .collect(),
            ],
        }
    }

    #[test]
    fn hot_cues_land_where_they_are_in_the_track_with_their_colour() {
        // 1000 points of 10 ms: a 10 s track.
        let wf = wave(1000);
        let cues = vec![
            Cue {
                kind: CueKind::Hot(0),
                position: 2.5,
                name: String::new(),
                color: Some(0x00FF00),
            },
            Cue {
                kind: CueKind::Hot(3),
                position: 7.5,
                name: String::new(),
                color: None,
            },
            Cue {
                kind: CueKind::Main,
                position: 1.0,
                name: String::new(),
                color: None,
            },
        ];
        let m = MiniWave::build(&wf, &cues);
        assert_eq!(m.points.len(), POINTS);
        assert_eq!(m.hot_cues.len(), 2, "hot cues only");
        assert!((m.hot_cues[0].0 - 0.25).abs() < 1e-6);
        assert_eq!(m.hot_cues[0].1, Color32::from_rgb(0, 255, 0));
        assert!((m.hot_cues[1].0 - 0.75).abs() < 1e-6);
        assert_eq!(m.hot_cues[1].1, rgb(crate::perform::PAD_COLORS[3]));
    }

    #[test]
    fn a_few_tracks_are_read_per_frame_and_kept() {
        let mut lib = Library::open_in_memory().unwrap();
        let mut ids = Vec::new();
        for i in 0..10 {
            let (id, _) = lib
                .upsert_track(&dj_library::TrackMeta {
                    path: format!("/m/{i}.wav").into(),
                    ..Default::default()
                })
                .unwrap();
            ids.push(id);
        }
        let mut w = MiniWaves::default();
        assert!(w.load(&lib, &ids), "more waiting after the first frame");
        assert_eq!(
            ids.iter().filter(|i| w.get(**i).is_some()).count(),
            LOADS_PER_FRAME
        );
        assert!(!w.load(&lib, &ids));
        // Not analysed: read, without a waveform.
        assert_eq!(w.get(ids[9]), Some(None));
        w.forget(ids[0]);
        assert_eq!(w.get(ids[0]), None);
    }
}
