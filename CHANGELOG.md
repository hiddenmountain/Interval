# Changelog

All notable changes to Interval are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Versions before the
public release were developed privately; release dates are omitted. Design
rationale lives in `docs/design.md`.

## [0.10.0] — 2026-08-22

The pre-release correctness pass ("the honesty pass"): every documented
feature now does what the spec says, or errors clearly. First public release.

### Added
- Ties (`~`) actually sustain: gate applies to the full extended duration,
  chord steps and subdivision slots tie correctly, restrikes truncate the
  held note.
- Soft pattern boundaries (`*~`, `~>>`): sounding notes carry legato into the
  next instance or extend under a leading tie; hard boundaries cut cleanly.
- Pipeline `humanize()` (correlated timing/velocity at coefficient 0.4,
  deterministic per seed), pipeline `swing()` (scoped to its pattern,
  overriding track-level swing there), `vary()` (one seeded draw per variant
  step), and `invert` / full `retrograde` (reverse → invert).
- Per-track seed derivation via FNV-1a — tracks without `seed=` no longer
  share one RNG stream.
- Sharp chord alterations parse (`G7#9`, `C7b9#11`, `G7#5b9`); documented
  `F#9`-vs-`F7#9` disambiguation.
- `@scale` block/timeline form is reachable; header directives may follow a
  scalar `@scale`; `+` inside subdivision brackets emits a chord.
- BarMarkers are emitted for every bar from the bar layout, so tracks with
  non-aligning `rate=` no longer silently disable hot-swap.

### Changed
- Canonical transform order (swing → expressive → humanize) is enforced per
  `->` chain with a compile error.
- First playback pass is loop 0 in the real-time scheduler, matching the SMF
  renderer's static evaluation (`[once]` now plays live; `[every:N]` fires on
  passes 1, N+1, …). A conditional NoteOff plays iff its NoteOn played.
- Every `[prob:N]` consumes exactly one RNG draw (including `[prob:1.0]`).
- `evolve()` offsets span the documented [-4, +4].
- More chords than beats in a bar is a compile error instead of silently
  unplayable chords.

### Fixed
- Echo clamps at its pattern-instance boundary; subdivision brackets keep the
  division remainder; coarse-rate `arp()` emits one slot instead of silence;
  `arp(rate=...)` named parameters parse.
- Bare Roman numerals re-derive diatonic quality across `@scale` mode changes.
- `gate_curve` retargets the correct NoteOff with the step's effective unit;
  `scale_lock(filter)` removes only the filtered note's own pair.
- Introspection pitch resolution matches the compiler (was one octave low).
- Deterministic NoteOff ordering (no hash-map iteration order in output).
- Real-time robustness: Ctrl+C cleanup, atomic-save file watching, hot-swap
  bar index/seek/tempo correctness, sustain reset on stop.
- SMF renderer validates tempo/time-signature/PPQ ranges and pads
  end-of-track to the stream's true end.

## [0.9.1]

### Added
- `interval-wasm` crate: raw `cdylib` WASM web target wrapping `interval-core`.
- `describe_chord_in_key` resolver in the introspection API.
- `step_index` on `TimedEvent` for piano-roll provenance.
- `PositionSnapshot` in `interval-rt` for UI re-anchoring after hot-swap.
- Release packaging: `interval` binary name, CI workflow, Apache-2.0 license.

### Fixed
- `transform_call_name()` now displays all parameters for `humanize`, `rubato`,
  `swell`, and `breathe` (with proper `TimingValue`/curve formatting).
- `@bars N` fill works correctly with transformed play expressions
  (`play: p -> reverse`); misleading doc comment corrected.

## [0.9]

### Added
- `@bars N` header directive: bare pattern references in `play:` are
  automatically repeated to fill N bars (`@bars off` disables).
- Rich cursor context API (`get_rich_context_at_cursor`): current chord, scale,
  resolved pitch name, harmony bar index, and pattern parameters at a cursor.
- Unified completion provider (`complete_at_cursor`) with context-aware chord,
  scale, transform, annotation, directive, and pattern-reference completions.
- Source editing helpers (`edit` module): insert/remove/replace step, set
  annotation, add transform, set track/header parameters — all validated by
  re-parsing.
- Roman numeral display on `HarmonyChordInfo`.
- Step-line spans are now populated by the parser.

## [0.8]

### Added
- Source spans (`Option<Span>`) on all AST nodes for IDE integration.
- `parse_only()` public entry point producing a `Program` AST.
- `TrackSummary` per-track metadata (channel, patterns, tick ranges) on
  `CompileOutput`.
- Structured harmony and scale timeline introspection (`harmony_timeline`,
  `scale_timeline_info`) with formatted chord symbols.
- `compile_with_ast()`, `resolve_step_pitches()`, and `get_context_at_cursor()`
  IDE convenience APIs.
- MIDI device selection API in `interval-rt` (`midi_devices` module:
  `list_midi_outputs`, `connect_midi_output`, `connect_midi_output_by_name`).

## [0.7]

### Added
- Continuous looping in `play` mode: the arrangement wraps to the start with
  notes-off, preserving conditional-step and voice-leading state.
- Tick-accurate hot-swap seeking with `--swap-mode=immediate|next`.
- `steps=` is now optional everywhere (inferred from the body; still validated
  when declared).
- Expanded music theory registries: 18 scales, additional chord qualities and
  voicings.

