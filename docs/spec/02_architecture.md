# 2. Architecture Overview

```
┌─────────────────────────────────────────┐
│           GLOBAL HEADER                 │
│  @ppq  @bpm  @ts  @title  @seed        │
└─────────────────────────────────────────┘
                    │
┌─────────────────────────────────────────┐
│          TONAL CONTEXT                  │
│  @scale root=  mode=  [timeline form]  │
└─────────────────────────────────────────┘
                    │
┌─────────────────────────────────────────┐
│         HARMONY TIMELINE(S)             │
│  @harmony [name]  play=  inv=           │
│  Chord symbols, Roman numerals, bar grid│
└─────────────────────────────────────────┘
                    │
┌─────────────────────────────────────────┐
│              PATTERNS                   │
│  @pattern <name>  steps=  unit=         │
│  Step lines, degree tokens, transforms  │
└─────────────────────────────────────────┘
                    │
┌─────────────────────────────────────────┐
│         DRUM MAPS (optional)            │
│  @drummap <name>  identifier=note       │
└─────────────────────────────────────────┘
                    │
┌─────────────────────────────────────────┐
│               TRACKS                    │
│  @track <name>  ch=  prog=  follow=     │
│  play: or steps: directive              │
└─────────────────────────────────────────┘
                    │
              MIDI OUTPUT
```

Each layer is defined in sequence in the file. Forward references (a track referencing a pattern defined later) are **not** permitted.

### 2.1 Tonal Inheritance Hierarchy

```
@scale → @harmony → @track
```

Each level inherits from the one above and can override for its scope:

- `@scale` sets the global `root` and `mode`. May be declared as a scalar or timeline (see §5).
- `@harmony` inherits `root` and `mode` from `@scale`. Roman numerals resolve via `@scale root=`. `section:` directives can modulate root within the harmony timeline (deprecated — prefer `@scale` timeline form).
- `@track` with `mode=` overrides the inherited mode for degree resolution on that track only. Chord context still comes from `follow=`. This is musically valid — a soloist playing Lydian over a major harmony.

If `@scale` is absent, the compiler falls back to C major with a warning.

### 2.2 Auto-Inference of Follow

When exactly one harmony block exists in the file (named or unnamed), every track automatically follows it without requiring an explicit `follow=` directive. `$chord` and `%n` tokens also resolve in this case without `follow=`. Explicit `follow=` is still accepted and takes precedence.

---

[← Back to Table of Contents](00_toc.md)
