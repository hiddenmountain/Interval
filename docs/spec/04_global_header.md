# 4. Global Header

The global header defines file-wide constants. All directives are optional; defaults are specified below.

```
@ppq   <int>          // default: 480
@bpm   <float>        // default: 120.0  (scalar, inline, or block form — see §12)
@ts    <int>/<int>    // default: 4/4    (scalar or inline form)
@title "<string>"     // default: none
@seed  <int>          // default: none (random)
@bars  <int> | off    // default: none
```

### 4.1 @ppq

Pulses per quarter note. Affects all tick calculations. Must be a positive integer. Standard values: 96, 120, 240, 480, 960.

### 4.2 @bpm

Tempo in beats per minute. `@bpm` supports three forms: scalar, inline timeline, and block timeline. See §12 for full details.

The scalar form `@bpm 120` serves as the single global tempo when no timeline entries are provided. Fractional values permitted (`@bpm 92.5`).

`@bpm` is used as the reference BPM for `ms`-format timing and `humanize` calculations even when timeline entries are present.

There is no `@tempo` directive. Writing one is a hard compile error directing the author to `@bpm`'s inline or block form (§12.7).

### 4.3 @ts

Time signature for SMF export metadata. `@ts` supports scalar and inline timeline forms. Numerator and denominator are positive integers. The denominator must be a power of 2.

### 4.4 @title

Optional string metadata written to the SMF file as a track name event.

### 4.5 @bars

Global bar count for automatic pattern fill. When present, bare pattern references in `play:` expressions (without explicit `* N` repeat) are automatically repeated to fill the specified number of bars.

```
@bars <int>     // Fill bare refs to N bars
@bars off       // Opt out of fill (bare refs produce one iteration)
```

**Semantics:**

- `@bars N` + bare `play: foo` → compiler computes `ceil(N_bars / pattern_bars)` and wraps in `Repeat`.
- `@bars N` + `play: foo * K` → explicit K iterations (unchanged, not overridden by `@bars`).
- `@bars` absent → default behavior (bare refs produce one iteration).
- `@bars off` → same as absent (explicit opt-out for clarity).

**Example:**
```
@bars 4

@pattern theme unit=1/4
  ^1 ^3 ^5 ^3

@track melody ch=1
  play: theme           // fills to 4 bars (4 iterations)

@track bass ch=2
  play: bassline * 2    // explicit: exactly 2 iterations
```

### 4.6 @seed

Global seed for all seeded operations (variant selection, generative transforms). Any non-negative integer. Per-track seeds override this value for that track.

**Seed resolution:**

- `@seed N` declared → fully deterministic, identical output every render.
- `@seed` absent → a random seed is generated at render start (from system time or OS random). True generative mode — output differs on every render.
- `--seed N` CLI flag → overrides both `@seed` and random generation.

**Seed logging:** The resolved seed is logged to stderr: `seed: <N>`.

**SMF embedding:** The resolved seed is embedded as a text meta-event in SMF track 0 (`seed:<N>`) only when an explicit seed was provided (via `@seed` or `--seed`). Ephemeral OS-random seeds are not embedded, so `.mid` files are artifact-stable when no random transforms are used.

**Per-track seed derivation:** When a track has no explicit `seed=` and a global seed is available, the track seed is derived as `fnv1a(global_seed, track_index)`.

---

[← Back to Table of Contents](00_toc.md)