### Fixed
- Sharp note roots (`F#`, `C#`, …) accepted at every `root=` site via the
  shared `parse_note_root()` helper.
- `PatternBoundary` events are now actually emitted by the compiler, making
  conditional playback (`[every:N]`, `[once]`, `[cond:X:Y]`, `[pre]`) functional.
- `@ts` timelines now affect musical timing via per-bar `BarLayout` (variable
  bar lengths), not just metadata.
- `rotate()` no longer corrupts segment metadata.

## [0.6]

### Added
- Inline pattern body form: `@pattern p unit=1/4: ^1 ^3 ^5 ^7` (with `steps=`
  inferred from the token count).

### Changed
- Emit pipeline refactored around a `MusicalContext` struct; per-step
  allocations and clones eliminated (bit-identical output).
- Hot-swap now only fires at `BarMarker` boundaries (no more mid-bar swaps).
- File-watch debounce reduced from 100ms to 30ms for faster hot-swap.

## [0.5]

### Added
- `%n` chord-ordinal tokens (1=root, 2=third, … with octave wrapping); `^n` is
  now purely scale-absolute.
- Unified timeline forms for `@bpm`, `@ts`, and `@scale`: scalar, inline
  (`120 * 8 | 140`), and block forms, including BPM ramps.
- Roman numeral diatonic quality inference (`ii` in major → minor triad, etc.).
- Optional harmony block names with automatic `follow=` inference when a single
  block exists.
- `arp()` emission-phase transform (up/down/updown/random, octave layers).
- `[prob:N]` probabilistic steps and `[glide]` portamento annotations.
- `@harmony inv=` block-level inversion default (harmony < track < step).
- 8 new scales (pentatonics, blues, altered, …) and 7 new chord qualities
  (`7sus4`, `m11`, `5`, `6/9`, …).

### Changed
- `->` replaces `|` as the transform pipe (old `|` is a hard error).
- Variant pools use commas: `{a,b,c}` (old `{a|b|c}` is a hard error).
- `$chord` replaces `$_` for the current-chord token (old form is a hard error).

### Removed
- `@tempo` (hard error) — superseded by `@bpm` timeline forms.

### Deprecated
- `section:` inside `@harmony` (warning) — use `@scale` timelines instead.

## [0.4]

### Added
- Parentheses in pattern expressions, with `|` flipped to loosest precedence
  (Unix pipe semantics: `(a >> b) | reverse`).
- `start=<bar>` track parameter to offset a track's entry.
- `$_` current-chord token (resolved from `follow=` harmony at compile time).
- Per-reference rate: `theme@2.0` in `play:` expressions.
- Baked-in transforms on `@pattern` declarations (stochastic ones re-roll per
  reference).
- `@tempo` timeline with constant segments and ramps.
- Introspection API (`introspect` module): chord qualities, scales, degree
  resolution, directives, transforms — WASM-safe for IDE integration.

## [0.3]

### Added
- `@scale` block as global tonal anchor (root + mode); `mode=` removed from
  `@harmony`.
- Roman numeral harmony (`Imaj7 | IV | V7`), including borrowed `b`/`#` roots.
- Conditional steps: `[every:N]`, `[cond:X:Y]`, `[once]`, `[pre]`, evaluated at
  runtime against playback state.
- Ratcheting: `[ratch:N]` with `[ratch_decay:F]`.
- Swing (`swing=` / `swingunit=` and pipeline form).
- Expressive performance transforms: `rubato`, `ritardando`, `accelerando`,
  `agogic`, `breathe`, `swell`, `phrase` with easing curves.
- Track-level `rate=` and cyclic harmony looping (`tick % total`).
- Generative transforms: `evolve()` (shift register), `euclid_gate()`
  (Bjorklund), `echo()`, `vel_curve()` / `gate_curve()`, `scale_lock()`.
- Seed handling: `@seed` optional, `--seed` CLI override, OS-random fallback,
  seed embedded in SMF when explicit.
- `PlaybackState` in the RT scheduler and `PatternBoundary` events.

### Fixed
- Sharp chord roots (`F#7`, `C#m7b5`), `mMaj7` quality, `scale_lock` filter
  orphaned note-offs.

## [0.2]

Initial complete implementation:

- Lexer (logos) and hand-written recursive-descent parser for the full v0.2
  grammar: global header, `@harmony`, `@pattern`, `@track`, `@drummap`.
- Harmony timeline as an interval tree with per-tick chord context queries,
  beat assignment, `steps:` blocks, and `section:` modulation.
- Pattern language: degrees, absolute pitches, MIDI numbers, chord symbols,
  rests, ties, simultaneous notes, nested subdivision brackets, variant pools,
  and step annotations (vel, gate, shift, ramps, CC, and more).
- Pattern composition operators (`*`, `*~`, `>>`, `~>>`) and step-level
  transforms (reverse, rotate, subset, mirror, interleave, stretch, compress,
  transpose, shift_oct, retrograde).
- Compiler: degree resolution, five voicing strategies, inversions including
  `inv=auto` voice leading, slash bass, timing/gate/shift, event emission with
  `BarMarker` events.
- SMF renderer (Type 1, via any `std::io::Write`) with round-trip validation.
- Real-time scheduler with play/pause/stop, active-note tracking, and
  `arc-swap` hot-swap at bar boundaries.
- CLI with `compile`, `play`, `check`, and `dump` subcommands and
  file-watching.
- Golden-file test harness; `interval-core` and `interval-smf` compile to
  `wasm32-unknown-unknown`.
