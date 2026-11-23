//! The rekordbox USB export (Outputs B and C) as a window
//! that walks through the steps: choose the stick, check it (the plan:
//! nothing is written), then export with progress and cancel. Opened with
//! Export… in the right-click menu of a crate (that crate and its
//! sub-crates) or of All tracks (all crates).
//!
//! The window is a modal over the dimmed app, like the settings window,
//! and keeps one size through the steps: a header with the title, what is
//! exported and the three steps, the step's content (scrolls), and the
//! buttons at the bottom right.
//!
//! The XML is written on the UI thread, where the library lives (quick);
//! baken's plan and the export run on a worker thread.

use crate::app::KaldorApp;
use crate::library::LibraryUi;
use crate::settings_ui::{button, card, card_fill, switch};
use crate::theme::{self, Palette};
use dj_export::usb::{
    CancelToken, PLAN_XML, Progress, UsbExportOptions, UsbFormat, UsbPlan, UsbReport, size,
};
use dj_export::volume::{self, Drive};
use dj_library::CrateId;
use egui::{
    Align, Align2, Color32, FontId, Id, Layout, Painter, Pos2, Rect, Response, RichText, Sense,
    Stroke, StrokeKind, UiBuilder, Vec2, pos2, vec2,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

/// Files done and total, shared with the worker.
#[derive(Default)]
pub struct Counter {
    done: AtomicUsize,
    total: AtomicUsize,
}

impl Counter {
    /// A counter at `done` of `total` (screenshots).
    pub fn at(done: usize, total: usize) -> Self {
        Self {
            done: AtomicUsize::new(done),
            total: AtomicUsize::new(total),
        }
    }
}

struct SharedProgress(Arc<Counter>);

impl Progress for SharedProgress {
    fn on_file_done(&self, done: usize, total: usize, _file: &Path) {
        self.0.done.store(done, Ordering::Relaxed);
        self.0.total.store(total, Ordering::Relaxed);
    }
}

/// Where the dialog is.
pub enum Stage {
    /// Choosing the stick and the options.
    Setup,
    /// baken is reading the XML, the audio and the stick.
    Planning(Receiver<Result<UsbPlan, String>>),
    /// The plan, waiting for "Export".
    Planned(Box<UsbPlan>),
    Exporting {
        rx: Receiver<Result<UsbReport, String>>,
        counter: Arc<Counter>,
        cancel: CancelToken,
    },
    Done(Result<UsbReport, String>),
}

/// How a drive is shown in the window: its name ("Music SSD (D:)") and a
/// second line (free space of a stick, or "Fixed drive").
#[derive(Clone, Debug, Default)]
pub struct DriveText {
    pub name: String,
    pub detail: String,
}

/// The dialog's state, kept in [`LibraryUi`].
pub struct UsbExport {
    pub open: bool,
    /// What is exported: these crates with their sub-crates; none for all
    /// crates.
    pub target: Vec<CrateId>,
    pub drives: Vec<Drive>,
    /// Name and detail line of each drive (read with the drive list).
    pub drive_text: Vec<DriveText>,
    /// The stick: a drive root such as `E:\`, or any folder.
    pub device: String,
    pub format: UsbFormat,
    pub prune: bool,
    pub artwork: bool,
    pub stage: Stage,
    /// The options of the plan being shown or written.
    last: Option<UsbExportOptions>,
    /// The outcome of "Eject stick" after the last export.
    ejected: Option<Result<(), String>>,
    /// Cancel was pressed during the export (it stops after the current
    /// track).
    cancelling: bool,
    /// The outcome was shown in the window (until then the top bar shows
    /// it).
    pub result_seen: bool,
    /// When the last export ended: a good outcome leaves the top bar
    /// [`DONE_PILL_SECS`] later by itself (a failure stays until seen).
    pub finished_at: Option<std::time::Instant>,
}

/// Seconds "Export done" (or "cancelled") stays in the top bar when it is
/// not clicked.
pub const DONE_PILL_SECS: u64 = 15;

impl Default for UsbExport {
    fn default() -> Self {
        Self {
            open: false,
            target: Vec::new(),
            drives: Vec::new(),
            drive_text: Vec::new(),
            device: String::new(),
            format: UsbFormat::Both,
            prune: true,
            artwork: true,
            stage: Stage::Setup,
            last: None,
            ejected: None,
            cancelling: false,
            result_seen: true,
            finished_at: None,
        }
    }
}

impl UsbExport {
    /// Opens the dialog for crates (with their sub-crates; none: all
    /// crates) and reads the drive list. While a stick is being checked or
    /// written the dialog keeps what it is working on.
    pub fn open_for(&mut self, target: Vec<CrateId>) {
        if !self.busy() {
            if self.target != target && matches!(self.stage, Stage::Planned(_)) {
                // A plan made for other crates does not apply.
                self.stage = Stage::Setup;
            }
            self.target = target;
        }
        self.open = true;
        self.refresh_drives();
        if matches!(self.stage, Stage::Done(_)) {
            self.stage = Stage::Setup;
        }
    }

    pub fn refresh_drives(&mut self) {
        self.drives = volume::drives();
        self.drive_text = self.drives.iter().map(drive_text).collect();
        if self.device.trim().is_empty()
            && let Some(d) = self.drives.iter().find(|d| d.removable)
        {
            self.device = d.root.display().to_string();
        }
    }

    /// Working on the stick (checking or writing).
    pub fn busy(&self) -> bool {
        matches!(self.stage, Stage::Planning(_) | Stage::Exporting { .. })
    }

    /// The window can be closed: in every step but the (short) check. A
    /// closed window keeps exporting; the top bar shows the progress.
    pub fn closable(&self) -> bool {
        !matches!(self.stage, Stage::Planning(_))
    }

    /// The stick is being written (also with the window closed).
    pub fn exporting(&self) -> bool {
        matches!(self.stage, Stage::Exporting { .. })
    }

    /// Opens the window again on the step it is at (the top bar's export
    /// progress was clicked).
    pub fn reopen(&mut self) {
        self.open = true;
    }

    /// Picks up finished work; call every frame.
    pub fn poll(&mut self) {
        match &self.stage {
            Stage::Planning(rx) => match rx.try_recv() {
                Ok(Ok(plan)) => self.stage = Stage::Planned(Box::new(plan)),
                Ok(Err(e)) => self.stage = Stage::Done(Err(e)),
                Err(TryRecvError::Disconnected) => {
                    self.stage = Stage::Done(Err("the check stopped unexpectedly".into()));
                }
                Err(TryRecvError::Empty) => {}
            },
            Stage::Exporting { rx, .. } => match rx.try_recv() {
                Ok(r) => {
                    self.stage = Stage::Done(r);
                    self.finished_at = Some(std::time::Instant::now());
                }
                Err(TryRecvError::Disconnected) => {
                    self.stage = Stage::Done(Err("the export stopped unexpectedly".into()));
                    self.finished_at = Some(std::time::Instant::now());
                }
                Err(TryRecvError::Empty) => {}
            },
            _ => {}
        }
    }

    /// The folder for the export's XML: next to the library file.
    pub fn work_dir(lib: &LibraryUi) -> PathBuf {
        lib.lib
            .file_path()
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .unwrap_or_else(std::env::temp_dir)
            .join("usb-export")
    }

    /// Step 1: writes the XML here, then plans on a worker thread.
    pub fn start_plan(lib: &mut LibraryUi) {
        let u = &lib.usb;
        let crates = u.target.clone();
        let opts = UsbExportOptions {
            device: PathBuf::from(u.device.trim()),
            crates,
            format: u.format,
            prune: u.prune,
            artwork: u.artwork,
            work_dir: Self::work_dir(lib),
        };
        if !opts.device.is_dir() {
            lib.usb.stage = Stage::Done(Err(format!(
                "{} is not a drive or folder that can be opened",
                opts.device.display()
            )));
            return;
        }
        let xml = match dj_export::write_plan_xml(&lib.lib, &opts) {
            Ok(x) => x,
            Err(e) => {
                lib.usb.stage = Stage::Done(Err(e.to_string()));
                return;
            }
        };
        let (tx, rx) = channel();
        let o = opts.clone();
        let spawned = std::thread::Builder::new()
            .name("usb-plan".into())
            .spawn(move || {
                let _ = tx.send(dj_export::plan_from_xml(xml, &o).map_err(|e| e.to_string()));
            });
        lib.usb.stage = match spawned {
            Ok(_) => Stage::Planning(rx),
            Err(e) => Stage::Done(Err(format!("could not start the check: {e}"))),
        };
        lib.usb.last = Some(opts);
        lib.usb.ejected = None;
    }

    /// Step 2: writes the plan to the stick on a worker thread.
    pub fn start_export(&mut self) {
        let Stage::Planned(plan) = std::mem::replace(&mut self.stage, Stage::Setup) else {
            return;
        };
        let counter = Arc::new(Counter::default());
        counter.total.store(plan.tracks, Ordering::Relaxed);
        let cancel = CancelToken::new();
        let (tx, rx) = channel();
        let (c, k) = (counter.clone(), cancel.clone());
        let spawned = std::thread::Builder::new()
            .name("usb-export".into())
            .spawn(move || {
                let r =
                    dj_export::export_usb(&plan, &SharedProgress(c), &k).map_err(|e| e.to_string());
                let _ = tx.send(r);
            });
        self.cancelling = false;
        self.result_seen = false;
        self.stage = match spawned {
            Ok(_) => Stage::Exporting {
                rx,
                counter,
                cancel,
            },
            Err(e) => Stage::Done(Err(format!("could not start the export: {e}"))),
        };
    }
}

/// A drive's name and detail line. Removable drives are asked for their
/// free space (quick on a local stick); other drives are not, so a slow
/// network drive cannot hold up the window.
fn drive_text(d: &Drive) -> DriveText {
    let root = d.root.display().to_string();
    let name = volume::display_name(&d.root).unwrap_or_else(|| {
        if d.label.is_empty() {
            root.clone()
        } else {
            format!("{} ({})", d.label, root.trim_end_matches(['\\', '/']))
        }
    });
    // The icon shows a removable drive as a stick.
    let detail = if d.removable {
        match volume::probe(&d.root).free_bytes {
            Some(free) => format!("{} free", size(free)),
            None => "Removable drive".to_string(),
        }
    } else {
        "Fixed drive".to_string()
    };
    DriveText { name, detail }
}

// ---------------------------------------------------------------------------
// The window

/// What the export covers, for the window's subtitle.
fn scope(lib: &LibraryUi) -> String {
    fn find(nodes: &[dj_library::CrateNode], id: dj_library::CrateId) -> Option<String> {
        nodes.iter().find_map(|n| {
            if n.id == id {
                Some(n.name.clone())
            } else {
                find(&n.children, id)
            }
        })
    }
    let names: Vec<String> = lib
        .usb
        .target
        .iter()
        .filter_map(|c| find(&lib.crates, *c))
        .map(|n| format!("\u{201c}{n}\u{201d}"))
        .collect();
    match (lib.usb.target.len(), names.len()) {
        (0, _) => "All crates, as rekordbox playlists".into(),
        (_, 0) => "Crates that no longer exist".into(),
        (_, 1) => format!("{} and its sub-crates, as rekordbox playlists", names[0]),
        (_, n) => format!("{n} crates with their sub-crates: {}", names.join(", ")),
    }
}

/// Side padding of the window's content.
const PAD: f32 = 28.0;
/// Title, subtitle and steps.
const HEADER_H: f32 = 132.0;
/// The button bar.
const FOOTER_H: f32 = 68.0;
/// Gap between tiles.
const GAP: f32 = 10.0;

/// The window's size: the same in every step, smaller only on a small
/// screen.
fn window_size(screen: Rect) -> Vec2 {
    vec2(
        (screen.width() - 32.0).clamp(300.0, 640.0),
        (screen.height() - 48.0).clamp(300.0, 780.0),
    )
}

/// Controls drawn this frame, for the UI map: name, rectangle, clip.
type Marks = Vec<(&'static str, Rect, Rect)>;

/// What a button asked for (applied after drawing).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Act {
    None,
    Close,
    Check,
    Export,
    Back,
    Cancel,
    New,
    Eject,
}

/// Draws the window when it is open.
pub(crate) fn window(app: &mut KaldorApp, ctx: &egui::Context) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    lib.usb.poll();
    // Also while the window is closed: the top bar shows the progress.
    if lib.usb.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    if !lib.usb.open {
        return;
    }
    if matches!(lib.usb.stage, Stage::Done(_)) {
        lib.usb.result_seen = true;
    }
    let what = scope(lib);
    let size = window_size(ctx.content_rect());
    let frame = egui::Frame::new()
        .fill(if pal.dark { pal.raised } else { pal.bg })
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(16.0)
        .shadow(egui::Shadow {
            offset: [0, 12],
            blur: 40,
            spread: 0,
            color: Color32::from_black_alpha(120),
        });
    let mut marks: Marks = Vec::new();
    let mut act = Act::None;
    let u = &mut lib.usb;
    egui::Modal::new(Id::unique("usb_export_window"))
        .backdrop_color(Color32::from_black_alpha(170))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            let full = ui.max_rect();
            marks.push(("usb_window", full, ui.clip_rect()));
            header(ui, &pal, u, &what, &mut marks, &mut act);

            // The step's content, scrolling between the header and the
            // buttons.
            let body = Rect::from_min_max(
                pos2(full.left(), full.top() + HEADER_H),
                pos2(full.right(), full.bottom() - FOOTER_H),
            );
            let mut bu = ui.new_child(
                UiBuilder::new()
                    .max_rect(body)
                    .layout(Layout::top_down(Align::Min)),
            );
            egui::ScrollArea::vertical()
                .id_salt("usb_body")
                .auto_shrink([false, false])
                .show(&mut bu, |ui| {
                    ui.add_space(20.0);
                    ui.horizontal_top(|ui| {
                        ui.add_space(PAD);
                        ui.vertical(|ui| {
                            ui.set_width((ui.available_width() - PAD).max(100.0));
                            if matches!(u.stage, Stage::Setup) {
                                setup_body(ui, &pal, u, &mut marks);
                                return;
                            }
                            match &u.stage {
                                Stage::Setup => {}
                                Stage::Planning(_) => planning_body(ui, &pal),
                                Stage::Planned(p) => planned_body(ui, &pal, p, u.last.as_ref()),
                                Stage::Exporting { counter, .. } => {
                                    exporting_body(ui, &pal, counter, u.cancelling);
                                }
                                Stage::Done(r) => done_body(ui, &pal, r, u),
                            }
                        });
                    });
                    ui.add_space(20.0);
                });

            // The buttons, right-aligned on a bar of their own.
            let foot = Rect::from_min_max(pos2(full.left(), full.bottom() - FOOTER_H), full.max);
            ui.painter().hline(
                full.left()..=full.right(),
                foot.top(),
                Stroke::new(1.0, pal.border),
            );
            let mut fu = ui.new_child(
                UiBuilder::new()
                    .max_rect(foot.shrink2(vec2(PAD, 0.0)))
                    .layout(Layout::right_to_left(Align::Center)),
            );
            footer(&mut fu, u, &mut marks, &mut act);
        });

    match act {
        Act::None => {}
        Act::Close => {
            // While the stick is written the export goes on (the top bar
            // shows it); only the short check keeps the window.
            if lib.usb.closable() {
                lib.usb.open = false;
                if !matches!(lib.usb.stage, Stage::Done(_) | Stage::Exporting { .. }) {
                    lib.usb.stage = Stage::Setup;
                }
            }
        }
        Act::Check => UsbExport::start_plan(lib),
        Act::Export => lib.usb.start_export(),
        Act::Back | Act::New => lib.usb.stage = Stage::Setup,
        Act::Cancel => {
            if let Stage::Exporting { cancel, .. } = &lib.usb.stage {
                cancel.cancel();
                lib.usb.cancelling = true;
            }
        }
        Act::Eject => {
            if let Some(o) = &lib.usb.last {
                lib.usb.ejected = Some(volume::eject(&o.device));
            }
        }
    }
    for (name, r, clip) in marks {
        app.map.add_rect(name, r, clip);
    }
}

