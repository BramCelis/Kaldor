//! The SAMPLER tab: 32 slots in 4 banks of 8, recording from the master,
//! and sample sets saved to and loaded from disk.
//!
//! A sample set is a TOML file listing each slot's file, mode, volume, sync
//! and tempo. Samples recorded from the master have no file yet; saving the
//! set writes them as WAV files next to it.

use crate::app::{KaldorApp, region_ui};
use crate::loader::{LoadJob, LoaderEvent};
use crate::theme::{self, Palette};
use crate::widgets::{self, flat_button, is_held};
use dj_audio::{
    Command, RecordBuffer, SAMPLER_BANK, SAMPLER_SLOTS, SampleMode, SamplerCommand, TrackAudio,
};
use egui::{Align, Color32, Id, Layout, Rect, Sense, Stroke, pos2, vec2};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Bank names.
pub const BANKS: [&str; 4] = ["A", "B", "C", "D"];

/// One sampler slot on the UI side.
#[derive(Default)]
pub struct SlotUi {
    pub name: String,
    /// File the sample came from (`None` for a recording not saved yet).
    pub path: Option<PathBuf>,
    pub bpm: Option<f64>,
    pub job: Option<LoadJob>,
    /// The audio, kept to write recordings into a sample set.
    pub audio: Option<Arc<TrackAudio>>,
    pub held: bool,
}

/// Sampler state on the UI side.
pub struct SamplerUi {
    /// Bank shown on the pads and in the tab (0-3).
    pub bank: usize,
    pub slots: Vec<SlotUi>,
    /// Sample set file.
    pub set_path: String,
    pub status: String,
    /// Slot tiles drawn in the last frame (for files dropped from the OS).
    pub drop_rects: Vec<(usize, Rect)>,
}

impl SamplerUi {
    pub fn new(set_path: Option<PathBuf>) -> Self {
        Self {
            bank: 0,
            slots: (0..SAMPLER_SLOTS).map(|_| SlotUi::default()).collect(),
            set_path: set_path
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            status: String::new(),
            drop_rects: Vec::new(),
        }
    }
}

/// Slot label: bank letter and number, "A1" to "D8".
pub fn slot_label(slot: usize) -> String {
    format!(
        "{}{}",
        BANKS[(slot / SAMPLER_BANK).min(3)],
        slot % SAMPLER_BANK + 1
    )
}

fn mode_name(m: SampleMode) -> &'static str {
    match m {
        SampleMode::OneShot => "one-shot",
        SampleMode::Hold => "hold",
        SampleMode::Toggle => "toggle",
        SampleMode::Loop => "loop",
    }
}

fn mode_from(s: &str) -> SampleMode {
    match s {
        "hold" => SampleMode::Hold,
        "toggle" => SampleMode::Toggle,
        "loop" => SampleMode::Loop,
        _ => SampleMode::OneShot,
    }
}

/// A sample set file.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SampleSet {
    #[serde(default)]
    pub slot: Vec<SetSlot>,
}

#[derive(Debug, PartialEq, Serialize, Deserialize)]
pub struct SetSlot {
    pub index: usize,
    pub path: PathBuf,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mode: String,
    #[serde(default = "one")]
    pub volume: f32,
    #[serde(default)]
    pub sync: bool,
    #[serde(default)]
    pub bpm: Option<f64>,
}

fn one() -> f32 {
    1.0
}

