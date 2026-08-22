//! Introspection API for Interval.
//!
//! Provides programmatic access to the language's chord qualities, scales,
//! directives, and transforms. Designed for IDE integration, autocomplete,
//! and educational tools. WASM-safe (no I/O dependencies).
//!
//! v0.8 additions: structured harmony/scale timelines, cursor context,
//! step pitch resolution, and compile_with_ast.

use crate::ast::{Block, ChordSymbol, Inversion, StepToken, VoicingStrategy};
use crate::compiler;
use crate::error::CompileResult;
use crate::harmony::{self, ChordContext, HarmonyIndex, ScaleTimeline, MODES};
use crate::voicing;

/// A chord quality with its name and interval pattern.
#[derive(Debug, Clone)]
pub struct ChordQualityInfo {
    /// Quality suffix (e.g., "maj7", "m7", "dim").
    pub name: &'static str,
    /// Intervals from root in semitones.
    pub intervals: &'static [u8],
}

/// All supported chord qualities.
pub fn all_chord_qualities() -> Vec<ChordQualityInfo> {
    vec![
        ChordQualityInfo {
            name: "maj13",
            intervals: &[0, 4, 7, 11, 14, 17, 21],
        },
        ChordQualityInfo {
            name: "maj11",
            intervals: &[0, 4, 7, 11, 14, 17],
        },
        ChordQualityInfo {
            name: "maj9",
            intervals: &[0, 4, 7, 11, 14],
        },
        ChordQualityInfo {
            name: "maj7",
            intervals: &[0, 4, 7, 11],
        },
        ChordQualityInfo {
            name: "min9",
            intervals: &[0, 3, 7, 10, 14],
        },
        ChordQualityInfo {
            name: "min7",
            intervals: &[0, 3, 7, 10],
        },
        ChordQualityInfo {
            name: "min",
            intervals: &[0, 3, 7],
        },
        ChordQualityInfo {
            name: "mMaj9",
            intervals: &[0, 3, 7, 11, 14],
        },
        ChordQualityInfo {
            name: "mMaj7",
            intervals: &[0, 3, 7, 11],
        },
        ChordQualityInfo {
            name: "mM9",
            intervals: &[0, 3, 7, 11, 14],
        },
        ChordQualityInfo {
            name: "mM7",
            intervals: &[0, 3, 7, 11],
        },
        ChordQualityInfo {
            name: "m7b5",
            intervals: &[0, 3, 6, 10],
        },
        ChordQualityInfo {
            name: "m(add9)",
            intervals: &[0, 3, 7, 14],
        },
        ChordQualityInfo {
            name: "madd9",
            intervals: &[0, 3, 7, 14],
        },
        ChordQualityInfo {
            name: "m13",
            intervals: &[0, 3, 7, 10, 14, 17, 21],
        },
        ChordQualityInfo {
            name: "m11",
            intervals: &[0, 3, 7, 10, 14, 17],
        },
        ChordQualityInfo {
            name: "m9",
            intervals: &[0, 3, 7, 10, 14],
        },
        ChordQualityInfo {
            name: "m7",
            intervals: &[0, 3, 7, 10],
        },
        ChordQualityInfo {
            name: "m6/9",
            intervals: &[0, 3, 7, 9, 14],
        },
        ChordQualityInfo {
            name: "m6",
            intervals: &[0, 3, 7, 9],
        },
        ChordQualityInfo {
            name: "m",
            intervals: &[0, 3, 7],
        },
        ChordQualityInfo {
            name: "M9",
            intervals: &[0, 4, 7, 11, 14],
        },
        ChordQualityInfo {
            name: "M7",
            intervals: &[0, 4, 7, 11],
        },
        ChordQualityInfo {
            name: "add11",
            intervals: &[0, 4, 7, 17],
        },
        ChordQualityInfo {
            name: "add9",
            intervals: &[0, 4, 7, 14],
        },
        ChordQualityInfo {
            name: "add2",
            intervals: &[0, 2, 4, 7],
        },
        ChordQualityInfo {
            name: "augmaj7",
            intervals: &[0, 4, 8, 11],
        },
        ChordQualityInfo {
            name: "aug7",
            intervals: &[0, 4, 8, 10],
        },
        ChordQualityInfo {
            name: "aug",
            intervals: &[0, 4, 8],
        },
        ChordQualityInfo {
            name: "dim7",
            intervals: &[0, 3, 6, 9],
        },
        ChordQualityInfo {
            name: "dim",
            intervals: &[0, 3, 6],
        },
        ChordQualityInfo {
            name: "sus2",
            intervals: &[0, 2, 7],
        },
        ChordQualityInfo {
            name: "sus4",
            intervals: &[0, 5, 7],
        },
        ChordQualityInfo {
            name: "13",
            intervals: &[0, 4, 7, 10, 14, 17, 21],
        },
        ChordQualityInfo {
            name: "11",
            intervals: &[0, 4, 7, 10, 14, 17],
        },
        ChordQualityInfo {
            name: "9sus4",
            intervals: &[0, 5, 7, 10, 14],
        },
        ChordQualityInfo {
            name: "9",
            intervals: &[0, 4, 7, 10, 14],
        },
        ChordQualityInfo {
            name: "7sus4",
            intervals: &[0, 5, 7, 10],
        },
        ChordQualityInfo {
            name: "7sus2",
            intervals: &[0, 2, 7, 10],
        },
        ChordQualityInfo {
            name: "7sus",
            intervals: &[0, 5, 7, 10],
        },
        ChordQualityInfo {
            name: "7",
            intervals: &[0, 4, 7, 10],
        },
        ChordQualityInfo {
            name: "6/9",
            intervals: &[0, 4, 7, 9, 14],
        },
        ChordQualityInfo {
            name: "6",
            intervals: &[0, 4, 7, 9],
        },
        ChordQualityInfo {
            name: "5",
            intervals: &[0, 7],
        },
        ChordQualityInfo {
            name: "",
            intervals: &[0, 4, 7],
        }, // major triad (default)
    ]
}

/// Autocomplete chord quality suffixes matching a prefix.
pub fn complete_chord(prefix: &str) -> Vec<&'static str> {
    let qualities = all_chord_qualities();
    qualities
        .iter()
        .filter(|q| !q.name.is_empty() && q.name.starts_with(prefix))
        .map(|q| q.name)
        .collect()
}

/// A scale/mode with its name and interval pattern.
#[derive(Debug, Clone)]
pub struct ScaleInfo {
    /// Scale name (e.g., "major", "dorian").
    pub name: &'static str,
    /// Intervals from root in semitones.
    pub intervals: &'static [u8],
}

/// All supported scales/modes.
pub fn all_scales() -> Vec<ScaleInfo> {
    MODES
        .iter()
        .map(|m| ScaleInfo {
            name: m.name,
            intervals: m.intervals,
        })
        .collect()
}

/// Compute the pitches of a scale given root pitch class and mode name.
///
/// Returns MIDI pitch classes (0-11) for each scale degree, or `None` if mode not found.
pub fn scale_pitches(root: u8, mode: &str) -> Option<Vec<u8>> {
    let intervals = harmony::lookup_mode(mode)?;
    Some(intervals.iter().map(|&i| (root + i) % 12).collect())
}

