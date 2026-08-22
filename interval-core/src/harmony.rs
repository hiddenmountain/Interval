//! Harmony timeline index.
//!
//! Builds an interval-based index from parsed `@harmony` blocks so that the
//! compiler can efficiently query "what chord is active at tick T?" The index
//! handles bar-level chords, beat-assigned chords, `steps:` block subdivisions,
//! and `section:` modulation directives.
//!
//! The harmony index is built *after* the header is fully parsed, because step
//! durations depend on `@ppq` and `@bpm`.

use crate::ast::{
    Bar, ChordSymbol, GlobalHeader, HarmonyBlock, RomanNumeralDegree, ScaleBlock, Section,
    TonalContext,
};
use crate::error::{CompileError, CompileResult, Span};
use serde::Serialize;

// ── Mode / Scale Tables ──────────────────────────────────────────────

/// Scale/mode intervals (semitones from root).
#[derive(Debug, Clone, Serialize)]
pub struct ScaleMode {
    /// Mode name.
    pub name: &'static str,
    /// Intervals in semitones from root.
    pub intervals: &'static [u8],
}

/// All supported modes with their interval tables.
pub static MODES: &[ScaleMode] = &[
    ScaleMode {
        name: "major",
        intervals: &[0, 2, 4, 5, 7, 9, 11],
    },
    ScaleMode {
        name: "ionian",
        intervals: &[0, 2, 4, 5, 7, 9, 11],
    },
    ScaleMode {
        name: "dorian",
        intervals: &[0, 2, 3, 5, 7, 9, 10],
    },
    ScaleMode {
        name: "phrygian",
        intervals: &[0, 1, 3, 5, 7, 8, 10],
    },
    ScaleMode {
        name: "lydian",
        intervals: &[0, 2, 4, 6, 7, 9, 11],
    },
    ScaleMode {
        name: "mixolydian",
        intervals: &[0, 2, 4, 5, 7, 9, 10],
    },
    ScaleMode {
        name: "aeolian",
        intervals: &[0, 2, 3, 5, 7, 8, 10],
    },
    ScaleMode {
        name: "minor",
        intervals: &[0, 2, 3, 5, 7, 8, 10],
    },
    ScaleMode {
        name: "locrian",
        intervals: &[0, 1, 3, 5, 6, 8, 10],
    },
    ScaleMode {
        name: "melodic_minor",
        intervals: &[0, 2, 3, 5, 7, 9, 11],
    },
    ScaleMode {
        name: "harmonic_minor",
        intervals: &[0, 2, 3, 5, 7, 8, 11],
    },
    ScaleMode {
        name: "whole_tone",
        intervals: &[0, 2, 4, 6, 8, 10],
    },
    ScaleMode {
        name: "diminished",
        intervals: &[0, 2, 3, 5, 6, 8, 9, 11],
    },
    ScaleMode {
        name: "chromatic",
        intervals: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
    },
    // Pentatonic / blues
    ScaleMode {
        name: "pentatonic_major",
        intervals: &[0, 2, 4, 7, 9],
    },
    ScaleMode {
        name: "pentatonic_minor",
        intervals: &[0, 3, 5, 7, 10],
    },
    ScaleMode {
        name: "blues",
        intervals: &[0, 3, 5, 6, 7, 10],
    },
    // Modal variants
    ScaleMode {
        name: "phrygian_dominant",
        intervals: &[0, 1, 4, 5, 7, 8, 10],
    },
    ScaleMode {
        name: "lydian_dominant",
        intervals: &[0, 2, 4, 6, 7, 9, 10],
    },
    ScaleMode {
        name: "altered",
        intervals: &[0, 1, 3, 4, 6, 8, 10],
    },
    ScaleMode {
        name: "harmonic_major",
        intervals: &[0, 2, 4, 5, 7, 8, 11],
    },
    ScaleMode {
        name: "double_harmonic",
        intervals: &[0, 1, 4, 5, 7, 8, 11],
    },
    // Melodic minor modes (missing modes 2, 3, 5, 6)
    ScaleMode {
        name: "dorian_b2",
        intervals: &[0, 1, 3, 5, 7, 9, 10],
    },
    ScaleMode {
        name: "lydian_augmented",
        intervals: &[0, 2, 4, 6, 8, 9, 11],
    },
    ScaleMode {
        name: "mixolydian_b6",
        intervals: &[0, 2, 4, 5, 7, 8, 10],
    },
    ScaleMode {
        name: "locrian_nat2",
        intervals: &[0, 2, 3, 5, 6, 8, 10],
    },
    // Symmetric
    ScaleMode {
        name: "diminished_half_whole",
        intervals: &[0, 1, 3, 4, 6, 7, 9, 10],
    },
    // Bebop scales
    ScaleMode {
        name: "bebop_dominant",
        intervals: &[0, 2, 4, 5, 7, 9, 10, 11],
    },
    ScaleMode {
        name: "bebop_major",
        intervals: &[0, 2, 4, 5, 7, 8, 9, 11],
    },
    ScaleMode {
        name: "bebop_dorian",
        intervals: &[0, 2, 3, 4, 5, 7, 9, 10],
    },
    // World / Exotic
    ScaleMode {
        name: "hungarian_minor",
        intervals: &[0, 2, 3, 6, 7, 8, 11],
    },
    ScaleMode {
        name: "neapolitan_major",
        intervals: &[0, 1, 3, 5, 7, 9, 11],
    },
    ScaleMode {
        name: "neapolitan_minor",
        intervals: &[0, 1, 3, 5, 7, 8, 11],
    },
    ScaleMode {
        name: "hirajoshi",
        intervals: &[0, 2, 3, 7, 8],
    },
    ScaleMode {
        name: "in_sen",
        intervals: &[0, 1, 5, 7, 10],
    },
    ScaleMode {
        name: "iwato",
        intervals: &[0, 1, 5, 6, 10],
    },
    // Symmetric / Modern
    ScaleMode {
        name: "augmented_scale",
        intervals: &[0, 3, 4, 7, 8, 11],
    },
    ScaleMode {
        name: "tritone_scale",
        intervals: &[0, 1, 4, 6, 7, 10],
    },
    ScaleMode {
        name: "prometheus",
        intervals: &[0, 2, 4, 6, 9, 10],
    },
    // Aliases
    ScaleMode {
        name: "super_locrian",
        intervals: &[0, 1, 3, 4, 6, 8, 10],
    },
];

/// Look up a mode by name. Returns the interval table.
pub fn lookup_mode(name: &str) -> Option<&'static [u8]> {
    MODES.iter().find(|m| m.name == name).map(|m| m.intervals)
}

// ── Chord Symbol Parsing ─────────────────────────────────────────────

/// Parse a note name to a pitch class (0=C, 1=C#/Db, ... 11=B).
pub fn parse_note_name(name: &str) -> Option<(u8, usize)> {
    let bytes = name.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let (base, consumed) = match bytes[0] {
        b'C' => (0u8, 1),
        b'D' => (2, 1),
        b'E' => (4, 1),
        b'F' => (5, 1),
        b'G' => (7, 1),
        b'A' => (9, 1),
        b'B' => (11, 1),
        _ => return None,
    };

    // Check for accidental
    if consumed < bytes.len() {
        match bytes[consumed] {
            b'#' => Some(((base + 1) % 12, consumed + 1)),
            b'b' => Some(((base + 11) % 12, consumed + 1)),
            _ => Some((base, consumed)),
        }
    } else {
        Some((base, consumed))
    }
}

// ── Roman Numeral Parsing ────────────────────────────────────────────

/// Try to parse a Roman numeral root from the start of a string.
///
/// Recognizes I–VII in upper or lowercase, with optional `b`/`#` prefix for
/// borrowed chords. Returns `(degree_index, is_minor, accidental, consumed)`:
/// - `degree_index`: 0-6 mapping to scale degrees 1-7
/// - `is_minor`: true if lowercase numeral (minor implicit quality)
/// - `accidental`: -1 for flat, 0 for natural, 1 for sharp
/// - `consumed`: bytes consumed from input
fn parse_roman_numeral_root(input: &str) -> Option<(usize, bool, i8, usize)> {
    let bytes = input.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    // Check for b/# prefix (borrowed chord)
    let (accidental, start) = match bytes[0] {
        b'b' if bytes.len() > 1 => {
            // Only treat as accidental if followed by uppercase Roman numeral
            // (lowercase 'b' followed by lowercase could be ambiguous with note 'b')
            let next = bytes[1];
            if next == b'I' || next == b'V' {
                (-1i8, 1)
            } else {
                return None;
            }
        }
        b'#' if bytes.len() > 1 => (1i8, 1),
        _ => (0i8, 0),
    };

    let remaining = &input[start..];

    // Try longest match first for Roman numerals
    // Must check longer numerals before shorter ones (VII before VI before V, etc.)
    let roman_table: &[(&str, &str, usize)] = &[
        // (uppercase, lowercase, degree_index)
        ("VII", "vii", 6),
        ("VI", "vi", 5),
        ("IV", "iv", 3),
        ("V", "v", 4),
        ("III", "iii", 2),
        ("II", "ii", 1),
        ("I", "i", 0),
    ];

    for &(upper, lower, degree) in roman_table {
        if remaining.starts_with(upper) {
            // Uppercase — major
            return Some((degree, false, accidental, start + upper.len()));
        }
        if let Some(after) = remaining.strip_prefix(lower) {
            // Verify this isn't a prefix of a longer identifier
            // e.g., "inv" should not match "i" + "nv"
            if !after.is_empty() {
                let next = after.as_bytes()[0];
                // Valid quality suffix starts: m, d, a, s, o (for dim, aug, sus, dim7...)
                if next.is_ascii_lowercase() && !matches!(next, b'm' | b'd' | b'a' | b's' | b'o') {
                    continue;
                }
            }
            // Lowercase — minor
            return Some((degree, true, accidental, start + lower.len()));
        }
    }

    None
}

// ── Diatonic Quality Inference (Phase 5 / §2) ───────────────────────

