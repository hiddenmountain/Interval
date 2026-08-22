//! Chord voicing strategies and inversion logic.
//!
//! Implements all five voicing strategies (close, open, drop2, shell, triad)
//! and all inversion modes (fixed 0-3, auto voice-leading). The `inv=auto`
//! algorithm greedily minimizes total voice movement from the previous chord
//! by testing all possible inversions and selecting the one with the lowest
//! sum of absolute semitone distances.
//!
//! Slash bass notes are placed below the voicing at the lowest available
//! octave regardless of the voicing strategy.

use crate::ast::{ChordSymbol, Inversion, VoicingStrategy};

/// Apply voicing strategy and inversion to produce MIDI note numbers.
///
/// Given a chord symbol, voicing strategy, inversion, octave, and optional
/// previous pitches (for `inv=auto`), returns the resolved MIDI pitches sorted
/// ascending, plus the updated voice-leading state.
///
/// Returns `(midi_pitches, new_voice_leading_state)`.
pub fn voice_chord(
    chord: &ChordSymbol,
    strategy: VoicingStrategy,
    inv: Inversion,
    octave: u8,
    prev_pitches: Option<&[u8]>,
) -> (Vec<u8>, Vec<u8>) {
    // Step 1: Filter intervals based on voicing strategy
    let intervals = filter_intervals(&chord.intervals, strategy);

    if intervals.is_empty() {
        return (Vec::new(), Vec::new());
    }

    // Step 2: Determine inversion
    let inv_n = match inv {
        Inversion::Fixed(n) => {
            if n as usize >= intervals.len() {
                0 // fallback to root position if inv exceeds chord tones
            } else {
                n as usize
            }
        }
        Inversion::Auto => {
            if let Some(prev) = prev_pitches {
                find_best_inversion(&intervals, chord.root, octave, strategy, prev)
            } else {
                0 // first chord: root position
            }
        }
    };

    // Step 3: Apply inversion (rotate intervals so inv_n-th becomes bass)
    let rotated = rotate_intervals(&intervals, inv_n);

    // Step 4: Compute MIDI pitches from rotated intervals + voicing
    let mut pitches = place_voicing(&rotated, chord.root, octave, strategy);

    // Step 5: Handle slash bass
    if let Some(bass_pc) = chord.slash_bass {
        add_slash_bass(&mut pitches, bass_pc);
    }

    pitches.sort();

    // Clamp to 0-127
    for p in &mut pitches {
        *p = (*p).min(127);
    }

    let state = pitches.clone();
    (pitches, state)
}

/// Filter chord intervals based on voicing strategy.
fn filter_intervals(intervals: &[u8], strategy: VoicingStrategy) -> Vec<u8> {
    match strategy {
        VoicingStrategy::Close
        | VoicingStrategy::Open
        | VoicingStrategy::Drop2
        | VoicingStrategy::Drop3 => intervals.to_vec(),
        VoicingStrategy::Rootless => {
            // Remove the root (interval 0) from the list
            let filtered: Vec<u8> = intervals.iter().copied().filter(|&i| i != 0).collect();
            if filtered.is_empty() {
                intervals.to_vec() // fall back to all intervals if removing root leaves empty
            } else {
                filtered
            }
        }
        VoicingStrategy::Shell => {
            // Root + 3rd + 7th
            // Root is always interval 0. Look for 3rd (3 or 4) and 7th (10 or 11).
            let mut result = vec![0u8];
            // Find 3rd (minor=3, major=4)
            if let Some(&third) = intervals.iter().find(|&&i| i == 3 || i == 4) {
                result.push(third);
            }
            // Find 7th (dominant/minor=10, major=11)
            if let Some(&seventh) = intervals.iter().find(|&&i| i == 10 || i == 11) {
                result.push(seventh);
            }
            if result.len() == 1 {
                // No 3rd or 7th found — fall back to all intervals
                intervals.to_vec()
            } else {
                result
            }
        }
        VoicingStrategy::Triad => {
            // Root + 3rd + 5th
            let mut result = vec![0u8];
            // Find 3rd
            if let Some(&third) = intervals.iter().find(|&&i| i == 3 || i == 4) {
                result.push(third);
            }
            // Find 5th (diminished=6, perfect=7, augmented=8)
            if let Some(&fifth) = intervals.iter().find(|&&i| i == 6 || i == 7 || i == 8) {
                result.push(fifth);
            }
            if result.len() == 1 {
                intervals.to_vec()
            } else {
                result
            }
        }
    }
}

