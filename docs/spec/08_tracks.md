# 8. Tracks

Tracks wire patterns to MIDI output channels and declare their relationship to the harmony timeline.

### 8.1 Declaration

```
@track <name>
  ch=<int>
  [prog=<int|gm_name>]
  [unit=<fraction>]
  [oct=<int>]
  [vel=<int>]
  [gate=<float>]
  [shift=<timing>]
  [lshift=<timing>]
  [follow=<harmony_name>]
  [voice=<voicing>]
  [inv=<int|auto>]
  [seed=<int>]
  [mode=<mode>]
  [rate=<float>]
  [swing=<float>]
  [swingunit=<fraction>]
  [start=<int>]
  [type=drums]
  [drummap=<name>]
  play: ...
  or
  steps: ...
```

Track parameters may be written on a single line or across multiple indented lines.

### 8.2 Track Parameters

|Parameter|Default|Description|
|---|---|---|
|`ch`|Required|MIDI channel 1–16|
|`prog`|none|GM program number 0–127, or GM name string|
|`unit`|Inherited from pattern|Default step unit for inline `steps:` block|
|`oct`|4|Default octave for degree resolution|
|`vel`|84|Default velocity|
|`gate`|0.9|Default gate ratio|
|`shift`|0|Global microtiming shift for note events|
|`lshift`|0|Global microtiming shift for CC/controller events|
|`follow`|auto-inferred if single harmony block|Name of harmony block for degree resolution|
|`voice`|`close`|Chord voicing strategy|
|`inv`|0|Voicing inversion. `auto` enables voice-leading optimization|
|`seed`|global seed|Per-track seed for variant and generative operations|
|`mode`|inherited from `@scale`|Override scale mode for degree resolution on this track only|
|`rate`|1.0|Playback rate multiplier (2.0 = double speed, 0.5 = half speed)|
|`swing`|none|Swing ratio (0.5 = straight, 0.67 = triplet swing)|
|`swingunit`|none|Swing unit as fraction (e.g. `1/8`)|
|`start`|1|Start bar (1-indexed). Events offset by `(start-1) * ticks_per_bar`|

### 8.3 Track Start Bar

`start=N` delays a track's first event to bar N. All events are offset by `(N - 1) * ticks_per_bar` ticks.

```
@track bass ch=2 start=5 follow=main
play: walking_bass * 8
// First note at tick (5-1)*1920 = 7680 (ppq=480, 4/4)
// Harmony resolves against bar 5's chord
```

`start=0` is a compile error (bars are 1-indexed). `start=1` is the default (no offset).

### 8.4 Track-Level Rate

A playback speed multiplier, distinct from the `stretch` transform. `stretch` is compile-time — it bakes new durations into the pattern definition. `rate` is a track property — the pattern definition is unchanged but events are emitted at a different speed.

```
@track piano rate=0.5    // half speed
@track keys  rate=2.0    // double speed
// both can reference the same pattern definition
```

**Harmony locking:** A rate-modified track remains locked to the correct harmony. Harmony resolution happens at the actual emitted tick, not at the pattern's internal position.

### 8.5 Track-Level Swing

Swing is a systematic rhythmic deformation — every even subdivision at a specified unit is pushed late by a consistent amount.

```
@track piano swing=0.67 swingunit=1/8    // triplet jazz swing on 8ths
@track drums swing=0.55 swingunit=1/16   // light funk shuffle on 16ths
```

**Tick calculation:**

```
swing_shift = unit_ticks * (ratio - 0.5) * 2
```

All even-numbered subdivisions of `swingunit` are shifted late by `swing_shift` ticks. Odd subdivisions are unaffected.

**Common ratio reference:**

|Ratio|Feel|
|---|---|
|`0.50`|Straight (no swing)|
|`0.54`|Very light swing|
|`0.58`|Light swing|
|`0.62`|Medium swing|
|`0.67`|Hard swing (triplet, 2:1)|

Swing does not apply inside subdivision brackets `(...)` — triplets are already triplets and swing must not recurse into them.

If both track-level `swing=` and pipeline `-> swing(...)` are present, the pipeline transform takes precedence for that pattern; track-level swing applies to everything else on the track.

### 8.6 Voicing Strategies

Applied to all multi-note steps (simultaneous notes, chord symbols, `%n` clusters, `$chord`).

|Value|Definition|
|---|---|
|`close`|All chord tones within a single octave above the lowest note|
|`open`|Notes spread freely; widest common voicing|
|`drop2`|Second-highest note of close voicing transposed down one octave|
|`shell`|Root + 3rd + 7th only. 5th omitted|
|`drop3`|Third-highest note of close voicing transposed down one octave|
|`rootless`|Root omitted from voicing. Common in jazz piano left-hand comping|
|`triad`|1st, 3rd, 5th only|

**Inversion (`inv`):**

- `inv=0` — root position (root is lowest note)
- `inv=1` — first inversion (3rd is lowest note)
- `inv=2` — second inversion (5th is lowest note)
- `inv=3` — third inversion (7th is lowest note, for seventh chords)
- `inv=auto` — compiler minimizes total voice movement between consecutive chord steps. State persists across pattern boundaries within the track's `play:` expression.

**Inversion hierarchy:** The final inversion for any step is determined by: `@harmony inv=` (base default) < `@track inv=` (overrides the harmony default when set to a non-default value). A step-level `[inv:N]` annotation is reserved for a future version and is not currently accepted.

### 8.7 Follow Directive

`follow=<name>` links the track to a harmony block and activates chord-relative degree resolution for `%n` tokens. `$chord` tokens also resolve against this harmony.

`follow=` is **auto-inferred** when exactly one harmony block exists in the file. Explicit `follow=` is accepted and takes precedence over auto-inference.

If `follow=` is absent and cannot be inferred (multiple harmony blocks or no harmony blocks), `%n` and `$chord` tokens produce compile errors. `^n` degree tokens fall back to the `@scale` context with a warning.

### 8.8 Play Directive

```
@track piano
  ch=1 prog=1 oct=4 follow=main
  play: verse_comp >> chorus_comp * 2 >> verse_comp >> outro
```

Transformations are applied using the `->` operator in `play:` contexts:

```
play: arp_up >> arp_up * 2 >> arp_up -> reverse
```

### 8.9 Steps Directive

For one-off patterns that don't warrant a standalone `@pattern` declaration:

```
@track pad
  ch=3 prog=48 oct=4 unit=1/2 follow=main
  steps:
    ^1+^3+^5+^9
    ~
    ~
    ~
```

`play:` and `steps:` are mutually exclusive.

### 8.10 Microtiming

`shift=<value>` applies a global timing offset to all note events. `lshift=<value>` applies to CC/controller events only.

Accepts `%` (percent of step), fraction (`1/32`), or `ms` (absolute milliseconds). Positive = late, negative = early.

Per-step `shift` annotations override the track-level shift for that step. They do not accumulate.

### 8.11 MIDI Program Change

If `prog=` is specified, a MIDI Program Change event is emitted at tick 0. GM program names are accepted (case-insensitive, underscores for spaces): `prog="acoustic_grand_piano"` or `prog=0`.

---

[← Back to Table of Contents](00_toc.md)
