# Interval — Design Rationale

This document collects the reasoning behind Interval's design, organized by topic.
It explains *why* the language and implementation are the way they are today; the
normative *what* lives in the specification (`docs/spec/`), and the release-by-release
history of changes lives in `CHANGELOG.md`.

---

## 1. Language Model and Layering

Interval is built on three strictly ordered layers:

```
Harmony  →  Patterns  →  Tracks
```

Harmony declares a chord timeline, patterns declare reusable step sequences in
degree-relative notation, and tracks bind patterns to MIDI channels and to a harmony
timeline. Everything downstream follows from one principle:

> **Harmony follows time, not patterns.** Harmony resolution always happens at the
> real tick position of each event, regardless of how the pattern was stretched,
> rate-modified, transformed, or looped. Patterns do not own their harmonic
> context — time does.

This is why a `rate=2.0` track, a reversed pattern, and a pattern that has looped
past the end of the progression all stay harmonically locked without special cases.

### Cyclic harmony

The harmony context query wraps cyclically: `effective_tick = tick % harmony_total_ticks`
(guarded against a zero-length timeline). Any tick beyond the timeline's end wraps to
its beginning; the full timeline — including modulations — is one cycle. A cyclically
repeating progression is exactly how jazz and pop work, so this is the correct
behavior rather than a workaround, and it makes `rate=` tracks, conditional-step
looping, and continuous play-mode looping interact with harmony correctly for free.

### Step-first patterns

Patterns are step grids (one step per line, or inline), not free-form note lists.
The step is the unit that annotations, conditions, probability, subdivision, and
swing all attach to. Ratcheting exists as an *annotation* precisely because
subdivision brackets are compositional structure while a ratchet is an expressive
property of one existing step.

---

## 2. The Tonal System

### Why `@scale` is separate from `@harmony`

Mode is a property of the tonal context, not of the chord timeline: a harmony block
can contain borrowed chords and chromatic alterations without the scale itself
changing. Early versions put `mode=` on `@harmony`; separating it into `@scale`
made the distinction explicit and enabled the inheritance hierarchy:

```
@scale root=C mode=major      // global tonal context
  ↓
@harmony                      // inherits root+mode; Roman numerals resolve via @scale root
  ↓
@track melody mode=lydian     // overrides mode for degree resolution only;
                              // chord context still comes from follow=
```

`@harmony` deliberately cannot override `mode=` (hard parse error). Allowing it
would create a confusing three-way interaction where a track's degree resolution
depends both on which harmony block it follows and on what that block declared.
Mode lives on `@scale` or `@track`, never `@harmony`. A track-level `mode=`
override is musically valid — a soloist playing Lydian over major harmony.

If `@scale` is absent, the compiler falls back to C major with a warning.

### Roman numerals and diatonic quality inference

Roman numeral roots are scale-relative by definition — that is their whole value:
changing `@scale root=C` to `root=Eb` transposes the entire piece. But if quality
suffixes were absolute, a silent class of transposition bugs appeared: `ii7` would
produce a *dominant* seventh on D, and bare `vii` a *minor* triad where the diatonic
triad is diminished. The case convention (uppercase major / lowercase minor) covers
five of seven diatonic triads in major and fails exactly where the diatonic quality
is diminished or augmented.

The fix is split inference: bare Roman numerals always take their triad quality
from the scale degree and active mode; numeric-only suffixes (`7`, `9`, `11`, `13`)
extend diatonically on minor/diminished triads but stay absolute on major/augmented
triads (so `V7`, `I7`, `IV7` remain dominant sevenths, as every jazz musician
expects); explicit quality suffixes (`maj7`, `m7`, `ø7`, …) always override.
Non-heptatonic modes are excluded — quality inference has no well-defined answer
there, so bare Roman numerals in those modes are a compile error.

### Why Roman numerals are parsed in the chord parser, not the lexer

Roman numeral roots only appear inside harmony bar lines, never in pattern steps.
Adding them as lexer tokens would pollute the global token set and create ambiguity
with identifiers. The chord parser recognizes them as an alternative root form;
after resolution against the scale timeline they are identical to letter-based
chords internally — nothing downstream knows the difference.

### Sharp roots and sharp alterations

