//! Command-line parsing (hand-written; the flag set is small).

use dj_audio::ToneTarget;
use dj_core::OutputRouting;
use std::path::PathBuf;

pub const USAGE: &str = "\
Usage: kaldor [OPTIONS]

Modes (default: start the UI):
  --smoke-test [--frames N]   Render N UI frames (default 120) headless with
                              the offline audio engine, then exit. Opens no
                              window and no audio device.
  --list-devices              List audio hosts and output devices, then exit.
  --analyze FILE              Analyse one audio file and print every stage of
                              the BPM/beatgrid detection, key and loudness.
                              Writes nothing to the library.
  --export-rekordbox-xml OUT  Write the library's crates as rekordbox XML to
                              OUT, then exit (uses --library if given).
  --export-usb DIR            Put the library's crates on a USB stick (a drive
                              root such as E:\\) for rekordbox players, then
                              exit. Prints the plan first.
      --usb-format both|device_library
                              Libraries to write (default both).
      --plan-only             Show the plan; write nothing.
      --keep-old              Do not remove old tracks from the stick.
  --test-tone master|cue      Play a 440 Hz tone on the master or headphone
                              output of the real device, then exit.
      --seconds S             Tone length (default 5, max 60).

Options:
  --config PATH               Config file (default: the user config dir).
  --host NAME                 Audio host, e.g. WASAPI or ASIO.
  --device NAME               Output device name (substring) or id.
  --routing laptop|split|ns4fx
                              Output routing preset. ns4fx = master on
                              outputs 1-2, headphones on 3-4.
  --null-audio                Run the UI without an audio device.
  --library PATH              Library database file (default: library.db in
                              the user config dir).
  --no-library                Run without a library.
  --no-controller             Do not use the DJ controller (NS4FX).
  -v, --verbose               Debug logging (or set RUST_LOG).
  -h, --help                  Show this help.
  -V, --version               Show the version.";

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Gui,
    Help,
    Version,
    SmokeTest { frames: usize },
    ListDevices,
    TestTone { target: ToneTarget, seconds: f32 },
    ExportXml { out: PathBuf },
    ExportUsb(UsbArgs),
    Analyze { file: PathBuf },
}