/// Diatonic triad quality for a scale degree.
#[derive(Debug, Clone, Copy, PartialEq)]
enum DiatonicQuality {
    Major,
    Minor,
    Diminished,
    Augmented,
}

/// Modes that support diatonic quality inference (heptatonic, well-defined triads).
/// Modes NOT in this list require explicit quality suffixes.
fn supports_diatonic_inference(mode: &str) -> bool {
    matches!(
        mode,
        "major"
            | "ionian"
            | "dorian"
            | "phrygian"
            | "lydian"
            | "mixolydian"
            | "aeolian"
            | "minor"
            | "locrian"
            | "melodic_minor"
            | "harmonic_minor"
            | "phrygian_dominant"
            | "lydian_dominant"
    )
}

/// Look up the diatonic triad quality for a given mode at 0-indexed degree (0=I ... 6=VII).
///
/// Returns `None` when the mode does not support diatonic inference.
fn diatonic_triad_quality(mode: &str, degree_idx: usize) -> Option<DiatonicQuality> {
    use DiatonicQuality::*;
    // Table order: I ii iii IV V vi vii (degree_idx 0..6)
    let qualities: Option<[DiatonicQuality; 7]> = match mode {
        "major" | "ionian" => Some([Major, Minor, Minor, Major, Major, Minor, Diminished]),
        "dorian" => Some([Minor, Minor, Major, Major, Minor, Diminished, Major]),
        "phrygian" => Some([Minor, Major, Major, Minor, Diminished, Major, Minor]),
        "lydian" => Some([Major, Major, Minor, Diminished, Major, Minor, Minor]),
        "mixolydian" => Some([Major, Minor, Diminished, Major, Minor, Minor, Major]),
        "aeolian" | "minor" => Some([Minor, Diminished, Major, Minor, Minor, Major, Major]),
        "locrian" => Some([Diminished, Major, Minor, Minor, Major, Major, Minor]),
        "melodic_minor" => Some([
            Minor, Minor, Augmented, Major, Major, Diminished, Diminished,
        ]),
        "harmonic_minor" => Some([
            Minor, Diminished, Augmented, Minor, Major, Major, Diminished,
        ]),
        // Exclude non-heptatonic and exotic modes from inference
        _ => None,
    };

    qualities.map(|q| q[degree_idx.min(6)])
}

/// Build diatonic chord intervals for a given degree, mode, and numeric extension.
///
/// `numeric` is 3 (triad only), 7, 9, 11, or 13. The 7th is 6 scale steps above
/// the root, the 9th is 8 steps, 11th is 10 steps, 13th is 12 steps.
fn build_diatonic_intervals(
    mode_intervals: &[u8],
    degree_idx: usize,
    triad_quality: DiatonicQuality,
    numeric: u8,
) -> Vec<u8> {
    use DiatonicQuality::*;

    // Diatonic triad intervals (these may differ from case-implied quality)
    let mut intervals = match triad_quality {
        Major => vec![0u8, 4, 7],
        Minor => vec![0, 3, 7],
        Diminished => vec![0, 3, 6],
        Augmented => vec![0, 4, 8],
    };

    if numeric <= 3 || mode_intervals.len() < 7 {
        return intervals;
    }

    // Compute the diatonic semitone interval N scale steps above the chord root.
    // - 7th  → 6 scale steps above root
    // - 9th  → 8 scale steps (= 2nd + one octave)
    // - 11th → 10 scale steps (= 4th + one octave)
    // - 13th → 12 scale steps (= 6th + one octave)
    let diatonic_interval = |scale_steps: usize| -> u8 {
        let target = (degree_idx + scale_steps) % 7;
        let octave_shift = (degree_idx + scale_steps) / 7;
        let interval = mode_intervals[target] as i16 - mode_intervals[degree_idx] as i16
            + (octave_shift as i16) * 12;
        interval.max(0) as u8
    };

    // 7th — 6 scale steps above root
    if numeric >= 7 {
        let seventh = diatonic_interval(6);
        if !intervals.contains(&seventh) {
            intervals.push(seventh);
        }
    }

    // 9th — 8 scale steps above root (2nd + octave)
    if numeric >= 9 {
        let ninth = diatonic_interval(8);
        if !intervals.contains(&ninth) {
            intervals.push(ninth);
        }
    }

    // 11th — 10 scale steps above root (4th + octave)
    if numeric >= 11 {
        let eleventh = diatonic_interval(10);
        if !intervals.contains(&eleventh) {
            intervals.push(eleventh);
        }
    }

    // 13th — 12 scale steps above root (6th + octave)
    if numeric >= 13 {
        let thirteenth = diatonic_interval(12);
        if !intervals.contains(&thirteenth) {
            intervals.push(thirteenth);
        }
    }

    intervals
}

/// The diatonic triad a bare Roman numeral receives at parse time under the
/// mode with these intervals, or `None` when the mode does not support
/// diatonic inference (then quality was case-implied or explicit and must
/// not be re-derived).
///
/// Used by `HarmonyIndex::build` to re-derive *implicit* Roman-numeral
/// qualities across `@scale` mode changes: replicates the parse-time path
/// (`diatonic_triad_quality` + `build_diatonic_intervals` with numeric=3)
/// exactly, keyed by the interval set since the compiled scale timeline
/// stores intervals, not mode names.
fn diatonic_triad_for_intervals(mode_intervals: &[u8], degree_idx: usize) -> Option<Vec<u8>> {
    let mode_name = MODES.iter().find(|m| m.intervals == mode_intervals)?.name;
    if !supports_diatonic_inference(mode_name) {
        return None;
    }
    let quality = diatonic_triad_quality(mode_name, degree_idx)?;
    Some(build_diatonic_intervals(
        mode_intervals,
        degree_idx,
        quality,
        3,
    ))
}

/// If `s` is a numeric-only quality suffix (7, 9, 11, or 13), return the number.
fn numeric_only_suffix(s: &str) -> Option<u8> {
    match s {
        "7" => Some(7),
        "9" => Some(9),
        "11" => Some(11),
        "13" => Some(13),
        _ => None,
    }
}

/// Parse a chord symbol that may be a Roman numeral, resolving against a tonal context.
///
/// If the input starts with a Roman numeral (I–VII, i–vii, with optional b/# prefix),
/// it is resolved to an absolute pitch class using the tonal context's root and mode.
/// Otherwise, delegates to `parse_chord_symbol()` for letter-based parsing.
pub fn parse_chord_symbol_with_context(
    input: &str,
    tonal_context: &TonalContext,
) -> CompileResult<ChordSymbol> {
    if let Some((degree_idx, is_minor, accidental, consumed)) = parse_roman_numeral_root(input) {
        let mode_intervals =
            lookup_mode(&tonal_context.mode).ok_or_else(|| CompileError::ParseError {
                message: format!("unknown mode '{}' in tonal context", tonal_context.mode),
                span: tonal_context.span.unwrap_or_else(|| Span::new(0, 0)),
            })?;

        let scale_root = tonal_context.root.unwrap_or(0);

        // Resolve root pitch class: scale_root + mode_interval[degree] + accidental
        let degree_semitones = if degree_idx < mode_intervals.len() {
            mode_intervals[degree_idx]
        } else {
            // Shouldn't happen for degrees 0-6, but be safe
            0
        };
        let root = ((scale_root as i16 + degree_semitones as i16 + accidental as i16) % 12 + 12)
            as u8
            % 12;

        let remaining = &input[consumed..];

        // Check for slash chord
        let (quality_part, slash_bass) = if let Some(slash_pos) = remaining.rfind('/') {
            let bass_str = &remaining[slash_pos + 1..];
            // Slash bass could be a letter note or a Roman numeral
            let bass = if let Some((bass_degree, _, bass_acc, _)) =
                parse_roman_numeral_root(bass_str)
            {
                let bass_semitones = if bass_degree < mode_intervals.len() {
                    mode_intervals[bass_degree]
                } else {
                    0
                };
                ((scale_root as i16 + bass_semitones as i16 + bass_acc as i16) % 12 + 12) as u8 % 12
            } else {
                parse_note_name(bass_str).map(|(pc, _)| pc).ok_or_else(|| {
                    CompileError::ParseError {
                        message: format!("invalid slash bass note in '{input}'"),
                        span: tonal_context.span.unwrap_or_else(|| Span::new(0, 0)),
                    }
                })?
            };
            (&remaining[..slash_pos], Some(bass))
        } else {
            (remaining, None)
        };

        // Diatonic triad quality for this degree and mode (Phase 5).
        // Only applies to natural (no accidental) Roman numerals — borrowed chords
        // (bVII, #IV, etc.) fall back to case-implied behavior.
        let diatonic_quality = if accidental == 0 {
            diatonic_triad_quality(&tonal_context.mode, degree_idx)
        } else {
            None
        };
        let use_diatonic = accidental == 0 && supports_diatonic_inference(&tonal_context.mode);

        // Parse quality suffix — with diatonic inference for Roman numerals
        let (mut intervals, after_quality) = if quality_part.is_empty() {
            // Bare Roman numeral — apply diatonic triad quality if mode supports it
            let triads = if use_diatonic {
                if let Some(q) = diatonic_quality {
                    build_diatonic_intervals(mode_intervals, degree_idx, q, 3)
                } else {
                    // Case-implied fallback
                    if is_minor {
                        vec![0, 3, 7]
                    } else {
                        vec![0, 4, 7]
                    }
                }
            } else {
                // Non-heptatonic / exotic mode — case-implied
                if is_minor {
                    vec![0, 3, 7]
                } else {
                    vec![0, 4, 7]
                }
            };
            (triads, 0)
        } else if use_diatonic {
            // Check if quality part is a numeric-only suffix
            if let Some(numeric) = numeric_only_suffix(quality_part) {
                // Numeric suffix: diatonic extension for minor/diminished, absolute for major/augmented
                let quality = diatonic_quality.unwrap_or(if is_minor {
                    DiatonicQuality::Minor
                } else {
                    DiatonicQuality::Major
                });
                use DiatonicQuality::{Diminished, Minor};
                if matches!(quality, Minor | Diminished) {
                    // Diatonic extension
                    (
                        build_diatonic_intervals(mode_intervals, degree_idx, quality, numeric),
                        quality_part.len(),
                    )
                } else {
                    // Major or augmented — absolute suffix (existing behavior)
                    let (intervals, consumed) = parse_chord_quality(quality_part);
                    (intervals, consumed)
                }
            } else {
                // Explicit quality suffix — absolute override (unchanged behavior)
                let (intervals, consumed) = parse_chord_quality(quality_part);
                if consumed == 0 {
                    if is_minor {
                        (vec![0, 3, 7], 0)
                    } else {
                        (vec![0, 4, 7], 0)
                    }
                } else {
                    (intervals, consumed)
                }
            }
        } else {
            // Mode doesn't support diatonic inference — absolute quality (unchanged)
            let (intervals, consumed) = parse_chord_quality(quality_part);
            if consumed == 0 {
                if is_minor {
                    (vec![0, 3, 7], 0)
                } else {
                    (vec![0, 4, 7], 0)
                }
            } else {
                (intervals, consumed)
            }
        };

        // Parse alterations from remaining quality string
        let alt_str = &quality_part[after_quality..];
        parse_alterations(alt_str, &mut intervals)?;

        Ok(ChordSymbol {
            root,
            intervals,
            slash_bass,
            // Store the raw Roman numeral degree so the compiler can re-resolve the
            // root bar-by-bar against the @scale timeline.
            roman: Some(RomanNumeralDegree {
                degree_idx: degree_idx as u8,
                accidental,
            }),
        })
    } else {
        // Not a Roman numeral — use standard letter-based parsing
        parse_chord_symbol(input)
    }
}