The lexer tokenizes `#` separately (it is a meaningful token in step-token
accidentals), so `F#` arrives as `Ident("F") + Sharp`. All `root=` sites share a
single `parse_note_root()` helper that reconstructs the sharp root. In chord
symbols, disambiguation is positional: a single note letter followed by `#` is a
sharp *root* (`F#9` is F-sharp dominant ninth); `#` after a longer symbol is an
*alteration* (`F7#9`, `G7#5#9`). An F-root sharp-nine chord is therefore spelled
`F7#9`, never `F#9`.

---

## 3. Degree Tokens: `^n` vs `%n`, and `$chord`

Early versions used `^n` for both scale-relative and chord-relative resolution:
chord-tone degrees (1, 3, 5, 7) resolved via chord intervals and the others via
scale intervals. That parity split was musically arbitrary — users writing
`^1 ^2 ^3` expected to walk up a scale — and fragile (extensions like the 9th
needed special-casing). The redesign separates the two audiences cleanly:

- **`^n` is always scale-absolute.** It resolves against `@scale` mode intervals,
  needs no harmony context, and a melody using only `^n` never needs `follow=`.
- **`%n` is chord-ordinal.** It selects the nth chord tone in root-position
  ascending order, wrapping upward with octave displacement
  (`tone_index = (n-1) % K`, `octave_shift = (n-1) / K` for a K-tone chord).
  A line like `%1 %5 %7` stays in-chord across any progression regardless of
  chord size — swapping `I` for `Imaj7` upgrades `%7` without breaking anything.

`%n` ordering is independent of inversion: `inv=` affects the voicing layout,
never the ordinal labeling. Accidentals are invalid on `%n` — ordinal positions
are fully determined by the chord quality.

**`$chord`** emits the whole active chord, voiced with the track's settings. It
generalizes `@harmony play=true` (which is essentially a hidden `$chord` sustained
per chord span) and, combined with `arp()`, subsumes the need for a dedicated
arpeggiator concept. The token was renamed from `$_`: an underscore is a *discard*
symbol in most languages — the opposite of "the semantically active chord" — so
the readable keyword won. Requiring a harmony context for `%n`/`$chord` is a hard
compile error rather than a silent default: there is nothing sensible to resolve
against.

---

## 4. Syntax Decisions

### Why `->` is the transform pipe (and `|` is not)

`|` was overloaded: bar separator in `@harmony` (and later in every timeline
directive) *and* transform pipe in `play:` expressions. Even with the parser able
to disambiguate, readers had to track context to know which `|` was which. `->`
reads as "then apply", matches the directional feel of `>>`/`~>>`, and never
appears in bar lines. After the change (and the variant-pool change below), `|`
means exactly one thing: timeline/bar separator. Old pipe usage is a hard error
(`DeprecatedPipeOperator`) rather than silently accepted — the language predates
any published corpus, so clean cuts beat compatibility shims.

### Why variant pools use commas

`{a|b|c}` read like a miniature bar line. `,` is the universal separator for
discrete alternatives and has no other meaning in the language. (`DeprecatedVariantPipe`
guards the old form.)

### Precedence: `()` → `*`/`*~` → `>>`/`~>>` → `->`

Users expect Unix-pipe semantics: build the sequence with `>>`, then process it.
Originally the pipe bound tighter than `>>`, so `a >> b | reverse` reversed only
`b`. The precedence was flipped so the transform applies to the entire preceding
expression, and parentheses were added for the less common per-segment case:
`a >> (b -> reverse) >> c`.

### The unified timeline model

`@bpm`, `@ts`, and `@scale` all support the same three forms — scalar, inline
timeline (`|`-separated segments with `* N` bar counts), and indented block — and
`@harmony` shares the same `* N` segment grammar. Before unification there were
two parallel tempo systems (`@bpm` scalar vs a `@tempo` block), key changes were
buried inside harmony blocks via `section:`, and meter changes had no syntax at
all. One mental model now covers all global time-varying state.

- **Why `@tempo` was removed (hard error):** it was concept-identical to `@bpm`'s
  timeline form — one bar-based tempo timeline split across two names for no
  user-facing reason. Direct substitution made a hard error the honest choice.
- **Why `section:` is deprecated (warning):** tonal modulation is a *global*
  property that every track should see, not something embedded in one harmony
  block. The `@scale` timeline expresses it at the right level. `section:` still
  parses (warning) because it has no exact one-line substitution.
