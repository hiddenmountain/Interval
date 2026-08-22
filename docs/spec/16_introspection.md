# 16. Introspection API

The `introspect` module in `interval-core` provides programmatic access to the language's knowledge base. All functions are WASM-safe.

### 16.1 Available Functions

| Function | Returns | Description |
|----------|---------|-------------|
| `all_chord_qualities()` | `Vec<ChordQualityInfo>` | All chord qualities with interval patterns |
| `complete_chord(prefix)` | `Vec<&str>` | Autocomplete chord suffixes matching prefix |
| `all_scales()` | `Vec<ScaleInfo>` | All scales/modes with interval patterns |
| `scale_pitches(root, mode)` | `Option<Vec<u8>>` | Pitch classes for a scale |
| `resolve_degree(root, mode, deg, oct, acc)` | `Option<u8>` | Scale-absolute degree to MIDI pitch |
| `all_directives()` | `Vec<DirectiveInfo>` | All `@` directives with descriptions |
| `all_transforms()` | `Vec<TransformInfo>` | All transforms with signatures |
| `complete_at_cursor(source, offset)` | `Vec<CompletionItem>` | Context-aware completions |

### 16.2 Usage (Rust)

```rust
use interval_core::introspect;

// List all chord qualities
for q in introspect::all_chord_qualities() {
    println!("{}: {:?}", q.name, q.intervals);
}

// Autocomplete "maj" → ["maj7", "maj9", "maj11"]
let completions = introspect::complete_chord("maj");

// C major scale pitches → [0, 2, 4, 5, 7, 9, 11]
let pitches = introspect::scale_pitches(0, "major").unwrap();

// Resolve scale degree 5 in C major, octave 4 → MIDI 67 (G4)
let midi = introspect::resolve_degree(0, "major", 5, 4, 0).unwrap();
```

### 16.3 Structured Harmony Timeline

```rust
pub fn harmony_timeline(
    index: &HarmonyIndex,
    bar_layout: &BarLayout,
    ppq: u32,
) -> Vec<HarmonyBarInfo>
```

Returns one `HarmonyBarInfo` per bar in the harmony timeline:

```rust
pub struct HarmonyBarInfo {
    pub bar: u32,              // 1-based
    pub chords: Vec<HarmonyChordInfo>,
}

pub struct HarmonyChordInfo {
    pub symbol: String,        // e.g. "Cmaj7"
    pub root: u8,              // MIDI pitch class 0-11
    pub intervals: Vec<u8>,    // Chord intervals from root
    pub roman_numeral: Option<String>,  // e.g. "iv7", "bVImaj7"
    pub beat_start: f64,       // Beat position within bar (0-based)
    pub beat_end: f64,
    pub tick_start: u64,
    pub tick_end: u64,
}
```

### 16.4 Structured Scale Timeline

```rust
pub fn scale_timeline_info(
    scale_timeline: &ScaleTimeline,
    total_bars: u32,
) -> Vec<ScaleBarInfo>
```

Returns one `ScaleBarInfo` per distinct scale change:

```rust
pub struct ScaleBarInfo {
    pub bar: u32,              // 1-based
    pub root: u8,              // Pitch class 0-11
    pub root_name: String,     // e.g. "C", "F#"
    pub mode: String,          // e.g. "major", "dorian"
    pub pitch_classes: Vec<u8>,
}
```

### 16.5 Step Pitch Resolution

```rust
pub fn resolve_step_pitches(
    token: &StepToken,
    chord: Option<&ChordContext>,
    scale_root: u8,
    mode: &str,
    octave: u8,
    voice: VoicingStrategy,
    inv: Inversion,
    prev_pitches: Option<&[u8]>,
) -> Option<Vec<u8>>
```

Resolves a single step token to its MIDI pitches given the current harmonic and
scale context. Returns `None` for rests and ties. Useful for hover/tooltip display
in an editor.

### 16.6 Cursor Context

```rust
pub fn get_context_at_cursor(
    source: &str,
    byte_offset: usize,
) -> CursorContext
```

Given a source string and byte offset, returns information about the block and
position at the cursor:

```rust
pub struct CursorContext {
    pub block_type: Option<BlockType>,
    pub block_name: Option<String>,
    pub step_index: Option<usize>,
    pub step_token: Option<String>,
    pub resolved_pitches: Option<Vec<u8>>,
    pub track_channel: Option<u8>,
    pub available_annotations: Vec<&'static str>,
    pub available_transforms: Vec<&'static str>,
    pub current_chord: Option<ChordInfo>,
    pub current_scale: Option<ScaleInfo>,
    pub resolved_pitch_name: Option<String>,
    pub harmony_bar_index: Option<usize>,
    pub pattern_params: Option<PatternBlockSummary>,
}

pub enum BlockType {
    Scale,
    Harmony,
    Pattern,
    Track,
    DrumMap,
}
```

### 16.7 Rich Cursor Context

```rust
pub fn get_rich_context_at_cursor(
    source: &str,
    byte_offset: usize,
    compile_output: Option<&CompileOutput>,
) -> CursorContext
```

Enriched version of `get_context_at_cursor` that accepts a pre-compiled `CompileOutput`
to populate chord, scale, and pattern parameter fields. The caller should cache the
compile output and pass it in for performance.

### 16.8 Unified Completion Provider

```rust
pub fn complete_at_cursor(source: &str, byte_offset: usize) -> Vec<CompletionItem>

pub struct CompletionItem {
    pub label: String,
    pub detail: Option<String>,
    pub kind: CompletionKind,
    pub insert_text: String,
    pub relevance: f32,
    pub category: String,
}

pub enum CompletionKind {
    ChordSymbol, ScaleName, Transform, Annotation,
    Directive, StepToken, PatternRef,
}
```

Context-aware completions based on cursor position:
- **Harmony body**: chord quality completions
- **Pattern body**: step tokens, annotations
- **Track block**: pattern references, transforms
- **Top-level**: directives, scale names

---

[← Back to Table of Contents](00_toc.md)
