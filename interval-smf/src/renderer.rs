//! SMF file renderer.
//!
//! Takes an `EventStream` from `interval-core` and writes a Type 1 SMF:
//! - Track 0: tempo, time signature, title metadata
//! - Tracks 1-N: one SMF track per `@track` declaration
//!
//! The input stream must already be sorted (the compiler's
//! `sort_event_stream` establishes tick order and the intra-tick ordering
//! NoteOff → ProgramChange → CC → PitchBend → Aftertouch → NoteOn); the
//! renderer validates monotonic ticks and rejects unsorted input rather
//! than silently collapsing deltas.
//!
//! BarMarker events are filtered out before writing. Delta time encoding
//! converts absolute ticks to per-track relative deltas.

use interval_core::event::{EventStream, MidiEvent};
use midly::num::{u15, u24, u28, u4, u7};
use midly::{Format, Header, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use std::io::Write;

/// Errors that can occur during SMF rendering.
#[derive(Debug, thiserror::Error)]
pub enum SmfError {
    /// An I/O error occurred while writing the SMF.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// PPQ value out of range for SMF (must be 1-32767).
    #[error("PPQ value {0} exceeds SMF maximum of 32767")]
    PpqOutOfRange(u32),
    /// PPQ of zero would produce a division header no player can interpret.
    #[error("PPQ must be at least 1")]
    PpqZero,
    /// Tick value out of range for delta encoding (must fit in u28).
    #[error("delta tick {0} exceeds SMF maximum")]
    DeltaOverflow(u64),
    /// Events were not in ascending tick order.
    #[error("event stream is not sorted: tick {tick} follows tick {last_tick}")]
    UnsortedEvents {
        /// The offending event's tick.
        tick: u64,
        /// The previous event's (larger) tick.
        last_tick: u64,
    },
    /// Tempo does not fit the SMF 24-bit microseconds-per-quarter field
    /// (valid range is roughly 3.58 to 60,000,000 BPM).
    #[error("tempo {0} BPM is outside the range representable in SMF")]
    TempoOutOfRange(f64),
    /// Time signature denominator must be a power of two.
    #[error("time signature denominator {0} is not a power of two")]
    InvalidTimeSignatureDenominator(u8),
}

/// Result type for SMF rendering operations.
pub type SmfResult<T> = Result<T, SmfError>;

/// Render an `EventStream` to a Type 1 Standard MIDI File.
///
/// # Arguments
/// * `events` — the sorted event stream from the compiler
/// * `ppq` — pulses per quarter note
/// * `writer` — any `std::io::Write` implementor (file, buffer, etc.)
///
/// # Errors
/// Returns `SmfError` if the PPQ is out of range, a delta overflows, or
/// an I/O error occurs during writing.
pub fn render<W: Write>(events: &EventStream, ppq: u32, writer: &mut W) -> SmfResult<()> {
    if ppq == 0 {
        return Err(SmfError::PpqZero);
    }
    if ppq > u15::max_value().as_int() as u32 {
        return Err(SmfError::PpqOutOfRange(ppq));
    }

    // Global end tick (including BarMarkers, which mark bar starts even in
    // trailing silence) — every track's EndOfTrack pads out to this point so
    // tracks of unequal length stay aligned and trailing rests survive.
    let end_tick = events.iter().map(|e| e.tick).max().unwrap_or(0);

    // Determine the number of SMF tracks. Track 0 is the tempo/meta track.
    // User tracks are 1-indexed in the event stream.
    let max_track = events.iter().map(|e| e.track).max().unwrap_or(0);
    let track_count = max_track + 1;

    // Partition events into per-track buckets, filtering out BarMarkers.
    let mut track_events: Vec<Vec<&interval_core::event::TimedEvent>> = vec![vec![]; track_count];
    for event in events {
        if matches!(
            event.event,
            MidiEvent::BarMarker { .. } | MidiEvent::PatternBoundary { .. }
        ) {
            continue;
        }
        // Static evaluation of conditions at iteration 1 (loop_count=0)
        if let Some(ref cond) = event.condition {
            if !smf_should_play(cond) {
                continue;
            }
        }
        if event.track < track_count {
            track_events[event.track].push(event);
        }
    }

    // Build SMF tracks with delta-time encoding. Track events borrow meta
    // strings (track names, text) directly from the input stream.
    let mut smf_tracks: Vec<Vec<TrackEvent<'_>>> = Vec::with_capacity(track_count);

    for track_bucket in &track_events {
        let mut smf_track = Vec::new();
        let mut last_tick: u64 = 0;

        for timed in track_bucket {
            if timed.tick < last_tick {
                return Err(SmfError::UnsortedEvents {
                    tick: timed.tick,
                    last_tick,
                });
            }
            let delta = timed.tick - last_tick;
            if delta > u28::max_value().as_int() as u64 {
                return Err(SmfError::DeltaOverflow(delta));
            }
            last_tick = timed.tick;

            if let Some(kind) = midi_event_to_track_event_kind(&timed.event)? {
                smf_track.push(TrackEvent {
                    delta: u28::new(delta as u32),
                    kind,
                });
            }
        }

        // Every track ends with EndOfTrack, padded to the global end tick so
        // trailing silence isn't truncated.
        let delta_to_end = end_tick.saturating_sub(last_tick);
        if delta_to_end > u28::max_value().as_int() as u64 {
            return Err(SmfError::DeltaOverflow(delta_to_end));
        }
        smf_track.push(TrackEvent {
            delta: u28::new(delta_to_end as u32),
            kind: TrackEventKind::Meta(midly::MetaMessage::EndOfTrack),
        });

        smf_tracks.push(smf_track);
    }

    let smf = Smf {
        header: Header::new(Format::Parallel, Timing::Metrical(u15::new(ppq as u16))),
        tracks: smf_tracks,
    };

    smf.write_std(writer)?;
    Ok(())
}

/// Convert a `MidiEvent` to a midly `TrackEventKind`, borrowing meta strings
/// from the event. Returns `Ok(None)` for events that should be stripped
/// (e.g., `BarMarker`) and an error for values SMF cannot represent.
fn midi_event_to_track_event_kind(event: &MidiEvent) -> SmfResult<Option<TrackEventKind<'_>>> {
    Ok(match event {
        MidiEvent::NoteOn {
            channel,
            note,
            velocity,
        } => Some(TrackEventKind::Midi {
            channel: u4::new(*channel),
            message: MidiMessage::NoteOn {
                key: u7::new(*note),
                vel: u7::new(*velocity),
            },
        }),
        MidiEvent::NoteOff { channel, note } => Some(TrackEventKind::Midi {
            channel: u4::new(*channel),
            message: MidiMessage::NoteOff {
                key: u7::new(*note),
                vel: u7::new(0),
            },
        }),
        MidiEvent::CC {
            channel,
            controller,
            value,
        } => Some(TrackEventKind::Midi {
            channel: u4::new(*channel),
            message: MidiMessage::Controller {
                controller: u7::new(*controller),
                value: u7::new(*value),
            },
        }),
        MidiEvent::ProgramChange { channel, program } => Some(TrackEventKind::Midi {
            channel: u4::new(*channel),
            message: MidiMessage::ProgramChange {
                program: u7::new(*program),
            },
        }),
        MidiEvent::PitchBend { channel, value } => Some(TrackEventKind::Midi {
            channel: u4::new(*channel),
            message: MidiMessage::PitchBend {
                bend: midly::PitchBend::from_int(*value),
            },
        }),
        MidiEvent::Aftertouch { channel, value } => Some(TrackEventKind::Midi {
            channel: u4::new(*channel),
            message: MidiMessage::ChannelAftertouch {
                vel: u7::new(*value),
            },
        }),
        MidiEvent::Tempo { bpm } => {
            // Convert BPM to microseconds per quarter note. midly's u24::new
            // masks out-of-range values (silently corrupting the tempo), so
            // validate before constructing.
            if !bpm.is_finite() || *bpm <= 0.0 {
                return Err(SmfError::TempoOutOfRange(*bpm));
            }
            let uspqn = (60_000_000.0 / bpm).round();
            if !(1.0..=u24::max_value().as_int() as f64).contains(&uspqn) {
                return Err(SmfError::TempoOutOfRange(*bpm));
            }
            Some(TrackEventKind::Meta(midly::MetaMessage::Tempo(u24::new(
                uspqn as u32,
            ))))
        }
        MidiEvent::TimeSignature {
            numerator,
            denominator,
        } => {
            // SMF time signature denominator is encoded as power of 2.
            if *denominator == 0 || !denominator.is_power_of_two() {
                return Err(SmfError::InvalidTimeSignatureDenominator(*denominator));
            }
            let den_power = denominator.trailing_zeros() as u8;
            Some(TrackEventKind::Meta(midly::MetaMessage::TimeSignature(
                *numerator, den_power, 24, // MIDI clocks per metronome click (standard)
                8,  // 32nd notes per quarter note (standard)
            )))
        }
        MidiEvent::TrackName { name } => Some(TrackEventKind::Meta(midly::MetaMessage::TrackName(
            name.as_bytes(),
        ))),
        MidiEvent::TextMeta { text } => Some(TrackEventKind::Meta(midly::MetaMessage::Text(
            text.as_bytes(),
        ))),
        MidiEvent::BarMarker { .. } | MidiEvent::PatternBoundary { .. } => None,
    })
}

