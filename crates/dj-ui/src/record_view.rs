//! Master recording in the UI: the REC button in the top bar starts and
//! stops it (see `app/top_bar.rs`); Settings > Recording has the folder,
//! format, file name and this session's recordings.

use crate::app::{AppEvent, KaldorApp};
use crate::recorder::{Recorder, RecorderEvent, default_folder};
use crate::theme::Palette;
use dj_audio::{Command, RecordReader};
use dj_core::config::RecordFormat;
use std::path::PathBuf;

/// A finished recording.
#[derive(Clone, Debug, PartialEq)]
pub struct Take {
    pub path: PathBuf,
    pub seconds: f64,
    pub bytes: u64,
}

#[derive(Default)]
pub struct RecordUi {
    /// The engine's record ring, until the first recording starts.
    pub(crate) reader: Option<RecordReader>,
    pub(crate) recorder: Option<Recorder>,
    /// The file being written.
    pub(crate) current: Option<PathBuf>,
    pub(crate) takes: Vec<Take>,
    pub(crate) status: String,
    /// The folder field (not saved until it changes).
    pub(crate) folder_edit: Option<String>,
}

impl KaldorApp {
    /// The folder recordings go to.
    pub fn recording_folder(&self) -> PathBuf {
        self.config
            .recording
            .folder
            .clone()
            .unwrap_or_else(default_folder)
    }

    /// Master recording is on.
    pub fn is_recording(&self) -> bool {
        self.engine.snapshot().recording
    }

    /// Starts a recording into a new time-stamped file, or stops it.
    pub fn toggle_recording(&mut self) {
        if self.is_recording() || self.rec.current.is_some() {
            self.send(Command::Record(false));
            return;
        }
        if self.rec.recorder.is_none() {
            match self.rec.reader.take() {
                Some(r) => self.rec.recorder = Some(Recorder::spawn(r)),
                None => {
                    self.rec.status = "Recording is not available on this output".into();
                    self.status_line = self.rec.status.clone();
                    return;
                }
            }
        }
        let format = self.config.recording.format;
        let folder = self.recording_folder();
        let base = recording_name(
            &self.config.recording.name_pattern,
            self.rec.takes.len() + 1,
        );
        let mut path = folder.join(format!("{base}.{}", format.extension()));
        let mut n = 2;
        while path.exists() {
            path = folder.join(format!("{base} ({n}).{}", format.extension()));
            n += 1;
        }
        let rate = self.output_info.sample_rate;
        let split = match self.config.recording.split_minutes {
            0 => None,
            m => Some(u64::from(m) * 60 * u64::from(rate)),
        };
        if let Some(rec) = &self.rec.recorder {
            rec.start_split(path.clone(), format, rate, split);
            self.send(Command::Record(true));
            self.rec.current = Some(path);
            self.rec.status.clear();
        }
    }

    /// Takes events from the writer (each UI frame).
    pub(crate) fn poll_recorder(&mut self) {
        let Some(rec) = &self.rec.recorder else {
            return;
        };
        for e in rec.poll() {
            match e {
                RecorderEvent::Started(p) => self.rec.current = Some(p),
                RecorderEvent::Finished {
                    path,
                    frames,
                    bytes,
                } => {
                    let seconds = frames as f64 / f64::from(self.output_info.sample_rate.max(1));
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.rec.status = format!("Saved {name} ({})", clock(seconds));
                    self.status_line = format!("Recording saved: {name} ({})", clock(seconds));
                    if self.rec.current.as_ref() == Some(&path) && !self.is_recording() {
                        self.rec.current = None;
                    }
                    if self.config.recording.add_to_library {
                        self.add_recording_to_library(&path);
                    }
                    self.rec.takes.push(Take {
                        path,
                        seconds,
                        bytes,
                    });
                }
                RecorderEvent::Failed(msg) => {
                    self.rec.status = format!("Recording failed: {msg}");
                    self.status_line = self.rec.status.clone();
                    self.rec.current = None;
                    self.send(Command::Record(false));
                }
            }
        }
    }

