//! Round-trip integration tests for the SMF renderer.
//!
//! Each test compiles Interval source → EventStream → SMF bytes → parse back
//! with midly → verify structure and event data.

use interval_core::ast::Block;
use interval_core::compiler;
use interval_core::lexer::tokenize;
use interval_core::parser::parse_header;
use interval_smf::renderer;
use midly::num::u15;
use midly::{Format, MidiMessage, Smf, Timing, TrackEventKind};

/// Helper: compile source to EventStream, render to SMF, parse back.
fn round_trip(source: &str) -> Smf<'static> {
    let (header, mut parser) = {
        let (tokens, lex_errors) = tokenize(source);
        assert!(lex_errors.is_empty(), "lexer errors: {lex_errors:?}");
        parse_header(tokens).expect("header parse failed")
    };

    let mut blocks = Vec::new();
    while parser.has_tokens() {
        parser.skip_newlines_pub();
        if !parser.has_tokens() {
            break;
        }
        if parser.peek_is_harmony() {
            blocks.push(Block::Harmony(
                parser.parse_harmony_block().expect("harmony parse failed"),
            ));
        } else if parser.peek_is_pattern() {
            blocks.push(Block::Pattern(
                parser.parse_pattern_block().expect("pattern parse failed"),
            ));
        } else if parser.peek_is_track() {
            blocks.push(Block::Track(
                parser.parse_track_block().expect("track parse failed"),
            ));
        } else if parser.peek_is_drummap() {
            blocks.push(Block::DrumMap(
                parser.parse_drummap_block().expect("drummap parse failed"),
            ));
        } else {
            break;
        }
        parser.skip_newlines_pub();
    }

    let output = compiler::compile(&header, &blocks).expect("compilation failed");

    let mut buf = Vec::new();
    renderer::render(&output.events, output.ppq, &mut buf).expect("SMF render failed");

    // Leak the buffer to get 'static lifetime (acceptable in tests).
    let leaked: &'static [u8] = Box::leak(buf.into_boxed_slice());
    Smf::parse(leaked).expect("SMF round-trip parse failed")
}

#[test]
fn test_round_trip_basic_two_notes() {
    let source = "\
@bpm 120
@ppq 480

@pattern melody steps=2 unit=1/4
C4
E4

@track piano ch=1
play: melody
";

    let smf = round_trip(source);

    assert_eq!(smf.header.format, Format::Parallel);
    assert_eq!(smf.header.timing, Timing::Metrical(u15::new(480)));

    // 2 tracks: tempo (track 0) + piano (track 1)
    assert_eq!(smf.tracks.len(), 2);

    // Track 0: Tempo + TimeSignature + EndOfTrack
    let track0 = &smf.tracks[0];
    assert!(matches!(
        track0[0].kind,
        TrackEventKind::Meta(midly::MetaMessage::Tempo(t)) if t.as_int() == 500_000
    ));
    assert!(matches!(
        track0.last().unwrap().kind,
        TrackEventKind::Meta(midly::MetaMessage::EndOfTrack)
    ));

    // Track 1: find NoteOn events
    let track1 = &smf.tracks[1];
    let note_ons: Vec<_> = track1
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                TrackEventKind::Midi {
                    message: MidiMessage::NoteOn { .. },
                    ..
                }
            )
        })
        .collect();
    assert_eq!(note_ons.len(), 2, "expected 2 NoteOn events");

    // First note: C4 (60)
    assert!(matches!(
        note_ons[0].kind,
        TrackEventKind::Midi { message: MidiMessage::NoteOn { key, .. }, .. }
        if key.as_int() == 60
    ));

    // Second note: E4 (64)
    assert!(matches!(
        note_ons[1].kind,
        TrackEventKind::Midi { message: MidiMessage::NoteOn { key, .. }, .. }
        if key.as_int() == 64
    ));
}