/// Evaluate a step condition statically for SMF rendering (iteration 1, loop_count=0).
///
/// - `Once` → plays (loop 0 qualifies)
/// - `Every(N)` → plays (0 % N == 0)
/// - `Cond(1, Y)` → plays (0 % Y == 0, X-1 == 0)
/// - `Cond(X, Y)` where X > 1 → does NOT play
/// - `Pre` → does NOT play (no previous conditional context)
fn smf_should_play(cond: &interval_core::ast::StepCondition) -> bool {
    use interval_core::ast::StepCondition;
    match cond {
        StepCondition::Once => true,
        StepCondition::Every(_) => true, // 0 % N == 0 for any N
        StepCondition::Cond(x, _y) => *x == 1, // loop 0 % Y == 0, matches when X-1 == 0
        StepCondition::Pre => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use interval_core::event::TimedEvent;

    #[test]
    fn test_render_empty() {
        let events: EventStream = vec![];
        let mut buf = Vec::new();
        render(&events, 480, &mut buf).unwrap();
        assert!(!buf.is_empty());

        // Parse back with midly to verify it's a valid SMF.
        let smf = Smf::parse(&buf).unwrap();
        assert_eq!(smf.header.format, Format::Parallel);
        assert_eq!(smf.header.timing, Timing::Metrical(u15::new(480)));
    }

    #[test]
    fn test_render_tempo_track() {
        let events: EventStream = vec![
            TimedEvent {
                tick: 0,
                track: 0,
                event: MidiEvent::Tempo { bpm: 120.0 },
                condition: None,
                step_index: None,
            },
            TimedEvent {
                tick: 0,
                track: 0,
                event: MidiEvent::TimeSignature {
                    numerator: 4,
                    denominator: 4,
                },
                condition: None,
                step_index: None,
            },
        ];
        let mut buf = Vec::new();
        render(&events, 480, &mut buf).unwrap();

        let smf = Smf::parse(&buf).unwrap();
        assert_eq!(smf.tracks.len(), 1); // only track 0
                                         // Check tempo event: 120 BPM = 500000 µs/quarter
        let tempo_event = &smf.tracks[0][0];
        assert_eq!(tempo_event.delta, u28::new(0));
        assert!(matches!(
            tempo_event.kind,
            TrackEventKind::Meta(midly::MetaMessage::Tempo(t)) if t.as_int() == 500_000
        ));
    }

    #[test]
    fn test_render_notes() {
        let events: EventStream = vec![
            TimedEvent {
                tick: 0,
                track: 0,
                event: MidiEvent::Tempo { bpm: 120.0 },
                condition: None,
                step_index: None,
            },
            TimedEvent {
                tick: 0,
                track: 1,
                event: MidiEvent::TrackName {
                    name: "piano".to_string(),
                },
                condition: None,
                step_index: None,
            },
            TimedEvent {
                tick: 0,
                track: 1,
                event: MidiEvent::NoteOn {
                    channel: 0,
                    note: 60,
                    velocity: 100,
                },
                condition: None,
                step_index: None,
            },
            TimedEvent {
                tick: 480,
                track: 1,
                event: MidiEvent::NoteOff {
                    channel: 0,
                    note: 60,
                },
                condition: None,
                step_index: None,
            },
        ];
        let mut buf = Vec::new();
        render(&events, 480, &mut buf).unwrap();

        let smf = Smf::parse(&buf).unwrap();
        assert_eq!(smf.tracks.len(), 2);

        // Track 1 should have: TrackName, NoteOn, NoteOff, EndOfTrack
        assert_eq!(smf.tracks[1].len(), 4);

        // NoteOn at delta 0
        let note_on = &smf.tracks[1][1];
        assert_eq!(note_on.delta, u28::new(0));
        assert!(matches!(
            note_on.kind,
            TrackEventKind::Midi {
                channel,
                message: MidiMessage::NoteOn { key, vel }
            } if channel.as_int() == 0 && key.as_int() == 60 && vel.as_int() == 100
        ));

        // NoteOff at delta 480
        let note_off = &smf.tracks[1][2];
        assert_eq!(note_off.delta, u28::new(480));
        assert!(matches!(
            note_off.kind,
            TrackEventKind::Midi {
                channel,
                message: MidiMessage::NoteOff { key, .. }
            } if channel.as_int() == 0 && key.as_int() == 60
        ));
    }

    #[test]
    fn test_render_bar_markers_stripped() {
        let events: EventStream = vec![
            TimedEvent {
                tick: 0,
                track: 0,
                event: MidiEvent::Tempo { bpm: 120.0 },
                condition: None,
                step_index: None,
            },
            TimedEvent {
                tick: 0,
                track: 1,
                event: MidiEvent::BarMarker { bar: 1 },
                condition: None,
                step_index: None,
            },
            TimedEvent {
                tick: 0,
                track: 1,
                event: MidiEvent::NoteOn {
                    channel: 0,
                    note: 60,
                    velocity: 100,
                },
                condition: None,
                step_index: None,
            },
        ];
        let mut buf = Vec::new();
        render(&events, 480, &mut buf).unwrap();

        let smf = Smf::parse(&buf).unwrap();
        // Track 1: NoteOn + EndOfTrack (BarMarker stripped)
        assert_eq!(smf.tracks[1].len(), 2);
    }

    #[test]
    fn test_render_ppq_out_of_range() {
        let events: EventStream = vec![];
        let mut buf = Vec::new();
        let result = render(&events, 40000, &mut buf);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            SmfError::PpqOutOfRange(40000)
        ));
    }

    #[test]
    fn test_render_program_change() {
        let events: EventStream = vec![TimedEvent {
            tick: 0,
            track: 1,
            event: MidiEvent::ProgramChange {
                channel: 0,
                program: 42,
            },
            condition: None,
            step_index: None,
        }];
        let mut buf = Vec::new();
        render(&events, 480, &mut buf).unwrap();

        let smf = Smf::parse(&buf).unwrap();
        // Track 0 (tempo): just EndOfTrack
        // Track 1: ProgramChange + EndOfTrack
        assert_eq!(smf.tracks[1].len(), 2);
        assert!(matches!(
            smf.tracks[1][0].kind,
            TrackEventKind::Midi {
                message: MidiMessage::ProgramChange { program },
                ..
            } if program.as_int() == 42
        ));
    }

    #[test]
    fn test_render_cc() {
        let events: EventStream = vec![TimedEvent {
            tick: 0,
            track: 1,
            event: MidiEvent::CC {
                channel: 0,
                controller: 64,
                value: 127,
            },
            condition: None,
            step_index: None,
        }];
        let mut buf = Vec::new();
        render(&events, 480, &mut buf).unwrap();

        let smf = Smf::parse(&buf).unwrap();
        assert!(matches!(
            smf.tracks[1][0].kind,
            TrackEventKind::Midi {
                message: MidiMessage::Controller { controller, value },
                ..
            } if controller.as_int() == 64 && value.as_int() == 127
        ));
    }

    #[test]
    fn test_render_pitch_bend() {
        let events: EventStream = vec![TimedEvent {
            tick: 0,
            track: 1,
            event: MidiEvent::PitchBend {
                channel: 0,
                value: 4096,
            },
            condition: None,
            step_index: None,
        }];
        let mut buf = Vec::new();
        render(&events, 480, &mut buf).unwrap();

        let smf = Smf::parse(&buf).unwrap();
        assert!(matches!(
            smf.tracks[1][0].kind,
            TrackEventKind::Midi {
                message: MidiMessage::PitchBend { .. },
                ..
            }
        ));
    }

    #[test]
    fn test_render_aftertouch() {
        let events: EventStream = vec![TimedEvent {
            tick: 0,
            track: 1,
            event: MidiEvent::Aftertouch {
                channel: 0,
                value: 80,
            },
            condition: None,
            step_index: None,
        }];
        let mut buf = Vec::new();
        render(&events, 480, &mut buf).unwrap();

        let smf = Smf::parse(&buf).unwrap();
        assert!(matches!(
            smf.tracks[1][0].kind,
            TrackEventKind::Midi {
                message: MidiMessage::ChannelAftertouch { vel },
                ..
            } if vel.as_int() == 80
        ));
    }

    #[test]
    fn test_render_ppq_zero_rejected() {
        let events: EventStream = vec![];
        let mut buf = Vec::new();
        assert!(matches!(
            render(&events, 0, &mut buf),
            Err(SmfError::PpqZero)
        ));
    }

    #[test]
    fn test_render_tempo_below_smf_range_rejected() {
        // 1 BPM needs 60,000,000 µs/qn — more than fits in u24. midly would
        // silently mask this to a wrong tempo; we must reject it instead.
        let events: EventStream = vec![TimedEvent {
            tick: 0,
            track: 0,
            event: MidiEvent::Tempo { bpm: 1.0 },
            condition: None,
            step_index: None,
        }];
        let mut buf = Vec::new();
        assert!(matches!(
            render(&events, 480, &mut buf),
            Err(SmfError::TempoOutOfRange(_))
        ));
    }

    #[test]
    fn test_render_nonpow2_denominator_rejected() {
        let events: EventStream = vec![TimedEvent {
            tick: 0,
            track: 0,
            event: MidiEvent::TimeSignature {
                numerator: 7,
                denominator: 6,
            },
            condition: None,
            step_index: None,
        }];
        let mut buf = Vec::new();
        assert!(matches!(
            render(&events, 480, &mut buf),
            Err(SmfError::InvalidTimeSignatureDenominator(6))
        ));
    }

    #[test]
    fn test_render_unsorted_stream_rejected() {
        let mk = |tick, note| TimedEvent {
            tick,
            track: 1,
            event: MidiEvent::NoteOn {
                channel: 0,
                note,
                velocity: 100,
            },
            condition: None,
            step_index: None,
        };
        let events: EventStream = vec![mk(480, 60), mk(0, 62)];
        let mut buf = Vec::new();
        assert!(matches!(
            render(&events, 480, &mut buf),
            Err(SmfError::UnsortedEvents {
                tick: 0,
                last_tick: 480
            })
        ));
    }

    #[test]
    fn test_render_end_of_track_padded_to_stream_end() {
        // A note ending at 480 plus a BarMarker at 1920: EOT must land at
        // 1920, preserving the trailing silence.
        let events: EventStream = vec![
            TimedEvent {
                tick: 0,
                track: 1,
                event: MidiEvent::NoteOn {
                    channel: 0,
                    note: 60,
                    velocity: 100,
                },
                condition: None,
                step_index: None,
            },
            TimedEvent {
                tick: 480,
                track: 1,
                event: MidiEvent::NoteOff {
                    channel: 0,
                    note: 60,
                },
                condition: None,
                step_index: None,
            },
            TimedEvent {
                tick: 1920,
                track: 1,
                event: MidiEvent::BarMarker { bar: 2 },
                condition: None,
                step_index: None,
            },
        ];
        let mut buf = Vec::new();
        render(&events, 480, &mut buf).unwrap();

        let smf = Smf::parse(&buf).unwrap();
        let eot = smf.tracks[1].last().unwrap();
        assert_eq!(eot.delta, u28::new(1440)); // 1920 - 480
        assert!(matches!(
            eot.kind,
            TrackEventKind::Meta(midly::MetaMessage::EndOfTrack)
        ));
    }

    #[test]
    fn test_bpm_to_tempo() {
        // 120 BPM = 500000 µs/quarter
        let uspqn = (60_000_000.0_f64 / 120.0).round() as u32;
        assert_eq!(uspqn, 500_000);

        // 140 BPM ≈ 428571 µs/quarter
        let uspqn = (60_000_000.0_f64 / 140.0).round() as u32;
        assert_eq!(uspqn, 428_571);
    }

    #[test]
    fn test_time_sig_denominator_encoding() {
        // 4 → log2(4) = 2
        assert_eq!((4f64).log2() as u8, 2);
        // 8 → log2(8) = 3
        assert_eq!((8f64).log2() as u8, 3);
        // 2 → log2(2) = 1
        assert_eq!((2f64).log2() as u8, 1);
    }
}