/// Parse a chord symbol string (e.g., "Cmaj7", "Dm7b5", "G7#9", "Ab/Eb").
///
/// Returns a `ChordSymbol` with root pitch class, intervals, and optional slash bass.
pub fn parse_chord_symbol(input: &str) -> CompileResult<ChordSymbol> {
    let original = input;
    let mut remaining = input;

    // Parse root note
    let (root, consumed) = parse_note_name(remaining).ok_or_else(|| CompileError::ParseError {
        message: format!("invalid chord root in '{original}'"),
        span: Span::new(0, 0),
    })?;
    remaining = &remaining[consumed..];

    // Check for slash chord — split off bass note.
    // Only treat `/X` as slash bass when X is a valid note name (A-G with optional b/#).
    // Otherwise the slash is part of the quality suffix (e.g., "6/9").
    let (quality_part, slash_bass) = if let Some(slash_pos) = remaining.rfind('/') {
        let bass_str = &remaining[slash_pos + 1..];
        if let Some((pc, _)) = parse_note_name(bass_str) {
            (&remaining[..slash_pos], Some(pc))
        } else {
            // Not a valid note name — treat as quality (e.g., "6/9" extension).
            (remaining, None)
        }
    } else {
        (remaining, None)
    };

    // Parse quality and alterations
    let (mut intervals, after_quality) = parse_chord_quality(quality_part);

    // Parse alterations from the remaining part
    let alt_str = &quality_part[after_quality..];
    parse_alterations(alt_str, &mut intervals)?;

    Ok(ChordSymbol {
        root,
        intervals,
        slash_bass,
        roman: None, // letter-based: root is absolute, no re-resolution needed
    })
}

/// Parse the chord quality portion and return (intervals, bytes_consumed).
fn parse_chord_quality(s: &str) -> (Vec<u8>, usize) {
    // Try longest matches first
    let quality_table: &[(&str, &[u8])] = &[
        // Extended chords (must come before shorter matches)
        ("maj13", &[0, 4, 7, 11, 14, 17, 21]),
        ("maj11", &[0, 4, 7, 11, 14, 17]),
        ("maj9", &[0, 4, 7, 11, 14]),
        ("maj7", &[0, 4, 7, 11]),
        ("min9", &[0, 3, 7, 10, 14]),
        ("min7", &[0, 3, 7, 10]),
        ("min", &[0, 3, 7]),
        ("mMaj9", &[0, 3, 7, 11, 14]),
        ("mMaj7", &[0, 3, 7, 11]),
        ("mM9", &[0, 3, 7, 11, 14]),
        ("mM7", &[0, 3, 7, 11]),
        ("m7b5", &[0, 3, 6, 10]),
        ("m(add9)", &[0, 3, 7, 14]),
        ("madd9", &[0, 3, 7, 14]),
        ("m13", &[0, 3, 7, 10, 14, 17, 21]),
        ("m11", &[0, 3, 7, 10, 14, 17]),
        ("m9", &[0, 3, 7, 10, 14]),
        ("m7", &[0, 3, 7, 10]),
        ("m6/9", &[0, 3, 7, 9, 14]),
        ("m6", &[0, 3, 7, 9]),
        ("m", &[0, 3, 7]),
        ("M9", &[0, 4, 7, 11, 14]),
        ("M7", &[0, 4, 7, 11]),
        ("add11", &[0, 4, 7, 17]),
        ("add9", &[0, 4, 7, 14]),
        ("add2", &[0, 2, 4, 7]),
        ("augmaj7", &[0, 4, 8, 11]),
        ("aug7", &[0, 4, 8, 10]),
        ("aug", &[0, 4, 8]),
        ("dim7", &[0, 3, 6, 9]),
        ("dim", &[0, 3, 6]),
        ("sus2", &[0, 2, 7]),
        ("sus4", &[0, 5, 7]),
        ("13", &[0, 4, 7, 10, 14, 17, 21]),
        ("11", &[0, 4, 7, 10, 14, 17]),
        ("9sus4", &[0, 5, 7, 10, 14]),
        ("9", &[0, 4, 7, 10, 14]),
        ("7sus4", &[0, 5, 7, 10]),
        ("7sus2", &[0, 2, 7, 10]),
        ("7sus", &[0, 5, 7, 10]),
        ("7", &[0, 4, 7, 10]),
        ("6/9", &[0, 4, 7, 9, 14]),
        ("6", &[0, 4, 7, 9]),
        ("5", &[0, 7]),
    ];

    // Check for dash as minor
    if s.starts_with('-') {
        // Check for -7
        if s.starts_with("-7") {
            return (vec![0, 3, 7, 10], 2);
        }
        return (vec![0, 3, 7], 1);
    }

    for (pattern, intervals) in quality_table {
        if s.starts_with(pattern) {
            return (intervals.to_vec(), pattern.len());
        }
    }

    // No quality matched — major triad
    (vec![0, 4, 7], 0)
}

/// Parse alteration suffixes (b5, #5, b9, #9, #11, b13) and modify intervals.
fn parse_alterations(s: &str, intervals: &mut Vec<u8>) -> CompileResult<()> {
    let mut remaining = s;

    while !remaining.is_empty() {
        let (alt_semitones, consumed) = if remaining.starts_with("b5") {
            // Flat fifth — replace or add 6 semitones
            replace_or_add(intervals, 7, 6);
            (None, 2)
        } else if remaining.starts_with("#5") {
            // Sharp fifth — replace or add 8 semitones
            replace_or_add(intervals, 7, 8);
            (None, 2)
        } else if remaining.starts_with("b9") {
            (Some(13u8), 2)
        } else if remaining.starts_with("#9") {
            (Some(15), 2)
        } else if remaining.starts_with("#11") {
            (Some(18), 3)
        } else if remaining.starts_with("b13") {
            (Some(20), 3)
        } else {
            return Err(CompileError::ParseError {
                message: format!("unrecognized chord alteration: '{remaining}'"),
                span: Span::new(0, 0),
            });
        };

        if let Some(semitones) = alt_semitones {
            if !intervals.contains(&semitones) {
                intervals.push(semitones);
            }
        }

        remaining = &remaining[consumed..];
    }

    Ok(())
}

/// Replace an interval value in the vec, or add the new value if the old isn't present.
fn replace_or_add(intervals: &mut Vec<u8>, old: u8, new: u8) {
    if let Some(pos) = intervals.iter().position(|&v| v == old) {
        intervals[pos] = new;
    } else {
        intervals.push(new);
    }
}

// ── Scale Timeline ───────────────────────────────────────────────────

/// A single entry in the `ScaleTimeline`.
struct ScaleTimelineEntry {
    /// 1-indexed bar where this entry takes effect (first entry is always bar 1).
    start_bar: u32,
    /// Scale root pitch class (0=C … 11=B).
    scale_root: u8,
    /// Mode intervals (semitones from root).
    mode_intervals: Vec<u8>,
}

/// The compiled `@scale` timeline — maps bar ranges to tonal contexts.
///
/// Built from either a `ScaleBlock` (timeline form) or a `TonalContext` (scalar form).
/// `HarmonyIndex::build()` uses this to re-resolve Roman numeral chord roots per-bar.
pub struct ScaleTimeline {
    entries: Vec<ScaleTimelineEntry>,
}

impl ScaleTimeline {
    /// Build from a `@scale` timeline block (inline or block form).
    ///
    /// Fields not specified in an entry (`root=` or `mode=` absent) inherit from the
    /// previous entry, per spec §5.2. The very first entry falls back to C major if
    /// neither field is provided.
    pub fn from_scale_block(sb: &ScaleBlock) -> CompileResult<Self> {
        let mut entries = Vec::with_capacity(sb.entries.len());
        let mut current_bar: u32 = 1;
        // Inherit state carried forward across entries.
        let mut prev_root: u8 = 0; // C
        let mut prev_mode: String = "major".to_string();
        for e in &sb.entries {
            let effective_root = e.root.unwrap_or(prev_root);
            let effective_mode = e.mode.as_deref().unwrap_or(&prev_mode);
            let intervals = lookup_mode(effective_mode)
                .ok_or_else(|| CompileError::ParseError {
                    message: format!("unknown mode '{effective_mode}' in @scale timeline"),
                    span: e.span.unwrap_or_else(|| Span::new(0, 0)),
                })?
                .to_vec();
            entries.push(ScaleTimelineEntry {
                start_bar: current_bar,
                scale_root: effective_root,
                mode_intervals: intervals,
            });
            // Carry forward for next entry.
            prev_root = effective_root;
            prev_mode = effective_mode.to_string();
            if let Some(bars) = e.bars {
                current_bar += bars;
            }
            // If bars is None, this is the last entry — no advance needed.
        }
        if entries.is_empty() {
            return Ok(Self::default_c_major());
        }
        Ok(ScaleTimeline { entries })
    }

