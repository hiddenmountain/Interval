# 14. Output Targets

The compiler's final product is a shared internal event stream. Two output targets consume this stream independently.

### 14.1 Event Stream Type

```rust
enum MidiEvent {
    NoteOn        { channel: u8, note: u8, velocity: u8 },
    NoteOff       { channel: u8, note: u8 },
    CC            { channel: u8, controller: u8, value: u8 },
    ProgramChange { channel: u8, program: u8 },
    PitchBend     { channel: u8, value: i16 },
    Aftertouch    { channel: u8, value: u8 },
    Tempo         { bpm: f64 },
    TimeSignature { numerator: u8, denominator: u8 },
    TrackName     { name: String },
    TextMeta      { text: String },
    BarMarker     { bar: u32 },
    PatternBoundary { track: usize, pattern_name: String },
}

struct TimedEvent {
    tick:       u64,
    track:      usize,
    event:      MidiEvent,
    condition:  Option<StepCondition>,
    step_index: Option<usize>,   // originating step within the resolved pattern
}

type EventStream = Vec<TimedEvent>;
```

`BarMarker` events are emitted once per bar per track at the bar's first step tick. They exist exclusively for the RT scheduler's hot-swap mechanism and are stripped by the SMF renderer.

`PatternBoundary` events mark where pattern instances begin, enabling the RT scheduler to track loop counts for conditional steps. Stripped by the SMF renderer.

`TextMeta` events carry metadata (e.g., `seed:<N>`). Written to SMF as text meta-events. Ignored by RT scheduler.

### 14.2 SMF Renderer

Consumes the event stream and writes a Type 1 Standard MIDI File.

- Track 0: tempo track. Contains `Tempo`, `TimeSignature`, `TrackName`, and `TextMeta` events.
- Tracks 1–N: one SMF track per `@track` declaration.

`BarMarker` and `PatternBoundary` events are stripped. All other event types map to SMF equivalents.

**Event ordering within a tick:** NoteOff → ProgramChange → CC → PitchBend → Aftertouch → NoteOn.

### 14.3 RT Scheduler

Consumes the event stream and sends events to a MIDI output port in real time. Supports playback, hot-swap on recompile, continuous looping, and conditional step evaluation.

**PlaybackState:**

```rust
struct PlaybackState {
    global_tick: u64,
    tracks: Vec<TrackState>,
}

struct TrackState {
    sequence_position: usize,
    pattern_loop_count: u32,
    current_pattern_name: Option<String>,
    last_conditional_played: bool,
    active_notes: HashSet<(u8, u8)>,
    rate_adjusted_tick: u64,
}
```

`PatternBoundary` events fire at the *start* of each pattern instance. The first boundary for a pattern begins pass 0 — `pattern_loop_count == 0` during the entire first pass — and each subsequent boundary for the same pattern increments the counter. A boundary for a different pattern resets the counter to 0. This matches the SMF renderer's static evaluation (loop 0), so `[once]` plays on the first pass in both outputs. Conditional step annotations are evaluated against this counter.

#### 14.3.1 Continuous Looping

In `play` mode, the scheduler continuously loops the arrangement. When the event cursor reaches the end of the event stream, it wraps to tick 0 and continues. In `compile` mode, the arrangement is rendered as finite for export.

**State at the wrap point:**

- `pattern_loop_count` — **not reset**. Conditional steps like `[every:4]` evolve across arrangement loops.
- Voice-leading state (`prev_pitches`) — **carries over**. Resetting would produce a voicing jump at the loop point.
- `active_notes` — **cleared** via `all_notes_off` before wrap to prevent stuck notes.
- Harmony timeline — already cyclic (`tick % total_ticks`), no change needed.

#### 14.3.2 Hot-Swap

1. New event stream compiled in background thread, staged via `arc-swap`.
2. At bar boundaries only (when a `BarMarker` event is dispatched), the scheduler checks for a staged stream. The `just_crossed_bar` flag is edge-triggered: set on `BarMarker` dispatch and reset immediately on the next scheduler cycle, regardless of whether a swap occurs. This ensures swaps never happen mid-bar — a swap staged between bar boundaries waits for the next `BarMarker`.
3. PlaybackState transfers: preserve `pattern_loop_count` if pattern identity unchanged, reset otherwise. Always emit NoteOff for active notes.
4. File-watch debounce is 30ms (sufficient for atomic editor writes).

#### 14.3.3 Swap Modes

Two swap modes control where playback resumes after a hot-swap. Exposed as `--swap-mode=<mode>` CLI flag.

| Mode | Behavior | Best for |
|------|----------|----------|
| `immediate` (default) | Swap at the next bar boundary, seek to beat 1 of that bar | Hearing the new version from the top of the bar |
| `next` | Swap at the next bar boundary, seek to the same beat position in the *next* bar | Continuous editing without positional resets |

**`immediate` mode:** On swap, seek to the start of the current bar in the new stream. The composer hears the edited bar from beat 1. Minimal latency between save and hearing the result.

**`next` mode:** On swap, compute `offset = current_tick - bar_start_tick`. Seek to the start of the next bar in the new stream plus the offset. Playback never repeats or rewinds — the music "catches up" seamlessly. Worst-case latency is almost one full bar before the edit is audible.

**Edge cases:**

- If the target bar doesn't exist in the new stream (structural change), fall back to bar 1.
- `bar_start_tick` is tracked per `BarMarker` dispatch for accurate beat-offset calculation.

**Transport:** `play()`, `pause()`, `stop()`. Pause/stop emit NoteOff for all active notes.

### 14.4 Crate Structure

```
Interval/
├── interval-core/    // lexer, parser, AST, compiler, introspect → EventStream (WASM-safe)
├── interval-smf/     // SMF renderer (WASM-safe)
├── interval-rt/      // RT scheduler (native-only, midir + arc-swap)
└── interval-cli/     // CLI tool (compile, play, check, dump)
```

`interval-core` and `interval-smf` compile to `wasm32-unknown-unknown` without modification. `interval-rt` is native-only due to `midir`.

---

[← Back to Table of Contents](00_toc.md)
