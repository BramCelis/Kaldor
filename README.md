# Kaldor

Free 4-deck DJ software written in Rust, with its source code public under a
noncommercial licence.

Status: **beta**.

## Features

- Four decks with key lock, sync, hot cues, loops, beat jump, slip and
  pad modes (cues, loops, roll, sampler, slicer, pitch play).
- Beatgrid, BPM, key and loudness analysis.
- Mixer with 3-band EQ, filter, color FX and a master limiter.
- Two FX units with 20 beat-synced effects.
- 32-slot sampler.
- Master recording to WAV or FLAC.
- Music library with crates, smart crates, search and play history.
- Import from rekordbox (XML, or kept in step with rekordbox's own library),
  Apple Music / iTunes XML and M3U / PLS playlists.
- Export to rekordbox XML and to a rekordbox USB stick.
- Controllers: Numark NS4FX and Pioneer DJ DDJ-FLX6 built in, plus MIDI
  learn for your own mappings.
- Low-latency WASAPI exclusive mode on Windows; optional ASIO build.

## Build

Requirements: Rust stable (1.95 or newer) and a C++14 compiler (key lock
compiles the vendored Signalsmith Stretch). On Windows use the MSVC toolchain.
On Linux install the ALSA, udev, xkbcommon and Wayland development packages.

```sh
cargo build --release
cargo run --release -p dj-app
```

`kaldor --help` lists the command-line options. The config file lives at
`%APPDATA%\Kaldor\config.toml` on Windows,
`~/Library/Application Support/Kaldor/` on macOS and `~/.config/Kaldor/` on
Linux.

### Windows installer

Unpack the [WiX Toolset v3.14](https://github.com/wixtoolset/wix3/releases)
binaries into `local/wix314`, then:

```powershell
powershell -ExecutionPolicy Bypass -File installer\build-msi.ps1
```

### ASIO (Windows, optional)

```sh
cargo build --release -p dj-app --features asio
```

Needs LLVM (`libclang`) and the Steinberg ASIO SDK unpacked so that
`local/asiosdk/common/asio.h` exists. A binary built with ASIO includes the
ASIO SDK, which has its own licence.

## License

Copyright (c) 2026 Bram Celis. The source code is licensed under the
[PolyForm Noncommercial License 1.0.0](LICENSE.md): you may read, use,
change and share it for noncommercial purposes. Selling Kaldor or software
based on it is not allowed without written permission.

The official builds on the [Releases](https://github.com/BramCelis/Kaldor/releases)
page may be used for anything, including paid DJ gigs.

Bundled third-party code (Signalsmith Stretch, the Archivo font) keeps its
own licence, included next to it.

## Contributing

Bug reports and ideas are welcome under Issues. Pull requests are reviewed
before anything is merged. By opening a pull request you agree that your
contribution may be used in Kaldor under any licence the copyright holder
chooses, including commercially.

This project is not affiliated with Serato, AlphaTheta / Pioneer DJ or
Numark. Product names are used only to describe compatibility.