    /// Build a single-entry timeline from a scalar `TonalContext`.
    pub fn from_tonal_context(tc: &TonalContext) -> CompileResult<Self> {
        let intervals = lookup_mode(&tc.mode)
            .ok_or_else(|| CompileError::ParseError {
                message: format!("unknown mode '{}' in @scale", tc.mode),
                span: tc.span.unwrap_or_else(|| Span::new(0, 0)),
            })?
            .to_vec();
        Ok(ScaleTimeline {
            entries: vec![ScaleTimelineEntry {
                start_bar: 1,
                scale_root: tc.root.unwrap_or(0),
                mode_intervals: intervals,
            }],
        })
    }

    /// Single-entry C major fallback.
    pub fn default_c_major() -> Self {
        ScaleTimeline {
            entries: vec![ScaleTimelineEntry {
                start_bar: 1,
                scale_root: 0,
                mode_intervals: vec![0, 2, 4, 5, 7, 9, 11],
            }],
        }
    }

    /// Return `(mode_intervals, scale_root)` for the given 1-indexed bar number.
    ///
    /// Finds the last entry whose `start_bar <= bar`. The first entry always matches.
    pub fn context_at_bar(&self, bar: u32) -> (&[u8], u8) {
        // entries are sorted by start_bar ascending; find the last one ≤ bar
        let idx = self.entries.partition_point(|e| e.start_bar <= bar);
        let idx = if idx == 0 { 0 } else { idx - 1 };
        (
            &self.entries[idx].mode_intervals,
            self.entries[idx].scale_root,
        )
    }
}

/// Re-resolve a chord's root pitch class from its stored Roman numeral degree.
///
/// For letter-based chords (`chord.roman == None`), returns the existing `chord.root`
/// unchanged. For Roman numeral chords, re-computes the root against the given
/// `mode_intervals` and `scale_root` — enabling correct resolution across scale
/// timeline boundaries.
fn re_resolve_chord_root(chord: &ChordSymbol, mode_intervals: &[u8], scale_root: u8) -> u8 {
    if let Some(ref rn) = chord.roman {
        if (rn.degree_idx as usize) < mode_intervals.len() {
            let s = mode_intervals[rn.degree_idx as usize] as i16;
            ((scale_root as i16 + s + rn.accidental as i16) % 12 + 12) as u8 % 12
        } else {
            chord.root // non-heptatonic or out-of-range: keep existing root
        }
    } else {
        chord.root // letter-based: root is absolute
    }
}

// ── Harmony Index ────────────────────────────────────────────────────

/// The chord context active at a given tick position.
#[derive(Debug, Clone, Serialize)]
pub struct ChordContext {
    /// The active chord symbol.
    pub chord: ChordSymbol,
    /// The active mode/scale intervals.
    pub mode_intervals: Vec<u8>,
    /// The active scale root (pitch class, 0=C).
    pub scale_root: u8,
}

/// A span in the harmony timeline: chord context over a tick range.
#[derive(Debug, Clone, Serialize)]
pub struct HarmonySpan {
    /// Start tick (inclusive).
    pub start_tick: u64,
    /// End tick (exclusive).
    pub end_tick: u64,
    /// The chord context for this span.
    pub context: ChordContext,
}

/// The harmony index — a sorted list of non-overlapping spans.
///
/// Implemented as a simple sorted vec (sufficient for the expected number
/// of chord changes). An interval tree could be added later if needed for
/// performance, but the query is already O(log n) via binary search.
#[derive(Debug, Clone, Serialize)]
pub struct HarmonyIndex {
    /// Sorted, non-overlapping spans covering the entire harmony timeline.
    spans: Vec<HarmonySpan>,
    /// Total duration in ticks.
    pub total_ticks: u64,
    /// Name of the harmony block this index was built from.
    pub name: String,
}

impl HarmonyIndex {
    /// Query the chord context at a given tick position.
    ///
    /// Ticks beyond the harmony timeline wrap cyclically to the beginning,
    /// so that `rate=` tracks and conditional looping interact with harmony
    /// correctly without special cases.
    pub fn query(&self, tick: u64) -> Option<&ChordContext> {
        let effective_tick = if self.total_ticks > 0 {
            tick % self.total_ticks
        } else {
            tick
        };
        // Binary search for the span containing this tick
        let idx = self
            .spans
            .partition_point(|span| span.end_tick <= effective_tick);
        if idx < self.spans.len() && self.spans[idx].start_tick <= effective_tick {
            Some(&self.spans[idx].context)
        } else {
            None
        }
    }

    /// Get all spans (for testing/debugging).
    pub fn spans(&self) -> &[HarmonySpan] {
        &self.spans
    }

    /// Build a harmony index from a parsed harmony block, global header, and scale timeline.
    ///
    /// The scale timeline provides the base mode and scale root per bar from `@scale`.
    /// If no `@scale` is declared, the caller passes `ScaleTimeline::default_c_major()`.
    ///
    /// For Roman numeral chords (`chord.roman.is_some()`), the chord root is re-resolved
    /// bar-by-bar against the scale timeline, so that later scale timeline entries
    /// (e.g. `@scale root=C mode=major * 4 | root=G mode=major`) correctly affect
    /// Roman numeral resolution.
    pub fn build(
        block: &HarmonyBlock,
        header: &GlobalHeader,
        scale_timeline: &ScaleTimeline,
        bar_layout: &crate::compiler::BarLayout,
    ) -> CompileResult<Self> {
        let ppq = header.ppq as u64;

        // Build section contexts from deprecated `section:` directives.
        // These are validated against the first scale timeline entry's base mode/root.
        let (base_mode_first, base_root_first) = scale_timeline.context_at_bar(1);
        let section_contexts = build_section_contexts(
            &block.sections,
            &block.bars,
            base_mode_first,
            base_root_first,
        )?;

        let mut spans = Vec::new();
        let mut current_tick: u64 = 0;

        for (bar_idx, bar) in block.bars.iter().enumerate() {
            let bar_num = bar_idx as u32 + 1;
            let bar_start = current_tick;
            let this_bar_ticks = bar_layout.ticks_for_bar(bar_num);
            let bar_end = bar_start + this_bar_ticks;
            let (ts_num_this, ts_den_this) = bar_layout.ts_for_bar(bar_num);
            let ticks_per_beat = ppq * 4 / ts_den_this as u64;

            // Per-bar scale context: start from @scale timeline, then overlay any
            // deprecated section: override for this bar.
            let (scale_mode_ivs, scale_root_base) = scale_timeline.context_at_bar(bar_num);
            let (mode_intervals, scale_root) =
                find_section_context(&section_contexts, bar_num, scale_mode_ivs, scale_root_base);

            // Helper: build a ChordContext for a given chord, re-resolving Roman numeral
            // roots against the current bar's scale context.
            let make_chord_context = |chord: &ChordSymbol| -> ChordContext {
                let resolved_root = re_resolve_chord_root(chord, mode_intervals, scale_root);
                let mut resolved_chord = chord.clone();
                resolved_chord.root = resolved_root;
                // Re-derive IMPLICIT (diatonic) Roman-numeral qualities against
                // the mode active for THIS bar. All Roman chords are parsed
                // against the first @scale timeline entry; without this, a
                // mode change (major → minor) would keep the first mode's
                // diatonic quality frozen while only the root moved.
                //
                // "Implicit" is detected as: natural (no accidental) numeral
                // whose intervals exactly equal the parse-time diatonic TRIAD
                // for its degree. Explicit qualities (`V7`, `IIImaj7`) and
                // numeric extensions differ from that triad and keep their
                // written quality — only the root moves. (RomanNumeralDegree
                // stores no explicit-vs-derived flag, so `IIIm` written in a
                // mode whose diatonic triad on III is minor is
                // indistinguishable from bare `iii` and is also re-derived —
                // an accepted limitation.)
                if let Some(ref rn) = chord.roman {
                    if rn.accidental == 0 {
                        let deg = rn.degree_idx as usize;
                        if let Some(parse_triad) =
                            diatonic_triad_for_intervals(base_mode_first, deg)
                        {
                            if chord.intervals == parse_triad {
                                if let Some(bar_triad) =
                                    diatonic_triad_for_intervals(mode_intervals, deg)
                                {
                                    resolved_chord.intervals = bar_triad;
                                }
                            }
                        }
                    }
                }
                ChordContext {
                    chord: resolved_chord,
                    mode_intervals: mode_intervals.to_vec(),
                    scale_root,
                }
            };

            if let Some(ref step_chords) = bar.steps {
                // steps: block — subdivide the bar evenly among step chords
                let step_count = step_chords.len() as u64;
                if step_count > 0 {
                    let step_ticks = this_bar_ticks.checked_div(step_count).unwrap_or(0);
                    let remainder = this_bar_ticks.checked_rem(step_count).unwrap_or(0) as usize;

                    let mut step_tick = bar_start;
                    for (i, step_chord) in step_chords.iter().enumerate() {
                        // Last `remainder` steps each get +1 tick for bar-boundary continuity
                        let this_step_ticks = if i >= step_count as usize - remainder {
                            step_ticks + 1
                        } else {
                            step_ticks
                        };

                        spans.push(HarmonySpan {
                            start_tick: step_tick,
                            end_tick: step_tick + this_step_ticks,
                            context: make_chord_context(step_chord),
                        });
                        step_tick += this_step_ticks;
                    }
                }
            } else {
                // Normal bar — distribute chords across beats
                let has_explicit_beats = bar.chords.iter().any(|c| c.beats.is_some());

                if has_explicit_beats {
                    // Validate: beat assignments must sum to ts numerator for this bar
                    let beat_sum: u32 =
                        bar.chords.iter().map(|c| c.beats.unwrap_or(1) as u32).sum();
                    if beat_sum != ts_num_this as u32 {
                        return Err(CompileError::BeatAssignmentMismatch {
                            name: block
                                .name
                                .clone()
                                .unwrap_or_else(|| "<unnamed>".to_string()),
                            bar: (bar_idx + 1) as u32,
                            actual: beat_sum,
                            expected: ts_num_this,
                            span: block.span.unwrap_or_else(|| Span::new(0, 0)),
                        });
                    }
                    // Explicit beat assignment
                    let mut chord_tick = bar_start;
                    for bar_chord in &bar.chords {
                        let beats = bar_chord.beats.unwrap_or(1) as u64;
                        let chord_ticks = beats * ticks_per_beat;
                        spans.push(HarmonySpan {
                            start_tick: chord_tick,
                            end_tick: chord_tick + chord_ticks,
                            context: make_chord_context(&bar_chord.chord),
                        });
                        chord_tick += chord_ticks;
                    }
                } else {
                    // Even distribution
                    let chord_count = bar.chords.len() as u64;
                    if chord_count > 0 {
                        let ts_num_u64 = ts_num_this as u64;
                        // More chords than beats would give the excess
                        // chords zero-length spans that `query()` can never
                        // return — silently unplayable harmony. Error
                        // instead of silently dropping.
                        if chord_count > ts_num_u64 {
                            return Err(CompileError::ParseError {
                                message: format!(
                                    "bar {bar_num} has {chord_count} chords but only \
                                     {ts_num_this} beats; use steps: or explicit beat counts"
                                ),
                                span: bar.span.or(block.span).unwrap_or_else(|| Span::new(0, 0)),
                            });
                        }
                        let beats_per_chord = ts_num_u64.checked_div(chord_count).unwrap_or(0);
                        let mut extra_beats = ts_num_u64.checked_rem(chord_count).unwrap_or(0);
                        let mut chord_tick = bar_start;

                        for bar_chord in &bar.chords {
                            let this_beats = if extra_beats > 0 {
                                extra_beats -= 1;
                                beats_per_chord + 1
                            } else {
                                beats_per_chord
                            };
                            let chord_ticks = this_beats * ticks_per_beat;
                            spans.push(HarmonySpan {
                                start_tick: chord_tick,
                                end_tick: chord_tick + chord_ticks,
                                context: make_chord_context(&bar_chord.chord),
                            });
                            chord_tick += chord_ticks;
                        }
                    }
                }
            }

            current_tick = bar_end;
        }

        Ok(HarmonyIndex {
            spans,
            total_ticks: current_tick,
            name: block.name.clone().unwrap_or_default(),
        })
    }
}