/// Resolve a scale degree to a MIDI pitch.
///
/// `root`: pitch class (0=C), `mode`: scale name, `degree`: 1-indexed,
/// `octave`: MIDI octave, `accidental`: -1/0/1 for flat/natural/sharp.
///
/// Delegates to `voicing::resolve_degree` — the compiler's resolver — so
/// introspection results match compiled output exactly (spec §13.3:
/// `midi = (oct * 12 + 12) + scale_root_semitone + interval`; spec §16:
/// degree 5 in C major at octave 4 is MIDI 67). Like the compiler, the
/// result is clamped to 0–127. Returns `None` only for an unknown mode or
/// degree 0.
pub fn resolve_degree(root: u8, mode: &str, degree: u8, octave: u8, accidental: i8) -> Option<u8> {
    let intervals = harmony::lookup_mode(mode)?;
    if degree == 0 {
        return None;
    }
    Some(voicing::resolve_degree(
        degree, accidental, octave, intervals, root,
    ))
}

/// A directive description for the introspection catalog.
#[derive(Debug, Clone)]
pub struct DirectiveInfo {
    /// Directive name (e.g., "@ppq", "@harmony").
    pub name: &'static str,
    /// Brief description.
    pub description: &'static str,
    /// Whether it's a header directive or block declaration.
    pub kind: DirectiveKind,
}

/// Directive category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectiveKind {
    Header,
    Block,
}

/// All supported directives.
pub fn all_directives() -> Vec<DirectiveInfo> {
    vec![
        DirectiveInfo {
            name: "@ppq",
            description: "Pulses per quarter note (default 480)",
            kind: DirectiveKind::Header,
        },
        DirectiveInfo {
            name: "@bpm",
            description: "Tempo: scalar, inline timeline, or block form (e.g., `120 * 8 | 140`)",
            kind: DirectiveKind::Header,
        },
        DirectiveInfo {
            name: "@ts",
            description: "Time signature: scalar or timeline form (e.g., `4/4 * 8 | 3/4`)",
            kind: DirectiveKind::Header,
        },
        DirectiveInfo {
            name: "@title",
            description: "Optional title string",
            kind: DirectiveKind::Header,
        },
        DirectiveInfo {
            name: "@seed",
            description: "Global seed for seeded operations",
            kind: DirectiveKind::Header,
        },
        DirectiveInfo {
            name: "@bars",
            description: "Global bar count for automatic pattern fill (e.g., `@bars 16`)",
            kind: DirectiveKind::Header,
        },
        DirectiveInfo {
            name: "@scale",
            description: "Tonal context: scalar or timeline form (e.g., `root=C mode=major * 8`)",
            kind: DirectiveKind::Block,
        },
        DirectiveInfo {
            name: "@harmony",
            description: "Harmony timeline with chord progression; name optional when single block",
            kind: DirectiveKind::Block,
        },
        DirectiveInfo {
            name: "@pattern",
            description: "Named pattern with step body",
            kind: DirectiveKind::Block,
        },
        DirectiveInfo {
            name: "@track",
            description: "Track declaration with play/steps",
            kind: DirectiveKind::Block,
        },
        DirectiveInfo {
            name: "@drummap",
            description: "Drum map (identifier to MIDI note)",
            kind: DirectiveKind::Block,
        },
        DirectiveInfo {
            name: "@tempo",
            description: "[DEPRECATED in v0.5] Use @bpm timeline form instead",
            kind: DirectiveKind::Block,
        },
    ]
}

/// A transform description for the introspection catalog.
#[derive(Debug, Clone)]
pub struct TransformInfo {
    /// Transform name (e.g., "reverse", "transpose").
    pub name: &'static str,
    /// Parameter signature (e.g., "(semitones)", "(ratio, unit)").
    pub signature: &'static str,
    /// Brief description.
    pub description: &'static str,
}

/// All supported transforms.
pub fn all_transforms() -> Vec<TransformInfo> {
    vec![
        TransformInfo {
            name: "reverse",
            signature: "",
            description: "Reverse step order",
        },
        TransformInfo {
            name: "invert",
            signature: "",
            description: "Invert intervals around first pitch",
        },
        TransformInfo {
            name: "retrograde",
            signature: "",
            description: "Reverse + invert",
        },
        TransformInfo {
            name: "mirror",
            signature: "",
            description: "Concatenate with own reverse",
        },
        TransformInfo {
            name: "rotate",
            signature: "(steps)",
            description: "Cyclic rotation by N steps",
        },
        TransformInfo {
            name: "stretch",
            signature: "(factor)",
            description: "Multiply durations by factor",
        },
        TransformInfo {
            name: "compress",
            signature: "(factor)",
            description: "Divide durations by factor",
        },
        TransformInfo {
            name: "transpose",
            signature: "(semitones)",
            description: "Transpose absolute pitches",
        },
        TransformInfo {
            name: "shift_oct",
            signature: "(octaves)",
            description: "Shift octave for all notes",
        },
        TransformInfo {
            name: "subset",
            signature: "(indices...)",
            description: "Retain only specified step indices",
        },
        TransformInfo {
            name: "interleave",
            signature: "(pattern_name)",
            description: "Alternate steps with another pattern",
        },
        TransformInfo {
            name: "humanize",
            signature: "(timing, intensity)",
            description: "Apply humanization",
        },
        TransformInfo {
            name: "vary",
            signature: "(probability)",
            description: "Select variant pool alternatives",
        },
        TransformInfo {
            name: "swing",
            signature: "(ratio, unit)",
            description: "Apply swing timing",
        },
        TransformInfo {
            name: "rubato",
            signature: "(depth, curve)",
            description: "Time envelope deformation",
        },
        TransformInfo {
            name: "ritardando",
            signature: "(depth)",
            description: "Gradual tempo decrease",
        },
        TransformInfo {
            name: "accelerando",
            signature: "(depth)",
            description: "Gradual tempo increase",
        },
        TransformInfo {
            name: "agogic",
            signature: "(steps...)",
            description: "Duration emphasis on steps",
        },
        TransformInfo {
            name: "breathe",
            signature: "(position, duration)",
            description: "Micro-pause at position",
        },
        TransformInfo {
            name: "swell",
            signature: "(peak, curve)",
            description: "Velocity envelope",
        },
        TransformInfo {
            name: "phrase",
            signature: "(tension, release)",
            description: "Composite rubato + agogic + swell",
        },
        TransformInfo {
            name: "evolve",
            signature: "(toggle_probability)",
            description: "Shift register mutation",
        },
        TransformInfo {
            name: "euclid_gate",
            signature: "(pulses, steps)",
            description: "Euclidean rhythm gate",
        },
        TransformInfo {
            name: "echo",
            signature: "(rate, repeats, decay)",
            description: "Repeat notes with decay",
        },
        TransformInfo {
            name: "vel_curve",
            signature: "(wave, min, max, repeat)",
            description: "Shape velocity across steps",
        },
        TransformInfo {
            name: "gate_curve",
            signature: "(wave, min, max, repeat)",
            description: "Shape gate across steps",
        },
        TransformInfo {
            name: "scale_lock",
            signature: "(scale, root, snap_mode)",
            description: "Snap pitches to a scale",
        },
        TransformInfo {
            name: "arp",
            signature: "(pattern, rate, octaves)",
            description: "Explode chord steps into arpeggiated sequences (emission-phase)",
        },
    ]
}

// ── Structured Harmony Timeline (Phase 4) ────────────────────────────

/// Per-bar harmony information for IDE display.
#[derive(Debug, Clone)]
pub struct HarmonyBarInfo {
    /// Bar number (1-indexed).
    pub bar: u32,
    /// Chords active in this bar.
    pub chords: Vec<HarmonyChordInfo>,
}

