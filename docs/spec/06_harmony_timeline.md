# 6. Harmony Timeline

The harmony timeline is the root context for all degree-relative notation. One or more harmony blocks may exist in a file.

### 6.1 Declaration

```
@harmony [name] [play=<bool>] [ch=<int>] [prog=<int>] [voice=<voicing>] [oct=<int>] [vel=<int>] [inv=<int|auto>]
```

**`[name]` is optional.** When there is exactly one harmony block in the file, the name may be omitted. If multiple harmony blocks exist, all of them must have names or the compiler produces a `MultipleHarmonyBlocksRequireNames` error.

**Auto-inference:** When exactly one harmony block exists (named or unnamed), all tracks without an explicit `follow=` automatically follow it. `$chord` and `%n` tokens also resolve against it without any configuration.

**Parameters:**

|Parameter|Default|Description|
|---|---|---|
|`play`|`false`|If `true`, the harmony block also emits voiced MIDI chords|
|`ch`|—|Required if `play=true`. MIDI channel 1–16|
|`prog`|—|GM program number if `play=true`|
|`voice`|`close`|Voicing strategy if `play=true`|
|`oct`|`4`|Base octave for chord voicing if `play=true`|
|`vel`|`72`|Velocity for chord notes if `play=true`|
|`inv`|`0`|Block-level default inversion. Overridden by track-level `inv=` when the track sets a non-default value (§8.6)|

**Note:** `mode=` is **not** valid on `@harmony`. Use `@scale root=<r> mode=<m>` instead (see §5). `mode=` on a harmony declaration is a parse error.

### 6.2 Bar Grid Syntax

Chord changes are declared using bar separators (`|`). Leading and trailing `|` are not used; only inner separators between bars are written.

```
@harmony main
Cm7 | Fm7 | Bb7 | Ebmaj7
Cm7 | Fm7 | Bb13 | Ebmaj7
```

Each segment between `|` separators is one bar. A line with no `|` is a single bar. Chords inside a bar are distributed **evenly across beats** by default. Beat count is determined by `@ts` numerator (default 4).

**Multiple chords per bar — even distribution:**

```
Dm7 G7 | Cmaj7    // bar 1: Dm7 for 2 beats, G7 for 2 beats (in 4/4); bar 2: Cmaj7
Dm7 Em7 Fmaj7 G7  // 1 beat each, single bar
```

**Multiple chords per bar — explicit beat assignment:**

Use `:<beats>` suffix to assign specific beat durations. All values in a bar must sum to the time signature numerator.

```
Dm7:3 G7:1 | Cmaj7   // bar 1: Dm7 for 3 beats, G7 for 1 beat; bar 2: Cmaj7
```

If beat assignments are present for any chord in a bar, all chords in that bar must have explicit assignments.

### 6.3 Intra-Bar Step Grid

For chromatic or complex motion within a bar, a `steps:` block overrides even beat distribution for that bar:

```
@harmony main
Cmaj7
  steps: Bbmaj7 Bbmaj7 Amaj7 Amaj7 Abmaj7 Abmaj7 Gmaj7 Gmaj7
Fmaj7
```

The `steps:` block follows the bar line and applies to the preceding bar. The number of tokens in `steps:` defines the subdivision of that bar. Each token is a chord symbol occupying one step. The step duration within the bar is `bar_duration / step_count`.

### 6.4 Roman Numeral Chord Symbols

Roman numerals provide a fully relative chord notation system. With `@scale root=C`, changing `root=C` to `root=Eb` transposes the entire composition without touching anything else.

**Convention (standard academic and jazz theory):**

- **Uppercase** — major root context: `I`, `II`, `IV`, `V`
- **Lowercase** — minor root context: `i`, `ii`, `iv`, `v`

Quality suffixes follow the same grammar as letter-based chord symbols. No space between Roman numeral root and quality suffix.

### 6.5 Diatonic Quality Inference

Bare Roman numerals in heptatonic modes automatically infer chord quality from the scale degree. No quality suffix is required for diatonic triads and seventh chords.