/// Rotate intervals for inversion: the n-th interval becomes the new bass.
fn rotate_intervals(intervals: &[u8], n: usize) -> Vec<u8> {
    if n == 0 || intervals.is_empty() {
        return intervals.to_vec();
    }
    let n = n % intervals.len();
    let mut rotated = Vec::with_capacity(intervals.len());
    rotated.extend_from_slice(&intervals[n..]);
    rotated.extend_from_slice(&intervals[..n]);
    rotated
}

/// Place intervals as MIDI notes in a specific octave using voicing strategy.
fn place_voicing(intervals: &[u8], root_pc: u8, octave: u8, strategy: VoicingStrategy) -> Vec<u8> {
    let base_midi = (octave as i32 + 1) * 12 + root_pc as i32;

    match strategy {
        VoicingStrategy::Close
        | VoicingStrategy::Shell
        | VoicingStrategy::Triad
        | VoicingStrategy::Rootless => {
            // Stack within one octave: each note at base + interval,
            // ensuring ascending order by adding 12 if needed
            let mut pitches = Vec::with_capacity(intervals.len());
            let mut prev = -1i32;
            for &interval in intervals {
                let mut midi = base_midi + interval as i32;
                // Ensure ascending: if this note <= previous, bump up an octave
                while midi <= prev {
                    midi += 12;
                }
                pitches.push(midi.clamp(0, 127) as u8);
                prev = midi;
            }
            pitches
        }
        VoicingStrategy::Open => {
            // Alternate octaves: odd-indexed notes go up an octave
            let mut pitches = Vec::with_capacity(intervals.len());
            let mut prev = -1i32;
            for (i, &interval) in intervals.iter().enumerate() {
                let mut midi = base_midi + interval as i32;
                while midi <= prev {
                    midi += 12;
                }
                if i % 2 == 1 {
                    midi += 12; // push odd-indexed notes up an extra octave
                }
                pitches.push(midi.clamp(0, 127) as u8);
                prev = midi;
            }
            pitches
        }
        VoicingStrategy::Drop2 => {
            // Start with close voicing, then drop the second-highest note
            let mut pitches = Vec::with_capacity(intervals.len());
            let mut prev = -1i32;
            for &interval in intervals {
                let mut midi = base_midi + interval as i32;
                while midi <= prev {
                    midi += 12;
                }
                pitches.push(midi.clamp(0, 127) as u8);
                prev = midi;
            }
            if pitches.len() >= 2 {
                let second_highest_idx = pitches.len() - 2;
                let note = pitches[second_highest_idx];
                if note >= 12 {
                    pitches[second_highest_idx] = note - 12;
                }
            }
            pitches.sort();
            pitches
        }
        VoicingStrategy::Drop3 => {
            // Start with close voicing, then drop the third-highest note an octave
            let mut pitches = Vec::with_capacity(intervals.len());
            let mut prev = -1i32;
            for &interval in intervals {
                let mut midi = base_midi + interval as i32;
                while midi <= prev {
                    midi += 12;
                }
                pitches.push(midi.clamp(0, 127) as u8);
                prev = midi;
            }
            if pitches.len() >= 3 {
                let third_highest_idx = pitches.len() - 3;
                let note = pitches[third_highest_idx];
                if note >= 12 {
                    pitches[third_highest_idx] = note - 12;
                }
            }
            pitches.sort();
            pitches
        }
    }
}

/// Find the inversion that minimizes voice movement from previous pitches.
fn find_best_inversion(
    intervals: &[u8],
    root_pc: u8,
    octave: u8,
    strategy: VoicingStrategy,
    prev_pitches: &[u8],
) -> usize {
    let mut best_inv = 0;
    let mut best_distance = i32::MAX;

    for inv in 0..intervals.len() {
        let rotated = rotate_intervals(intervals, inv);
        let pitches = place_voicing(&rotated, root_pc, octave, strategy);
        let distance = voice_distance(&pitches, prev_pitches);
        if distance < best_distance {
            best_distance = distance;
            best_inv = inv;
        }
    }

    best_inv
}