/// A single chord within a bar's harmony.
#[derive(Debug, Clone)]
pub struct HarmonyChordInfo {
    /// Display symbol (e.g., "Cmaj7").
    pub symbol: String,
    /// Root pitch class (0=C … 11=B).
    pub root: u8,
    /// Chord intervals from root.
    pub intervals: Vec<u8>,
    /// Roman numeral representation (e.g., "iv7", "bVImaj7"), if chord was specified as Roman numeral.
    pub roman_numeral: Option<String>,
    /// Beat start within the bar (0-based, fractional).
    pub beat_start: f64,
    /// Beat end within the bar (fractional).
    pub beat_end: f64,
    /// Tick start (absolute).
    pub tick_start: u64,
    /// Tick end (absolute, exclusive).
    pub tick_end: u64,
}

/// Pitch class to note name (sharps preferred).
fn pitch_class_name(pc: u8) -> &'static str {
    match pc % 12 {
        0 => "C",
        1 => "C#",
        2 => "D",
        3 => "D#",
        4 => "E",
        5 => "F",
        6 => "F#",
        7 => "G",
        8 => "G#",
        9 => "A",
        10 => "A#",
        11 => "B",
        _ => unreachable!(),
    }
}

/// Reverse-map a chord's intervals to a quality suffix string.
fn chord_quality_suffix(intervals: &[u8]) -> &'static str {
    for q in all_chord_qualities() {
        if q.intervals == intervals {
            return q.name;
        }
    }
    "" // fallback: major triad
}

/// Format a Roman numeral string from a `RomanNumeralDegree` and chord intervals.
///
/// Produces strings like "I", "iv7", "bVImaj7". Minor chords use lowercase numerals;
/// major/dominant use uppercase.
fn format_roman_numeral(roman: &crate::ast::RomanNumeralDegree, intervals: &[u8]) -> String {
    let numerals = ["I", "II", "III", "IV", "V", "VI", "VII"];
    let idx = roman.degree_idx as usize % numerals.len();
    let base = numerals[idx];

    // Determine if the chord is minor (has minor 3rd interval)
    let is_minor = intervals.len() >= 2 && intervals[1] == 3;

    let numeral = if is_minor {
        base.to_lowercase()
    } else {
        base.to_string()
    };

    let accidental = match roman.accidental {
        -1 => "b",
        1 => "#",
        _ => "",
    };

    let suffix = chord_quality_suffix(intervals);
    // For minor chords, strip leading "min"/"m" from suffix since lowercase numeral implies it
    let display_suffix = if is_minor {
        if let Some(rest) = suffix.strip_prefix("min") {
            rest
        } else if let Some(rest) = suffix.strip_prefix('m') {
            // Be careful not to strip 'm' from "maj" — only strip bare "m" or "m7" etc.
            if rest.is_empty()
                || rest.starts_with(|c: char| c.is_ascii_digit())
                || rest.starts_with('(')
            {
                rest
            } else {
                suffix
            }
        } else {
            suffix
        }
    } else {
        suffix
    };

    format!("{accidental}{numeral}{display_suffix}")
}

/// Format a chord symbol string from root + intervals.
fn format_chord_symbol(chord: &ChordSymbol) -> String {
    let root_name = pitch_class_name(chord.root);
    let suffix = chord_quality_suffix(&chord.intervals);
    let slash = chord
        .slash_bass
        .map(|b| format!("/{}", pitch_class_name(b)))
        .unwrap_or_default();
    format!("{root_name}{suffix}{slash}")
}

/// A chord resolved against a key for two-row display in a chord track.
///
/// `rendered` is the absolute name (e.g. "Cmaj7", "G7"); `relative` is the
/// Roman-numeral name in the key (e.g. "I", "V7", "bVII"). `root`/`intervals` are
/// the resolved concrete chord, so a caller can voice it without re-parsing.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChordDescription {
    pub rendered: String,
    pub relative: String,
    pub root: u8,
    pub intervals: Vec<u8>,
}

/// Resolve a chord symbol — a Roman numeral (`V7`, `bVII`) OR a letter chord
/// (`Cmaj7`, `Dm`) — against a key into its rendered (absolute) + relative (Roman)
/// names. Returns `None` if the symbol can't be parsed. Intended for IDE/host
/// integrations that display both absolute and Roman-numeral chord names.
pub fn describe_chord_in_key(
    symbol: &str,
    key_root: u8,
    key_mode: &str,
) -> Option<ChordDescription> {
    let tc = crate::ast::TonalContext {
        root: Some(key_root % 12),
        mode: key_mode.to_string(),
        span: None,
    };
    let chord = harmony::parse_chord_symbol_with_context(symbol, &tc).ok()?;
    let rendered = format_chord_symbol(&chord);
    let relative = match &chord.roman {
        // Typed as a Roman numeral — the degree is already known.
        Some(rn) => format_roman_numeral(rn, &chord.intervals),
        // Typed as a letter chord — derive the Roman numeral from the root + key.
        None => roman_from_absolute(chord.root, &chord.intervals, key_root, key_mode),
    };
    Some(ChordDescription {
        rendered,
        relative,
        root: chord.root,
        intervals: chord.intervals,
    })
}

/// Derive a Roman-numeral name for an ABSOLUTE chord (root pitch-class + intervals)
/// relative to a key. Exact scale degrees resolve cleanly (the diatonic case);
/// a chromatic root falls back to the nearest degree with a `b`/`#` accidental
/// (flat-of-the-degree-above first — the common bII/bIII/bVI/bVII spelling). Exact
/// borrowed-chord spelling conventions are a later refinement (advanced-harmony scope).
fn roman_from_absolute(root: u8, intervals: &[u8], key_root: u8, key_mode: &str) -> String {
    let mode_ivs = match harmony::lookup_mode(key_mode) {
        Some(ivs) => ivs,
        None => return String::new(),
    };
    let rel = (((root as i16 - key_root as i16) % 12 + 12) % 12) as u8;
    let mk = |degree_idx: usize, accidental: i8| {
        format_roman_numeral(
            &crate::ast::RomanNumeralDegree {
                degree_idx: degree_idx as u8,
                accidental,
            },
            intervals,
        )
    };
    if let Some(idx) = mode_ivs.iter().position(|&iv| iv == rel) {
        return mk(idx, 0); // exact degree (diatonic)
    }
    if let Some(idx) = mode_ivs.iter().position(|&iv| iv == (rel + 1) % 12) {
        return mk(idx, -1); // flat of the degree above
    }
    if let Some(idx) = mode_ivs.iter().position(|&iv| iv == (rel + 11) % 12) {
        return mk(idx, 1); // sharp of the degree below
    }
    String::new()
}

