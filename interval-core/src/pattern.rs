//! Pattern resolution and composition.
//!
//! Resolves pattern composition expressions (`*`, `*~`, `>>`, `~>>`),
//! handles tie carry-over across soft boundaries, and validates unit
//! compatibility between composed patterns. Also handles pattern
//! assignment expressions (`@pattern name = expr`).

use std::collections::HashMap;

use crate::ast::{PatternBlock, PatternBody, PatternExpr, StepLine, StepToken, TransformCall};
use crate::error::{CompileError, CompileResult, Span};

/// Boundary type between two adjacent segments in a resolved pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum Boundary {
    /// Hard boundary: ties cut, voice-leading state does not carry over.
    Hard,
    /// Soft boundary: trailing active notes carry over into the next segment.
    Soft,
}

/// Default velocity, gate, and octave for a segment of a resolved pattern.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct SegmentDefaults {
    pub velocity: u8,
    pub gate: f64,
    pub octave: u8,
    /// Per-segment rate multiplier (1.0 = normal). Applied on top of track rate.
    pub rate: f64,
    /// Number of steps in this segment.
    pub step_count: usize,
}

/// Records the name and step count of one pattern instance in a resolved sequence.
/// Used by the compiler to emit `PatternBoundary` events for RT loop counting.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PatternInstance {
    /// Pattern name (e.g. "theme", "bass").
    pub name: String,
    /// Number of steps this instance contributes.
    pub step_count: usize,
    /// Emission-phase transforms scoped to this instance: `swing`,
    /// `humanize`, and `vary` recorded from the pipeline (or baked into the
    /// `@pattern` declaration). Spec §10.3/§10.5: these apply to the
    /// pattern(s) they are piped from, not the whole track. The compiler
    /// reads them during event emission; stochastic entries re-roll per
    /// reference because each reference clones this metadata and the RNG
    /// draws are sequential (spec §7, "stochastic transforms are
    /// re-evaluated per reference").
    ///
    /// Not serialized: golden tests capture pattern structure, not emission
    /// scoping metadata.
    #[serde(skip)]
    pub emission_transforms: Vec<TransformCall>,
}

/// A resolved pattern — a flat sequence of step lines with boundary info.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ResolvedPattern {
    /// Effective step unit as a fraction (numerator, denominator).
    pub unit: (u32, u32),
    /// All step lines in order.
    pub steps: Vec<StepLine>,
    /// Boundary types between segments. `boundaries[i]` is the boundary
    /// *after* the last step of segment `i` (before the first step of
    /// segment `i+1`). Length is one less than the number of segments
    /// in the original composition. For a single pattern with no
    /// composition, this is empty.
    pub boundaries: Vec<Boundary>,
    /// Per-segment default values. Each entry corresponds to a segment
    /// in the resolved pattern. For a single pattern with no composition,
    /// this contains one entry. Empty means "use track defaults".
    pub segment_defaults: Vec<SegmentDefaults>,
    /// Pattern instance boundaries for RT loop counting. Each entry
    /// records a pattern name and the number of steps it contributes.
    /// The compiler uses this to emit `PatternBoundary` events.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pattern_instances: Vec<PatternInstance>,
}

/// A registry of named patterns available for composition resolution.
pub type PatternRegistry = HashMap<String, PatternBlock>;

/// Resolve all pattern blocks in declaration order.
///
/// Pattern assignment expressions are resolved using previously declared
/// patterns. Forward references are rejected. The result maps each pattern
/// name to its resolved flat step sequence.
pub fn resolve_all(blocks: &[PatternBlock]) -> CompileResult<HashMap<String, ResolvedPattern>> {
    let mut registry = PatternRegistry::new();
    let mut resolved = HashMap::new();

    for block in blocks {
        match &block.body {
            PatternBody::Steps(lines) => {
                // Direct step body — already resolved
                let mut rp = ResolvedPattern {
                    unit: block.unit,
                    steps: lines.clone(),
                    boundaries: Vec::new(),
                    segment_defaults: vec![SegmentDefaults {
                        velocity: block.velocity,
                        gate: block.gate,
                        octave: block.octave,
                        rate: 1.0,
                        step_count: lines.len(),
                    }],
                    pattern_instances: vec![PatternInstance {
                        name: block.name.clone(),
                        step_count: lines.len(),
                        emission_transforms: Vec::new(),
                    }],
                };
                // Apply baked-in transforms
                for t in &block.transforms {
                    rp = apply_transform(rp, t, &registry, &resolved)?;
                }
                resolved.insert(block.name.clone(), rp);
                registry.insert(block.name.clone(), block.clone());
            }
            PatternBody::Expression(expr) => {
                let rp = resolve_expr(expr, &registry, &resolved)?;
                // Build a synthetic PatternBlock for the registry so downstream
                // expressions can reference this pattern.
                let synthetic = PatternBlock {
                    name: block.name.clone(),
                    steps: rp.steps.len() as u32,
                    unit: rp.unit,
                    velocity: block.velocity,
                    gate: block.gate,
                    octave: block.octave,
                    transforms: Vec::new(),
                    body: PatternBody::Steps(rp.steps.clone()),
                    span: None,
                };
                resolved.insert(block.name.clone(), rp);
                registry.insert(block.name.clone(), synthetic);
            }
        }
    }

    Ok(resolved)
}

/// Resolve a single pattern expression recursively.
fn resolve_expr(
    expr: &PatternExpr,
    registry: &PatternRegistry,
    resolved: &HashMap<String, ResolvedPattern>,
) -> CompileResult<ResolvedPattern> {
    match expr {
        PatternExpr::Ref { name, rate } => {
            if let Some(rp) = resolved.get(name) {
                let mut result = rp.clone();
                // Apply per-reference rate to segment defaults
                if let Some(r) = rate {
                    for seg in &mut result.segment_defaults {
                        seg.rate *= r;
                    }
                }
                Ok(result)
            } else {
                Err(CompileError::ForwardReference {
                    name: name.clone(),
                    span: Span::new(0, 0),
                })
            }
        }

        PatternExpr::Repeat { pattern, count } => {
            let inner = resolve_expr(pattern, registry, resolved)?;
            repeat_pattern(&inner, *count, Boundary::Hard)
        }

        PatternExpr::RepeatSoft { pattern, count } => {
            let inner = resolve_expr(pattern, registry, resolved)?;
            repeat_pattern(&inner, *count, Boundary::Soft)
        }

        PatternExpr::Concat { left, right } => {
            let lhs = resolve_expr(left, registry, resolved)?;
            let rhs = resolve_expr(right, registry, resolved)?;
            concat_patterns(lhs, rhs, Boundary::Hard)
        }

        PatternExpr::ConcatSoft { left, right } => {
            let lhs = resolve_expr(left, registry, resolved)?;
            let rhs = resolve_expr(right, registry, resolved)?;
            concat_patterns(lhs, rhs, Boundary::Soft)
        }

        PatternExpr::Transform { pattern, transform } => {
            let inner = resolve_expr(pattern, registry, resolved)?;
            apply_transform(inner, transform, registry, resolved)
        }
    }
}

