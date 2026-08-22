//! Interval compiler core.
//!
//! This crate contains the full language frontend and middle-end: lexer, parser,
//! AST, harmony index, pattern transforms, compiler, and event stream emission.
//! It has no I/O dependencies and compiles to `wasm32-unknown-unknown`.

/// Abstract syntax tree types for all Interval constructs.
pub mod ast;
/// Full compilation pipeline: resolves patterns against harmony and emits events.
pub mod compiler;
/// Source editing helpers: span-based text surgery on Interval source files.
pub mod edit;
/// Error types with source spans for diagnostic reporting.
pub mod error;
/// Event stream types emitted by the compiler (MIDI events, markers, meta).
pub mod event;
/// Chord parser, harmony block parser, and interval-tree harmony index.
pub mod harmony;
/// Introspection API: chord qualities, scales, directives, transforms.
pub mod introspect;
/// Logos-based lexer and token definitions.
pub mod lexer;
/// Recursive-descent parser for all Interval blocks.
pub mod parser;
/// Pattern composition resolver (repeat, concat, transforms, validation).
pub mod pattern;
/// Step-level transforms (transpose, shift_oct, retrograde, shuffle).
pub mod transform;
/// Voicing strategies, inversion, slash bass, degree resolution, and scale snapping.
pub mod voicing;

// Re-export key entry points for convenience.
pub use parser::parse_only;