/// Title, what is exported, the close button and the steps.
fn header(
    ui: &mut egui::Ui,
    pal: &Palette,
    u: &UsbExport,
    what: &str,
    marks: &mut Marks,
    act: &mut Act,
) {
    let full = ui.max_rect();
    let left = full.left() + PAD;
    let right = full.right() - PAD;
    let top = full.top();
    let p = ui.painter().clone();
    p.text(
        pos2(left, top + 24.0),
        Align2::LEFT_TOP,
        "Export to USB",
        theme::prop(22.0),
        pal.text,
    );
    let sub = fit(
        &p,
        what,
        theme::prop(13.0),
        pal.text_weak,
        right - left - 56.0,
    );
    p.galley(pos2(left, top + 56.0), sub, pal.text_weak);

    // Round close button: not during the check; during the export it
    // goes on with the window closed.
    let busy = !u.closable();
    let close = Rect::from_center_size(pos2(right - 12.0, top + 38.0), vec2(34.0, 34.0));
    let r = ui
        .interact(
            close,
            Id::unique("usb_close"),
            if busy { Sense::hover() } else { Sense::click() },
        )
        .on_hover_text(if busy {
            "Wait until the check is done"
        } else if u.busy() {
            "Close (Escape): the export goes on, the top bar shows how far it is"
        } else {
            "Close (Escape)"
        });
    if r.hovered() && !busy {
        p.circle_filled(close.center(), 17.0, pal.border);
    }
    let ink = if busy {
        pal.text_weak.gamma_multiply(0.5)
    } else {
        pal.text
    };
    let (c, k) = (close.center(), 6.0);
    for d in [vec2(k, k), vec2(k, -k)] {
        p.line_segment([c - d, c + d], Stroke::new(2.0, ink));
    }
    marks.push(("usb_close", close, ui.clip_rect()));
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape)) && !crate::browse::busy();
    if !busy && (r.clicked() || escape) {
        *act = Act::Close;
    }

    let (current, all_done) = match &u.stage {
        Stage::Setup => (0, false),
        Stage::Planning(_) | Stage::Planned(_) => (1, false),
        Stage::Exporting { .. } => (2, false),
        Stage::Done(r) => (2, matches!(r, Ok(r) if !r.cancelled)),
    };
    steps(
        &p,
        pal,
        pos2(left, top + 90.0),
        right - left,
        current,
        all_done,
    );
    p.hline(
        full.left()..=full.right(),
        top + HEADER_H - 1.0,
        Stroke::new(1.0, pal.border),
    );
}

