# 7. Patterns

Patterns are the primary composition unit. They are reusable, named, and transformable. They contain step sequences and may reference other patterns.

### 7.1 Declaration

```
@pattern <name> [steps=<int>] unit=<fraction> [vel=<int>] [gate=<float>] [oct=<int>] [-> transform ...]
```

**Parameters:**

|Parameter|Default|Description|
|---|---|---|
|`steps`|Optional (inferred from body)|Number of steps in the pattern|
|`unit`|Required|Duration of each step as a fraction of a whole note|
|`vel`|84|Default note velocity (1–127)|
|`gate`|0.9|Default gate ratio (0.0–1.0). Note duration = step_duration * gate|
|`oct`|4|Default octave for degree resolution|

`steps=` is optional for all pattern forms — both multi-line and inline. When omitted, the step count is inferred from the body (line count for multi-line, token count for inline). When provided, the declared count is validated against the actual body count; a mismatch is a `StepCountMismatch` error. `unit=` is always required.

**Baked-in transforms** may be appended after the parameters on the declaration line using `-> transform`. See §7.14 for details.

**Common unit values:**

|Fraction|Name|
|---|---|
|`1/1`|Whole note|
|`1/2`|Half note|
|`1/4`|Quarter note|
|`1/8`|Eighth note|
|`1/16`|Sixteenth note|
|`1/32`|Thirty-second note|
|`1/12`|Eighth note triplet|
|`1/24`|Sixteenth note triplet|

### 7.2 Pattern Body

The pattern body follows the declaration line. Each non-empty line after the declaration and before the next `@` block is a step. Steps are numbered from 1. When `steps=` is declared, the total number of step lines must equal the declared value; a mismatch is a compiler error.

```
@pattern walking_bass unit=1/4
^1
^3
^5
^b7
```

The above is equivalent to `@pattern walking_bass steps=4 unit=1/4` — the step count is inferred as 4 from the body line count.

#### 7.2.1 Inline Pattern Body

A colon after the parameter list (and optional baked-in transforms) introduces an inline body on the same line. Each whitespace-separated token (or `+`-connected cluster) is one step.

```
@pattern arp unit=1/4: ^1 ^3 ^5 ^7
```

This is equivalent to the multi-line form above. Chord steps use `+`:

```
@pattern comp unit=1/4: ^1+^3+^5 .
```

**`steps=` is optional in both forms.** When omitted, the step count is inferred from the number of tokens on the line (inline) or the number of body lines (multi-line):

```
@pattern arp unit=1/4: ^1 ^3 ^5       // steps inferred as 3
```

When `steps=` is provided, the declared count is validated against the actual token count. A mismatch is a `StepCountMismatch` error.

**`unit=` is always required** — it cannot be inferred.

**Baked-in transforms** appear before the colon:

```
@pattern desc unit=1/4 -> reverse: ^1 ^3 ^5 ^7
```

All step token types are valid in inline form: degree tokens, chord ordinals, absolute pitches, rests (`.`), ties (`~`), chord symbols (`$chord`), and annotations (`[vel:80]`). Subdivision brackets and variant pools are also supported but the multi-line form is typically clearer for complex patterns.

Multiple tokens on a single line produce simultaneous events (chord step):

```
@pattern shell steps=4 unit=1/4
^1+^7
~
^3+^5
~
```

### 7.3 Step Tokens

#### 7.3.1 Degree Tokens

`^<degree>` where degree is a scale degree with optional accidental.

`^n` ALWAYS resolves against `@scale` mode intervals — it is scale-absolute, never chord-relative, regardless of `follow=`. Use `%n` (§7.3.2) for chord-tone selection.

