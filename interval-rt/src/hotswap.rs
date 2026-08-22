//! Hot-swap mechanism for live recompilation.
//!
//! When the source file changes and a new event stream is compiled:
//! 1. The new stream is staged via `arc-swap` (lock-free atomic swap).
//! 2. The scheduler detects the staged stream at the next `BarMarker`.
//! 3. At that bar boundary, the scheduler swaps to the new stream and
//!    seeks to the equivalent bar position.
//! 4. If the new stream is shorter, wraps to bar 1.
//! 5. NoteOff is emitted for all active notes before the swap.

use arc_swap::ArcSwap;
use interval_core::event::{EventStream, MidiEvent};
use std::sync::Arc;

/// A compiled event stream with pre-computed bar index for fast seeking.
#[derive(Debug, Clone)]
pub struct CompiledStream {
    /// The event stream (sorted by tick).
    pub events: EventStream,
    /// PPQ value for this stream.
    pub ppq: u32,
    /// BPM value for this stream.
    pub bpm: f64,
    /// Pre-computed bar-to-tick index: `bar_ticks[i]` is the tick of bar `i+1`,
    /// or `None` for a bar number no marker was emitted for. Bar 1 legitimately
    /// starts at tick 0, so absence must be represented explicitly — a `0`
    /// sentinel would fabricate phantom bars at tick 0.
    bar_ticks: Vec<Option<u64>>,
}

impl CompiledStream {
    /// Create a new `CompiledStream` from an event stream.
    pub fn new(events: EventStream, ppq: u32, bpm: f64) -> Self {
        let mut bar_ticks: Vec<Option<u64>> = Vec::new();
        for event in &events {
            if let MidiEvent::BarMarker { bar } = &event.event {
                let idx = (*bar as usize).saturating_sub(1);
                // Ensure vec is large enough; skipped bars stay None.
                if bar_ticks.len() <= idx {
                    bar_ticks.resize(idx + 1, None);
                }
                // Use the first occurrence (lowest tick) for each bar —
                // BarMarkers are emitted per track, so each bar appears once
                // per track.
                if bar_ticks[idx].is_none_or(|t| event.tick < t) {
                    bar_ticks[idx] = Some(event.tick);
                }
            }
            // PatternBoundary events are preserved in the stream for the
            // scheduler to process; no separate index is needed.
        }
        Self {
            events,
            ppq,
            bpm,
            bar_ticks,
        }
    }

    /// Find the tick position corresponding to a given bar number (1-indexed).
    /// Returns `None` if no marker for that bar exists in this stream.
    pub fn bar_tick(&self, bar: u32) -> Option<u64> {
        let idx = (bar as usize).checked_sub(1)?;
        self.bar_ticks.get(idx).copied().flatten()
    }

    /// Find the event index corresponding to a given tick position.
    /// Returns the index of the first event at or after the given tick.
    pub fn event_index_at_tick(&self, tick: u64) -> usize {
        self.events.partition_point(|e| e.tick < tick)
    }

    /// Total number of bars in this stream.
    pub fn bar_count(&self) -> u32 {
        self.bar_ticks.len() as u32
    }

    /// Total duration of this stream in ticks.
    ///
    /// If bar markers exist, this returns the tick at which the last bar would end
    /// (estimated from bar spacing). Otherwise falls back to the last event's tick.
    /// This ensures the scheduler waits for the full last bar to elapse before looping,
    /// rather than cutting off silence at the end.
    pub fn total_ticks(&self) -> u64 {
        // Consider only bars a marker actually exists for.
        let mut known = self.bar_ticks.iter().flatten().copied();
        let (first, last) = match (known.next(), known.next_back()) {
            (Some(f), Some(l)) => (f, l),
            (Some(f), None) => {
                // Single known bar: assume ticks_per_bar = ppq * 4 (for 4/4).
                return f + self.ppq as u64 * 4;
            }
            _ => {
                // No bar markers: use last event tick.
                return self.events.last().map(|e| e.tick).unwrap_or(0);
            }
        };
        // Estimate the final bar's length from the spacing of the last two
        // known bars (assumes constant meter at the end of the piece).
        let second_last = self
            .bar_ticks
            .iter()
            .flatten()
            .copied()
            .rfind(|&t| t < last)
            .unwrap_or(first);
        let bar_len = last.saturating_sub(second_last).max(1);
        last + bar_len
    }
}

/// The hot-swap staging slot.
///
/// The background compiler thread stores a newly compiled stream here.
/// The scheduler thread checks for a staged stream at each bar boundary.
pub struct HotSwapSlot {
    staged: ArcSwap<Option<CompiledStream>>,
}

impl HotSwapSlot {
    /// Create a new empty hot-swap slot.
    pub fn new() -> Self {
        Self {
            staged: ArcSwap::from_pointee(None),
        }
    }

    /// Stage a new compiled stream for hot-swap.
    /// Called by the background compilation thread.
    pub fn stage(&self, stream: CompiledStream) {
        self.staged.store(Arc::new(Some(stream)));
    }

    /// Take the staged stream if one is available.
    /// Returns `None` if no stream has been staged since the last take.
    /// Called by the scheduler at bar boundaries.
    pub fn take(&self) -> Option<CompiledStream> {
        let current = self.staged.swap(Arc::new(None));
        // If a reader still holds a strong reference (e.g. a concurrent
        // `load_full`), clone instead of unwrapping — silently dropping the
        // staged stream would lose the user's edit with no diagnostic.
        match Arc::try_unwrap(current) {
            Ok(inner) => inner,
            Err(shared) => (*shared).clone(),
        }
    }