/// `--export-usb` and its options.
#[derive(Debug, Clone, PartialEq)]
pub struct UsbArgs {
    pub device: PathBuf,
    pub format: dj_export::UsbFormat,
    pub plan_only: bool,
    pub prune: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub mode: Mode,
    pub config: Option<PathBuf>,
    pub host: Option<String>,
    pub device: Option<String>,
    pub routing: Option<OutputRouting>,
    pub null_audio: bool,
    pub library: Option<PathBuf>,
    pub no_library: bool,
    pub no_controller: bool,
    pub verbose: bool,
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Options, String> {
    let mut o = Options {
        mode: Mode::Gui,
        config: None,
        host: None,
        device: None,
        routing: None,
        null_audio: false,
        library: None,
        no_library: false,
        no_controller: false,
        verbose: false,
    };
    let mut frames: Option<usize> = None;
    let mut seconds: Option<f32> = None;
    let mut usb_format = None;
    let (mut plan_only, mut keep_old) = (false, false);
    let mut args = args.into_iter();
    let value = |args: &mut dyn Iterator<Item = String>, flag: &str| {
        args.next().ok_or_else(|| format!("{flag} needs a value"))
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "-h" | "--help" => o.mode = Mode::Help,
            "-V" | "--version" => o.mode = Mode::Version,
            "-v" | "--verbose" => o.verbose = true,
            "--smoke-test" => o.mode = Mode::SmokeTest { frames: 120 },
            "--frames" => {
                let v = value(&mut args, "--frames")?;
                frames = Some(v.parse().map_err(|_| format!("bad --frames value '{v}'"))?);
            }
            "--list-devices" => o.mode = Mode::ListDevices,
            "--analyze" => {
                o.mode = Mode::Analyze {
                    file: PathBuf::from(value(&mut args, "--analyze")?),
                }
            }
            "--export-rekordbox-xml" => {
                o.mode = Mode::ExportXml {
                    out: PathBuf::from(value(&mut args, "--export-rekordbox-xml")?),
                }
            }
            "--export-usb" => {
                o.mode = Mode::ExportUsb(UsbArgs {
                    device: PathBuf::from(value(&mut args, "--export-usb")?),
                    format: dj_export::UsbFormat::default(),
                    plan_only: false,
                    prune: true,
                })
            }
            "--usb-format" => {
                let v = value(&mut args, "--usb-format")?;
                usb_format = Some(dj_export::UsbFormat::from_id(&v).ok_or_else(|| {
                    format!("--usb-format must be both or device_library, not '{v}'")
                })?);
            }
            "--plan-only" => plan_only = true,
            "--keep-old" => keep_old = true,
            "--test-tone" => {
                let v = value(&mut args, "--test-tone")?;
                let target = match v.as_str() {
                    "master" => ToneTarget::Master,
                    "cue" | "headphones" => ToneTarget::Cue,
                    _ => return Err(format!("--test-tone must be master or cue, not '{v}'")),
                };
                o.mode = Mode::TestTone {
                    target,
                    seconds: 5.0,
                };
            }
            "--seconds" => {
                let v = value(&mut args, "--seconds")?;
                seconds = Some(
                    v.parse()
                        .map_err(|_| format!("bad --seconds value '{v}'"))?,
                );
            }
            "--config" => o.config = Some(PathBuf::from(value(&mut args, "--config")?)),
            "--host" => o.host = Some(value(&mut args, "--host")?),
            "--device" => o.device = Some(value(&mut args, "--device")?),
            "--routing" => {
                let v = value(&mut args, "--routing")?;
                o.routing = Some(match v.as_str() {
                    "laptop" => OutputRouting::laptop(),
                    "split" => OutputRouting::laptop_split_cue(),
                    "ns4fx" => OutputRouting::ns4fx(),
                    _ => return Err(format!("unknown routing '{v}'")),
                });
            }
            "--null-audio" => o.null_audio = true,
            "--library" => o.library = Some(PathBuf::from(value(&mut args, "--library")?)),
            "--no-library" => o.no_library = true,
            "--no-controller" => o.no_controller = true,
            other => return Err(format!("unknown argument '{other}'")),
        }
    }
    if let Mode::SmokeTest { frames: f } = &mut o.mode
        && let Some(n) = frames
    {
        *f = n.clamp(1, 100_000);
    }
    if let Mode::TestTone { seconds: s, .. } = &mut o.mode
        && let Some(v) = seconds
    {
        *s = v;
    }
    if let Mode::ExportUsb(u) = &mut o.mode {
        u.format = usb_format.unwrap_or_default();
        u.plan_only = plan_only;
        u.prune = !keep_old;
    } else if usb_format.is_some() || plan_only || keep_old {
        return Err("--usb-format, --plan-only and --keep-old go with --export-usb".into());
    }
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Options, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn defaults_to_gui() {
        assert_eq!(p(&[]).unwrap().mode, Mode::Gui);
    }

    #[test]
    fn usb_export() {
        assert_eq!(
            p(&["--export-usb", "E:\\"]).unwrap().mode,
            Mode::ExportUsb(UsbArgs {
                device: PathBuf::from("E:\\"),
                format: dj_export::UsbFormat::Both,
                plan_only: false,
                prune: true,
            })
        );
        assert_eq!(
            p(&[
                "--plan-only",
                "--export-usb",
                "F:\\",
                "--usb-format",
                "device_library",
                "--keep-old"
            ])
            .unwrap()
            .mode,
            Mode::ExportUsb(UsbArgs {
                device: PathBuf::from("F:\\"),
                format: dj_export::UsbFormat::DeviceLibrary,
                plan_only: true,
                prune: false,
            })
        );
        assert!(p(&["--export-usb", "E:\\", "--usb-format", "x"]).is_err());
        assert!(p(&["--plan-only"]).is_err());
    }

    #[test]
    fn smoke_test_frames() {
        assert_eq!(
            p(&["--smoke-test", "--frames", "10"]).unwrap().mode,
            Mode::SmokeTest { frames: 10 }
        );
        assert_eq!(
            p(&["--smoke-test"]).unwrap().mode,
            Mode::SmokeTest { frames: 120 }
        );
    }

    #[test]
    fn test_tone_and_routing() {
        let o = p(&["--test-tone", "cue", "--seconds", "3", "--routing", "ns4fx"]).unwrap();
        assert_eq!(
            o.mode,
            Mode::TestTone {
                target: ToneTarget::Cue,
                seconds: 3.0
            }
        );
        assert_eq!(o.routing, Some(OutputRouting::ns4fx()));
    }

    #[test]
    fn errors() {
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["--frames"]).is_err());
        assert!(p(&["--test-tone", "left"]).is_err());
    }
}
