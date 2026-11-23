//! Kaldor: 4-deck DJ software.
//!
//! Run without arguments to start the UI. See `kaldor --help` for the
//! diagnostic modes (smoke test, device list, test tone).

// Release builds on Windows are a windowed program: starting Kaldor opens no
// console window. The command-line modes attach to the console they were
// started from (`attach_console`). Debug builds keep the console (logs,
// tests that read the output).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod cli;
mod config_file;
mod paths;
mod real_audio;
mod smoke;

use anyhow::{Context, Result};
use cli::{Mode, Options};
use dj_audio::{AudioOutput, Engine, EngineConfig};
use std::process::ExitCode;

// In debug builds, the audio callback is wrapped in `assert_no_alloc`, which
// aborts on any allocation inside it. This allocator makes that check work.
#[cfg(debug_assertions)]
#[global_allocator]
static ALLOC: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

fn main() -> ExitCode {
    let options = match cli::parse(std::env::args().skip(1)) {
        Ok(o) => o,
        Err(e) => {
            attach_console();
            eprintln!("error: {e}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    // Everything but the window prints to the terminal it was started from.
    if !matches!(options.mode, Mode::Gui) {
        attach_console();
    }
    init_logging(options.verbose);
    match run(options) {
        Ok(code) => code,
        Err(e) => {
            tracing::error!("{e:#}");
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// A windowed release build has no console: use the one of the terminal
/// Kaldor was started from, if any, so `kaldor --help` and the diagnostic
/// modes print there. Output already redirected (to a file or a pipe) is
/// left as it is. Does nothing in debug builds and off Windows.
fn attach_console() {
    #[cfg(all(windows, not(debug_assertions)))]
    {
        use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
        // SAFETY: no pointers; fails harmlessly when there is no parent
        // console or one is attached already.
        let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };
    }
}

fn init_logging(verbose: bool) {
    let default = if verbose { "debug" } else { "info" };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

fn run(options: Options) -> Result<ExitCode> {
    match options.mode {
        Mode::Help => {
            println!("{}", cli::USAGE);
            Ok(ExitCode::SUCCESS)
        }
        Mode::Version => {
            println!("Kaldor {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        Mode::SmokeTest { frames } => smoke::run(frames),
        Mode::ListDevices => list_devices(options.host.as_deref()),
        Mode::TestTone { target, seconds } => test_tone(&options, target, seconds),
        Mode::Gui => gui(&options),
        Mode::ExportXml { ref out } => export_xml(&options, out),
        Mode::ExportUsb(ref u) => export_usb(&options, u),
        Mode::Analyze { ref file } => analyze(file),
    }
}

fn list_devices(host: Option<&str>) -> Result<ExitCode> {
    println!("Audio hosts: {}", dj_audio::device::host_names().join(", "));
    if host.is_some_and(|h| h.eq_ignore_ascii_case("ASIO")) {
        match dj_audio::device::asio_driver_report() {
            Some(drivers) => {
                println!("Registered ASIO drivers ({}):", drivers.len());
                for d in drivers {
                    match d.result {
                        Ok((outs, ins)) => {
                            println!("  {}: loads, {outs} outputs, {ins} inputs", d.name)
                        }
                        Err(e) => println!("  {}: does not load ({e})", d.name),
                    }
                }
            }
            None => println!("This build has no ASIO support (build with --features asio)."),
        }
        println!("Usable ASIO output devices:");
    }
    if host.is_some_and(|h| h.eq_ignore_ascii_case(dj_audio::device::EXCLUSIVE_HOST)) {
        return list_exclusive_devices();
    }
    for d in dj_audio::device::list_output_devices(host)? {
        println!(
            "[{}] {}{}\n    id: {}\n    up to {} output channels, {}-{} Hz",
            d.host,
            d.name,
            if d.is_default { " (default)" } else { "" },
            d.id,
            d.max_output_channels,
            d.min_sample_rate,
            d.max_sample_rate
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// Prints what each output device accepts in WASAPI exclusive mode.
#[cfg(windows)]
fn list_exclusive_devices() -> Result<ExitCode> {
    for d in dj_audio::wasapi_exclusive::report()? {
        println!(
            "[{}] {}{}\n    id: {}\n    Windows format: {}",
            dj_audio::device::EXCLUSIVE_HOST,
            d.name,
            if d.is_default { " (default)" } else { "" },
            d.id,
            d.mix_format
        );
        if let Some((def, min)) = d.periods_ms {
            println!("    device period: default {def:.2} ms, minimum {min:.2} ms");
        }
        if d.formats.is_empty() {
            println!("    exclusive formats: none accepted (or the device is in use)");
        } else {
            println!("    exclusive formats: {}", d.formats.join(", "));
        }
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(not(windows))]
fn list_exclusive_devices() -> Result<ExitCode> {
    anyhow::bail!("WASAPI exclusive mode exists only on Windows")
}

fn audio_config(options: &Options) -> Result<dj_core::AppConfig> {
    let mut cfg = config_file::load(options.config.as_deref())?;
    if let Some(h) = &options.host {
        cfg.audio.host = Some(h.clone());
    }
    if let Some(d) = &options.device {
        cfg.audio.device = Some(d.clone());
    }
    if let Some(r) = options.routing {
        cfg.audio.routing = r;
    }
    Ok(cfg)
}

/// Plays the test tone on a real device for a fixed time, then exits.
fn test_tone(options: &Options, target: dj_audio::ToneTarget, seconds: f32) -> Result<ExitCode> {
    let cfg = audio_config(options)?;
    let plan = dj_audio::device::plan_output(&cfg.audio)?;
    let info = plan.info().clone();
    println!(
        "Playing test tone on {:?} via {} ({} ch, {} Hz) for {seconds} s",
        target, info.device, info.channels, info.sample_rate
    );
    let (engine, mut handle) = Engine::new(EngineConfig {
        sample_rate: info.sample_rate,
        max_block_frames: 512,
        routing: cfg.audio.routing,
    });
    let out = plan.start(engine)?;
    handle
        .send(dj_audio::Command::SetTestTone(Some(
            dj_audio::TestTone::new(target),
        )))
        .ok();
    let end =
        std::time::Instant::now() + std::time::Duration::from_secs_f32(seconds.clamp(0.5, 60.0));
    while std::time::Instant::now() < end {
        std::thread::sleep(std::time::Duration::from_millis(100));
        handle.update();
    }
    handle.send(dj_audio::Command::SetTestTone(None)).ok();
    std::thread::sleep(std::time::Duration::from_millis(100));
    println!(
        "Done. Underruns: {}. Reported output latency: {}",
        out.xruns(),
        out.output_latency()
            .map(|l| format!("{:.2} ms", l.as_secs_f64() * 1000.0))
            .unwrap_or_else(|| "n/a".into())
    );
    Ok(ExitCode::SUCCESS)
}

fn gui(options: &Options) -> Result<ExitCode> {
    // The app directory was called djapp before the rename to Kaldor.
    paths::migrate();
    let cfg = audio_config(options)?;
    let mut backend = real_audio::RealAudio;
    let opened = if options.null_audio {
        dj_ui::audio::Fallback {
            audio: dj_ui::audio::open_null(&cfg.audio).map_err(anyhow::Error::msg)?,
            running: cfg.audio.clone(),
            notice: None,
        }
    } else {
        dj_ui::audio::open_with_fallback(&mut backend, &cfg.audio).map_err(anyhow::Error::msg)?
    };
    if let Some(n) = &opened.notice {
        tracing::warn!("{n}");
    }
    let dj_ui::audio::Fallback {
        audio,
        running,
        notice,
    } = opened;
    tracing::info!(
        "audio: {} / {} at {} Hz",
        audio.info.host,
        audio.info.device,
        audio.info.sample_rate
    );
    let config_path = options.config.clone().or_else(config_file::default_path);
    let mut app = dj_ui::KaldorApp::new(audio.engine, audio.info, Some(audio.output), cfg.clone())
        .with_audio_backend(Box::new(backend), running, notice);
    if let Some(dir) = paths::app_dir() {
        // Sample sets default to <app dir>/samples/.
        app = app.with_data_dir(dir);
    }
    if let Some(p) = config_path {
        match config_file::backup_once(&p) {
            Ok(Some(b)) => tracing::info!("config backed up to {}", b.display()),
            Ok(None) => {}
            Err(e) => tracing::warn!("{e:#}"),
        }
        app = app.on_config_change(move |c| {
            if let Err(e) = config_file::save(&p, c) {
                tracing::warn!("could not save config: {e:#}");
            }
        });
    }
    if !options.no_library {
        let path = library_path(options, &cfg);
        match path.map(|p| (dj_library::Library::open(&p), p)) {
            Some((Ok(lib), p)) => {
                tracing::info!("library: {}", p.display());
                app = app.with_library(lib).use_rekordbox_default_folder();
            }
            Some((Err(e), p)) => tracing::error!("cannot open library {}: {e}", p.display()),
            None => tracing::warn!("no user directory for the library; running without one"),
        }
    }
    if !options.no_controller && cfg.controller.enabled {
        match controller_profiles(&cfg) {
            // Opens the controller when it is plugged in (now or later).
            Ok(p) => app = app.with_controller(dj_ui::controller::ControllerLink::connect_any(p)),
            Err(e) => tracing::warn!("controller profile: {e:#}"),
        }
    }
    dj_ui::run_native(app).map_err(|e| anyhow::anyhow!("UI error: {e}"))?;
    Ok(ExitCode::SUCCESS)
}

/// The controller profiles to look for: `[controller] profile` from the
/// config, else the bundled one chosen with `model` ("auto": every bundled
/// profile, the first one plugged in is used).
fn controller_profiles(cfg: &dj_core::AppConfig) -> Result<Vec<dj_midi::Profile>> {
    if let Some(path) = &cfg.controller.profile {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        return Ok(vec![dj_midi::Profile::parse(&text)?]);
    }
    let bundled = dj_midi::Profile::bundled();
    let chosen: Vec<_> = bundled
        .iter()
        .filter(|(id, _)| *id == cfg.controller.model)
        .map(|(_, p)| p.clone())
        .collect();
    Ok(if chosen.is_empty() {
        bundled.into_iter().map(|(_, p)| p).collect()
    } else {
        chosen
    })
}

fn library_path(options: &Options, cfg: &dj_core::AppConfig) -> Option<std::path::PathBuf> {
    options
        .library
        .clone()
        .or_else(|| cfg.library.path.clone())
        .or_else(|| paths::app_dir().map(|d| d.join("library.db")))
}

/// Writes all crates of the library as rekordbox XML, then exits.
fn export_xml(options: &Options, out: &std::path::Path) -> Result<ExitCode> {
    let cfg = config_file::load(options.config.as_deref())?;
    let path = library_path(options, &cfg).context("no library path")?;
    anyhow::ensure!(path.exists(), "library {} does not exist", path.display());
    let lib = dj_library::Library::open(&path)?;
    let out = &dj_export::xml_output_path(&out.to_string_lossy());
    let file = std::fs::File::create(out).with_context(|| format!("creating {}", out.display()))?;
    let report = dj_export::export_xml(
        &lib,
        &dj_export::XmlExportOptions::default(),
        std::io::BufWriter::new(file),
    )?;
    println!(
        "Wrote {}: {} tracks, {} playlists, {} folders, {} cues, {} loops{}",
        out.display(),
        report.tracks,
        report.playlists,
        report.folders,
        report.cues,
        report.loops,
        if report.missing_files > 0 {
            format!(" ({} files missing on disk)", report.missing_files)
        } else {
            String::new()
        }
    );
    Ok(ExitCode::SUCCESS)
}

/// Prints the file count every 25 tracks.
struct PrintProgress;

impl dj_export::Progress for PrintProgress {
    fn on_file_done(&self, done: usize, total: usize, _file: &std::path::Path) {
        if done.is_multiple_of(25) || done == total {
            println!("  {done}/{total}");
        }
    }
}

/// Puts all crates on a USB stick (Outputs B and C), then exits.
fn export_usb(options: &Options, u: &cli::UsbArgs) -> Result<ExitCode> {
    use dj_export::usb::{self, UsbExportOptions};
    let cfg = config_file::load(options.config.as_deref())?;
    let path = library_path(options, &cfg).context("no library path")?;
    anyhow::ensure!(path.exists(), "library {} does not exist", path.display());
    anyhow::ensure!(
        u.device.is_dir(),
        "{} is not a drive or folder",
        u.device.display()
    );
    let lib = dj_library::Library::open(&path)?;
    let opts = UsbExportOptions {
        device: u.device.clone(),
        crates: Vec::new(),
        format: u.format,
        prune: u.prune,
        artwork: true,
        work_dir: path
            .parent()
            .map_or_else(std::env::temp_dir, std::path::Path::to_path_buf)
            .join("usb-export"),
    };
    let plan = usb::plan_usb(&lib, &opts)?;
    println!("Stick:      {}", u.device.display());
    println!("Libraries:  {}", plan.format.label());
    println!(
        "Tracks:     {} ({} without a beat grid)",
        plan.tracks, plan.without_grid
    );
    println!("Playlists:  {}", plan.playlists.len());
    println!(
        "File system: {}",
        plan.filesystem.as_deref().unwrap_or("unknown")
    );
    println!(
        "Space:      up to {} needed{}",
        usb::size(plan.space_needed),
        plan.space_available
            .map(|a| format!(", {} free", usb::size(a)))
            .unwrap_or_default()
    );
    for w in &plan.warnings {
        println!("Warning:    {w}");
    }
    if plan.replaces_library {
        println!("Note:       the stick's rekordbox library is replaced");
    }
    for (name, why) in &plan.skipped {
        println!("Left out:   {name}: {why}");
    }
    if u.plan_only {
        return Ok(ExitCode::SUCCESS);
    }
    println!("Writing…");
    let r = usb::export_usb(&plan, &PrintProgress, &usb::CancelToken::new())?;
    println!(
        "Done: {} copied, {} kept, {} analysis files, {} removed, {} covers",
        r.copied, r.kept, r.anlz_files, r.pruned, r.artwork
    );
    if let Some(e) = &r.onelibrary_error {
        println!("OneLibrary not written: {e}");
    } else if r.onelibrary_written {
        println!("Device Library and OneLibrary written.");
    }
    for (name, why) in &r.failures {
        println!("Failed:     {name}: {why}");
    }
    println!(
        "Check it:   export-verify {} --xml {}",
        u.device.display(),
        opts.work_dir.join(usb::PLAN_XML).display()
    );
    Ok(if r.failures.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Analyses one file and prints every stage (diagnostics for wrong BPMs).
fn analyze(file: &std::path::Path) -> Result<ExitCode> {
    let (info, audio) =
        dj_decode::decode_file(file, 0).with_context(|| format!("decoding {}", file.display()))?;
    let sr = audio.sample_rate();
    println!(
        "{}\n  {} Hz, {} ch, {:.1} s",
        file.display(),
        info.source_sample_rate,
        info.source_channels,
        audio.duration_seconds()
    );
    let opts = dj_analysis::AnalysisOptions::default();
    let mono = dj_analysis::analyze::mono(&audio);
    print!(
        "{}",
        dj_analysis::tempo::tempo_report(&mono, sr, opts.tempo)
    );
    let a = dj_analysis::analyze_audio(&audio, &opts);
    println!(
        "key: {}  loudness: {}",
        a.key.map_or("none".to_string(), |k| k.camelot()),
        a.lufs
            .map_or("none".to_string(), |l| format!("{l:.1} LUFS"))
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_controller_model_picks_the_profiles() {
        let mut cfg = dj_core::AppConfig::default();
        let names = |cfg: &dj_core::AppConfig| -> Vec<String> {
            controller_profiles(cfg)
                .unwrap()
                .into_iter()
                .map(|p| p.name)
                .collect()
        };
        assert_eq!(cfg.controller.model, "auto");
        assert_eq!(names(&cfg).len(), 2, "automatic: every bundled profile");
        cfg.controller.model = "ddj-flx6".into();
        assert_eq!(names(&cfg), ["Pioneer DJ DDJ-FLX6"]);
        cfg.controller.model = "ns4fx".into();
        assert_eq!(names(&cfg), ["Numark NS4FX"]);
        cfg.controller.model = "something else".into();
        assert_eq!(names(&cfg).len(), 2);
    }
}