- **ms-format timing uses the effective BPM at the event's tick,** not a static
  reference — a 50ms shift should stay 50 real milliseconds through a ramp.

### Ergonomics: optional names, `steps=`, `follow=` inference, `@bars`

In a single-harmony file, neither the harmony name nor `follow=` carries any
information, so both are optional/inferred; with multiple harmony blocks, names
and explicit `follow=` become mandatory. `steps=` is optional everywhere: the
parser always knows the actual count after parsing the body, and requiring the
declaration meant every added step line forced a second edit. When declared it is
still validated (`StepCountMismatch`) as an opt-in safety net. `unit=` cannot be
inferred and stays required. `@bars N` fills bare pattern references to N bars at
resolution time (wrapping them in `Repeat`), not at runtime — keeping the compiled
stream deterministic and the scheduler free of bar-count logic.

### Smaller notational choices

- **Inline pattern bodies** use a `:` separator (`@pattern p unit=1/4: ^1 ^3 ^5`):
  `=` conflicts with pattern assignment, `|` with bar lines, bare whitespace is
  ambiguous. In inline form whitespace separates *steps*, and `+` remains the only
  way to express simultaneity in both forms.
- **Per-reference rate** is a bare `@` suffix (`theme@2.0`): concise, reads as
  "theme at double speed", and cannot collide with `@pattern`-style directives
  because the lexer longest-matches. It multiplies with track-level `rate=`.
- **`start=<bar>`** exists because padding entrances with silence patterns was
  wasteful and leaked pattern defaults; a bar-based offset is easier to reason
  about than ticks and covers all identified use cases.
- **`scale_lock`** is not called "quantize" because quantize conventionally means
  *timing* snap in music production.

---

## 5. Determinism and Seeding

Interval guarantees identical output for identical (source, seed). Three
consequences follow:

1. **No `rand` crate.** Its sequence for a given seed is not guaranteed stable
   across versions; a dependency bump could silently change every seeded render.
   The PRNG is a hand-written xorshift64 (seed 0 mapped to a nonzero constant),
   and per-track seeds derive from the global seed via a hand-written FNV-1a over
   `(seed_le_bytes, index_le_bytes)`. Both algorithms are normative — spec §11.
2. **Seed resolution lives in the CLI/RT layer, not the core.** OS randomness and
   clocks are unavailable under `wasm32-unknown-unknown`; the core accepts a fully
   resolved `Option<u64>` seed and never generates entropy. When no seed is given,
   an ephemeral one is drawn at render start and logged (`seed: <N>`), so a happy
   generative accident is always recoverable; explicitly-provided seeds are also
   embedded as an SMF text meta-event.
3. **A fixed draw contract.** Every seeded operation consumes a fixed number of
   RNG draws in a fixed order regardless of parameters or outcome (one per
   `[prob:N]` step, one per multi-alternative variant step under `vary`, two per
   humanized note, …). SMF and RT consume the identical stream, so their outputs
   match event-for-event. The accepted tradeoff: inserting or removing a seeded
   operation shifts all later draws on that track. `[prob:N]` deliberately shares
   the same per-track stream as `humanize`/`vary` rather than owning a private
   one — one stream is easier to reason about and keeps the two output paths in
   lockstep.

Stochastic transforms baked into a `@pattern` declaration re-roll per reference:
identical humanization on every repetition would sound mechanical and defeat the
purpose. Deterministic transforms (reverse, transpose, …) are stable per reference.
This matches the DAW mental model of insert effects with random modulation.

---

## 6. Compiler Architecture

### Two-pass parsing

The global header (`@ppq`, `@bpm`, `@ts`) determines tick math for everything —
harmony `steps:` durations, pattern `unit` conversion. Pass 1 extracts the header;
pass 2 parses all blocks with header values in hand. The alternative (deferring
all tick calculation) complicates every downstream stage for no benefit.

### Hand-written recursive descent over `logos` tokens

The grammar is small enough that a hand-written parser produces clearer, targeted
error messages with source spans. `nom` combinators obscure the grammar structure;
`pest` adds a separate grammar file and generated code that is harder to attach
good diagnostics to. Errors carry `codespan`-compatible spans; the core never
depends on `codespan-reporting` itself (the CLI renders).