#[test]
fn test_round_trip_tempo_and_time_sig() {
    let source = "\
@bpm 140
@ts 3/4
@ppq 480

@pattern p steps=1 unit=1/4
C4

@track t ch=1
play: p
";

    let smf = round_trip(source);

    let track0 = &smf.tracks[0];

    // Tempo: 140 BPM = 428571 µs/quarter
    let tempo = track0
        .iter()
        .find(|e| matches!(e.kind, TrackEventKind::Meta(midly::MetaMessage::Tempo(_))))
        .expect("no tempo event");
    if let TrackEventKind::Meta(midly::MetaMessage::Tempo(t)) = tempo.kind {
        assert_eq!(t.as_int(), 428_571);
    }

    // Time signature: 3/4 → numerator=3, den_power=2
    let timesig = track0
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                TrackEventKind::Meta(midly::MetaMessage::TimeSignature(..))
            )
        })
        .expect("no time signature event");
    if let TrackEventKind::Meta(midly::MetaMessage::TimeSignature(num, den_pow, _, _)) =
        timesig.kind
    {
        assert_eq!(num, 3);
        assert_eq!(den_pow, 2);
    }
}

#[test]
fn test_round_trip_program_change() {
    let source = "\
@bpm 120
@ppq 480

@pattern p steps=1 unit=1/4
C4

@track strings ch=1 prog=48
play: p
";

    let smf = round_trip(source);
    let track1 = &smf.tracks[1];

    let pc = track1
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                TrackEventKind::Midi {
                    message: MidiMessage::ProgramChange { .. },
                    ..
                }
            )
        })
        .expect("no ProgramChange event");

    if let TrackEventKind::Midi {
        message: MidiMessage::ProgramChange { program },
        ..
    } = pc.kind
    {
        assert_eq!(program.as_int(), 48);
    }
}

#[test]
fn test_round_trip_delta_times() {
    let source = "\
@bpm 120
@ppq 480

@pattern p steps=2 unit=1/4
C4
G4

@track t ch=1
play: p
";

    let smf = round_trip(source);
    let track1 = &smf.tracks[1];

    // Reconstruct absolute ticks from deltas
    let mut tick = 0u32;
    let mut note_on_ticks = Vec::new();
    for e in track1 {
        tick += e.delta.as_int();
        if matches!(
            e.kind,
            TrackEventKind::Midi {
                message: MidiMessage::NoteOn { .. },
                ..
            }
        ) {
            note_on_ticks.push(tick);
        }
    }

    assert_eq!(note_on_ticks.len(), 2);
    assert_eq!(note_on_ticks[0], 0);
    assert_eq!(note_on_ticks[1], 480);
}

#[test]
fn test_round_trip_bar_markers_stripped() {
    let source = "\
@bpm 120
@ppq 480
@ts 4/4

@pattern p steps=4 unit=1/4
C4
E4
G4
C5

@track t ch=1
play: p
";

    let smf = round_trip(source);

    // No unknown meta events (BarMarkers are stripped entirely)
    for (i, track) in smf.tracks.iter().enumerate() {
        for event in track {
            assert!(
                !matches!(
                    event.kind,
                    TrackEventKind::Meta(midly::MetaMessage::Unknown(..))
                ),
                "unexpected unknown meta event in track {i}"
            );
        }
    }
}

#[test]
fn test_round_trip_track_name() {
    let source = "\
@bpm 120
@ppq 480

@pattern p steps=1 unit=1/4
C4

@track piano ch=1
play: p
";

    let smf = round_trip(source);
    let track1 = &smf.tracks[1];

    let name_event = track1
        .iter()
        .find(|e| {
            matches!(
                e.kind,
                TrackEventKind::Meta(midly::MetaMessage::TrackName(_))
            )
        })
        .expect("no TrackName event");

    if let TrackEventKind::Meta(midly::MetaMessage::TrackName(name)) = name_event.kind {
        assert_eq!(std::str::from_utf8(name).unwrap(), "piano");
    }
}