/// The three steps in a row: done ones with a check, the current one
/// filled, the rest outlined, joined by lines.
fn steps(p: &Painter, pal: &Palette, at: Pos2, width: f32, current: usize, all_done: bool) {
    const LABELS: [&str; 3] = ["Choose stick", "Check", "Export"];
    let slot = width / 3.0;
    let r = 11.0;
    let font = theme::prop(13.5);
    let mut ends: Vec<(f32, f32)> = Vec::new();
    for (i, label) in LABELS.iter().enumerate() {
        let x = at.x + i as f32 * slot;
        let c = pos2(x + r, at.y + r);
        let done = i < current || all_done;
        let on = i == current && !all_done;
        if done || on {
            p.circle_filled(c, r, pal.accent);
        } else {
            p.circle_stroke(c, r - 0.5, Stroke::new(1.5, pal.border));
        }
        if done {
            paint_check(p, c, r * 0.9, Color32::WHITE);
        } else {
            p.text(
                c,
                Align2::CENTER_CENTER,
                (i + 1).to_string(),
                theme::prop(12.0),
                if on { Color32::WHITE } else { pal.text_weak },
            );
        }
        let t = p.text(
            pos2(c.x + r + 8.0, c.y),
            Align2::LEFT_CENTER,
            *label,
            font.clone(),
            if done || on { pal.text } else { pal.text_weak },
        );
        ends.push((x, t.right()));
    }
    for i in 0..2 {
        let from = ends[i].1 + 10.0;
        let to = ends[i + 1].0 - 10.0;
        if to > from + 8.0 {
            let done = i < current || all_done;
            p.hline(
                from..=to,
                at.y + r,
                Stroke::new(1.5, if done { pal.accent } else { pal.border }),
            );
        }
    }
}

