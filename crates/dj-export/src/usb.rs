//! Outputs B and C: a rekordbox USB stick.
//!
//! The stick is written by [baken-export] (MIT), a writer for the Device
//! Library (`export.pdb` and the per-track ANLZ `.DAT`/`.EXT`/`.2EX` files)
//! and OneLibrary (`exportLibrary.db`), whose layouts were read off real
//! rekordbox 7 exports. Its input is a rekordbox XML, which Kaldor already
//! writes (Output A, checked in rekordbox):
//!
//! 1. [`plan_usb`] writes the XML for the chosen crates into a work folder,
//!    then asks baken for a plan: what goes on the stick, what is skipped
//!    and why, the space it needs, and the stick's format. Nothing is
//!    written to the stick.
//! 2. [`export_usb`] writes the plan: audio into `Contents/`, analysis
//!    files, `export.pdb`, and with [`UsbFormat::Both`] the OneLibrary.
//!    Files already on the stick and unchanged are not written again; with
//!    `prune`, files of tracks no longer exported are removed.
//!
//! The analysis files are always computed from Kaldor's data: beat grid and
//! cues from the XML, waveforms from the audio. baken is pointed at an empty
//! analysis folder, so a rekordbox installation on the same computer is
//! never used. rekordbox's "My Settings" are not written (the player keeps
//! its own).
//!
//! [baken-export]: https://github.com/M-Igashi/baken

use crate::rekordbox_xml::{ExportError, ExportReport, XmlExportOptions, export_xml};
use crate::volume;
use baken_export as bx;

use dj_library::{CrateId, Library};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub use baken_core::{CancelToken, Progress};

/// Which libraries go on the stick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UsbFormat {
    /// Device Library and OneLibrary, as rekordbox 7.2.11 and later write
    /// them: one stick for old and new players.
    #[default]
    Both,
    /// Device Library only (`export.pdb`): CDJ-3000, XDJ-XZ and older.
    DeviceLibrary,
}

impl UsbFormat {
    pub const ALL: [UsbFormat; 2] = [UsbFormat::Both, UsbFormat::DeviceLibrary];

    pub fn label(self) -> &'static str {
        match self {
            UsbFormat::Both => "Device Library + OneLibrary (all players)",
            UsbFormat::DeviceLibrary => "Device Library only (CDJ-3000 and older)",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            UsbFormat::Both => "both",
            UsbFormat::DeviceLibrary => "device_library",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.id() == id)
    }
}

/// What to export where.
#[derive(Clone, Debug)]
pub struct UsbExportOptions {
    /// The stick (a drive root such as `E:\`), or any folder (tests).
    pub device: PathBuf,
    /// Crates to export (with their sub-crates); empty = all crates.
    pub crates: Vec<CrateId>,
    pub format: UsbFormat,
    /// Remove audio and analysis of tracks this export no longer has.
    pub prune: bool,
    /// Put each file's embedded cover on the stick as artwork.
    pub artwork: bool,
    /// Where the intermediate rekordbox XML and the empty analysis folder
    /// go (Kaldor's app folder).
    pub work_dir: PathBuf,
}

/// Errors of planning or writing the stick.
#[derive(Debug, thiserror::Error)]
pub enum UsbError {
    #[error(transparent)]
    Xml(#[from] ExportError),
    #[error("{0}")]
    Plan(String),
    #[error(
        "not enough space on the stick: the export needs up to {} and {} is free",
        size(*needed),
        size(*available)
    )]
    NotEnoughSpace { needed: u64, available: u64 },
    #[error("{0}")]
    Export(String),
    #[error("write error: {0}")]
    Io(#[from] std::io::Error),
}

/// A plan: what the export would do. Nothing is written yet.
pub struct UsbPlan {
    plan: bx::Plan,
    /// The intermediate XML.
    pub xml: ExportReport,
    /// Tracks that go on the stick.
    pub tracks: usize,
    /// Playlist paths (`Folder/Name`).
    pub playlists: Vec<String>,
    /// Tracks left out, with the reason.
    pub skipped: Vec<(String, String)>,
    /// Format warnings (file system, partition table), as sentences.
    pub warnings: Vec<String>,
    /// The stick's file system as read, if it could be.
    pub filesystem: Option<String>,
    pub space_needed: u64,
    pub space_available: Option<u64>,
    /// Tracks without a beat grid (they get none on the stick either).
    pub without_grid: usize,
    pub format: UsbFormat,
    /// The stick already holds a rekordbox library this export replaces.
    pub replaces_library: bool,
    /// Remove files of tracks no longer exported (see `prune_stick`).
    prune: bool,
}

