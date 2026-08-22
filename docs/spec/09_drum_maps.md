# 9. Drum Maps and Drum Tracks

### 9.1 Drum Map Declaration

```
@drummap [name]
  <identifier> = <midi_note>
  ...
```

`[name]` is optional. If unnamed, the drummap is the default.

**Example:**

```
@drummap kit1
  kick    = 36
  snare   = 38
  hh      = 42
  ohh     = 46
  clap    = 39
  ride    = 51
  crash   = 49
```

If no `@drummap` is declared, the compiler uses the General MIDI percussion map. Default names: `kick` (36), `snare` (38), `clap` (39), `snare_rim` (40), `tom_lo` (41), `hh` (42), `tom_mid` (43), `ohh` (46), `tom_hi` (48), `crash` (49), `ride` (51), `ride_bell` (53), `cowbell` (56), `bongo_hi` (60), `bongo_lo` (61), `conga_hi` (62), `conga_lo` (63).

### 9.2 Drum Track Declaration

```
@track drums
  ch=10
  type=drums
  [drummap=<name>]
  [vel=<int>]
  [shift=<timing>]
  play: ...
```

`type=drums` disables harmony resolution. `follow=` on a drum track is a compiler error.

In drum patterns, step tokens are drummap identifiers:

```
@pattern groove steps=8 unit=1/8
kick
.
snare
.
kick
kick
snare
.
```

All step annotations, simultaneous hits (`+`), subdivisions, and MIDI note numbers are valid in drum patterns.

`[glide]` annotations are ignored on drum tracks with a warning.

---

[← Back to Table of Contents](00_toc.md)