    /// Check if a stream is staged without consuming it.
    pub fn has_staged(&self) -> bool {
        self.staged.load().is_some()
    }
}

impl Default for HotSwapSlot {
    fn default() -> Self {
        Self::new()
    }
}

/// Determine the seek position in a new stream given the current bar.
///
/// If the new stream contains the current bar, returns its tick.
/// Otherwise wraps to bar 1.
pub fn seek_bar_in_new_stream(new_stream: &CompiledStream, current_bar: u32) -> u64 {
    if let Some(tick) = new_stream.bar_tick(current_bar) {
        tick
    } else {
        new_stream.bar_tick(1).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interval_core::event::{MidiEvent, TimedEvent};

    fn make_bar_event(tick: u64, bar: u32) -> TimedEvent {
        TimedEvent {
            tick,
            track: 1,
            event: MidiEvent::BarMarker { bar },
            condition: None,
            step_index: None,
        }
    }

    fn make_note_event(tick: u64, note: u8) -> TimedEvent {
        TimedEvent {
            tick,
            track: 1,
            event: MidiEvent::NoteOn {
                channel: 0,
                note,
                velocity: 100,
            },
            condition: None,
            step_index: None,
        }
    }

    #[test]
    fn test_compiled_stream_bar_index() {
        let events = vec![
            make_bar_event(0, 1),
            make_note_event(0, 60),
            make_bar_event(1920, 2),
            make_note_event(1920, 64),
            make_bar_event(3840, 3),
            make_note_event(3840, 67),
        ];
        let stream = CompiledStream::new(events, 480, 120.0);

        assert_eq!(stream.bar_count(), 3);
        assert_eq!(stream.bar_tick(1), Some(0));
        assert_eq!(stream.bar_tick(2), Some(1920));
        assert_eq!(stream.bar_tick(3), Some(3840));
        assert_eq!(stream.bar_tick(4), None);
    }

    #[test]
    fn test_event_index_at_tick() {
        let events = vec![
            make_bar_event(0, 1),
            make_note_event(0, 60),
            make_note_event(480, 64),
            make_note_event(960, 67),
        ];
        let stream = CompiledStream::new(events, 480, 120.0);

        assert_eq!(stream.event_index_at_tick(0), 0);
        assert_eq!(stream.event_index_at_tick(480), 2);
        assert_eq!(stream.event_index_at_tick(960), 3);
        assert_eq!(stream.event_index_at_tick(1000), 4);
    }

    #[test]
    fn test_seek_bar_exists() {
        let events = vec![
            make_bar_event(0, 1),
            make_bar_event(1920, 2),
            make_bar_event(3840, 3),
        ];
        let stream = CompiledStream::new(events, 480, 120.0);

        assert_eq!(seek_bar_in_new_stream(&stream, 2), 1920);
    }

    #[test]
    fn test_seek_bar_wraps_to_bar_1() {
        let events = vec![make_bar_event(0, 1), make_bar_event(1920, 2)];
        let stream = CompiledStream::new(events, 480, 120.0);

        // Bar 5 doesn't exist, wraps to bar 1.
        assert_eq!(seek_bar_in_new_stream(&stream, 5), 0);
    }

    #[test]
    fn test_no_phantom_bars_before_first_marker() {
        // First marker is bar 5 (e.g. every track uses start=5). Bars 1-4
        // must report None, not tick 0 — a 0 sentinel here previously made
        // seeks land at tick 0 and inflated total_ticks estimates.
        let events = vec![make_bar_event(7680, 5), make_bar_event(9600, 6)];
        let stream = CompiledStream::new(events, 480, 120.0);

        assert_eq!(stream.bar_tick(1), None);
        assert_eq!(stream.bar_tick(4), None);
        assert_eq!(stream.bar_tick(5), Some(7680));
        assert_eq!(stream.bar_tick(6), Some(9600));
        // total_ticks estimates the last bar from the last two KNOWN bars:
        // 9600 + (9600 - 7680) = 11520 — not distorted by phantom zeros.
        assert_eq!(stream.total_ticks(), 11520);
    }

    #[test]
    fn test_bar_one_at_tick_zero_is_real() {
        let events = vec![make_bar_event(0, 1), make_bar_event(1920, 2)];
        let stream = CompiledStream::new(events, 480, 120.0);
        assert_eq!(stream.bar_tick(1), Some(0));
        assert_eq!(stream.total_ticks(), 3840);
    }

    #[test]
    fn test_hotswap_slot_stage_and_take() {
        let slot = HotSwapSlot::new();
        assert!(!slot.has_staged());
        assert!(slot.take().is_none());

        let stream = CompiledStream::new(vec![], 480, 120.0);
        slot.stage(stream);
        assert!(slot.has_staged());

        let taken = slot.take();
        assert!(taken.is_some());
        assert!(!slot.has_staged());
        assert!(slot.take().is_none());
    }

    #[test]
    fn test_hotswap_slot_overwrite() {
        let slot = HotSwapSlot::new();

        let stream1 = CompiledStream::new(vec![make_bar_event(0, 1)], 480, 120.0);
        slot.stage(stream1);

        let stream2 = CompiledStream::new(
            vec![make_bar_event(0, 1), make_bar_event(1920, 2)],
            480,
            140.0,
        );
        slot.stage(stream2);

        let taken = slot.take().unwrap();
        assert_eq!(taken.bpm, 140.0);
        assert_eq!(taken.bar_count(), 2);
    }
}