### The harmony index is an interval tree

The index maps tick ranges to `ChordContext` (root, quality, extensions, slash
bass, mode, scale root). Patterns ask "what chord is active at tick T?" in
O(log n), and the tree handles the nesting of bar-level chords with step-level
`steps:` overrides naturally, where a sorted vec would need careful overlapping-
range handling.

### Sequential per-track emission and voice-leading state

`inv=auto` chooses each multi-note step's inversion to minimize voice movement
from the *previous* step's resolved pitches — inherently sequential. The state is
threaded explicitly as `Option<Vec<u8>>` through the emission loop; it cannot live
in the pattern AST because patterns are shared between tracks (each track keeps
independent state), and it persists across pattern boundaries within a track's
`play:` expression (including soft/hard boundaries and the play-mode loop wrap).
The per-track compilation pass must not be parallelized.

### `MusicalContext` bundles the stateless emit parameters

The emit functions once took 25+ parameters, most of them per-track constants
(channel, harmony index, scale timeline, header, BPM lookup, …). They are bundled
in a `MusicalContext<'a>` borrow; only genuinely per-step state (tie state,
voice-leading state, RNG, evolve offset, arp config) travels separately. Adding a
new timeline becomes a new field, not a parameter threaded through every function.
The refactor was behavior-preserving by construction (golden-test verified).

### Transform phases and enforced ordering

Transforms run in three phases: structural (pattern resolution time), emission
(per-step offsets), and stream (post-emission event rewriting). Within emission
pipelines the canonical order **swing → expressive → humanize** is *enforced* — a
differently ordered pipeline is a compile error rather than being silently
reordered, because silent reordering hides bugs and surprises users.

- **`arp()` is emission-phase**, not structural: `$chord` and `%n` have no pitches
  until the harmony index is queried at compile time, so a structural arp would
  have nothing to explode. The transform is a no-op at pattern resolution; the
  config rides down to `emit_token`, which expands multi-pitch steps after chord
  resolution.
- **Swing is applied during step emission**, not as a post-pass: swing must not
  apply inside subdivision brackets, and by the time events are flattened,
  subdivision provenance is lost. Applying at emission avoids a `from_subdivision`
  flag on every event.
- **Echo clamps at the pattern boundary.** Bleed into the next instance would
  interact horribly with conditional steps, loop counting, and voice-leading
  state. Composers who want a tail add one.
- **`ritardando`/`accelerando` are constrained** to the last/first pattern
  instance of a `play:` expression — they change total duration, and applied
  mid-arrangement they would desynchronize the track from the bar grid
  (mid-placement warns with the tick overshoot).
- **Scale snapping is shared:** `evolve()` snaps to the nearest in-scale pitch and
  `scale_lock` snaps down/up/filters, via common helpers in `voicing.rs`.

### Per-segment defaults

`play: silence * 20 >> burst` must not leak `silence`'s velocity/gate/octave into
`burst`. Resolved patterns carry `SegmentDefaults` per constituent pattern, and
the emission loop looks up the segment containing each step. The rejected
alternative — track-level values always overriding pattern defaults — would break
the model that patterns carry their own defaults.

---

## 7. Event Stream and the Real-Time Engine

### Structural markers in the stream

`BarMarker` and `PatternBoundary` events are compiled into the stream itself
rather than derived from side metadata, keeping the event stream the single
source of truth for timing (no drift between two computations of the same
boundaries). The SMF renderer strips them; the RT scheduler consumes them:
hot-swap only triggers at a `BarMarker`, and `PatternBoundary` drives conditional-
step loop counting.

### `PlaybackState` belongs to the RT scheduler

Conditional steps (`[every:N]`, `[cond:X:Y]`, `[once]`, `[pre]`) depend on how
many times a pattern has looped — inherently dynamic, meaningless at compile
time. The compiler emits `StepCondition` metadata; the scheduler evaluates it
against per-track `TrackState` (loop count, current pattern name, last-conditional
flag, active notes). The SMF renderer instead does a static evaluation pass and
never touches playback state.

**The first pass is loop 0 in both outputs.** `PatternBoundary` fires at the
*start* of each instance, so the first boundary begins pass 0; `[once]` plays on
the first pass and `[every:4]` fires on the same passes in SMF and RT.