/// Step 1: the stick, the libraries and the options.
fn setup_body(ui: &mut egui::Ui, pal: &Palette, u: &mut UsbExport, marks: &mut Marks) {
    if let Some(p) = crate::browse::take("usb_folder") {
        u.device = p.display().to_string();
    }
    // The heading, with Refresh and Other folder… on its right.
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            section(
                ui,
                pal,
                "Stick",
                "FAT32 works in every player, exFAT in the newer ones.",
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
            let f = button(ui, "Other folder…", false, None)
                .on_hover_text("Exports into a folder of your choice instead of a drive");
            marks.push(("usb_folder", f.rect, ui.clip_rect()));
            if f.clicked() {
                crate::browse::open(
                    "usb_folder",
                    "Export to which drive or folder?",
                    crate::browse::Pick::Folder,
                    &u.device,
                );
            }
            let r = button(ui, "Refresh", false, None).on_hover_text("Reads the drive list again");
            marks.push(("usb_refresh", r.rect, ui.clip_rect()));
            if r.clicked() {
                u.refresh_drives();
            }
        });
    });
    // The drives, then the chosen folder if it is not one of them.
    let mut items: Vec<(String, DriveText, Place)> = u
        .drives
        .iter()
        .zip(&u.drive_text)
        .map(|(d, t)| {
            (
                d.root.display().to_string(),
                t.clone(),
                if d.removable {
                    Place::Stick
                } else {
                    Place::Disk
                },
            )
        })
        .collect();
    let device = u.device.trim().to_string();
    if !device.is_empty() && !items.iter().any(|(root, ..)| *root == device) {
        let path = PathBuf::from(&device);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| device.clone());
        items.push((
            device.clone(),
            DriveText {
                name,
                detail: device.clone(),
            },
            Place::Folder,
        ));
    }
    if items.is_empty() {
        let w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(vec2(w, 62.0), Sense::hover());
        ui.painter().rect(
            rect,
            10.0,
            pal.well,
            Stroke::new(1.0, pal.border),
            StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "No drives found. Plug in a stick, then Refresh.",
            theme::prop(13.0),
            pal.text_weak,
        );
    }
    // Three tiles in a row when they still fit a drive's name.
    let per_row = if ui.available_width() >= 540.0 { 3 } else { 2 };
    let tile_w = ((ui.available_width() - GAP * (per_row - 1) as f32) / per_row as f32).floor();
    for row in items.chunks(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            for (root, text, place) in row {
                let selected = *root == device;
                let r = drive_tile(ui, pal, tile_w, text, *place, selected);
                marks.push(("usb_drive", r.rect, ui.clip_rect()));
                if r.clicked() {
                    u.device = root.clone();
                }
            }
        });
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }

    ui.add_space(16.0);
    section(
        ui,
        pal,
        "Libraries on the stick",
        "Which rekordbox libraries are written.",
    );
    for f in UsbFormat::ALL {
        let (title, desc) = match f {
            UsbFormat::Both => (
                "All players",
                "Device Library and OneLibrary, as rekordbox 7 writes a stick",
            ),
            UsbFormat::DeviceLibrary => ("CDJ-3000 and older", "Device Library only"),
        };
        let r = option_tile(ui, pal, title, desc, u.format == f);
        marks.push(("usb_format", r.rect, ui.clip_rect()));
        if r.clicked() {
            u.format = f;
        }
        ui.add_space(GAP - ui.spacing().item_spacing.y);
    }

    ui.add_space(10.0);
    card(ui, pal, "Options", |rows| {
        rows.row(
            "Remove old tracks",
            "Takes tracks of earlier exports that are not in this one off the stick.",
            |ui| {
                let r = switch(ui, u.prune);
                marks.push(("usb_prune", r.rect, ui.clip_rect()));
                if r.clicked() {
                    u.prune = !u.prune;
                }
            },
        );
        rows.row("Artwork", "Copies the covers from the audio files.", |ui| {
            let r = switch(ui, u.artwork);
            marks.push(("usb_artwork", r.rect, ui.clip_rect()));
            if r.clicked() {
                u.artwork = !u.artwork;
            }
        });
    });
}

