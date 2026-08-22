//! Abstract syntax tree types for Interval.
//!
//! Defines the typed representation of a parsed Interval file:
//! - `Program`: top-level container (header + blocks)
//! - `GlobalHeader`: ppq, bpm, time signature, title, seed
//! - `HarmonyBlock`: named harmony timeline with bar grid and sections
//! - `PatternBlock`: named pattern with step body or composition expression
//! - `TrackBlock`: track declaration with play/steps directive
//! - `DrumMapBlock`: named drum map with identifier-to-MIDI mappings
//! - `Step` variants: degree, absolute pitch, MIDI number, chord, rest, tie
//! - `Annotation`: per-step overrides (vel, gate, shift, CC, etc.)
//! - `Subdivision`: nested bracket groups
//! - `VariantPool`: `{a | b | c}` alternatives

use serde::Serialize;

use crate::error::Span;

/// Top-level representation of a parsed Interval file.
#[derive(Debug, Clone, Serialize)]
pub struct Program {
    /// Global header directives.
    pub header: GlobalHeader,
    /// All blocks in declaration order.
    pub blocks: Vec<Block>,
    /// Source span covering the entire program.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// Setting for the `@bars` directive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum BarsSetting {
    /// Fill all bare pattern references to this many bars.
    Count(u32),
    /// Explicitly disabled (`@bars off`).
    Off,
}

/// Global header with file-wide constants.
#[derive(Debug, Clone, Serialize)]
pub struct GlobalHeader {
    /// Pulses per quarter note (default: 480).
    pub ppq: u32,
    /// Tempo in beats per minute (default: 120.0).
    pub bpm: f64,
    /// Time signature numerator (default: 4).
    pub ts_numerator: u8,
    /// Time signature denominator (default: 4).
    pub ts_denominator: u8,
    /// Optional title string.
    pub title: Option<String>,
    /// Global seed for seeded operations (None = not specified, resolved by CLI/RT).
    /// When `Some`, the seed is embedded as a TextMeta event in the output.
    pub seed: Option<u64>,
    /// Resolved seed for the compiler to use in random transforms (humanize, vary, etc.).
    /// Set by the CLI/RT layer. Not embedded in output.
    pub resolved_seed: Option<u64>,
    /// BPM timeline (populated for `@bpm` timeline form; takes precedence over `bpm`).
    pub bpm_block: Option<BpmBlock>,
    /// TS timeline (populated for `@ts` timeline form; first entry also sets ts_numerator/ts_denominator).
    pub ts_block: Option<TsBlock>,
    /// Scale timeline (populated for `@scale` timeline form).
    pub scale_block: Option<ScaleBlock>,
    /// Global bar count for automatic pattern fill (`@bars N` / `@bars off`).
    pub bars: Option<BarsSetting>,
    /// Source span covering the header directives.
    #[serde(skip)]
    pub span: Option<Span>,
}

impl Default for GlobalHeader {
    fn default() -> Self {
        Self {
            ppq: 480,
            bpm: 120.0,
            ts_numerator: 4,
            ts_denominator: 4,
            title: None,
            seed: None,
            resolved_seed: None,
            bpm_block: None,
            ts_block: None,
            scale_block: None,
            bars: None,
            span: None,
        }
    }
}