/// Writes interleaved stereo float samples as a 32-bit float WAV file.
pub fn write_wav(path: &Path, sample_rate: u32, interleaved: &[f32]) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for s in interleaved {
        w.write_sample(*s).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

impl KaldorApp {
    fn sampler_cmd(&mut self, c: SamplerCommand) {
        self.send(Command::Sampler(c));
    }

    /// Loads a file into a sampler slot (decoded in the background).
    /// Loads a library track into a sampler slot (with its BPM, for SYNC).
    pub fn load_library_sample(&mut self, slot: usize, id: dj_library::TrackId) {
        if let Some(t) = self.library.as_ref().and_then(|l| l.track(id)).cloned() {
            self.load_sample(slot, t.meta.path.clone(), t.bpm, t.display_name());
        }
    }

    pub fn load_sample(&mut self, slot: usize, path: PathBuf, bpm: Option<f64>, name: String) {
        if slot >= SAMPLER_SLOTS {
            return;
        }
        let rate = self.output_info.sample_rate;
        match LoadJob::start(path.clone(), rate, false, false) {
            Ok(job) => {
                let s = &mut self.sampler.slots[slot];
                s.name = if name.is_empty() {
                    path.file_stem()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                } else {
                    name
                };
                s.path = Some(path);
                s.bpm = bpm;
                s.job = Some(job);
            }
            Err(e) => self.sampler.status = format!("cannot load the sample: {e}"),
        }
    }

    /// Puts audio that is already in memory into a slot.
    pub fn load_sample_audio(
        &mut self,
        slot: usize,
        name: &str,
        audio: Arc<TrackAudio>,
        bpm: Option<f64>,
    ) {
        if slot >= SAMPLER_SLOTS {
            return;
        }
        let s = &mut self.sampler.slots[slot];
        s.name = name.to_string();
        s.path = None;
        s.bpm = bpm;
        s.job = None;
        s.audio = Some(Arc::clone(&audio));
        self.sampler_cmd(SamplerCommand::Load {
            slot: slot as u8,
            audio,
            bpm,
        });
    }

    pub fn eject_sample(&mut self, slot: usize) {
        if slot < SAMPLER_SLOTS {
            self.sampler.slots[slot] = SlotUi::default();
            self.sampler_cmd(SamplerCommand::Eject(slot as u8));
        }
    }

    /// Starts recording the master into `slot`, or stops the recording in
    /// progress.
    pub fn toggle_record(&mut self, slot: usize) {
        let snap = *self.engine.snapshot();
        if snap.sampler.recording.is_some() {
            self.sampler_cmd(SamplerCommand::StopRecord);
            return;
        }
        let rate = f64::from(snap.sample_rate.max(1));
        let beats = f64::from(self.config.performance.sampler_record_beats.max(1.0));
        let seconds = snap
            .master_bpm
            .map_or(8.0, |bpm| beats * 60.0 / bpm)
            .clamp(0.5, 60.0);
        let mut buf = RecordBuffer::new(slot as u8, (seconds * rate) as usize);
        buf.bpm = snap.master_bpm;
        self.sampler_cmd(SamplerCommand::Record(buf));
        self.sampler.status = format!("recording {} from the master", slot_label(slot));
    }

    /// Sampler work for this frame: decoded samples, finished recordings.
    pub(crate) fn poll_sampler(&mut self) {
        for slot in 0..SAMPLER_SLOTS {
            loop {
                let Some(ev) = self.sampler.slots[slot]
                    .job
                    .as_ref()
                    .and_then(LoadJob::poll)
                else {
                    break;
                };
                match ev {
                    LoaderEvent::Started { audio, .. } => {
                        let bpm = self.sampler.slots[slot].bpm;
                        self.sampler.slots[slot].audio = Some(Arc::clone(&audio));
                        self.sampler_cmd(SamplerCommand::Load {
                            slot: slot as u8,
                            audio,
                            bpm,
                        });
                    }
                    LoaderEvent::Finished { .. }
                    | LoaderEvent::Waveform(_)
                    | LoaderEvent::Detail(_) => {
                        self.sampler.slots[slot].job = None;
                        break;
                    }
                    LoaderEvent::Failed(e) => {
                        self.sampler.status = format!("{}: {e}", slot_label(slot));
                        self.sampler.slots[slot] = SlotUi::default();
                        break;
                    }
                }
            }
        }
        let rate = self.output_info.sample_rate;
        for rec in self.engine.take_recordings() {
            let slot = usize::from(rec.slot);
            if rec.written == 0 {
                continue;
            }
            let audio = TrackAudio::from_interleaved(rate, rec.recorded());
            let name = format!("Recording {}", slot_label(slot));
            self.load_sample_audio(slot, &name, audio, rec.bpm);
            self.sampler.status = format!(
                "recorded {:.1} s into {}",
                rec.written as f64 / f64::from(rate.max(1)),
                slot_label(slot)
            );
        }
    }

    /// After the audio output changed (a new engine): puts the samples and
    /// their settings back. Files are decoded again at the new rate; a
    /// recording is kept when the rate did not change.
    pub(crate) fn restore_sampler(&mut self, snap: &dj_audio::EngineSnapshot, old_rate: u32) {
        self.sampler_cmd(SamplerCommand::Volume(snap.sampler.volume));
        let same_rate = old_rate == self.output_info.sample_rate;
        let mut lost = 0;
        for slot in 0..SAMPLER_SLOTS {
            let s = snap.sampler.slots[slot];
            let id = slot as u8;
            self.sampler_cmd(SamplerCommand::SetMode(id, s.mode));
            self.sampler_cmd(SamplerCommand::SetVolume(id, s.volume));
            self.sampler_cmd(SamplerCommand::SetSync(id, s.sync));
            if !s.loaded {
                continue;
            }
            let ui = &self.sampler.slots[slot];
            match (ui.path.clone(), ui.audio.clone()) {
                (Some(path), _) => {
                    let (bpm, name) = (ui.bpm, ui.name.clone());
                    self.load_sample(slot, path, bpm, name);
                }
                (None, Some(audio)) if same_rate => {
                    let (bpm, name) = (ui.bpm, ui.name.clone());
                    self.load_sample_audio(slot, &name, audio, bpm);
                }
                _ => {
                    self.sampler.slots[slot] = SlotUi::default();
                    lost += 1;
                }
            }
        }
        if lost > 0 {
            self.sampler.status = format!(
                "{lost} recording(s) not saved in a set were dropped: the output's sample rate changed"
            );
        }
    }

    /// Saves the sample set to `path` (recordings become WAV files next to
    /// it).
    pub fn save_sample_set(&mut self, path: &Path) -> Result<usize, String> {
        let snap = self.engine.snapshot().sampler;
        let rate = self.output_info.sample_rate;
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "samples".into());
        let mut set = SampleSet::default();
        for i in 0..SAMPLER_SLOTS {
            let s = &self.sampler.slots[i];
            if !snap.slots[i].loaded {
                continue;
            }
            let file = match (&s.path, &s.audio) {
                (Some(p), _) => p.clone(),
                (None, Some(a)) => {
                    let wav = path.with_file_name(format!("{stem}-{}.wav", slot_label(i)));
                    let mut buf = vec![0.0f32; a.len_frames() as usize * 2];
                    a.read_interleaved(0, &mut buf);
                    write_wav(&wav, rate, &buf)?;
                    wav
                }
                (None, None) => continue,
            };
            set.slot.push(SetSlot {
                index: i,
                path: file.clone(),
                name: s.name.clone(),
                mode: mode_name(snap.slots[i].mode).into(),
                volume: snap.slots[i].volume,
                sync: snap.slots[i].sync,
                bpm: s.bpm,
            });
            self.sampler.slots[i].path = Some(file);
        }
        let text = toml::to_string_pretty(&set).map_err(|e| e.to_string())?;
        std::fs::write(path, format!("# Kaldor sample set\n{text}")).map_err(|e| e.to_string())?;
        Ok(set.slot.len())
    }

    /// Loads a sample set: every listed slot is replaced.
    pub fn load_sample_set(&mut self, path: &Path) -> Result<usize, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let set: SampleSet = toml::from_str(&text).map_err(|e| e.to_string())?;
        let mut n = 0;
        for s in set.slot {
            if s.index >= SAMPLER_SLOTS {
                continue;
            }
            let slot = s.index as u8;
            self.sampler_cmd(SamplerCommand::SetMode(slot, mode_from(&s.mode)));
            self.sampler_cmd(SamplerCommand::SetVolume(slot, s.volume.clamp(0.0, 1.0)));
            self.sampler_cmd(SamplerCommand::SetSync(slot, s.sync));
            // A set folder that was moved (or renamed, as when the app
            // folder became Kaldor): look for the file next to the set.
            let file = match (s.path.exists(), s.path.file_name()) {
                (false, Some(name)) if path.with_file_name(name).exists() => {
                    path.with_file_name(name)
                }
                _ => s.path,
            };
            self.load_sample(s.index, file, s.bpm, s.name);
            n += 1;
        }
        Ok(n)
    }

    pub fn sampler_bank(&self) -> usize {
        self.sampler.bank
    }

    pub fn set_sampler_bank(&mut self, bank: usize) {
        self.sampler.bank = bank.min(BANKS.len() - 1);
    }

    pub fn sample_name(&self, slot: usize) -> &str {
        self.sampler.slots.get(slot).map_or("", |s| s.name.as_str())
    }
}

