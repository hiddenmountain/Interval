//! Error types for the Interval compiler.
//!
//! All error types are `Clone + Send + Sync` for WASM compatibility.
//! Errors carry source spans for precise location reporting.
//! The core crate never renders errors to strings — that happens
//! in the CLI crate via `codespan-reporting`.

use serde::Serialize;

/// A byte-offset span in the source file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    /// Start byte offset (inclusive).
    pub start: usize,
    /// End byte offset (exclusive).
    pub end: usize,
}

impl Span {
    /// Create a new span from start and end byte offsets.
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// A compile error with source location and description.
#[derive(Debug, Clone, thiserror::Error, Serialize)]
pub enum CompileError {
    /// Pattern step count mismatch.
    #[error("pattern '{name}': declared steps={declared} but body has {actual} lines")]
    StepCountMismatch {
        /// Pattern name.
        name: String,
        /// Declared step count.
        declared: u32,
        /// Actual body line count.
        actual: u32,
        /// Source span.
        span: Span,
    },

    /// Tie at pattern start with no prior context.
    #[error("pattern '{name}' step 1: tie with no prior note")]
    TieWithNoPriorNote {
        /// Pattern name.
        name: String,
        /// Source span.
        span: Span,
    },

    /// Follow references undefined harmony block.
    #[error("track '{track}': harmony block '{harmony}' not defined")]
    UndefinedHarmonyBlock {
        /// Track name.
        track: String,
        /// Referenced harmony block name.
        harmony: String,
        /// Source span.
        span: Span,
    },

    /// Current chord token ($chord) used without follow= harmony reference.
    #[error("track '{name}': $chord requires follow= to reference a harmony block")]
    CurrentChordWithoutFollow {
        /// Track name.
        name: String,
        /// Source span.
        span: Span,
    },

    /// Drum track with follow directive.
    #[error("track '{name}': drum tracks cannot follow a harmony block")]
    DrumTrackWithFollow {
        /// Track name.
        name: String,
        /// Source span.
        span: Span,
    },

    /// Track has both play and steps.
    #[error("track '{name}': cannot have both play: and steps:")]
    PlayAndSteps {
        /// Track name.
        name: String,
        /// Source span.
        span: Span,
    },

    /// Track has neither play nor steps.
    #[error("track '{name}': must have either play: or steps:")]
    NeitherPlayNorSteps {
        /// Track name.
        name: String,
        /// Source span.
        span: Span,
    },

    /// Pattern composition with mismatched units.
    #[error("pattern expression: cannot compose patterns with different units")]
    UnitMismatch {
        /// Source span.
        span: Span,
    },

    /// Channel out of range.
    #[error("track '{name}': ch must be 1-16")]
    ChannelOutOfRange {
        /// Track name.
        name: String,
        /// Source span.
        span: Span,
    },

    /// Velocity out of range.
    #[error("{context}: vel must be 1-127")]
    VelocityOutOfRange {
        /// Context description.
        context: String,
        /// Source span.
        span: Span,
    },

    /// Gate out of range.
    #[error("{context}: gate must be 0.0-1.0")]
    GateOutOfRange {
        /// Context description.
        context: String,
        /// Source span.
        span: Span,
    },

    /// Inversion exceeds chord tone count.
    #[error("{context}: inversion {inv} exceeds chord tone count")]
    InversionExceedsChordTones {
        /// Context description.
        context: String,
        /// Requested inversion.
        inv: u8,
        /// Source span.
        span: Span,
    },

    /// Bar beat assignments don't sum to time signature numerator.
    #[error("harmony '{name}' bar {bar}: beat assignments sum to {actual}, expected {expected}")]
    BeatAssignmentMismatch {
        /// Harmony block name.
        name: String,
        /// Bar number.
        bar: u32,
        /// Actual sum.
        actual: u32,
        /// Expected sum (ts numerator).
        expected: u8,
        /// Source span.
        span: Span,
    },

    /// Undefined pattern reference.
    #[error("track '{track}': pattern '{pattern}' not defined")]
    UndefinedPattern {
        /// Track name.
        track: String,
        /// Referenced pattern name.
        pattern: String,
        /// Source span.
        span: Span,
    },

    /// Interleave step count mismatch.
    #[error("interleave: pattern step counts must match")]
    InterleaveMismatch {
        /// Source span.
        span: Span,
    },

    /// Forward reference to pattern.
    #[error("pattern '{name}': forward references not permitted")]
    ForwardReference {
        /// Pattern name.
        name: String,
        /// Source span.
        span: Span,
    },

    /// Section bar numbers not strictly increasing.
    #[error("harmony '{name}': section bar numbers must be strictly increasing")]
    SectionBarNotIncreasing {
        /// Harmony block name.
        name: String,
        /// Source span.
        span: Span,
    },