/// A BPM timeline entry (for `@bpm` block/inline form).
#[derive(Debug, Clone, Serialize)]
pub struct BpmEntry {
    /// Target BPM for this segment.
    pub bpm: f64,
    /// Number of bars this entry spans (None = last segment, holds forever).
    pub bars: Option<u32>,
    /// Optional ramp curve name (None = instant). If present, BPM ramps from
    /// the previous entry's BPM to this entry's BPM over `bars` bars.
    pub ramp: Option<String>,
    /// Source span.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// A BPM timeline block (new `@bpm` timeline form).
#[derive(Debug, Clone, Serialize)]
pub struct BpmBlock {
    /// Ordered BPM entries.
    pub entries: Vec<BpmEntry>,
    /// Source span.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// A time-signature timeline entry (for `@ts` timeline form).
#[derive(Debug, Clone, Serialize)]
pub struct TsEntry {
    /// Time signature numerator.
    pub numerator: u8,
    /// Time signature denominator (must be power of 2).
    pub denominator: u8,
    /// Number of bars this time signature applies to (None = holds forever).
    pub bars: Option<u32>,
    /// Source span.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// A time-signature timeline block (new `@ts` timeline form).
#[derive(Debug, Clone, Serialize)]
pub struct TsBlock {
    /// Ordered time-signature entries.
    pub entries: Vec<TsEntry>,
    /// Source span.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// A scale timeline entry (for `@scale` timeline form).
#[derive(Debug, Clone, Serialize)]
pub struct ScaleEntry {
    /// Root pitch class (0=C … 11=B). None = not specified — inherit from previous entry.
    pub root: Option<u8>,
    /// Scale/mode name. None = not specified — inherit from previous entry.
    pub mode: Option<String>,
    /// Number of bars this scale context applies to (None = holds forever).
    pub bars: Option<u32>,
    /// Source span.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// A scale timeline block (new `@scale` timeline form).
#[derive(Debug, Clone, Serialize)]
pub struct ScaleBlock {
    /// Ordered scale entries.
    pub entries: Vec<ScaleEntry>,
    /// Source span.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// Global tonal context from `@scale` block.
#[derive(Debug, Clone, Serialize)]
pub struct TonalContext {
    /// Root pitch class (0=C, 1=C#, ... 11=B). None = inherit C default.
    pub root: Option<u8>,
    /// Scale/mode name (e.g., "major", "dorian").
    pub mode: String,
    /// Source span.
    #[serde(skip)]
    pub span: Option<Span>,
}

impl Default for TonalContext {
    fn default() -> Self {
        Self {
            root: Some(0), // C
            mode: "major".to_string(),
            span: None,
        }
    }
}

/// A top-level block declaration.
#[derive(Debug, Clone, Serialize)]
pub enum Block {
    /// `@scale` block — global tonal context (scalar form).
    Scale(TonalContext),
    /// `@scale` timeline block.
    ScaleTimeline(ScaleBlock),
    /// `@harmony` block.
    Harmony(HarmonyBlock),
    /// `@pattern` block.
    Pattern(PatternBlock),
    /// `@track` block.
    Track(TrackBlock),
    /// `@drummap` block.
    DrumMap(DrumMapBlock),
    /// `@tempo` block — per-bar tempo timeline (DEPRECATED in v0.5).
    Tempo(TempoBlock),
    /// `@bpm` timeline block (new v0.5 form).
    BpmTimeline(BpmBlock),
    /// `@ts` timeline block (new v0.5 form).
    TsTimeline(TsBlock),
}

/// A tempo timeline block.
#[derive(Debug, Clone, Serialize)]
pub struct TempoBlock {
    /// Per-bar tempo entries.
    pub entries: Vec<TempoEntry>,
}

/// A single bar's tempo specification.
#[derive(Debug, Clone, Serialize)]
pub enum TempoEntry {
    /// Constant BPM for this bar.
    Constant(f64),
    /// Linear ramp from start BPM to end BPM over this bar.
    Ramp { start: f64, end: f64 },
}

/// A named harmony timeline block.
#[derive(Debug, Clone, Serialize)]
pub struct HarmonyBlock {
    /// Block name (identifier after `@harmony`). `None` = unnamed (v0.5).
    /// When unnamed, the block can be auto-followed by tracks in single-block files.
    pub name: Option<String>,
    /// Whether to emit voiced MIDI chords.
    pub play: bool,
    /// MIDI channel (1-16, user-facing; required if play=true). The compiler
    /// subtracts 1 to produce 0-indexed channels in the event stream.
    pub channel: Option<u8>,
    /// GM program number.
    pub program: Option<u8>,
    /// Voicing strategy.
    pub voice: VoicingStrategy,
    /// Base octave for chord voicing.
    pub octave: u8,
    /// Velocity for chord notes.
    pub velocity: u8,
    /// Block-level inversion default (lowest priority: harmony < track < step).
    pub inv: Inversion,
    /// Bar definitions in order.
    pub bars: Vec<Bar>,
    /// Section modulation directives.
    pub sections: Vec<Section>,
    /// Source span covering the entire harmony block.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// A single bar in the harmony timeline.
#[derive(Debug, Clone, Serialize)]
pub struct Bar {
    /// Chord entries in this bar.
    pub chords: Vec<BarChord>,
    /// Optional step-level subdivision override.
    pub steps: Option<Vec<ChordSymbol>>,
    /// Source span.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// A chord within a bar, with optional beat duration.
#[derive(Debug, Clone, Serialize)]
pub struct BarChord {
    /// The chord symbol.
    pub chord: ChordSymbol,
    /// Explicit beat count (None = even distribution).
    pub beats: Option<u8>,
    /// Source span.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// Stores the raw Roman numeral degree so the compiler can re-resolve the chord root
/// against the correct `@scale` timeline entry for each bar.
///
/// Without this, Roman numeral roots would be baked in at parse time using only the
/// *first* scale timeline entry, causing later entries to be ignored.
#[derive(Debug, Clone, Serialize)]
pub struct RomanNumeralDegree {
    /// 0-based degree index (0=I … 6=VII).
    pub degree_idx: u8,
    /// Accidental: -1=flat, 0=natural, +1=sharp.
    pub accidental: i8,
}

/// A parsed chord symbol (root + quality + alterations + optional slash bass).
#[derive(Debug, Clone, Serialize)]
pub struct ChordSymbol {
    /// Root pitch class (0=C, 1=C#, ... 11=B).
    /// For Roman numeral chords, this is the root resolved at *parse time* against the
    /// first scale entry and serves as a fallback; the compiler re-resolves via `roman`.
    pub root: u8,
    /// Chord quality intervals (semitones from root).
    pub intervals: Vec<u8>,
    /// Optional slash bass pitch class.
    pub slash_bass: Option<u8>,
    /// If this chord was specified as a Roman numeral, stores the raw degree so the
    /// compiler can re-resolve the root bar-by-bar against the `@scale` timeline.
    /// `None` for letter-based chords (e.g. "Cmaj7") — their root is absolute.
    pub roman: Option<RomanNumeralDegree>,
}

/// A section modulation directive within a harmony block.
#[derive(Debug, Clone, Serialize)]
pub struct Section {
    /// Bar number where this section takes effect (1-indexed).
    pub bar: u32,
    /// New mode (if changing).
    pub mode: Option<String>,
    /// New root pitch class (if changing key).
    pub root: Option<u8>,
    /// Source span.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// A named pattern block.
#[derive(Debug, Clone, Serialize)]
pub struct PatternBlock {
    /// Pattern name.
    pub name: String,
    /// Declared step count.
    pub steps: u32,
    /// Step unit as a fraction (numerator, denominator).
    pub unit: (u32, u32),
    /// Default velocity.
    pub velocity: u8,
    /// Default gate ratio.
    pub gate: f64,
    /// Default octave.
    pub octave: u8,
    /// Baked-in transforms (applied at resolution time).
    pub transforms: Vec<TransformCall>,
    /// Pattern body: either step lines or a composition expression.
    pub body: PatternBody,
    /// Source span covering the entire pattern block.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// Pattern body content.
#[derive(Debug, Clone, Serialize)]
pub enum PatternBody {
    /// Inline step lines.
    Steps(Vec<StepLine>),
    /// Composition expression (assignment).
    Expression(PatternExpr),
}

/// A single step line in a pattern body.
#[derive(Debug, Clone, Serialize)]
pub struct StepLine {
    /// The tokens on this line (simultaneous notes separated by `+`).
    pub tokens: Vec<StepToken>,
    /// Per-token source spans (parallel to `tokens`).
    #[serde(skip)]
    pub token_spans: Vec<Option<Span>>,
    /// Source span covering the entire step line.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// A step token — the atomic unit within a step line.
#[derive(Debug, Clone, Serialize)]
pub enum StepToken {
    /// Scale degree with optional accidental and octave displacement.
    Degree {
        /// Scale degree number (1-13).
        degree: u8,
        /// Accidental: -1 = flat, 0 = natural, 1 = sharp.
        accidental: i8,
        /// Optional octave override.
        octave: Option<u8>,
        /// Step annotations.
        annotations: Vec<Annotation>,
    },
    /// Absolute pitch (e.g., C4, D#5).
    AbsolutePitch {
        /// MIDI note number.
        midi_note: u8,
        /// Step annotations.
        annotations: Vec<Annotation>,
    },
    /// Direct MIDI note number (e.g., n60).
    MidiNumber {
        /// MIDI note number (0-127).
        note: u8,
        /// Step annotations.
        annotations: Vec<Annotation>,
    },
    /// Chord symbol in step context (e.g., $Cmaj7).
    ChordStep {
        /// The chord symbol.
        chord: ChordSymbol,
        /// Step annotations.
        annotations: Vec<Annotation>,
    },
    /// Rest — silence for one step.
    Rest,
    /// Tie — extend previous note by one step.
    Tie,
    /// Subdivision bracket — divides parent step among contained tokens.
    Subdivision {
        /// Tokens within the subdivision.
        tokens: Vec<StepToken>,
    },
    /// Variant pool — alternatives selected by `vary()` transform.
    Variant {
        /// Alternative step tokens (first is default).
        alternatives: Vec<Vec<StepToken>>,
    },
    /// Drum hit by name (resolved via drummap).
    DrumHit {
        /// Identifier name from drummap.
        name: String,
        /// Step annotations.
        annotations: Vec<Annotation>,
    },
    /// Current harmony chord (`$chord`). Resolved at compile time via harmony index.
    CurrentChord {
        /// Step annotations.
        annotations: Vec<Annotation>,
    },
    /// Chord ordinal (`%n`) — Nth chord tone in ordinal order.
    ///
    /// `%1` = root, `%2` = 3rd (second tone), `%3` = 5th (third tone), etc.
    /// Ordinals beyond the chord tone count wrap with an octave shift:
    /// `%5` over a 4-note chord gives the root + one octave.
    ChordOrdinal {
        /// 1-based ordinal position.
        degree: u32,
        /// Forced octave override (from `%1/4` notation). None = use default + ordinal shift.
        octave: Option<u8>,
        /// Step annotations.
        annotations: Vec<Annotation>,
    },
}

/// A step annotation (per-step override).
#[derive(Debug, Clone, Serialize)]
pub enum Annotation {
    /// Velocity override.
    Vel(u8),
    /// Gate ratio override.
    Gate(f64),
    /// Explicit duration as fraction (numerator, denominator).
    Dur(u32, u32),
    /// Microtiming shift.
    Shift(TimingValue),
    /// Lane event timing shift.
    LShift(TimingValue),
    /// Octave override.
    Oct(u8),
    /// CC11 expression (static or ramp).
    Expr(CcValue),
    /// CC1 modulation (static or ramp).
    Dyn(CcValue),
    /// CC64 sustain pedal.
    Sus(u8),
    /// CC10 pan (static or ramp).
    Pan(CcValue),
    /// CC7 volume (static or ramp).
    Vol(CcValue),
    /// Pitch bend.
    PitchBend(i16),
    /// Channel aftertouch.
    Aftertouch(u8),
    /// Arbitrary CC number (static or ramp).
    Cc(u8, CcValue),
    /// Conditional step playback.
    Condition(StepCondition),
    /// Ratchet: repeat the note N times within the step duration.
    Ratch(u32),
    /// Ratchet velocity decay factor (default 1.0 = no decay).
    RatchDecay(f64),
    /// Probability of playing this step (0.0–1.0). Checked per-step at emission.
    Prob(f64),
    /// Portamento/glide to this note (None = full duration, Some(f) = fractional).
    Glide(Option<f64>),
}

/// A conditional step annotation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum StepCondition {
    /// Play on every Nth loop (shorthand for `cond:1:N`).
    Every(u32),
    /// Play on the Xth iteration of every Y loops.
    Cond(u32, u32),
    /// Play on the first loop only.
    Once,
    /// Play only if the previous conditional step played.
    Pre,
}

/// A CC value: either static or a ramp.
#[derive(Debug, Clone, Serialize)]
pub enum CcValue {
    /// Static value.
    Static(u8),
    /// Ramp from start to end, interpolated over step duration.
    Ramp { start: u8, end: u8 },
}

/// A timing value in one of three formats.
#[derive(Debug, Clone, Serialize)]
pub enum TimingValue {
    /// Percent of step duration.
    Percent(f64),
    /// Fraction of a whole note (numerator, denominator).
    Fraction(i32, u32),
    /// Absolute milliseconds.
    Milliseconds(f64),
}

impl std::fmt::Display for TimingValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimingValue::Percent(p) => write!(f, "{}%", p),
            TimingValue::Fraction(n, d) => write!(f, "{}/{}", n, d),
            TimingValue::Milliseconds(ms) => write!(f, "{}ms", ms),
        }
    }
}

/// Voicing strategy for multi-note steps.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum VoicingStrategy {
    /// All tones within one octave above bass.
    #[default]
    Close,
    /// Spread across two octaves.
    Open,
    /// Second-highest note dropped an octave.
    Drop2,
    /// Root + 3rd + 7th only.
    Shell,
    /// Root + 3rd + 5th only.
    Triad,
    /// Third-highest note dropped an octave (big band trombones).
    Drop3,
    /// Omit the root entirely (jazz piano).
    Rootless,
}

/// Inversion setting for voicing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Inversion {
    /// Fixed inversion (0=root, 1=first, 2=second, 3=third).
    Fixed(u8),
    /// Automatic voice-leading optimization.
    Auto,
}

impl Default for Inversion {
    fn default() -> Self {
        Self::Fixed(0)
    }
}

/// A pattern composition expression.
#[derive(Debug, Clone, Serialize)]
pub enum PatternExpr {
    /// Reference to a named pattern, with optional per-reference rate.
    Ref { name: String, rate: Option<f64> },
    /// Repetition with hard boundary.
    Repeat {
        pattern: Box<PatternExpr>,
        count: u32,
    },
    /// Repetition with soft tie boundary.
    RepeatSoft {
        pattern: Box<PatternExpr>,
        count: u32,
    },
    /// Concatenation with hard boundary.
    Concat {
        left: Box<PatternExpr>,
        right: Box<PatternExpr>,
    },
    /// Concatenation with soft tie boundary.
    ConcatSoft {
        left: Box<PatternExpr>,
        right: Box<PatternExpr>,
    },
    /// Transform application.
    Transform {
        pattern: Box<PatternExpr>,
        transform: TransformCall,
    },
}

/// A transform function call.
#[derive(Debug, Clone, Serialize)]
pub enum TransformCall {
    /// Reverse step order, recalculate ties.
    Reverse,
    /// Invert intervals around first pitch.
    Invert,
    /// Alias for reverse | invert.
    Retrograde,
    /// Cyclic rotation by n steps.
    Rotate(i32),
    /// Multiply all durations by factor (numerator, denominator).
    Stretch(u32, u32),
    /// Alias for stretch(1/n).
    Compress(u32, u32),
    /// Transpose absolute pitches by n semitones.
    Transpose(i32),
    /// Shift octave for all notes.
    ShiftOct(i32),
    /// Retain only specified step indices (1-indexed).
    Subset(Vec<u32>),
    /// Alternate steps with another pattern.
    Interleave(String),
    /// Concatenate with own reverse.
    Mirror,
    /// Apply humanization (timing deviation, intensity).
    Humanize(TimingValue, f64),
    /// Select variant pool alternatives with given probability.
    Vary(f64),
    /// Swing transform: (ratio, unit_numerator, unit_denominator).
    Swing(f64, u32, u32),
    /// Rubato: time envelope (depth, curve).
    Rubato(f64, ExpressiveCurve),
    /// Ritardando: gradual tempo decrease (depth).
    Ritardando(f64),
    /// Accelerando: gradual tempo increase (depth).
    Accelerando(f64),
    /// Agogic accent: duration emphasis on specified steps (1-indexed).
    Agogic(Vec<u32>),
    /// Breathe: micro-pause at step position (position, duration as TimingValue).
    Breathe(u32, TimingValue),
    /// Swell: velocity envelope with timing expansion (peak, curve).
    Swell(f64, ExpressiveCurve),
    /// Phrase: composite rubato + agogic + swell (tension, release).
    Phrase(f64, f64),
    /// Evolve: shift register mutation (toggle probability 0.0-1.0).
    Evolve(f64),
    /// Euclidean gate: suppress steps not in euclidean(pulses, steps) pattern.
    EuclidGate(u32, u32),
    /// Echo: repeat notes (rate as fraction num/den, repeats, velocity decay).
    Echo(u32, u32, u32, f64),
    /// Velocity curve: shape velocity across steps (wave, min, max, repeat).
    VelCurve(WaveShape, u8, u8, u32),
    /// Gate curve: shape gate across steps (wave, min, max as f64 pair, repeat).
    GateCurve(WaveShape, f64, f64, u32),
    /// Scale lock: snap pitches to a scale (scale name, root pitch class, snap mode).
    ScaleLock(Option<String>, Option<u8>, SnapMode),
    /// Arp: explode multi-note steps into arpeggiated sequences at emission time.
    Arp {
        /// Arp direction pattern.
        pattern: ArpPattern,
        /// Onset-to-onset spacing as a fraction (numerator, denominator).
        rate: (u32, u32),
        /// Number of octave layers (1 = single octave, 2 = two octaves, etc.).
        octaves: u32,
    },
}

/// Arp pattern direction.
#[derive(Debug, Clone, Serialize)]
pub enum ArpPattern {
    /// Ascending (root → top).
    Up,
    /// Descending (top → root).
    Down,
    /// Ascending then descending, no repeat at top or bottom.
    UpDown,
    /// Random order (seeded).
    Random,
}

/// Curve function for expressive transforms.
#[derive(Debug, Clone, Serialize)]
pub enum ExpressiveCurve {
    EaseIn,
    EaseOut,
    EaseInOut,
    Arch,
}

impl std::fmt::Display for ExpressiveCurve {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpressiveCurve::EaseIn => write!(f, "ease_in"),
            ExpressiveCurve::EaseOut => write!(f, "ease_out"),
            ExpressiveCurve::EaseInOut => write!(f, "ease_in_out"),
            ExpressiveCurve::Arch => write!(f, "arch"),
        }
    }
}

/// Wave shape for velocity/gate curve transforms.
#[derive(Debug, Clone, Serialize)]
pub enum WaveShape {
    /// Sinusoidal wave (0→1→0).
    Sine,
    /// Triangle wave (0→1→0 linear).
    Tri,
    /// Ramp (0→1 linear).
    Ramp,
    /// Square wave (0 or 1).
    Square,
    /// Seeded random values.
    Random,
}

/// Snap mode for `scale_lock` transform.
#[derive(Debug, Clone, Serialize)]
pub enum SnapMode {
    /// Snap to nearest lower in-scale pitch.
    Down,
    /// Snap to nearest higher in-scale pitch.
    Up,
    /// Remove out-of-scale notes entirely.
    Filter,
}

/// A track block declaration.
#[derive(Debug, Clone, Serialize)]
pub struct TrackBlock {
    /// Track name.
    pub name: String,
    /// MIDI channel (1-16, user-facing). The compiler subtracts 1 to produce
    /// 0-indexed channels in the event stream.
    pub channel: u8,
    /// GM program number.
    pub program: Option<u8>,
    /// Step unit for inline steps (numerator, denominator).
    pub unit: Option<(u32, u32)>,
    /// Default octave.
    pub octave: u8,
    /// Default velocity.
    pub velocity: u8,
    /// Default gate ratio.
    pub gate: f64,
    /// Global microtiming shift for note events.
    pub shift: Option<TimingValue>,
    /// Global microtiming shift for lane/CC events.
    pub lshift: Option<TimingValue>,
    /// Name of harmony block to follow.
    pub follow: Option<String>,
    /// Voicing strategy.
    pub voice: VoicingStrategy,
    /// Inversion setting.
    pub inv: Inversion,
    /// Per-track seed override.
    pub seed: Option<u64>,
    /// Per-track mode override for degree resolution.
    pub mode: Option<String>,
    /// Playback rate multiplier (1.0 = normal, 2.0 = double speed).
    pub rate: Option<f64>,
    /// Swing ratio (0.5 = straight, 0.67 = triplet).
    pub swing: Option<f64>,
    /// Swing unit as fraction (numerator, denominator), e.g. (1, 8) for 1/8.
    pub swing_unit: Option<(u32, u32)>,
    /// Start bar (1-indexed). Events are offset by `(start - 1) * ticks_per_bar`.
    pub start: Option<u32>,
    /// Whether this is a drum track.
    pub is_drum: bool,
    /// Drummap reference name.
    pub drummap: Option<String>,
    /// Track content: play directive or inline steps.
    pub content: TrackContent,
    /// Source span covering the entire track block.
    #[serde(skip)]
    pub span: Option<Span>,
}

/// Track content — either a play expression or inline steps.
#[derive(Debug, Clone, Serialize)]
pub enum TrackContent {
    /// `play:` directive with pattern expression.
    Play(PatternExpr),
    /// `steps:` inline step block.
    Steps(Vec<StepLine>),
}

/// A drum map block.
#[derive(Debug, Clone, Serialize)]
pub struct DrumMapBlock {
    /// Optional name (None = default map).
    pub name: Option<String>,
    /// Identifier to MIDI note mappings.
    pub mappings: Vec<(String, u8)>,
    /// Source span covering the entire drummap block.
    #[serde(skip)]
    pub span: Option<Span>,
}