/// What an export did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UsbReport {
    pub copied: usize,
    pub kept: usize,
    pub anlz_files: usize,
    pub anlz_unchanged: usize,
    pub pruned: usize,
    pub onelibrary_written: bool,
    pub onelibrary_error: Option<String>,
    pub artwork: usize,
    pub cancelled: bool,
    /// Tracks that failed while writing, with the message.
    pub failures: Vec<(String, String)>,
    /// Pruning was asked for but left out, because tracks failed or their
    /// files were not found (their old copies on the stick stay).
    pub prune_skipped: bool,
}

/// `1.2 GB`-style sizes for messages.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1000.0 && u + 1 < UNITS.len() {
        v /= 1000.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// The XML written for the export, inside the work folder.
pub const PLAN_XML: &str = "kaldor-usb-export.xml";
/// An empty folder baken looks in for rekordbox's analysis (none, on
/// purpose: Kaldor's grids, cues and waveforms are used).
const NO_ANALYSIS_DIR: &str = "no-rekordbox-analysis";

/// Plans the export (step 1): writes the XML into `work_dir` and reads the
/// stick. Writes nothing to the stick.
pub fn plan_usb(lib: &Library, opts: &UsbExportOptions) -> Result<UsbPlan, UsbError> {
    let xml = write_plan_xml(lib, opts)?;
    plan_from_xml(xml, opts)
}

/// The library half of [`plan_usb`]: the XML for the chosen crates, into
/// `work_dir`. Needs the library, so it runs where the library lives (the
/// UI thread); it is quick.
pub fn write_plan_xml(lib: &Library, opts: &UsbExportOptions) -> Result<ExportReport, UsbError> {
    std::fs::create_dir_all(&opts.work_dir)?;
    let f = std::io::BufWriter::new(std::fs::File::create(opts.work_dir.join(PLAN_XML))?);
    Ok(export_xml(
        lib,
        &XmlExportOptions {
            crates: opts.crates.clone(),
            all_tracks: false,
            loops_to_free_pads: true,
            precise_tempo: true,
        },
        f,
    )?)
}

/// The stick half of [`plan_usb`], after [`write_plan_xml`]: reads the
/// XML, the audio files and the stick. Can take a while with many tracks;
/// needs no library, so it can run on a worker thread.
pub fn plan_from_xml(xml: ExportReport, opts: &UsbExportOptions) -> Result<UsbPlan, UsbError> {
    let xml_path = opts.work_dir.join(PLAN_XML);
    let no_anlz = opts.work_dir.join(NO_ANALYSIS_DIR);
    if no_anlz.exists() {
        std::fs::remove_dir_all(&no_anlz)?;
    }
    std::fs::create_dir_all(&no_anlz)?;
    let bopts = bx::Options {
        xml: xml_path,
        device: opts.device.clone(),
        playlists: Vec::new(),
        anlz_roots: vec![no_anlz],
        settings_dir: None,
        no_settings: true,
        device_name: None,
        cdjsafe: false,
        generate_analysis: true,
        // On Windows baken's own prune compares `E:\Contents/A/b.mp3` with
        // `E:\Contents\A\b.mp3` and removes what it just wrote; Kaldor
        // prunes there itself (`prune_stick`).
        prune: opts.prune && !cfg!(windows),
        onelibrary: opts.format == UsbFormat::Both,
        artwork: opts.artwork,
        my_tags: None,
    };
    let mut plan = bx::plan(&bopts).map_err(|e| UsbError::Plan(e.to_string()))?;
    // baken reads the volume on macOS and Linux; on Windows Kaldor does.
    let v = volume::probe(&opts.device);
    if plan.filesystem.is_none() {
        plan.filesystem = v.filesystem.clone();
    }
    if plan.partition_table.is_none() {
        plan.partition_table = v.partition_table;
    }
    if plan.partition_type.is_none() {
        plan.partition_type = v.partition_type;
    }
    if plan.space_available.is_none() {
        plan.space_available = v.free_bytes;
    }
    if let Some(available) = plan.space_available
        && plan.space_needed > available
    {
        return Err(UsbError::NotEnoughSpace {
            needed: plan.space_needed,
            available,
        });
    }
    let warnings = plan
        .format_warnings()
        .iter()
        .map(ToString::to_string)
        .collect();
    let skipped = plan
        .skipped
        .iter()
        .map(|s| (s.name.clone(), s.reason.to_string()))
        .collect();
    let replaces_library = ["rekordbox/export.pdb", "rekordbox/exportLibrary.db"]
        .iter()
        .any(|f| opts.device.join(plan.root.name()).join(f).exists());
    Ok(UsbPlan {
        xml,
        tracks: plan.tracks.len(),
        playlists: plan
            .playlist_names()
            .into_iter()
            .map(String::from)
            .collect(),
        skipped,
        warnings,
        filesystem: plan.filesystem.as_ref().map(ToString::to_string),
        space_needed: plan.space_needed,
        space_available: plan.space_available,
        without_grid: plan.without_grid(),
        format: opts.format,
        replaces_library,
        prune: opts.prune,
        plan,
    })
}

