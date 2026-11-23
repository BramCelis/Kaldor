//! Changing the audio output while the app runs, and the start-up fallback,
//! with a fake backend built on null outputs (no real device is opened).

use dj_audio::{Command, DeckCommand, MixerCommand};
use dj_core::{AppConfig, AudioConfig, DeckId, OutputRouting};
use dj_ui::audio::{AudioBackend, DeviceChoice, OpenedAudio, open_null, open_with_fallback};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Devices: "dev44" runs at 44.1 kHz, "dev48" at 48 kHz, the default output
/// (no device) at 48 kHz and stereo only. Anything else is "unplugged".
struct FakeBackend {
    working: bool,
}

impl AudioBackend for FakeBackend {
    fn hosts(&self) -> Vec<String> {
        vec!["Fake".into()]
    }

    fn devices(&self, _host: Option<&str>) -> Result<Vec<DeviceChoice>, String> {
        Ok(vec![])
    }

    fn open(&mut self, cfg: &AudioConfig) -> Result<OpenedAudio, String> {
        if !self.working {
            return Err("no audio at all".into());
        }
        let rate = match cfg.device.as_deref() {
            Some("dev44") => 44_100,
            Some("dev48") => 48_000,
            None if cfg.routing.channels_needed() <= 2 => 48_000,
            None => return Err("default output has 2 channels".into()),
            Some(d) => return Err(format!("no output device matches '{d}'")),
        };
        open_null(&AudioConfig {
            sample_rate: rate,
            ..cfg.clone()
        })
    }
}

fn cfg(device: Option<&str>, routing: OutputRouting) -> AudioConfig {
    AudioConfig {
        host: Some("Fake".into()),
        device: device.map(str::to_string),
        routing,
        ..AudioConfig::default()
    }
}

#[test]
fn missing_device_falls_back_to_the_default_output() {
    let mut b = FakeBackend { working: true };
    let f = open_with_fallback(&mut b, &cfg(Some("NS4FX"), OutputRouting::ns4fx())).unwrap();
    assert_eq!(f.running.device, None);
    assert_eq!(f.running.host, None);
    assert_eq!(f.running.routing, OutputRouting::laptop());
    let n = f.notice.unwrap();
    assert!(n.contains("NS4FX") && n.contains("Setup"), "{n}");

    let ok = open_with_fallback(&mut b, &cfg(Some("dev48"), OutputRouting::ns4fx())).unwrap();
    assert!(ok.notice.is_none());
    assert_eq!(ok.running.device.as_deref(), Some("dev48"));
}

#[test]
fn nothing_opens_gives_the_null_output() {
    let mut b = FakeBackend { working: false };
    let f = open_with_fallback(&mut b, &cfg(Some("NS4FX"), OutputRouting::ns4fx())).unwrap();
    assert_eq!(f.audio.info.host, "null");
    assert!(f.notice.unwrap().contains("without sound"));
}