**Heptatonic modes that support inference:** `major`/`ionian`, `dorian`, `phrygian`, `lydian`, `mixolydian`, `aeolian`/`minor`, `locrian`, `melodic_minor`, `harmonic_minor`.

**Diatonic triads in C major:**

|Symbol|Inferred chord|
|---|---|
|`I`|C major|
|`ii`|D minor|
|`iii`|E minor|
|`IV`|F major|
|`V`|G major|
|`vi`|A minor|
|`vii`|B diminished|

**Diatonic 7th chords:**

|Symbol|Inferred chord (C major)|
|---|---|
|`Imaj7`|Cmaj7|
|`ii7`|Dm7|
|`iii7`|Em7|
|`IVmaj7`|Fmaj7|
|`V7`|G7|
|`vi7`|Am7|
|`vii7`|Bø7 (half-diminished)|

The inference applies by looking up the scale degree's quality in the diatonic function table for the active mode. For example, `vii7` in C major infers half-diminished (ø7) because the 7th degree of major is a diminished triad.

**Explicit quality suffixes always override inference.** Writing `VIIM7` (capital quality suffix) is valid and overrides the default minor seventh that would be inferred for a VII degree in a minor key.

**Non-heptatonic modes excluded from inference:** `pentatonic_major`, `pentatonic_minor`, `blues`, `whole_tone`, `diminished`, `chromatic`. Bare Roman numerals in these modes produce a compile error (`BareRomanNumeralInNonHeptatonicMode`).

**Mixed notation is valid:** Letter-based and Roman numeral chord symbols may be mixed freely in the same harmony block. Letter-based symbols are always absolute; Roman numerals resolve via `@scale root=`.

### 6.6 Chord Symbol Grammar

#### 6.6.1 Roots

`A` `B` `C` `D` `E` `F` `G` with optional `#` (sharp) or `b` (flat). Both sharp and flat roots are valid in harmony blocks: `F#7`, `C#m7`, `Gb7`, `Bbmaj7`.

**Sharp-root vs sharp-alteration disambiguation:** a single note letter followed by `#` is always a sharp *root* — `F#9` is an F-sharp dominant ninth. A `#` after a longer symbol is an *alteration* — `F7#9` is an F dominant seventh with a sharpened ninth, `G7#5#9` and `C7b9#11` stack alterations. To write a sharp-nine chord on an altered-root, spell the quality first: `F#7#9`.

Sharp roots (`F#`, `C#`, `D#`, etc.) are accepted at all `root=` parameter sites (`@scale`, `@scale` timeline entries, `section:`, `scale_lock`). The parser uses a shared `parse_note_root()` helper that reconstructs sharp roots from the `Ident + Sharp` token sequence produced by the lexer; flat roots (`Bb`, `Gb`) arrive as single `Ident` tokens.

#### 6.6.2 Qualities

