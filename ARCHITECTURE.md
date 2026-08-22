# Interval Architecture

A compact orientation for contributors. The full language specification lives
in `docs/spec/` (one file per chapter); the design rationale is in
`docs/design.md`.

## The language model

Interval is a step-first harmonic composition language that compiles to MIDI.
A program defines three strictly ordered layers:

```
Harmony  →  Patterns  →  Tracks
```

- **Harmony** (`@harmony`) is a timeline of chords over bars, anchored by the
  global `@scale` tonal context.
- **Patterns** (`@pattern`) are reusable step sequences whose tokens can be
  degree-relative (`^n` scale-absolute, `%n` chord-ordinal, `$chord`) rather
  than absolute pitches.
- **Tracks** (`@track`) place pattern expressions on MIDI channels and bind
  them to a harmony timeline via `follow=`.

Degree-relative notation resolves against the shared harmonic timeline at
compile time, producing a deterministic MIDI event stream.

## Crate layout

```
interval-core   language frontend + middle-end (WASM-safe, no I/O)
                src/: lexer, parser, ast, harmony, pattern, transform,
                      compiler, voicing, event, error, introspect, edit
interval-smf    EventStream → Type 1 Standard MIDI File (WASM-safe)
                src/: renderer
interval-rt     real-time MIDI scheduler (native-only: midir, arc-swap)
                src/: scheduler, hotswap, playback_state, midi_devices
interval-cli    the `interval` binary: compile / play / check / dump,
                file-watch hot-swap, error rendering, seed resolution
interval-wasm   standalone cdylib exposing the core to wasm32-unknown-unknown
```

Hard boundaries:

- `interval-core` and `interval-smf` must compile to
  `wasm32-unknown-unknown`: no `std::fs`/`std::time`/`std::thread`, no
  printing, no native-only dependencies. Errors carry source spans; the CLI
  renders them with `codespan-reporting` (the core never depends on it).
- `interval-smf` and `interval-rt` never depend on each other. Both consume
  the same `EventStream` from the core.

## Compilation pipeline

1. **Two-pass parse.** Pass 1 extracts the global header (`@ppq`, `@bpm`,
   `@ts`, …) because tick math everywhere depends on it; pass 2 parses all
   blocks with header values in hand. The parser is hand-written recursive
   descent over `logos` tokens (chosen over nom/pest for error-message
   quality). `parse_only()` is the public entry point producing a `Program`.

2. **Harmony index.** The harmony timeline compiles into an interval tree
   mapping tick ranges to `ChordContext` (root, quality, extensions, slash
   bass, mode, scale root). Patterns ask "what chord is active at tick T?" in
   O(log n); `steps:` subdivisions and bar-level chords nest naturally. The
   timeline is cyclic: ticks beyond its end wrap via `tick % total_ticks`.

3. **Per-track sequential emission.** For each track, pattern expressions are
   resolved (repeats, concatenation, transforms), then steps are emitted in
   order. This pass is deliberately sequential: `inv=auto` voice leading picks
   each chord's inversion to minimize movement from the *previous* resolved
   pitches, with that state threaded explicitly through the loop (and across
   pattern boundaries within a track). Do not parallelize it.

4. **Sorted event stream.** Output is a single stream of timed events, sorted
   by tick → priority → track. Besides MIDI events it contains structural
   markers: `BarMarker` (bar boundaries) and `PatternBoundary` (pattern loop
   completion). The RT scheduler consumes the markers — hot-swap only occurs
   at a `BarMarker`, and `PatternBoundary` drives conditional-step loop
   counting — while the SMF renderer strips them. This keeps the event stream
   the single source of truth for timing: no side channels.

Runtime-conditional steps (`[every:N]`, `[once]`, …) are compiled as
`StepCondition` metadata on events; `interval-rt` evaluates them against its
`PlaybackState` during playback, while the SMF renderer does a static
evaluation pass.

## Determinism policy

Interval guarantees identical output for identical (source, seed):

- Random operations (`humanize`, `vary`, `[prob:N]`, `evolve`, …) use a
  hand-rolled **xorshift64** PRNG, not the `rand` crate, so a dependency bump
  can never change the sequence for a given seed.
- Per-track seeds derive from the global seed via **FNV-1a** (also implemented
  directly).
- Seed *resolution* (`--seed` flag > `@seed` directive > OS random) happens in
  `interval-cli`/`interval-rt`, never in the core — the core accepts a fully
  resolved seed, keeping it free of OS entropy and clocks (and WASM-safe).

## Where to read more

- `docs/spec/` — the living language specification (syntax, semantics, error
  conditions, introspection and IDE APIs).
- `docs/design.md` — the design rationale, organized by topic.
- `CHANGELOG.md` — release history and migration notes.
- `CONTRIBUTING.md` — build/test workflow, golden tests, and the rules above
  in checklist form.