/// Context for a section: mode intervals and scale root.
struct SectionContext {
    bar: u32,
    mode_intervals: Vec<u8>,
    scale_root: u8,
}

/// Build section contexts from section directives.
fn build_section_contexts(
    sections: &[Section],
    bars: &[Bar],
    base_mode: &[u8],
    default_root: u8,
) -> CompileResult<Vec<SectionContext>> {
    let total_bars = bars.len() as u32;
    let mut contexts: Vec<SectionContext> = Vec::new();
    let mut prev_bar = 0u32;

    for section in sections {
        // Validate strictly increasing bar numbers
        if section.bar <= prev_bar {
            return Err(CompileError::SectionBarNotIncreasing {
                name: String::new(), // filled by caller
                span: Span::new(0, 0),
            });
        }
        // Validate bar number within range
        if section.bar > total_bars {
            return Err(CompileError::SectionBarExceedsTotal {
                name: String::new(),
                bar: section.bar,
                span: Span::new(0, 0),
            });
        }

        let mode_intervals = if let Some(ref mode_name) = section.mode {
            lookup_mode(mode_name)
                .ok_or_else(|| CompileError::ParseError {
                    message: format!("unknown mode '{mode_name}'"),
                    span: Span::new(0, 0),
                })?
                .to_vec()
        } else if let Some(last) = contexts.last() {
            last.mode_intervals.clone()
        } else {
            base_mode.to_vec()
        };

        let scale_root = section.root.unwrap_or_else(|| {
            contexts
                .last()
                .map(|c| c.scale_root)
                .unwrap_or(default_root)
        });

        prev_bar = section.bar;
        contexts.push(SectionContext {
            bar: section.bar,
            mode_intervals,
            scale_root,
        });
    }

    Ok(contexts)
}

/// Find the active section context for a given bar number.
fn find_section_context<'a>(
    sections: &'a [SectionContext],
    bar: u32,
    base_mode: &'a [u8],
    default_root: u8,
) -> (&'a [u8], u8) {
    // Find the last section whose bar <= current bar
    let mut active_mode: &[u8] = base_mode;
    let mut active_root = default_root;

    for section in sections {
        if section.bar <= bar {
            active_mode = &section.mode_intervals;
            active_root = section.scale_root;
        } else {
            break;
        }
    }

    (active_mode, active_root)
}

// ── Chord symbol parsing for Unicode variants ────────────────────────