/// Repeat a pattern N times with the given boundary type between instances.
fn repeat_pattern(
    pattern: &ResolvedPattern,
    count: u32,
    boundary: Boundary,
) -> CompileResult<ResolvedPattern> {
    if count == 0 {
        return Ok(ResolvedPattern {
            unit: pattern.unit,
            steps: Vec::new(),
            boundaries: Vec::new(),
            segment_defaults: Vec::new(),
            pattern_instances: Vec::new(),
        });
    }

    let mut steps = Vec::new();
    let mut boundaries = Vec::new();
    let mut segment_defaults = Vec::new();
    let mut pattern_instances = Vec::new();

    for i in 0..count {
        // Carry over internal boundaries from the original pattern
        if i > 0 {
            boundaries.push(boundary);
        }
        steps.extend(pattern.steps.iter().cloned());
        // Append the original pattern's internal boundaries
        boundaries.extend(pattern.boundaries.iter().copied());
        segment_defaults.extend(pattern.segment_defaults.iter().copied());
        pattern_instances.extend(pattern.pattern_instances.iter().cloned());
    }

    Ok(ResolvedPattern {
        unit: pattern.unit,
        steps,
        boundaries,
        segment_defaults,
        pattern_instances,
    })
}

/// Concatenate two patterns with the given boundary type at the join point.
fn concat_patterns(
    mut lhs: ResolvedPattern,
    rhs: ResolvedPattern,
    boundary: Boundary,
) -> CompileResult<ResolvedPattern> {
    // Validate unit compatibility
    if lhs.unit != rhs.unit {
        return Err(CompileError::UnitMismatch {
            span: Span::new(0, 0),
        });
    }

    if !lhs.steps.is_empty() && !rhs.steps.is_empty() {
        lhs.boundaries.push(boundary);
    }
    lhs.steps.extend(rhs.steps);
    lhs.boundaries.extend(rhs.boundaries);
    lhs.segment_defaults.extend(rhs.segment_defaults);
    lhs.pattern_instances.extend(rhs.pattern_instances);

    Ok(lhs)
}

