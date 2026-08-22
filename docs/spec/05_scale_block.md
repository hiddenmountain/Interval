# 5. Scale Block

The `@scale` block declares the global tonal context. All degree tokens and Roman numeral chord symbols resolve against this context unless overridden at a lower level.

### 5.1 Scalar Form

```
@scale root=<pitch> mode=<mode>
```

**Parameters:**

|Parameter|Default|Description|
|---|---|---|
|`root`|C|Root pitch class (A–G with optional # or b)|
|`mode`|major|Scale/mode for degree resolution|

Sharp roots (`root=F#`, `root=C#`, `root=D#`) and flat roots (`root=Bb`, `root=Gb`, `root=Eb`) are both fully supported. The parser reconstructs sharp roots from the two-token sequence `Ident + Sharp` at all `root=` parameter sites via the shared `parse_note_root()` helper.

If `@scale` is absent, the compiler falls back to C major with a warning: `"no @scale declared — add @scale root=C mode=major to set tonal context"`.

### 5.2 Timeline Form

The `@scale` declaration may include an inline timeline to modulate tonal context across bars. This is the preferred mechanism for mid-piece modulation; the `section:` directive inside `@harmony` blocks is deprecated (§6.7).

```
@scale root=C mode=major * 8 | root=A mode=minor * 4 | root=C mode=major
```

Each entry specifies a `root=` and/or `mode=` change, followed by `* N` for the number of bars. The final entry has no `* N` and applies for all remaining bars. Fields not specified in an entry inherit from the previous entry.

```
// 8 bars of C major, then 4 bars of A minor, then C major for the rest
@scale root=C mode=major * 8 | root=A mode=minor * 4 | root=C mode=major

// Mode change only, root stays C
@scale root=C mode=major * 4 | mode=dorian * 4 | mode=major

// Sharp root in timeline entries
@scale root=F# mode=minor * 8 | root=C# mode=major
```

**Note:** `section:` directives inside `@harmony` blocks parse and function but produce a deprecation warning. Prefer the `@scale` timeline form for tonal modulation.

### 5.3 Supported Modes

|Identifier|Intervals (semitones from root)|
|---|---|
|`major` / `ionian`|0 2 4 5 7 9 11|
|`dorian`|0 2 3 5 7 9 10|
|`phrygian`|0 1 3 5 7 8 10|
|`lydian`|0 2 4 6 7 9 11|
|`mixolydian`|0 2 4 5 7 9 10|
|`aeolian` / `minor`|0 2 3 5 7 8 10|
|`locrian`|0 1 3 5 6 8 10|
|`melodic_minor`|0 2 3 5 7 9 11|
|`harmonic_minor`|0 2 3 5 7 8 11|
|`harmonic_major`|0 2 4 5 7 8 11|
|`double_harmonic`|0 1 4 5 7 8 11|
|`phrygian_dominant`|0 1 4 5 7 8 10|
|`lydian_dominant`|0 2 4 6 7 9 10|
|`altered`|0 1 3 4 6 8 10|
|`whole_tone`|0 2 4 6 8 10|
|`diminished`|0 2 3 5 6 8 9 11|
|`pentatonic_major`|0 2 4 7 9|
|`pentatonic_minor`|0 3 5 7 10|
|`blues`|0 3 5 6 7 10|
|`chromatic`|0 1 2 3 4 5 6 7 8 9 10 11|
|`dorian_b2`|0 1 3 5 7 9 10|
|`lydian_augmented`|0 2 4 6 8 9 11|
|`mixolydian_b6`|0 2 4 5 7 8 10|
|`locrian_nat2`|0 2 3 5 6 8 10|
|`diminished_half_whole`|0 1 3 4 6 7 9 10|
|`bebop_dominant`|0 2 4 5 7 9 10 11|
|`bebop_major`|0 2 4 5 7 8 9 11|
|`bebop_dorian`|0 2 3 4 5 7 9 10|
|`hungarian_minor`|0 2 3 6 7 8 11|
|`neapolitan_major`|0 1 3 5 7 9 11|
|`neapolitan_minor`|0 1 3 5 7 8 11|
|`hirajoshi`|0 2 3 7 8|
|`in_sen`|0 1 5 7 10|
|`iwato`|0 1 5 6 10|
|`augmented_scale`|0 3 4 7 8 11|
|`tritone_scale`|0 1 4 6 7 10|
|`prometheus`|0 2 4 6 9 10|
|`super_locrian`|0 1 3 4 6 8 10|

The modes table includes all seven church modes, melodic and harmonic minor families, symmetric scales (whole tone, diminished, chromatic), pentatonic and blues scales, bebop scales, and world/exotic scales (hirajoshi, in-sen, iwato). Non-heptatonic modes (5, 6, or 8 notes) do not support diatonic quality inference for bare Roman numerals.

---

[← Back to Table of Contents](00_toc.md)
