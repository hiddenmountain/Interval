# 10. Transformation Pipeline

Transformations are applied using the `->` pipe operator in `play:` directives, `@pattern` assignment expressions, or as baked-in transforms on `@pattern` declarations. They execute in three phases:

- **Structural transforms** (`reverse`, `transpose`, `rotate`, `stretch`, etc.): applied at pattern resolution time. Modify the step sequence before any iteration begins.
- **Emission transforms** (`swing`, expressive transforms, `humanize`, `vary`, `evolve`, `euclid_gate`, `arp`): offsets are precomputed, then applied per-step during event iteration. Do not change step structure.
- **Stream transforms** (`vel_curve`, `gate_curve`, `scale_lock`, `echo`): applied to the finalized event stream after all steps have been emitted (see §13.9 step 10). Modify note values only; cannot alter timing structure.

### 10.1 Pipe Operator

```
pattern_name -> transform
pattern_name -> transform1 -> transform2 -> transform3
```

Transforms are applied left to right. Each transform receives the output of the preceding transform.

`->` is the only pipe operator. Using `|` in an expression context produces a `DeprecatedPipeOperator` error.

**Transform pipeline ordering is enforced:** The canonical order is: **swing → expressive transforms → humanize**. If a user writes transforms in a different order, the compiler rejects it with an error. The check applies per contiguous `->` chain — pipelines separated by `>>` or parentheses are independent, so `(a -> humanize(...)) >> (b -> swing(...))` is legal. Structural transforms (reverse, transpose, rotate, …) and stream transforms (echo, vel_curve, gate_curve, scale_lock) are orthogonal to this ordering.

### 10.2 Deterministic Transforms

These transforms produce the same output regardless of seed.

#### `reverse`

Reverses the order of steps. Subdivision brackets are reversed as a unit; their internal order is also reversed. Ties are recalculated.

#### `invert`

Inverts all intervals around the first pitch. Each interval from the first step's pitch class is negated.

#### `retrograde`

Alias for `reverse -> invert`. Classical retrograde-inversion.

#### `rotate(n)`

Shifts the pattern start point by `n` steps. Steps shifted off the beginning are appended to the end. Negative `n` rotates in the opposite direction.

#### `stretch(n)`

Multiplies all step durations by `n`. Updates the pattern's effective unit. `n` must be a positive integer or simple fraction.

**Tick rounding:** The round-half-up rule applies to resulting step durations.

#### `compress(n)`

Alias for `stretch(1/n)`.

#### `transpose(n)`

Transposes all absolute pitches by `n` semitones. Degree tokens are unaffected. Chord ordinal (`%n`) tokens are also unaffected (chord tones resolve at runtime from the harmony context).

#### `shift_oct(n)`

Transposes all notes by `n` octaves at resolution time.

#### `subset(i, j, k, ...)`

Retains only steps at the specified 1-indexed positions. Unspecified steps become rests. Pattern length unchanged.

#### `interleave(pattern_b)`

Alternates steps between the calling pattern and `pattern_b`. Both must have the same step count. Resulting pattern has `steps * 2` steps.

#### `mirror`

Concatenates the pattern with its own retrograde: `pattern >> (pattern -> reverse)`.

### 10.3 Swing Transform

```
play: comp -> swing(0.62, 1/8)
```

Applies swing to a specific pattern in the pipeline. Parameters: ratio (float), unit (fraction). Same behavior as track-level swing but scoped to one pattern.

### 10.4 Arp Transform

```
play: chords -> arp(pattern=up, rate=1/8, octaves=1)
```

Explodes multi-note steps into arpeggiated sequences. Applies at emission time (after chord resolution) because chord tones must be fully resolved to MIDI pitches first.

**Parameters (all optional):**

|Parameter|Default|Description|
|---|---|---|
|`pattern`|`up`|Arp pattern: `up`, `down`, `updown`, `random`|
|`rate`|`1/8`|Onset-to-onset spacing as a musical fraction|
|`octaves`|`1`|Number of octave layers|

**Patterns:**

- `up` — ascending order by pitch
- `down` — descending order by pitch
- `updown` — ascending then descending without repeating the top note. E.g., [C,E,G] → [C,E,G,E]
- `random` — seeded random order (uses per-track seed)

**Octave layers:** `octaves=N` stacks N octave copies of the chord tones above the base pitches into one tone pool (duplicates removed) before the direction pattern is applied. `up` ascends through the full stack, `down` descends from the top of the stack, `updown` traverses the full stack up then down.

**Rate coarser than the step unit:** `rate` is onset-to-onset spacing. If the rate is coarser than (or equal to) the step's unit, the step still emits one arp slot — the first tone of the cycle, clamped at the step end — rather than swallowing the chord.

**Single-note steps are unchanged.** Arp only acts on steps with two or more simultaneous pitches (`%n` chords, `$chord`, `+` clusters).

```
// Arpeggio the current chord in eighth notes, upward
play: pads -> arp(pattern=up, rate=1/8)

// Two-octave downward arpeggio in 16ths
play: comp -> arp(pattern=down, rate=1/16, octaves=2)
```

### 10.5 Seeded Transforms

These transforms produce output that varies by seed. All are deterministic given a fixed seed.

#### `humanize(timing, intensity)`

Applies random variation to timing and velocity. Both parameters required.

- `timing` — maximum timing deviation. Accepts `%`, fraction, or `ms`.
- `intensity` — float 0.0–1.0. Scales overall strength. At `intensity=1.0` the
  maximum velocity deviation is ±32; velocity is clamped to 1–127.

Velocity variation is correlated with timing variation at coefficient 0.4 — a note pushed late tends to be slightly softer. Timing offsets never push a note before tick 0 or past its own note-off. Each note consumes two RNG draws from the per-track seeded stream; a `humanize` baked into a `@pattern` declaration re-rolls per reference in a `play:` expression.