/// Apply a step-level transform to a resolved pattern.
fn apply_transform(
    mut pattern: ResolvedPattern,
    transform: &TransformCall,
    _registry: &PatternRegistry,
    resolved: &HashMap<String, ResolvedPattern>,
) -> CompileResult<ResolvedPattern> {
    match transform {
        TransformCall::Reverse => {
            pattern.steps.reverse();
            pattern.boundaries.reverse();
            pattern.segment_defaults.reverse();
            pattern.pattern_instances.reverse();
            Ok(pattern)
        }

        TransformCall::Rotate(n) => {
            let len = pattern.steps.len();
            if len == 0 {
                return Ok(pattern);
            }
            // Normalize rotation to positive index
            let n = ((*n % len as i32) + len as i32) as usize % len;

            // Expand segment_defaults to per-step, rotate alongside steps,
            // then re-coalesce into segments. This preserves correct defaults
            // after rotation of composed patterns with mixed defaults.
            let mut per_step_defaults: Vec<SegmentDefaults> = Vec::with_capacity(len);
            for seg in &pattern.segment_defaults {
                for _ in 0..seg.step_count {
                    per_step_defaults.push(SegmentDefaults {
                        step_count: 1, // will be re-coalesced
                        ..*seg
                    });
                }
            }
            // Pad if segment_defaults was shorter than steps (shouldn't happen, but safety)
            while per_step_defaults.len() < len {
                per_step_defaults.push(SegmentDefaults {
                    velocity: 100,
                    gate: 0.8,
                    octave: 4,
                    rate: 1.0,
                    step_count: 1,
                });
            }

            pattern.steps.rotate_left(n);
            per_step_defaults.rotate_left(n);

            // Re-coalesce consecutive identical defaults into segments
            let mut coalesced: Vec<SegmentDefaults> = Vec::new();
            for d in &per_step_defaults {
                if let Some(last) = coalesced.last_mut() {
                    if last.velocity == d.velocity
                        && last.gate == d.gate
                        && last.octave == d.octave
                        && last.rate == d.rate
                    {
                        last.step_count += 1;
                        continue;
                    }
                }
                coalesced.push(SegmentDefaults {
                    step_count: 1,
                    ..*d
                });
            }
            pattern.segment_defaults = coalesced;

            // Rebuild boundaries: one Hard between each segment
            pattern.boundaries = if pattern.segment_defaults.len() > 1 {
                vec![Boundary::Hard; pattern.segment_defaults.len() - 1]
            } else {
                Vec::new()
            };

            // Collapse pattern_instances — rotation scrambles instance boundaries
            collapse_instances(&mut pattern, len);

            Ok(pattern)
        }

        TransformCall::Subset(indices) => {
            // indices are 1-indexed
            let new_steps: Vec<StepLine> = indices
                .iter()
                .filter_map(|&i| {
                    if i >= 1 && (i as usize) <= pattern.steps.len() {
                        Some(pattern.steps[i as usize - 1].clone())
                    } else {
                        None
                    }
                })
                .collect();
            let first_defaults = pattern.segment_defaults.first().copied();
            pattern.steps = new_steps;
            pattern.boundaries = if pattern.steps.len() > 1 {
                vec![Boundary::Hard; pattern.steps.len() - 1]
            } else {
                Vec::new()
            };
            pattern.segment_defaults = if let Some(mut d) = first_defaults {
                d.step_count = pattern.steps.len();
                vec![d]
            } else {
                Vec::new()
            };
            // Collapse pattern instances — subset scrambles instance boundaries
            let count = pattern.steps.len();
            collapse_instances(&mut pattern, count);
            Ok(pattern)
        }

        TransformCall::Mirror => {
            // Concatenate pattern with its reverse
            let mut mirrored = pattern.steps.clone();
            mirrored.reverse();
            let mut mirrored_defaults = pattern.segment_defaults.clone();
            mirrored_defaults.reverse();
            pattern.boundaries.push(Boundary::Hard);
            let mirror_boundaries = if mirrored.len() > 1 {
                vec![Boundary::Hard; mirrored.len() - 1]
            } else {
                Vec::new()
            };
            let mut mirrored_instances = pattern.pattern_instances.clone();
            mirrored_instances.reverse();
            pattern.steps.extend(mirrored);
            pattern.boundaries.extend(mirror_boundaries);
            pattern.segment_defaults.extend(mirrored_defaults);
            pattern.pattern_instances.extend(mirrored_instances);
            Ok(pattern)
        }

        TransformCall::Interleave(other_name) => {
            let other = resolved
                .get(other_name)
                .ok_or_else(|| CompileError::ForwardReference {
                    name: other_name.clone(),
                    span: Span::new(0, 0),
                })?;

            if pattern.steps.len() != other.steps.len() {
                return Err(CompileError::InterleaveMismatch {
                    span: Span::new(0, 0),
                });
            }

            let mut interleaved = Vec::with_capacity(pattern.steps.len() * 2);
            for (a, b) in pattern.steps.iter().zip(other.steps.iter()) {
                interleaved.push(a.clone());
                interleaved.push(b.clone());
            }
            let first_defaults = pattern.segment_defaults.first().copied();
            pattern.steps = interleaved;
            pattern.boundaries = if pattern.steps.len() > 1 {
                vec![Boundary::Hard; pattern.steps.len() - 1]
            } else {
                Vec::new()
            };
            pattern.segment_defaults = if let Some(mut d) = first_defaults {
                d.step_count = pattern.steps.len();
                vec![d]
            } else {
                Vec::new()
            };
            // Collapse pattern instances — interleave scrambles instance boundaries
            let count = pattern.steps.len();
            collapse_instances(&mut pattern, count);
            Ok(pattern)
        }

        TransformCall::Retrograde => {
            // Spec §10.2: retrograde is the alias for `reverse -> invert`
            // (classical retrograde-inversion). Reverse the step order,
            // then invert all intervals around the (new) first pitch using
            // the same machinery as the standalone `invert` transform.
            pattern.steps.reverse();
            pattern.boundaries.reverse();
            pattern.segment_defaults.reverse();
            pattern.pattern_instances.reverse();
            invert_steps(&mut pattern.steps);
            Ok(pattern)
        }

        TransformCall::Transpose(n) => {
            // Transpose only affects absolute pitches and MIDI note numbers.
            // Degree tokens are NOT affected (they resolve against harmony).
            for step in &mut pattern.steps {
                for token in &mut step.tokens {
                    transpose_token(token, *n);
                }
            }
            Ok(pattern)
        }

        TransformCall::ShiftOct(n) => {
            // Shift all notes by n octaves.
            // For Degree: modifies the octave field.
            // For AbsolutePitch/MidiNumber: shifts by n*12 semitones.
            for step in &mut pattern.steps {
                for token in &mut step.tokens {
                    shift_oct_token(token, *n);
                }
            }
            Ok(pattern)
        }

        TransformCall::Invert => {
            // Spec §10.2: "Inverts all intervals around the first pitch.
            // Each interval from the first step's pitch class is negated."
            // Applied at pattern-resolution time (like reverse):
            // - chromatic tokens (absolute pitch, MIDI number) mirror around
            //   the first chromatic pitch: new = 2*pivot - old;
            // - degree tokens mirror diatonically around the first degree
            //   token in scale-step space (octave*7 + degree-1, octave
            //   defaulting to 4 as in shift_oct), with accidentals negated —
            //   degrees resolve against harmony later, so inversion happens
            //   in scale steps, not semitones;
            // - harmony-context tokens (%n, $chord, $Chord), drum hits,
            //   rests and ties are unaffected (their pitches resolve at
            //   emission time — the same treatment transpose gives them).
            invert_steps(&mut pattern.steps);
            Ok(pattern)
        }

        TransformCall::Stretch(num, denom) => {
            // Stretch modifies the effective unit: unit * (num/denom)
            // e.g., unit=1/8, stretch(2) → unit=1/4
            let new_num = pattern.unit.0 * num;
            let new_denom = pattern.unit.1 * denom;
            pattern.unit = simplify_fraction(new_num, new_denom);
            Ok(pattern)
        }

        TransformCall::Compress(num, denom) => {
            // Compress is the inverse of stretch: unit * (denom/num)
            let new_num = pattern.unit.0 * denom;
            let new_denom = pattern.unit.1 * num;
            pattern.unit = simplify_fraction(new_num, new_denom);
            Ok(pattern)
        }

        // Scoped emission transforms: applied per-step during event
        // emission, but only to the pattern instance(s) they were piped
        // onto (spec §10.3 swing, §10.5 humanize/vary). Record them on
        // every instance of this (sub)pattern; the compiler reads them
        // when emitting each instance's steps.
        TransformCall::Humanize(..) | TransformCall::Vary(..) | TransformCall::Swing(..) => {
            for inst in &mut pattern.pattern_instances {
                inst.emission_transforms.push(transform.clone());
            }
            Ok(pattern)
        }

        // These transforms are applied during compilation, not pattern resolution.
        TransformCall::Rubato(..)
        | TransformCall::Ritardando(..)
        | TransformCall::Accelerando(..)
        | TransformCall::Agogic(..)
        | TransformCall::Breathe(..)
        | TransformCall::Swell(..)
        | TransformCall::Phrase(..)
        | TransformCall::Evolve(..)
        | TransformCall::EuclidGate(..)
        | TransformCall::Echo(..)
        | TransformCall::VelCurve(..)
        | TransformCall::GateCurve(..)
        | TransformCall::ScaleLock(..)
        | TransformCall::Arp { .. } => Ok(pattern),
    }
}