/// Build a structured harmony timeline from a HarmonyIndex.
///
/// Groups spans into bars for display. Uses the bar layout to compute
/// beat positions within each bar.
pub fn harmony_timeline(
    index: &HarmonyIndex,
    bar_layout: &compiler::BarLayout,
    ppq: u32,
) -> Vec<HarmonyBarInfo> {
    let mut bars: Vec<HarmonyBarInfo> = Vec::new();
    let mut current_bar = 0u32;
    let mut current_bar_info: Option<HarmonyBarInfo> = None;

    for span in index.spans() {
        let (bar_num, bar_start) = bar_layout.bar_at_tick(span.start_tick);
        let (ts_num, ts_den) = bar_layout.ts_for_bar(bar_num);
        let ticks_per_beat = ppq as f64 * 4.0 / ts_den as f64;
        let _ = ts_num; // used implicitly via bar_layout

        if bar_num != current_bar {
            if let Some(info) = current_bar_info.take() {
                bars.push(info);
            }
            current_bar = bar_num;
            current_bar_info = Some(HarmonyBarInfo {
                bar: bar_num,
                chords: Vec::new(),
            });
        }

        let beat_start = (span.start_tick as f64 - bar_start as f64) / ticks_per_beat;
        let beat_end = (span.end_tick as f64 - bar_start as f64) / ticks_per_beat;

        if let Some(ref mut info) = current_bar_info {
            let roman_numeral = span
                .context
                .chord
                .roman
                .as_ref()
                .map(|r| format_roman_numeral(r, &span.context.chord.intervals));
            info.chords.push(HarmonyChordInfo {
                symbol: format_chord_symbol(&span.context.chord),
                root: span.context.chord.root,
                intervals: span.context.chord.intervals.clone(),
                roman_numeral,
                beat_start,
                beat_end,
                tick_start: span.start_tick,
                tick_end: span.end_tick,
            });
        }
    }
    if let Some(info) = current_bar_info {
        bars.push(info);
    }
    bars
}

// ── Structured Scale Timeline (Phase 5) ──────────────────────────────

/// Per-bar scale information for IDE display.
#[derive(Debug, Clone)]
pub struct ScaleBarInfo {
    /// Bar number (1-indexed).
    pub bar: u32,
    /// Root pitch class (0=C … 11=B).
    pub root: u8,
    /// Root note name.
    pub root_name: String,
    /// Mode/scale name.
    pub mode: String,
    /// Pitch classes in this scale.
    pub pitch_classes: Vec<u8>,
}

/// Build a structured scale timeline.
///
/// Returns one entry per distinct scale change, with the bar range where it applies.
pub fn scale_timeline_info(scale_timeline: &ScaleTimeline, total_bars: u32) -> Vec<ScaleBarInfo> {
    let mut result = Vec::new();
    for bar in 1..=total_bars.max(1) {
        let (intervals, root) = scale_timeline.context_at_bar(bar);
        // Deduplicate: only emit when root or intervals change
        let should_emit = result.last().is_none_or(|prev: &ScaleBarInfo| {
            prev.root != root
                || prev.pitch_classes
                    != intervals
                        .iter()
                        .map(|&i| (root + i) % 12)
                        .collect::<Vec<_>>()
        });
        if should_emit {
            let mode_name = MODES
                .iter()
                .find(|m| m.intervals == intervals)
                .map(|m| m.name)
                .unwrap_or("unknown");
            result.push(ScaleBarInfo {
                bar,
                root,
                root_name: pitch_class_name(root).to_string(),
                mode: mode_name.to_string(),
                pitch_classes: intervals.iter().map(|&i| (root + i) % 12).collect(),
            });
        }
    }
    result
}

// ── Step Pitch Resolution (Phase 6b) ────────────────────────────────

/// Resolve a single step token to MIDI pitches.
///
/// Returns `None` for Rest/Tie, `Some(pitches)` for note tokens.
/// This is a convenience wrapper around the voicing/degree resolution logic,
/// suitable for IDE hover display.
#[allow(clippy::too_many_arguments)]
pub fn resolve_step_pitches(
    token: &StepToken,
    chord: Option<&ChordContext>,
    scale_root: u8,
    mode: &str,
    octave: u8,
    voice: VoicingStrategy,
    inv: Inversion,
    prev_pitches: Option<&[u8]>,
) -> Option<Vec<u8>> {
    match token {
        StepToken::Rest | StepToken::Tie => None,
        StepToken::Subdivision { .. } | StepToken::Variant { .. } => None,

        StepToken::Degree {
            degree,
            accidental,
            octave: oct_override,
            ..
        } => {
            let oct = oct_override.unwrap_or(octave);
            resolve_degree(scale_root, mode, *degree, oct, *accidental).map(|p| vec![p])
        }

        StepToken::AbsolutePitch { midi_note, .. } => Some(vec![*midi_note]),

        StepToken::MidiNumber { note, .. } => Some(vec![*note]),

        StepToken::DrumHit { .. } => None, // drum hits resolve via drummap, not pitch

        StepToken::CurrentChord { .. } => chord.map(|ctx| {
            let (pitches, _) = voicing::voice_chord(&ctx.chord, voice, inv, octave, prev_pitches);
            pitches
        }),

        StepToken::ChordOrdinal {
            degree,
            octave: oct_override,
            ..
        } => chord.map(|ctx| {
            // Delegate to the compiler's resolver so IDE hover matches
            // compiled output exactly (spec §13.3 step 4: `(oct * 12 + 12)`
            // base; a forced `%n/oct` octave suppresses the ordinal wrap
            // shift, matching `emit_token`).
            vec![voicing::resolve_chord_ordinal(
                *degree,
                octave,
                *oct_override,
                &ctx.chord.intervals,
                ctx.chord.root,
            )]
        }),

        StepToken::ChordStep { chord: cs, .. } => {
            let (pitches, _) = voicing::voice_chord(cs, voice, inv, octave, prev_pitches);
            Some(pitches)
        }
    }
}

// ── Cursor Context (Phase 6c) ───────────────────────────────────────

/// Block type identifier for cursor context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    Scale,
    Harmony,
    Pattern,
    Track,
    DrumMap,
}

/// Summary of a pattern block's parameters.
#[derive(Debug, Clone)]
pub struct PatternBlockSummary {
    /// Pattern name.
    pub name: String,
    /// Step count.
    pub steps: u32,
    /// Step unit as fraction (numerator, denominator).
    pub unit: (u32, u32),
    /// Base octave.
    pub octave: u8,
    /// Base velocity.
    pub velocity: u8,
    /// Base gate ratio.
    pub gate: f64,
}

/// Chord info at cursor position.
#[derive(Debug, Clone)]
pub struct ChordInfo {
    /// Display symbol (e.g., "Cmaj7").
    pub symbol: String,
    /// Root pitch class (0=C … 11=B).
    pub root: u8,
    /// Chord intervals.
    pub intervals: Vec<u8>,
    /// Roman numeral if applicable.
    pub roman_numeral: Option<String>,
}

/// Context information at a cursor position in the source.
#[derive(Debug, Clone, Default)]
pub struct CursorContext {
    /// Type of the block the cursor is in.
    pub block_type: Option<BlockType>,
    /// Name of the block (if named).
    pub block_name: Option<String>,
    /// Step index within the block (if in a step body).
    pub step_index: Option<usize>,
    /// Step token text at cursor.
    pub step_token: Option<String>,
    /// Resolved MIDI pitch(es) at cursor.
    pub resolved_pitches: Option<Vec<u8>>,
    /// Track parameters (if in a track block).
    pub track_channel: Option<u8>,
    /// Available annotation names in this context.
    pub available_annotations: Vec<&'static str>,
    /// Available transform names in this context.
    pub available_transforms: Vec<&'static str>,
    /// Active chord at cursor position (if in a track with follow= or in harmony body).
    pub current_chord: Option<ChordInfo>,
    /// Active scale at cursor position.
    pub current_scale: Option<ScaleInfo>,
    /// Human-readable pitch name for the step token (e.g., "Eb4").
    pub resolved_pitch_name: Option<String>,
    /// Harmony bar index (1-based) the cursor is in.
    pub harmony_bar_index: Option<usize>,
    /// Pattern parameters if cursor is inside a pattern block.
    pub pattern_params: Option<PatternBlockSummary>,
}