    /// Section bar exceeds total bar count.
    #[error("harmony '{name}': section bar={bar} exceeds total bar count")]
    SectionBarExceedsTotal {
        /// Harmony block name.
        name: String,
        /// Bar number.
        bar: u32,
        /// Source span.
        span: Span,
    },

    /// Deprecated `|` used as transform pipe operator (v0.5: use `->` instead).
    #[error(
        "`|` is no longer the transform pipe in v0.5 — use `->` instead (e.g., `pat -> reverse`)"
    )]
    DeprecatedPipeOperator {
        /// Source span.
        span: Span,
    },

    /// Deprecated `|` inside `{{}}` variant pool (v0.5: use `,` instead).
    #[error("`|` inside `{{}}` is no longer valid in v0.5 — use `,` to separate variants (e.g., `{{a, b, c}}`)")]
    DeprecatedVariantPipe {
        /// Source span.
        span: Span,
    },

    /// Deprecated `$_` current-chord token (v0.5: use `$chord` instead).
    #[error("`$_` was renamed to `$chord` in v0.5")]
    DeprecatedCurrentChordToken {
        /// Source span.
        span: Span,
    },

    /// Chord ordinal token (%n) used without follow= harmony reference.
    #[error("track '{name}': %n requires follow= to reference a harmony block")]
    ChordOrdinalWithoutFollow {
        /// Track name.
        name: String,
        /// Source span.
        span: Span,
    },

    /// Multiple @harmony blocks exist but at least one is unnamed; all must be named for
    /// explicit follow= references.
    #[error("multiple @harmony blocks require names — add a name after @harmony")]
    MultipleHarmonyBlocksRequireNames {
        /// Source span.
        span: Span,
    },

    /// `@tempo` was removed in v0.5 — use `@bpm` block/inline form.
    #[error("@tempo was removed in v0.5 — use @bpm block or inline form instead")]
    DeprecatedTempo {
        /// Source span.
        span: Span,
    },

    /// Generic parse error.
    #[error("{message}")]
    ParseError {
        /// Error message.
        message: String,
        /// Source span.
        span: Span,
    },
}

/// A compile warning with source location.
#[derive(Debug, Clone, thiserror::Error, Serialize)]
pub enum CompileWarning {
    /// Degree token with no follow directive.
    #[error("track '{track}': degree token without follow= — defaulting to C major")]
    DegreeWithNoFollow {
        /// Track name.
        track: String,
        /// Source span.
        span: Span,
    },

    /// Note clamped to MIDI range.
    #[error("track '{track}' step {step}: note {original} clamped to MIDI range 0-127")]
    NoteClamped {
        /// Track name.
        track: String,
        /// Step number.
        step: u32,
        /// Original note value.
        original: i32,
        /// Source span.
        span: Span,
    },

    /// play=true without ch= on harmony block.
    #[error("@harmony '{name}': play=true requires ch= — defaulting to channel 1")]
    PlayWithoutChannel {
        /// Harmony block name.
        name: String,
        /// Source span.
        span: Span,
    },

    /// `section:` inside `@harmony` is deprecated in v0.5.
    #[error(
        "`section:` inside @harmony is deprecated in v0.5 — use `@scale` timeline form instead"
    )]
    DeprecatedSection {
        /// Source span.
        span: Span,
    },

    /// `[prob:N]` annotation on a rest step — has no effect since rests never play.
    #[error("track '{track}': [prob:N] on a rest has no effect — rests are always silent")]
    ProbOnRest {
        /// Track name.
        track: String,
        /// Source span.
        span: Span,
    },

    /// `[prob:0.0]` on a note step — step can never play.
    #[error("track '{track}': [prob:0.0] — step can never play")]
    ProbZeroNeverPlays {
        /// Track name.
        track: String,
        /// Source span.
        span: Span,
    },

    /// `[glide]` on the first note of a pattern — no prior pitch to glide from, ignored.
    #[error("track '{track}': [glide] on first note has no prior pitch to glide from — ignored")]
    GlideOnFirstNote {
        /// Track name.
        track: String,
        /// Source span.
        span: Span,
    },

    /// `[glide]` on a tied note — glide has no effect on ties, ignored.
    #[error("track '{track}': [glide] on a tied note has no effect — ignored")]
    GlideOnTiedNote {
        /// Track name.
        track: String,
        /// Source span.
        span: Span,
    },

    /// `[glide]` on a drum track — portamento is not meaningful on drums, ignored.
    #[error("track '{track}': [glide] annotation ignored on drum track")]
    GlideOnDrumTrack {
        /// Track name.
        track: String,
        /// Source span.
        span: Span,
    },
}

/// Result type for compilation operations.
pub type CompileResult<T> = Result<T, CompileError>;