/// A path as a comparison key: separators unified, case folded (FAT and
/// exFAT ignore case).
fn key(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect::<Vec<_>>()
        .join("/")
}

/// The stick path `/Contents/A/b.mp3` under `device`, with native separators.
fn on_device(device: &Path, usb_path: &str) -> PathBuf {
    let mut p = device.to_path_buf();
    p.extend(usb_path.split('/').filter(|s| !s.is_empty()));
    p
}

/// Removes the audio and analysis files under `Contents/` and
/// `<root>/USBANLZ/` that the plan does not have (Windows; elsewhere baken
/// does this). Folders left empty go too. Returns the files removed; what
/// cannot be removed is noted in `errors` and stays.
fn prune_stick(plan: &bx::Plan, errors: &mut Vec<(String, String)>) -> usize {
    let mut keep = HashSet::new();
    for t in &plan.tracks {
        keep.insert(key(&on_device(&plan.device, &t.device.usb_path)));
        for ext in ["DAT", "EXT", "2EX"] {
            keep.insert(key(&on_device(&plan.device, &t.device.anlz_path(ext))));
        }
    }
    fn walk(
        dir: &Path,
        keep: &HashSet<String>,
        removed: &mut usize,
        errors: &mut Vec<(String, String)>,
    ) -> bool {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return false;
        };
        let mut empty = true;
        for e in entries.flatten() {
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if walk(&p, keep, removed, errors) {
                    if std::fs::remove_dir(&p).is_err() {
                        empty = false;
                    }
                } else {
                    empty = false;
                }
            } else if keep.contains(&key(&p)) {
                empty = false;
            } else if let Err(err) = std::fs::remove_file(&p) {
                errors.push((p.display().to_string(), format!("not removed: {err}")));
                empty = false;
            } else {
                *removed += 1;
            }
        }
        empty
    }
    let mut removed = 0;
    for root in [
        plan.device.join("Contents"),
        plan.device.join(plan.root.name()).join("USBANLZ"),
    ] {
        walk(&root, &keep, &mut removed, errors);
    }
    removed
}

/// Writes a plan to the stick (step 2). `progress` hears about every file;
/// `cancel` stops between tracks (the stick keeps its old library).
pub fn export_usb(
    plan: &UsbPlan,
    progress: &dyn Progress,
    cancel: &CancelToken,
) -> Result<UsbReport, UsbError> {
    let r =
        bx::export(&plan.plan, progress, cancel).map_err(|e| UsbError::Export(e.to_string()))?;
    let mut report = UsbReport {
        copied: r.copied,
        kept: r.kept,
        anlz_files: r.anlz_files,
        anlz_unchanged: r.anlz_unchanged,
        pruned: r.pruned,
        onelibrary_written: r.onelibrary_written,
        onelibrary_error: r.onelibrary_error.clone(),
        artwork: r.artwork,
        cancelled: r.cancelled,
        failures: r
            .failures
            .iter()
            .map(|f| (f.name.clone(), f.message.clone()))
            .collect(),
        prune_skipped: r.prune_skipped,
    };
    // As baken does: never prune after a run that left tracks out (a
    // failure, a source not found), their files may be the only copy.
    if cfg!(windows) && plan.prune && !report.cancelled {
        let missing = plan.plan.skipped.iter().any(|s| s.source_missing);
        if report.failures.is_empty() && !missing {
            let mut errors = Vec::new();
            report.pruned = prune_stick(&plan.plan, &mut errors);
            report.failures.extend(errors);
        } else {
            report.prune_skipped = true;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_well() {
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1_500_000), "1.5 MB");
        assert_eq!(size(7_300_000_000), "7.3 GB");
    }

    #[test]
    fn format_ids_round_trip() {
        for f in UsbFormat::ALL {
            assert_eq!(UsbFormat::from_id(f.id()), Some(f));
        }
    }
}