/// Step 2, while baken reads the tracks and the stick.
fn planning_body(ui: &mut egui::Ui, pal: &Palette) {
    ui.add_space(40.0);
    ui.vertical_centered(|ui| {
        ui.add(egui::Spinner::new().size(28.0).color(pal.accent));
        ui.add_space(14.0);
        ui.label(
            RichText::new("Checking the tracks and the stick…")
                .size(16.0)
                .color(pal.text),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new("Nothing is written yet.")
                .size(13.0)
                .color(pal.text_weak),
        );
    });
}

/// Step 2: what the export would do.
fn planned_body(ui: &mut egui::Ui, pal: &Palette, p: &UsbPlan, opts: Option<&UsbExportOptions>) {
    let free = p.space_available.map(|a| format!("{} free", size(a)));
    stats(
        ui,
        pal,
        &[
            ("Tracks", p.tracks.to_string(), None),
            ("Playlists", p.playlists.len().to_string(), None),
            ("Size, at most", size(p.space_needed), free),
        ],
    );
    if let Some(avail) = p.space_available {
        ui.add_space(12.0);
        let frac = if avail == 0 {
            1.0
        } else {
            p.space_needed as f32 / avail as f32
        };
        bar(
            ui,
            pal,
            frac,
            if frac > 1.0 { pal.error } else { pal.accent },
        );
    }
    ui.add_space(6.0);
    card(ui, pal, "Details", |rows| {
        if let Some(o) = opts {
            let dev = o.device.display().to_string();
            rows.row("Stick", "", |ui| {
                ui.label(RichText::new(dev).color(pal.text))
            });
        }
        let fs = p.filesystem.clone().unwrap_or_else(|| "unknown".into());
        rows.row("File system", "", |ui| {
            ui.label(RichText::new(fs).color(pal.text))
        });
        rows.row("Libraries", "", |ui| {
            ui.label(RichText::new(p.format.label()).color(pal.text))
        });
    });
    ui.add_space(14.0);
    if p.tracks == 0 {
        callout(
            ui,
            pal,
            pal.error,
            "Nothing to export: no track of this crate can go on the stick.",
        );
    }
    for w in &p.warnings {
        callout(ui, pal, pal.warn, w);
    }
    if p.replaces_library {
        callout(
            ui,
            pal,
            pal.warn,
            "The stick already has a rekordbox library. This export replaces it.",
        );
    }
    if p.without_grid > 0 {
        callout(
            ui,
            pal,
            pal.text_weak,
            &format!(
                "{} track(s) have no beat grid yet: analyse them first, or they go without one.",
                p.without_grid
            ),
        );
    }
    if !p.skipped.is_empty() {
        callout_list(
            ui,
            pal,
            pal.warn,
            &format!("{} track(s) left out", p.skipped.len()),
            &p.skipped,
        );
    }
}

/// Step 3, while the stick is written.
fn exporting_body(ui: &mut egui::Ui, pal: &Palette, counter: &Counter, cancelling: bool) {
    let done = counter.done.load(Ordering::Relaxed);
    let total = counter.total.load(Ordering::Relaxed).max(1);
    let frac = done as f32 / total as f32;
    ui.add_space(24.0);
    ui.label(
        RichText::new(if cancelling {
            "Stopping after the current track…"
        } else {
            "Writing to the stick"
        })
        .size(19.0)
        .color(pal.text),
    );
    ui.add_space(4.0);
    ui.label(
        RichText::new(
            "Keep the stick plugged in. You can close this window and keep \
             working: the export goes on, and the top bar shows how far it is. \
             Cancel stops after the current track; the stick then keeps the \
             library it had before.",
        )
        .size(13.0)
        .color(pal.text_weak),
    );
    ui.add_space(22.0);
    bar(ui, pal, frac, pal.accent);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(format!("{done} of {total} tracks"))
                .size(13.0)
                .color(pal.text),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{:.0} %", frac * 100.0))
                    .size(13.0)
                    .color(pal.text_weak),
            );
        });
    });
}

/// The outcome.
fn done_body(ui: &mut egui::Ui, pal: &Palette, r: &Result<UsbReport, String>, u: &UsbExport) {
    let (badge, title, sub) = match r {
        Err(e) => (Badge::Failed, "Export failed".to_string(), e.clone()),
        Ok(r) if r.cancelled => (
            Badge::Stopped,
            "Export cancelled".to_string(),
            "The stick keeps the library it had before.".to_string(),
        ),
        Ok(r) => (
            Badge::Ok,
            "Export complete".to_string(),
            format!(
                "{} track(s) are on the stick as rekordbox playlists.",
                r.copied + r.kept
            ),
        ),
    };
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(44.0, 44.0), Sense::hover());
        paint_badge(ui.painter(), rect.center(), 20.0, badge, pal);
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).size(19.0).color(pal.text));
            ui.add(egui::Label::new(RichText::new(sub).size(13.0).color(pal.text_weak)).wrap());
        });
    });
    ui.add_space(18.0);
    if let Ok(r) = r
        && !r.cancelled
    {
        stats(
            ui,
            pal,
            &[
                ("Copied", r.copied.to_string(), None),
                ("Already there", r.kept.to_string(), None),
                ("Analysis files", r.anlz_files.to_string(), None),
            ],
        );
        ui.add_space(14.0);
        let mut notes = Vec::new();
        if r.onelibrary_written {
            notes.push("Device Library and OneLibrary written.".to_string());
        }
        if r.pruned > 0 {
            notes.push(format!("{} old file(s) removed.", r.pruned));
        }
        if r.prune_skipped {
            notes.push("Old files were kept, because some tracks were not exported.".into());
        }
        if r.artwork > 0 {
            notes.push(format!("{} cover(s) written.", r.artwork));
        }
        for n in notes {
            ui.label(RichText::new(n).size(13.0).color(pal.text_weak));
        }
        if !r.failures.is_empty() || r.onelibrary_error.is_some() {
            ui.add_space(10.0);
        }
        if let Some(e) = &r.onelibrary_error {
            callout(
                ui,
                pal,
                pal.warn,
                &format!("OneLibrary not written ({e}); the Device Library is on the stick."),
            );
        }
        if !r.failures.is_empty() {
            callout_list(
                ui,
                pal,
                pal.warn,
                &format!("{} problem(s)", r.failures.len()),
                &r.failures,
            );
        }
    }
    ui.add_space(10.0);
    match &u.ejected {
        Some(Ok(())) => callout(ui, pal, pal.play, "Ejected: the stick can be unplugged."),
        Some(Err(e)) => callout(ui, pal, pal.warn, &format!("Not ejected: {e}")),
        None => {
            if r.is_ok() {
                ui.label(
                    RichText::new("Eject the stick before you unplug it.")
                        .size(13.0)
                        .color(pal.text_weak),
                );
            }
        }
    }
    if let (Ok(_), Some(o)) = (r, u.last.as_ref()) {
        ui.add_space(14.0);
        ui.add(
            egui::Label::new(
                RichText::new(format!(
                    "To check it: export-verify {} --xml {}",
                    o.device.display(),
                    o.work_dir.join(PLAN_XML).display()
                ))
                .size(11.5)
                .color(pal.text_weak),
            )
            .wrap(),
        );
    }
}

