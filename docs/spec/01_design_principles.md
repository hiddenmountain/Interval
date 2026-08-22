# 1. Design Principles

1. **Harmony is the root layer.** All degree-relative notation resolves against a shared harmonic context. Tracks and patterns are subordinate to the harmony timeline.
2. **Three layers, strictly ordered.** Harmony → Patterns → Tracks. No layer reaches upward.
3. **Deterministic output.** Given identical input and identical seeds, the compiler always produces identical MIDI. All controlled variation is seeded.
4. **Opt-out, not opt-in.** Absolute pitch notation opts out of harmony resolution. No special declaration needed.
5. **Defaults reduce verbosity.** A file with one harmony block, one pattern, and one track should be short to write. When exactly one harmony block exists, `follow=` is auto-inferred.
6. **Human-readable and LLM-efficient.** The language should be parseable by eye and by model without a separate toolchain.
7. **Transformations are composable.** The transformation pipeline is a first-class citizen, not a post-processing step.
8. **MIDI fidelity.** Every construct has a precise, unambiguous MIDI representation. No assumptions are made about playback context.
9. **Harmony follows time, not patterns.** Harmony resolution always happens at the real tick position of each event, regardless of how the pattern was stretched, rate-modified, transformed, or looped. Patterns do not own their harmonic context — time does.

---

[← Back to Table of Contents](00_toc.md)
