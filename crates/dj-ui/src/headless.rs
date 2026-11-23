//! Headless rendering: runs UI frames through egui and its tessellator
//! without a window or GPU. Used by `--smoke-test` and the UI tests.
//!
//! [`run_frames_scaled`] also takes a HiDPI scale factor, and
//! [`screenshot`] rasterises the last frame on the CPU into an RGBA image,
//! so layouts can be checked (and looked at) without a window or GPU.

use crate::app::KaldorApp;
use egui::epaint::{ClippedPrimitive, Primitive};
use egui::{Color32, TextureId};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// What a headless run produced.
#[derive(Clone, Debug, Default)]
pub struct HeadlessReport {
    pub frames: usize,
    /// Tessellated primitives in the last frame.
    pub primitives: usize,
    /// Vertices in the last frame.
    pub vertices: usize,
    /// Slowest frame (UI code plus tessellation).
    pub slowest_frame: Duration,
    pub total: Duration,
    /// UI time (UI code plus tessellation) of every frame, in order.
    pub frame_times: Vec<Duration>,
}

/// Runs `frames` UI frames at `size` points. `between` is called before each
/// frame so the caller can advance the audio engine (for example with the
/// offline renderer) or inject state.
pub fn run_frames(
    app: &mut KaldorApp,
    frames: usize,
    size: egui::Vec2,
    between: impl FnMut(&mut KaldorApp, usize),
) -> HeadlessReport {
    run_frames_scaled(app, frames, size, 1.0, between)
}

/// Like [`run_frames`] with a scale factor (physical pixels per point), as
/// on a HiDPI screen: a 1920x1080 screen at 150 % is `size` 1280x720 with
/// `pixels_per_point` 1.5.
pub fn run_frames_scaled(
    app: &mut KaldorApp,
    frames: usize,
    size: egui::Vec2,
    pixels_per_point: f32,
    between: impl FnMut(&mut KaldorApp, usize),
) -> HeadlessReport {
    run(
        app,
        frames,
        size,
        pixels_per_point,
        None,
        between,
        None,
        |_| Vec::new(),
    )
}

/// Like [`run_frames_scaled`] with input: `events(i)` gives the egui events
/// (pointer, keys) of frame `i`. All frames run in one egui context, so a
/// press in one frame and a release in a later one make a click or a drag.
pub fn run_frames_with_events(
    app: &mut KaldorApp,
    frames: usize,
    size: egui::Vec2,
    pixels_per_point: f32,
    between: impl FnMut(&mut KaldorApp, usize),
    events: impl FnMut(usize) -> Vec<egui::Event>,
) -> HeadlessReport {
    run(
        app,
        frames,
        size,
        pixels_per_point,
        None,
        between,
        None,
        events,
    )
}

