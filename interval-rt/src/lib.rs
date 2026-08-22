//! Interval real-time MIDI scheduler.
//!
//! Consumes an `EventStream` and dispatches events to a MIDI output port
//! in real time. Supports play/pause/stop transport, active note tracking,
//! and hot-swap at bar boundaries via `arc-swap`.
//!
//! This crate is native-only (depends on `midir` for MIDI I/O).

pub mod hotswap;
pub mod midi_devices;
pub mod playback_state;
pub mod scheduler;