/// Apply a step-level transform to a resolved pattern from a `play:` expression.
///
/// This is the public entry point used by the compiler when a `play:` expression
/// contains inline transforms (e.g., `play: p | reverse`). Event-level transforms
/// (humanize, echo, etc.) are handled separately during event emission.
pub fn apply_step_transform(
    pattern: ResolvedPattern,
    transform: &TransformCall,
    resolved: &HashMap<String, ResolvedPattern>,
) -> CompileResult<ResolvedPattern> {
    apply_transform(pattern, transform, &PatternRegistry::new(), resolved)
}

/// Collapse all pattern instances into one spanning `step_count` steps.
/// Used by transforms that scramble instance boundaries (rotate, subset,
/// interleave). Emission-transform scoping is preserved conservatively: the
/// collapsed instance carries the union of all instances' emission
/// transforms (duplicates are harmless — the compiler uses the last entry
/// of each kind).
fn collapse_instances(pattern: &mut ResolvedPattern, step_count: usize) {
    let total_name = pattern
        .pattern_instances
        .first()
        .map(|pi| pi.name.clone())
        .unwrap_or_default();
    let merged: Vec<TransformCall> = pattern
        .pattern_instances
        .iter()
        .flat_map(|pi| pi.emission_transforms.iter().cloned())
        .collect();
    pattern.pattern_instances = vec![PatternInstance {
        name: total_name,
        step_count,
        emission_transforms: merged,
    }];
}

/// Default octave used when a degree token has no explicit octave.
/// Matches the default used by `shift_oct_token`.
const INVERT_DEFAULT_OCTAVE: i32 = 4;

/// Invert all intervals around the first pitch (spec §10.2 `invert`).
///
/// Two independent pivot families:
/// - chromatic: the first `AbsolutePitch`/`MidiNumber` token (depth-first,
///   step order) is the pivot for all chromatic tokens;
/// - diatonic: the first `Degree` token is the pivot for all degree tokens,
///   working in scale-step space (`octave*7 + degree-1`).
///
/// Tokens whose pitches only resolve at emission time (`%n`, `$chord`,
/// chord symbols, drum hits) and non-pitch tokens are unaffected.
fn invert_steps(steps: &mut [StepLine]) {
    // Locate pivots.
    let mut chromatic_pivot: Option<i32> = None;
    let mut degree_pivot: Option<i32> = None;
    for step in steps.iter() {
        for token in &step.tokens {
            find_invert_pivots(token, &mut chromatic_pivot, &mut degree_pivot);
        }
    }
    // Apply inversion.
    for step in steps.iter_mut() {
        for token in &mut step.tokens {
            invert_token(token, chromatic_pivot, degree_pivot);
        }
    }
}

/// Scale-step index of a degree token: `octave*7 + (degree-1)`.
fn degree_scale_index(degree: u8, octave: Option<u8>) -> i32 {
    octave.map_or(INVERT_DEFAULT_OCTAVE, |o| o as i32) * 7 + (degree as i32 - 1)
}

/// Record the first chromatic and first diatonic pitch as inversion pivots.
fn find_invert_pivots(token: &StepToken, chromatic: &mut Option<i32>, diatonic: &mut Option<i32>) {
    match token {
        StepToken::AbsolutePitch { midi_note, .. } => {
            if chromatic.is_none() {
                *chromatic = Some(*midi_note as i32);
            }
        }
        StepToken::MidiNumber { note, .. } => {
            if chromatic.is_none() {
                *chromatic = Some(*note as i32);
            }
        }
        StepToken::Degree { degree, octave, .. } => {
            if diatonic.is_none() {
                *diatonic = Some(degree_scale_index(*degree, *octave));
            }
        }
        StepToken::Subdivision { tokens } => {
            for t in tokens {
                find_invert_pivots(t, chromatic, diatonic);
            }
        }
        StepToken::Variant { alternatives } => {
            for alt in alternatives {
                for t in alt {
                    find_invert_pivots(t, chromatic, diatonic);
                }
            }
        }
        _ => {}
    }
}

/// Mirror a single token around the family pivots (see `invert_steps`).
fn invert_token(token: &mut StepToken, chromatic_pivot: Option<i32>, degree_pivot: Option<i32>) {
    match token {
        StepToken::AbsolutePitch { midi_note, .. } => {
            if let Some(pivot) = chromatic_pivot {
                *midi_note = (2 * pivot - *midi_note as i32).clamp(0, 127) as u8;
            }
        }
        StepToken::MidiNumber { note, .. } => {
            if let Some(pivot) = chromatic_pivot {
                *note = (2 * pivot - *note as i32).clamp(0, 127) as u8;
            }
        }
        StepToken::Degree {
            degree,
            accidental,
            octave,
            ..
        } => {
            if let Some(pivot) = degree_pivot {
                let inverted = 2 * pivot - degree_scale_index(*degree, *octave);
                *degree = (inverted.rem_euclid(7) + 1) as u8;
                *octave = Some(inverted.div_euclid(7).clamp(0, 9) as u8);
                // Negating the interval also negates the accidental
                // displacement from the diatonic pitch.
                *accidental = -*accidental;
            }
        }
        StepToken::Subdivision { tokens } => {
            for t in tokens {
                invert_token(t, chromatic_pivot, degree_pivot);
            }
        }
        StepToken::Variant { alternatives } => {
            for alt in alternatives {
                for t in alt {
                    invert_token(t, chromatic_pivot, degree_pivot);
                }
            }
        }
        _ => {}
    }
}

/// Transpose a single step token by `n` semitones.
/// Only affects AbsolutePitch and MidiNumber tokens. Recurses into
/// Subdivision and Variant containers.
fn transpose_token(token: &mut StepToken, n: i32) {
    match token {
        StepToken::AbsolutePitch { midi_note, .. } => {
            let new_note = (*midi_note as i32) + n;
            *midi_note = new_note.clamp(0, 127) as u8;
        }
        StepToken::MidiNumber { note, .. } => {
            let new_note = (*note as i32) + n;
            *note = new_note.clamp(0, 127) as u8;
        }
        StepToken::Subdivision { tokens } => {
            for t in tokens {
                transpose_token(t, n);
            }
        }
        StepToken::Variant { alternatives } => {
            for alt in alternatives {
                for t in alt {
                    transpose_token(t, n);
                }
            }
        }
        _ => {}
    }
}

