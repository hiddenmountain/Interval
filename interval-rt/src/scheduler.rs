//! Real-time MIDI event scheduler.
//!
//! Maintains a playback clock based on `@bpm` and `@ppq`. Dispatches events
//! from the event stream to a `midir` output port at the correct wall-clock
//! time. Sleeps in ≤1ms increments between dispatch cycles.
//!
//! Transport controls:
//! - `play()`: begin from tick 0 or current position if paused
//! - `pause()`: halt, retain position, NoteOff all active notes
//! - `stop()`: halt, reset to tick 0, NoteOff all active notes
//!
//! Active note tracking prevents stuck notes across transport changes
//! and hot-swap boundaries.

use crate::hotswap::{CompiledStream, HotSwapSlot};
use crate::playback_state::PlaybackState;
use interval_core::event::MidiEvent;
use midir::MidiOutputConnection;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Transport state of the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportState {
    /// Stopped — tick position reset to 0.
    Stopped,
    /// Playing — actively dispatching events.
    Playing,
    /// Paused — tick position retained, not dispatching.
    Paused,
}

/// Snapshot of the scheduler's playback position, updated each loop iteration.
///
/// Consumers (e.g., a UI) can poll `Scheduler::position_snapshot()` to obtain
/// the current tick/bar/BPM and re-anchor their own position estimation,
/// avoiding accumulated drift from mid-stream tempo changes or hot-swaps.
#[derive(Debug, Clone, Copy)]
pub struct PositionSnapshot {
    pub tick: u64,
    pub bar: u32,
    pub bpm: f64,
    pub ppq: u32,
    pub state: TransportState,
}

impl PositionSnapshot {
    fn initial(ppq: u32, bpm: f64) -> Self {
        Self {
            tick: 0,
            bar: 1,
            bpm,
            ppq,
            state: TransportState::Stopped,
        }
    }
}

/// Hot-swap seek behavior after swapping to a new event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapMode {
    /// Swap at bar boundary, seek to bar start (beat 1).
    Immediate,
    /// Swap at bar boundary, seek to same beat position in the next bar.
    Next,
}

/// An active note tracked by the scheduler: (channel, note).
type ActiveNote = (u8, u8);

/// The real-time MIDI scheduler.
///
/// Owns the playback thread, the current event stream, and the MIDI connection.
/// Transport commands are communicated via atomic flags; the playback loop
/// checks them on each cycle.
pub struct Scheduler {
    /// Signal to request play.
    play_signal: Arc<AtomicBool>,
    /// Signal to request pause.
    pause_signal: Arc<AtomicBool>,
    /// Signal to request stop.
    stop_signal: Arc<AtomicBool>,
    /// Signal to shut down the playback thread.
    shutdown_signal: Arc<AtomicBool>,
    /// The hot-swap slot shared with the background compiler.
    hot_swap: Arc<HotSwapSlot>,
    /// Shared position snapshot, updated each playback loop iteration.
    position: Arc<Mutex<PositionSnapshot>>,
    /// Handle to the playback thread.
    thread_handle: Option<thread::JoinHandle<()>>,
}

impl Scheduler {
    /// Create and start a new scheduler.
    ///
    /// The scheduler begins in the `Stopped` state. Call `play()` to begin
    /// playback.
    ///
    /// # Arguments
    /// * `stream` — the initial compiled event stream
    /// * `conn` — an open MIDI output connection
    /// * `hot_swap` — shared hot-swap slot for live recompilation
    /// * `loop_playback` — if true, wrap to tick 0 at end of stream instead of stopping
    /// * `swap_mode` — hot-swap seek behavior (`Immediate` or `Next`)
    pub fn new(
        stream: CompiledStream,
        conn: MidiOutputConnection,
        hot_swap: Arc<HotSwapSlot>,
        loop_playback: bool,
        swap_mode: SwapMode,
    ) -> Self {
        let play_signal = Arc::new(AtomicBool::new(false));
        let pause_signal = Arc::new(AtomicBool::new(false));
        let stop_signal = Arc::new(AtomicBool::new(false));
        let shutdown_signal = Arc::new(AtomicBool::new(false));
        let position = Arc::new(Mutex::new(PositionSnapshot::initial(
            stream.ppq, stream.bpm,
        )));

        let thread_handle = {
            let play = Arc::clone(&play_signal);
            let pause = Arc::clone(&pause_signal);
            let stop = Arc::clone(&stop_signal);
            let shutdown = Arc::clone(&shutdown_signal);
            let swap = Arc::clone(&hot_swap);
            let pos = Arc::clone(&position);

            thread::spawn(move || {
                playback_loop(
                    stream,
                    conn,
                    play,
                    pause,
                    stop,
                    shutdown,
                    pos,
                    swap,
                    loop_playback,
                    swap_mode,
                );
            })
        };

        Self {
            play_signal,
            pause_signal,
            stop_signal,
            shutdown_signal,
            hot_swap,
            position,
            thread_handle: Some(thread_handle),
        }
    }