/// The SAMPLER tab content.
pub(crate) fn sampler_panel(app: &mut KaldorApp, ui: &mut egui::Ui, rect: Rect) {
    let pal = Palette::get(app.config.ui.dark_theme);
    app.sampler.drop_rects.clear();
    let snap = *app.engine.snapshot();
    let bar = Rect::from_min_size(rect.min, vec2(rect.width(), 24.0));
    let tiles = Rect::from_min_max(pos2(rect.left(), bar.bottom() + 6.0), rect.max);

    // Bar: banks, volume, set file, save, load, status.
    let mut x = bar.left();
    for (b, name) in BANKS.iter().enumerate() {
        let r = Rect::from_min_size(pos2(x, bar.top()), vec2(34.0, bar.height()));
        let resp = flat_button(
            ui,
            r,
            Id::unique(("sampler_bank", b)),
            name,
            theme::prop(12.5),
            None,
            app.sampler.bank == b,
            pal.accent,
            false,
        )
        .on_hover_text(format!(
            "Bank {name} (slots {name}1 to {name}8) on the pads"
        ));
        if resp.clicked() {
            app.sampler.bank = b;
        }
        app.map.add(ui, "sampler_bank", None, r);
        x += 38.0;
    }
    x += 10.0;
    let vol_r = Rect::from_min_size(pos2(x, bar.top() + 4.0), vec2(90.0, bar.height() - 8.0));
    let (vresp, new) = widgets::hfader(
        ui,
        vol_r,
        Id::unique("sampler_volume"),
        snap.sampler.volume / 1.5,
        pal.accent,
    );
    vresp.on_hover_text(format!(
        "Sampler volume {:.0}%",
        snap.sampler.volume * 100.0
    ));
    if let Some(t) = new {
        app.send(Command::Sampler(SamplerCommand::Volume(t * 1.5)));
    }
    app.map.add(ui, "sampler_volume", None, vol_r);
    x = vol_r.right() + 12.0;
    let mut row = region_ui(
        ui,
        Rect::from_min_max(pos2(x, bar.top()), bar.max),
        Layout::left_to_right(Align::Center),
        "sampler_bar",
    );
    // The file field gives way first on narrow windows.
    let field_w = (row.available_width() - 250.0).clamp(60.0, 220.0);
    let f = row.add(
        egui::TextEdit::singleline(&mut app.sampler.set_path)
            .hint_text("sample set file (.toml)")
            .desired_width(field_w),
    );
    let mut marks = vec![("sampler_set_path", f.rect)];
    let save = row
        .button("Save set")
        .on_hover_text("Saves the slots to the file; recordings become WAV files next to it");
    marks.push(("sampler_save", save.rect));
    let load = row
        .button("Load set")
        .on_hover_text("Loads the slots listed in the file");
    marks.push(("sampler_load", load.rect));
    let stop = row.button("Stop all").on_hover_text("Stops every sample");
    marks.push(("sampler_stop_all", stop.rect));
    if !app.sampler.status.is_empty() {
        row.label(egui::RichText::new(&app.sampler.status).color(pal.text_weak));
    }
    for (n, r) in marks {
        app.map.add(ui, n, None, r);
    }
    let path = PathBuf::from(app.sampler.set_path.trim());
    if save.clicked() && !path.as_os_str().is_empty() {
        app.sampler.status = match app.save_sample_set(&path) {
            Ok(n) => format!("saved {n} samples to {}", path.display()),
            Err(e) => format!("cannot save: {e}"),
        };
    }
    if load.clicked() && !path.as_os_str().is_empty() {
        app.sampler.status = match app.load_sample_set(&path) {
            Ok(n) => format!("loading {n} samples"),
            Err(e) => format!("cannot load: {e}"),
        };
    }
    if stop.clicked() {
        app.send(Command::Sampler(SamplerCommand::StopAll));
    }

    // Tiles: one row of 8, or two rows of 4 when narrow.
    if tiles.height() < 24.0 {
        return;
    }
    // One row of 8 while the tiles stay usable; it keeps them tall.
    let per_row = if (tiles.width() - 7.0 * 6.0) / 8.0 >= 88.0 {
        8
    } else {
        4
    };
    let rows = SAMPLER_BANK / per_row;
    let gap = 6.0;
    let tw = (tiles.width() - gap * (per_row as f32 - 1.0)) / per_row as f32;
    let th = ((tiles.height() - gap * (rows as f32 - 1.0)) / rows as f32).min(120.0);
    for k in 0..SAMPLER_BANK {
        let slot = app.sampler.bank * SAMPLER_BANK + k;
        let (c, r) = (k % per_row, k / per_row);
        let tr = Rect::from_min_size(
            pos2(
                tiles.left() + c as f32 * (tw + gap),
                tiles.top() + r as f32 * (th + gap),
            ),
            vec2(tw, th),
        );
        slot_tile(app, ui, slot, tr, &snap);
    }
}