/// The buttons of the step, right to left (the main one rightmost).
fn footer(ui: &mut egui::Ui, u: &UsbExport, marks: &mut Marks, act: &mut Act) {
    let clip = ui.clip_rect();
    let mut push = |name: &'static str, r: &Response, a: Act, act: &mut Act| {
        marks.push((name, r.rect, clip));
        if r.clicked() {
            *act = a;
        }
    };
    match &u.stage {
        Stage::Setup => {
            let ok = !u.device.trim().is_empty();
            let b = action_button(ui, "Check stick", true, ok)
                .on_hover_text("Reads the tracks and the stick and shows what the export would do. Writes nothing yet");
            push("usb_check", &b, Act::Check, act);
            ui.add_space(2.0);
            let c = button(ui, "Cancel", false, None);
            push("usb_cancel", &c, Act::Close, act);
        }
        Stage::Planning(_) => {
            let b = action_button(ui, "Export", true, false);
            push("usb_go", &b, Act::None, act);
        }
        Stage::Planned(p) => {
            let label = format!(
                "Export {} track{}",
                p.tracks,
                if p.tracks == 1 { "" } else { "s" }
            );
            let b = action_button(ui, &label, true, p.tracks > 0);
            push("usb_go", &b, Act::Export, act);
            ui.add_space(2.0);
            let c = button(ui, "Back", false, None);
            push("usb_back", &c, Act::Back, act);
        }
        Stage::Exporting { .. } => {
            let h = button(ui, "Close", true, None)
                .on_hover_text("The export goes on; the top bar shows how far it is");
            push("usb_hide", &h, Act::Close, act);
            ui.add_space(2.0);
            let b = action_button(ui, "Cancel export", false, !u.cancelling)
                .on_hover_text("Stops after the current track; the stick keeps its old library");
            push("usb_stop", &b, Act::Cancel, act);
        }
        Stage::Done(_) => {
            let b = button(ui, "Done", true, None);
            push("usb_done", &b, Act::Close, act);
            ui.add_space(2.0);
            let can_eject = !matches!(u.ejected, Some(Ok(())))
                && u.last
                    .as_ref()
                    .is_some_and(|o| o.device.components().count() <= 2);
            if can_eject {
                let e = button(ui, "Eject stick", false, None).on_hover_text(
                    "Writes out what is cached and ejects the stick, like Windows' Eject",
                );
                push("usb_eject", &e, Act::Eject, act);
                ui.add_space(2.0);
            }
            let n = button(ui, "New export", false, None);
            push("usb_new", &n, Act::New, act);
        }
    }
}

// ---------------------------------------------------------------------------
// The top bar