/// Shift a single step token by `n` octaves.
/// For Degree: modifies the octave field (or sets it if None, using default 4).
/// For AbsolutePitch/MidiNumber: shifts by n*12 semitones.
/// Recurses into Subdivision and Variant containers.
fn shift_oct_token(token: &mut StepToken, n: i32) {
    match token {
        StepToken::Degree { octave, .. } => {
            let base = octave.unwrap_or(4) as i32;
            let new_oct = (base + n).clamp(0, 9);
            *octave = Some(new_oct as u8);
        }
        StepToken::AbsolutePitch { midi_note, .. } => {
            let new_note = (*midi_note as i32) + n * 12;
            *midi_note = new_note.clamp(0, 127) as u8;
        }
        StepToken::MidiNumber { note, .. } => {
            let new_note = (*note as i32) + n * 12;
            *note = new_note.clamp(0, 127) as u8;
        }
        StepToken::Subdivision { tokens } => {
            for t in tokens {
                shift_oct_token(t, n);
            }
        }
        StepToken::Variant { alternatives } => {
            for alt in alternatives {
                for t in alt {
                    shift_oct_token(t, n);
                }
            }
        }
        _ => {}
    }
}

/// Simplify a fraction by dividing by GCD.
fn simplify_fraction(num: u32, denom: u32) -> (u32, u32) {
    let g = gcd(num, denom);
    (num / g, denom / g)
}