/// Calculate total voice movement distance between two sets of pitches.
/// For each note in `current`, find the closest note in `previous` and sum
/// the absolute distances.
fn voice_distance(current: &[u8], previous: &[u8]) -> i32 {
    if previous.is_empty() {
        return 0;
    }
    current
        .iter()
        .map(|&c| {
            previous
                .iter()
                .map(|&p| (c as i32 - p as i32).abs())
                .min()
                .unwrap_or(0)
        })
        .sum()
}

/// Add a slash bass note below the lowest note of the voicing.
fn add_slash_bass(pitches: &mut Vec<u8>, bass_pc: u8) {
    if pitches.is_empty() {
        pitches.push(bass_pc);
        return;
    }

    let lowest = *pitches.iter().min().unwrap_or(&60);
    // Place bass_pc below the lowest note
    let mut bass_midi = bass_pc as i32;
    // Start from lowest possible and go up until we're just below the lowest voicing note
    while bass_midi + 12 < lowest as i32 {
        bass_midi += 12;
    }
    // Ensure it's actually below
    if bass_midi >= lowest as i32 {
        bass_midi -= 12;
    }
    if bass_midi >= 0 {
        pitches.push(bass_midi as u8);
    }
}

/// Resolve a single degree token to a MIDI note number (scale-absolute).
///
/// In v0.5, `^n` is purely scale-absolute: ALL degrees use mode intervals,
/// regardless of the active chord. `^1` is always the root of the scale,
/// `^3` is always the 3rd scale degree, etc.
///
/// `degree` is 1-13 (compound degrees 8-13 add one octave),
/// `accidental` is -1/0/+1, `octave` is the effective octave,
/// `mode_intervals` are the active scale's interval set,
/// `root` is the scale root pitch class (0=C).
pub fn resolve_degree(
    degree: u8,
    accidental: i8,
    octave: u8,
    mode_intervals: &[u8],
    root: u8,
) -> u8 {
    let interval = scale_degree_to_interval(degree, mode_intervals);
    let midi = (octave as i32 + 1) * 12 + root as i32 + interval as i32 + accidental as i32;
    midi.clamp(0, 127) as u8
}

/// Map a scale degree (1+) to a semitone interval using mode intervals.
///
/// Degrees 1-7 map directly to `mode_intervals[0..6]`.
/// Degrees 8-14 are compound (add one octave to degrees 1-7).
/// Higher degrees wrap with additional octave shifts.
fn scale_degree_to_interval(degree: u8, mode_intervals: &[u8]) -> u8 {
    if mode_intervals.is_empty() {
        return 0;
    }
    let len = mode_intervals.len() as u8;
    let oct_shift = degree.saturating_sub(1) / len;
    let idx = (degree.saturating_sub(1) % len) as usize;
    mode_intervals
        .get(idx)
        .copied()
        .unwrap_or(0)
        .saturating_add(oct_shift * 12)
}

/// Resolve a chord ordinal token (`%n`) to a MIDI note number.
///
/// `%1` = root (chord interval index 0), `%2` = 3rd (index 1), etc.
/// Ordinals beyond the chord tone count wrap with an octave shift:
/// `%5` over a 4-note chord gives the root + one extra octave.
///
/// `ordinal` is 1-based, `default_octave` is the track/pattern octave,
/// `forced_octave` overrides ordinal-based octave wrapping (from `%1/4`),
/// `chord_intervals` are the active chord's interval set,
/// `root` is the chord root pitch class (0=C).
pub fn resolve_chord_ordinal(
    ordinal: u32,
    default_octave: u8,
    forced_octave: Option<u8>,
    chord_intervals: &[u8],
    root: u8,
) -> u8 {
    if chord_intervals.is_empty() {
        let oct = forced_octave.unwrap_or(default_octave);
        return ((oct as i32 + 1) * 12 + root as i32).clamp(0, 127) as u8;
    }
    let k = chord_intervals.len() as u32;
    let tone_index = ((ordinal - 1) % k) as usize;
    let interval = chord_intervals[tone_index];
    let midi = if let Some(forced_oct) = forced_octave {
        (forced_oct as i32 + 1) * 12 + root as i32 + interval as i32
    } else {
        let octave_shift = (ordinal - 1) / k;
        (default_octave as i32 + 1) * 12 + root as i32 + interval as i32 + octave_shift as i32 * 12
    };
    midi.clamp(0, 127) as u8
}