|Token|Meaning|
|---|---|
|`^1`|Root of the active scale|
|`^2`|2nd scale degree|
|`^3`|3rd scale degree|
|`^4`|4th scale degree|
|`^5`|5th scale degree|
|`^6`|6th scale degree|
|`^7`|7th scale degree|
|`^8`|Octave above root|
|`^9`|9th (= ^2 + octave)|
|`^11`|11th (= ^4 + octave)|
|`^13`|13th (= ^6 + octave)|
|`^b<n>`|Degree n, flatted|
|`^#<n>`|Degree n, sharped|

`^n` does not require `follow=` or a harmony context — it resolves against `@scale` (or C major fallback) at the step's tick position.

**Octave displacement:** Append `/<int>` to force a specific octave:

```
^1/2   // root in octave 2
^5/5   // fifth in octave 5
```

Without displacement, the compiler places the note at the track's `oct` value.

#### 7.3.2 Chord Ordinal Tokens

`%n` selects the nth chord tone (1-indexed) from the currently active harmony chord.

|Token|Meaning|
|---|---|
|`%1`|Root of the active chord|
|`%2`|3rd of the active chord|
|`%3`|5th of the active chord|
|`%4`|7th of the active chord (for seventh chords)|
|`%5`|Wraps up an octave: root + 1 octave|
|`%6`|3rd + 1 octave|
|`%7` and beyond|Continue wrapping upward|

Wrapping is cyclic with octave increments: `%5` = `%1` up an octave, `%6` = `%2` up an octave, and so on.

**Harmony context required:** `%n` requires a harmony context — either explicit `follow=` on the track or auto-inferred when exactly one harmony block exists. Without a harmony context, `%n` is a compile error (`ChordOrdinalWithoutHarmony`).

**Optional forced octave:** `%1/4` forces octave 4 for this chord tone (same syntax as degree tokens).

**No accidentals:** Accidentals (`%b1`, `%#3`) are not valid on chord ordinal tokens.

```
@pattern arp_chord steps=4 unit=1/4
%1
%2
%3
%4

@pattern comp steps=4 unit=1/4
%1+%2+%3[vel:80]
.
%2+%3+%4[vel:60 shift:+2%]
.
```

#### 7.3.3 Absolute Pitch Tokens

`<letter>[#|b]<octave>`

|Example|MIDI note|
|---|---|
|`C4`|60|
|`D#4`|63|
|`Bb3`|58|
|`G5`|79|

Absolute pitches do not resolve against harmony. They are passed through unchanged regardless of `follow=` setting.

#### 7.3.4 MIDI Note Numbers

`n<int>` — direct MIDI note number. `n60` = C4. Range: n0–n127.

#### 7.3.5 Chord Symbols in Patterns

`$<chord>` — literal chord symbol. Voiced according to the track's `voice=` setting. Resolves to absolute pitches at compile time.

```
$Cmaj7   // voiced C major seventh chord
$G7b9    // voiced G dominant seventh flat nine
```

#### 7.3.6 Current Chord Token

`$chord` — emits the currently active harmony chord as a voiced multi-note event. Resolves at compile time via the harmony index at the step's tick position. Uses the track's `voice=`, `inv=`, `oct=` settings for voicing.

`$chord` is the only spelling of this token. Using `$_` produces a `DeprecatedCurrentChord` error.

```
@pattern comp steps=4 unit=1/4
$chord[vel:80]
.
$chord[vel:60]
.
```

**Rules:**

- Supports all step annotations: `$chord[vel:100]`, `$chord[gate:0.5]`, etc.
- Requires harmony context (explicit `follow=` or auto-inferred single harmony block). Without one, produces a `CurrentChordWithoutHarmony` error.
- Voice-leading state (`inv=auto`) flows through `$chord` steps like any other multi-note step.

#### 7.3.7 Rest

`.` — silence for the duration of one step. No note-on or note-off is emitted.

#### 7.3.8 Tie / Hold

`~` — extends the duration of the previous active notes by one step. A tie following a rest is a compiler error. A tie at the start of a pattern (with no prior notes in context) is a compiler error unless the preceding pattern in a `*~` or `~>>` sequence has active notes.

Tie semantics (normative):

