//! Event stream types for the Interval compiler output.
//!
//! The compiler produces a `Vec<TimedEvent>` sorted by tick. This event stream
//! is the shared intermediate representation consumed by both the SMF renderer
//! and the RT scheduler. `BarMarker` events exist for RT hot-swap seeking and
//! are stripped by the SMF renderer.

use crate::ast::StepCondition;
use serde::Serialize;

/// A MIDI event with no timing information.
#[derive(Debug, Clone, Serialize)]
pub enum MidiEvent {
    /// Note-on with channel, note, and velocity.
    NoteOn {
        /// MIDI channel (0-15 internal, 1-16 user-facing).
        channel: u8,
        /// MIDI note number (0-127).
        note: u8,
        /// Velocity (1-127).
        velocity: u8,
    },
    /// Note-off with channel and note.
    NoteOff {
        /// MIDI channel.
        channel: u8,
        /// MIDI note number.
        note: u8,
    },
    /// Control change.
    CC {
        /// MIDI channel.
        channel: u8,
        /// Controller number (0-127).
        controller: u8,
        /// Value (0-127).
        value: u8,
    },
    /// Program change.
    ProgramChange {
        /// MIDI channel.
        channel: u8,
        /// Program number (0-127).
        program: u8,
    },
    /// Pitch bend.
    PitchBend {
        /// MIDI channel.
        channel: u8,
        /// Pitch bend value (-8192 to 8191).
        value: i16,
    },
    /// Channel aftertouch.
    Aftertouch {
        /// MIDI channel.
        channel: u8,
        /// Aftertouch value (0-127).
        value: u8,
    },
    /// Tempo meta event.
    Tempo {
        /// Tempo in beats per minute.
        bpm: f64,
    },
    /// Time signature meta event.
    TimeSignature {
        /// Numerator.
        numerator: u8,
        /// Denominator.
        denominator: u8,
    },
    /// Track name meta event.
    TrackName {
        /// Track name string.
        name: String,
    },
    /// Text meta event (for embedding metadata like seed).
    TextMeta {
        /// Text content.
        text: String,
    },
    /// Bar marker (for RT scheduler hot-swap, stripped by SMF renderer).
    BarMarker {
        /// Bar number (1-indexed).
        bar: u32,
    },
    /// Pattern boundary marker (for RT scheduler loop counting, stripped by SMF renderer).
    PatternBoundary {
        /// Track index this boundary belongs to.
        track: usize,
        /// Pattern name for identity comparison during hot-swap.
        pattern_name: String,
    },
}

impl MidiEvent {
    /// Returns the sort priority for intra-tick event ordering.
    /// Lower values sort first:
    /// Meta (Tempo/TS/TrackName) → NoteOff → ProgramChange → CC → PitchBend → Aftertouch → NoteOn → BarMarker.
    pub fn sort_priority(&self) -> u8 {
        match self {
            MidiEvent::Tempo { .. } => 0,
            MidiEvent::TimeSignature { .. } => 1,
            MidiEvent::TrackName { .. } => 2,
            MidiEvent::TextMeta { .. } => 2, // same priority as TrackName
            MidiEvent::NoteOff { .. } => 3,
            MidiEvent::ProgramChange { .. } => 4,
            MidiEvent::CC { .. } => 5,
            MidiEvent::PitchBend { .. } => 6,
            MidiEvent::Aftertouch { .. } => 7,
            MidiEvent::NoteOn { .. } => 8,
            MidiEvent::BarMarker { .. } => 9,
            MidiEvent::PatternBoundary { .. } => 10,
        }
    }
}

/// A timed event in the event stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimedEvent {
    /// Absolute tick position.
    pub tick: u64,
    /// Track index (0 = tempo track, 1+ = user tracks in declaration order).
    pub track: usize,
    /// The MIDI event.
    pub event: MidiEvent,
    /// Optional conditional playback annotation. When present, the RT scheduler
    /// evaluates the condition against `PlaybackState` loop count before emitting
    /// the event. The SMF renderer evaluates at iteration 1 (first pass).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<StepCondition>,
    /// Index of the originating step within the resolved pattern.
    /// Set for note/CC events emitted from the step iteration loop;
    /// `None` for structural events (BarMarker, Tempo, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_index: Option<usize>,
}

