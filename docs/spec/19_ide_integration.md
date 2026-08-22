# 19. IDE Integration API

Interval's `interval-core` crate exposes a set of APIs designed for embedding in
editors and the Syntax Tauri app. All APIs in this chapter are WASM-safe unless
noted otherwise.

## 19.1 Parse-Only Entry Point

```rust
pub fn parse_only(source: &str) -> CompileResult<Program>
```

Tokenizes and parses an Interval source string into a typed AST without compiling.
Returns `Program { header, blocks, span }`. Useful for syntax highlighting, cursor
context, and structural analysis.

`resolved_seed` is left as `None` — seed resolution requires OS APIs not available
in WASM. Set it on `program.header.resolved_seed` before calling `compile()` if
seeded transforms are needed.

## 19.2 Compile with AST

```rust
pub fn compile_with_ast(source: &str) -> CompileResult<CompileOutput>
```

Convenience wrapper: `parse_only()` → `compile()` → attaches the parsed `Program`
to `CompileOutput.program`. Returns the full compilation result with events, track
summaries, warnings, and the AST.

## 19.3 Per-Track Metadata

After compilation, `CompileOutput.tracks` contains a `Vec<TrackSummary>`:

```rust
pub struct TrackSummary {
    pub name: String,
    pub channel: u8,           // 0-indexed
    pub program: Option<u8>,
    pub follow: Option<String>,
    pub voice: VoicingStrategy,
    pub inv: Inversion,
    pub is_drum: bool,
    pub patterns: Vec<TrackPatternInstance>,
    pub span: Option<Span>,
}

pub struct TrackPatternInstance {
    pub pattern_name: String,
    pub start_tick: u64,
    pub end_tick: u64,
    pub start_bar: u32,
    pub end_bar: u32,
    pub transforms: Vec<String>,
}
```

## 19.4 AST Source Spans

All major AST structs carry `pub span: Option<Span>` where `Span` is
`{ start: usize, end: usize }` (byte offsets). Spans are populated by the parser
for source-originated nodes and `None` for synthetic nodes (created during pattern
resolution or transforms).

`StepLine` additionally has `pub token_spans: Vec<Option<Span>>` parallel to its
`tokens` field, and `pub span: Option<Span>` covering the entire step line
(populated by the parser).

All span fields are `#[serde(skip)]` and do not appear in serialized output.

## 19.5 MIDI Device API (Native Only)

The `interval-rt::midi_devices` module provides MIDI port enumeration and connection.
This is **not** WASM-safe — it requires OS-level MIDI access.

```rust
pub fn list_midi_outputs() -> Result<Vec<MidiPortInfo>, MidiDeviceError>
pub fn connect_midi_output(port_index: usize) -> Result<MidiOutputConnection, MidiDeviceError>
pub fn connect_midi_output_by_name(name: &str) -> Result<MidiOutputConnection, MidiDeviceError>
```

`connect_midi_output_by_name` performs case-insensitive substring matching against
port names.

## 19.6 Source Editing Helpers

The `edit` module in `interval-core` provides span-based text surgery functions.
All functions take source text, perform a targeted edit, re-parse to validate, and
return the modified source.

```rust
pub fn insert_step(source, pattern_name, index, token_text) -> Result<String, EditError>
pub fn remove_step(source, pattern_name, index) -> Result<String, EditError>
pub fn replace_step(source, pattern_name, index, new_text) -> Result<String, EditError>
pub fn set_annotation(source, pattern_name, step_index, annotation) -> Result<String, EditError>
pub fn add_transform(source, track_name, transform_text) -> Result<String, EditError>
pub fn set_track_param(source, track_name, param, value) -> Result<String, EditError>
pub fn set_header_param(source, param, value) -> Result<String, EditError>
```

The edit module uses AST spans from `parse_only()` to locate edit sites precisely.
Each function validates the result by re-parsing, returning `EditError::InvalidResult`
if the edit produces unparseable source.

## 19.7 Typical IDE Integration Flow

```
Source text
    |
    v
parse_only(source)          -> Program (AST with spans)
    |
    |-> get_context_at_cursor(source, offset) -> CursorContext
    |     (block type, name, annotations, transforms)
    |
    |-> complete_at_cursor(source, offset) -> Vec<CompletionItem>
    |     (context-aware completions)
    |
    |-> resolve_step_pitches(token, chord, ...) -> Option<Vec<u8>>
    |     (hover: show MIDI pitches for a token)
    |
    |-> edit::replace_step(source, ...) -> Result<String, EditError>
    |     (grid cell edit → source text mutation)
    |
    v
compile_with_ast(source)    -> CompileOutput
    |
    |-> .tracks              -> Vec<TrackSummary>
    |-> .events              -> EventStream (for playback/export)
    |-> .program             -> Some(Program) (AST)
    |
    |-> get_rich_context_at_cursor(source, offset, Some(&output))
    |     (enriched cursor context with chord/scale/pitch info)
    |
    |-> harmony_timeline(index, layout, ppq) -> Vec<HarmonyBarInfo>
    |-> scale_timeline_info(timeline, bars)  -> Vec<ScaleBarInfo>
```

---

[← Back to Table of Contents](00_toc.md)