- **Gate applies to the full extended duration** of a tie chain: `^1 ~ ~ ~` at `unit=1/4`, `gate=1.0` is exactly one whole note; at `gate=0.9` the note-off lands at 90% of the four-step total. An explicit `[dur:]` overrides gate, and tie extensions on a `[dur:]` note add raw step durations.
- A tie extends **all** pitches of a chord step (`^1+^3+^5 ~`, `$chord ~`), and works inside subdivision brackets (extending by the bracket slot's duration).
- **Soft boundaries** (`*~`, `~>>`): notes sounding at the boundary carry into the next instance and sustain legato until that instance's first note onset (overriding gate); if the next instance begins with `~`, they extend through that step instead. **Hard boundaries** (`*`, `>>`) settle all notes at their scheduled ends; a leading `~` after a hard boundary is a compile error.
- Tie legality is purely structural: a tie after a `[prob:]`-suppressed note is not an error and does not resurrect the suppressed note — compile errors never depend on the seed.
- A new note on a pitch that is still sounding (a restrike) ends the old note at the new note's onset.

### 7.4 Simultaneous Notes

`+` separates simultaneous tokens within a step. All notes on either side of `+` have the same start tick.

```
^1+^3+^5        // scale-degree chord, three simultaneous notes
%1+%3           // chord tones 1 and 3
^1+C4           // degree token and absolute pitch simultaneously
```

### 7.5 Step Annotations

`[key:value key:value ...]` appended to a token overrides track defaults for that step.

**Annotation keys:**

|Key|Type|Description|
|---|---|---|
|`vel`|int 1–127|Note velocity|
|`gate`|float 0–1|Gate ratio for this step|
|`dur`|fraction|Explicit duration, overrides gate calculation|
|`shift`|timing value|Microtiming shift. Positive = late, negative = early|
|`lshift`|timing value|Timing shift for CC/controller events only. Same format as `shift`|
|`oct`|int|Octave override for degree resolution on this step|
|`expr`|int or ramp|CC11 expression|
|`dyn`|int or ramp|CC1 modulation|
|`sus`|int 0/127|CC64 sustain pedal|
|`pan`|int or ramp|CC10 pan|
|`vol`|int or ramp|CC7 volume|
|`pb`|int|Pitch bend (-8192 to 8191)|
|`at`|int|Channel aftertouch|
|`cc<n>`|int or ramp|Arbitrary CC number|
|`ratch`|int|Ratchet: repeat note N times within step duration|
|`ratch_decay`|float|Ratchet velocity decay factor (default 1.0)|
|`every`|int N|Conditional: play on every Nth loop|
|`cond`|X:Y|Conditional: play on Xth iteration of every Y loops|
|`once`|—|Conditional: play on first loop only|
|`pre`|—|Conditional: play only if previous conditional step played|
|`prob`|float 0.0–1.0 or percent|Probability of the step playing|
|`glide`|optional float|Portamento to this note|

A step-level `[inv:N]` inversion override is reserved for a future version; it is not currently accepted. Inversion is controlled at the harmony-block and track level (§8.6).

**Ramp syntax:** `<start>-><end>` linearly interpolates from start to end over the step duration. Sampled at 4 points per step.

```
^1[vel:110 shift:-3%]
^3[expr:40->88]
^5[gate:0.3]
%b7[vel:72 cc74:30->80]
```

Annotations apply to all simultaneous notes in a `+` cluster unless attached to a specific token:

```
^1[vel:110]+^3+^5    // only ^1 gets vel:110; ^3 and ^5 use track default
^1+^3+^5[vel:110]    // only ^5 gets vel:110
```

### 7.6 Probability Annotation

`[prob:N]` sets the probability of a step playing, where N is 0.0–1.0 or a percent value.

```
^1[prob:0.75]          // 75% chance of playing
^5[prob:50%]           // 50% chance of playing
kick[prob:0.3]         // 30% chance
```

- `[prob:0.0]` — step never plays (rest). Produces a compiler warning: `"probability annotation of 0.0 — step can never play"`.
- `[prob:1.0]` — always plays (no-op, accepted without warning).
- Uses the per-track seeded xorshift64 RNG (same PRNG as `humanize`/`vary`) for determinism. See §11 for the full determinism contract.
- Stacks with all other annotations: `^5[prob:0.5 vel:110 every:2]`
- When a step is suppressed by probability, it produces a rest for that step's duration (like `.`).

### 7.7 Glide Annotation

`[glide]` or `[glide:N]` applies portamento to a note.

```
^3[glide]        // full portamento (CC5=64 default)
^5[glide:0.5]    // half-strength portamento (CC5=64)
^7[glide:1.0]    // maximum portamento time (CC5=127)
```

- `[glide]` — equivalent to `[glide:0.5]`. Emits CC65=127 (Portamento On) + CC5=64 before NoteOn, then CC65=0 after NoteOff.
- `[glide:N]` where N is 0.0–1.0 — CC5 value = `round(N × 127)`. CC65 toggling is the same.
- Glide is ignored on drum tracks with a warning: `"[glide] annotation ignored on drum track"`.
- Glide annotations have no effect if `follow=` is absent and the note is absolute pitch (portamento is still emitted — the annotation is syntactically valid for absolute pitches too).

### 7.8 Ratcheting

A step triggers its note multiple times within its own duration. Different from subdivision brackets `(...)` which are compositional — ratchet is an expressive annotation on an existing step.

```
^1[ratch:2]                    // note triggers twice within the step
^5[ratch:3]                    // triplet ratchet feel
kick[ratch:4]                  // four rapid hits
^1[ratch:3 ratch_decay:0.7]    // each hit is 70% velocity of the previous
```

Each ratchet hit uses `step_duration / ratch_count` as its individual duration. Gate is applied per hit. Total duration fills the step exactly.

`ratch_decay` is optional (default `1.0` = no decay). At `0.7` with velocity 100: hits play at 100, 70, 49. Clamped to 1–127.

### 7.9 Conditional Steps

Individual steps carry a condition annotation that determines whether they play, evaluated per loop iteration.

|Annotation|Meaning|
|---|---|
|`[every:N]`|Shorthand for `[cond:1:N]` — plays on 1st of every N loops|
|`[cond:X:Y]`|Plays on the Xth iteration of every Y loops|
|`[once]`|Plays on first loop only, never again|
|`[pre]`|Plays only if the previous conditional step on this track played|

```
^5[every:2]       // plays on loops 1, 3, 5... (every other loop)
^5[cond:2:4]      // plays on the 2nd of every 4 loops only
^9[once]          // plays on first loop only, silent forever after
^3[pre]           // plays only if the previous conditional step played
```

**RT scheduler behavior:** At each step's emit time, the scheduler queries `TrackState.pattern_loop_count`:

- `every:N` → plays when `loop_count % N == 0`
- `cond:X:Y` → plays when `loop_count % Y == (X - 1)`
- `once` → plays when `loop_count == 0`
- `pre` → plays when `last_conditional_played == true`

**SMF renderer behavior:** Evaluates all conditional steps as iteration 1 (first playthrough):

- `once` → always plays
- `every:N` and `cond:1:Y` → play (iteration 1 qualifies)
- `cond:X:Y` where X > 1 → does not play

### 7.10 Subdivision Brackets

`(token token ...)` divides the parent step duration equally among the tokens inside.

```
^1  (^2 ^3 ^2)  ^5  (^b7 ^1)
//  ^^^^^^^^^^^       ^^^^^^^^^
//  3-way div (triplet)  2-way div (duplet)
```

The compiler calculates: `subdivision_duration = parent_step_duration / token_count`.

**Nesting:** Subdivision brackets may nest. Inner brackets further subdivide the slot allocated to their position.

```
^1  ((^2 ^3) ^5 ^3)  ^5  ~
//   ^^^^^^^ gets half of the first third of the step
```

**Rests inside subdivisions:** `.` is valid inside `(...)`.

**Annotations inside subdivisions:** Step annotations are valid on individual tokens inside `(...)`.

**Ties inside subdivisions:** `~` inside `(...)` ties the previous note within the subdivision.

### 7.11 Variant Pools

`{token,token,token}` declares a variant pool for a step. Commas separate alternatives. Variant pools are metadata consumed by `vary()` (see §10.5 and §11.4) — without it, the first alternative is always used.

The separator inside `{}` is `,` (comma). Using `|` inside `{}` produces a `DeprecatedVariantPipe` error.

```
^1  {^3,^b3,^#5}  ^5  ~
```

### 7.12 Pattern Composition

#### 7.12.1 Operator Precedence

Pattern expressions follow this precedence (tightest to loosest):

| Precedence | Operator | Associativity | Description |
|------------|----------|---------------|-------------|
| 1 (tight) | `()` | — | Grouping |
| 2 | `*` / `*~` | Left | Repetition |
| 3 | `>>` / `~>>` | Right | Concatenation |
| 4 (loose) | `->` | Left | Transform pipe |

This means `a >> b -> reverse` is parsed as `(a >> b) -> reverse` — the transform applies to the entire concatenated sequence. Use parentheses for explicit grouping: `a >> (b -> reverse) >> c`.

#### 7.12.2 Repetition

```
pattern_name * N      // hard boundary (ties cut, no voice leading continuity)
pattern_name *~ N     // soft boundary (ties carry, voice leading continues)
```

N must be a positive integer.

#### 7.12.3 Concatenation

```
pattern_a >> pattern_b     // hard boundary
pattern_a ~>> pattern_b    // soft tie boundary
```

Concatenation chains: `verse >> chorus >> verse >> outro`

Mixed boundaries are valid: `intro ~>> verse >> chorus >> verse >> outro`

#### 7.12.4 Pattern Assignment

A new named pattern may be defined as a composition expression:

```
@pattern full_song = intro >> verse * 4 >> chorus * 2 >> verse * 2 >> outro
```

This pattern has no body of its own; it is entirely defined by the expression. Constituent patterns must share the same effective `unit` (after any transforms) or the assignment is a compiler error.

### 7.13 Per-Reference Rate

The `@rate` suffix overrides playback speed for a single pattern reference:

```
play: theme@2.0              // double speed
play: a >> b@0.5 >> c@2.0    // mixed rates per segment
```

**Rules:**

- `@rate` is appended directly to the pattern name with no spaces: `name@value`
- Rate must be a positive number
- Rate multiplies the effective step duration: `effective_unit = base_unit / rate`
- At rate 2.0, steps are half as long (double speed)
- At rate 0.5, steps are twice as long (half speed)
- Rates are per-segment in concatenated expressions
- Track-level `rate=` and per-reference `@rate` multiply: `final_rate = track_rate * segment_rate`

### 7.14 Baked-In Transforms

Transforms can be applied directly to `@pattern` definitions on the declaration line. These are resolved when the pattern is referenced, before any call-site transforms.

```
@pattern ascending steps=4 unit=1/4
C4
E4
G4
C5

@pattern descending = ascending -> reverse

@pattern transposed steps=3 unit=1/4 -> transpose(2)
C4
E4
G4
```

**Rules:**

- Baked-in transforms appear after the parameters using `-> transform`
- Multiple transforms can be chained: `-> transpose(2) -> reverse`
- Transform application order:
  1. Step body is parsed
  2. Baked-in transforms are applied (in order)
  3. At the call site, any additional call-site transforms are applied on top
- Deterministic transforms (reverse, transpose, etc.) produce stable results
- Stochastic transforms (humanize, vary) are re-evaluated per reference

---

[← Back to Table of Contents](00_toc.md)