    /// A new engine (audio output changed): a running recording ends with
    /// what was recorded so far; the next one uses the new engine's ring.
    pub(crate) fn new_record_reader(&mut self) {
        if self.rec.current.take().is_some() {
            self.rec.status =
                "Recording stopped: the audio output changed (the file is saved)".into();
            self.status_line = self.rec.status.clone();
        }
        // Dropping the writer finishes its file.
        if let Some(old) = self.rec.recorder.take() {
            let events = old.poll();
            drop(old);
            for e in events {
                if let RecorderEvent::Finished {
                    path,
                    frames,
                    bytes,
                } = e
                {
                    self.rec.takes.push(Take {
                        path,
                        seconds: frames as f64 / f64::from(self.output_info.sample_rate.max(1)),
                        bytes,
                    });
                }
            }
        }
        self.rec.reader = self.engine.take_record_reader();
    }
}

/// The file name (without extension) for a recording: `pattern` with
/// `{date}`, `{time}` and `{n}` filled in, characters Windows does not
/// allow in names replaced by `-`. An empty pattern gives the default.
pub fn recording_name(pattern: &str, n: usize) -> String {
    let (y, mo, d, h, mi, s) = crate::recorder::now_parts();
    let pattern = if pattern.trim().is_empty() {
        dj_core::config::RecordingConfig::DEFAULT_PATTERN
    } else {
        pattern.trim()
    };
    let name = pattern
        .replace("{date}", &format!("{y:04}-{mo:02}-{d:02}"))
        .replace("{time}", &format!("{h:02}-{mi:02}-{s:02}"))
        .replace("{n}", &n.to_string());
    let clean: String = name
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_end_matches('.').to_string();
    if clean.is_empty() {
        crate::recorder::timestamp_name()
    } else {
        clean
    }
}

impl KaldorApp {
    /// A finished recording goes into the library, in the crate
    /// "Recordings" (made when missing; not with a protected library).
    fn add_recording_to_library(&mut self, path: &std::path::Path) {
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        let Some(id) = lib.add_file(path) else {
            return;
        };
        if lib.protect {
            return;
        }
        let existing = lib
            .crate_list()
            .into_iter()
            .find(|(_, n, depth)| *depth == 0 && n == "Recordings")
            .map(|(c, _, _)| c);
        let crate_id = match existing {
            Some(c) => Some(c),
            None => lib.lib.create_crate("Recordings", None).ok(),
        };
        if let Some(c) = crate_id {
            let _ = lib.lib.add_to_crate(c, &[id]);
            lib.reload();
        }
    }
}