### Continuous looping in play mode

Play mode always loops (wrap to tick 0 at stream end); compile mode stays finite
for export. The state decisions at the wrap point are deliberate: loop counts are
*not* reset (conditional steps keep evolving across arrangement loops),
voice-leading state carries over (a reset would produce a voicing jump), active
notes are flushed to prevent stuck notes, and the harmony timeline is already
cyclic so it needs nothing.

### Hot-swap

New streams compile in a background thread and stage via `arc-swap`; the swap is
gated on a `just_crossed_bar` flag set when a `BarMarker` dispatches, so swaps
land on musically clean boundaries. `SwapMode` is an enum (`Immediate` seeks to
the top of the current bar; `Next` seeks to the same beat offset in the next bar
so playback never rewinds), not a bool — future modes like phrase-aligned swap
stay representable. On swap, per-track loop counts survive when the pattern
identity is unchanged, and orphaned notes get NoteOffs. The file-watch debounce
is 30ms: editors write atomically in <1ms and the OS notification dominates, so
a long debounce only added latency.

---

## 8. Introspection and IDE Integration

- **Introspection lives in `interval-core`** so the WASM build can serve IDE
  features; a separate crate would only add a dependency edge for code that reads
  static tables and does no I/O.
- **`parse_only()`** is the core-owned parse entry point (the CLI used to own the
  block-parsing loop); seed resolution stays out of it for WASM-safety.
  **`compile_with_ast()`** lives in `introspect.rs`, not `compiler.rs` — attaching
  the AST to output is an IDE convenience, and `compile()` stays pure.
- **AST spans are `Option<Span>`** because transforms synthesize nodes with no
  source location; fake spans would mislead hover/click. Span fields are
  `#[serde(skip)]` so golden JSON never changes.
- **`TrackSummary` is built during compilation** because pattern-instance tick
  ranges only exist after rate scaling and bar layout; building from the AST
  would duplicate tick math. Similarly `harmony_timeline()` reads the compiled
  `HarmonyIndex`, not the raw block.
- **Cheap vs rich cursor context** are separate functions so an IDE can call the
  parse-only version per keystroke and pass a cached `CompileOutput` for the rich
  one; both degrade gracefully on parse failure instead of blocking the editor.
- **Source-editing helpers validate by re-parsing** their result — one extra
  parse (<1ms on typical files) buys protection against edits that produce
  unparseable source, with none of the fragility of structural validation.

---

## 9. Roads Not Taken

Features that would be natural extensions, considered and rejected or deferred:

- **`@harmony mode=` override** — rejected; see §2. Mode lives on `@scale` or
  `@track`, never `@harmony`.
- **Echo bleed across pattern boundaries** — musically desirable for transitions,
  but the interactions with conditional steps and loop counting are nightmarish.
- **Swing inside subdivision brackets** — swung triplets are musically unusual
  and the nested-subdivision interaction is complex; swing stops at brackets.
- **Arbitrary transform pipeline ordering** — silently reordering internally
  would hide bugs; the canonical order is enforced instead.
- **Track-level defaults overriding pattern defaults** — would break the
  contract that patterns carry their own feel; per-segment defaults won.
- **Tick-based `offset=` instead of `start=<bar>`** — more flexible, harder to
  reason about, no identified use case.
- **`evolve()` as a pattern *generator*** (in addition to the transform form) —
  the transform form is more composable and proved sufficient.
- **A dedicated polymetric arpeggiator (Arpoly)** — largely subsumed by
  `$chord` + `arp()`; anything beyond that waits on independent parameter lanes.
- **Deferred pending real design work** (see spec §18): independent parameter
  lanes (Matriceal — requires multiple independent playheads per track, a
  fundamental change to the compilation model), `reharmonize()`,
  `voice_lead()`, user-defined transform functions (inline expression language
  vs plugin API is unresolved; the latter breaks the single-file model),
  mid-track program changes, polyphonic aftertouch, sysex/NRPN.
- **Cross-track note context** (what else is sounding at tick T) — would enable
  register-aware voicing and `reharmonize()`, and is computable at compile time
  via a pre-pass, but it breaks per-track compilation isolation. Deliberately
  postponed until a concrete feature needs it.