fn write_clicks() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-switch");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("clicks.wav");
    let samples = dj_analysis::synth::ClickTrack::new(128.0, 8.0).render();
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for s in samples {
        w.write_sample((s * 32_000.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    path
}

/// Runs UI frames until `done` or 30 s.
fn frames_until(
    app: &mut dj_ui::KaldorApp,
    mut done: impl FnMut(&dj_ui::KaldorApp) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        dj_ui::headless::run_frames(app, 1, egui::vec2(1280.0, 800.0), |_, _| {});
        if done(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn switching_output_keeps_mixer_and_reloads_tracks_at_the_same_place() {
    let start = cfg(Some("dev48"), OutputRouting::laptop());
    let mut backend = FakeBackend { working: true };
    let audio = backend.open(&start).unwrap();
    let config = AppConfig {
        audio: start.clone(),
        ..AppConfig::default()
    };
    let mut app = dj_ui::KaldorApp::new(audio.engine, audio.info, Some(audio.output), config)
        .with_audio_backend(Box::new(backend), start, None);
    let d1 = DeckId::ALL[0];
    app.load_path(d1, write_clicks());
    assert!(
        frames_until(&mut app, |a| {
            let d = a.snapshot().decks[0];
            d.loaded && d.load_complete && a.has_waveform(d1)
        }),
        "track loads"
    );
    assert!((app.snapshot().decks[0].length - 384_000.0).abs() <= 1.0);

    app.send(Command::Mixer(MixerCommand::Fader(d1, 0.3)));
    app.send(Command::Mixer(MixerCommand::Crossfader(-0.5)));
    app.send(Command::Deck(d1, DeckCommand::SetPitch(0.5)));
    app.send(Command::Deck(d1, DeckCommand::Seek(96_000.0)));
    assert!(frames_until(&mut app, |a| {
        let s = a.snapshot();
        s.decks[0].position == 96_000.0 && s.mixer.channels[0].fader == 0.3
    }));

    // To a 44.1 kHz device: everything carries over, the track is reloaded
    // at the new rate and the playhead stays at 2 s.
    app.switch_audio(cfg(Some("dev44"), OutputRouting::laptop()))
        .unwrap();
    assert_eq!(app.output_info().sample_rate, 44_100);
    assert_eq!(app.config().audio.device.as_deref(), Some("dev44"));
    assert!(app.take_events().contains(&dj_ui::AppEvent::ConfigChanged));
    assert!(
        frames_until(&mut app, |a| {
            let d = a.snapshot().decks[0];
            d.loaded && d.load_complete && d.position > 0.0
        }),
        "track reloads"
    );
    let s = *app.snapshot();
    assert!(
        (s.decks[0].length - 352_800.0).abs() <= 1.0,
        "{}",
        s.decks[0].length
    );
    assert!(
        (s.decks[0].position - 88_200.0).abs() <= 1.0,
        "{}",
        s.decks[0].position
    );
    assert_eq!(s.decks[0].pitch, 0.5);
    assert!(!s.decks[0].playing);
    assert_eq!(s.mixer.channels[0].fader, 0.3);
    assert_eq!(s.mixer.crossfader, -0.5);
    assert!(app.has_waveform(d1));

    // An unplugged device: an error, and the previous output comes back.
    let err = app
        .switch_audio(cfg(Some("NS4FX"), OutputRouting::ns4fx()))
        .unwrap_err();
    assert!(err.contains("NS4FX"), "{err}");
    assert_eq!(app.output_info().sample_rate, 44_100);
    assert_eq!(app.config().audio.device.as_deref(), Some("dev44"));
    assert!(frames_until(&mut app, |a| {
        let s = a.snapshot();
        s.decks[0].loaded && s.mixer.channels[0].fader == 0.3
    }));
}

/// A start-up fallback opens Setup with the output picker; it draws.
#[test]
fn fallback_notice_opens_setup_with_the_picker() {
    let mut backend = FakeBackend { working: true };
    let wanted = cfg(Some("NS4FX"), OutputRouting::ns4fx());
    let f = open_with_fallback(&mut backend, &wanted).unwrap();
    let config = AppConfig {
        audio: wanted,
        ..AppConfig::default()
    };
    let mut app = dj_ui::KaldorApp::new(f.audio.engine, f.audio.info, Some(f.audio.output), config)
        .with_audio_backend(Box::new(backend), f.running, f.notice);
    let r = dj_ui::headless::run_frames(&mut app, 5, egui::vec2(1600.0, 950.0), |_, _| {});
    assert_eq!(r.frames, 5);
    // The picker has every row, the sample rate included.
    for name in [
        "driver",
        "device",
        "outputs",
        "sample_rate",
        "buffer",
        "apply",
    ] {
        assert!(
            !app.ui_map().find(name, None).is_empty(),
            "{name} in the picker"
        );
    }
    // The saved setting is untouched: next start tries the NS4FX again.
    assert_eq!(app.config().audio.device.as_deref(), Some("NS4FX"));
    assert!(app.take_events().is_empty());
}

/// Performance settings survive an output change: quantize, smart sync, slip,
/// and sampler slots loaded from files (with their mode and volume).
#[test]
fn switching_output_keeps_performance_settings_and_samples() {
    let start = cfg(Some("dev48"), OutputRouting::laptop());
    let mut backend = FakeBackend { working: true };
    let audio = backend.open(&start).unwrap();
    let config = AppConfig {
        audio: start.clone(),
        ..AppConfig::default()
    };
    let mut app = dj_ui::KaldorApp::new(audio.engine, audio.info, Some(audio.output), config)
        .with_audio_backend(Box::new(backend), start, None);
    let d1 = DeckId::ALL[0];
    let clicks = write_clicks();
    app.load_path(d1, clicks.clone());
    app.load_sample(2, clicks, Some(128.0), "clicks".into());
    assert!(frames_until(&mut app, |a| {
        let s = a.snapshot();
        s.decks[0].load_complete && s.sampler.slots[2].loaded
    }));
    app.set_quantize(true);
    app.set_smart_sync(false);
    app.send(Command::Deck(d1, DeckCommand::SetSlip(true)));
    app.send(Command::Sampler(dj_audio::SamplerCommand::SetMode(
        2,
        dj_audio::SampleMode::Loop,
    )));
    app.send(Command::Sampler(dj_audio::SamplerCommand::SetVolume(
        2, 0.4,
    )));
    assert!(frames_until(&mut app, |a| {
        let s = a.snapshot();
        s.quantize && s.decks[0].slip && s.sampler.slots[2].mode == dj_audio::SampleMode::Loop
    }));

    app.switch_audio(cfg(Some("dev44"), OutputRouting::laptop()))
        .unwrap();
    assert!(
        frames_until(&mut app, |a| {
            let s = a.snapshot();
            s.decks[0].load_complete && s.sampler.slots[2].loaded
        }),
        "deck and sample reload at 44.1 kHz"
    );
    let s = *app.snapshot();
    assert!(s.quantize);
    assert_eq!(s.sync_mode, dj_audio::SyncMode::Tempo);
    assert!(s.decks[0].slip);
    assert_eq!(s.sampler.slots[2].mode, dj_audio::SampleMode::Loop);
    assert!((s.sampler.slots[2].volume - 0.4).abs() < 1e-6);
    assert_eq!(app.sample_name(2), "clicks");
}

/// A device that can be unplugged: its open outputs stop and it cannot be
/// opened until it is plugged in again.
struct Pluggable {
    plugged: std::sync::Arc<std::sync::atomic::AtomicBool>,
    outputs: std::sync::Arc<std::sync::Mutex<Vec<std::sync::Arc<std::sync::atomic::AtomicBool>>>>,
}

struct PluggableOutput {
    inner: Box<dyn dj_audio::AudioOutput>,
    alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl dj_audio::AudioOutput for PluggableOutput {
    fn info(&self) -> &dj_audio::OutputInfo {
        self.inner.info()
    }
    fn xruns(&self) -> u64 {
        0
    }
    fn output_latency(&self) -> Option<Duration> {
        None
    }
    fn is_running(&self) -> bool {
        self.alive.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl AudioBackend for Pluggable {
    fn hosts(&self) -> Vec<String> {
        vec!["Fake".into()]
    }
    fn devices(&self, _host: Option<&str>) -> Result<Vec<DeviceChoice>, String> {
        Ok(vec![])
    }
    fn open(&mut self, cfg: &AudioConfig) -> Result<OpenedAudio, String> {
        if !self.plugged.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("no output device matches 'NS4FX'".into());
        }
        let mut a = open_null(cfg)?;
        let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        self.outputs
            .lock()
            .unwrap()
            .push(std::sync::Arc::clone(&alive));
        a.output = Box::new(PluggableOutput {
            inner: a.output,
            alive,
        });
        Ok(a)
    }
}

/// Unplugging the audio device and plugging it back in brings the
/// output back by itself (no Apply in Setup), with the mixer settings.
#[test]
fn an_unplugged_output_comes_back_by_itself() {
    use std::sync::atomic::Ordering;
    let plugged = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let outputs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut backend = Pluggable {
        plugged: std::sync::Arc::clone(&plugged),
        outputs: std::sync::Arc::clone(&outputs),
    };
    let start = cfg(Some("NS4FX"), OutputRouting::ns4fx());
    let audio = backend.open(&start).unwrap();
    let config = AppConfig {
        audio: start.clone(),
        ..AppConfig::default()
    };
    let mut app = dj_ui::KaldorApp::new(audio.engine, audio.info, Some(audio.output), config)
        .with_audio_backend(Box::new(backend), start, None);
    let d1 = DeckId::ALL[0];
    app.send(Command::Mixer(MixerCommand::Fader(d1, 0.3)));
    assert!(frames_until(&mut app, |a| a.snapshot().mixer.channels[0]
        .fader
        == 0.3));

    // Unplug: the output stops; the app notices and keeps trying.
    plugged.store(false, Ordering::Relaxed);
    for o in outputs.lock().unwrap().iter() {
        o.store(false, Ordering::Relaxed);
    }
    assert!(frames_until(&mut app, |a| a.audio_lost()), "noticed");
    assert!(app.status_line().contains("lost"), "{}", app.status_line());
    std::thread::sleep(Duration::from_millis(1_200));
    dj_ui::headless::run_frames(&mut app, 2, egui::vec2(1280.0, 800.0), |_, _| {});
    assert!(app.audio_lost(), "still away");

    // Plug back in: within about a second the output is back, same device,
    // with the fader where it was.
    plugged.store(true, Ordering::Relaxed);
    assert!(frames_until(&mut app, |a| !a.audio_lost()), "came back");
    assert!(app.status_line().contains("back"), "{}", app.status_line());
    assert!(frames_until(&mut app, |a| a.snapshot().mixer.channels[0]
        .fader
        == 0.3));
    assert_eq!(app.output_info().channels, 4);
}