    /// Read the current playback position.
    ///
    /// Returns a snapshot of tick, bar, BPM, PPQ, and transport state as of the
    /// most recent playback loop iteration (≤1ms stale while playing).
    pub fn position_snapshot(&self) -> PositionSnapshot {
        self.position
            .lock()
            .map(|g| *g)
            .unwrap_or_else(|poisoned| *poisoned.into_inner())
    }

    /// Clone the shared position handle, for callers that want to poll the
    /// position from a different thread without going through the Scheduler.
    pub fn position_handle(&self) -> Arc<Mutex<PositionSnapshot>> {
        Arc::clone(&self.position)
    }

    /// Begin playback from tick 0 or resume from current position if paused.
    pub fn play(&self) {
        self.play_signal.store(true, Ordering::Release);
    }

    /// Pause playback, retaining position. Sends NoteOff for all active notes.
    pub fn pause(&self) {
        self.pause_signal.store(true, Ordering::Release);
    }

    /// Stop playback, reset to tick 0. Sends NoteOff for all active notes.
    pub fn stop(&self) {
        self.stop_signal.store(true, Ordering::Release);
    }

    /// Stage a new compiled stream for hot-swap.
    pub fn stage(&self, stream: CompiledStream) {
        self.hot_swap.stage(stream);
    }

