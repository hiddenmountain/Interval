# 11. Generative System

The generative system covers everything in the language whose output depends on a random draw: probabilistic steps (`[prob:N]`), variant pools under `vary()`, `humanize()`, `evolve()`, the `random` arp pattern, and the `random` wave shape of `vel_curve`/`gate_curve`. All of it is driven by one seeded pseudorandom stream per track, which makes every render fully reproducible: **identical (source, seed) always produces an identical event stream**, in both the SMF renderer and the RT scheduler.

`euclid_gate()` is included in this chapter for completeness but consumes no randomness — Bjorklund's algorithm is fully deterministic.

### 11.1 Seeds

**Global seed.** `@seed N` in the header (§4.6) fixes the composition's seed. When absent, an ephemeral seed is drawn from the OS at render start (and logged to stderr); the `--seed N` CLI flag overrides both. Seed *resolution* happens in the CLI/RT layer, never in `interval-core` — the core accepts a fully resolved seed, keeping it free of OS entropy and WASM-safe.

**Per-track seeds.** Each track owns an independent RNG stream. An explicit `@track seed=N` wins; otherwise the track seed is derived from the global seed and the track's declaration index:

```
track_seed = fnv1a(global_seed, track_index)
```

Derivation guarantees that tracks do not mutate in lockstep — two tracks playing the same humanized pattern get different timing offsets.

**The algorithms are part of the language guarantee.** Seed-stable output across compiler versions is a promise Interval makes, so the exact algorithms are normative and implemented directly (never via an external crate whose sequence could change with a version bump).

Per-track derivation is FNV-1a over the little-endian bytes of the global seed followed by the little-endian bytes of the track index:

```rust
fn fnv1a_derive(seed: u64, index: usize) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;  // FNV offset basis
    for byte in seed.to_le_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);  // FNV prime
    }
    for byte in (index as u64).to_le_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
```

The per-track stream itself is xorshift64:

```rust
fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}
```

A seed of 0 (the xorshift fixed point) is mapped to the nonzero constant `0x517cc1b727220a95` before first use.

### 11.2 The Determinism Contract

Each seeded operation consumes a fixed number of draws from the track's stream, in a fixed order, **regardless of parameter values or outcome**:

| Operation | Draws consumed |
|---|---|
| `[prob:N]` | 1 per annotated step — even for `prob:1.0` and `prob:0.0` |
| `vary(p)` at a variant step | 1 per step whose pool has two or more alternatives — even when the roll fails and the first alternative plays |
| `humanize(timing, intensity)` | 2 per humanized note (timing draw + independent velocity draw) |
| `evolve(toggle)` | 1 to initialize the register, then 1 per step of the resolved sequence |
| `arp(pattern=random)` | K−1 per multi-note step (Fisher–Yates shuffle of a K-tone stack) |
| `vel_curve`/`gate_curve` with `wave=random` | 1 per step, after emission |
| `euclid_gate(pulses, steps)` | 0 (deterministic) |

Within a track, draws occur in a fixed order: `evolve()` offsets are computed up front (register init + one draw per step across the whole resolved step sequence); then emission proceeds step by step — variant selection, then the `[prob:N]` roll, then per-note humanize draws and any random arp shuffle; finally, `random`-wave curve draws happen in the stream-transform pass.

**Draw-order dependence is accepted and documented:** adding or removing any seeded operation earlier in a track (for example, deleting a `[prob:0.9]` annotation) shifts every subsequent draw on that track and changes the downstream humanize/vary/evolve output. This is the same tradeoff as any seeded sequence, and it is the price of the stronger guarantee: for a fixed source and seed, SMF export and real-time playback are event-for-event identical.

A stochastic transform baked into a `@pattern` declaration re-rolls per reference (§7.14) — each instance in a `play:` expression simply continues consuming the same per-track stream, so repetitions differ from each other while the whole remains reproducible.

### 11.3 Probabilistic Steps — `[prob:N]`

`[prob:N]` (§7.6) gives a step an N probability of playing, where N is `0.0`–`1.0` or a percent value. Every annotated step consumes exactly one draw; the value only decides suppression. A suppressed step settles prior sounding notes exactly as a rest would, and a following tie finds nothing to extend — a suppressed note is never resurrected. Tie *legality* is purely structural and never depends on the roll (§7.3.8).