/// Annotations available in step context.
const STEP_ANNOTATIONS: &[&str] = &[
    "vel",
    "gate",
    "dur",
    "shift",
    "lshift",
    "oct",
    "expr",
    "dyn",
    "sus",
    "pan",
    "vol",
    "pb",
    "at",
    "cc",
    "cond",
    "every",
    "once",
    "pre",
    "ratch",
    "ratch_decay",
    "prob",
    "glide",
];

/// Get context information at a cursor byte offset in the source.
///
/// Parses the source and walks the AST to determine what block, step,
/// and token the cursor is positioned on.
pub fn get_context_at_cursor(source: &str, byte_offset: usize) -> CursorContext {
    let program = match crate::parser::parse_only(source) {
        Ok(p) => p,
        Err(_) => return CursorContext::default(),
    };

    let mut ctx = CursorContext::default();

    // Walk blocks to find which one contains the cursor
    for block in &program.blocks {
        match block {
            Block::Harmony(h) => {
                if let Some(span) = h.span {
                    if span.start <= byte_offset && byte_offset < span.end {
                        ctx.block_type = Some(BlockType::Harmony);
                        ctx.block_name = h.name.clone();
                        return ctx;
                    }
                }
            }
            Block::Pattern(p) => {
                if let Some(span) = p.span {
                    if span.start <= byte_offset && byte_offset < span.end {
                        ctx.block_type = Some(BlockType::Pattern);
                        ctx.block_name = Some(p.name.clone());
                        ctx.available_annotations = STEP_ANNOTATIONS.to_vec();
                        ctx.available_transforms =
                            all_transforms().iter().map(|t| t.name).collect();
                        // Check individual step lines
                        if let crate::ast::PatternBody::Steps(steps) = &p.body {
                            for (i, step) in steps.iter().enumerate() {
                                if let Some(step_span) = step.span {
                                    if step_span.start <= byte_offset && byte_offset < step_span.end
                                    {
                                        ctx.step_index = Some(i);
                                        break;
                                    }
                                }
                            }
                        }
                        return ctx;
                    }
                }
            }
            Block::Track(t) => {
                if let Some(span) = t.span {
                    if span.start <= byte_offset && byte_offset < span.end {
                        ctx.block_type = Some(BlockType::Track);
                        ctx.block_name = Some(t.name.clone());
                        ctx.track_channel = Some(t.channel);
                        ctx.available_annotations = STEP_ANNOTATIONS.to_vec();
                        ctx.available_transforms =
                            all_transforms().iter().map(|t| t.name).collect();
                        return ctx;
                    }
                }
            }
            Block::DrumMap(d) => {
                if let Some(span) = d.span {
                    if span.start <= byte_offset && byte_offset < span.end {
                        ctx.block_type = Some(BlockType::DrumMap);
                        ctx.block_name = d.name.clone();
                        return ctx;
                    }
                }
            }
            Block::Scale(_) => {
                ctx.block_type = Some(BlockType::Scale);
                // Scalar scale blocks don't have spans on TonalContext
            }
            _ => {}
        }
    }

    ctx
}

/// Format a MIDI note number as a readable pitch name (e.g., 60 → "C4", 63 → "Eb4").
fn midi_note_name(note: u8) -> String {
    let names = [
        "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
    ];
    let octave = (note / 12) as i8 - 1;
    let pc = (note % 12) as usize;
    format!("{}{}", names[pc], octave)
}

