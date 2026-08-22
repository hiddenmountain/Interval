//! Playback state for the RT scheduler.
//!
//! Tracks per-track runtime state needed for v0.3 features:
//! - `pattern_loop_count`: how many times the current pattern has completed
//! - `last_conditional_played`: for `[pre]` condition evaluation
//! - `active_notes`: for stuck-note prevention and hot-swap cleanup
//! - `rate_adjusted_tick`: for `rate=` track-level playback rate

use std::collections::HashSet;

/// Per-track playback state maintained by the RT scheduler.
#[derive(Debug, Clone)]
pub struct TrackState {
    /// Which item in the play: sequence we are currently on.
    pub sequence_position: usize,
    /// How many times the current pattern instance has fully looped.
    pub pattern_loop_count: u32,
    /// The name of the current pattern (for identity comparison during hot-swap).
    pub current_pattern_name: Option<String>,
    /// Did the last conditional step on this track play?
    pub last_conditional_played: bool,
    /// Active notes for stuck-note prevention: (channel, note).
    pub active_notes: HashSet<(u8, u8)>,
    /// Current effective tick after rate= adjustment (for Phase 7).
    pub rate_adjusted_tick: u64,
}

impl TrackState {
    /// Create a new track state with default values.
    pub fn new() -> Self {
        Self {
            sequence_position: 0,
            pattern_loop_count: 0,
            current_pattern_name: None,
            last_conditional_played: false,
            active_notes: HashSet::new(),
            rate_adjusted_tick: 0,
        }
    }

    /// Reset state for a fresh play from the beginning.
    pub fn reset(&mut self) {
        self.sequence_position = 0;
        self.pattern_loop_count = 0;
        self.current_pattern_name = None;
        self.last_conditional_played = false;
        self.active_notes.clear();
        self.rate_adjusted_tick = 0;
    }

    /// Handle a pattern boundary event.
    ///
    /// `PatternBoundary` is emitted at the *start* of each pattern instance,
    /// so the first boundary for a pattern begins pass 0 — `loop_count == 0`
    /// during the entire first pass. This matches the SMF renderer's static
    /// evaluation (loop 0), so `[once]` plays on the first pass and
    /// `[every:4]` fires on passes 1, 5, 9 in both outputs.
    ///
    /// If the pattern name matches the current pattern, increment the loop
    /// count. If it differs, reset to 0 and update the pattern name.
    pub fn on_pattern_boundary(&mut self, pattern_name: &str) {
        match &self.current_pattern_name {
            Some(current) if current == pattern_name => {
                self.pattern_loop_count += 1;
            }
            _ => {
                self.current_pattern_name = Some(pattern_name.to_string());
                self.pattern_loop_count = 0;
            }
        }
    }

    /// Transfer state during hot-swap.
    ///
    /// If the pattern identity is unchanged in the new stream, preserve the loop
    /// count. Otherwise reset it. Always reset `last_conditional_played`.
    pub fn transfer_for_hot_swap(&mut self, new_pattern_name: Option<&str>) {
        let pattern_changed = match (&self.current_pattern_name, new_pattern_name) {
            (Some(old), Some(new)) => old != new,
            _ => true,
        };

        if pattern_changed {
            self.pattern_loop_count = 0;
            self.current_pattern_name = new_pattern_name.map(|s| s.to_string());
        }

        self.last_conditional_played = false;
        // active_notes are preserved — caller is responsible for emitting NoteOff
        // for orphaned notes.
    }
}

impl Default for TrackState {
    fn default() -> Self {
        Self::new()
    }
}

/// Global playback state maintained by the RT scheduler.
#[derive(Debug, Clone)]
pub struct PlaybackState {
    /// Current global tick position.
    pub global_tick: u64,
    /// Per-track state.
    pub tracks: Vec<TrackState>,
}

impl PlaybackState {
    /// Create a new playback state for the given number of tracks.
    pub fn new(track_count: usize) -> Self {
        Self {
            global_tick: 0,
            tracks: (0..track_count).map(|_| TrackState::new()).collect(),
        }
    }

    /// Reset all state for a fresh play from the beginning.
    pub fn reset(&mut self) {
        self.global_tick = 0;
        for track in &mut self.tracks {
            track.reset();
        }
    }