/// Greatest common divisor (Euclidean algorithm).
fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{PatternBlock, PatternBody, StepLine, StepToken};

    fn make_degree(d: u8) -> StepToken {
        StepToken::Degree {
            degree: d,
            accidental: 0,
            octave: None,
            annotations: Vec::new(),
        }
    }

    fn make_step_line(tokens: Vec<StepToken>) -> StepLine {
        let token_spans = vec![None; tokens.len()];
        StepLine {
            tokens,
            token_spans,
            span: None,
        }
    }

    fn make_pattern(name: &str, unit: (u32, u32), steps: Vec<StepLine>) -> PatternBlock {
        PatternBlock {
            name: name.to_string(),
            steps: steps.len() as u32,
            unit,
            velocity: 84,
            gate: 0.9,
            octave: 4,
            transforms: Vec::new(),
            body: PatternBody::Steps(steps),
            span: None,
        }
    }

    fn make_expr_pattern(name: &str, expr: PatternExpr) -> PatternBlock {
        PatternBlock {
            name: name.to_string(),
            steps: 0,
            unit: (0, 1),
            velocity: 84,
            gate: 0.9,
            octave: 4,
            transforms: Vec::new(),
            body: PatternBody::Expression(expr),
            span: None,
        }
    }

    #[test]
    fn test_resolve_simple_pattern() {
        let blocks = vec![make_pattern(
            "bass",
            (1, 4),
            vec![
                make_step_line(vec![make_degree(1)]),
                make_step_line(vec![make_degree(5)]),
            ],
        )];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["bass"];
        assert_eq!(rp.steps.len(), 2);
        assert_eq!(rp.unit, (1, 4));
        assert!(rp.boundaries.is_empty());
    }

    #[test]
    fn test_resolve_hard_repeat() {
        let blocks = vec![
            make_pattern(
                "motif",
                (1, 8),
                vec![
                    make_step_line(vec![make_degree(1)]),
                    make_step_line(vec![make_degree(3)]),
                ],
            ),
            make_expr_pattern(
                "repeated",
                PatternExpr::Repeat {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "motif".to_string(),
                        rate: None,
                    }),
                    count: 3,
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["repeated"];
        assert_eq!(rp.steps.len(), 6); // 2 * 3
        assert_eq!(rp.unit, (1, 8));
        assert_eq!(rp.boundaries.len(), 2); // 2 boundaries between 3 segments
        assert!(rp.boundaries.iter().all(|b| *b == Boundary::Hard));
    }

    #[test]
    fn test_resolve_soft_repeat() {
        let blocks = vec![
            make_pattern("motif", (1, 4), vec![make_step_line(vec![make_degree(1)])]),
            make_expr_pattern(
                "soft",
                PatternExpr::RepeatSoft {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "motif".to_string(),
                        rate: None,
                    }),
                    count: 4,
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["soft"];
        assert_eq!(rp.steps.len(), 4);
        assert_eq!(rp.boundaries.len(), 3);
        assert!(rp.boundaries.iter().all(|b| *b == Boundary::Soft));
    }

    #[test]
    fn test_resolve_hard_concat() {
        let blocks = vec![
            make_pattern("a", (1, 4), vec![make_step_line(vec![make_degree(1)])]),
            make_pattern("b", (1, 4), vec![make_step_line(vec![make_degree(5)])]),
            make_expr_pattern(
                "ab",
                PatternExpr::Concat {
                    left: Box::new(PatternExpr::Ref {
                        name: "a".to_string(),
                        rate: None,
                    }),
                    right: Box::new(PatternExpr::Ref {
                        name: "b".to_string(),
                        rate: None,
                    }),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["ab"];
        assert_eq!(rp.steps.len(), 2);
        assert_eq!(rp.boundaries, vec![Boundary::Hard]);
    }

    #[test]
    fn test_resolve_soft_concat() {
        let blocks = vec![
            make_pattern("a", (1, 4), vec![make_step_line(vec![make_degree(1)])]),
            make_pattern("b", (1, 4), vec![make_step_line(vec![make_degree(5)])]),
            make_expr_pattern(
                "ab",
                PatternExpr::ConcatSoft {
                    left: Box::new(PatternExpr::Ref {
                        name: "a".to_string(),
                        rate: None,
                    }),
                    right: Box::new(PatternExpr::Ref {
                        name: "b".to_string(),
                        rate: None,
                    }),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["ab"];
        assert_eq!(rp.steps.len(), 2);
        assert_eq!(rp.boundaries, vec![Boundary::Soft]);
    }

    #[test]
    fn test_unit_mismatch_error() {
        let blocks = vec![
            make_pattern("a", (1, 4), vec![make_step_line(vec![make_degree(1)])]),
            make_pattern("b", (1, 8), vec![make_step_line(vec![make_degree(5)])]),
            make_expr_pattern(
                "bad",
                PatternExpr::Concat {
                    left: Box::new(PatternExpr::Ref {
                        name: "a".to_string(),
                        rate: None,
                    }),
                    right: Box::new(PatternExpr::Ref {
                        name: "b".to_string(),
                        rate: None,
                    }),
                },
            ),
        ];

        let result = resolve_all(&blocks);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CompileError::UnitMismatch { .. }
        ));
    }

    #[test]
    fn test_forward_reference_error() {
        let blocks = vec![make_expr_pattern(
            "bad",
            PatternExpr::Ref {
                name: "nonexistent".to_string(),
                rate: None,
            },
        )];

        let result = resolve_all(&blocks);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            CompileError::ForwardReference { .. }
        ));
    }

    #[test]
    fn test_transform_reverse() {
        let blocks = vec![
            make_pattern(
                "up",
                (1, 4),
                vec![
                    make_step_line(vec![make_degree(1)]),
                    make_step_line(vec![make_degree(3)]),
                    make_step_line(vec![make_degree(5)]),
                ],
            ),
            make_expr_pattern(
                "down",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "up".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Reverse,
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["down"];
        assert_eq!(rp.steps.len(), 3);
        // First step should now be degree 5 (was last)
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::Degree { degree: 5, .. }
        ));
    }

    #[test]
    fn test_transform_rotate() {
        let blocks = vec![
            make_pattern(
                "base",
                (1, 4),
                vec![
                    make_step_line(vec![make_degree(1)]),
                    make_step_line(vec![make_degree(3)]),
                    make_step_line(vec![make_degree(5)]),
                ],
            ),
            make_expr_pattern(
                "rotated",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "base".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Rotate(1),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["rotated"];
        // rotate_left(1): [1, 3, 5] → [3, 5, 1]
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::Degree { degree: 3, .. }
        ));
        assert!(matches!(
            &rp.steps[2].tokens[0],
            StepToken::Degree { degree: 1, .. }
        ));
    }

    #[test]
    fn test_transform_subset() {
        let blocks = vec![
            make_pattern(
                "base",
                (1, 4),
                vec![
                    make_step_line(vec![make_degree(1)]),
                    make_step_line(vec![make_degree(3)]),
                    make_step_line(vec![make_degree(5)]),
                    make_step_line(vec![make_degree(7)]),
                ],
            ),
            make_expr_pattern(
                "sub",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "base".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Subset(vec![1, 3]),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["sub"];
        assert_eq!(rp.steps.len(), 2);
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::Degree { degree: 1, .. }
        ));
        assert!(matches!(
            &rp.steps[1].tokens[0],
            StepToken::Degree { degree: 5, .. }
        ));
    }

    #[test]
    fn test_transform_mirror() {
        let blocks = vec![
            make_pattern(
                "up",
                (1, 4),
                vec![
                    make_step_line(vec![make_degree(1)]),
                    make_step_line(vec![make_degree(3)]),
                ],
            ),
            make_expr_pattern(
                "mir",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "up".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Mirror,
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["mir"];
        assert_eq!(rp.steps.len(), 4); // [1, 3, 3, 1]
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::Degree { degree: 1, .. }
        ));
        assert!(matches!(
            &rp.steps[3].tokens[0],
            StepToken::Degree { degree: 1, .. }
        ));
    }

    #[test]
    fn test_transform_stretch() {
        let blocks = vec![
            make_pattern("fast", (1, 8), vec![make_step_line(vec![make_degree(1)])]),
            make_expr_pattern(
                "slow",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "fast".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Stretch(2, 1),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["slow"];
        assert_eq!(rp.unit, (1, 4)); // 1/8 * 2/1 = 2/8 = 1/4
    }

    #[test]
    fn test_transform_compress() {
        let blocks = vec![
            make_pattern("slow", (1, 4), vec![make_step_line(vec![make_degree(1)])]),
            make_expr_pattern(
                "fast",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "slow".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Compress(2, 1),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["fast"];
        assert_eq!(rp.unit, (1, 8)); // 1/4 * 1/2 = 1/8
    }

    #[test]
    fn test_mixed_operators() {
        // intro ~>> verse *2 >> outro
        let blocks = vec![
            make_pattern("intro", (1, 4), vec![make_step_line(vec![make_degree(1)])]),
            make_pattern("verse", (1, 4), vec![make_step_line(vec![make_degree(3)])]),
            make_pattern("outro", (1, 4), vec![make_step_line(vec![make_degree(5)])]),
            make_expr_pattern(
                "song",
                PatternExpr::ConcatSoft {
                    left: Box::new(PatternExpr::Ref {
                        name: "intro".to_string(),
                        rate: None,
                    }),
                    right: Box::new(PatternExpr::Concat {
                        left: Box::new(PatternExpr::Repeat {
                            pattern: Box::new(PatternExpr::Ref {
                                name: "verse".to_string(),
                                rate: None,
                            }),
                            count: 2,
                        }),
                        right: Box::new(PatternExpr::Ref {
                            name: "outro".to_string(),
                            rate: None,
                        }),
                    }),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["song"];
        assert_eq!(rp.steps.len(), 4); // 1 + 2 + 1
                                       // Boundaries: Soft (intro~>>verse), Hard (verse*2 internal), Hard (verse>>outro)
        assert_eq!(rp.boundaries.len(), 3);
        assert_eq!(rp.boundaries[0], Boundary::Soft); // intro ~>> verse
        assert_eq!(rp.boundaries[1], Boundary::Hard); // verse * 2 internal
        assert_eq!(rp.boundaries[2], Boundary::Hard); // verse >> outro
    }

    #[test]
    fn test_interleave() {
        let blocks = vec![
            make_pattern(
                "a",
                (1, 4),
                vec![
                    make_step_line(vec![make_degree(1)]),
                    make_step_line(vec![make_degree(3)]),
                ],
            ),
            make_pattern(
                "b",
                (1, 4),
                vec![
                    make_step_line(vec![make_degree(5)]),
                    make_step_line(vec![make_degree(7)]),
                ],
            ),
            make_expr_pattern(
                "mixed",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "a".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Interleave("b".to_string()),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["mixed"];
        assert_eq!(rp.steps.len(), 4); // [1, 5, 3, 7]
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::Degree { degree: 1, .. }
        ));
        assert!(matches!(
            &rp.steps[1].tokens[0],
            StepToken::Degree { degree: 5, .. }
        ));
    }

    #[test]
    fn test_interleave_mismatch() {
        let blocks = vec![
            make_pattern("a", (1, 4), vec![make_step_line(vec![make_degree(1)])]),
            make_pattern(
                "b",
                (1, 4),
                vec![
                    make_step_line(vec![make_degree(5)]),
                    make_step_line(vec![make_degree(7)]),
                ],
            ),
            make_expr_pattern(
                "bad",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "a".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Interleave("b".to_string()),
                },
            ),
        ];

        let result = resolve_all(&blocks);
        assert!(matches!(
            result.unwrap_err(),
            CompileError::InterleaveMismatch { .. }
        ));
    }

    #[test]
    fn test_chained_transforms() {
        // base | reverse | rotate(1)
        let blocks = vec![
            make_pattern(
                "base",
                (1, 4),
                vec![
                    make_step_line(vec![make_degree(1)]),
                    make_step_line(vec![make_degree(3)]),
                    make_step_line(vec![make_degree(5)]),
                ],
            ),
            make_expr_pattern(
                "chain",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Transform {
                        pattern: Box::new(PatternExpr::Ref {
                            name: "base".to_string(),
                            rate: None,
                        }),
                        transform: TransformCall::Reverse,
                    }),
                    transform: TransformCall::Rotate(1),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["chain"];
        // base: [1, 3, 5] → reverse: [5, 3, 1] → rotate(1): [3, 1, 5]
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::Degree { degree: 3, .. }
        ));
        assert!(matches!(
            &rp.steps[1].tokens[0],
            StepToken::Degree { degree: 1, .. }
        ));
        assert!(matches!(
            &rp.steps[2].tokens[0],
            StepToken::Degree { degree: 5, .. }
        ));
    }

    #[test]
    fn test_stretch_then_concat_compatible() {
        // fast (1/8) | stretch(2) becomes 1/4, then can concat with a 1/4 pattern
        let blocks = vec![
            make_pattern("fast", (1, 8), vec![make_step_line(vec![make_degree(1)])]),
            make_pattern("slow", (1, 4), vec![make_step_line(vec![make_degree(5)])]),
            make_expr_pattern(
                "combo",
                PatternExpr::Concat {
                    left: Box::new(PatternExpr::Transform {
                        pattern: Box::new(PatternExpr::Ref {
                            name: "fast".to_string(),
                            rate: None,
                        }),
                        transform: TransformCall::Stretch(2, 1),
                    }),
                    right: Box::new(PatternExpr::Ref {
                        name: "slow".to_string(),
                        rate: None,
                    }),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["combo"];
        assert_eq!(rp.steps.len(), 2);
        assert_eq!(rp.unit, (1, 4));
    }

    fn make_absolute_pitch(note: u8) -> StepToken {
        StepToken::AbsolutePitch {
            midi_note: note,
            annotations: Vec::new(),
        }
    }

    fn make_midi_number(note: u8) -> StepToken {
        StepToken::MidiNumber {
            note,
            annotations: Vec::new(),
        }
    }

    #[test]
    fn test_transpose_absolute_pitch() {
        let blocks = vec![
            make_pattern(
                "base",
                (1, 4),
                vec![
                    make_step_line(vec![make_absolute_pitch(60)]), // C4
                    make_step_line(vec![make_absolute_pitch(64)]), // E4
                ],
            ),
            make_expr_pattern(
                "up",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "base".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Transpose(7),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["up"];
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::AbsolutePitch { midi_note: 67, .. } // G4
        ));
        assert!(matches!(
            &rp.steps[1].tokens[0],
            StepToken::AbsolutePitch { midi_note: 71, .. } // B4
        ));
    }

    #[test]
    fn test_transpose_midi_number() {
        let blocks = vec![
            make_pattern(
                "base",
                (1, 4),
                vec![make_step_line(vec![make_midi_number(60)])],
            ),
            make_expr_pattern(
                "down",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "base".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Transpose(-12),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["down"];
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::MidiNumber { note: 48, .. }
        ));
    }

    #[test]
    fn test_transpose_clamps_to_range() {
        let blocks = vec![
            make_pattern(
                "high",
                (1, 4),
                vec![make_step_line(vec![make_absolute_pitch(120)])],
            ),
            make_expr_pattern(
                "too_high",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "high".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Transpose(20),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["too_high"];
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::AbsolutePitch { midi_note: 127, .. } // clamped
        ));
    }

    #[test]
    fn test_transpose_does_not_affect_degrees() {
        let blocks = vec![
            make_pattern("base", (1, 4), vec![make_step_line(vec![make_degree(3)])]),
            make_expr_pattern(
                "same",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "base".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Transpose(5),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["same"];
        // Degree should be unchanged
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::Degree {
                degree: 3,
                accidental: 0,
                ..
            }
        ));
    }

    #[test]
    fn test_shift_oct_degree() {
        let blocks = vec![
            make_pattern("base", (1, 4), vec![make_step_line(vec![make_degree(1)])]),
            make_expr_pattern(
                "up",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "base".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::ShiftOct(2),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["up"];
        // Default octave is 4, shifted by 2 → octave 6
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::Degree {
                degree: 1,
                octave: Some(6),
                ..
            }
        ));
    }

    #[test]
    fn test_shift_oct_absolute_pitch() {
        let blocks = vec![
            make_pattern(
                "base",
                (1, 4),
                vec![make_step_line(vec![make_absolute_pitch(60)])], // C4
            ),
            make_expr_pattern(
                "up",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "base".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::ShiftOct(1),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["up"];
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::AbsolutePitch { midi_note: 72, .. } // C5
        ));
    }

    #[test]
    fn test_invert_absolute_pitch_mirrors_around_first() {
        // Spec §10.2: intervals from the first pitch are negated.
        // [60, 64, 67] inverted around 60 → [60, 56, 53].
        let blocks = vec![
            make_pattern(
                "up",
                (1, 4),
                vec![
                    make_step_line(vec![make_absolute_pitch(60)]),
                    make_step_line(vec![make_absolute_pitch(64)]),
                    make_step_line(vec![make_absolute_pitch(67)]),
                ],
            ),
            make_expr_pattern(
                "inv",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "up".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Invert,
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["inv"];
        let notes: Vec<u8> = rp
            .steps
            .iter()
            .map(|s| match &s.tokens[0] {
                StepToken::AbsolutePitch { midi_note, .. } => *midi_note,
                other => panic!("unexpected token: {other:?}"),
            })
            .collect();
        assert_eq!(notes, vec![60, 56, 53]);
    }

    #[test]
    fn test_invert_degrees_diatonic() {
        // Degrees invert in scale-step space around the first degree.
        // ^1 ^3 ^5 (octave None → 4) → indices 28, 30, 32; mirrored around
        // 28 → 28, 26, 24 → ^1 oct4, ^6 oct3, ^4 oct3.
        let blocks = vec![
            make_pattern(
                "up",
                (1, 4),
                vec![
                    make_step_line(vec![make_degree(1)]),
                    make_step_line(vec![make_degree(3)]),
                    make_step_line(vec![make_degree(5)]),
                ],
            ),
            make_expr_pattern(
                "inv",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "up".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Invert,
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["inv"];
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::Degree {
                degree: 1,
                octave: Some(4),
                ..
            }
        ));
        assert!(matches!(
            &rp.steps[1].tokens[0],
            StepToken::Degree {
                degree: 6,
                octave: Some(3),
                ..
            }
        ));
        assert!(matches!(
            &rp.steps[2].tokens[0],
            StepToken::Degree {
                degree: 4,
                octave: Some(3),
                ..
            }
        ));
    }

    #[test]
    fn test_invert_negates_accidental() {
        let blocks = vec![
            make_pattern(
                "p",
                (1, 4),
                vec![
                    make_step_line(vec![make_degree(1)]),
                    make_step_line(vec![StepToken::Degree {
                        degree: 3,
                        accidental: -1, // b3
                        octave: None,
                        annotations: Vec::new(),
                    }]),
                ],
            ),
            make_expr_pattern(
                "inv",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "p".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Invert,
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["inv"];
        assert!(matches!(
            &rp.steps[1].tokens[0],
            StepToken::Degree {
                degree: 6,
                accidental: 1, // negated
                octave: Some(3),
                ..
            }
        ));
    }

    #[test]
    fn test_emission_transforms_recorded_on_instances() {
        // swing/humanize/vary applied through a pipeline are recorded on
        // every instance of the subexpression they pipe from — not applied
        // structurally.
        let blocks = vec![
            make_pattern("a", (1, 4), vec![make_step_line(vec![make_degree(1)])]),
            make_expr_pattern(
                "v",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Repeat {
                        pattern: Box::new(PatternExpr::Ref {
                            name: "a".to_string(),
                            rate: None,
                        }),
                        count: 2,
                    }),
                    transform: TransformCall::Vary(0.5),
                },
            ),
        ];
        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["v"];
        assert_eq!(rp.steps.len(), 2, "vary must not change step structure");
        assert_eq!(rp.pattern_instances.len(), 2);
        for inst in &rp.pattern_instances {
            assert_eq!(inst.emission_transforms.len(), 1);
            assert!(matches!(
                inst.emission_transforms[0],
                TransformCall::Vary(p) if (p - 0.5).abs() < 1e-9
            ));
        }
    }

    #[test]
    fn test_retrograde_is_reverse_then_invert_degrees() {
        // Spec §10.2: retrograde = reverse -> invert. ^1 ^3 ^5 reversed is
        // [^5 ^3 ^1]; inverted around the new first pitch (^5, scale index
        // 32 at default octave 4): 32→32, 30→34, 28→36, i.e.
        // ^5@4, ^7@4, ^2@5.
        let blocks = vec![
            make_pattern(
                "rise",
                (1, 4),
                vec![
                    make_step_line(vec![make_degree(1)]),
                    make_step_line(vec![make_degree(3)]),
                    make_step_line(vec![make_degree(5)]),
                ],
            ),
            make_expr_pattern(
                "retro",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "rise".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Retrograde,
                },
            ),
        ];
        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["retro"];
        let degs: Vec<(u8, Option<u8>)> = rp
            .steps
            .iter()
            .map(|s| match &s.tokens[0] {
                StepToken::Degree { degree, octave, .. } => (*degree, *octave),
                other => panic!("unexpected token: {other:?}"),
            })
            .collect();
        assert_eq!(degs, vec![(5, Some(4)), (7, Some(4)), (2, Some(5))]);
    }

    #[test]
    fn test_retrograde_is_reverse_then_invert_absolute() {
        // [60, 64, 67] reversed is [67, 64, 60]; inverted around 67:
        // [67, 70, 74] — classical retrograde-inversion.
        let blocks = vec![
            make_pattern(
                "rise",
                (1, 4),
                vec![
                    make_step_line(vec![make_absolute_pitch(60)]),
                    make_step_line(vec![make_absolute_pitch(64)]),
                    make_step_line(vec![make_absolute_pitch(67)]),
                ],
            ),
            make_expr_pattern(
                "retro",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "rise".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::Retrograde,
                },
            ),
        ];
        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["retro"];
        let notes: Vec<u8> = rp
            .steps
            .iter()
            .map(|s| match &s.tokens[0] {
                StepToken::AbsolutePitch { midi_note, .. } => *midi_note,
                other => panic!("unexpected token: {other:?}"),
            })
            .collect();
        assert_eq!(notes, vec![67, 70, 74]);
    }

    #[test]
    fn test_shift_oct_clamps() {
        let blocks = vec![
            make_pattern(
                "base",
                (1, 4),
                vec![make_step_line(vec![make_absolute_pitch(120)])],
            ),
            make_expr_pattern(
                "high",
                PatternExpr::Transform {
                    pattern: Box::new(PatternExpr::Ref {
                        name: "base".to_string(),
                        rate: None,
                    }),
                    transform: TransformCall::ShiftOct(2),
                },
            ),
        ];

        let resolved = resolve_all(&blocks).unwrap();
        let rp = &resolved["high"];
        assert!(matches!(
            &rp.steps[0].tokens[0],
            StepToken::AbsolutePitch { midi_note: 127, .. } // clamped
        ));
    }
}