/// The export in the top bar while its window is closed: a pill with the
/// tracks written and a small bar, then the outcome until the window has
/// shown it. A click opens the window. Returns the pill's rectangle.
pub(crate) fn top_bar_pill(ui: &mut egui::Ui, u: &mut UsbExport, pal: &Palette) -> Option<Rect> {
    if u.open {
        return None;
    }
    // (text, progress while writing, outcome badge when done)
    let (text, frac, badge, tip) = match &u.stage {
        Stage::Exporting { counter, .. } => {
            let done = counter.done.load(Ordering::Relaxed);
            let total = counter.total.load(Ordering::Relaxed).max(1);
            let text = if u.cancelling {
                "USB  stopping…".to_string()
            } else {
                format!("USB  {done}/{total}")
            };
            let tip = format!(
                "Exporting to USB: {done} of {total} tracks written. Keep the stick plugged in. \
                 Click to open the export window."
            );
            (text, Some(done as f32 / total as f32), None, tip)
        }
        Stage::Done(r) if !u.result_seen => {
            // A good outcome goes by itself after a while; a failure stays
            // until it is looked at.
            let age = u.finished_at.map_or(0, |t| t.elapsed().as_secs());
            if r.is_ok() {
                if age >= DONE_PILL_SECS {
                    u.result_seen = true;
                    return None;
                }
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs(1));
            }
            let (badge, text) = match r {
                Ok(r) if r.cancelled => (Badge::Stopped, "Export cancelled"),
                Ok(_) => (Badge::Ok, "Export done"),
                Err(_) => (Badge::Failed, "Export failed"),
            };
            (
                text.to_string(),
                None,
                Some(badge),
                "Click to see the result (and eject the stick)".to_string(),
            )
        }
        _ => return None,
    };
    let font = theme::prop(12.5);
    let text_w = ui
        .painter()
        .layout_no_wrap(text.clone(), font.clone(), pal.text)
        .size()
        .x;
    let bar_w = if frac.is_some() { 44.0 } else { 0.0 };
    let h = 22.0;
    ui.add_space(12.0);
    let (rect, resp) =
        ui.allocate_exact_size(vec2(28.0 + text_w + bar_w + 12.0, h), Sense::click());
    let p = ui.painter();
    p.rect(
        rect,
        h / 2.0,
        pal.well,
        Stroke::new(
            1.0,
            if resp.hovered() {
                pal.accent_line
            } else {
                pal.border
            },
        ),
        StrokeKind::Inside,
    );
    let ic = pos2(rect.left() + 14.0, rect.center().y);
    match badge {
        Some(b) => paint_badge(p, ic, 7.0, b, pal),
        None => {
            // A small stick.
            let ink = Stroke::new(1.4, pal.accent);
            let body = Rect::from_center_size(ic + vec2(0.0, 1.5), vec2(7.0, 8.0));
            p.rect_stroke(body, 1.5, ink, StrokeKind::Middle);
            let plug = Rect::from_min_max(
                pos2(ic.x - 2.0, body.top() - 3.0),
                pos2(ic.x + 2.0, body.top()),
            );
            p.rect_stroke(plug, 0.5, ink, StrokeKind::Middle);
        }
    }
    p.text(
        pos2(rect.left() + 26.0, rect.center().y),
        Align2::LEFT_CENTER,
        &text,
        font,
        pal.text,
    );
    if let Some(f) = frac {
        let track = Rect::from_min_size(
            pos2(rect.right() - 12.0 - bar_w + 4.0, rect.center().y - 2.0),
            vec2(bar_w - 4.0, 4.0),
        );
        p.rect_filled(track, 2.0, pal.border);
        let w = track.width() * f.clamp(0.0, 1.0);
        if w > 0.5 {
            p.rect_filled(
                Rect::from_min_size(track.min, vec2(w.max(4.0), track.height())),
                2.0,
                pal.accent,
            );
        }
    }
    let resp = resp
        .on_hover_text(tip)
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        u.reopen();
    }
    Some(rect)
}

// ---------------------------------------------------------------------------
// Building blocks

/// A section title with a line of explanation.
fn section(ui: &mut egui::Ui, pal: &Palette, title: &str, desc: &str) {
    ui.label(
        RichText::new(title)
            .size(13.5)
            .strong()
            .color(pal.text_weak),
    );
    ui.label(RichText::new(desc).size(12.0).color(pal.text_weak));
    ui.add_space(10.0);
}

/// A settings button that is greyed out and ignores clicks when off.
fn action_button(ui: &mut egui::Ui, label: &str, primary: bool, enabled: bool) -> Response {
    ui.add_enabled_ui(enabled, |ui| {
        if !enabled {
            ui.multiply_opacity(0.4);
        }
        button(ui, label, primary, None)
    })
    .inner
}

/// A tile's background: the accent outline when chosen.
fn tile_bg(p: &Painter, rect: Rect, pal: &Palette, selected: bool, hovered: bool) {
    let (fill, stroke) = if selected {
        (
            pal.accent.gamma_multiply(0.14),
            Stroke::new(1.5, pal.accent),
        )
    } else if hovered {
        (card_fill(pal), Stroke::new(1.0, pal.accent_line))
    } else {
        (card_fill(pal), Stroke::new(1.0, pal.border))
    };
    p.rect(rect, 10.0, fill, stroke, StrokeKind::Inside);
}

/// What a drive tile shows as its icon.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Stick,
    Disk,
    Folder,
}

/// A drive (or folder) to export to: icon, name, detail line, and a check
/// when it is the one chosen.
fn drive_tile(
    ui: &mut egui::Ui,
    pal: &Palette,
    w: f32,
    text: &DriveText,
    place: Place,
    selected: bool,
) -> Response {
    let (rect, r) = ui.allocate_exact_size(vec2(w, 62.0), Sense::click());
    let p = ui.painter();
    tile_bg(p, rect, pal, selected, r.hovered());
    let ic = pos2(rect.left() + 30.0, rect.center().y);
    p.circle_filled(
        ic,
        18.0,
        if selected {
            pal.accent.gamma_multiply(0.22)
        } else {
            pal.well
        },
    );
    paint_place(
        p,
        ic,
        place,
        if selected { pal.accent } else { pal.text_weak },
    );
    let x = rect.left() + 58.0;
    let room = rect.right() - x - 32.0;
    let name = fit(p, &text.name, theme::prop(14.0), pal.text, room);
    p.galley(pos2(x, rect.top() + 13.0), name, pal.text);
    let detail = fit(p, &text.detail, theme::prop(12.0), pal.text_weak, room);
    p.galley(pos2(x, rect.top() + 34.0), detail, pal.text_weak);
    if selected {
        let c = pos2(rect.right() - 18.0, rect.top() + 18.0);
        p.circle_filled(c, 9.0, pal.accent);
        paint_check(p, c, 8.0, Color32::WHITE);
    }
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A choice of several: a radio circle, a title and a description.
fn option_tile(
    ui: &mut egui::Ui,
    pal: &Palette,
    title: &str,
    desc: &str,
    selected: bool,
) -> Response {
    let w = ui.available_width();
    let (rect, r) = ui.allocate_exact_size(vec2(w, 56.0), Sense::click());
    let p = ui.painter();
    tile_bg(p, rect, pal, selected, r.hovered());
    let c = pos2(rect.left() + 24.0, rect.center().y);
    if selected {
        p.circle_stroke(c, 8.5, Stroke::new(2.0, pal.accent));
        p.circle_filled(c, 4.5, pal.accent);
    } else {
        p.circle_stroke(c, 8.5, Stroke::new(1.5, pal.text_weak));
    }
    let x = rect.left() + 46.0;
    p.text(
        pos2(x, rect.top() + 10.0),
        Align2::LEFT_TOP,
        title,
        theme::prop(14.0),
        pal.text,
    );
    let d = fit(
        p,
        desc,
        theme::prop(12.0),
        pal.text_weak,
        rect.right() - x - 14.0,
    );
    p.galley(pos2(x, rect.top() + 31.0), d, pal.text_weak);
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Figures side by side in tiles: label, value, an optional note.
fn stats(ui: &mut egui::Ui, pal: &Palette, items: &[(&str, String, Option<String>)]) {
    let n = items.len().max(1) as f32;
    let w = ((ui.available_width() - GAP * (n - 1.0)) / n).floor();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for (label, value, note) in items {
            let (rect, _) = ui.allocate_exact_size(vec2(w, 84.0), Sense::hover());
            let p = ui.painter();
            p.rect(
                rect,
                10.0,
                card_fill(pal),
                Stroke::new(1.0, pal.border),
                StrokeKind::Inside,
            );
            let x = rect.left() + 14.0;
            let room = rect.width() - 28.0;
            p.text(
                pos2(x, rect.top() + 12.0),
                Align2::LEFT_TOP,
                *label,
                theme::prop(12.0),
                pal.text_weak,
            );
            let v = fit(p, value, theme::prop(22.0), pal.text, room);
            p.galley(pos2(x, rect.top() + 30.0), v, pal.text);
            if let Some(note) = note {
                let t = fit(p, note, theme::prop(11.5), pal.text_weak, room);
                p.galley(pos2(x, rect.top() + 62.0), t, pal.text_weak);
            }
        }
    });
}