    /// Shut down the scheduler and join the playback thread.
    pub fn shutdown(mut self) {
        self.shutdown_signal.store(true, Ordering::Release);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Scheduler {
    fn drop(&mut self) {
        self.shutdown_signal.store(true, Ordering::Release);
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }
}

/// The main playback loop. Runs on a dedicated thread.
#[allow(clippy::too_many_arguments)]
fn playback_loop(
    mut stream: CompiledStream,
    mut conn: MidiOutputConnection,
    play_signal: Arc<AtomicBool>,
    pause_signal: Arc<AtomicBool>,
    stop_signal: Arc<AtomicBool>,
    shutdown_signal: Arc<AtomicBool>,
    position: Arc<Mutex<PositionSnapshot>>,
    hot_swap: Arc<HotSwapSlot>,
    loop_playback: bool,
    swap_mode: SwapMode,
) {
    let mut transport = TransportState::Stopped;
    let mut event_cursor: usize = 0;
    let mut current_tick: f64 = 0.0;
    let mut active_notes: HashSet<ActiveNote> = HashSet::new();
    let mut current_bar: u32 = 1;
    let mut just_crossed_bar: bool = false;
    let mut bar_start_tick: u64 = 0;
    let mut last_instant = Instant::now();
    let mut playback_state = PlaybackState::new(0);

    // ticks_per_ms = (ppq * bpm) / 60000
    let mut ticks_per_ms = (stream.ppq as f64 * stream.bpm) / 60_000.0;

    loop {
        // Check shutdown.
        if shutdown_signal.load(Ordering::Acquire) {
            all_notes_off(&mut conn, &mut active_notes);
            reset_controllers(&mut conn);
            break;
        }

        // Check stop signal.
        if stop_signal.swap(false, Ordering::AcqRel) {
            all_notes_off(&mut conn, &mut active_notes);
            reset_controllers(&mut conn);
            transport = TransportState::Stopped;
            current_tick = 0.0;
            event_cursor = 0;
            current_bar = 1;
            playback_state.reset();
            last_instant = Instant::now();
        }

        // Check pause signal.
        if pause_signal.swap(false, Ordering::AcqRel) && transport == TransportState::Playing {
            all_notes_off(&mut conn, &mut active_notes);
            reset_controllers(&mut conn);
            for track in &mut playback_state.tracks {
                track.active_notes.clear();
            }
            transport = TransportState::Paused;
        }

        // Check play signal.
        if play_signal.swap(false, Ordering::AcqRel) && transport != TransportState::Playing {
            if transport == TransportState::Stopped {
                current_tick = 0.0;
                event_cursor = 0;
                current_bar = 1;
                playback_state.reset();
            }
            transport = TransportState::Playing;
            last_instant = Instant::now();
        }

        if transport != TransportState::Playing {
            if let Ok(mut g) = position.lock() {
                *g = PositionSnapshot {
                    tick: current_tick as u64,
                    bar: current_bar,
                    bpm: ticks_per_ms * 60_000.0 / stream.ppq as f64,
                    ppq: stream.ppq,
                    state: transport,
                };
            }
            thread::sleep(Duration::from_millis(1));
            continue;
        }

        // Advance the clock.
        let now = Instant::now();
        let elapsed_ms = now.duration_since(last_instant).as_secs_f64() * 1000.0;
        last_instant = now;
        current_tick += elapsed_ms * ticks_per_ms;
        let tick_pos = current_tick as u64;
        playback_state.global_tick = tick_pos;

        // Dispatch events up to current tick.
        while event_cursor < stream.events.len() {
            let event = &stream.events[event_cursor];
            if event.tick > tick_pos {
                break;
            }

            // Evaluate conditional playback. A conditional NoteOff is never
            // re-evaluated against current state — it plays iff its NoteOn
            // played (tracked per-track), otherwise state changes between the
            // pair (a PatternBoundary, a [pre] update from a later step under
            // swing) would strand the note on or emit an orphan off.
            let should_play = match (&event.condition, &event.event) {
                (None, _) => true,
                (Some(_), MidiEvent::NoteOff { channel, note }) => {
                    playback_state.ensure_track_count(event.track + 1);
                    playback_state.tracks[event.track]
                        .active_notes
                        .contains(&(*channel, *note))
                }
                (Some(cond), _) => {
                    playback_state.ensure_track_count(event.track + 1);
                    let track_state = &playback_state.tracks[event.track];
                    let result = match cond {
                        interval_core::ast::StepCondition::Pre => {
                            track_state.last_conditional_played
                        }
                        other => should_play_condition(other, track_state.pattern_loop_count),
                    };
                    // Update last_conditional_played for subsequent [pre]
                    // evaluation (NoteOn/CC-bearing events only — NoteOffs
                    // are excluded above).
                    playback_state.tracks[event.track].last_conditional_played = result;
                    result
                }
            };

            if should_play {
                dispatch_event(
                    &event.event,
                    event.tick,
                    event.track,
                    &mut conn,
                    &mut active_notes,
                    &mut current_bar,
                    &mut playback_state,
                    &mut ticks_per_ms,
                    stream.ppq,
                    &mut just_crossed_bar,
                    &mut bar_start_tick,
                );
            }
            event_cursor += 1;
        }

        // Check for hot-swap only at bar boundaries for musically clean transitions.
        // Reset flag unconditionally so a mid-bar staged swap waits for the next boundary.
        if just_crossed_bar {
            just_crossed_bar = false;
            if let Some(new_stream) = hot_swap.take() {
                // NoteOff all active before swap.
                all_notes_off(&mut conn, &mut active_notes);

                // Resolve the bar we will land on in the new stream (falling
                // back to bar 1 if the requested bar doesn't exist there),
                // and the seek tick within it.
                let requested_bar = match swap_mode {
                    SwapMode::Immediate => current_bar,
                    SwapMode::Next => current_bar + 1,
                };
                let (target_bar, target_bar_tick) = match new_stream.bar_tick(requested_bar) {
                    Some(tick) => (requested_bar, tick),
                    None => (1, new_stream.bar_tick(1).unwrap_or(0)),
                };
                let seek_tick = match swap_mode {
                    SwapMode::Immediate => target_bar_tick,
                    SwapMode::Next => {
                        // Same beat position within the target bar, but only
                        // when we actually landed on the requested next bar.
                        if target_bar == requested_bar {
                            let offset = (current_tick as u64).saturating_sub(bar_start_tick);
                            target_bar_tick + offset
                        } else {
                            target_bar_tick
                        }
                    }
                };

                // Transfer playback state, comparing pattern identity at the
                // bar we actually seek to (not a bar that may not exist).
                let new_pattern_names = extract_pattern_names_at_bar(&new_stream, target_bar);
                for (track_idx, track_state) in playback_state.tracks.iter_mut().enumerate() {
                    let new_name = new_pattern_names.get(&track_idx).map(|s| s.as_str());
                    track_state.transfer_for_hot_swap(new_name);
                    track_state.active_notes.clear();
                }

                event_cursor = new_stream.event_index_at_tick(seek_tick);
                current_tick = seek_tick as f64;
                // Set bar state explicitly: with a mid-bar beat offset the
                // cursor lands past the target bar's BarMarker, so we cannot
                // rely on re-dispatching it to update these.
                current_bar = target_bar;
                bar_start_tick = target_bar_tick;
                // Adopt the tempo in effect at the seek position, not the
                // stream's initial BPM — a seek into a later @bpm segment
                // must not revert the tempo until the next Tempo event.
                let effective_bpm = new_stream.events[..event_cursor]
                    .iter()
                    .rev()
                    .find_map(|e| match e.event {
                        MidiEvent::Tempo { bpm } => Some(bpm),
                        _ => None,
                    })
                    .unwrap_or(new_stream.bpm);
                ticks_per_ms = (new_stream.ppq as f64 * effective_bpm) / 60_000.0;
                stream = new_stream;
                last_instant = Instant::now();
            }
        }

        // Check if we've reached the end of the stream.
        // Wait until current_tick reaches total_ticks (end of last bar) so trailing
        // silence plays out fully before looping — prevents the loop from starting early.
        let stream_end = stream.total_ticks();
        if event_cursor >= stream.events.len()
            && (current_tick as u64) >= stream_end
            && stream_end > 0
        {
            all_notes_off(&mut conn, &mut active_notes);
            if loop_playback {
                // Wrap to beginning. Don't reset pattern_loop_count or
                // prev_pitches — conditionals should evolve across loops
                // and voice leading should carry over. Per-track active_notes
                // are cleared to mirror the all_notes_off above.
                for track in &mut playback_state.tracks {
                    track.active_notes.clear();
                }
                current_tick = 0.0;
                event_cursor = 0;
                current_bar = 1;
                last_instant = Instant::now();
            } else {
                transport = TransportState::Stopped;
                current_tick = 0.0;
                event_cursor = 0;
                current_bar = 1;
                playback_state.reset();
            }
        }

        // Publish position snapshot for consumers (e.g., UI re-anchoring).
        // ticks_per_ms = (ppq * bpm) / 60_000 → bpm = ticks_per_ms * 60_000 / ppq
        let bpm = ticks_per_ms * 60_000.0 / stream.ppq as f64;
        if let Ok(mut g) = position.lock() {
            *g = PositionSnapshot {
                tick: current_tick as u64,
                bar: current_bar,
                bpm,
                ppq: stream.ppq,
                state: transport,
            };
        }

        // Sleep ~1ms to avoid busy-waiting.
        thread::sleep(Duration::from_millis(1));
    }
}

/// Dispatch a single event to the MIDI output.
#[allow(clippy::too_many_arguments)]
fn dispatch_event(
    event: &MidiEvent,
    event_tick: u64,
    event_track: usize,
    conn: &mut MidiOutputConnection,
    active_notes: &mut HashSet<ActiveNote>,
    current_bar: &mut u32,
    playback_state: &mut PlaybackState,
    ticks_per_ms: &mut f64,
    ppq: u32,
    just_crossed_bar: &mut bool,
    bar_start_tick: &mut u64,
) {
    match event {
        MidiEvent::NoteOn {
            channel,
            note,
            velocity,
        } => {
            let msg = [0x90 | channel, *note, *velocity];
            let _ = conn.send(&msg);
            active_notes.insert((*channel, *note));
            // Also track in per-track state.
            playback_state.ensure_track_count(event_track + 1);
            playback_state.tracks[event_track]
                .active_notes
                .insert((*channel, *note));
        }
        MidiEvent::NoteOff { channel, note } => {
            let msg = [0x80 | channel, *note, 0];
            let _ = conn.send(&msg);
            active_notes.remove(&(*channel, *note));
            playback_state.ensure_track_count(event_track + 1);
            playback_state.tracks[event_track]
                .active_notes
                .remove(&(*channel, *note));
        }
        MidiEvent::CC {
            channel,
            controller,
            value,
        } => {
            let msg = [0xB0 | channel, *controller, *value];
            let _ = conn.send(&msg);
        }
        MidiEvent::ProgramChange { channel, program } => {
            let msg = [0xC0 | channel, *program];
            let _ = conn.send(&msg);
        }
        MidiEvent::PitchBend { channel, value } => {
            // Convert signed i16 (-8192..8191) to unsigned 14-bit (0..16383).
            let unsigned = (*value as i32 + 8192) as u16;
            let lsb = (unsigned & 0x7F) as u8;
            let msb = ((unsigned >> 7) & 0x7F) as u8;
            let msg = [0xE0 | channel, lsb, msb];
            let _ = conn.send(&msg);
        }
        MidiEvent::Aftertouch { channel, value } => {
            let msg = [0xD0 | channel, *value];
            let _ = conn.send(&msg);
        }
        MidiEvent::BarMarker { bar } => {
            *current_bar = *bar;
            *just_crossed_bar = true;
            *bar_start_tick = event_tick;
            // BarMarkers are not sent to MIDI output.
        }
        MidiEvent::PatternBoundary {
            track,
            pattern_name,
        } => {
            playback_state.ensure_track_count(*track + 1);
            playback_state.tracks[*track].on_pattern_boundary(pattern_name);
            // PatternBoundary events are not sent to MIDI output.
        }
        MidiEvent::Tempo { bpm } => {
            // Update the real-time tick rate so mid-stream BPM changes take effect.
            *ticks_per_ms = (ppq as f64 * bpm) / 60_000.0;
        }
        MidiEvent::TimeSignature { .. }
        | MidiEvent::TrackName { .. }
        | MidiEvent::TextMeta { .. } => {
            // Meta events are not sent to MIDI output.
        }
    }
}

/// Evaluate whether a conditional step should play given the current loop count.
fn should_play_condition(cond: &interval_core::ast::StepCondition, loop_count: u32) -> bool {
    use interval_core::ast::StepCondition;
    match cond {
        StepCondition::Every(n) => loop_count.is_multiple_of(*n),
        StepCondition::Cond(x, y) => loop_count % y == (x - 1),
        StepCondition::Once => loop_count == 0,
        StepCondition::Pre => {
            // Pre is handled at the call site — the last_conditional_played
            // flag is checked there. Here we just return false as a default.
            // The actual evaluation happens before this function is called.
            false
        }
    }
}

/// Extract pattern names from the new stream near the given bar for hot-swap
/// identity comparison. Returns a map of track_index → pattern_name.
fn extract_pattern_names_at_bar(
    stream: &CompiledStream,
    bar: u32,
) -> std::collections::HashMap<usize, String> {
    use std::collections::HashMap;
    let mut result: HashMap<usize, String> = HashMap::new();

    // Look for PatternBoundary events near the target bar.
    // We scan the entire stream for the most recent PatternBoundary per track
    // that occurs at or before the bar's tick position.
    let bar_tick = stream.bar_tick(bar).unwrap_or(0);

    for event in &stream.events {
        if event.tick > bar_tick {
            break;
        }
        if let MidiEvent::PatternBoundary {
            track,
            pattern_name,
        } = &event.event
        {
            result.insert(*track, pattern_name.clone());
        }
    }

    result
}

/// Send NoteOff for all currently active notes and clear the set.
fn all_notes_off(conn: &mut MidiOutputConnection, active_notes: &mut HashSet<ActiveNote>) {
    for &(channel, note) in active_notes.iter() {
        let msg = [0x80 | channel, note, 0];
        let _ = conn.send(&msg);
    }
    active_notes.clear();
}

/// Reset sustain and release any notes the per-note NoteOff pass couldn't
/// know about: CC 64 (sustain) → 0 and CC 123 (All Notes Off) on every
/// channel. Sent on stop/pause/shutdown — a synth holding CC64 would
/// otherwise keep ringing after every note was individually released.
fn reset_controllers(conn: &mut MidiOutputConnection) {
    for channel in 0u8..16 {
        let _ = conn.send(&[0xB0 | channel, 64, 0]);
        let _ = conn.send(&[0xB0 | channel, 123, 0]);
    }
}

/// Build MIDI bytes for a NoteOff message (used by tests).
#[cfg(test)]
pub(crate) fn note_off_bytes(channel: u8, note: u8) -> [u8; 3] {
    [0x80 | channel, note, 0]
}

/// Build MIDI bytes for a NoteOn message (used by tests).
#[cfg(test)]
pub(crate) fn note_on_bytes(channel: u8, note: u8, velocity: u8) -> [u8; 3] {
    [0x90 | channel, note, velocity]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_on_bytes() {
        // Channel 0, note 60, velocity 100
        assert_eq!(note_on_bytes(0, 60, 100), [0x90, 60, 100]);
        // Channel 9 (drums), note 36, velocity 127
        assert_eq!(note_on_bytes(9, 36, 127), [0x99, 36, 127]);
    }

    #[test]
    fn test_note_off_bytes() {
        assert_eq!(note_off_bytes(0, 60), [0x80, 60, 0]);
        assert_eq!(note_off_bytes(9, 36), [0x89, 36, 0]);
    }

    #[test]
    fn test_pitch_bend_encoding() {
        // Center (0) → unsigned 8192 → LSB=0, MSB=64
        let value: i16 = 0;
        let unsigned = (value as i32 + 8192) as u16;
        let lsb = (unsigned & 0x7F) as u8;
        let msb = ((unsigned >> 7) & 0x7F) as u8;
        assert_eq!(lsb, 0);
        assert_eq!(msb, 64);

        // Max (+8191) → unsigned 16383 → LSB=127, MSB=127
        let value: i16 = 8191;
        let unsigned = (value as i32 + 8192) as u16;
        let lsb = (unsigned & 0x7F) as u8;
        let msb = ((unsigned >> 7) & 0x7F) as u8;
        assert_eq!(lsb, 127);
        assert_eq!(msb, 127);

        // Min (-8192) → unsigned 0 → LSB=0, MSB=0
        let value: i16 = -8192;
        let unsigned = (value as i32 + 8192) as u16;
        let lsb = (unsigned & 0x7F) as u8;
        let msb = ((unsigned >> 7) & 0x7F) as u8;
        assert_eq!(lsb, 0);
        assert_eq!(msb, 0);
    }

    #[test]
    fn test_active_note_tracking() {
        let mut active: HashSet<ActiveNote> = HashSet::new();
        active.insert((0, 60));
        active.insert((0, 64));
        active.insert((1, 60));

        assert_eq!(active.len(), 3);

        // Remove one.
        active.remove(&(0, 60));
        assert_eq!(active.len(), 2);
        assert!(!active.contains(&(0, 60)));
        assert!(active.contains(&(0, 64)));
        assert!(active.contains(&(1, 60)));
    }

    #[test]
    fn test_cc_bytes() {
        // CC on channel 0, controller 64 (sustain), value 127
        let channel: u8 = 0;
        let controller: u8 = 64;
        let value: u8 = 127;
        let msg = [0xB0 | channel, controller, value];
        assert_eq!(msg, [0xB0, 64, 127]);
    }

    #[test]
    fn test_program_change_bytes() {
        let channel: u8 = 0;
        let program: u8 = 48;
        let msg = [0xC0 | channel, program];
        assert_eq!(msg, [0xC0, 48]);
    }

    #[test]
    fn test_aftertouch_bytes() {
        let channel: u8 = 0;
        let value: u8 = 80;
        let msg = [0xD0 | channel, value];
        assert_eq!(msg, [0xD0, 80]);
    }

    #[test]
    fn test_first_pass_conditionals_match_smf_static_eval() {
        use interval_core::ast::StepCondition;
        // The SMF renderer statically evaluates at loop_count = 0:
        // Once → plays, Every(_) → plays, Cond(1, _) → plays, Cond(x>1, _)
        // → doesn't. The RT scheduler must agree on the very first pass.
        let first_pass = 0;
        assert!(should_play_condition(&StepCondition::Once, first_pass));
        assert!(should_play_condition(&StepCondition::Every(4), first_pass));
        assert!(should_play_condition(
            &StepCondition::Cond(1, 4),
            first_pass
        ));
        assert!(!should_play_condition(
            &StepCondition::Cond(2, 4),
            first_pass
        ));

        // [once] never fires again; [every:4] fires on passes 1, 5, 9
        // (counts 0, 4, 8); [cond:2:4] fires on pass 2 (count 1).
        assert!(!should_play_condition(&StepCondition::Once, 1));
        assert!(!should_play_condition(&StepCondition::Every(4), 1));
        assert!(should_play_condition(&StepCondition::Every(4), 4));
        assert!(should_play_condition(&StepCondition::Cond(2, 4), 1));
    }

    #[test]
    fn test_ticks_per_ms() {
        // 480 PPQ, 120 BPM: ticks_per_ms = (480 * 120) / 60000 = 0.96
        let tpms: f64 = (480.0 * 120.0) / 60_000.0;
        assert!((tpms - 0.96).abs() < 1e-10);

        // 480 PPQ, 60 BPM: ticks_per_ms = (480 * 60) / 60000 = 0.48
        let tpms: f64 = (480.0 * 60.0) / 60_000.0;
        assert!((tpms - 0.48).abs() < 1e-10);
    }
}