#[test]
fn test_round_trip_end_of_track() {
    let source = "\
@bpm 120
@ppq 480

@pattern p steps=1 unit=1/4
C4

@track t ch=1
play: p
";

    let smf = round_trip(source);

    for (i, track) in smf.tracks.iter().enumerate() {
        assert!(!track.is_empty(), "track {i} is empty");
        assert!(
            matches!(
                track.last().unwrap().kind,
                TrackEventKind::Meta(midly::MetaMessage::EndOfTrack)
            ),
            "track {i} does not end with EndOfTrack"
        );
    }
}

#[test]
fn test_round_trip_note_off_velocity_zero() {
    let source = "\
@bpm 120
@ppq 480

@pattern p steps=1 unit=1/4
C4

@track t ch=1
play: p
";

    let smf = round_trip(source);
    let track1 = &smf.tracks[1];

    for event in track1 {
        if let TrackEventKind::Midi {
            message: MidiMessage::NoteOff { vel, .. },
            ..
        } = event.kind
        {
            assert_eq!(vel.as_int(), 0, "NoteOff velocity should be 0");
        }
    }
}

#[test]
fn test_round_trip_ppq_preserved() {
    for ppq in [96u16, 240, 480, 960] {
        let source = format!(
            "\
@bpm 120
@ppq {ppq}

@pattern p steps=1 unit=1/4
C4

@track t ch=1
play: p
"
        );

        let smf = round_trip(&source);
        assert_eq!(
            smf.header.timing,
            Timing::Metrical(u15::new(ppq)),
            "PPQ {ppq} not preserved in SMF header"
        );
    }
}

#[test]
fn test_round_trip_multiple_tracks() {
    let source = "\
@bpm 120
@ppq 480

@pattern melody steps=1 unit=1/4
C4

@pattern bass steps=1 unit=1/4
C3

@track lead ch=1
play: melody

@track bass_track ch=2
play: bass
";

    let smf = round_trip(source);
    assert_eq!(smf.tracks.len(), 3);

    for (i, expected_name) in [(1, "lead"), (2, "bass_track")] {
        let name_event = smf.tracks[i].iter().find(|e| {
            matches!(
                e.kind,
                TrackEventKind::Meta(midly::MetaMessage::TrackName(_))
            )
        });
        if let Some(e) = name_event {
            if let TrackEventKind::Meta(midly::MetaMessage::TrackName(name)) = e.kind {
                assert_eq!(
                    std::str::from_utf8(name).unwrap(),
                    expected_name,
                    "track {i} name mismatch"
                );
            }
        }
    }
}

#[test]
fn test_round_trip_deterministic() {
    let source = "\
@bpm 120
@ppq 480

@pattern p steps=4 unit=1/4
C4
E4
G4
C5

@track t ch=1
play: p
";

    let (header, mut parser) = {
        let (tokens, lex_errors) = tokenize(source);
        assert!(lex_errors.is_empty());
        parse_header(tokens).unwrap()
    };

    let mut blocks = Vec::new();
    while parser.has_tokens() {
        parser.skip_newlines_pub();
        if !parser.has_tokens() {
            break;
        }
        if parser.peek_is_pattern() {
            blocks.push(Block::Pattern(parser.parse_pattern_block().unwrap()));
        } else if parser.peek_is_track() {
            blocks.push(Block::Track(parser.parse_track_block().unwrap()));
        } else {
            break;
        }
        parser.skip_newlines_pub();
    }

    let output = compiler::compile(&header, &blocks).unwrap();

    let mut buf1 = Vec::new();
    renderer::render(&output.events, output.ppq, &mut buf1).unwrap();

    let mut buf2 = Vec::new();
    renderer::render(&output.events, output.ppq, &mut buf2).unwrap();

    assert_eq!(buf1, buf2, "SMF output should be deterministic");
}