|Symbol|Quality|Intervals|
|---|---|---|
|_(none)_|Major triad|0 4 7|
|`m` / `min` / `-`|Minor triad|0 3 7|
|`maj7` / `M7` / `Δ7`|Major seventh|0 4 7 11|
|`7`|Dominant seventh|0 4 7 10|
|`m7` / `min7` / `-7`|Minor seventh|0 3 7 10|
|`mMaj7` / `mM7`|Minor-major seventh|0 3 7 11|
|`maj9` / `M9`|Major ninth|0 4 7 11 14|
|`9`|Dominant ninth|0 4 7 10 14|
|`m9`|Minor ninth|0 3 7 10 14|
|`maj11`|Major eleventh|0 4 7 11 14 17|
|`11`|Dominant eleventh|0 4 7 10 14 17|
|`m11`|Minor eleventh|0 3 7 10 14 17|
|`13`|Dominant thirteenth|0 4 7 10 14 17 21|
|`dim` / `°`|Diminished triad|0 3 6|
|`dim7` / `°7`|Diminished seventh|0 3 6 9|
|`m7b5` / `ø` / `ø7`|Half-diminished|0 3 6 10|
|`aug` / `+`|Augmented triad|0 4 8|
|`sus2`|Suspended 2nd|0 2 7|
|`sus4`|Suspended 4th|0 5 7|
|`7sus4` / `7sus`|Dominant 7th suspended 4th|0 5 7 10|
|`9sus4`|Dominant 9th suspended 4th|0 5 7 10 14|
|`add9`|Add ninth|0 4 7 14|
|`m(add9)` / `madd9`|Minor add ninth|0 3 7 14|
|`6`|Major sixth|0 4 7 9|
|`m6`|Minor sixth|0 3 7 9|
|`6/9`|Major sixth ninth|0 4 7 9 14|
|`add11`|Add eleventh|0 4 7 17|
|`maj13`|Major thirteenth|0 4 7 11 14 17 21|
|`m13`|Minor thirteenth|0 3 7 10 14 17 21|
|`mMaj9` / `mM9`|Minor-major ninth|0 3 7 11 14|
|`aug7` / `+7`|Augmented seventh|0 4 8 10|
|`augmaj7` / `augM7` / `+M7`|Augmented major seventh|0 4 8 11|
|`7sus2`|Dominant 7th suspended 2nd|0 2 7 10|
|`m6/9`|Minor sixth ninth|0 3 7 9 14|
|`add2`|Add second|0 2 4 7|
|`5`|Power chord|0 7|

#### 6.6.3 Alterations

Alterations are appended to the quality:

|Symbol|Meaning|
|---|---|
|`b5`|Flat fifth|
|`#5`|Sharp fifth|
|`b9`|Flat ninth|
|`#9`|Sharp ninth|
|`#11`|Sharp eleventh (Lydian dominant)|
|`b13`|Flat thirteenth|

Multiple alterations are concatenated: `G7b9#11`

#### 6.6.4 Slash Chords

`C/E` — C major chord with E in the bass. In pattern step lines, the `$` prefix applies as usual: `$C/E`. The bass note is always the lowest sounding note, placed below the chord voicing regardless of octave. The bass note is specified as a pitch class only (no octave).

#### 6.6.5 Chord Prefix

All chord symbols in the harmony timeline are written **without** a prefix. In step lines (pattern bodies), chord symbols use the `$` prefix to distinguish them from identifiers:

```
// In @harmony block — no prefix
Cmaj7 | Am7

// In @pattern step line — $ prefix
$Cmaj7  ~  ~  ~
```

### 6.7 Mid-Piece Modulation

The preferred approach for mid-piece modulation is the `@scale` timeline form (§5.2). The `section:` directive inside `@harmony` blocks functions but produces a deprecation warning.

**Deprecated `section:` syntax:**

```
@harmony main
Cmaj7 | Am7 | Dm7 G7 | Cmaj7

section: bar=5 root=D
Dm7 | Gm7 | Am7b5 D7b9 | Gm7
```

**Preferred form:**

```
@scale root=C mode=major * 4 | root=D mode=major

@harmony main
Cmaj7 | Am7 | Dm7 G7 | Cmaj7
Dm7   | Gm7 | Am7b5 D7b9 | Gm7
```

### 6.8 Multiple Harmony Timelines

Multiple named harmony blocks are permitted. Each block is independent. When multiple blocks exist, all must be named.

```
@scale root=C mode=dorian

@harmony main
Im7 | IVm7 | bVII7 | IIImaj7

@harmony ostinato
Imaj7 | Imaj7 | Imaj7 | Imaj7
```

Tracks reference harmony blocks by name via `follow=<name>`. When exactly one harmony block exists, `follow=` is auto-inferred for all tracks.

### 6.9 Cyclic Harmony Looping

The harmony context query wraps cyclically:

```
effective_tick = tick % harmony_total_ticks
```

Any tick beyond the end of the harmony timeline wraps back to the beginning. The full timeline — including all `section:` modulation blocks — is treated as one complete cycle. This makes `rate=` tracks, conditional step looping, and long patterns interact with harmony correctly without special cases.

---

[← Back to Table of Contents](00_toc.md)