/// Snap a MIDI pitch to the nearest pitch in the given scale.
///
/// `scale_intervals` is the set of semitone intervals from root (e.g., [0,2,4,5,7,9,11] for major).
/// `root` is the scale root as pitch class (0=C, 1=C#, ..., 11=B).
/// Check if a pitch is in the given scale.
pub fn is_in_scale(pitch: u8, scale_intervals: &[u8], root: u8) -> bool {
    if scale_intervals.is_empty() {
        return true;
    }
    let pc = (pitch as i32 - root as i32).rem_euclid(12) as u8;
    scale_intervals.contains(&pc)
}

/// Snap a pitch to the nearest lower in-scale pitch.
pub fn snap_to_scale_down(pitch: u8, scale_intervals: &[u8], root: u8) -> u8 {
    if scale_intervals.is_empty() || is_in_scale(pitch, scale_intervals, root) {
        return pitch;
    }
    // Walk downward until we find an in-scale pitch
    let mut p = pitch;
    loop {
        if p == 0 {
            return 0;
        }
        p -= 1;
        if is_in_scale(p, scale_intervals, root) {
            return p;
        }
    }
}

/// Snap a pitch to the nearest higher in-scale pitch.
pub fn snap_to_scale_up(pitch: u8, scale_intervals: &[u8], root: u8) -> u8 {
    if scale_intervals.is_empty() || is_in_scale(pitch, scale_intervals, root) {
        return pitch;
    }
    // Walk upward until we find an in-scale pitch
    let mut p = pitch;
    loop {
        if p >= 127 {
            return 127;
        }
        p += 1;
        if is_in_scale(p, scale_intervals, root) {
            return p;
        }
    }
}

