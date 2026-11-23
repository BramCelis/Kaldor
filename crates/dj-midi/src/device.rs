//! MIDI ports through `midir` (WinMM on Windows, CoreMIDI on macOS, ALSA
//! on Linux). Tests never open a port; this module is used by the tools and
//! the app only.

use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use thiserror::Error;

/// Client name shown to the operating system.
pub const CLIENT_NAME: &str = "Kaldor";

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("MIDI is not available: {0}")]
    Init(String),
    #[error("no MIDI {kind} port matches \"{wanted}\" (ports: {ports})")]
    NoPort {
        kind: &'static str,
        wanted: String,
        ports: String,
    },
    #[error("cannot open MIDI port {0}")]
    Connect(String),
}

/// Names of the MIDI input ports.
pub fn input_ports() -> Result<Vec<String>, DeviceError> {
    let m = MidiInput::new(CLIENT_NAME).map_err(|e| DeviceError::Init(e.to_string()))?;
    Ok(m.ports()
        .iter()
        .filter_map(|p| m.port_name(p).ok())
        .collect())
}

/// Names of the MIDI output ports.
pub fn output_ports() -> Result<Vec<String>, DeviceError> {
    let m = MidiOutput::new(CLIENT_NAME).map_err(|e| DeviceError::Init(e.to_string()))?;
    Ok(m.ports()
        .iter()
        .filter_map(|p| m.port_name(p).ok())
        .collect())
}

/// The first port whose name contains `wanted` (case-insensitive).
pub fn find_port(ports: &[String], wanted: &str) -> Option<String> {
    let w = wanted.to_lowercase();
    ports
        .iter()
        .find(|p| p.to_lowercase().contains(&w))
        .cloned()
}

/// An open input port; messages go to the callback given to
/// [`open_input`] until this is dropped.
pub struct Input {
    pub name: String,
    _conn: MidiInputConnection<()>,
}

/// Opens the first input port whose name contains `wanted` and calls
/// `on_message(timestamp_us, bytes)` for every message, on the MIDI
/// driver's thread. System exclusive messages are delivered too.
pub fn open_input(
    wanted: &str,
    mut on_message: impl FnMut(u64, &[u8]) + Send + 'static,
) -> Result<Input, DeviceError> {
    let mut m = MidiInput::new(CLIENT_NAME).map_err(|e| DeviceError::Init(e.to_string()))?;
    m.ignore(Ignore::None);
    let ports = m.ports();
    let names: Vec<String> = ports.iter().filter_map(|p| m.port_name(p).ok()).collect();
    let w = wanted.to_lowercase();
    let Some((port, name)) = ports
        .iter()
        .zip(&names)
        .find(|(_, n)| n.to_lowercase().contains(&w))
    else {
        return Err(DeviceError::NoPort {
            kind: "input",
            wanted: wanted.into(),
            ports: names.join(", "),
        });
    };
    let name = name.clone();
    let conn = m
        .connect(port, "kaldor-in", move |t, b, _| on_message(t, b), ())
        .map_err(|e| DeviceError::Connect(format!("{name}: {e}")))?;
    Ok(Input { name, _conn: conn })
}

/// An open output port.
pub struct Output {
    pub name: String,
    conn: MidiOutputConnection,
}

impl Output {
    pub fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.conn.send(bytes).map_err(|e| e.to_string())
    }
}

/// Opens the first output port whose name contains `wanted`.
pub fn open_output(wanted: &str) -> Result<Output, DeviceError> {
    let m = MidiOutput::new(CLIENT_NAME).map_err(|e| DeviceError::Init(e.to_string()))?;
    let ports = m.ports();
    let names: Vec<String> = ports.iter().filter_map(|p| m.port_name(p).ok()).collect();
    let w = wanted.to_lowercase();
    let Some((port, name)) = ports
        .iter()
        .zip(&names)
        .find(|(_, n)| n.to_lowercase().contains(&w))
    else {
        return Err(DeviceError::NoPort {
            kind: "output",
            wanted: wanted.into(),
            ports: names.join(", "),
        });
    };
    let name = name.clone();
    let conn = m
        .connect(port, "kaldor-out")
        .map_err(|e| DeviceError::Connect(format!("{name}: {e}")))?;
    Ok(Output { name, conn })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_ports_by_part_of_the_name() {
        let ports = vec![
            "Microsoft GS Wavetable Synth".to_string(),
            "NS4FX".to_string(),
        ];
        assert_eq!(find_port(&ports, "ns4"), Some("NS4FX".into()));
        assert_eq!(find_port(&ports, "launchpad"), None);
    }
}