/// Get enriched context at a cursor position, using a pre-compiled output.
///
/// This is the rich version of `get_context_at_cursor`. The caller should cache
/// `CompileOutput` and pass it in for performance (avoids re-compiling on every
/// cursor move).
///
/// Falls back gracefully: if `compile_output` is `None`, returns the same
/// result as `get_context_at_cursor`.
pub fn get_rich_context_at_cursor(
    source: &str,
    byte_offset: usize,
    compile_output: Option<&compiler::CompileOutput>,
) -> CursorContext {
    let mut ctx = get_context_at_cursor(source, byte_offset);

    let output = match compile_output {
        Some(o) => o,
        None => return ctx,
    };

    let program = match &output.program {
        Some(p) => p,
        None => return ctx,
    };

    // Populate pattern_params if cursor is in a pattern block
    if ctx.block_type == Some(BlockType::Pattern) {
        if let Some(ref name) = ctx.block_name {
            for block in &program.blocks {
                if let Block::Pattern(p) = block {
                    if &p.name == name {
                        ctx.pattern_params = Some(PatternBlockSummary {
                            name: p.name.clone(),
                            steps: p.steps,
                            unit: p.unit,
                            octave: p.octave,
                            velocity: p.velocity,
                            gate: p.gate,
                        });
                        break;
                    }
                }
            }
        }
    }

    // Populate current_scale from scale timeline
    if let Some(ref prog) = output.program {
        // Build scale timeline from the header
        let tc = prog
            .blocks
            .iter()
            .find_map(|b| {
                if let Block::Scale(tc) = b {
                    Some(tc.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let mode_name = tc.mode.as_str();
        if let Some(mode_entry) = MODES.iter().find(|m| m.name == mode_name) {
            ctx.current_scale = Some(ScaleInfo {
                name: mode_entry.name,
                intervals: mode_entry.intervals,
            });
        }
    }

    // Enrich with chord/pitch info using step_index and pattern unit ticks
    if ctx.block_type == Some(BlockType::Track) || ctx.block_type == Some(BlockType::Pattern) {
        if let Some(step_idx) = ctx.step_index {
            // Compute unit ticks from pattern params
            let unit_ticks = ctx.pattern_params.as_ref().map(|pp| {
                if pp.unit.1 > 0 {
                    (output.ppq as u64 * 4 * pp.unit.0 as u64) / pp.unit.1 as u64
                } else {
                    output.ppq as u64 // fallback: quarter note
                }
            });

            if let Some(ut) = unit_ticks {
                // Find the first track using this pattern to get absolute start tick
                let pattern_name = ctx.block_name.as_deref().unwrap_or("");
                let base_tick = output
                    .tracks
                    .iter()
                    .flat_map(|t| t.patterns.iter())
                    .find(|pi| pi.pattern_name == pattern_name)
                    .map(|pi| pi.start_tick)
                    .unwrap_or(0);

                let cursor_tick = base_tick + step_idx as u64 * ut;

                // Compute bar index from tick using header time signature
                let header = &program.header;
                let ticks_per_bar = (output.ppq as u64 * 4 * header.ts_numerator as u64)
                    / header.ts_denominator as u64;
                if let Some(bar) = cursor_tick.checked_div(ticks_per_bar) {
                    ctx.harmony_bar_index = Some(bar as usize + 1);
                }
            }
        }

        // Resolve pitch names from resolved_pitches
        if let Some(ref pitches) = ctx.resolved_pitches {
            if let Some(&note) = pitches.first() {
                ctx.resolved_pitch_name = Some(midi_note_name(note));
            }
        }
    }

    ctx
}

// ── Unified Completion Provider (v0.9 Phase 4) ─────────────────────

/// Category of a completion item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    /// Chord symbol (e.g., "Cmaj7", "Dm7").
    ChordSymbol,
    /// Scale/mode name.
    ScaleName,
    /// Transform name.
    Transform,
    /// Annotation keyword.
    Annotation,
    /// Directive (e.g., "@harmony", "@pattern").
    Directive,
    /// Step token (e.g., "^1", "%1", ".", "~").
    StepToken,
    /// Pattern reference name.
    PatternRef,
}

/// A single completion suggestion.
#[derive(Debug, Clone)]
pub struct CompletionItem {
    /// Display label (shown in autocomplete list).
    pub label: String,
    /// Optional detail string (e.g., parameter signature or description).
    pub detail: Option<String>,
    /// Completion category.
    pub kind: CompletionKind,
    /// Text to insert when accepted.
    pub insert_text: String,
    /// Relevance score (0.0–1.0, higher = more relevant).
    pub relevance: f32,
    /// Grouping category for display.
    pub category: String,
}

/// Provide context-aware completions at a cursor position.
///
/// Uses `get_context_at_cursor` to determine the editing context, then returns
/// appropriate completions ranked by relevance. Pattern names from the source
/// are included when in a track's `play:` context.
pub fn complete_at_cursor(source: &str, byte_offset: usize) -> Vec<CompletionItem> {
    let ctx = get_context_at_cursor(source, byte_offset);
    let mut items = Vec::new();

    match ctx.block_type {
        Some(BlockType::Harmony) => {
            // Chord quality completions
            for q in all_chord_qualities() {
                if !q.name.is_empty() {
                    items.push(CompletionItem {
                        label: q.name.to_string(),
                        detail: Some(format!("intervals: {:?}", q.intervals)),
                        kind: CompletionKind::ChordSymbol,
                        insert_text: q.name.to_string(),
                        relevance: 0.8,
                        category: "Chord Quality".to_string(),
                    });
                }
            }
        }
        Some(BlockType::Pattern) => {
            // Step token completions
            let step_tokens = [
                ("^1", "Scale degree 1"),
                ("^2", "Scale degree 2"),
                ("^3", "Scale degree 3"),
                ("^4", "Scale degree 4"),
                ("^5", "Scale degree 5"),
                ("^6", "Scale degree 6"),
                ("^7", "Scale degree 7"),
                ("%1", "Chord tone 1 (root)"),
                ("%2", "Chord tone 2 (3rd)"),
                ("%3", "Chord tone 3 (5th)"),
                ("%4", "Chord tone 4 (7th)"),
                (".", "Rest"),
                ("~", "Tie"),
                ("$chord", "Current harmony chord"),
            ];
            for (token, desc) in step_tokens {
                items.push(CompletionItem {
                    label: token.to_string(),
                    detail: Some(desc.to_string()),
                    kind: CompletionKind::StepToken,
                    insert_text: token.to_string(),
                    relevance: 0.9,
                    category: "Step Token".to_string(),
                });
            }
            // Annotation completions
            for &ann in STEP_ANNOTATIONS {
                items.push(CompletionItem {
                    label: format!("[{ann}:]"),
                    detail: Some(format!("{ann} annotation")),
                    kind: CompletionKind::Annotation,
                    insert_text: format!("[{ann}:]"),
                    relevance: 0.6,
                    category: "Annotation".to_string(),
                });
            }
        }
        Some(BlockType::Track) => {
            // Pattern reference completions — extract pattern names from source
            if let Ok(program) = crate::parser::parse_only(source) {
                for block in &program.blocks {
                    if let Block::Pattern(p) = block {
                        items.push(CompletionItem {
                            label: p.name.clone(),
                            detail: Some(format!(
                                "pattern ({} steps, unit={}/{})",
                                p.steps, p.unit.0, p.unit.1
                            )),
                            kind: CompletionKind::PatternRef,
                            insert_text: p.name.clone(),
                            relevance: 0.95,
                            category: "Pattern".to_string(),
                        });
                    }
                }
            }
            // Transform completions (for after ->)
            for t in all_transforms() {
                items.push(CompletionItem {
                    label: t.name.to_string(),
                    detail: Some(format!("{}{} — {}", t.name, t.signature, t.description)),
                    kind: CompletionKind::Transform,
                    insert_text: if t.signature.is_empty() {
                        t.name.to_string()
                    } else {
                        format!("{}()", t.name)
                    },
                    relevance: 0.7,
                    category: "Transform".to_string(),
                });
            }
        }
        Some(BlockType::Scale) | Some(BlockType::DrumMap) | None => {
            // Top-level: directive completions
            for d in all_directives() {
                items.push(CompletionItem {
                    label: d.name.to_string(),
                    detail: Some(d.description.to_string()),
                    kind: CompletionKind::Directive,
                    insert_text: d.name.to_string(),
                    relevance: 0.9,
                    category: match d.kind {
                        DirectiveKind::Header => "Header Directive".to_string(),
                        DirectiveKind::Block => "Block Directive".to_string(),
                    },
                });
            }
            // Scale name completions (for @scale mode=)
            for s in all_scales() {
                items.push(CompletionItem {
                    label: s.name.to_string(),
                    detail: Some(format!("intervals: {:?}", s.intervals)),
                    kind: CompletionKind::ScaleName,
                    insert_text: s.name.to_string(),
                    relevance: 0.7,
                    category: "Scale".to_string(),
                });
            }
        }
    }

    // Sort by relevance descending
    items.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    items
}

// ── compile_with_ast (Phase 6a) ─────────────────────────────────────

/// Compile source and return the result with the parsed AST attached.
///
/// This is the primary entry point for IDE integration. The returned
/// `CompileOutput` has `program` set to `Some(ast)`.
///
/// Note: `resolved_seed` is left as `None`. The caller should set it
/// on `program.header` before calling `compiler::compile()` if seeded
/// transforms are needed.
pub fn compile_with_ast(source: &str) -> CompileResult<compiler::CompileOutput> {
    let program = crate::parser::parse_only(source)?;
    let mut output = compiler::compile(&program.header, &program.blocks)?;
    output.program = Some(program);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_chord_qualities_non_empty() {
        let qualities = all_chord_qualities();
        assert!(qualities.len() >= 44);
        // Every quality has at least root interval
        for q in &qualities {
            assert!(!q.intervals.is_empty());
            assert_eq!(q.intervals[0], 0);
        }
    }

    #[test]
    fn test_complete_chord_prefix() {
        let results = complete_chord("maj");
        assert!(results.contains(&"maj7"));
        assert!(results.contains(&"maj9"));
        assert!(results.contains(&"maj11"));
        assert!(!results.contains(&"m7"));
    }

    #[test]
    fn test_complete_chord_empty_prefix() {
        let results = complete_chord("");
        // Should return all non-empty quality names
        assert!(results.len() >= 43);
    }

    #[test]
    fn test_all_scales_non_empty() {
        let scales = all_scales();
        assert!(scales.len() >= 40);
        for s in &scales {
            assert!(!s.intervals.is_empty());
            assert_eq!(s.intervals[0], 0);
        }
    }

    #[test]
    fn test_scale_pitches_c_major() {
        let pitches = scale_pitches(0, "major").unwrap();
        assert_eq!(pitches, vec![0, 2, 4, 5, 7, 9, 11]);
    }

    #[test]
    fn test_scale_pitches_d_dorian() {
        let pitches = scale_pitches(2, "dorian").unwrap();
        // D dorian: D(2) E(4) F(5) G(7) A(9) B(11) C(0)
        assert_eq!(pitches, vec![2, 4, 5, 7, 9, 11, 0]);
    }

    #[test]
    fn test_scale_pitches_unknown_mode() {
        assert!(scale_pitches(0, "nonexistent").is_none());
    }

    #[test]
    fn test_resolve_degree_c_major() {
        // Compiler convention (spec §13.3 / §16): C4 = MIDI 60.
        // C4 = degree 1 in C major at octave 4
        assert_eq!(resolve_degree(0, "major", 1, 4, 0), Some(60));
        // E4 = degree 3
        assert_eq!(resolve_degree(0, "major", 3, 4, 0), Some(64));
        // G4 = degree 5 (spec §16 example: MIDI 67)
        assert_eq!(resolve_degree(0, "major", 5, 4, 0), Some(67));
    }

    #[test]
    fn test_resolve_degree_with_accidental() {
        // Eb4 = degree 3 flat in C major
        assert_eq!(resolve_degree(0, "major", 3, 4, -1), Some(63));
    }

    #[test]
    fn test_resolve_degree_matches_compiler_voicing() {
        // Property-style parity: for the same inputs, introspection must
        // produce exactly the pitch the compiler produces (the compiler —
        // voicing::resolve_degree — is the source of truth).
        for &(root, mode) in &[
            (0u8, "major"),
            (2, "dorian"),
            (7, "mixolydian"),
            (9, "minor"),
            (4, "phrygian"),
            (1, "harmonic_minor"),
        ] {
            let intervals = harmony::lookup_mode(mode).unwrap();
            for degree in 1u8..=13 {
                for octave in [0u8, 3, 4, 5, 8] {
                    for accidental in [-1i8, 0, 1] {
                        let via_introspect = resolve_degree(root, mode, degree, octave, accidental)
                            .expect("degree > 0 with known mode must resolve");
                        let via_compiler = crate::voicing::resolve_degree(
                            degree, accidental, octave, intervals, root,
                        );
                        assert_eq!(
                            via_introspect, via_compiler,
                            "mismatch: root={root} mode={mode} deg={degree} \
                             oct={octave} acc={accidental}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_resolve_step_pitches_chord_ordinal_matches_compiler() {
        // The ChordOrdinal branch must match voicing::resolve_chord_ordinal
        // for both default and forced octaves.
        let ctx = ChordContext {
            chord: crate::ast::ChordSymbol {
                root: 7,                      // G
                intervals: vec![0, 4, 7, 10], // G7
                slash_bass: None,
                roman: None,
            },
            mode_intervals: vec![0, 2, 4, 5, 7, 9, 11],
            scale_root: 0,
        };
        for degree in 1u32..=8 {
            for oct_override in [None, Some(3u8), Some(5)] {
                let token = StepToken::ChordOrdinal {
                    degree,
                    octave: oct_override,
                    annotations: vec![],
                };
                let via_introspect = resolve_step_pitches(
                    &token,
                    Some(&ctx),
                    0,
                    "major",
                    4,
                    VoicingStrategy::Close,
                    Inversion::Fixed(0),
                    None,
                )
                .expect("chord ordinal with context resolves");
                let via_compiler = crate::voicing::resolve_chord_ordinal(
                    degree,
                    4,
                    oct_override,
                    &ctx.chord.intervals,
                    ctx.chord.root,
                );
                assert_eq!(
                    via_introspect,
                    vec![via_compiler],
                    "mismatch: degree={degree} oct_override={oct_override:?}"
                );
            }
        }
    }

    #[test]
    fn test_all_directives_complete() {
        let dirs = all_directives();
        let names: Vec<&str> = dirs.iter().map(|d| d.name).collect();
        assert!(names.contains(&"@ppq"));
        assert!(names.contains(&"@harmony"));
        assert!(names.contains(&"@tempo"));
    }

    #[test]
    fn test_all_transforms_complete() {
        let transforms = all_transforms();
        let names: Vec<&str> = transforms.iter().map(|t| t.name).collect();
        assert!(names.contains(&"reverse"));
        assert!(names.contains(&"transpose"));
        assert!(names.contains(&"humanize"));
        assert!(names.contains(&"scale_lock"));
        assert!(transforms.len() >= 28);
    }

    // ── v0.8 tests ──────────────────────────────────────────────────

    #[test]
    fn test_parse_only_valid_source() {
        let source = "@bpm 120\n@ts 4/4\n";
        let program = crate::parser::parse_only(source).unwrap();
        assert_eq!(program.header.bpm, 120.0);
        assert!(program.span.is_some());
    }

    #[test]
    fn test_parse_only_empty_source() {
        let program = crate::parser::parse_only("").unwrap();
        assert_eq!(program.header.ppq, 480);
        assert!(program.blocks.is_empty());
    }

    #[test]
    fn test_parse_only_with_blocks() {
        let source = "\
@scale root=C mode=major
@harmony main
  | C | Am | F | G |
@pattern p unit=1/4
  ^1
  ^3
  ^5
  ^3
@track melody ch=1 follow=main
  play: p * 1
";
        let program = crate::parser::parse_only(source).unwrap();
        assert_eq!(program.blocks.len(), 4); // scale + harmony + pattern + track
    }

    #[test]
    fn test_compile_with_ast() {
        let source = "\
@scale root=C mode=major
@harmony main
  | C | Am |
@pattern p unit=1/4
  ^1
  ^3
  ^5
  ^3
@track melody ch=1 follow=main
  play: p * 1
";
        let output = compile_with_ast(source).unwrap();
        assert!(output.program.is_some());
        assert!(!output.events.is_empty());
        assert_eq!(output.tracks.len(), 1);
        assert_eq!(output.tracks[0].name, "melody");
        assert_eq!(output.tracks[0].channel, 0); // 0-indexed
    }

    #[test]
    fn test_track_summary_patterns() {
        let source = "\
@harmony main
  | C | Am | F | G |
@pattern p unit=1/4
  ^1
  ^3
  ^5
  ^3
@track melody ch=1 follow=main
  play: p * 2
";
        let output = compile_with_ast(source).unwrap();
        assert_eq!(output.tracks.len(), 1);
        let track = &output.tracks[0];
        assert_eq!(track.patterns.len(), 2);
        assert_eq!(track.patterns[0].pattern_name, "p");
        assert_eq!(track.patterns[1].pattern_name, "p");
        assert!(track.patterns[1].start_tick > track.patterns[0].start_tick);
    }

    #[test]
    fn test_resolve_step_pitches_degree() {
        let token = StepToken::Degree {
            degree: 1,
            accidental: 0,
            octave: Some(4),
            annotations: vec![],
        };
        let pitches = resolve_step_pitches(
            &token,
            None,
            0,
            "major",
            4,
            VoicingStrategy::Close,
            Inversion::Fixed(0),
            None,
        );
        assert_eq!(pitches, Some(vec![60])); // C4 (compiler convention)
    }

    #[test]
    fn test_resolve_step_pitches_absolute() {
        let token = StepToken::AbsolutePitch {
            midi_note: 60,
            annotations: vec![],
        };
        let pitches = resolve_step_pitches(
            &token,
            None,
            0,
            "major",
            4,
            VoicingStrategy::Close,
            Inversion::Fixed(0),
            None,
        );
        assert_eq!(pitches, Some(vec![60]));
    }

    #[test]
    fn test_resolve_step_pitches_rest() {
        let pitches = resolve_step_pitches(
            &StepToken::Rest,
            None,
            0,
            "major",
            4,
            VoicingStrategy::Close,
            Inversion::Fixed(0),
            None,
        );
        assert_eq!(pitches, None);
    }

    #[test]
    fn test_cursor_context_in_pattern() {
        let source = "\
@pattern p unit=1/4
  ^1
  ^3
  ^5
  ^3
";
        // Cursor somewhere in the pattern body
        let ctx = get_context_at_cursor(source, 30);
        assert_eq!(ctx.block_type, Some(BlockType::Pattern));
        assert_eq!(ctx.block_name, Some("p".to_string()));
        assert!(!ctx.available_annotations.is_empty());
        assert!(!ctx.available_transforms.is_empty());
    }

    #[test]
    fn test_cursor_context_empty_source() {
        let ctx = get_context_at_cursor("", 0);
        assert_eq!(ctx.block_type, None);
    }

    #[test]
    fn test_cursor_context_in_track() {
        let source = "\
@harmony main
  | C | Am |
@pattern p unit=1/4
  ^1
  ^3
@track melody ch=2 follow=main
  play: p * 1
";
        // Cursor in the track block
        let ctx = get_context_at_cursor(source, source.len() - 5);
        assert_eq!(ctx.block_type, Some(BlockType::Track));
        assert_eq!(ctx.block_name, Some("melody".to_string()));
        assert_eq!(ctx.track_channel, Some(2));
    }

    #[test]
    fn test_scale_timeline_info_single_scale() {
        use crate::ast::TonalContext;
        let tc = TonalContext {
            root: Some(0),
            mode: "major".to_string(),
            span: None,
        };
        let st = ScaleTimeline::from_tonal_context(&tc).unwrap();
        let info = scale_timeline_info(&st, 4);
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].root_name, "C");
        assert_eq!(info[0].mode, "major");
        assert_eq!(info[0].pitch_classes, vec![0, 2, 4, 5, 7, 9, 11]);
    }

    #[test]
    fn test_chord_quality_suffix_roundtrip() {
        // Major triad
        assert_eq!(chord_quality_suffix(&[0, 4, 7]), "");
        // Minor (first match in list is "min")
        assert_eq!(chord_quality_suffix(&[0, 3, 7]), "min");
        // Dominant 7
        assert_eq!(chord_quality_suffix(&[0, 4, 7, 10]), "7");
    }

    #[test]
    fn test_format_chord_symbol() {
        let cs = ChordSymbol {
            root: 0,
            intervals: vec![0, 4, 7, 11],
            slash_bass: None,
            roman: None,
        };
        assert_eq!(format_chord_symbol(&cs), "Cmaj7");
    }

    #[test]
    fn test_pitch_class_name() {
        assert_eq!(pitch_class_name(0), "C");
        assert_eq!(pitch_class_name(1), "C#");
        assert_eq!(pitch_class_name(9), "A");
        assert_eq!(pitch_class_name(11), "B");
    }

    #[test]
    fn test_midi_note_name() {
        assert_eq!(midi_note_name(60), "C4");
        assert_eq!(midi_note_name(63), "Eb4");
        assert_eq!(midi_note_name(69), "A4");
        assert_eq!(midi_note_name(0), "C-1");
    }

    #[test]
    fn test_format_roman_numeral() {
        use crate::ast::RomanNumeralDegree;
        // Major triad on I
        let r = RomanNumeralDegree {
            degree_idx: 0,
            accidental: 0,
        };
        assert_eq!(format_roman_numeral(&r, &[0, 4, 7]), "I");
        // Minor triad on ii
        let r = RomanNumeralDegree {
            degree_idx: 1,
            accidental: 0,
        };
        assert_eq!(format_roman_numeral(&r, &[0, 3, 7]), "ii");
        // Dominant 7 on V
        let r = RomanNumeralDegree {
            degree_idx: 4,
            accidental: 0,
        };
        assert_eq!(format_roman_numeral(&r, &[0, 4, 7, 10]), "V7");
        // Flat VI major
        let r = RomanNumeralDegree {
            degree_idx: 5,
            accidental: -1,
        };
        assert_eq!(format_roman_numeral(&r, &[0, 4, 7]), "bVI");
        // Minor 7 on vi
        let r = RomanNumeralDegree {
            degree_idx: 5,
            accidental: 0,
        };
        assert_eq!(format_roman_numeral(&r, &[0, 3, 7, 10]), "vi7");
    }

    #[test]
    fn test_describe_chord_in_key() {
        // Roman input in C major: rendered absolute, relative echoes the numeral.
        let d = describe_chord_in_key("I", 0, "major").unwrap();
        assert_eq!(d.rendered, "C");
        assert_eq!(d.relative, "I");
        let d = describe_chord_in_key("V7", 0, "major").unwrap();
        assert_eq!(d.rendered, "G7");
        assert_eq!(d.relative, "V7");

        // Letter input in C major: relative is DERIVED (absolute -> Roman).
        let d = describe_chord_in_key("Dm", 0, "major").unwrap();
        assert_eq!(d.relative, "ii");
        let d = describe_chord_in_key("Cmaj7", 0, "major").unwrap();
        assert_eq!(d.rendered, "Cmaj7");
        assert_eq!(d.relative, "Imaj7"); // maj7 quality carried onto the numeral

        // Borrowed letter chord: chromatic root -> flat-of-above spelling.
        // A# = pitch class 10 = b7 in C major.
        let d = describe_chord_in_key("A#", 0, "major").unwrap();
        assert_eq!(d.relative, "bVII");

        // The same numeral transposes with the key: V7 in G major = D7.
        let d = describe_chord_in_key("V7", 7, "major").unwrap();
        assert_eq!(d.rendered, "D7");
        assert_eq!(d.relative, "V7");

        // Unparseable -> None.
        assert!(describe_chord_in_key("???", 0, "major").is_none());
    }

    #[test]
    fn test_rich_context_in_pattern() {
        let source = "\
@scale root=C mode=major
@harmony main
  | C | Am |
@pattern p unit=1/4
  ^1
  ^3
  ^5
  ^3
@track melody ch=1 follow=main
  play: p * 1
";
        let output = compile_with_ast(source).unwrap();
        let ctx = get_rich_context_at_cursor(source, 70, Some(&output));
        assert_eq!(ctx.block_type, Some(BlockType::Pattern));
        assert!(ctx.pattern_params.is_some());
        let params = ctx.pattern_params.unwrap();
        assert_eq!(params.name, "p");
        assert_eq!(params.steps, 4);
        assert_eq!(params.unit, (1, 4));
        assert!(ctx.current_scale.is_some());
        assert_eq!(ctx.current_scale.unwrap().name, "major");
    }

    #[test]
    fn test_complete_at_cursor_pattern() {
        let source = "@pattern p unit=1/4\n  ^1\n  ^3\n";
        let items = complete_at_cursor(source, 25);
        assert!(!items.is_empty());
        let kinds: Vec<CompletionKind> = items.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&CompletionKind::StepToken));
        assert!(kinds.contains(&CompletionKind::Annotation));
    }

    #[test]
    fn test_complete_at_cursor_toplevel() {
        let items = complete_at_cursor("", 0);
        assert!(!items.is_empty());
        let kinds: Vec<CompletionKind> = items.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&CompletionKind::Directive));
        assert!(kinds.contains(&CompletionKind::ScaleName));
    }

    #[test]
    fn test_complete_at_cursor_track_has_pattern_refs() {
        let source = "\
@pattern p unit=1/4
  ^1
  ^3
@track melody ch=1
  play: p * 1
";
        let items = complete_at_cursor(source, source.len() - 5);
        let pattern_refs: Vec<_> = items
            .iter()
            .filter(|i| i.kind == CompletionKind::PatternRef)
            .collect();
        assert!(!pattern_refs.is_empty());
        assert_eq!(pattern_refs[0].label, "p");
    }

    #[test]
    fn test_rich_context_without_compile_output() {
        let source = "@pattern p unit=1/4\n  ^1\n  ^3\n";
        let ctx = get_rich_context_at_cursor(source, 25, None);
        assert_eq!(ctx.block_type, Some(BlockType::Pattern));
        assert!(ctx.pattern_params.is_none()); // No compile output, no enrichment
    }
}
