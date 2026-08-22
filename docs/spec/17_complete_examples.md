# 17. Complete Examples

### 17.1 Minimal File (Auto-Inference)

```
@ppq 480
@bpm 120
@ts 4/4

@scale root=C mode=major

// Single unnamed harmony block — follow= is auto-inferred for all tracks
@harmony
I | IV | V | I

// $chord and %n work without explicit follow=
@pattern comp unit=1/4
$chord[vel:80]
.
$chord[vel:60]
.

@track piano ch=1 prog=1
play: comp * 4
```

### 17.2 Jazz Walking Bass

```
@ppq 480
@bpm 138
@ts 4/4
@title "Autumn Walk"

@scale root=C mode=major

@harmony main
Imaj7 | VIm7 | IIm7:2 V7:2 | Imaj7
Imaj7 | VIm7 | IIm7:2 V7:2 | Imaj7
IVmaj7 | IVm7 bVII7 | Imaj7:2 VI7:2 | IIm7:2 V7:2
Imaj7 | VIm7 | IIm7:2 V7:2 | Imaj7

@pattern wb unit=1/4
%1
%2
%3
%4

@track bass
  ch=2 prog=32 oct=2 vel=88 gate=0.85
  follow=main
  play: wb * 16
```

### 17.3 Swinging Jazz Trio

```
@ppq 480
@bpm 140
@ts 4/4
@title "Medium Swing"

@scale root=Bb mode=major

@harmony changes
Imaj7 | IVm7 bVII7 | Imaj7 | VIm7 IIm7

@pattern comp unit=1/8
.
%1+%2+%4[vel:78]
.
%2+%3+%5[vel:70 shift:+3%]
%1+%4[vel:72]
.
.
%2+%3+%5[shift:-2% vel:68]

@pattern wb unit=1/4
%1
%3
%4
%2

@drummap kit
  kick  = 36
  snare = 38
  hh    = 42
  ride  = 51

@pattern ride_pat unit=1/8
ride
.
ride
.
ride
.
ride
.

@track piano
  ch=1 prog=1 oct=4
  follow=changes
  voice=drop2 inv=auto
  swing=0.67 swingunit=1/8
  play: comp -> vary(0.25) -> humanize(6ms, 0.5) * 4

@track bass
  ch=2 prog=32 oct=2 vel=90 gate=0.88
  follow=changes
  swing=0.67 swingunit=1/8
  play: wb * 4

@track drums
  ch=10 type=drums drummap=kit vel=95
  swing=0.62 swingunit=1/8
  play: ride_pat * 4
```

### 17.4 Arpeggiated Chords with Chord Ordinals

```
@ppq 480
@bpm 100
@ts 4/4
@seed 7

@scale root=D mode=dorian

@harmony pads
Im7 | IVm7 | bVII7 | Im7

// Use %n to select specific chord tones for arpeggio
@pattern arp_template unit=1/4
%1+%2+%3+%4
~
~
~

@pattern bass_line unit=1/4
%1
%1
%3
%4

@track keys
  ch=1 prog=88 oct=4
  follow=pads
  // Explode chords into 8th-note upward arpeggios
  play: arp_template -> arp(pattern=up, rate=1/8, octaves=2) * 4

@track bass
  ch=2 prog=34 oct=2 vel=80 gate=0.9
  follow=pads
  play: bass_line * 4
```

### 17.5 Evolving Ambient with BPM Ramp

```
@ppq 480
@bpm 72 * 8 | 72->90 ramp=ease_in * 4 | 90
@ts 4/4
@seed 42

@scale root=D mode=dorian

@harmony pad
Im7 | IVm7 | Im7 | bVII7

@pattern arp unit=1/16
%1
%2
%3
%4
%3
%2
%1
%4
%3
%4
%3
%2
%1
%2
%3
%1

@track keys
  ch=1 prog=88 oct=5
  follow=pad
  play: arp -> evolve(0.1) -> vel_curve(wave=sine, min=40, max=100) -> echo(1/8, 2, 0.5) * 4

@track bass
  ch=2 prog=38 oct=2 vel=70 gate=0.95
  follow=pad
  steps:
    %1
    ~
    ~
    ~
```

### 17.6 Conditional Drum Pattern with Probability

```
@ppq 480
@bpm 110
@ts 4/4

@drummap kit
  kick  = 36
  snare = 38
  hh    = 42
  ohh   = 46

@pattern beat unit=1/16
kick+hh
hh
hh
hh
snare+hh
hh
hh
hh
kick+hh
hh
kick[every:2]
hh
snare+ohh[cond:3:4]
hh[prob:0.5]
hh[pre]
hh

@track drums
  ch=10 type=drums drummap=kit vel=95
  play: beat * 8
```

### 17.7 Multi-Harmony with Named Blocks and Diatonic Inference

```
@ppq 480
@bpm 120
@ts 4/4

// @scale timeline: 8 bars in C major, then modulate to A minor
@scale root=C mode=major * 8 | root=A mode=minor

@harmony main
// Diatonic inference in C major — no quality suffixes needed
I | vi | IV | V
I | vi | ii | V
// Bars 9-12 resolve in A minor context
i | bVI | bIII | bVII

@harmony ostinato
I | I | I | I

@pattern melody unit=1/8
^1
^2
^3
^5
^3
^2
^1
.

@track lead
  ch=1 prog=73 oct=5
  follow=main
  play: melody * 3

@track accompaniment
  ch=2 prog=48 oct=3 inv=auto voice=open
  follow=ostinato
  steps:
    $chord[vel:60]
    ~
    ~
    ~
```

### 17.8 Inline Patterns with Sharp Root

```
@ppq 480
@bpm 92
@ts 4/4

@scale root=F# mode=minor

@harmony
i | bVI | bIII | bVII

// Inline patterns — steps= inferred from token count
@pattern arp unit=1/8: %1 %2 %3 %4 %3 %2 %1 .

// Multi-line pattern — steps= inferred from body line count
@pattern bass unit=1/4
%1
%1
%3
%4

@track keys ch=1 prog=4 oct=5
play: arp * 4

@track bass ch=2 prog=33 oct=2 vel=85 gate=0.9
play: bass * 4
```

---

[← Back to Table of Contents](00_toc.md)
