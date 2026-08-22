//! MIDI device enumeration and connection API.
//!
//! Provides a clean interface for listing available MIDI output ports and
//! connecting by index or name. Used by the Tauri app for port enumeration
//! and by the CLI for port selection.

use midir::{MidiOutput, MidiOutputConnection};

/// Information about an available MIDI output port.
#[derive(Debug, Clone)]
pub struct MidiPortInfo {
    /// Zero-based port index.
    pub index: usize,
    /// Port name as reported by the OS.
    pub name: String,
}

/// Errors from MIDI device operations.
#[derive(Debug, thiserror::Error)]
pub enum MidiDeviceError {
    /// No MIDI output ports available on the system.
    #[error("no MIDI output ports available")]
    NoPorts,
    /// Port index is out of range.
    #[error("port index {0} out of range")]
    PortOutOfRange(usize),
    /// No port matching the given name.
    #[error("no port matching '{0}'")]
    PortNotFound(String),
    /// Failed to initialize MIDI subsystem.
    #[error("MIDI init failed: {0}")]
    InitFailed(String),
    /// Failed to connect to the selected port.
    #[error("failed to connect: {0}")]
    ConnectionFailed(String),
}

/// List all available MIDI output ports.
pub fn list_midi_outputs() -> Result<Vec<MidiPortInfo>, MidiDeviceError> {
    let midi_out = MidiOutput::new("Interval-enumerate")
        .map_err(|e| MidiDeviceError::InitFailed(e.to_string()))?;
    let ports = midi_out.ports();
    let mut result = Vec::with_capacity(ports.len());
    for (i, port) in ports.iter().enumerate() {
        let name = midi_out.port_name(port).unwrap_or_else(|_| "?".into());
        result.push(MidiPortInfo { index: i, name });
    }
    Ok(result)
}

/// Open a MIDI output connection by port index.
pub fn connect_midi_output(port_index: usize) -> Result<MidiOutputConnection, MidiDeviceError> {
    let midi_out =
        MidiOutput::new("Interval").map_err(|e| MidiDeviceError::InitFailed(e.to_string()))?;
    let ports = midi_out.ports();
    if ports.is_empty() {
        return Err(MidiDeviceError::NoPorts);
    }
    if port_index >= ports.len() {
        return Err(MidiDeviceError::PortOutOfRange(port_index));
    }
    midi_out
        .connect(&ports[port_index], "interval-play")
        .map_err(|e| MidiDeviceError::ConnectionFailed(format!("{}", e.kind())))
}

/// Open a MIDI output connection by port name (partial match).
///
/// Finds the first port whose name contains the given string (case-insensitive).
pub fn connect_midi_output_by_name(name: &str) -> Result<MidiOutputConnection, MidiDeviceError> {
    let midi_out =
        MidiOutput::new("Interval").map_err(|e| MidiDeviceError::InitFailed(e.to_string()))?;
    let ports = midi_out.ports();
    if ports.is_empty() {
        return Err(MidiDeviceError::NoPorts);
    }
    let lower = name.to_lowercase();
    for port in &ports {
        let port_name = midi_out.port_name(port).unwrap_or_default();
        if port_name.to_lowercase().contains(&lower) {
            return midi_out
                .connect(port, "interval-play")
                .map_err(|e| MidiDeviceError::ConnectionFailed(format!("{}", e.kind())));
        }
    }
    Err(MidiDeviceError::PortNotFound(name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_midi_outputs_does_not_panic() {
        // This test just verifies the API doesn't panic.
        // Actual port availability depends on the system.
        let result = list_midi_outputs();
        // InitFailed is acceptable on machines without MIDI subsystem support
        assert!(result.is_ok() || matches!(result, Err(MidiDeviceError::InitFailed(_))));
    }

    #[test]
    fn test_connect_out_of_range() {
        let result = connect_midi_output(999);
        match result {
            Ok(_) => panic!("expected error, got Ok"),
            Err(MidiDeviceError::PortOutOfRange(idx)) => assert_eq!(idx, 999),
            Err(MidiDeviceError::NoPorts) => {} // acceptable if no ports
            Err(MidiDeviceError::InitFailed(_)) => {} // acceptable on headless/CI
            Err(other) => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn test_connect_by_name_not_found() {
        let result = connect_midi_output_by_name("zzz_nonexistent_port_zzz");
        // InitFailed is acceptable on machines without MIDI subsystem support
        assert!(result.is_err());
    }
}