- `[prob:1.0]` always plays (still consumes its draw).
- `[prob:0.0]` never plays and produces a compile warning.

### 11.4 Variant Pools and `vary(p)`

A `{a,b,c}` variant pool (§7.11) is inert without `vary()`: the first alternative always plays and no draw is consumed. Under `vary(p)`, each step containing a pool with two or more alternatives consumes one draw `u`:

- if `u < p`, the accepted draw is rescaled to uniform `[0,1)` via `u / p` and indexes the alternatives **uniformly — the first alternative can be re-selected**;
- otherwise the first alternative plays.

Single-alternative pools have no variability and consume no draw (the parser also encodes `+` clusters inside subdivision brackets as single-alternative pools; these are plain chords, not variant steps).

### 11.5 Humanize

`humanize(timing, intensity)` (§10.5) draws two independent uniforms in `[-1, 1)` per note. The first scales to the maximum timing deviation (resolved from `%`, fraction, or `ms` at the step's effective BPM, §12.5). The velocity deviation is correlated with the timing deviation at coefficient 0.4:

```
vel_offset_norm = 0.4 * timing_norm + sqrt(1 - 0.4²) * independent_vel
```

applied with a negative sign so a note pushed late tends to be softer. At `intensity=1.0` the maximum velocity deviation is ±32; velocity is clamped to 1–127. Onsets are clamped to tick 0 and the deferred note-off follows the shifted onset, so a note can never end before it starts. Ratchet hits and arp expansions are generative sequences and are not humanized.

### 11.6 Evolve — the Shift-Register Sequence

`evolve(toggle)` (§10.5) runs a 16-bit shift register (the Turing Machine model). The register is initialized from one draw. Per step:

1. The register value maps to a pitch offset in **[-4, +4] semitones** (9 values, via `register % 9 - 4`).
2. The register shifts left; the shifted-out MSB feeds back into the LSB.
3. One draw decides whether the feedback bit is flipped, with probability `toggle`.

`toggle=0.0` locks the sequence into an identical loop; `toggle=1.0` is fully random; `0.05`–`0.20` gives the musically useful slowly-mutating zone. Nonzero offsets are applied after pitch resolution and snapped to the active scale context — `snap_to_scale()` moves each shifted pitch to the *nearest* in-scale pitch for the mode and scale root active at the step's tick.

### 11.7 Arp

`arp(pattern, rate, octaves)` (§10.4) explodes multi-note steps into sequences at emission time, after chord resolution. Defaults: `pattern=up`, `rate=1/8`, `octaves=1`.

- The step's resolved pitches are sorted and deduplicated; `octaves=N` stacks N octave copies above the base tones into one pool.
- `up` ascends the pool; `down` descends from the top; `updown` ascends then descends without repeating top or bottom (e.g. `[C,E,G]` → `C E G E`); `random` is a seeded Fisher–Yates shuffle of the pool, re-shuffled per chord step.
- `rate` is onset-to-onset spacing. The cycle repeats across the step's slots; each slot's duration is `rate × gate`, and the final slot is clamped at the step end. A rate coarser than or equal to the step unit still emits **one clamped slot** (the first cycle tone for the full step) rather than silently swallowing the chord.
- Single-note steps are unchanged. Arp slot on/offs are emitted inline, so a following tie extends nothing.

### 11.8 Euclidean Gating

`euclid_gate(pulses, steps)` (§10.5) computes a Bjorklund distribution of `pulses` across `steps` and cycles that mask across the pattern's step sequence. Masked steps are silenced exactly like rests — prior notes settle, and tie legality is computed from what the unmasked step *would* have been, so gating can never turn a valid tie into an error (or vice versa). No randomness is involved; the same mask applies on every pass.

### 11.9 Reserved for Future Versions

Generative features that are named but not part of the language yet (see §18): `reharmonize()`, `voice_lead()`, user-defined transform functions, independent parameter lanes (Matriceal), and the Arpoly polymetric arpeggiator.

---

[← Back to Table of Contents](00_toc.md)