/// A slim rounded bar filled to `frac` (0..1, more is shown full).
fn bar(ui: &mut egui::Ui, pal: &Palette, frac: f32, color: Color32) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 8.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, pal.well);
    let fw = rect.width() * frac.clamp(0.0, 1.0);
    if fw > 0.5 {
        p.rect_filled(
            Rect::from_min_size(rect.min, vec2(fw.max(8.0), rect.height())),
            4.0,
            color,
        );
    }
}

/// A tinted note with a dot in `color`.
fn callout(ui: &mut egui::Ui, pal: &Palette, color: Color32, text: &str) {
    callout_frame(color).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            dot(ui, color);
            ui.add(egui::Label::new(RichText::new(text).size(13.0).color(pal.text)).wrap());
        });
    });
    ui.add_space(8.0);
}

/// A tinted note with a title and a short scrolling list (name: why).
fn callout_list(
    ui: &mut egui::Ui,
    pal: &Palette,
    color: Color32,
    title: &str,
    items: &[(String, String)],
) {
    callout_frame(color).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            dot(ui, color);
            ui.label(RichText::new(title).size(13.0).strong().color(pal.text));
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt(title)
            .max_height(110.0)
            .show(ui, |ui| {
                for (name, why) in items {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{name}: {why}"))
                                .size(12.5)
                                .color(pal.text_weak),
                        )
                        .wrap(),
                    );
                }
            });
    });
    ui.add_space(8.0);
}

fn callout_frame(color: Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.12))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.45)))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(12, 9))
}

fn dot(ui: &mut egui::Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
    ui.painter()
        .circle_filled(pos2(r.left() + 4.0, r.top() + 8.5), 3.5, color);
}

/// Text cut to `max_w` with an ellipsis.
fn fit(p: &Painter, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(font, color),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(max_w.max(10.0));
    p.layout_job(job)
}

/// A check mark of size `s` around `c`.
fn paint_check(p: &Painter, c: Pos2, s: f32, color: Color32) {
    let ink = Stroke::new((s * 0.22).max(1.8), color);
    let a = c + vec2(-0.45 * s, 0.0);
    let b = c + vec2(-0.12 * s, 0.33 * s);
    let d = c + vec2(0.48 * s, -0.36 * s);
    p.line_segment([a, b], ink);
    p.line_segment([b, d], ink);
}

/// A USB stick, a disk or a folder, drawn in about 24 points.
fn paint_place(p: &Painter, c: Pos2, place: Place, color: Color32) {
    let ink = Stroke::new(1.8, color);
    match place {
        Place::Stick => {
            let body = Rect::from_center_size(c + vec2(0.0, 3.0), vec2(12.0, 15.0));
            p.rect_stroke(body, 3.0, ink, StrokeKind::Middle);
            let plug = Rect::from_min_max(
                pos2(c.x - 4.0, body.top() - 6.0),
                pos2(c.x + 4.0, body.top()),
            );
            p.rect_stroke(plug, 1.0, ink, StrokeKind::Middle);
        }
        Place::Disk => {
            let body = Rect::from_center_size(c, vec2(20.0, 14.0));
            p.rect_stroke(body, 3.0, ink, StrokeKind::Middle);
            p.circle_filled(pos2(body.right() - 5.0, body.bottom() - 4.5), 1.6, color);
        }
        Place::Folder => {
            let body = Rect::from_min_max(c + vec2(-10.0, -4.0), c + vec2(10.0, 8.0));
            p.rect_stroke(body, 2.5, ink, StrokeKind::Middle);
            let tab = Rect::from_min_max(c + vec2(-10.0, -8.0), c + vec2(-2.0, -4.0));
            p.rect_stroke(tab, 1.5, ink, StrokeKind::Middle);
        }
    }
}

/// The outcome's round badge.
#[derive(Clone, Copy)]
enum Badge {
    Ok,
    Failed,
    Stopped,
}

fn paint_badge(p: &Painter, c: Pos2, r: f32, badge: Badge, pal: &Palette) {
    let fill = match badge {
        Badge::Ok => pal.play,
        Badge::Failed => pal.error,
        Badge::Stopped => pal.text_weak,
    };
    p.circle_filled(c, r, fill);
    let ink = Stroke::new((r * 0.13).max(1.5), Color32::WHITE);
    match badge {
        Badge::Ok => paint_check(p, c, r * 0.95, Color32::WHITE),
        Badge::Failed => {
            let k = r * 0.38;
            p.line_segment([c - vec2(k, k), c + vec2(k, k)], ink);
            p.line_segment([c - vec2(k, -k), c + vec2(k, -k)], ink);
        }
        Badge::Stopped => {
            let k = r * 0.42;
            p.line_segment([c - vec2(k, 0.0), c + vec2(k, 0.0)], ink);
        }
    }
}