/// `1:02:03` or `2:03`.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The settings window's Recording tab: the file format, folder and name,
/// and what was recorded this session. Starting and stopping, the time and
/// lost frames are on the REC button in the top bar.
pub(crate) fn record_settings(app: &mut KaldorApp, ui: &mut egui::Ui) {
    use crate::settings_ui::{button, card, segmented};
    let pal = Palette::get(app.config.ui.dark_theme);
    // The file settings are locked while recording.
    let on = app.engine.snapshot().recording;
    let mut marks: Vec<(&'static str, egui::Rect)> = Vec::new();
    let format = app.config.recording.format;
    let mut new_format = None;
    let folder_now = app.recording_folder().display().to_string();
    let mut folder_done: Option<Option<PathBuf>> = None;
    let mut default = false;
    let mut name_done = false;
    let mut new_split: Option<u32> = None;
    card(ui, &pal, "File", |rows| {
        rows.row(
            "Format",
            "WAV: split every ~4 hours. FLAC: about half the size.",
            |ui| {
                ui.add_enabled_ui(!on, |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let sel = Some(usize::from(format == RecordFormat::Flac));
                        let (c, rects) = segmented(ui, &["WAV", "FLAC"], sel);
                        if let Some(i) = c {
                            new_format = Some(if i == 0 {
                                RecordFormat::Wav
                            } else {
                                RecordFormat::Flac
                            });
                        }
                        if let [w, f] = rects[..] {
                            marks.push(("record_wav", w));
                            marks.push(("record_flac", f));
                        }
                    });
                });
            },
        );
        rows.row("Folder", "Where the recordings go.", |ui| {
            ui.add_enabled_ui(!on, |ui| {
                let d = button(ui, "Default", false, None)
                    .on_hover_text(default_folder().display().to_string());
                default = d.clicked();
                let edit = app.rec.folder_edit.get_or_insert(folder_now.clone());
                let r = ui.add(egui::TextEdit::singleline(edit).desired_width(240.0));
                marks.push(("record_folder", r.rect));
                let (br, picked) = crate::browse::button(
                    ui,
                    "record_folder",
                    "Put the recordings in",
                    crate::browse::Pick::Folder,
                    &folder_now,
                );
                marks.push(("record_browse", br.rect));
                if let Some(p) = picked {
                    app.rec.folder_edit = None;
                    folder_done = Some(Some(p));
                } else if r.lost_focus() {
                    let text = app.rec.folder_edit.take().unwrap_or_default();
                    folder_done =
                        Some((!text.trim().is_empty()).then(|| PathBuf::from(text.trim())));
                }
            });
        });
        let preview = recording_name(&app.config.recording.name_pattern, 1);
        rows.row(
            "File name",
            &format!(
                "{{date}}, {{time}} and {{n}} (the number of the recording) are filled in. Next: {preview}"
            ),
            |ui| {
                ui.add_enabled_ui(!on, |ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut app.config.recording.name_pattern)
                            .hint_text(dj_core::config::RecordingConfig::DEFAULT_PATTERN)
                            .desired_width(240.0),
                    );
                    marks.push(("record_name", r.rect));
                    if r.lost_focus() {
                        name_done = true;
                    }
                });
            },
        );
        let splits: [(u32, &str); 5] = [
            (0, "One file"),
            (30, "30 min"),
            (60, "1 hour"),
            (90, "90 min"),
            (120, "2 hours"),
        ];
        rows.row(
            "Split into parts",
            "A long set in several files (name-part2, name-part3, ...). WAV files are always split before 4 GB.",
            |ui| {
                ui.add_enabled_ui(!on, |ui| {
                    let now = app.config.recording.split_minutes;
                    let r = egui::ComboBox::from_id_salt("record_split")
                        .selected_text(
                            splits
                                .iter()
                                .find(|(m, _)| *m == now)
                                .map_or_else(|| format!("{now} min"), |(_, l)| (*l).to_string()),
                        )
                        .width(120.0)
                        .show_ui(ui, |ui| {
                            for (m, l) in splits {
                                if ui.selectable_label(m == now, l).clicked() {
                                    new_split = Some(m);
                                }
                            }
                        });
                    marks.push(("record_split", r.response.rect));
                });
            },
        );
        rows.row(
            "Add to the library",
            "Each finished recording goes into the library, in the crate Recordings.",
            |ui| {
                let r = crate::settings_ui::switch(ui, app.config.recording.add_to_library);
                if r.clicked() {
                    app.config.recording.add_to_library = !app.config.recording.add_to_library;
                    name_done = true;
                }
                marks.push(("record_add_library", r.rect));
            },
        );
    });
    if let Some(m) = new_split {
        app.config.recording.split_minutes = m;
        app.events.push(AppEvent::ConfigChanged);
    }
    if name_done {
        app.events.push(AppEvent::ConfigChanged);
    }
    if let Some(f) = new_format
        && f != app.config.recording.format
    {
        app.config.recording.format = f;
        app.events.push(AppEvent::ConfigChanged);
    }
    if let Some(new) = folder_done
        && new != app.config.recording.folder
    {
        app.config.recording.folder = new;
        app.events.push(AppEvent::ConfigChanged);
    }
    if default {
        app.config.recording.folder = None;
        app.rec.folder_edit = None;
        app.events.push(AppEvent::ConfigChanged);
    }
    if !app.rec.takes.is_empty() {
        let takes: Vec<(String, String)> = app
            .rec
            .takes
            .iter()
            .rev()
            .map(|t| {
                (
                    t.path.file_name().map_or_else(
                        || t.path.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    ),
                    format!("{}  ·  {:.1} MB", clock(t.seconds), t.bytes as f64 / 1e6),
                )
            })
            .collect();
        card(ui, &pal, "Recorded this session", |rows| {
            for (name, info) in &takes {
                rows.row(name, info, |_| {});
            }
        });
    }
    for (name, r) in marks {
        app.map.add(ui, name, None, r);
    }
}

impl KaldorApp {
    /// Files recorded this session (oldest first).
    pub fn recorded_files(&self) -> Vec<PathBuf> {
        self.rec.takes.iter().map(|t| t.path.clone()).collect()
    }
}
