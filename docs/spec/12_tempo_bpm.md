# 12. Tempo and BPM Timeline

`@bpm` carries all tempo timeline functionality. There is no separate `@tempo` directive — writing one produces a hard compile error (§12.7).

### 12.1 Scalar Form

```
@bpm 120
```

Single global tempo. No tempo events beyond the initial SetTempo at tick 0.

### 12.2 Inline Timeline Form

Multiple BPM entries separated by `|`, with optional `* N` bar counts and ramp curves:

```
@bpm 120 * 4 | 140 * 8 | 120->140 ramp=ease_in * 4 | 140
```

Each entry has the form: `<value>[*<bars>][ ramp=<curve>]`

Where:
- `<value>` is a constant BPM or a `start->end` ramp expression
- `* <bars>` is an optional bar count (the final entry has no bar count — applies for remaining bars)
- `ramp=<curve>` specifies the interpolation curve for ramp entries (default: `linear`)

**Ramp curves:**

|Curve|Behavior|
|---|---|
|`linear`|Uniform BPM increase/decrease (default)|
|`ease_in`|Slow start, accelerates toward end|
|`ease_out`|Fast start, decelerates toward end|
|`ease_in_out`|Slow start and end, fastest at midpoint|
|`arch`|Peaks at midpoint — speed up then slow down|

### 12.3 Block Form

`@bpm` followed by indented entries, one per line:

```
@bpm
  120 * 4
  120->160 ramp=ease_in * 4
  160 * 8
  160->120 ramp=ease_out * 4
  120
```

Block form and inline form are functionally identical. Block form is preferred for readability when many entries exist.

### 12.4 Time Signature Timeline

`@ts` also supports an inline timeline form for meter changes:

```
@ts 4/4 * 8 | 3/4 * 4 | 4/4
```

Each entry specifies a time signature followed by an optional `* N` bar count. Entries are separated by `|`. The final entry applies for all remaining bars. The scalar form `@ts 4/4` sets a single time signature for the whole piece.

### 12.5 ms-Format Timing Uses Effective BPM at Tick

When converting `ms`-format shift values (e.g. `shift=50ms`) to ticks, the compiler uses the **effective BPM at the event's tick position**, not a fixed scalar reference. This means a `shift=500ms` at bar 9 — where the BPM timeline has ramped to 140 — resolves to 500 real milliseconds at 140 BPM, not at the original scalar value.

The same applies to `humanize` timing offsets: randomized offsets are scaled by the effective BPM at each step's tick, so humanization amounts remain perceptually consistent regardless of tempo changes.

With a scalar `@bpm 120` (no timeline), there is only one BPM value and the distinction is moot.

### 12.6 MIDI Behavior

- Tempo changes emit MIDI SetTempo meta events at the appropriate tick positions.
- Ticks are constant — only the real-time duration of each tick changes.
- For constant entries, a Tempo event is only emitted when the BPM changes from the previous bar.
- For ramp entries, the start BPM is emitted at the bar boundary (if different from previous), then 8 interpolated values are emitted at evenly spaced tick positions within the bar.

### 12.7 No @tempo Directive

`@tempo` is not part of the language. Writing it produces a hard compile error (the message quotes the removal release):

> `@tempo was removed in v0.5 — use @bpm block or inline form instead`

Everything a tempo block could express is written with `@bpm`:

```
// invalid
@bpm 120
@tempo
120 | 120 | 120->160 | 160

// equivalent @bpm timeline
@bpm 120 * 2 | 120->160 | 160
```

### 12.8 Example

```
@ppq 480
@bpm
  120 * 2
  120->160 ramp=ease_in * 2
  160
@ts 4/4

@scale root=C mode=major

@harmony
Imaj7 | VIm7 | IIm7 | V7

@pattern theme steps=4 unit=1/4
%1
%2
%3
%4

@track melody ch=1
play: theme * 4
// Bars 1-2: 120 BPM
// Bars 3-4: ramp from 120 to 160 BPM (8 intermediate tempo events per bar)
// Bar 5+: 160 BPM
```

---

[← Back to Table of Contents](00_toc.md)
