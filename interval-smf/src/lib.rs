//! Interval SMF renderer.
//!
//! Converts the compiler's `EventStream` into a Type 1 Standard MIDI File.
//! Writes to any `std::io::Write` implementor. Strips `BarMarker` events,
//! applies correct intra-tick event ordering, and encodes delta times.
//!
//! This crate has no `std::fs` dependency and compiles to `wasm32-unknown-unknown`.

pub mod renderer;