/// One slot: name, pad (hold to play), mode, sync, record, eject, volume.
fn slot_tile(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    slot: usize,
    rect: Rect,
    snap: &dj_audio::EngineSnapshot,
) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let s = snap.sampler.slots[slot];
    let recording = snap.sampler.recording.filter(|r| usize::from(r.0) == slot);
    app.sampler.drop_rects.push((slot, rect));
    ui.painter().rect(
        rect,
        5.0,
        pal.raised,
        Stroke::new(1.0, pal.border),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(4.0);
    let tag = Some(slot);
    // Library tracks dropped on the tile load into the slot (the outline is
    // drawn by `paint_drop_highlight`).
    let dz = ui.interact(rect, Id::unique(("sample_drop", slot)), Sense::hover());
    if let Some(p) = dz.dnd_release_payload::<crate::library_view::TrackDrag>() {
        app.load_library_sample(slot, p.0);
    }

    // Parts by height: the pad always, then the controls row, the name and
    // the volume. Everything is also in the pad's right-click menu.
    let row_h = 20.0;
    let h = inner.height();
    let show_ctrl = h >= row_h + 3.0 + 16.0;
    let show_name = h >= 16.0 + 3.0 + 16.0 + 3.0 + row_h;
    let show_vol = h >= 16.0 + 3.0 + 16.0 + 4.0 + 8.0 + 3.0 + row_h;
    let (mut top, mut bottom) = (inner.top(), inner.bottom());
    let ctrl_r = show_ctrl.then(|| {
        let r = Rect::from_min_max(
            pos2(inner.left(), bottom - row_h),
            pos2(inner.right(), bottom),
        );
        bottom = r.top() - 3.0;
        r
    });
    let vol_r = show_vol.then(|| {
        let r = Rect::from_min_max(
            pos2(inner.left(), bottom - 8.0),
            pos2(inner.right(), bottom),
        );
        bottom = r.top() - 4.0;
        r
    });
    let name = app.sampler.slots[slot].name.clone();
    let label = if s.loaded || app.sampler.slots[slot].job.is_some() {
        format!("{}  {}", slot_label(slot), name)
    } else {
        format!("{}  (drop a track)", slot_label(slot))
    };
    if show_name {
        let name_r = Rect::from_min_size(inner.min, vec2(inner.width(), 16.0));
        top = name_r.bottom() + 3.0;
        widgets::text_in(
            &ui.painter_at(name_r),
            name_r,
            &label,
            theme::prop(11.5),
            if s.loaded { pal.text } else { pal.text_weak },
            false,
        );
        app.map.add(ui, "sample_name", tag, name_r);
    }
    let pad_r = Rect::from_min_max(
        pos2(inner.left(), top),
        pos2(inner.right(), bottom.max(top + 4.0)),
    );

    let color = Color32::from_rgb(0x4A, 0x7C, 0xF0);
    let pad_label = if recording.is_some() {
        "REC".to_string()
    } else if s.playing {
        "PLAYING".to_string()
    } else if show_name {
        String::new()
    } else {
        label.clone()
    };
    let pad = flat_button(
        ui,
        pad_r,
        Id::unique(("sample_pad", slot)),
        &pad_label,
        theme::prop(if show_name { 13.0 } else { 11.0 }),
        None,
        s.playing,
        if recording.is_some() {
            pal.error
        } else {
            color
        },
        true,
    )
    .on_hover_text(format!(
        "{label}: hold to play ({}, {:.1} s). Right-click: stop, mode, sync, record, empty",
        s.mode.label(),
        s.length_seconds
    ));
    if s.loaded && !s.playing && recording.is_none() {
        // Loaded slots keep a thin bar in the pad colour.
        ui.painter().rect_filled(
            Rect::from_min_size(pad_r.min + vec2(3.0, 3.0), vec2(pad_r.width() - 6.0, 3.0)),
            1.0,
            color.gamma_multiply(0.7),
        );
    }
    if s.playing && s.progress > 0.0 {
        let w = pad_r.width() * s.progress.clamp(0.0, 1.0);
        ui.painter().rect_filled(
            Rect::from_min_size(pos2(pad_r.left(), pad_r.bottom() - 3.0), vec2(w, 3.0)),
            1.0,
            Palette::on(color),
        );
    }
    if let Some((_, f)) = recording {
        let w = pad_r.width() * f.clamp(0.0, 1.0);
        ui.painter().rect_filled(
            Rect::from_min_size(pos2(pad_r.left(), pad_r.bottom() - 3.0), vec2(w, 3.0)),
            1.0,
            pal.error,
        );
    }
    let held = is_held(&pad);
    if held != app.sampler.slots[slot].held {
        app.sampler.slots[slot].held = held;
        app.send(Command::Sampler(SamplerCommand::Trigger {
            slot: slot as u8,
            pressed: held,
        }));
    }
    pad.context_menu(|ui| {
        if ui.button("Stop").clicked() {
            app.send(Command::Sampler(SamplerCommand::Stop(slot as u8)));
            ui.close();
        }
        ui.separator();
        for m in SampleMode::ALL {
            if ui.selectable_label(s.mode == m, m.label()).clicked() {
                app.send(Command::Sampler(SamplerCommand::SetMode(slot as u8, m)));
                ui.close();
            }
        }
        ui.separator();
        if ui
            .selectable_label(s.sync, "Sync to the master tempo")
            .clicked()
        {
            app.send(Command::Sampler(SamplerCommand::SetSync(
                slot as u8, !s.sync,
            )));
            ui.close();
        }
        let rec_label = if recording.is_some() {
            "Stop recording"
        } else {
            "Record the master here"
        };
        if ui.button(rec_label).clicked() {
            app.toggle_record(slot);
            ui.close();
        }
        if ui.button("Empty this slot").clicked() {
            app.eject_sample(slot);
            ui.close();
        }
    });
    app.map.add(ui, "sample_pad", tag, pad_r);

    if let Some(vol_r) = vol_r {
        let (vr, new) = widgets::hfader(
            ui,
            vol_r,
            Id::unique(("sample_volume", slot)),
            s.volume,
            pal.accent,
        );
        vr.on_hover_text(format!("Volume {:.0}%", s.volume * 100.0));
        if let Some(t) = new {
            app.send(Command::Sampler(SamplerCommand::SetVolume(slot as u8, t)));
        }
        app.map.add(ui, "sample_volume", tag, vol_r);
    }
    let Some(ctrl_r) = ctrl_r else {
        return;
    };

    // Mode (click cycles), SYNC, REC, eject.
    let w = ctrl_r.width();
    let mode_w = (w * 0.38).floor();
    let small = ((w - mode_w - 9.0) / 3.0).floor();
    let mode_r = Rect::from_min_size(ctrl_r.min, vec2(mode_w, row_h));
    let sync_r = Rect::from_min_size(pos2(mode_r.right() + 3.0, ctrl_r.top()), vec2(small, row_h));
    let rec_r = Rect::from_min_size(pos2(sync_r.right() + 3.0, ctrl_r.top()), vec2(small, row_h));
    let ej_r = Rect::from_min_max(pos2(rec_r.right() + 3.0, ctrl_r.top()), ctrl_r.max);
    let m = flat_button(
        ui,
        mode_r,
        Id::unique(("sample_mode", slot)),
        mode_name(s.mode),
        theme::prop(10.5),
        None,
        false,
        pal.accent,
        false,
    )
    .on_hover_text("Mode (click for the next one): one-shot, hold, toggle, loop");
    if m.clicked() {
        let i = SampleMode::ALL
            .iter()
            .position(|x| *x == s.mode)
            .unwrap_or(0);
        let next = SampleMode::ALL[(i + 1) % SampleMode::ALL.len()];
        app.send(Command::Sampler(SamplerCommand::SetMode(slot as u8, next)));
    }
    app.map.add(ui, "sample_mode", tag, mode_r);
    let sy = flat_button(
        ui,
        sync_r,
        Id::unique(("sample_sync", slot)),
        "SYNC",
        theme::prop(10.0),
        None,
        s.sync,
        pal.accent,
        false,
    )
    .on_hover_text(match s.bpm {
        Some(b) => format!("Play at the master tempo (sample: {b:.1} BPM)"),
        None => "Play at the master tempo (needs the sample's BPM: drop it from the library or record it)".into(),
    });
    if sy.clicked() {
        app.send(Command::Sampler(SamplerCommand::SetSync(
            slot as u8, !s.sync,
        )));
    }
    app.map.add(ui, "sample_sync", tag, sync_r);
    let rec = flat_button(
        ui,
        rec_r,
        Id::unique(("sample_rec", slot)),
        "REC",
        theme::prop(10.0),
        None,
        recording.is_some(),
        pal.error,
        false,
    )
    .on_hover_text("Record the master output into this slot (again: stop)");
    if rec.clicked() {
        app.toggle_record(slot);
    }
    app.map.add(ui, "sample_rec", tag, rec_r);
    let ej = flat_button(
        ui,
        ej_r,
        Id::unique(("sample_eject", slot)),
        "×",
        theme::prop(12.0),
        None,
        false,
        pal.accent,
        false,
    )
    .on_hover_text("Empty this slot");
    if ej.clicked() {
        app.eject_sample(slot);
    }
    app.map.add(ui, "sample_eject", tag, ej_r);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_modes_and_set_file() {
        assert_eq!(slot_label(0), "A1");
        assert_eq!(slot_label(31), "D8");
        for m in SampleMode::ALL {
            assert_eq!(mode_from(mode_name(m)), m);
        }
        let set = SampleSet {
            slot: vec![SetSlot {
                index: 3,
                path: PathBuf::from("kick.wav"),
                name: "kick".into(),
                mode: "loop".into(),
                volume: 0.5,
                sync: true,
                bpm: Some(126.0),
            }],
        };
        let text = toml::to_string_pretty(&set).expect("toml");
        let back: SampleSet = toml::from_str(&text).expect("parse");
        assert_eq!(back, set);
        let minimal: SampleSet =
            toml::from_str("[[slot]]\nindex = 1\npath = \"a.wav\"\n").expect("parse");
        assert_eq!(minimal.slot[0].volume, 1.0);
    }
}