```
play: groove -> humanize(5%, 0.5)
```

#### `vary(probability)`

At each step containing a `{...}` variant pool, with probability `p`, selects a random alternative (uniformly among all alternatives, including the first). Steps without variant pools are unaffected.

Exactly one RNG draw is consumed per variant step per pattern instance, regardless of outcome — the same determinism contract as `[prob:N]`. A pool with a single alternative has no variability and consumes no draw. Without `vary`, a bare `{a,b,c}` pool always takes its first alternative and consumes no draw. See §11.4.

```
play: comp -> vary(0.25)
```

#### `evolve(toggle)`

A 16-bit shift register generates a self-referential looping sequence. At each step the register shifts, the last bit feeds back to the front, and `toggle` probability determines if the feedback bit is flipped:

- `toggle=0.0` → sequence locks, loops identically every time
- `toggle=1.0` → fully random
- `toggle=0.05–0.20` → slowly evolving, stable for several bars then gradually mutating

```
play: base_pattern -> evolve(0.15)
```

Pitch offsets from the register are applied after resolution and snapped to the active scale context.

#### `euclid_gate(pulses, steps)`

Distributes N pulses as evenly as possible across M steps using Bjorklund's algorithm. Steps not in the Euclidean pattern are silenced.

```
play: melody -> euclid_gate(3, 8)    // Cuban tresillo pattern
play: melody -> euclid_gate(5, 8)    // Cuban cinquillo pattern
```

#### `echo(rate, repeats, decay)`

Each note generates N copies at a defined interval, with velocity decaying per copy. MIDI echo — not audio.

```
play: melody -> echo(1/8, 3, 0.6)
```

At `decay=0.6` with original velocity 100: echoes at 60, 36, 22.

Echo copies are clamped at the pattern boundary — no bleed into the next pattern instance.

#### `vel_curve(wave=<shape>, min=<int>, max=<int>[, repeat=<int>])`

Applies a waveform shape to velocity across all steps. Bakes the shape permanently into the pattern (unlike `swell()` which is an expressive transform).

```
play: melody -> vel_curve(wave=sine, min=40, max=110, repeat=1)
play: melody -> vel_curve(wave=ramp, min=20, max=100)     // fade in
```

**Supported waves:** `sine`, `tri`, `ramp`, `square`, `random` (seeded).

`repeat` defaults to 1. Values > 1 repeat the wave cycle that many times across the pattern.

#### `gate_curve(wave=<shape>, min=<float>, max=<float>[, repeat=<int>])`

Same as `vel_curve` but applies to gate values (NoteOff timing).

```
play: melody -> gate_curve(wave=tri, min=0.3, max=0.95)
```

#### `scale_lock([scale=<mode>,] [root=<note>,] mode=<down|up|filter>)`

Forces all notes to the nearest pitch within a specified scale.

```
play: recorded  -> scale_lock(scale=dorian, root=C, mode=down)
play: generated -> scale_lock(scale=major, root=Eb, mode=up)
play: chromatic -> scale_lock(mode=filter)    // uses @scale context
```

**`mode` options:**

- `down` — snap out-of-scale note to nearest lower scale note
- `up` — snap to nearest higher scale note
- `filter` — remove out-of-scale notes entirely

If `scale=` and `root=` are absent, inherits from `@scale`.

### 10.6 Expressive Performance Transforms

These transforms simulate the breathing quality of human and classical performance. All operate on relative time within the pattern.

#### `rubato(depth, curve)`

Time envelope over the whole pattern. Redistributes time internally without changing total duration.

- `depth` — float 0.0–1.0, maximum deviation from strict time
- `curve` — `ease_in`, `ease_out`, `ease_in_out`, `arch`

```
play: melody -> rubato(0.3, arch)
```

#### `ritardando(depth)`

Gradual tempo decrease toward the end. First step at full speed, last step at `(1 - depth)` speed. Total duration increases.

```
play: cadence -> ritardando(0.4)
```

#### `accelerando(depth)`

Inverse of ritardando. Speeds up toward the end. Total duration decreases.

```
play: buildup -> accelerando(0.3)
```

#### `agogic(step, step, ...)`

Emphasis through duration rather than velocity. Specified steps (1-indexed) are lengthened by 15% of their step duration; the immediately following step is shortened by the same amount. Total duration unchanged.

```
play: melody -> agogic(1, 3, 5)
```

#### `breathe(position, duration)`

Micro-pause at a specified step position. Simulates a breath mark or bow change.

```
play: phrase -> breathe(4, 2%)
```

#### `swell(peak, curve)`

Velocity envelope combined with subtle timing expansion at the dynamic peak.

- `peak` — normalized position 0.0–1.0 of velocity peak
- `curve` — `arch`, `ease_in`, `ease_out`, `ease_in_out`

```
play: strings -> swell(0.6, arch)
```

#### `phrase(tension, release)`

High-level composite transform combining rubato, agogic accent, and swell. The pattern pushes forward through a tension point and relaxes at release.

```
play: melody -> phrase(0.7, 0.9)
```

### 10.7 Curve Functions

All curve functions map normalized time `t ∈ [0, 1]` to a deformation factor:

|Curve|Formula|Character|
|---|---|---|
|`ease_in`|`t²`|Slow start, fast end|
|`ease_out`|`1 - (1-t)²`|Fast start, slow end|
|`ease_in_out`|`3t² - 2t³` (smoothstep)|Slow start and end|
|`arch`|`sin(π·t)`|Peak at midpoint, zero at ends|

---

[← Back to Table of Contents](00_toc.md)