impl TimedEvent {
    /// Create a new unconditional timed event.
    pub fn new(tick: u64, track: usize, event: MidiEvent) -> Self {
        Self {
            tick,
            track,
            event,
            condition: None,
            step_index: None,
        }
    }

    /// Create a new conditional timed event.
    pub fn with_condition(
        tick: u64,
        track: usize,
        event: MidiEvent,
        condition: StepCondition,
    ) -> Self {
        Self {
            tick,
            track,
            event,
            condition: Some(condition),
            step_index: None,
        }
    }
}

/// The complete event stream output of the compiler.
pub type EventStream = Vec<TimedEvent>;

/// Sort an event stream by tick, then by event priority, then by track index.
pub fn sort_event_stream(events: &mut EventStream) {
    events.sort_by(|a, b| {
        a.tick
            .cmp(&b.tick)
            .then_with(|| a.event.sort_priority().cmp(&b.event.sort_priority()))
            .then_with(|| a.track.cmp(&b.track))
    });
}

impl PartialEq for MidiEvent {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                MidiEvent::NoteOn {
                    channel: c1,
                    note: n1,
                    velocity: v1,
                },
                MidiEvent::NoteOn {
                    channel: c2,
                    note: n2,
                    velocity: v2,
                },
            ) => c1 == c2 && n1 == n2 && v1 == v2,
            (
                MidiEvent::NoteOff {
                    channel: c1,
                    note: n1,
                },
                MidiEvent::NoteOff {
                    channel: c2,
                    note: n2,
                },
            ) => c1 == c2 && n1 == n2,
            (
                MidiEvent::CC {
                    channel: c1,
                    controller: ct1,
                    value: v1,
                },
                MidiEvent::CC {
                    channel: c2,
                    controller: ct2,
                    value: v2,
                },
            ) => c1 == c2 && ct1 == ct2 && v1 == v2,
            (
                MidiEvent::ProgramChange {
                    channel: c1,
                    program: p1,
                },
                MidiEvent::ProgramChange {
                    channel: c2,
                    program: p2,
                },
            ) => c1 == c2 && p1 == p2,
            (
                MidiEvent::PitchBend {
                    channel: c1,
                    value: v1,
                },
                MidiEvent::PitchBend {
                    channel: c2,
                    value: v2,
                },
            ) => c1 == c2 && v1 == v2,
            (
                MidiEvent::Aftertouch {
                    channel: c1,
                    value: v1,
                },
                MidiEvent::Aftertouch {
                    channel: c2,
                    value: v2,
                },
            ) => c1 == c2 && v1 == v2,
            (MidiEvent::Tempo { bpm: b1 }, MidiEvent::Tempo { bpm: b2 }) => {
                b1.to_bits() == b2.to_bits()
            }
            (
                MidiEvent::TimeSignature {
                    numerator: n1,
                    denominator: d1,
                },
                MidiEvent::TimeSignature {
                    numerator: n2,
                    denominator: d2,
                },
            ) => n1 == n2 && d1 == d2,
            (MidiEvent::TrackName { name: n1 }, MidiEvent::TrackName { name: n2 }) => n1 == n2,
            (MidiEvent::TextMeta { text: t1 }, MidiEvent::TextMeta { text: t2 }) => t1 == t2,
            (MidiEvent::BarMarker { bar: b1 }, MidiEvent::BarMarker { bar: b2 }) => b1 == b2,
            (
                MidiEvent::PatternBoundary {
                    track: t1,
                    pattern_name: n1,
                },
                MidiEvent::PatternBoundary {
                    track: t2,
                    pattern_name: n2,
                },
            ) => t1 == t2 && n1 == n2,
            _ => false,
        }
    }
}

impl Eq for MidiEvent {}