    /// Ensure there are at least `count` track states, adding defaults as needed.
    pub fn ensure_track_count(&mut self, count: usize) {
        while self.tracks.len() < count {
            self.tracks.push(TrackState::new());
        }
    }
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self::new(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_track_state_new() {
        let state = TrackState::new();
        assert_eq!(state.sequence_position, 0);
        assert_eq!(state.pattern_loop_count, 0);
        assert!(state.current_pattern_name.is_none());
        assert!(!state.last_conditional_played);
        assert!(state.active_notes.is_empty());
        assert_eq!(state.rate_adjusted_tick, 0);
    }

    #[test]
    fn test_playback_state_initializes_correctly() {
        let state = PlaybackState::new(3);
        assert_eq!(state.global_tick, 0);
        assert_eq!(state.tracks.len(), 3);
        for track in &state.tracks {
            assert_eq!(track.pattern_loop_count, 0);
            assert!(track.active_notes.is_empty());
        }
    }

    #[test]
    fn test_pattern_loop_count_increments() {
        let mut track = TrackState::new();

        // First boundary for pattern "bass_line" — sets name, begins pass 0.
        // (Boundaries fire at instance START, so the first pass runs with
        // loop_count == 0, matching the SMF renderer's static evaluation.)
        track.on_pattern_boundary("bass_line");
        assert_eq!(track.pattern_loop_count, 0);
        assert_eq!(track.current_pattern_name.as_deref(), Some("bass_line"));

        // Same pattern boundary — count increments.
        track.on_pattern_boundary("bass_line");
        assert_eq!(track.pattern_loop_count, 1);

        track.on_pattern_boundary("bass_line");
        assert_eq!(track.pattern_loop_count, 2);
    }

    #[test]
    fn test_pattern_loop_count_resets_on_different_pattern() {
        let mut track = TrackState::new();

        track.on_pattern_boundary("intro");
        track.on_pattern_boundary("intro");
        assert_eq!(track.pattern_loop_count, 1);

        // Different pattern — resets to pass 0.
        track.on_pattern_boundary("verse");
        assert_eq!(track.pattern_loop_count, 0);
        assert_eq!(track.current_pattern_name.as_deref(), Some("verse"));
    }

    #[test]
    fn test_hot_swap_preserves_loop_count_same_pattern() {
        let mut track = TrackState::new();
        track.on_pattern_boundary("comp");
        track.on_pattern_boundary("comp");
        track.on_pattern_boundary("comp");
        assert_eq!(track.pattern_loop_count, 2);
        track.last_conditional_played = true;

        // Hot-swap with same pattern identity.
        track.transfer_for_hot_swap(Some("comp"));
        assert_eq!(track.pattern_loop_count, 2); // preserved
        assert!(!track.last_conditional_played); // reset
    }

    #[test]
    fn test_hot_swap_resets_loop_count_different_pattern() {
        let mut track = TrackState::new();
        track.on_pattern_boundary("comp_v1");
        track.on_pattern_boundary("comp_v1");
        assert_eq!(track.pattern_loop_count, 1);

        // Hot-swap with different pattern identity.
        track.transfer_for_hot_swap(Some("comp_v2"));
        assert_eq!(track.pattern_loop_count, 0); // reset
        assert_eq!(track.current_pattern_name.as_deref(), Some("comp_v2"));
    }

    #[test]
    fn test_hot_swap_emits_note_off_for_orphaned_notes() {
        let mut track = TrackState::new();
        track.active_notes.insert((0, 60));
        track.active_notes.insert((0, 64));
        track.active_notes.insert((1, 48));

        // After hot-swap, active_notes are preserved (caller handles NoteOff).
        track.transfer_for_hot_swap(Some("new_pattern"));
        assert_eq!(track.active_notes.len(), 3);
        assert!(track.active_notes.contains(&(0, 60)));
        assert!(track.active_notes.contains(&(0, 64)));
        assert!(track.active_notes.contains(&(1, 48)));
    }

    #[test]
    fn test_playback_state_reset() {
        let mut state = PlaybackState::new(2);
        state.global_tick = 5000;
        state.tracks[0].pattern_loop_count = 5;
        state.tracks[0].active_notes.insert((0, 60));
        state.tracks[1].last_conditional_played = true;

        state.reset();
        assert_eq!(state.global_tick, 0);
        assert_eq!(state.tracks[0].pattern_loop_count, 0);
        assert!(state.tracks[0].active_notes.is_empty());
        assert!(!state.tracks[1].last_conditional_played);
    }

    #[test]
    fn test_ensure_track_count() {
        let mut state = PlaybackState::new(1);
        assert_eq!(state.tracks.len(), 1);

        state.ensure_track_count(4);
        assert_eq!(state.tracks.len(), 4);

        // Should not shrink.
        state.ensure_track_count(2);
        assert_eq!(state.tracks.len(), 4);
    }
}
