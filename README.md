# Interval

A step-first harmonic composition language that compiles to MIDI.

Interval defines three strictly ordered layers — **Harmony**, **Patterns**, **Tracks** — where degree-relative notation in patterns resolves against a shared harmonic timeline at compile time to produce a deterministic MIDI event stream. Change one chord in the harmony and every track updates. Transpose the entire piece by changing a single `@scale root=` value.

Interval is quite experimental. It was designed for sound installations and generative music — pieces that run unattended, vary deterministically from a seed, and follow a harmonic plan with no DAW in sight. But it composes just as well as it generates: because everything is plain text, it also works as a live-coding system (edit the source while it plays; changes hot-swap in at the next bar) and as a terminal-first composition tool for drafting complex harmonic ideas quickly — often faster than a piano roll allows.

> **Status:** pre-1.0. The language and APIs are still settling; see
> [docs/spec](docs/spec/00_toc.md) for the current language specification and
> [docs/design.md](docs/design.md) for the design rationale.

## Installing

Requires Rust 1.87+.

```sh
cargo install --path interval-cli
```

This installs the `interval` binary. Or build in place (`cargo build --release`;
the binary is at `target/release/interval`).

## Usage

### Compile to MIDI

```sh
interval compile song.interval
```

Produces `song.mid` alongside the source file. Override the output path with `-o`:

```sh
interval compile song.interval -o output.mid
```

### Check for errors

```sh
interval check song.interval
```

Validates syntax and semantics without producing output. Reports note count and track count on success.

### Real-time playback

```sh
interval play song.interval
```

With a single MIDI output port available, playback starts on it; with several, the CLI lists them and prompts. Select one directly with `--port N`:

```sh
interval play song.interval --port 1
```

Editing the source file during playback triggers a **hot-swap** — the new version compiles in the background and swaps in at the next bar boundary.

Press Enter or Ctrl+C to stop.

### Dump event stream

```sh
interval dump song.interval
```

Prints the compiled event stream as human-readable text for debugging.

### Seed control

All subcommands that compile accept `--seed N` to override the random seed for deterministic output:

```sh
interval compile song.interval --seed 42
```

If the source file contains `@seed 42`, that value is used unless overridden by `--seed`. Without either, an ephemeral time-derived seed is used (not embedded in the output), and `play` keeps it stable across hot-swap recompiles.

## Quick Example

```
@title "Autumn Sketch"
@bpm 96
@ts 4/4
@seed 1

@scale root=D mode=dorian

@harmony main
| im7 | IV7 | bVIImaj7 | IIImaj7 |

@pattern walk steps=4 unit=1/4 oct=3
^1
^3
^5
^7

@pattern chords steps=4 unit=1/4
^1+^3+^5
.
^1+^3+^5
.

@track bass ch=2 prog=33 follow=main vel=90 gate=0.85
  play: walk * 4

@track keys ch=3 prog=5 follow=main voice=drop2 inv=auto vel=72 gate=0.70
  play: chords * 4
```

Compile it:

```sh
interval compile sketch.interval
```

## Language Overview

### Header

Global settings at the top of every file:

```
@title "My Song"
@bpm 120
@ts 4/4
@ppq 480
@seed 42
```

### Scale

Sets the tonal center. Roman numeral chords in harmony blocks resolve relative to this root and mode:

```
@scale root=Eb mode=minor
```

### Harmony

Defines the chord progression as bar lines. Chords can be letter-based or Roman numeral:

```
@harmony main
| Cmaj7 Am7 | Dm7 G7 |
```

```
@harmony main
| im7 | IV7 | bVIImaj7 | V7 |
```

### Patterns

Step sequences with one step per line. Scale-degree tokens resolve against the active `@scale`; chord tokens resolve against the harmony timeline at each step's tick:

| Token | Meaning |
|-------|---------|
| `^1`, `^5` | Scale degree (scale-absolute) |
| `^b7`, `^#11` | Altered degree |
| `^1/3` | Degree in a specific octave |
| `%1`, `%3` | Chord tone by ordinal (root, 5th of the active chord) |
| `$chord` | The active harmony chord, voiced |
| `$Cmaj7` | Literal chord symbol, voiced |
| `C4`, `Eb5` | Absolute pitch |
| `n60` | MIDI note number |
| `.` | Rest |
| `~` | Tie (extend previous note) |
| `(^1 ^3 ^5)` | Subdivision (notes share one step) |
| `^1+^3+^5` | Simultaneous notes (chord) |
| `{^1,^3,^5}` | Variant pool (randomized with `vary`) |

```
@pattern bass steps=4 unit=1/4 oct=3
^1
^5
^1
(^3 ^5)
```

### Tracks

Assign patterns to MIDI channels with playback parameters:

```
@track piano ch=1 prog=1 follow=main voice=drop2 inv=auto vel=80 gate=0.75
  play: intro >> verse * 4 >> outro
```

Key parameters: `ch=`, `prog=`, `vel=`, `gate=`, `oct=`, `follow=`, `voice=`, `inv=`, `mode=`, `rate=`, `swing=`, `swingunit=`.

### Drum tracks

```
@drummap kit
  kick = 36
  snare = 38
  hh = 42

@track drums ch=10 type=drums drummap=kit vel=100
  play: beat * 8
```

### Transforms

Chain transforms in `play:` expressions with `->`:

```
play: melody * 8 -> rubato(0.15, arch) -> humanize(3%, 0.30)
```

Available transforms: `transpose`, `shift_oct`, `retrograde`, `reverse`, `rotate`, `mirror`, `invert`, `stretch`, `compress`, `subset`, `interleave`, `arp`, `humanize`, `vary`, `swing`, `rubato`, `ritardando`, `accelerando`, `agogic`, `breathe`, `swell`, `phrase`, `evolve`, `euclid_gate`, `echo`, `vel_curve`, `gate_curve`, `scale_lock`.

The full syntax reference lives in [docs/spec](docs/spec/00_toc.md) — 19 chapters
covering the harmony timeline, patterns, tracks, transforms, and the compiler.

## Examples

The [examples/](examples/) directory contains complete pieces — Bach's Prelude in
C, Für Elise, Pachelbel's Canon, plus originals exercising odd meters, BPM ramps,
conditional drum grids, and scale modulation. Each ships with its compiled `.mid`.

## Project Structure

| Crate | Purpose |
|-------|---------|
| `interval-core` | Lexer, parser, compiler, event stream (WASM-safe) |
| `interval-smf` | Standard MIDI File renderer (WASM-safe) |
| `interval-rt` | Real-time scheduler with hot-swap (native) |
| `interval-cli` | CLI tool |
| `interval-wasm` | Raw-cdylib WASM bindings for web hosts (standalone crate) |

## Testing

```sh
cargo test          # unit tests + golden file tests
cargo clippy        # lint
```

Golden file tests live in `interval-core/tests/golden/` (one directory per case:
`input.interval` + `expected.json`). After an intentional output change, re-bless
them with:

```sh
cargo test --test golden_tests -- --ignored
```

## License

Apache-2.0 — see [LICENSE](LICENSE). Attribution is required: keep the copyright
and license notices and the [NOTICE](NOTICE) file with any redistribution. The
Interval name is not covered by the code license — please don't use it to imply
endorsement or to brand derived products.