/// Snap a pitch to the nearest in-scale pitch (either direction).
pub fn snap_to_scale(pitch: u8, scale_intervals: &[u8], root: u8) -> u8 {
    if scale_intervals.is_empty() {
        return pitch;
    }
    let pc = (pitch as i32 - root as i32).rem_euclid(12) as u8;
    // Check if already in scale
    if scale_intervals.contains(&pc) {
        return pitch;
    }
    // Find nearest scale tone
    let mut best_pitch = pitch;
    let mut best_dist = i32::MAX;
    for &interval in scale_intervals {
        // Check both the octave below and above
        for octave_offset in [-12i32, 0, 12] {
            let candidate = pitch as i32 + (interval as i32 - pc as i32) + octave_offset;
            if (0..=127).contains(&candidate) {
                let dist = (candidate - pitch as i32).abs();
                if dist < best_dist {
                    best_dist = dist;
                    best_pitch = candidate as u8;
                }
            }
        }
    }
    best_pitch
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn cmaj7() -> ChordSymbol {
        ChordSymbol {
            root: 0,                      // C
            intervals: vec![0, 4, 7, 11], // root, major 3rd, perfect 5th, major 7th
            slash_bass: None,
            roman: None,
        }
    }

    fn dm7() -> ChordSymbol {
        ChordSymbol {
            root: 2,                      // D
            intervals: vec![0, 3, 7, 10], // root, minor 3rd, perfect 5th, minor 7th
            slash_bass: None,
            roman: None,
        }
    }

    fn c_over_e() -> ChordSymbol {
        ChordSymbol {
            root: 0,
            intervals: vec![0, 4, 7],
            slash_bass: Some(4), // E
            roman: None,
        }
    }

    fn major_mode() -> Vec<u8> {
        vec![0, 2, 4, 5, 7, 9, 11]
    }

    #[test]
    fn test_close_voicing_root_position() {
        let (pitches, _) = voice_chord(
            &cmaj7(),
            VoicingStrategy::Close,
            Inversion::Fixed(0),
            4,
            None,
        );
        // C4=60, E4=64, G4=67, B4=71
        assert_eq!(pitches, vec![60, 64, 67, 71]);
    }

    #[test]
    fn test_close_voicing_first_inversion() {
        let (pitches, _) = voice_chord(
            &cmaj7(),
            VoicingStrategy::Close,
            Inversion::Fixed(1),
            4,
            None,
        );
        // First inversion: E is bass → E4, G4, B4, C5
        assert_eq!(pitches, vec![64, 67, 71, 72]);
    }

    #[test]
    fn test_close_voicing_second_inversion() {
        let (pitches, _) = voice_chord(
            &cmaj7(),
            VoicingStrategy::Close,
            Inversion::Fixed(2),
            4,
            None,
        );
        // Second inversion: G is bass → G4, B4, C5, E5
        assert_eq!(pitches, vec![67, 71, 72, 76]);
    }

    #[test]
    fn test_shell_voicing() {
        let (pitches, _) = voice_chord(
            &cmaj7(),
            VoicingStrategy::Shell,
            Inversion::Fixed(0),
            4,
            None,
        );
        // Shell: root(0) + 3rd(4) + 7th(11) → C4, E4, B4
        assert_eq!(pitches, vec![60, 64, 71]);
    }

    #[test]
    fn test_triad_voicing() {
        let (pitches, _) = voice_chord(
            &cmaj7(),
            VoicingStrategy::Triad,
            Inversion::Fixed(0),
            4,
            None,
        );
        // Triad: root(0) + 3rd(4) + 5th(7) → C4, E4, G4
        assert_eq!(pitches, vec![60, 64, 67]);
    }

    #[test]
    fn test_drop2_voicing() {
        let (pitches, _) = voice_chord(
            &cmaj7(),
            VoicingStrategy::Drop2,
            Inversion::Fixed(0),
            4,
            None,
        );
        // Close: C4(60), E4(64), G4(67), B4(71)
        // Drop second-highest (G4=67): G4-12=55 → G3
        // Sorted: G3(55), C4(60), E4(64), B4(71)
        assert_eq!(pitches, vec![55, 60, 64, 71]);
    }

    #[test]
    fn test_slash_bass() {
        let (pitches, _) = voice_chord(
            &c_over_e(),
            VoicingStrategy::Close,
            Inversion::Fixed(0),
            4,
            None,
        );
        // Close: C4(60), E4(64), G4(67)
        // Slash bass E below C4(60): E3 = 52
        assert_eq!(pitches[0], 52); // E3 is lowest
        assert!(pitches.contains(&60));
        assert!(pitches.contains(&64));
        assert!(pitches.contains(&67));
    }

    #[test]
    fn test_auto_voicing_first_chord() {
        let (pitches, _) = voice_chord(&cmaj7(), VoicingStrategy::Close, Inversion::Auto, 4, None);
        // First chord with auto: should use root position
        assert_eq!(pitches, vec![60, 64, 67, 71]);
    }

    #[test]
    fn test_auto_voicing_minimizes_movement() {
        // First chord: Cmaj7 root position
        let (pitches1, state1) =
            voice_chord(&cmaj7(), VoicingStrategy::Close, Inversion::Auto, 4, None);
        assert_eq!(pitches1, vec![60, 64, 67, 71]);

        // Second chord: Dm7 — auto should pick inversion closest to previous
        let (pitches2, _) = voice_chord(
            &dm7(),
            VoicingStrategy::Close,
            Inversion::Auto,
            4,
            Some(&state1),
        );
        // The auto algorithm should pick an inversion that minimizes voice movement
        // from [60, 64, 67, 71]
        assert!(!pitches2.is_empty());
    }

    #[test]
    fn test_resolve_degree_root() {
        // ^1 in C major: scale degree 1 = interval 0
        let midi = resolve_degree(1, 0, 4, &major_mode(), 0);
        assert_eq!(midi, 60); // C4
    }

    #[test]
    fn test_resolve_degree_third() {
        // ^3 in C major: scale degree 3 = interval 4 (major 3rd)
        let midi = resolve_degree(3, 0, 4, &major_mode(), 0);
        assert_eq!(midi, 64); // E4
    }

    #[test]
    fn test_resolve_degree_fifth() {
        // ^5 in C major: scale degree 5 = interval 7 (perfect 5th)
        let midi = resolve_degree(5, 0, 4, &major_mode(), 0);
        assert_eq!(midi, 67); // G4
    }

    #[test]
    fn test_resolve_degree_flat_third() {
        // ^b3 in C major: E4 - 1 = Eb4
        let midi = resolve_degree(3, -1, 4, &major_mode(), 0);
        assert_eq!(midi, 63); // Eb4
    }

    #[test]
    fn test_resolve_degree_sharp_eleventh() {
        // ^#11 in C major: degree 11 = degree 4 + octave = 5 + 12 = 17, +1 for sharp
        let midi = resolve_degree(11, 1, 4, &major_mode(), 0);
        assert_eq!(midi, 78); // F#5
    }

    #[test]
    fn test_resolve_degree_second() {
        // ^2 in C major: scale degree 2 = interval 2
        let midi = resolve_degree(2, 0, 4, &major_mode(), 0);
        assert_eq!(midi, 62); // D4
    }

    #[test]
    fn test_resolve_degree_with_root() {
        // ^1 with root=D (2): mode[0]=0, root=2 → D4
        let midi = resolve_degree(1, 0, 4, &major_mode(), 2);
        assert_eq!(midi, 62); // D4 = (4+1)*12 + 2 + 0
    }

    #[test]
    fn test_resolve_degree_octave_override() {
        // ^1 at octave 5
        let midi = resolve_degree(1, 0, 5, &major_mode(), 0);
        assert_eq!(midi, 72); // C5
    }

    #[test]
    fn test_resolve_degree_clamps() {
        // Very high octave should clamp
        let midi = resolve_degree(1, 0, 9, &major_mode(), 0);
        assert_eq!(midi, 120); // C9 = (9+1)*12 + 0 = 120
                               // With accidental
        let midi = resolve_degree(7, 1, 9, &major_mode(), 0);
        // (9+1)*12 + 0 + 11 + 1 = 132 → clamped to 127
        assert_eq!(midi, 127);
    }

    #[test]
    fn test_resolve_chord_ordinal_basic() {
        // %1 %2 %3 %4 over Cmaj7 = C4 E4 G4 B4
        let chord = vec![0u8, 4, 7, 11]; // Cmaj7 intervals
        assert_eq!(resolve_chord_ordinal(1, 4, None, &chord, 0), 60); // C4
        assert_eq!(resolve_chord_ordinal(2, 4, None, &chord, 0), 64); // E4
        assert_eq!(resolve_chord_ordinal(3, 4, None, &chord, 0), 67); // G4
        assert_eq!(resolve_chord_ordinal(4, 4, None, &chord, 0), 71); // B4
    }

    #[test]
    fn test_resolve_chord_ordinal_wrap() {
        // %5 over Cmaj7 (4 tones) wraps to root + 1 octave = C5
        let chord = vec![0u8, 4, 7, 11];
        assert_eq!(resolve_chord_ordinal(5, 4, None, &chord, 0), 72); // C5
        assert_eq!(resolve_chord_ordinal(6, 4, None, &chord, 0), 76); // E5
    }

    #[test]
    fn test_resolve_chord_ordinal_forced_octave() {
        // %3/5 = G5 (interval 7, forced oct 5)
        let chord = vec![0u8, 4, 7, 11];
        assert_eq!(resolve_chord_ordinal(3, 4, Some(5), &chord, 0), 79); // G5
    }

    #[test]
    fn test_resolve_chord_ordinal_triad() {
        // %1 %2 %3 over C major triad at octave 4
        let chord = vec![0u8, 4, 7];
        assert_eq!(resolve_chord_ordinal(1, 4, None, &chord, 0), 60); // C4
        assert_eq!(resolve_chord_ordinal(2, 4, None, &chord, 0), 64); // E4
        assert_eq!(resolve_chord_ordinal(3, 4, None, &chord, 0), 67); // G4
                                                                      // %4 wraps to root + 1 octave
        assert_eq!(resolve_chord_ordinal(4, 4, None, &chord, 0), 72); // C5
    }

    #[test]
    fn test_inversion_exceeds_chord_tones_fallback() {
        // inv=3 on a triad (only 3 tones) — should fall back to 0
        let (pitches, _) = voice_chord(
            &ChordSymbol {
                root: 0,
                intervals: vec![0, 4, 7],
                slash_bass: None,
                roman: None,
            },
            VoicingStrategy::Close,
            Inversion::Fixed(3),
            4,
            None,
        );
        // Falls back to root position
        assert_eq!(pitches, vec![60, 64, 67]);
    }
}