/// Parse a chord symbol that may use Unicode quality symbols (Δ, °, ø).
/// This handles the conversion from lexer tokens back to a parseable string.
pub fn parse_chord_from_tokens(root_str: &str, quality_suffix: &str) -> CompileResult<ChordSymbol> {
    let full = format!("{root_str}{quality_suffix}");
    parse_chord_symbol(&full)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Note name tests ──────────────────────────────────────────────

    #[test]
    fn test_parse_note_names() {
        assert_eq!(parse_note_name("C"), Some((0, 1)));
        assert_eq!(parse_note_name("C#"), Some((1, 2)));
        assert_eq!(parse_note_name("Db"), Some((1, 2)));
        assert_eq!(parse_note_name("D"), Some((2, 1)));
        assert_eq!(parse_note_name("E"), Some((4, 1)));
        assert_eq!(parse_note_name("Eb"), Some((3, 2)));
        assert_eq!(parse_note_name("F"), Some((5, 1)));
        assert_eq!(parse_note_name("F#"), Some((6, 2)));
        assert_eq!(parse_note_name("G"), Some((7, 1)));
        assert_eq!(parse_note_name("Ab"), Some((8, 2)));
        assert_eq!(parse_note_name("A"), Some((9, 1)));
        assert_eq!(parse_note_name("Bb"), Some((10, 2)));
        assert_eq!(parse_note_name("B"), Some((11, 1)));
    }

    // ── Roman numeral tests ─────────────────────────────────────────

    #[test]
    fn test_roman_numeral_root_parsing() {
        // Basic uppercase
        assert_eq!(parse_roman_numeral_root("I"), Some((0, false, 0, 1)));
        assert_eq!(parse_roman_numeral_root("II"), Some((1, false, 0, 2)));
        assert_eq!(parse_roman_numeral_root("III"), Some((2, false, 0, 3)));
        assert_eq!(parse_roman_numeral_root("IV"), Some((3, false, 0, 2)));
        assert_eq!(parse_roman_numeral_root("V"), Some((4, false, 0, 1)));
        assert_eq!(parse_roman_numeral_root("VI"), Some((5, false, 0, 2)));
        assert_eq!(parse_roman_numeral_root("VII"), Some((6, false, 0, 3)));

        // Basic lowercase
        assert_eq!(parse_roman_numeral_root("i"), Some((0, true, 0, 1)));
        assert_eq!(parse_roman_numeral_root("ii"), Some((1, true, 0, 2)));
        assert_eq!(parse_roman_numeral_root("vii"), Some((6, true, 0, 3)));

        // Accidentals
        assert_eq!(parse_roman_numeral_root("bVII"), Some((6, false, -1, 4)));
        assert_eq!(parse_roman_numeral_root("#IV"), Some((3, false, 1, 3)));

        // Not a Roman numeral
        assert!(parse_roman_numeral_root("C").is_none());
        assert!(parse_roman_numeral_root("Dm7").is_none());
    }

    #[test]
    fn test_roman_numeral_c_major() {
        let tc = TonalContext {
            root: Some(0),
            mode: "major".to_string(),
            span: None,
        };
        // I in C major = C major triad
        let chord = parse_chord_symbol_with_context("I", &tc).unwrap();
        assert_eq!(chord.root, 0); // C
        assert_eq!(chord.intervals, vec![0, 4, 7]); // major triad

        // V in C major = G major triad
        let chord = parse_chord_symbol_with_context("V", &tc).unwrap();
        assert_eq!(chord.root, 7); // G

        // IV in C major = F major triad
        let chord = parse_chord_symbol_with_context("IV", &tc).unwrap();
        assert_eq!(chord.root, 5); // F
    }

    #[test]
    fn test_roman_numeral_minor() {
        let tc = TonalContext {
            root: Some(0),
            mode: "major".to_string(),
            span: None,
        };
        // ii in C major = Dm (minor triad)
        let chord = parse_chord_symbol_with_context("ii", &tc).unwrap();
        assert_eq!(chord.root, 2); // D
        assert_eq!(chord.intervals, vec![0, 3, 7]); // minor triad

        // vi in C major = Am
        let chord = parse_chord_symbol_with_context("vi", &tc).unwrap();
        assert_eq!(chord.root, 9); // A
    }

    #[test]
    fn test_roman_numeral_with_quality() {
        let tc = TonalContext {
            root: Some(0),
            mode: "major".to_string(),
            span: None,
        };
        // Imaj7 in C major = Cmaj7
        let chord = parse_chord_symbol_with_context("Imaj7", &tc).unwrap();
        assert_eq!(chord.root, 0); // C
        assert_eq!(chord.intervals, vec![0, 4, 7, 11]); // maj7

        // IIm7 in C major = Dm7
        let chord = parse_chord_symbol_with_context("IIm7", &tc).unwrap();
        assert_eq!(chord.root, 2); // D
        assert_eq!(chord.intervals, vec![0, 3, 7, 10]); // m7

        // V7 in C major = G7
        let chord = parse_chord_symbol_with_context("V7", &tc).unwrap();
        assert_eq!(chord.root, 7); // G
        assert_eq!(chord.intervals, vec![0, 4, 7, 10]); // dom7
    }

    #[test]
    fn test_roman_numeral_borrowed() {
        let tc = TonalContext {
            root: Some(0),
            mode: "major".to_string(),
            span: None,
        };
        // bVII in C major = Bb major
        let chord = parse_chord_symbol_with_context("bVII", &tc).unwrap();
        assert_eq!(chord.root, 10); // Bb
        assert_eq!(chord.intervals, vec![0, 4, 7]); // major triad

        // bIII in C major = Eb major
        let chord = parse_chord_symbol_with_context("bIII", &tc).unwrap();
        assert_eq!(chord.root, 3); // Eb
    }

    #[test]
    fn test_roman_numeral_transposition() {
        // Same progression in C vs Eb
        let tc_c = TonalContext {
            root: Some(0),
            mode: "major".to_string(),
            span: None,
        };
        let tc_eb = TonalContext {
            root: Some(3),
            mode: "major".to_string(),
            span: None,
        };

        // I in C = C, I in Eb = Eb
        let c = parse_chord_symbol_with_context("I", &tc_c).unwrap();
        let eb = parse_chord_symbol_with_context("I", &tc_eb).unwrap();
        assert_eq!(c.root, 0);
        assert_eq!(eb.root, 3);

        // V in C = G, V in Eb = Bb
        let c = parse_chord_symbol_with_context("V", &tc_c).unwrap();
        let eb = parse_chord_symbol_with_context("V", &tc_eb).unwrap();
        assert_eq!(c.root, 7);
        assert_eq!(eb.root, 10);
    }

    #[test]
    fn test_roman_numeral_diminished() {
        let tc = TonalContext {
            root: Some(0),
            mode: "major".to_string(),
            span: None,
        };
        // viio7 in C major = Bdim7
        let chord = parse_chord_symbol_with_context("viidim7", &tc).unwrap();
        assert_eq!(chord.root, 11); // B
        assert_eq!(chord.intervals, vec![0, 3, 6, 9]); // dim7
    }

    #[test]
    fn test_roman_numeral_letter_fallback() {
        // Letter-based chords still work
        let tc = TonalContext {
            root: Some(0),
            mode: "major".to_string(),
            span: None,
        };
        let chord = parse_chord_symbol_with_context("Cmaj7", &tc).unwrap();
        assert_eq!(chord.root, 0);
        assert_eq!(chord.intervals, vec![0, 4, 7, 11]);
    }

    #[test]
    fn test_minor_major_seventh() {
        // mMaj7 — minor triad + major 7th
        let chord = parse_chord_symbol("CmMaj7").unwrap();
        assert_eq!(chord.root, 0);
        assert_eq!(chord.intervals, vec![0, 3, 7, 11]);

        // mM7 — alternate spelling
        let chord = parse_chord_symbol("AmM7").unwrap();
        assert_eq!(chord.root, 9);
        assert_eq!(chord.intervals, vec![0, 3, 7, 11]);

        // via Roman numeral: imMaj7 in C major = CmMaj7
        let tc = TonalContext {
            root: Some(0),
            mode: "major".to_string(),
            span: None,
        };
        let chord = parse_chord_symbol_with_context("imMaj7", &tc).unwrap();
        assert_eq!(chord.root, 0);
        assert_eq!(chord.intervals, vec![0, 3, 7, 11]);
    }

    // ── Chord quality tests ──────────────────────────────────────────

    #[test]
    fn test_major_triad() {
        let chord = parse_chord_symbol("C").unwrap();
        assert_eq!(chord.root, 0);
        assert_eq!(chord.intervals, vec![0, 4, 7]);
        assert!(chord.slash_bass.is_none());
    }

    #[test]
    fn test_minor_triad() {
        let chord = parse_chord_symbol("Am").unwrap();
        assert_eq!(chord.root, 9);
        assert_eq!(chord.intervals, vec![0, 3, 7]);
    }

    #[test]
    fn test_minor_dash() {
        let chord = parse_chord_symbol("A-").unwrap();
        assert_eq!(chord.intervals, vec![0, 3, 7]);
    }

    #[test]
    fn test_minor_min() {
        let chord = parse_chord_symbol("Amin").unwrap();
        assert_eq!(chord.intervals, vec![0, 3, 7]);
    }

    #[test]
    fn test_major_seventh() {
        let chord = parse_chord_symbol("Cmaj7").unwrap();
        assert_eq!(chord.root, 0);
        assert_eq!(chord.intervals, vec![0, 4, 7, 11]);
    }

    #[test]
    fn test_dominant_seventh() {
        let chord = parse_chord_symbol("G7").unwrap();
        assert_eq!(chord.root, 7);
        assert_eq!(chord.intervals, vec![0, 4, 7, 10]);
    }

    #[test]
    fn test_minor_seventh() {
        let chord = parse_chord_symbol("Dm7").unwrap();
        assert_eq!(chord.root, 2);
        assert_eq!(chord.intervals, vec![0, 3, 7, 10]);
    }

    #[test]
    fn test_dominant_ninth() {
        let chord = parse_chord_symbol("G9").unwrap();
        assert_eq!(chord.intervals, vec![0, 4, 7, 10, 14]);
    }

    #[test]
    fn test_major_ninth() {
        let chord = parse_chord_symbol("Cmaj9").unwrap();
        assert_eq!(chord.intervals, vec![0, 4, 7, 11, 14]);
    }

    #[test]
    fn test_dominant_thirteenth() {
        let chord = parse_chord_symbol("Bb13").unwrap();
        assert_eq!(chord.root, 10);
        assert_eq!(chord.intervals, vec![0, 4, 7, 10, 14, 17, 21]);
    }

    #[test]
    fn test_diminished_triad() {
        let chord = parse_chord_symbol("Bdim").unwrap();
        assert_eq!(chord.root, 11);
        assert_eq!(chord.intervals, vec![0, 3, 6]);
    }

    #[test]
    fn test_diminished_seventh() {
        let chord = parse_chord_symbol("Bdim7").unwrap();
        assert_eq!(chord.intervals, vec![0, 3, 6, 9]);
    }

    #[test]
    fn test_half_diminished() {
        let chord = parse_chord_symbol("Am7b5").unwrap();
        assert_eq!(chord.root, 9);
        assert_eq!(chord.intervals, vec![0, 3, 6, 10]);
    }

    #[test]
    fn test_augmented() {
        let chord = parse_chord_symbol("Caug").unwrap();
        assert_eq!(chord.intervals, vec![0, 4, 8]);
    }

    #[test]
    fn test_sus2() {
        let chord = parse_chord_symbol("Csus2").unwrap();
        assert_eq!(chord.intervals, vec![0, 2, 7]);
    }

    #[test]
    fn test_sus4() {
        let chord = parse_chord_symbol("Csus4").unwrap();
        assert_eq!(chord.intervals, vec![0, 5, 7]);
    }

    #[test]
    fn test_add9() {
        let chord = parse_chord_symbol("Cadd9").unwrap();
        assert_eq!(chord.intervals, vec![0, 4, 7, 14]);
    }

    #[test]
    fn test_major_sixth() {
        let chord = parse_chord_symbol("C6").unwrap();
        assert_eq!(chord.intervals, vec![0, 4, 7, 9]);
    }

    #[test]
    fn test_minor_sixth() {
        let chord = parse_chord_symbol("Cm6").unwrap();
        assert_eq!(chord.intervals, vec![0, 3, 7, 9]);
    }

    // ── Alteration tests ─────────────────────────────────────────────

    #[test]
    fn test_dominant_flat_nine() {
        let chord = parse_chord_symbol("G7b9").unwrap();
        assert_eq!(chord.root, 7);
        assert!(chord.intervals.contains(&0));
        assert!(chord.intervals.contains(&4));
        assert!(chord.intervals.contains(&7));
        assert!(chord.intervals.contains(&10));
        assert!(chord.intervals.contains(&13)); // b9
    }

    #[test]
    fn test_dominant_sharp_nine() {
        let chord = parse_chord_symbol("G7#9").unwrap();
        assert!(chord.intervals.contains(&15)); // #9
    }

    #[test]
    fn test_sharp_eleven() {
        let chord = parse_chord_symbol("G7#11").unwrap();
        assert!(chord.intervals.contains(&18)); // #11
    }

    #[test]
    fn test_multiple_alterations() {
        let chord = parse_chord_symbol("G7b9#11").unwrap();
        assert!(chord.intervals.contains(&10)); // 7
        assert!(chord.intervals.contains(&13)); // b9
        assert!(chord.intervals.contains(&18)); // #11
    }

    #[test]
    fn test_flat_five() {
        let chord = parse_chord_symbol("G7b5").unwrap();
        // 5th (7 semitones) should be replaced by b5 (6 semitones)
        assert!(chord.intervals.contains(&6));
        assert!(!chord.intervals.contains(&7));
    }

    // ── Slash chord tests ────────────────────────────────────────────

    #[test]
    fn test_slash_chord() {
        let chord = parse_chord_symbol("C/E").unwrap();
        assert_eq!(chord.root, 0);
        assert_eq!(chord.intervals, vec![0, 4, 7]);
        assert_eq!(chord.slash_bass, Some(4)); // E
    }

    #[test]
    fn test_slash_chord_flat_bass() {
        let chord = parse_chord_symbol("Cmaj7/Bb").unwrap();
        assert_eq!(chord.root, 0);
        assert_eq!(chord.slash_bass, Some(10)); // Bb
    }

    // ── Flat-key chord tests ─────────────────────────────────────────

    #[test]
    fn test_flat_root_chords() {
        let chord = parse_chord_symbol("Ebmaj7").unwrap();
        assert_eq!(chord.root, 3); // Eb
        assert_eq!(chord.intervals, vec![0, 4, 7, 11]);

        let chord = parse_chord_symbol("Abmaj7").unwrap();
        assert_eq!(chord.root, 8); // Ab

        let chord = parse_chord_symbol("Bbm7").unwrap();
        assert_eq!(chord.root, 10); // Bb
        assert_eq!(chord.intervals, vec![0, 3, 7, 10]);
    }

    // ── Mode lookup tests ────────────────────────────────────────────

    #[test]
    fn test_mode_lookup() {
        assert_eq!(lookup_mode("major"), Some(&[0, 2, 4, 5, 7, 9, 11][..]));
        assert_eq!(lookup_mode("ionian"), Some(&[0, 2, 4, 5, 7, 9, 11][..]));
        assert_eq!(lookup_mode("dorian"), Some(&[0, 2, 3, 5, 7, 9, 10][..]));
        assert_eq!(lookup_mode("minor"), Some(&[0, 2, 3, 5, 7, 8, 10][..]));
        assert_eq!(lookup_mode("aeolian"), Some(&[0, 2, 3, 5, 7, 8, 10][..]));
        assert!(lookup_mode("nonexistent").is_none());
    }

    // ── Harmony Index tests ──────────────────────────────────────────

    fn make_header() -> GlobalHeader {
        GlobalHeader {
            ppq: 480,
            bpm: 120.0,
            ts_numerator: 4,
            ts_denominator: 4,
            ..Default::default()
        }
    }

    fn make_chord(root_str: &str) -> ChordSymbol {
        parse_chord_symbol(root_str).unwrap()
    }

    fn default_scale_timeline() -> ScaleTimeline {
        ScaleTimeline::from_tonal_context(&TonalContext::default()).unwrap()
    }

    #[test]
    fn test_simple_harmony_index() {
        let block = HarmonyBlock {
            name: Some("main".to_string()),
            play: false,
            channel: None,
            program: None,
            voice: crate::ast::VoicingStrategy::Close,
            octave: 4,
            velocity: 72,
            inv: crate::ast::Inversion::Fixed(0),
            bars: vec![
                Bar {
                    chords: vec![crate::ast::BarChord {
                        chord: make_chord("Cmaj7"),
                        beats: None,
                        span: None,
                    }],
                    steps: None,
                    span: None,
                },
                Bar {
                    chords: vec![crate::ast::BarChord {
                        chord: make_chord("Am7"),
                        beats: None,
                        span: None,
                    }],
                    steps: None,
                    span: None,
                },
                Bar {
                    chords: vec![
                        crate::ast::BarChord {
                            chord: make_chord("Dm7"),
                            beats: None,
                            span: None,
                        },
                        crate::ast::BarChord {
                            chord: make_chord("G7"),
                            beats: None,
                            span: None,
                        },
                    ],
                    steps: None,
                    span: None,
                },
                Bar {
                    chords: vec![crate::ast::BarChord {
                        chord: make_chord("Cmaj7"),
                        beats: None,
                        span: None,
                    }],
                    steps: None,
                    span: None,
                },
            ],
            sections: vec![],
            span: None,
        };

        let header = make_header();
        let index = HarmonyIndex::build(
            &block,
            &header,
            &default_scale_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .unwrap();

        // 4 bars at 480 ppq, 4/4 = 1920 ticks per bar, total 7680
        assert_eq!(index.total_ticks, 7680);

        // Bar 1: Cmaj7 for full bar
        let ctx = index.query(0).unwrap();
        assert_eq!(ctx.chord.root, 0); // C
        let ctx = index.query(1919).unwrap();
        assert_eq!(ctx.chord.root, 0); // still Cmaj7

        // Bar 2: Am7
        let ctx = index.query(1920).unwrap();
        assert_eq!(ctx.chord.root, 9); // A

        // Bar 3: Dm7 for 2 beats, G7 for 2 beats
        let ctx = index.query(3840).unwrap();
        assert_eq!(ctx.chord.root, 2); // D (Dm7)
        let ctx = index.query(4800).unwrap();
        assert_eq!(ctx.chord.root, 7); // G (G7)

        // Bar 4: Cmaj7
        let ctx = index.query(5760).unwrap();
        assert_eq!(ctx.chord.root, 0);
    }

    /// Build a minimal harmony block from per-bar chord lists.
    fn make_block(bars: Vec<Vec<ChordSymbol>>) -> HarmonyBlock {
        HarmonyBlock {
            name: Some("main".to_string()),
            play: false,
            channel: None,
            program: None,
            voice: crate::ast::VoicingStrategy::Close,
            octave: 4,
            velocity: 72,
            inv: crate::ast::Inversion::Fixed(0),
            bars: bars
                .into_iter()
                .map(|chords| Bar {
                    chords: chords
                        .into_iter()
                        .map(|chord| crate::ast::BarChord {
                            chord,
                            beats: None,
                            span: None,
                        })
                        .collect(),
                    steps: None,
                    span: None,
                })
                .collect(),
            sections: vec![],
            span: None,
        }
    }

    #[test]
    fn test_more_chords_than_beats_is_error() {
        // 5 chords in a 4/4 bar with even distribution: the 5th chord would
        // get a zero-length span that query() can never return. Must be a
        // compile error, not silent dropping.
        let block = make_block(vec![vec![
            make_chord("C"),
            make_chord("Dm"),
            make_chord("Em"),
            make_chord("F"),
            make_chord("G"),
        ]]);
        let header = make_header();
        let err = HarmonyIndex::build(
            &block,
            &header,
            &default_scale_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .expect_err("5 chords in 4 beats must error");
        let msg = err.to_string();
        assert!(
            msg.contains("bar 1 has 5 chords but only 4 beats"),
            "unexpected message: {msg}"
        );
        assert!(msg.contains("use steps: or explicit beat counts"));
    }

    #[test]
    fn test_chords_equal_beats_still_allowed() {
        let block = make_block(vec![vec![
            make_chord("C"),
            make_chord("Dm"),
            make_chord("Em"),
            make_chord("F"),
        ]]);
        let header = make_header();
        let index = HarmonyIndex::build(
            &block,
            &header,
            &default_scale_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .expect("4 chords in 4 beats is fine");
        assert_eq!(index.spans().len(), 4);
        assert!(index.spans().iter().all(|s| s.end_tick > s.start_tick));
    }

    // ── Roman-numeral quality re-resolution across mode changes ──

    /// C major for 1 bar, then C minor (aeolian) from bar 2 on.
    fn major_then_minor_timeline() -> ScaleTimeline {
        ScaleTimeline::from_scale_block(&ScaleBlock {
            entries: vec![
                crate::ast::ScaleEntry {
                    root: Some(0),
                    mode: Some("major".to_string()),
                    bars: Some(1),
                    span: None,
                },
                crate::ast::ScaleEntry {
                    root: None,
                    mode: Some("minor".to_string()),
                    bars: None,
                    span: None,
                },
            ],
            span: None,
        })
        .unwrap()
    }

    fn c_major_context() -> TonalContext {
        TonalContext {
            root: Some(0),
            mode: "major".to_string(),
            span: None,
        }
    }

    #[test]
    fn test_bare_roman_quality_rederived_across_mode_change() {
        // Bare `I` parsed in C major = C major triad. On a bar where the
        // scale timeline has moved to C minor, the diatonic triad on the
        // tonic is MINOR: the quality must be re-derived, not frozen at
        // the parse-time mode.
        let i_chord = parse_chord_symbol_with_context("I", &c_major_context()).unwrap();
        let block = make_block(vec![vec![i_chord.clone()], vec![i_chord]]);
        let header = make_header();
        let index = HarmonyIndex::build(
            &block,
            &header,
            &major_then_minor_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .unwrap();

        let bar1 = index.query(0).unwrap();
        assert_eq!(bar1.chord.root, 0);
        assert_eq!(bar1.chord.intervals, vec![0, 4, 7], "bar 1: C major triad");

        let bar2 = index.query(1920).unwrap();
        assert_eq!(bar2.chord.root, 0);
        assert_eq!(
            bar2.chord.intervals,
            vec![0, 3, 7],
            "bar 2 (C minor): tonic triad re-derived to minor"
        );
    }

    #[test]
    fn test_bare_roman_diminished_rederived_across_mode_change() {
        // `ii` in C major = D minor. In C natural minor the supertonic
        // triad is DIMINISHED (ii°): quality re-derives, root moves to the
        // minor-mode degree (still D, interval 2).
        let ii_chord = parse_chord_symbol_with_context("ii", &c_major_context()).unwrap();
        let block = make_block(vec![vec![ii_chord.clone()], vec![ii_chord]]);
        let header = make_header();
        let index = HarmonyIndex::build(
            &block,
            &header,
            &major_then_minor_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .unwrap();
        let bar1 = index.query(0).unwrap();
        assert_eq!(
            (bar1.chord.root, bar1.chord.intervals.clone()),
            (2, vec![0, 3, 7])
        );
        let bar2 = index.query(1920).unwrap();
        assert_eq!(
            (bar2.chord.root, bar2.chord.intervals.clone()),
            (2, vec![0, 3, 6]),
            "ii in C minor is the diminished supertonic triad"
        );
    }

    #[test]
    fn test_explicit_roman_quality_kept_across_mode_change() {
        // `V7` carries an explicit dominant-seventh quality: across the
        // mode change only the ROOT re-resolves; the written quality must
        // never be re-derived (it is not the bare diatonic triad).
        let v7_chord = parse_chord_symbol_with_context("V7", &c_major_context()).unwrap();
        assert_eq!(v7_chord.intervals, vec![0, 4, 7, 10]);
        let maj7 = parse_chord_symbol_with_context("IIImaj7", &c_major_context()).unwrap();
        let block = make_block(vec![vec![v7_chord.clone()], vec![v7_chord, maj7]]);
        let header = make_header();
        let index = HarmonyIndex::build(
            &block,
            &header,
            &major_then_minor_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .unwrap();
        let bar2_v7 = index.query(1920).unwrap();
        assert_eq!(
            bar2_v7.chord.root, 7,
            "root re-resolves against the bar's mode"
        );
        assert_eq!(
            bar2_v7.chord.intervals,
            vec![0, 4, 7, 10],
            "explicit V7 quality is kept verbatim"
        );
        let bar2_iiimaj7 = index.query(2880).unwrap();
        assert_eq!(
            bar2_iiimaj7.chord.root, 3,
            "III root moves to the minor-mode mediant (Eb)"
        );
        assert_eq!(
            bar2_iiimaj7.chord.intervals,
            vec![0, 4, 7, 11],
            "explicit maj7 quality is kept verbatim"
        );
    }

    #[test]
    fn test_letter_chords_untouched_by_mode_change() {
        let block = make_block(vec![vec![make_chord("Cmaj7")], vec![make_chord("Cmaj7")]]);
        let header = make_header();
        let index = HarmonyIndex::build(
            &block,
            &header,
            &major_then_minor_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .unwrap();
        let bar2 = index.query(1920).unwrap();
        assert_eq!(bar2.chord.root, 0);
        assert_eq!(bar2.chord.intervals, vec![0, 4, 7, 11]);
    }

    #[test]
    fn test_explicit_beat_assignment() {
        let block = HarmonyBlock {
            name: Some("main".to_string()),
            play: false,
            channel: None,
            program: None,
            voice: crate::ast::VoicingStrategy::Close,
            octave: 4,
            velocity: 72,
            inv: crate::ast::Inversion::Fixed(0),
            bars: vec![Bar {
                chords: vec![
                    crate::ast::BarChord {
                        chord: make_chord("Dm7"),
                        beats: Some(3),
                        span: None,
                    },
                    crate::ast::BarChord {
                        chord: make_chord("G7"),
                        beats: Some(1),
                        span: None,
                    },
                ],
                steps: None,
                span: None,
            }],
            sections: vec![],
            span: None,
        };

        let header = make_header();
        let index = HarmonyIndex::build(
            &block,
            &header,
            &default_scale_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .unwrap();

        // Dm7 for 3 beats = 3 * 480 = 1440 ticks
        let ctx = index.query(0).unwrap();
        assert_eq!(ctx.chord.root, 2); // D
        let ctx = index.query(1439).unwrap();
        assert_eq!(ctx.chord.root, 2); // still Dm7

        // G7 for 1 beat = 480 ticks
        let ctx = index.query(1440).unwrap();
        assert_eq!(ctx.chord.root, 7); // G
    }

    #[test]
    fn test_steps_block() {
        let block = HarmonyBlock {
            name: Some("main".to_string()),
            play: false,
            channel: None,
            program: None,
            voice: crate::ast::VoicingStrategy::Close,
            octave: 4,
            velocity: 72,
            inv: crate::ast::Inversion::Fixed(0),
            bars: vec![Bar {
                chords: vec![crate::ast::BarChord {
                    chord: make_chord("Cmaj7"),
                    beats: None,
                    span: None,
                }],
                steps: Some(vec![
                    make_chord("Bbmaj7"),
                    make_chord("Bbmaj7"),
                    make_chord("Amaj7"),
                    make_chord("Amaj7"),
                    make_chord("Abmaj7"),
                    make_chord("Abmaj7"),
                    make_chord("Gmaj7"),
                    make_chord("Gmaj7"),
                ]),
                span: None,
            }],
            sections: vec![],
            span: None,
        };

        let header = make_header();
        let index = HarmonyIndex::build(
            &block,
            &header,
            &default_scale_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .unwrap();

        // 8 steps in one bar of 1920 ticks = 240 ticks per step
        let ctx = index.query(0).unwrap();
        assert_eq!(ctx.chord.root, 10); // Bb

        let ctx = index.query(480).unwrap();
        assert_eq!(ctx.chord.root, 9); // A (step 3)

        let ctx = index.query(960).unwrap();
        assert_eq!(ctx.chord.root, 8); // Ab (step 5)

        let ctx = index.query(1440).unwrap();
        assert_eq!(ctx.chord.root, 7); // G (step 7)
    }

    #[test]
    fn test_section_modulation() {
        let block = HarmonyBlock {
            name: Some("main".to_string()),
            play: false,
            channel: None,
            program: None,
            voice: crate::ast::VoicingStrategy::Close,
            octave: 4,
            velocity: 72,
            inv: crate::ast::Inversion::Fixed(0),
            bars: vec![
                Bar {
                    chords: vec![crate::ast::BarChord {
                        chord: make_chord("Cmaj7"),
                        beats: None,
                        span: None,
                    }],
                    steps: None,
                    span: None,
                },
                Bar {
                    chords: vec![crate::ast::BarChord {
                        chord: make_chord("Cmaj7"),
                        beats: None,
                        span: None,
                    }],
                    steps: None,
                    span: None,
                },
                Bar {
                    chords: vec![crate::ast::BarChord {
                        chord: make_chord("Dm7"),
                        beats: None,
                        span: None,
                    }],
                    steps: None,
                    span: None,
                },
                Bar {
                    chords: vec![crate::ast::BarChord {
                        chord: make_chord("Dm7"),
                        beats: None,
                        span: None,
                    }],
                    steps: None,
                    span: None,
                },
            ],
            sections: vec![
                Section {
                    bar: 3,
                    mode: Some("dorian".to_string()),
                    root: Some(2),
                    span: None,
                }, // D dorian at bar 3
            ],
            span: None,
        };

        let header = make_header();
        let index = HarmonyIndex::build(
            &block,
            &header,
            &default_scale_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .unwrap();

        // Bar 1-2: major mode, root defaults
        let ctx = index.query(0).unwrap();
        assert_eq!(ctx.mode_intervals, vec![0, 2, 4, 5, 7, 9, 11]); // major

        // Bar 3-4: dorian mode, root D
        let ctx = index.query(3840).unwrap();
        assert_eq!(ctx.mode_intervals, vec![0, 2, 3, 5, 7, 9, 10]); // dorian
        assert_eq!(ctx.scale_root, 2); // D
    }

    #[test]
    fn test_query_cyclic_wrap() {
        let block = HarmonyBlock {
            name: Some("main".to_string()),
            play: false,
            channel: None,
            program: None,
            voice: crate::ast::VoicingStrategy::Close,
            octave: 4,
            velocity: 72,
            inv: crate::ast::Inversion::Fixed(0),
            bars: vec![Bar {
                chords: vec![crate::ast::BarChord {
                    chord: make_chord("C"),
                    beats: None,
                    span: None,
                }],
                steps: None,
                span: None,
            }],
            sections: vec![],
            span: None,
        };

        let header = make_header();
        let index = HarmonyIndex::build(
            &block,
            &header,
            &default_scale_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .unwrap();

        // Past the end: wraps cyclically (2000 % 1920 = 80, within C chord span)
        let ctx = index.query(2000);
        assert!(ctx.is_some());
        assert_eq!(ctx.unwrap().chord.root, 0); // C
    }

    #[test]
    fn test_remainder_goes_to_last_steps() {
        // 3 chords in one bar. ticks_per_bar = 1920, 1920/3 = 640 rem 0 → even.
        // Use ppq=100, ts=5/4 → ticks_per_bar = 500, 500/3 = 166 rem 2.
        // Expected: [166, 168, 166] is WRONG. Correct: [166, 166, 168] → last steps absorb.
        // Actually: 500/3 = 166 rem 2 → last 2 steps get +1 → [166, 167, 167].
        let block = HarmonyBlock {
            name: Some("test".to_string()),
            play: false,
            channel: None,
            program: None,
            voice: crate::ast::VoicingStrategy::Close,
            octave: 4,
            velocity: 72,
            inv: crate::ast::Inversion::Fixed(0),
            bars: vec![Bar {
                chords: vec![crate::ast::BarChord {
                    chord: make_chord("C"),
                    beats: None,
                    span: None,
                }],
                steps: Some(vec![make_chord("C"), make_chord("E"), make_chord("G")]),
                span: None,
            }],
            sections: vec![],
            span: None,
        };

        let mut header = make_header();
        header.ppq = 100;
        header.ts_numerator = 5;
        let index = HarmonyIndex::build(
            &block,
            &header,
            &default_scale_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        )
        .unwrap();
        let spans = index.spans();
        assert_eq!(spans.len(), 3);
        // First step: 166 ticks
        assert_eq!(spans[0].end_tick - spans[0].start_tick, 166);
        // Second step: 167 ticks (last 2 absorb remainder)
        assert_eq!(spans[1].end_tick - spans[1].start_tick, 167);
        // Third step: 167 ticks
        assert_eq!(spans[2].end_tick - spans[2].start_tick, 167);
        // Total should equal ticks_per_bar
        assert_eq!(spans[2].end_tick, 500);
    }

    #[test]
    fn test_beat_assignment_mismatch() {
        // 4/4 time but beats sum to 3
        let block = HarmonyBlock {
            name: Some("test".to_string()),
            play: false,
            channel: None,
            program: None,
            voice: crate::ast::VoicingStrategy::Close,
            octave: 4,
            velocity: 72,
            inv: crate::ast::Inversion::Fixed(0),
            bars: vec![Bar {
                chords: vec![
                    crate::ast::BarChord {
                        chord: make_chord("C"),
                        beats: Some(2),
                        span: None,
                    },
                    crate::ast::BarChord {
                        chord: make_chord("G"),
                        beats: Some(1),
                        span: None,
                    },
                ],
                steps: None,
                span: None,
            }],
            sections: vec![],
            span: None,
        };

        let header = make_header(); // 4/4
        let result = HarmonyIndex::build(
            &block,
            &header,
            &default_scale_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.to_lowercase().contains("beat"),
            "expected beat mismatch error, got: {err}"
        );
    }

    #[test]
    fn test_beat_assignment_valid() {
        // 4/4 time, beats sum to 4
        let block = HarmonyBlock {
            name: Some("test".to_string()),
            play: false,
            channel: None,
            program: None,
            voice: crate::ast::VoicingStrategy::Close,
            octave: 4,
            velocity: 72,
            inv: crate::ast::Inversion::Fixed(0),
            bars: vec![Bar {
                chords: vec![
                    crate::ast::BarChord {
                        chord: make_chord("C"),
                        beats: Some(3),
                        span: None,
                    },
                    crate::ast::BarChord {
                        chord: make_chord("G"),
                        beats: Some(1),
                        span: None,
                    },
                ],
                steps: None,
                span: None,
            }],
            sections: vec![],
            span: None,
        };

        let header = make_header(); // 4/4
        let result = HarmonyIndex::build(
            &block,
            &header,
            &default_scale_timeline(),
            &crate::compiler::BarLayout::from_header(&header),
        );
        assert!(result.is_ok());
    }
}