/// An RGBA image (8 bits per channel, rows top to bottom).
#[derive(Clone, Debug)]
pub struct Screenshot {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Runs `frames` frames and rasterises the last one on the CPU (no GPU, no
/// window). The image is `size * pixels_per_point` pixels.
pub fn screenshot(
    app: &mut KaldorApp,
    frames: usize,
    size: egui::Vec2,
    pixels_per_point: f32,
    between: impl FnMut(&mut KaldorApp, usize),
) -> Screenshot {
    let mut shot = None;
    run(
        app,
        frames.max(1),
        size,
        pixels_per_point,
        Some(&mut shot),
        between,
        None,
        |_| Vec::new(),
    );
    shot.unwrap_or(Screenshot {
        width: 0,
        height: 0,
        rgba: Vec::new(),
    })
}

#[allow(clippy::too_many_arguments)]
fn run(
    app: &mut KaldorApp,
    frames: usize,
    size: egui::Vec2,
    pixels_per_point: f32,
    mut capture: Option<&mut Option<Screenshot>>,
    mut between: impl FnMut(&mut KaldorApp, usize),
    session: Option<&mut Session>,
    mut events: impl FnMut(usize) -> Vec<egui::Event>,
) -> HeadlessReport {
    // A session carries the egui context (open menus, focus) and the clock
    // over from one run to the next; without one every run starts fresh.
    let (ctx, first, modifiers) = match &session {
        Some(s) => (s.ctx.clone(), s.frame, Some(s.modifiers)),
        None => (egui::Context::default(), 0, None),
    };
    let mut report = HeadlessReport::default();
    let mut textures: HashMap<TextureId, Texture> = HashMap::new();
    let start = Instant::now();
    for i in 0..frames {
        between(app, i);
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some((first + i as u64) as f64 / 60.0),
            // In a session the held modifiers are re-sent every frame.
            events: modifiers
                .map(egui::Event::ModifiersChanged)
                .into_iter()
                .chain(events(i))
                .collect(),
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(pixels_per_point);
        let t0 = Instant::now();
        let mut output = ctx.run_ui(input, |ui| app.ui(ui));
        let ppp = output.pixels_per_point;
        if capture.is_some() {
            for (id, deltas) in &output.textures_delta.set {
                for d in deltas {
                    apply_delta(&mut textures, *id, d);
                }
            }
        }
        // There is no GPU to upload font textures to in headless mode.
        output.textures_delta.clear();
        let prims = ctx.tessellate(output.shapes, ppp);
        let took = t0.elapsed();
        report.slowest_frame = report.slowest_frame.max(took);
        report.frame_times.push(took);
        report.primitives = prims.len();
        report.vertices = prims
            .iter()
            .map(|p| match &p.primitive {
                Primitive::Mesh(m) => m.vertices.len(),
                Primitive::Callback(_) => 0,
            })
            .sum();
        report.frames += 1;
        if i + 1 == frames
            && let Some(slot) = capture.as_deref_mut()
        {
            *slot = Some(rasterize(&prims, &textures, size, ppp));
        }
    }
    if let Some(s) = session {
        s.frame += frames as u64;
    }
    report.total = start.elapsed();
    report
}

/// An egui context kept across headless runs (see [`Demo::keep_context`]).
struct Session {
    ctx: egui::Context,
    frame: u64,
    /// Modifier keys held during the runs (Ctrl, Shift, ...).
    modifiers: egui::Modifiers,
}

/// A texture as the CPU rasteriser keeps it.
struct Texture {
    size: [usize; 2],
    pixels: Vec<Color32>,
}

fn apply_delta(
    textures: &mut HashMap<TextureId, Texture>,
    id: TextureId,
    d: &egui::epaint::ImageDelta,
) {
    let egui::ImageData::Color(img) = &d.image;
    match d.pos {
        None => {
            textures.insert(
                id,
                Texture {
                    size: img.size,
                    pixels: img.pixels.clone(),
                },
            );
        }
        Some([x0, y0]) => {
            let Some(t) = textures.get_mut(&id) else {
                return;
            };
            for y in 0..img.size[1] {
                for x in 0..img.size[0] {
                    let (tx, ty) = (x0 + x, y0 + y);
                    if tx < t.size[0] && ty < t.size[1] {
                        t.pixels[ty * t.size[0] + tx] = img.pixels[y * img.size[0] + x];
                    }
                }
            }
        }
    }
}

fn sample(t: &Texture, u: f32, v: f32) -> [f32; 4] {
    // Bilinear, clamped to the edge.
    let (w, h) = (t.size[0], t.size[1]);
    if w == 0 || h == 0 {
        return [1.0; 4];
    }
    let x = (u * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
    let y = (v * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |x: usize, y: usize| {
        let c = t.pixels[y * w + x];
        [c.r(), c.g(), c.b(), c.a()].map(|v| f32::from(v) / 255.0)
    };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    std::array::from_fn(|k| {
        let top = a[k] + (b[k] - a[k]) * fx;
        let bottom = c[k] + (d[k] - c[k]) * fx;
        top + (bottom - top) * fy
    })
}

/// Draws egui's triangles with premultiplied-alpha blending in gamma space
/// (close to what the wgpu renderer shows; good enough to judge a layout).
fn rasterize(
    prims: &[ClippedPrimitive],
    textures: &HashMap<TextureId, Texture>,
    size: egui::Vec2,
    ppp: f32,
) -> Screenshot {
    let width = (size.x * ppp).round().max(1.0) as usize;
    let height = (size.y * ppp).round().max(1.0) as usize;
    let mut buf = vec![[0.0f32, 0.0, 0.0, 1.0]; width * height];
    for p in prims {
        let Primitive::Mesh(mesh) = &p.primitive else {
            continue;
        };
        let tex = textures.get(&mesh.texture_id);
        let clip = p.clip_rect;
        let cx0 = (clip.min.x * ppp).floor().max(0.0) as usize;
        let cy0 = (clip.min.y * ppp).floor().max(0.0) as usize;
        let cx1 = ((clip.max.x * ppp).ceil().max(0.0) as usize).min(width);
        let cy1 = ((clip.max.y * ppp).ceil().max(0.0) as usize).min(height);
        for tri in mesh.indices.as_chunks::<3>().0 {
            let v = [tri[0], tri[1], tri[2]].map(|i| mesh.vertices[i as usize]);
            let pts = v.map(|v| (v.pos.x * ppp, v.pos.y * ppp));
            let area = (pts[1].0 - pts[0].0) * (pts[2].1 - pts[0].1)
                - (pts[2].0 - pts[0].0) * (pts[1].1 - pts[0].1);
            if area.abs() < 1e-6 {
                continue;
            }
            let minx = pts.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
            let maxx = pts.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
            let miny = pts.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
            let maxy = pts.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
            let x0 = (minx.floor().max(0.0) as usize).max(cx0);
            let x1 = (maxx.ceil().max(0.0) as usize).min(cx1);
            let y0 = (miny.floor().max(0.0) as usize).max(cy0);
            let y1 = (maxy.ceil().max(0.0) as usize).min(cy1);
            let cols = v.map(|v| {
                let c = v.color;
                [c.r(), c.g(), c.b(), c.a()].map(|x| f32::from(x) / 255.0)
            });
            for y in y0..y1 {
                for x in x0..x1 {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((pts[1].0 - px) * (pts[2].1 - py)
                        - (pts[2].0 - px) * (pts[1].1 - py))
                        / area;
                    let w1 = ((pts[2].0 - px) * (pts[0].1 - py)
                        - (pts[0].0 - px) * (pts[2].1 - py))
                        / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let mut src: [f32; 4] = std::array::from_fn(|k| {
                        cols[0][k] * w0 + cols[1][k] * w1 + cols[2][k] * w2
                    });
                    if let Some(t) = tex {
                        let u = v[0].uv.x * w0 + v[1].uv.x * w1 + v[2].uv.x * w2;
                        let vv = v[0].uv.y * w0 + v[1].uv.y * w1 + v[2].uv.y * w2;
                        let s = sample(t, u, vv);
                        for k in 0..4 {
                            src[k] *= s[k];
                        }
                    }
                    let dst = &mut buf[y * width + x];
                    for k in 0..3 {
                        dst[k] = src[k] + dst[k] * (1.0 - src[3]);
                    }
                }
            }
        }
    }
    let rgba = buf
        .iter()
        .flat_map(|c| {
            [
                (c[0].clamp(0.0, 1.0) * 255.0).round() as u8,
                (c[1].clamp(0.0, 1.0) * 255.0).round() as u8,
                (c[2].clamp(0.0, 1.0) * 255.0).round() as u8,
                255,
            ]
        })
        .collect();
    Screenshot {
        width,
        height,
        rgba,
    }
}

/// A demo session for headless runs, layout tests and screenshots: four
/// generated, analysed tracks playing on the four decks (with beatgrids, one
/// key-locked and pitched), and an in-memory library with those tracks in a
/// crate. Nothing touches a device or the disk.
pub struct Demo {
    pub app: KaldorApp,
    pub renderer: dj_audio::offline::OfflineRenderer,
    session: Option<Session>,
}

impl Demo {
    pub const SAMPLE_RATE: u32 = 48_000;
    pub const BLOCK: usize = 256;

    /// Builds the demo with `config` (layout, theme, ...).
    pub fn new(config: dj_core::AppConfig) -> Self {
        use dj_audio::{Command, DeckCommand, Engine, EngineConfig, OutputInfo};
        use dj_core::DeckId;
        let (engine, handle) = Engine::new(EngineConfig {
            sample_rate: Self::SAMPLE_RATE,
            max_block_frames: 512,
            routing: dj_core::OutputRouting::ns4fx(),
        });
        let renderer = dj_audio::offline::OfflineRenderer::new(engine, 4, Self::BLOCK);
        let info = OutputInfo {
            host: "offline".into(),
            device: "demo renderer".into(),
            sample_rate: Self::SAMPLE_RATE,
            channels: 4,
            buffer_frames: Some(Self::BLOCK as u32),
            sample_format: "f32".into(),
        };
        let mut app = KaldorApp::new(handle, info, None, config);
        let mut lib = dj_library::Library::open_in_memory().ok();
        let crate_id = lib
            .as_mut()
            .and_then(|l| l.create_crate("Demo crate", None).ok());
        let names = [
            ("Northline", "Glass Harbour"),
            ("Kite Theory", "Slow Orbit"),
            ("Mara Venn", "Concrete Bloom"),
            ("Oddsfield", "Night Bus 44"),
        ];
        for (i, d) in DeckId::ALL.into_iter().enumerate() {
            let bpm = [124.0, 128.0, 140.0, 174.0][i];
            let click = dj_analysis::synth::ClickTrack {
                sample_rate: Self::SAMPLE_RATE,
                noise: 0.05,
                seed: i as u64 + 1,
                ..dj_analysis::synth::ClickTrack::new(bpm, 30.0)
            };
            let audio = click.to_track();
            let mut analysis =
                dj_analysis::analyze_audio(&audio, &dj_analysis::AnalysisOptions::default());
            // Click tracks have no key; give each one for the key display.
            analysis.key = Some(dj_core::MusicalKey::new([9, 0, 4, 7][i], i % 2 == 0));
            let (artist, title) = names[i];
            if let Some(l) = lib.as_mut()
                && let Ok((id, _)) = l.upsert_track(&dj_library::TrackMeta {
                    path: format!("demo://deck{}.wav", i + 1).into(),
                    artist: artist.into(),
                    title: title.into(),
                    album: "Demo".into(),
                    genre: "Test".into(),
                    duration: 30.0,
                    ..Default::default()
                })
            {
                let _ = l.store_analysis(id, &analysis);
                if let Some(c) = crate_id {
                    let _ = l.add_to_crate(c, &[id]);
                }
            }
            app.load_audio_analyzed(d, &format!("{artist} - {title}"), audio, &analysis);
            app.send(Command::Deck(d, DeckCommand::Play(true)));
        }
        app.send(Command::Deck(DeckId::ALL[1], DeckCommand::SetKeyLock(true)));
        app.send(Command::Deck(DeckId::ALL[1], DeckCommand::SetPitch(0.25)));
        if let Some(l) = lib {
            app = app.with_library(l);
            // The demo tracks have no files: no file checks, all found.
            if let Some(l) = app.library() {
                l.files = crate::file_check::FileChecker::disabled();
                for id in l.view().to_vec() {
                    l.files.set(id, crate::file_check::FileState::Found);
                }
            }
        }
        Self {
            app,
            renderer,
            session: None,
        }
    }

    /// Puts the decks into a performance state for screenshots and tests:
    /// hot cues on every deck, a loop on deck 3, deck 2 synced to deck 1,
    /// slip on deck 4 and three samples in bank A.
    /// FX for screenshots: unit 1 on with echo and reverb, unit 2 in single
    /// mode with a filter on the master.
    pub fn add_fx_state(&mut self) {
        use dj_audio::{Command, EffectKind, FxCommand, FxMode};
        for c in [
            FxCommand::UnitOn(0, true),
            FxCommand::SlotOn(0, 0, true),
            FxCommand::SlotOn(0, 1, true),
            FxCommand::Beats(0, 4),
            FxCommand::Mode(1, FxMode::Single),
            FxCommand::SlotKind(1, 0, EffectKind::Filter),
            FxCommand::SlotOn(1, 0, true),
            FxCommand::Assign(1, dj_audio::MASTER_TARGET, true),
        ] {
            self.app.send(Command::Fx(c));
        }
    }

    pub fn add_performance_state(&mut self) {
        use dj_audio::{Command, DeckCommand, SamplerCommand};
        use dj_core::DeckId;
        let sr = f64::from(Self::SAMPLE_RATE);
        for d in DeckId::ALL {
            for (slot, t) in [(0u8, 1.0), (1, 4.0), (3, 9.5), (6, 16.0)] {
                self.app.send(Command::Deck(
                    d,
                    DeckCommand::SetHotCue {
                        slot,
                        position: Some(t * sr),
                    },
                ));
            }
        }
        self.app.send(Command::Quantize(true));
        self.app.send(Command::Sync(DeckId::ALL[1], true));
        self.app.send(Command::Sync(DeckId::ALL[0], true));
        self.app
            .send(Command::Deck(DeckId::ALL[2], DeckCommand::AutoLoop(2.0)));
        self.app
            .send(Command::Deck(DeckId::ALL[3], DeckCommand::SetSlip(true)));
        for slot in 0..3 {
            let click = dj_analysis::synth::ClickTrack {
                sample_rate: Self::SAMPLE_RATE,
                ..dj_analysis::synth::ClickTrack::new(126.0, 2.0)
            };
            let name = ["Air horn", "Vocal stab", "Riser"][slot];
            self.app
                .load_sample_audio(slot, name, click.to_track(), Some(126.0));
        }
        self.app.send(Command::Sampler(SamplerCommand::SetMode(
            1,
            dj_audio::SampleMode::Loop,
        )));
        self.app.send(Command::Sampler(SamplerCommand::Trigger {
            slot: 1,
            pressed: true,
        }));
    }

    /// Audio blocks per 60 Hz UI frame.
    pub fn blocks_per_frame() -> usize {
        (Self::SAMPLE_RATE as usize / 60).div_ceil(Self::BLOCK)
    }

    /// Holds modifier keys (Ctrl, Shift, ...) in the following runs, as if
    /// they were pressed; `Modifiers::default()` releases them. Keeps the
    /// egui context from now on (see [`Self::keep_context`]).
    pub fn hold_modifiers(&mut self, m: egui::Modifiers) {
        if self.session.is_none() {
            self.keep_context();
        }
        if let Some(s) = self.session.as_mut() {
            s.modifiers = m;
        }
    }

    /// Makes later runs continue in one egui context, as in the real app:
    /// menus stay open and focus stays put from one run to the next.
    pub fn keep_context(&mut self) {
        self.session = Some(Session {
            ctx: egui::Context::default(),
            frame: 0,
            modifiers: egui::Modifiers::default(),
        });
    }

    /// Runs frames with the audio advancing in real time.
    pub fn run(&mut self, frames: usize, size: egui::Vec2, ppp: f32) -> HeadlessReport {
        let r = &mut self.renderer;
        let n = Self::blocks_per_frame();
        run(
            &mut self.app,
            frames,
            size,
            ppp,
            None,
            |_, _| {
                for _ in 0..n {
                    r.render_block();
                }
            },
            self.session.as_mut(),
            |_| Vec::new(),
        )
    }

    /// Runs frames with input events (see [`run_frames_with_events`]).
    pub fn run_events(
        &mut self,
        frames: usize,
        size: egui::Vec2,
        ppp: f32,
        events: impl FnMut(usize) -> Vec<egui::Event>,
    ) -> HeadlessReport {
        let r = &mut self.renderer;
        let n = Self::blocks_per_frame();
        run(
            &mut self.app,
            frames,
            size,
            ppp,
            None,
            |_, _| {
                for _ in 0..n {
                    r.render_block();
                }
            },
            self.session.as_mut(),
            events,
        )
    }

    /// Like [`Self::screenshot`], with input events (a fresh egui context:
    /// the picture needs the font texture from the first frame).
    pub fn screenshot_events(
        &mut self,
        frames: usize,
        size: egui::Vec2,
        ppp: f32,
        events: impl FnMut(usize) -> Vec<egui::Event>,
    ) -> Screenshot {
        let r = &mut self.renderer;
        let n = Self::blocks_per_frame();
        let mut shot = None;
        run(
            &mut self.app,
            frames.max(1),
            size,
            ppp,
            Some(&mut shot),
            |_, _| {
                for _ in 0..n {
                    r.render_block();
                }
            },
            None,
            events,
        );
        shot.unwrap_or(Screenshot {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        })
    }

    /// Runs frames and returns a CPU-rasterised picture of the last one.
    pub fn screenshot(&mut self, frames: usize, size: egui::Vec2, ppp: f32) -> Screenshot {
        let r = &mut self.renderer;
        let n = Self::blocks_per_frame();
        screenshot(&mut self.app, frames, size, ppp, |_, _| {
            for _ in 0..n {
                r.render_block();
            }
        })
    }
}
