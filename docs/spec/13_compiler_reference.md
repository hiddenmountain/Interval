# 13. Compiler Reference

### 13.1 Timing Model

```
unit_ticks     = ppq * 4 * (unit_numerator / unit_denominator)
step_start     = step_index * unit_ticks   // step_index is 0-based
note_on_tick   = step_start + shift_ticks
note_duration  = unit_ticks * gate
note_off_tick  = note_on_tick + note_duration
```

For `dur=<fraction>` annotation, `note_duration = ppq * 4 * (dur_numerator / dur_denominator)`, overriding gate calculation entirely.

For `rate=<float>` on a track, `effective_unit_ticks = unit_ticks / rate`.

For per-reference `@rate`, `effective_unit_ticks = unit_ticks / (track_rate * segment_rate)`.

All tick values are integers. Fractional results are rounded to the nearest integer (round-half-up).

### 13.2 Microtiming Tick Conversion

Shift values are specified in one of three formats:

**Percent of step (`%`):**

```
shift_ticks = round((percent / 100) * unit_ticks)
```

**Fraction of whole note:**

```
shift_ticks = round(ppq * 4 * (numerator / denominator))
```

**Absolute milliseconds (`ms`):**

```
ticks_per_ms = (ppq * bpm) / 60000
shift_ticks  = round(shift_ms * ticks_per_ms)
```

### 13.3 Harmony Resolution

At each step tick position, the compiler queries the active harmony block for the current chord and the current mode/scale.

**`^n` degree tokens (scale-absolute):** Always resolve against the `@scale` context (or C major fallback). The resolution root is the `@scale root`. Scale intervals are looked up from the active scale/mode at the current tick position. No chord context is required.

**`%n` chord ordinal tokens:** The resolution root is the root of the active chord at the current tick. Chord-tone ordering is: root (`%1`), third (`%2`), fifth (`%3`), seventh (`%4`), then cyclic with octave wrapping from `%5` onward. Requires harmony context (`follow=` or auto-inferred).

**Without `follow=` (and no auto-inference):** `%n` and `$chord` tokens produce compile errors. `^n` degree tokens fall back to `@scale` context with a warning.

**Track mode override:** If a track has `mode=<mode>`, that mode is used for `^n` degree resolution instead of the inherited `@scale` mode.

**`$chord` resolution:** At each step tick, queries the harmony index for the current chord, then voices it according to the track's `voice=`, `inv=`, `oct=` settings. Requires harmony context.

**Degree-to-MIDI resolution for `^n` (scale-absolute):**

1. Identify the current scale root at this tick position (from `@scale` timeline or scalar).
2. Look up the interval for degree n from the active scale/mode interval table.
3. Apply accidentals (`^b`, `^#`): ±1 semitone after interval lookup.
4. Calculate MIDI note: `midi = (oct * 12 + 12) + scale_root_semitone + interval`
5. Apply `oct` override from annotation or track default.
6. Apply `shift_oct` transform offset if any.
7. Clamp to valid MIDI range 0–127.

**Chord-ordinal resolution for `%n`:**

1. Identify the current chord root at this tick position.
2. Determine the chord tone list from the chord quality's interval table.
3. Index into the chord tones: `chord_tone_index = (n - 1) % chord_tone_count`, `octave_offset = (n - 1) / chord_tone_count`.
4. Calculate MIDI note: `midi = (oct * 12 + 12) + chord_root_semitone + tone_interval + (octave_offset * 12)`
5. Apply forced octave `%n/oct` if specified.
6. Clamp to valid MIDI range 0–127.

### 13.4 Chord Voicing

**Step 1 — Collect raw pitches:** All intervals from the chord symbol.

**Step 2 — Apply voicing strategy** (close, open, drop2, shell, triad).

**Step 3 — Apply inversion.** Resolve final inversion from the hierarchy: `@harmony inv=` → `@track inv=` → step `[inv:N]`. For `inv=auto`, select the inversion that minimizes total voice movement from the prior chord (greedy).

**Step 4 — Octave placement.** Place the lowest note near `oct` octave.

**Step 5 — Slash bass.** If specified, add bass pitch class below the voicing.

### 13.5 Subdivision Compilation

```
subdivision_ticks = parent_step_ticks / n
token_k_start     = step_start + (k-1) * subdivision_ticks
```

Nested subdivisions recurse.

### 13.6 Lane Events (CC)

For ramp annotations (`expr:40->88`): four CC events are emitted per ramp per step at positions `[0, 0.25, 0.5, 0.75]` through the step duration.

For `[glide]` and `[glide:N]`: CC65=127 (Portamento On) + CC5=value are emitted before the NoteOn; CC65=0 is emitted after the NoteOff.

### 13.7 Overlapping Same-Pitch Notes

If a note-on is emitted for a pitch with an active note-on on the same channel: emit note-off first, then the new note-on at the same tick.

### 13.8 Swing Application

Swing tick offsets are applied during step emission when the token is not inside a `Subdivision` node, before events are emitted. This avoids needing a subdivision flag on every event.

### 13.9 Arp Emission

Arp transform is applied at emission time, after chord resolution but before event stream serialization. For each multi-note step, the resolved MIDI pitches are sorted per the arp pattern, then emitted as individual NoteOn/NoteOff pairs at intervals of `rate` ticks. The total duration of all arp notes equals the original step duration. Single-note steps are passed through unchanged.

### 13.10 Probability Evaluation

`[prob:N]` is evaluated per step during emission using the per-track xorshift64 RNG. If the random value exceeds the probability threshold, the step is replaced with a rest of equivalent duration. Probability is evaluated after conditional annotations but before arp expansion.

### 13.11 Compilation Order

1. Parse and validate global header.
2. Parse `@scale` block (scalar or timeline). Store as global tonal context.
3. Parse `@bpm` (scalar, inline, or block form). Build BPM timeline.
4. Parse `@ts` (scalar or inline form). Build time signature timeline.
5. Parse and validate all `@harmony` blocks. Resolve Roman numerals against `@scale`. Build harmony timeline index. Apply `@harmony inv=` defaults.
6. Parse and validate all `@drummap` blocks.
7. Parse and validate all `@pattern` blocks. Infer `steps=` from body when not declared. Resolve composition expressions. Apply baked-in transforms. Apply call-site transforms.
8. Parse and validate all `@track` blocks. Resolve `play:` expressions. Apply auto-inferred `follow=` where applicable.
9. Emit tempo events from `@bpm` timeline entries.
10. For each track: apply `start=` offset, iterate steps, query harmony context, resolve degrees (`^n` scale-absolute, `%n` chord-ordinal, `$chord`), apply voicing, calculate timing (with per-reference `@rate`), evaluate `[prob:N]`, apply swing/expressive/humanize, apply arp expansion, emit events.
11. Apply `vel_curve`, `gate_curve`, `scale_lock`, and `echo` transforms per track.
12. Sort all events per track by tick. Insert `BarMarker` and `PatternBoundary` events.
13. Pass event stream to the selected output target.

### 13.12 CompileOutput

The `compile()` function returns `CompileOutput`:

```rust
pub struct CompileOutput {
    pub events: EventStream,
    pub ppq: u32,
    pub warnings: Vec<CompileWarning>,
    pub tracks: Vec<TrackSummary>,
    pub program: Option<Program>,
}
```

`tracks` is populated during compilation with per-track metadata. `program` is `None`
by default; use `compile_with_ast()` to get it populated.

---

[← Back to Table of Contents](00_toc.md)
