//! Main compiler pipeline.
//!
//! Orchestrates the full compilation from parsed AST to event stream:
//! 1. Build harmony index from `@harmony` blocks
//! 2. Resolve pattern composition expressions and apply transforms
//! 3. For each track, iterate steps and emit events:
//!    a. Query harmony context at step tick position
//!    b. Resolve degree tokens to MIDI note numbers
//!    c. Apply voicing strategy and inversion
//!    d. Calculate timing (note-on/off ticks, shifts)
//!    e. Emit MIDI events
//! 4. Sort events, insert BarMarker events at bar boundaries
//!
//! `inv=auto` voice leading is a sequential stateful pass — each chord's
//! voicing depends on the previous chord's resolved pitches. This state is
//! threaded explicitly through the compilation loop per track.

use std::collections::HashMap;

use crate::ast::{
    Annotation, ArpPattern, BarsSetting, Block, BpmBlock, CcValue, GlobalHeader, Inversion,
    PatternBlock, PatternExpr, StepLine, StepToken, TimingValue, TonalContext, TrackBlock,
    TrackContent, TsBlock, VoicingStrategy,
};
use crate::error::{CompileError, CompileResult, Span};
use crate::event::{sort_event_stream, EventStream, MidiEvent, TimedEvent};
use crate::harmony::{ChordContext, HarmonyIndex, ScaleTimeline};
use crate::pattern::{resolve_all, Boundary, ResolvedPattern};
use crate::transform;
use crate::voicing;

// ── Bar Layout ───────────────────────────────────────────────────────

/// Per-bar tick layout, supporting variable time signatures.
///
/// When a `@ts` timeline is present, each bar may have a different tick
/// length based on its time signature. When no timeline exists, all bars
/// share the same tick length derived from the header.
pub struct BarLayout {
    /// Per-bar info: (bar_start_tick, ticks_for_this_bar, ts_num, ts_den).
    /// One entry per bar in the timeline. After the last entry, the final
    /// time signature repeats indefinitely.
    bars: Vec<(u64, u64, u8, u8)>,
    /// Default ticks per bar (from the first / header TS). Used for bars
    /// beyond the explicitly defined timeline.
    default_ticks_per_bar: u64,
    /// Default time signature numerator.
    default_ts_num: u8,
    /// Default time signature denominator.
    default_ts_den: u8,
}

impl BarLayout {
    /// Build from a `@ts` timeline block.
    pub fn from_ts_block(
        ts_block: &TsBlock,
        ppq: u32,
        default_ts_num: u8,
        default_ts_den: u8,
    ) -> Self {
        let default_ticks_per_bar =
            Self::compute_bar_ticks_for(ppq, default_ts_num, default_ts_den);
        let mut bars = Vec::new();
        let mut current_tick: u64 = 0;

        for entry in &ts_block.entries {
            let tpb = Self::compute_bar_ticks_for(ppq, entry.numerator, entry.denominator);
            let num_bars = entry.bars.unwrap_or(1);
            for _ in 0..num_bars {
                bars.push((current_tick, tpb, entry.numerator, entry.denominator));
                current_tick += tpb;
            }
        }

        // Use the last entry's TS as the default for bars beyond the timeline
        let (final_tpb, final_num, final_den) = bars
            .last()
            .map(|&(_, tpb, n, d)| (tpb, n, d))
            .unwrap_or((default_ticks_per_bar, default_ts_num, default_ts_den));

        Self {
            bars,
            default_ticks_per_bar: final_tpb,
            default_ts_num: final_num,
            default_ts_den: final_den,
        }
    }

    /// Build from a fixed header (no timeline — all bars the same).
    pub fn from_header(header: &GlobalHeader) -> Self {
        let tpb =
            Self::compute_bar_ticks_for(header.ppq, header.ts_numerator, header.ts_denominator);
        Self {
            bars: Vec::new(), // empty = use default for all bars
            default_ticks_per_bar: tpb,
            default_ts_num: header.ts_numerator,
            default_ts_den: header.ts_denominator,
        }
    }

    /// Compute ticks for a single bar with the given time signature.
    fn compute_bar_ticks_for(ppq: u32, ts_num: u8, ts_den: u8) -> u64 {
        let beat_ticks = ppq as u64 * 4 / ts_den as u64;
        beat_ticks * ts_num as u64
    }

    /// Get the tick at which bar `bar_num` (1-indexed) starts.
    pub fn bar_start_tick(&self, bar_num: u32) -> u64 {
        let idx = bar_num as usize - 1;
        if idx < self.bars.len() {
            self.bars[idx].0
        } else {
            // Beyond defined bars: extrapolate from last defined bar
            let base_tick = self.bars.last().map(|&(t, tpb, _, _)| t + tpb).unwrap_or(0);
            let extra_bars = idx - self.bars.len();
            base_tick + extra_bars as u64 * self.default_ticks_per_bar
        }
    }

    /// Get ticks per bar for bar `bar_num` (1-indexed).
    pub fn ticks_for_bar(&self, bar_num: u32) -> u64 {
        let idx = bar_num as usize - 1;
        if idx < self.bars.len() {
            self.bars[idx].1
        } else {
            self.default_ticks_per_bar
        }
    }

    /// Get the (ts_num, ts_den) for bar `bar_num` (1-indexed).
    pub fn ts_for_bar(&self, bar_num: u32) -> (u8, u8) {
        let idx = bar_num as usize - 1;
        if idx < self.bars.len() {
            (self.bars[idx].2, self.bars[idx].3)
        } else {
            (self.default_ts_num, self.default_ts_den)
        }
    }

    /// Find which bar a given tick falls in. Returns (bar_number_1indexed, bar_start_tick).
    pub fn bar_at_tick(&self, tick: u64) -> (u32, u64) {
        // Binary search in the defined bars
        if !self.bars.is_empty() {
            let pos = self.bars.partition_point(|&(start, _, _, _)| start <= tick);
            if pos > 0 && pos <= self.bars.len() {
                let bar_idx = pos - 1;
                let (bar_start, bar_ticks, _, _) = self.bars[bar_idx];
                // Check if tick is within this bar
                if tick < bar_start + bar_ticks {
                    return ((bar_idx + 1) as u32, bar_start);
                }
            }
            // Beyond defined bars
            let base_tick = self.bars.last().map(|&(t, tpb, _, _)| t + tpb).unwrap_or(0);
            if tick >= base_tick {
                let extra = ((tick - base_tick) / self.default_ticks_per_bar) as u32;
                let bar_num = self.bars.len() as u32 + 1 + extra;
                let bar_start = base_tick + extra as u64 * self.default_ticks_per_bar;
                return (bar_num, bar_start);
            }
        }
        // No timeline or tick is 0
        let bar_num = (tick / self.default_ticks_per_bar) as u32 + 1;
        let bar_start = (bar_num as u64 - 1) * self.default_ticks_per_bar;
        (bar_num, bar_start)
    }

    /// Check if a tick is exactly at a bar boundary.
    pub fn is_bar_start(&self, tick: u64) -> bool {
        let (_, bar_start) = self.bar_at_tick(tick);
        tick == bar_start
    }
}

// ── Musical Context ───────────────────────────────────────────────────

/// Bundles stateless emit-function parameters into a single struct.
///
/// Reduces emit_step_line / emit_token / emit_subdivision from 25-26
/// parameters to ~10 (context + mutable state + per-step overrides).
///
/// Mutable state (`prev_pitches`, `active_notes`, `rng_state`) stays as
/// direct `&mut` parameters because they change on every step.
/// `evolve_offset` and `arp_config` stay as direct parameters because
/// they change per-step / per-pattern instance.
pub struct MusicalContext<'a> {
    // Track identity
    pub track_number: usize,
    pub channel: u8,

    // Defaults (updated per-segment in compile loop)
    pub default_vel: u8,
    pub default_gate: f64,
    pub default_octave: u8,
    pub track_shift_ticks: i64,
    pub track_lshift_ticks: i64,

    // Harmony & voicing (fixed per-track)
    pub harmony_index: Option<&'a HarmonyIndex>,
    pub drummap: Option<&'a HashMap<String, u8>>,
    pub track: &'a TrackBlock,
    pub effective_inv: Inversion,
    pub scale_timeline: &'a ScaleTimeline,

    // Global context
    pub header: &'a GlobalHeader,
    pub bpm_lookup: &'a [(u64, f64)],
    pub bar_layout: &'a BarLayout,

    // Per-step scoped emission transforms (spec §10.3/§10.5). Set from the
    // step's pattern-instance pipeline transforms; `None` when the step is
    // not covered.
    /// Active `humanize(timing, intensity)` for this step, if any.
    pub humanize: Option<(TimingValue, f64)>,
    /// Active `vary(probability)` for this step, if any.
    pub vary: Option<f64>,
}

// ── Compilation Result ─────────────────────────────────────────────────

/// Summary of a pattern instance within a track's play expression.
#[derive(Debug, Clone)]
pub struct TrackPatternInstance {
    /// Pattern name.
    pub pattern_name: String,
    /// Start tick (absolute).
    pub start_tick: u64,
    /// End tick (absolute, exclusive).
    pub end_tick: u64,
    /// Start bar (1-indexed).
    pub start_bar: u32,
    /// End bar (1-indexed, inclusive of last bar containing events).
    pub end_bar: u32,
    /// Transform names applied to this instance.
    pub transforms: Vec<String>,
}

/// Per-track metadata summary.
#[derive(Debug, Clone)]
pub struct TrackSummary {
    /// Track name.
    pub name: String,
    /// MIDI channel (0-indexed).
    pub channel: u8,
    /// GM program number.
    pub program: Option<u8>,
    /// Name of the followed harmony block.
    pub follow: Option<String>,
    /// Voicing strategy.
    pub voice: VoicingStrategy,
    /// Inversion setting.
    pub inv: Inversion,
    /// Whether this is a drum track.
    pub is_drum: bool,
    /// Pattern instances in this track's arrangement.
    pub patterns: Vec<TrackPatternInstance>,
    /// Source span of the track block.
    pub span: Option<Span>,
}

/// Result of a full compilation: the event stream plus any warnings.
pub struct CompileOutput {
    /// The sorted event stream.
    pub events: EventStream,
    /// PPQ value from the header (needed by the SMF renderer).
    pub ppq: u32,
    /// Non-fatal warnings generated during compilation.
    pub warnings: Vec<crate::error::CompileWarning>,
    /// Per-track metadata summaries.
    pub tracks: Vec<TrackSummary>,
    /// The parsed AST, if requested via `compile_with_ast()`.
    pub program: Option<crate::ast::Program>,
}

// ── Top-Level Compile ──────────────────────────────────────────────────

/// Compile a parsed program into a sorted MIDI event stream.
///
/// Takes the global header and all blocks in declaration order. Returns
/// the complete event stream with bar markers inserted.
pub fn compile(header: &GlobalHeader, blocks: &[Block]) -> CompileResult<CompileOutput> {
    // Collect blocks by type
    let mut harmony_blocks = Vec::new();
    let mut pattern_blocks = Vec::new();
    let mut track_blocks = Vec::new();
    let mut drummap_blocks = Vec::new();
    // Initialize tonal context from header scale_block (if present) or default.
    let mut tonal_context = if let Some(ref sb) = header.scale_block {
        sb.entries
            .first()
            .map(|e| TonalContext {
                root: e.root,
                mode: e.mode.clone().unwrap_or_else(|| "major".to_string()),
                span: None,
            })
            .unwrap_or_default()
    } else {
        TonalContext::default()
    };

    for block in blocks {
        match block {
            Block::Scale(tc) => tonal_context = tc.clone(),
            Block::ScaleTimeline(sb) => {
                // Set tonal_context from the first entry of the scale timeline
                if let Some(first) = sb.entries.first() {
                    tonal_context = TonalContext {
                        root: first.root,
                        mode: first.mode.clone().unwrap_or_else(|| "major".to_string()),
                        span: None,
                    };
                }
            }
            Block::Harmony(h) => harmony_blocks.push(h.clone()),
            Block::Pattern(p) => pattern_blocks.push(p.clone()),
            Block::Track(t) => track_blocks.push(t.clone()),
            Block::DrumMap(d) => drummap_blocks.push(d.clone()),
            // Unreachable since v0.5: `@tempo` is a hard parse error
            // everywhere (`DeprecatedTempo`), so the parser can no longer
            // construct a `Block::Tempo`. The arm remains only for match
            // exhaustiveness over the AST enum.
            Block::Tempo(_) => {}
            Block::BpmTimeline(_) => {} // BpmTimeline is handled via header.bpm_block
            Block::TsTimeline(_) => {}  // TsTimeline is handled via header.ts_block
        }
    }

    // Build the ScaleTimeline for per-bar Roman numeral resolution and ^n degree resolution.
    // Prefer header.scale_block (timeline form) over the scalar tonal_context.
    let scale_timeline = if let Some(ref sb) = header.scale_block {
        ScaleTimeline::from_scale_block(sb)?
    } else {
        ScaleTimeline::from_tonal_context(&tonal_context)?
    };

    // Build bar layout for variable time signature support (needed by harmony index)
    let bar_layout = if let Some(ref ts_block) = header.ts_block {
        BarLayout::from_ts_block(
            ts_block,
            header.ppq,
            header.ts_numerator,
            header.ts_denominator,
        )
    } else {
        BarLayout::from_header(header)
    };

    // Build harmony indices
    // For unnamed blocks, use empty string as key (only one unnamed allowed)
    let mut harmony_indices: HashMap<String, HarmonyIndex> = HashMap::new();
    let mut harmony_invs: HashMap<String, Inversion> = HashMap::new();
    for hb in &harmony_blocks {
        let index = HarmonyIndex::build(hb, header, &scale_timeline, &bar_layout)?;
        let key = hb.name.clone().unwrap_or_default();
        harmony_indices.insert(key.clone(), index);
        harmony_invs.insert(key, hb.inv);
    }

    // Phase 4: validate multiple-block naming rules and determine auto-follow target
    // If >1 harmony block exists and any is unnamed, that's an error.
    let auto_follow_key: Option<String> = if harmony_blocks.len() == 1 {
        // Single block — any track without follow= will use this
        Some(harmony_blocks[0].name.clone().unwrap_or_default())
    } else if harmony_blocks.len() > 1 {
        for hb in &harmony_blocks {
            if hb.name.is_none() {
                return Err(CompileError::MultipleHarmonyBlocksRequireNames {
                    span: hb.span.unwrap_or_else(|| Span::new(0, 0)),
                });
            }
        }
        None // multiple named blocks — no auto-follow
    } else {
        None // no harmony blocks
    };

    // Build drummap registry
    let mut drummap_registry: HashMap<String, HashMap<String, u8>> = HashMap::new();
    for dm in &drummap_blocks {
        let name = dm.name.clone().unwrap_or_default();
        let map: HashMap<String, u8> = dm.mappings.iter().cloned().collect();
        drummap_registry.insert(name, map);
    }

    // Enforce canonical transform pipeline order (spec §10.1:
    // swing → expressive transforms → humanize; wrong order is a compile
    // error, never a silent reorder) on every written pipeline: baked-in
    // `@pattern` transforms, pattern assignment expressions, and track
    // `play:` expressions.
    for pb in &pattern_blocks {
        let span = pb.span.unwrap_or_else(|| Span::new(0, 0));
        validate_transform_order(&pb.transforms, span)?;
        if let crate::ast::PatternBody::Expression(ref expr) = pb.body {
            validate_expr_transform_order(expr, span)?;
        }
    }
    for tb in &track_blocks {
        if let TrackContent::Play(ref expr) = tb.content {
            validate_expr_transform_order(expr, tb.span.unwrap_or_else(|| Span::new(0, 0)))?;
        }
    }

    // Resolve pattern compositions
    let resolved_patterns = resolve_all(&pattern_blocks)?;

    // Collect non-fatal compile warnings.
    let mut compile_warnings: Vec<crate::error::CompileWarning> = Vec::new();

    // Warn when @harmony blocks still use section: (deprecated in v0.5).
    for hb in &harmony_blocks {
        if !hb.sections.is_empty() {
            compile_warnings.push(crate::error::CompileWarning::DeprecatedSection {
                span: hb.span.unwrap_or_else(|| Span::new(0, 0)),
            });
        }
    }

    // Emit tempo track events (track 0)
    let mut events: EventStream = Vec::new();

    // Build BPM lookup for ms→tick conversion that uses effective BPM at each tick.
    let bpm_lookup = build_bpm_lookup(header.bpm_block.as_ref(), header.bpm, &bar_layout);

    if let Some(ref bpm_block) = header.bpm_block {
        // @bpm timeline form — emit per-segment tempo events
        emit_bpm_timeline(&mut events, bpm_block, &bar_layout);
    } else {
        // Default: single tempo event from @bpm
        events.push(TimedEvent {
            tick: 0,
            track: 0,
            event: MidiEvent::Tempo { bpm: header.bpm },
            condition: None,
            step_index: None,
        });
    }

    // Emit time signature event(s).
    // When a @ts timeline is present, emit_ts_timeline handles everything (including
    // the tick-0 event for the first entry). Otherwise emit a single static event.
    if let Some(ref ts_block) = header.ts_block {
        emit_ts_timeline(&mut events, ts_block, &bar_layout);
    } else {
        events.push(TimedEvent {
            tick: 0,
            track: 0,
            event: MidiEvent::TimeSignature {
                numerator: header.ts_numerator,
                denominator: header.ts_denominator,
            },
            condition: None,
            step_index: None,
        });
    }
    if let Some(ref title) = header.title {
        events.push(TimedEvent {
            tick: 0,
            track: 0,
            event: MidiEvent::TrackName {
                name: title.clone(),
            },
            condition: None,
            step_index: None,
        });
    }
    if let Some(seed) = header.seed {
        events.push(TimedEvent {
            tick: 0,
            track: 0,
            event: MidiEvent::TextMeta {
                text: format!("seed:{seed}"),
            },
            condition: None,
            step_index: None,
        });
    }

    // Emit harmony blocks with play=true as voiced chord tracks
    let mut harmony_track_count = 0usize;
    for hb in &harmony_blocks {
        if !hb.play {
            continue;
        }
        harmony_track_count += 1;
        let track_number = harmony_track_count; // after tempo track (0)
        let channel = hb.channel.unwrap_or(1).saturating_sub(1); // 1-indexed to 0-indexed

        // Track name
        events.push(TimedEvent {
            tick: 0,
            track: track_number,
            event: MidiEvent::TrackName {
                name: hb.name.clone().unwrap_or_else(|| "harmony".to_string()),
            },
            condition: None,
            step_index: None,
        });

        // Program change
        if let Some(prog) = hb.program {
            events.push(TimedEvent {
                tick: 0,
                track: track_number,
                event: MidiEvent::ProgramChange {
                    channel,
                    program: prog,
                },
                condition: None,
                step_index: None,
            });
        }

        // Voice chords from the harmony index
        let hb_key = hb.name.clone().unwrap_or_default();
        if let Some(index) = harmony_indices.get(&hb_key) {
            let mut prev_pitches: Option<Vec<u8>> = None;
            for span in index.spans() {
                let (pitches, new_state) = voicing::voice_chord(
                    &span.context.chord,
                    hb.voice,
                    hb.inv,
                    hb.octave,
                    prev_pitches.as_deref(),
                );
                prev_pitches = Some(new_state);

                for &note in &pitches {
                    events.push(TimedEvent {
                        tick: span.start_tick,
                        track: track_number,
                        event: MidiEvent::NoteOn {
                            channel,
                            note,
                            velocity: hb.velocity,
                        },
                        condition: None,
                        step_index: None,
                    });
                    events.push(TimedEvent {
                        tick: span.end_tick,
                        track: track_number,
                        event: MidiEvent::NoteOff { channel, note },
                        condition: None,
                        step_index: None,
                    });
                }
            }
        }
    }

    // Per-track metadata for introspection.
    let mut track_summaries: Vec<TrackSummary> = Vec::new();

    // Compile each track
    for (track_idx, track) in track_blocks.iter().enumerate() {
        let track_number = track_idx + 1 + harmony_track_count; // after tempo + harmony play tracks

        // Resolve the track's step sequence
        let resolved = resolve_track_steps(
            track,
            &pattern_blocks,
            &resolved_patterns,
            header,
            &bar_layout,
        )?;

        // Look up harmony index — explicit follow= or auto-infer single block (Phase 4)
        let explicit_follow = track.follow.is_some();
        let follow_key: Option<String> = track.follow.clone().or(auto_follow_key.clone());
        let harmony_index = if let Some(ref key) = follow_key {
            if explicit_follow {
                Some(harmony_indices.get(key).ok_or_else(|| {
                    CompileError::UndefinedHarmonyBlock {
                        track: track.name.clone(),
                        harmony: key.clone(),
                        span: track.span.unwrap_or_else(|| Span::new(0, 0)),
                    }
                })?)
            } else {
                harmony_indices.get(key)
            }
        } else {
            None
        };
        // Harmony block-level inversion default. Resolved hierarchy: harmony < track < step.
        let harmony_block_inv = follow_key
            .as_deref()
            .and_then(|k| harmony_invs.get(k).copied())
            .unwrap_or(Inversion::Fixed(0));
        // Effective inversion for $chord: track.inv overrides harmony_block_inv unless track
        // is at default (Fixed(0)), in which case harmony_block_inv applies.
        let effective_inv = if track.inv != Inversion::Fixed(0) {
            track.inv
        } else {
            harmony_block_inv
        };

        // Validate: $chord requires follow=
        if harmony_index.is_none() {
            if let Some(line) = resolved.steps.iter().find(|line| {
                line.tokens
                    .iter()
                    .any(|t| matches!(t, StepToken::CurrentChord { .. }))
            }) {
                return Err(CompileError::CurrentChordWithoutFollow {
                    name: track.name.clone(),
                    span: line.span.or(track.span).unwrap_or_else(|| Span::new(0, 0)),
                });
            }
            // Validate: %n requires follow=
            if let Some(line) = resolved.steps.iter().find(|line| {
                line.tokens
                    .iter()
                    .any(|t| matches!(t, StepToken::ChordOrdinal { .. }))
            }) {
                return Err(CompileError::ChordOrdinalWithoutFollow {
                    name: track.name.clone(),
                    span: line.span.or(track.span).unwrap_or_else(|| Span::new(0, 0)),
                });
            }
        }

        // ── Pre-scan: collect per-track warnings ──────────────────────
        {
            let mut first_note_seen = false;
            'scan: for step_line in &resolved.steps {
                for token in &step_line.tokens {
                    let annotations = token_annotations(token);
                    // Skip non-note tokens (rest, tie, subdivision, variant).
                    if matches!(
                        token,
                        StepToken::Rest
                            | StepToken::Tie
                            | StepToken::Subdivision { .. }
                            | StepToken::Variant { .. }
                    ) {
                        continue;
                    }
                    let has_glide = annotations
                        .iter()
                        .any(|a| matches!(a, Annotation::Glide(_)));
                    let prob_val = annotations.iter().find_map(|a| {
                        if let Annotation::Prob(p) = a {
                            Some(*p)
                        } else {
                            None
                        }
                    });

                    let step_span = step_line
                        .span
                        .or(track.span)
                        .unwrap_or_else(|| Span::new(0, 0));

                    // [prob:0.0] — step can never play.
                    if prob_val == Some(0.0) {
                        compile_warnings.push(crate::error::CompileWarning::ProbZeroNeverPlays {
                            track: track.name.clone(),
                            span: step_span,
                        });
                    }
                    // [glide] on first note in pattern.
                    if has_glide && !first_note_seen {
                        compile_warnings.push(crate::error::CompileWarning::GlideOnFirstNote {
                            track: track.name.clone(),
                            span: step_span,
                        });
                    }
                    // [glide] on a drum track.
                    if has_glide && track.is_drum {
                        compile_warnings.push(crate::error::CompileWarning::GlideOnDrumTrack {
                            track: track.name.clone(),
                            span: step_span,
                        });
                        break 'scan; // only warn once per track
                    }

                    first_note_seen = true;
                }
            }
        }

        // Look up drummap
        let drummap = if track.is_drum {
            if let Some(ref dm_name) = track.drummap {
                drummap_registry.get(dm_name)
            } else {
                drummap_registry.get("")
            }
        } else {
            None
        };

        // Emit program change
        let channel = track.channel - 1; // Convert 1-16 to 0-15
        if let Some(prog) = track.program {
            events.push(TimedEvent {
                tick: 0,
                track: track_number,
                event: MidiEvent::ProgramChange {
                    channel,
                    program: prog,
                },
                condition: None,
                step_index: None,
            });
        }

        // Track name
        events.push(TimedEvent {
            tick: 0,
            track: track_number,
            event: MidiEvent::TrackName {
                name: track.name.clone(),
            },
            condition: None,
            step_index: None,
        });

        // Calculate unit ticks, scaled by rate
        let (unit_num, unit_den) = resolved.unit;
        let base_unit_ticks = compute_unit_ticks(header.ppq, unit_num, unit_den);
        let rate = track.rate.unwrap_or(1.0);
        let unit_ticks = if rate != 1.0 {
            (base_unit_ticks as f64 / rate).round() as u64
        } else {
            base_unit_ticks
        };

        // Compute track-level shift in ticks
        let track_shift_ticks = track
            .shift
            .as_ref()
            .map(|tv| resolve_timing_value(tv, unit_ticks, header.ppq, &bpm_lookup, 0))
            .unwrap_or(0);
        let track_lshift_ticks = track
            .lshift
            .as_ref()
            .map(|tv| resolve_timing_value(tv, unit_ticks, header.ppq, &bpm_lookup, 0))
            .unwrap_or(0);

        // Precompute cumulative step boundaries for segment lookup.
        // Each entry is (cumulative_step_end, defaults).
        let segment_boundaries: Vec<(usize, crate::pattern::SegmentDefaults)> = {
            let mut boundaries = Vec::new();
            let mut cumulative = 0usize;
            for seg in &resolved.segment_defaults {
                cumulative += seg.step_count;
                boundaries.push((cumulative, *seg));
            }
            boundaries
        };

        // Precompute cumulative pattern instance boundaries for PatternBoundary emission.
        // Each entry is the cumulative step index where a new instance begins.
        let pattern_instance_starts: Vec<(usize, String)> = {
            let mut starts = Vec::new();
            let mut cumulative = 0usize;
            for inst in &resolved.pattern_instances {
                starts.push((cumulative, inst.name.clone()));
                cumulative += inst.step_count;
            }
            starts
        };

        // Collect expressive transforms from play expression (needed by both
        // TrackSummary and the emission loop below).
        let play_transforms = collect_play_transforms(&track.content);

        // Build TrackSummary for introspection.
        {
            let start_offset =
                track.start.unwrap_or(1).saturating_sub(1) as u64 * bar_layout.ticks_for_bar(1);
            let transform_names: Vec<String> =
                play_transforms.iter().map(transform_call_name).collect();
            let mut pattern_instances = Vec::new();
            let mut step_tick = start_offset;
            for inst in &resolved.pattern_instances {
                let inst_start = step_tick;
                let inst_end = step_tick + inst.step_count as u64 * unit_ticks;
                let (start_bar, _) = bar_layout.bar_at_tick(inst_start);
                let (end_bar, _) = if inst_end > 0 {
                    bar_layout.bar_at_tick(inst_end.saturating_sub(1))
                } else {
                    (start_bar, 0)
                };
                pattern_instances.push(TrackPatternInstance {
                    pattern_name: inst.name.clone(),
                    start_tick: inst_start,
                    end_tick: inst_end,
                    start_bar,
                    end_bar,
                    transforms: transform_names.clone(),
                });
                step_tick = inst_end;
            }
            track_summaries.push(TrackSummary {
                name: track.name.clone(),
                channel,
                program: track.program,
                follow: track.follow.clone(),
                voice: track.voice,
                inv: effective_inv,
                is_drum: track.is_drum,
                patterns: pattern_instances,
                span: track.span,
            });
        }

        // Seed for humanize/vary/prob/evolve. An explicit `@track seed=N`
        // wins verbatim; otherwise the per-track seed is derived from the
        // global seed and the track's declaration index via FNV-1a
        // (spec §11.1:
        // `track_seed = fnv1a(global_seed, track_index)`) so tracks do not
        // mutate in lockstep.
        let global_seed = header.resolved_seed.or(header.seed).unwrap_or(0);
        let track_seed = track
            .seed
            .unwrap_or_else(|| transform::fnv1a_derive(global_seed, track_idx));
        let mut rng_state = transform::seed_state(track_seed);

        // Voice leading state for inv=auto
        let mut prev_pitches: Option<Vec<u8>> = None;

        // Active-note state for tie tracking: NoteOffs are deferred until
        // each note's full extent is known (`~` extends notes across steps
        // and across soft pattern boundaries). Insertion-ordered for
        // deterministic NoteOff emission.
        let mut tie_state = TieState::new(
            resolved
                .pattern_instances
                .first()
                .map(|inst| inst.name.clone())
                .unwrap_or_else(|| track.name.clone()),
        );

        // Boundary before each pattern-instance start step (instances 1..):
        // `resolved.boundaries[i-1]` sits between instance i-1 and i. Used
        // for tie carry-over: hard boundaries cut ties, soft boundaries
        // carry the last sounding notes into the next instance.
        let instance_boundaries: Vec<(usize, Boundary)> = {
            let mut v: Vec<(usize, Boundary)> = Vec::new();
            let mut cumulative = 0usize;
            for (i, inst) in resolved.pattern_instances.iter().enumerate() {
                if i > 0 {
                    if let Some(&b) = resolved.boundaries.get(i - 1) {
                        match v.last_mut() {
                            // Zero-step instances can collapse boundaries onto
                            // the same step index; a hard cut dominates.
                            Some((idx, existing)) if *idx == cumulative => {
                                if b == Boundary::Hard {
                                    *existing = Boundary::Hard;
                                }
                            }
                            _ => v.push((cumulative, b)),
                        }
                    }
                }
                cumulative += inst.step_count;
            }
            v
        };

        let expressive_offsets = compute_expressive_offsets(
            resolved.steps.len(),
            unit_ticks,
            &play_transforms,
            header.ppq,
            &bpm_lookup,
        );

        // Per-step scoped emission transforms (spec §10.3/§10.5): pipeline
        // swing/humanize/vary recorded on pattern instances apply only to
        // that instance's steps. Pipeline swing takes precedence over
        // track-level swing= for its pattern; track-level swing still
        // applies to everything else on the track.
        let track_swing = compute_swing_params(track, header);
        let step_count = resolved.steps.len();
        let mut step_swing: Vec<(u64, i64)> = Vec::with_capacity(step_count);
        let mut step_humanize: Vec<Option<(TimingValue, f64)>> = Vec::with_capacity(step_count);
        let mut step_vary: Vec<Option<f64>> = Vec::with_capacity(step_count);
        for inst in &resolved.pattern_instances {
            let mut inst_swing: Option<(u64, i64)> = None;
            let mut inst_humanize: Option<(TimingValue, f64)> = None;
            let mut inst_vary: Option<f64> = None;
            // Last entry of each kind wins (outermost pipe application).
            for t in &inst.emission_transforms {
                match t {
                    crate::ast::TransformCall::Swing(ratio, num, den) => {
                        let su_ticks = compute_unit_ticks(header.ppq, *num, *den);
                        let shift = (su_ticks as f64 * (ratio - 0.5) * 2.0).round() as i64;
                        inst_swing = Some((su_ticks, shift));
                    }
                    crate::ast::TransformCall::Humanize(tv, intensity) => {
                        inst_humanize = Some((tv.clone(), *intensity));
                    }
                    crate::ast::TransformCall::Vary(p) => {
                        inst_vary = Some(*p);
                    }
                    _ => {}
                }
            }
            for _ in 0..inst.step_count {
                step_swing.push(inst_swing.unwrap_or(track_swing));
                step_humanize.push(inst_humanize.clone());
                step_vary.push(inst_vary);
            }
        }
        // Defensive: instance step counts should always cover all steps.
        step_swing.resize(step_count, track_swing);
        step_humanize.resize(step_count, None);
        step_vary.resize(step_count, None);
        step_swing.truncate(step_count);
        step_humanize.truncate(step_count);
        step_vary.truncate(step_count);

        // Euclidean gate: compute per-step gate mask
        let euclid_gate_mask = {
            let euclid = play_transforms.iter().find_map(|t| {
                if let crate::ast::TransformCall::EuclidGate(pulses, steps) = t {
                    Some((*pulses, *steps))
                } else {
                    None
                }
            });
            if let Some((pulses, euclid_steps)) = euclid {
                compute_euclid_gate_mask(resolved.steps.len(), pulses, euclid_steps)
            } else {
                vec![true; resolved.steps.len()]
            }
        };

        // Evolve transform: compute per-step pitch offsets from shift register
        let evolve_offsets = {
            let evolve_toggle = play_transforms.iter().find_map(|t| {
                if let crate::ast::TransformCall::Evolve(toggle) = t {
                    Some(*toggle)
                } else {
                    None
                }
            });
            if let Some(toggle) = evolve_toggle {
                compute_evolve_offsets(resolved.steps.len(), toggle, &mut rng_state)
            } else {
                vec![0i8; resolved.steps.len()]
            }
        };

        // Arp transform: extract config for emission-phase arpeggiation
        let arp_config: Option<(ArpPattern, u32, u32, u32)> =
            play_transforms.iter().find_map(|t| {
                if let crate::ast::TransformCall::Arp {
                    pattern,
                    rate,
                    octaves,
                } = t
                {
                    Some((pattern.clone(), rate.0, rate.1, *octaves))
                } else {
                    None
                }
            });

        // Compute start offset for start= parameter (1-indexed bar number)
        let start_offset = track
            .start
            .map(|b| bar_layout.bar_start_tick(b))
            .unwrap_or(0);

        // Precompute per-step tick positions and effective unit ticks accounting for per-segment rate
        let (step_tick_positions, step_effective_unit_ticks): (Vec<u64>, Vec<u64>) = {
            let mut positions = Vec::with_capacity(resolved.steps.len());
            let mut eff_units = Vec::with_capacity(resolved.steps.len());
            let mut cumulative_tick = 0u64;
            let mut seg_idx = 0usize;
            let mut steps_in_seg = 0usize;
            for _step_idx in 0..resolved.steps.len() {
                // Advance segment if needed
                while seg_idx < segment_boundaries.len()
                    && steps_in_seg >= segment_boundaries[seg_idx].1.step_count
                {
                    steps_in_seg = 0;
                    seg_idx += 1;
                }
                let seg_rate = if seg_idx < segment_boundaries.len() {
                    segment_boundaries[seg_idx].1.rate
                } else {
                    1.0
                };
                let effective_unit = if seg_rate != 1.0 {
                    (unit_ticks as f64 / seg_rate).round() as u64
                } else {
                    unit_ticks
                };
                positions.push(cumulative_tick + start_offset);
                eff_units.push(effective_unit);
                cumulative_tick += effective_unit;
                steps_in_seg += 1;
            }
            (positions, eff_units)
        };

        // Total track extent: end of the last step (used by the BarMarker
        // sweep below and the echo clamp after emission).
        let total_ticks = if let Some(&last_pos) = step_tick_positions.last() {
            let last_unit = step_effective_unit_ticks
                .last()
                .copied()
                .unwrap_or(unit_ticks);
            last_pos + last_unit
        } else {
            start_offset
        };

        // Emit BarMarkers for every bar boundary in the track's tick range,
        // directly from BarLayout — decoupled from step alignment. A track
        // whose rate never re-aligns to the bar grid (e.g. rate=1.3) must
        // still produce markers, or RT hot-swap and bar seeking are
        // silently disabled. For step-aligned tracks this emits exactly
        // the markers the old per-step check produced (same ticks, same
        // count): every bar start in [start_offset, total_ticks).
        {
            let (first_bar, _) = bar_layout.bar_at_tick(start_offset);
            let mut bar = first_bar;
            loop {
                let bar_start = bar_layout.bar_start_tick(bar);
                if bar_start >= total_ticks {
                    break;
                }
                if bar_start >= start_offset {
                    events.push(TimedEvent {
                        tick: bar_start,
                        track: track_number,
                        event: MidiEvent::BarMarker { bar },
                        condition: None,
                        step_index: None,
                    });
                }
                bar += 1;
            }
        }

        // Iterate steps
        for (step_idx, step_line) in resolved.steps.iter().enumerate() {
            let base_step_start = step_tick_positions[step_idx];
            let effective_unit_ticks = step_effective_unit_ticks[step_idx];

            // Pattern-boundary handling for tie carry-over. At a hard
            // boundary (`*`, `>>`) sounding notes are settled at their
            // scheduled offs and the tie context resets — a leading `~` in
            // the next instance is a TieWithNoPriorNote error. At a soft
            // boundary (`*~`, `~>>`) sounding notes are marked carried:
            // they sustain into the next instance up to its first onset
            // (legato), and a leading `~` extends them instead.
            if let Some(&(_, boundary)) = instance_boundaries.iter().find(|(s, _)| *s == step_idx) {
                match boundary {
                    Boundary::Hard => {
                        settle_active_notes(
                            &mut events,
                            &mut tie_state,
                            track_number,
                            channel,
                            None,
                            None,
                            None,
                        );
                        tie_state.tie_legal = false;
                    }
                    Boundary::Soft => {
                        for an in &mut tie_state.active {
                            an.carried = true;
                        }
                    }
                }
            }
            tie_state.current_span = step_line.span;

            // Look up per-segment defaults for this step, falling back to track defaults
            let (default_vel, default_gate, default_octave) = segment_boundaries
                .iter()
                .find(|(end, _)| step_idx < *end)
                .map(|(_, seg)| (seg.velocity, seg.gate, seg.octave))
                .unwrap_or((track.velocity, track.gate, track.octave));

            // Apply swing: shift even-numbered subdivisions of the swing
            // unit. Per-step params: pipeline swing for this step's pattern
            // instance if present, else track-level swing.
            let (swing_unit_ticks, swing_shift) = step_swing[step_idx];
            let swung_start = if swing_unit_ticks > 0 && swing_shift != 0 {
                let grid_pos = base_step_start / swing_unit_ticks;
                if grid_pos % 2 == 1 {
                    (base_step_start as i64 + swing_shift).max(0) as u64
                } else {
                    base_step_start
                }
            } else {
                base_step_start
            };

            // Apply expressive transform offset
            let (expr_offset, expr_vel_scale) = expressive_offsets[step_idx];
            let step_start = (swung_start as i64 + expr_offset).max(0) as u64;

            // Emit PatternBoundary at pattern instance boundaries
            if let Some((_, ref pat_name)) = pattern_instance_starts
                .iter()
                .find(|(start_step, _)| *start_step == step_idx)
            {
                tie_state.current_pattern = pat_name.clone();
                events.push(TimedEvent {
                    tick: base_step_start,
                    track: track_number,
                    event: MidiEvent::PatternBoundary {
                        track: track_number,
                        pattern_name: pat_name.clone(),
                    },
                    condition: None,
                    step_index: None,
                });
            }

            // Apply expressive velocity scale
            let step_vel = if expr_vel_scale != 1.0 {
                ((default_vel as f64 * expr_vel_scale).round() as u8).clamp(1, 127)
            } else {
                default_vel
            };

            // Euclidean gate: suppress gated steps (emit as rest)
            if !euclid_gate_mask[step_idx] {
                settle_active_notes(
                    &mut events,
                    &mut tie_state,
                    track_number,
                    channel,
                    Some(step_start),
                    Some(step_start),
                    None,
                );
                // Keep tie-legality consistent with what the unmasked step
                // would have produced, so euclid gating never turns a valid
                // tie into a TieWithNoPriorNote error (or vice versa).
                if step_line
                    .tokens
                    .iter()
                    .any(|t| !matches!(t, StepToken::Rest | StepToken::Tie))
                {
                    tie_state.tie_legal = true;
                } else if step_line
                    .tokens
                    .iter()
                    .any(|t| matches!(t, StepToken::Rest))
                {
                    tie_state.tie_legal = false;
                }
                continue;
            }

            // Build per-step musical context
            let ctx = MusicalContext {
                track_number,
                channel,
                default_vel: step_vel,
                default_gate,
                default_octave,
                track_shift_ticks,
                track_lshift_ticks,
                harmony_index,
                drummap,
                track,
                effective_inv,
                scale_timeline: &scale_timeline,
                header,
                bpm_lookup: &bpm_lookup,
                bar_layout: &bar_layout,
                humanize: step_humanize[step_idx].clone(),
                vary: step_vary[step_idx],
            };

            // Process each token in the step line (simultaneous notes)
            emit_step_line(
                &mut events,
                step_line,
                step_start,
                effective_unit_ticks,
                &ctx,
                &mut prev_pitches,
                &mut tie_state,
                &mut rng_state,
                evolve_offsets[step_idx],
                arp_config.as_ref(),
                Some(step_idx),
            )?;
        }

        // Emit the deferred note-offs for any still-sounding notes (e.g. a
        // trailing tie chain) at their scheduled off ticks.
        settle_active_notes(
            &mut events,
            &mut tie_state,
            track_number,
            channel,
            None,
            None,
            None,
        );

        // Velocity curve transform: shape velocity across NoteOn events
        if let Some((wave, min_v, max_v, repeat)) = play_transforms.iter().find_map(|t| {
            if let crate::ast::TransformCall::VelCurve(w, mn, mx, r) = t {
                Some((w.clone(), *mn, *mx, *r))
            } else {
                None
            }
        }) {
            let note_ons: Vec<usize> = events
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    e.track == track_number && matches!(e.event, MidiEvent::NoteOn { .. })
                })
                .map(|(i, _)| i)
                .collect();
            let count = note_ons.len();
            if count > 0 {
                for (idx, &ev_idx) in note_ons.iter().enumerate() {
                    let t = compute_wave_value(&wave, idx, count, repeat, &mut rng_state);
                    let vel = (min_v as f64 + t * (max_v as f64 - min_v as f64)).round() as u8;
                    let vel = vel.clamp(1, 127);
                    if let MidiEvent::NoteOn {
                        ref mut velocity, ..
                    } = events[ev_idx].event
                    {
                        *velocity = vel;
                    }
                }
            }
        }

        // Gate curve transform: shape gate across NoteOn events
        if let Some((wave, min_g, max_g, repeat)) = play_transforms.iter().find_map(|t| {
            if let crate::ast::TransformCall::GateCurve(w, mn, mx, r) = t {
                Some((w.clone(), *mn, *mx, *r))
            } else {
                None
            }
        }) {
            // Collect NoteOn indices, ticks, and pairing keys.
            let note_ons: Vec<(usize, u64, u8, u8, Option<usize>)> = events
                .iter()
                .enumerate()
                .filter_map(|(i, e)| {
                    if e.track == track_number {
                        if let MidiEvent::NoteOn {
                            channel: ch,
                            note: n,
                            ..
                        } = &e.event
                        {
                            return Some((i, e.tick, *ch, *n, e.step_index));
                        }
                    }
                    None
                })
                .collect();
            let count = note_ons.len();
            if count > 0 {
                // Retarget each note's OWN NoteOff (paired via step_index —
                // see find_paired_note_off), never the first matching off in
                // the stream, and size the gate from the owning step's
                // EFFECTIVE unit (per-segment rate applied), not the
                // track-level unit.
                let mut claimed = vec![false; events.len()];
                for (idx, &(_, on_tick, ch, n, si)) in note_ons.iter().enumerate() {
                    let t = compute_wave_value(&wave, idx, count, repeat, &mut rng_state);
                    let new_gate = min_g + t * (max_g - min_g);
                    if let Some(off_idx) =
                        find_paired_note_off(&events, &claimed, track_number, ch, n, si, on_tick)
                    {
                        claimed[off_idx] = true;
                        let step_dur = si
                            .and_then(|s| step_effective_unit_ticks.get(s))
                            .copied()
                            .unwrap_or(unit_ticks);
                        let new_dur = (step_dur as f64 * new_gate).round() as u64;
                        events[off_idx].tick = on_tick + new_dur.max(1);
                    }
                }
            }
        }

        // Scale lock transform: snap or filter pitches to a scale
        if let Some((scale_name, root_pc, snap_mode)) = play_transforms.iter().find_map(|t| {
            if let crate::ast::TransformCall::ScaleLock(s, r, m) = t {
                Some((s.clone(), *r, m.clone()))
            } else {
                None
            }
        }) {
            // Resolve scale intervals: explicit scale= or inherit from @scale timeline bar 1
            let (sl_base_ivs, sl_base_root) = scale_timeline.context_at_bar(1);
            let scale_intervals = if let Some(ref name) = scale_name {
                crate::harmony::lookup_mode(name).unwrap_or(sl_base_ivs)
            } else {
                sl_base_ivs
            };
            let root = root_pc.unwrap_or(sl_base_root);

            match snap_mode {
                crate::ast::SnapMode::Filter => {
                    // Remove each out-of-scale NoteOn together with ITS OWN
                    // paired NoteOff (via step_index — see
                    // find_paired_note_off). The previous retain predicate
                    // deleted every later NoteOff at the filtered pitch,
                    // orphaning other (in-scale) notes at the same pitch.
                    let mut remove = vec![false; events.len()];
                    for i in 0..events.len() {
                        if events[i].track != track_number {
                            continue;
                        }
                        let (ch, n) = match &events[i].event {
                            MidiEvent::NoteOn { channel, note, .. }
                                if !crate::voicing::is_in_scale(*note, scale_intervals, root) =>
                            {
                                (*channel, *note)
                            }
                            _ => continue,
                        };
                        remove[i] = true;
                        if let Some(off_idx) = find_paired_note_off(
                            &events,
                            &remove,
                            track_number,
                            ch,
                            n,
                            events[i].step_index,
                            events[i].tick,
                        ) {
                            remove[off_idx] = true;
                        }
                    }
                    let mut keep = remove.iter();
                    events.retain(|_| !*keep.next().unwrap_or(&false));
                }
                crate::ast::SnapMode::Down => {
                    for event in events.iter_mut() {
                        if event.track == track_number {
                            if let MidiEvent::NoteOn { ref mut note, .. } = event.event {
                                *note = crate::voicing::snap_to_scale_down(
                                    *note,
                                    scale_intervals,
                                    root,
                                );
                            }
                            if let MidiEvent::NoteOff { ref mut note, .. } = event.event {
                                *note = crate::voicing::snap_to_scale_down(
                                    *note,
                                    scale_intervals,
                                    root,
                                );
                            }
                        }
                    }
                }
                crate::ast::SnapMode::Up => {
                    for event in events.iter_mut() {
                        if event.track == track_number {
                            if let MidiEvent::NoteOn { ref mut note, .. } = event.event {
                                *note =
                                    crate::voicing::snap_to_scale_up(*note, scale_intervals, root);
                            }
                            if let MidiEvent::NoteOff { ref mut note, .. } = event.event {
                                *note =
                                    crate::voicing::snap_to_scale_up(*note, scale_intervals, root);
                            }
                        }
                    }
                }
            }
        }

        // Echo transform: emit copies of NoteOn/NoteOff at rate intervals
        if let Some((rate_num, rate_den, repeats, decay)) = play_transforms.iter().find_map(|t| {
            if let crate::ast::TransformCall::Echo(rn, rd, rep, dec) = t {
                Some((*rn, *rd, *rep, *dec))
            } else {
                None
            }
        }) {
            let echo_interval = compute_unit_ticks(header.ppq, rate_num, rate_den);
            let mut echo_events = Vec::new();

            // Pattern-instance tick ranges for the echo clamp. Per
            // spec §10.5, echo copies are clamped at the PATTERN boundary —
            // no bleed into the next pattern instance. Each source note's
            // echoes are clamped at the end of the instance that produced
            // it, not at the whole-track end. Ranges come from the same
            // cumulative step data as PatternBoundary emission; zero-step
            // instances are skipped.
            let instance_ranges: Vec<(u64, u64)> = pattern_instance_starts
                .iter()
                .enumerate()
                .filter_map(|(i, (start_step, _))| {
                    let start_tick = step_tick_positions.get(*start_step).copied()?;
                    let end_step = pattern_instance_starts
                        .get(i + 1)
                        .map(|(s, _)| *s)
                        .unwrap_or(step_tick_positions.len());
                    let end_tick = step_tick_positions
                        .get(end_step)
                        .copied()
                        .unwrap_or(total_ticks);
                    (end_tick > start_tick).then_some((start_tick, end_tick))
                })
                .collect();
            let instance_end_at = |tick: u64| -> u64 {
                instance_ranges
                    .iter()
                    .rev()
                    .find(|(s, _)| *s <= tick)
                    .or(instance_ranges.first())
                    .map(|(_, e)| *e)
                    .unwrap_or(total_ticks)
            };

            // Collect NoteOn events for this track
            let note_ons: Vec<_> = events
                .iter()
                .filter(|e| e.track == track_number && matches!(e.event, MidiEvent::NoteOn { .. }))
                .cloned()
                .collect();

            for note_on in &note_ons {
                if let MidiEvent::NoteOn {
                    channel: ch,
                    note: n,
                    velocity: v,
                } = &note_on.event
                {
                    // Find the corresponding NoteOff to get note duration
                    let note_dur = events
                        .iter()
                        .filter(|e| e.track == track_number && e.tick > note_on.tick)
                        .find_map(|e| {
                            if let MidiEvent::NoteOff {
                                channel: c,
                                note: nn,
                            } = &e.event
                            {
                                if *c == *ch && *nn == *n {
                                    Some(e.tick - note_on.tick)
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        })
                        .unwrap_or(echo_interval);

                    // Clamp bound: end of the pattern instance that
                    // produced this source note.
                    let clamp_end = instance_end_at(note_on.tick);
                    let mut current_vel = *v as f64;
                    for k in 1..=repeats {
                        current_vel *= decay;
                        let echo_tick = note_on.tick + k as u64 * echo_interval;
                        // Clamp at the pattern-instance boundary
                        if echo_tick >= clamp_end {
                            break;
                        }
                        let vel = (current_vel.round() as u8).clamp(1, 127);
                        let echo_off = (echo_tick + note_dur).min(clamp_end);
                        echo_events.push(TimedEvent {
                            tick: echo_tick,
                            track: track_number,
                            event: MidiEvent::NoteOn {
                                channel: *ch,
                                note: *n,
                                velocity: vel,
                            },
                            condition: note_on.condition.clone(),
                            step_index: note_on.step_index,
                        });
                        echo_events.push(TimedEvent {
                            tick: echo_off,
                            track: track_number,
                            event: MidiEvent::NoteOff {
                                channel: *ch,
                                note: *n,
                            },
                            condition: note_on.condition.clone(),
                            step_index: note_on.step_index,
                        });
                    }
                }
            }
            events.extend(echo_events);
        }
    }

    // Sort the event stream
    sort_event_stream(&mut events);

    Ok(CompileOutput {
        events,
        ppq: header.ppq,
        warnings: compile_warnings,
        tracks: track_summaries,
        program: None,
    })
}

// ── Track Step Resolution ──────────────────────────────────────────────

/// Resolve a track's content to a flat ResolvedPattern.
/// The returned ResolvedPattern contains per-segment defaults from source patterns.
fn resolve_track_steps(
    track: &TrackBlock,
    _pattern_blocks: &[PatternBlock],
    resolved_patterns: &HashMap<String, ResolvedPattern>,
    header: &GlobalHeader,
    bar_layout: &BarLayout,
) -> CompileResult<ResolvedPattern> {
    match &track.content {
        TrackContent::Play(expr) => {
            // Apply @bars fill: if the expression is a bare Ref (no explicit repeat)
            // and @bars is Count(n), wrap in Repeat to fill n bars.
            let effective_expr = apply_bars_fill(expr, header, resolved_patterns, bar_layout);
            // Resolve the play expression using already-resolved patterns.
            // Per-segment defaults are carried through from pattern resolution.
            resolve_play_expr(&effective_expr, resolved_patterns)
        }
        TrackContent::Steps(step_lines) => {
            // Inline steps — wrap as a ResolvedPattern with no segment defaults
            // (track-level defaults will be used)
            let unit = track.unit.unwrap_or((1, 4));
            Ok(ResolvedPattern {
                unit,
                steps: step_lines.clone(),
                boundaries: Vec::new(),
                segment_defaults: Vec::new(),
                pattern_instances: vec![crate::pattern::PatternInstance {
                    name: track.name.clone(),
                    step_count: step_lines.len(),
                    emission_transforms: Vec::new(),
                }],
            })
        }
    }
}

/// Walk through Transform wrappers to find the innermost Ref.
/// Returns None for Repeat, Concat, RepeatSoft, ConcatSoft (these have explicit structure).
fn innermost_ref(expr: &PatternExpr) -> Option<(&str, &Option<f64>)> {
    match expr {
        PatternExpr::Ref { name, rate } => Some((name.as_str(), rate)),
        PatternExpr::Transform { pattern, .. } => innermost_ref(pattern),
        _ => None,
    }
}

/// Apply `@bars N` fill logic to a play expression.
///
/// If the expression is a bare `Ref` (possibly wrapped in transforms, but with no
/// explicit repeat/concat) and `@bars` is `Count(n)`, computes how many iterations
/// are needed to fill `n` bars and wraps the entire expression in a `Repeat`.
/// Expressions with explicit repeat counts or concatenation are left unchanged.
fn apply_bars_fill<'a>(
    expr: &'a PatternExpr,
    header: &GlobalHeader,
    resolved: &HashMap<String, ResolvedPattern>,
    bar_layout: &BarLayout,
) -> std::borrow::Cow<'a, PatternExpr> {
    let target_bars = match header.bars {
        Some(BarsSetting::Count(n)) => n,
        _ => return std::borrow::Cow::Borrowed(expr),
    };

    // Only apply to bare Ref or Ref with transforms (no explicit repeat/concat)
    let (name, _rate) = match innermost_ref(expr) {
        Some(pair) => pair,
        None => return std::borrow::Cow::Borrowed(expr),
    };

    // Look up the resolved pattern to compute how many bars it occupies
    let rp = match resolved.get(name) {
        Some(rp) => rp,
        None => return std::borrow::Cow::Borrowed(expr),
    };

    // Compute total ticks for one iteration of this pattern
    let (unit_num, unit_den) = rp.unit;
    let step_ticks = header.ppq as u64 * 4 * unit_num as u64 / unit_den as u64;
    let pattern_total_ticks = step_ticks * rp.steps.len() as u64;

    if pattern_total_ticks == 0 {
        return std::borrow::Cow::Borrowed(expr);
    }

    // Compute total ticks for target_bars bars
    let mut target_ticks: u64 = 0;
    for bar in 1..=target_bars {
        target_ticks += bar_layout.ticks_for_bar(bar);
    }

    // Compute repeat count: ceil(target_ticks / pattern_total_ticks)
    let count = target_ticks.div_ceil(pattern_total_ticks) as u32;
    if count <= 1 {
        return std::borrow::Cow::Borrowed(expr);
    }

    std::borrow::Cow::Owned(PatternExpr::Repeat {
        pattern: Box::new(expr.clone()),
        count,
    })
}

/// Resolve a play: expression to a ResolvedPattern.
/// This handles repeat/concat/transform by operating on already-resolved patterns.
fn resolve_play_expr(
    expr: &PatternExpr,
    resolved: &HashMap<String, ResolvedPattern>,
) -> CompileResult<ResolvedPattern> {
    match expr {
        PatternExpr::Ref { name, rate } => {
            let mut rp =
                resolved
                    .get(name)
                    .cloned()
                    .ok_or_else(|| CompileError::UndefinedPattern {
                        track: String::new(),
                        pattern: name.clone(),
                        span: Span::new(0, 0),
                    })?;
            // Apply per-reference rate to segment defaults
            if let Some(r) = rate {
                for seg in &mut rp.segment_defaults {
                    seg.rate *= r;
                }
            }
            Ok(rp)
        }
        PatternExpr::Repeat { pattern, count } => {
            let inner = resolve_play_expr(pattern, resolved)?;
            repeat_resolved(&inner, *count, Boundary::Hard)
        }
        PatternExpr::RepeatSoft { pattern, count } => {
            let inner = resolve_play_expr(pattern, resolved)?;
            repeat_resolved(&inner, *count, Boundary::Soft)
        }
        PatternExpr::Concat { left, right } => {
            let l = resolve_play_expr(left, resolved)?;
            let r = resolve_play_expr(right, resolved)?;
            concat_resolved(l, r, Boundary::Hard)
        }
        PatternExpr::ConcatSoft { left, right } => {
            let l = resolve_play_expr(left, resolved)?;
            let r = resolve_play_expr(right, resolved)?;
            concat_resolved(l, r, Boundary::Soft)
        }
        PatternExpr::Transform { pattern, transform } => {
            let inner = resolve_play_expr(pattern, resolved)?;
            match transform {
                // Scoped emission transforms: recorded per pattern instance
                // by apply_step_transform so they apply only to the
                // pattern(s) they are piped from (spec §10.3/§10.5), then
                // applied during event emission.
                crate::ast::TransformCall::Humanize(..)
                | crate::ast::TransformCall::Vary(..)
                | crate::ast::TransformCall::Swing(..) => {
                    crate::pattern::apply_step_transform(inner, transform, resolved)
                }
                // Track-wide event-level transforms: pass through (applied
                // during event emission / stream post-processing).
                crate::ast::TransformCall::Rubato(..)
                | crate::ast::TransformCall::Ritardando(..)
                | crate::ast::TransformCall::Accelerando(..)
                | crate::ast::TransformCall::Agogic(..)
                | crate::ast::TransformCall::Breathe(..)
                | crate::ast::TransformCall::Swell(..)
                | crate::ast::TransformCall::Phrase(..)
                | crate::ast::TransformCall::Evolve(..)
                | crate::ast::TransformCall::EuclidGate(..)
                | crate::ast::TransformCall::Echo(..)
                | crate::ast::TransformCall::VelCurve(..)
                | crate::ast::TransformCall::GateCurve(..)
                | crate::ast::TransformCall::ScaleLock(..) => Ok(inner),
                // Step-level transforms: apply now
                _ => crate::pattern::apply_step_transform(inner, transform, resolved),
            }
        }
    }
}

/// Repeat a resolved pattern N times with the given boundary type.
fn repeat_resolved(
    pattern: &ResolvedPattern,
    count: u32,
    boundary: Boundary,
) -> CompileResult<ResolvedPattern> {
    if count == 0 {
        return Ok(ResolvedPattern {
            unit: pattern.unit,
            steps: Vec::new(),
            boundaries: Vec::new(),
            segment_defaults: Vec::new(),
            pattern_instances: Vec::new(),
        });
    }
    let mut steps = Vec::new();
    let mut boundaries = Vec::new();
    let mut segment_defaults = Vec::new();
    let mut pattern_instances = Vec::new();
    for i in 0..count {
        if i > 0 {
            boundaries.push(boundary);
        }
        steps.extend(pattern.steps.iter().cloned());
        boundaries.extend(pattern.boundaries.iter().copied());
        segment_defaults.extend(pattern.segment_defaults.iter().copied());
        pattern_instances.extend(pattern.pattern_instances.iter().cloned());
    }
    Ok(ResolvedPattern {
        unit: pattern.unit,
        steps,
        boundaries,
        segment_defaults,
        pattern_instances,
    })
}

/// Concatenate two resolved patterns.
fn concat_resolved(
    mut lhs: ResolvedPattern,
    rhs: ResolvedPattern,
    boundary: Boundary,
) -> CompileResult<ResolvedPattern> {
    // Validate unit compatibility (same check as pattern resolver)
    if lhs.unit != rhs.unit {
        return Err(CompileError::UnitMismatch {
            span: Span::new(0, 0),
        });
    }
    if !lhs.steps.is_empty() && !rhs.steps.is_empty() {
        lhs.boundaries.push(boundary);
    }
    lhs.steps.extend(rhs.steps);
    lhs.boundaries.extend(rhs.boundaries);
    lhs.segment_defaults.extend(rhs.segment_defaults);
    lhs.pattern_instances.extend(rhs.pattern_instances);
    Ok(lhs)
}

// ── Timing Helpers ─────────────────────────────────────────────────────

/// Compute the number of ticks per step unit.
/// `unit_ticks = ppq * 4 * (numerator / denominator)`
fn compute_unit_ticks(ppq: u32, unit_num: u32, unit_den: u32) -> u64 {
    // ppq * 4 * num / den, using integer math with round-half-up
    let numerator = ppq as u64 * 4 * unit_num as u64;
    round_div(numerator, unit_den as u64)
}

/// Compute ticks per bar from header.
///
/// The compile pipeline itself now goes through `BarLayout` (variable time
/// signatures); this scalar helper survives for the unit tests that pin the
/// 4/4 and 3/4 bar-tick math.
#[cfg(test)]
fn compute_bar_ticks(header: &GlobalHeader) -> u64 {
    // A bar in time signature N/D has N beats, each beat is ppq * 4 / D ticks
    let beat_ticks = header.ppq as u64 * 4 / header.ts_denominator as u64;
    beat_ticks * header.ts_numerator as u64
}

/// Emit tempo events from a `@bpm` timeline block.
///
/// Each `BpmEntry` with `bars: Some(n)` spans `n` bars. `ramp=` entries interpolate
/// from the previous BPM to this entry's BPM using 8 sample points per bar.
/// The last entry (bars=None) holds its BPM forever (just emits a single event at its start).
fn emit_bpm_timeline(events: &mut EventStream, bpm_block: &BpmBlock, bar_layout: &BarLayout) {
    let mut current_tick: u64 = 0;
    let mut current_bar: u32 = 1;
    let mut prev_bpm: Option<f64> = None;

    for entry in &bpm_block.entries {
        let num_bars = entry.bars.unwrap_or(1);

        match &entry.ramp {
            None => {
                // Instant BPM change: emit at segment start if changed.
                if prev_bpm != Some(entry.bpm) {
                    events.push(TimedEvent {
                        tick: current_tick,
                        track: 0,
                        event: MidiEvent::Tempo { bpm: entry.bpm },
                        condition: None,
                        step_index: None,
                    });
                }
            }
            Some(curve) => {
                // Ramp from prev_bpm to entry.bpm over num_bars bars.
                let start_bpm = prev_bpm.unwrap_or(entry.bpm);
                let end_bpm = entry.bpm;
                // Compute total ticks for this segment using per-bar ticks
                let total_ticks: u64 = (0..num_bars)
                    .map(|i| bar_layout.ticks_for_bar(current_bar + i))
                    .sum();
                let steps = 8u64 * num_bars as u64;
                // Emit start if changed
                if prev_bpm != Some(start_bpm) {
                    events.push(TimedEvent {
                        tick: current_tick,
                        track: 0,
                        event: MidiEvent::Tempo { bpm: start_bpm },
                        condition: None,
                        step_index: None,
                    });
                }
                for i in 1..=steps {
                    let t_raw = i as f64 / steps as f64;
                    let t = apply_bpm_curve(t_raw, curve);
                    let bpm = start_bpm + (end_bpm - start_bpm) * t;
                    let tick = current_tick + (total_ticks * i / steps);
                    events.push(TimedEvent {
                        tick,
                        track: 0,
                        event: MidiEvent::Tempo { bpm },
                        condition: None,
                        step_index: None,
                    });
                }
            }
        }

        prev_bpm = Some(entry.bpm);

        if entry.bars.is_some() {
            let segment_ticks: u64 = (0..num_bars)
                .map(|i| bar_layout.ticks_for_bar(current_bar + i))
                .sum();
            current_tick += segment_ticks;
            current_bar += num_bars;
        }
        // If bars=None, it's the last entry — no tick advance needed.
    }
}

/// Apply a named curve function to normalized time `t ∈ [0,1]`.
fn apply_bpm_curve(t: f64, curve: &str) -> f64 {
    match curve {
        "ease_in" => t * t,
        "ease_out" => 1.0 - (1.0 - t) * (1.0 - t),
        "ease_in_out" => 3.0 * t * t - 2.0 * t * t * t,
        "arch" => (std::f64::consts::PI * t).sin(),
        _ => t, // linear (default)
    }
}

/// Build a sorted BPM lookup table from the `@bpm` block (or a single default entry).
///
/// Returns `Vec<(start_tick, bpm)>` in ascending tick order. Used by `effective_bpm_at`
/// to look up the active BPM at any tick without re-scanning the BpmBlock.
fn build_bpm_lookup(
    bpm_block: Option<&crate::ast::BpmBlock>,
    default_bpm: f64,
    bar_layout: &BarLayout,
) -> Vec<(u64, f64)> {
    let mut lookup: Vec<(u64, f64)> = Vec::new();

    if let Some(block) = bpm_block {
        let mut current_tick: u64 = 0;
        let mut current_bar: u32 = 1;
        let mut prev_bpm: Option<f64> = None;

        for entry in &block.entries {
            let num_bars = entry.bars.unwrap_or(1);

            match &entry.ramp {
                None => {
                    if prev_bpm != Some(entry.bpm) {
                        lookup.push((current_tick, entry.bpm));
                    }
                }
                Some(curve) => {
                    let start_bpm = prev_bpm.unwrap_or(entry.bpm);
                    let end_bpm = entry.bpm;
                    let total_ticks: u64 = (0..num_bars)
                        .map(|i| bar_layout.ticks_for_bar(current_bar + i))
                        .sum();
                    let steps = 8u64 * num_bars as u64;
                    if prev_bpm != Some(start_bpm) {
                        lookup.push((current_tick, start_bpm));
                    }
                    for i in 1..=steps {
                        let t_raw = i as f64 / steps as f64;
                        let t = apply_bpm_curve(t_raw, curve);
                        let bpm = start_bpm + (end_bpm - start_bpm) * t;
                        let tick = current_tick + (total_ticks * i / steps);
                        lookup.push((tick, bpm));
                    }
                }
            }

            prev_bpm = Some(entry.bpm);
            if entry.bars.is_some() {
                let segment_ticks: u64 = (0..num_bars)
                    .map(|i| bar_layout.ticks_for_bar(current_bar + i))
                    .sum();
                current_tick += segment_ticks;
                current_bar += num_bars;
            }
        }
    }

    if lookup.is_empty() {
        lookup.push((0, default_bpm));
    }
    lookup
}

/// Return the effective BPM at the given tick.
///
/// Finds the last entry in `lookup` with `start_tick ≤ tick`. The lookup must be
/// sorted ascending by tick (as produced by `build_bpm_lookup`).
fn effective_bpm_at(lookup: &[(u64, f64)], tick: u64) -> f64 {
    let pos = lookup.partition_point(|(t, _)| *t <= tick);
    if pos == 0 {
        lookup.first().map_or(120.0, |(_, bpm)| *bpm)
    } else {
        lookup[pos - 1].1
    }
}

/// Emit time signature events from a `@ts` timeline block.
fn emit_ts_timeline(events: &mut EventStream, ts_block: &TsBlock, bar_layout: &BarLayout) {
    let mut current_tick: u64 = 0;
    let mut current_bar: u32 = 1;
    let mut prev_ts: Option<(u8, u8)> = None;

    for entry in &ts_block.entries {
        let ts = (entry.numerator, entry.denominator);
        if prev_ts != Some(ts) {
            events.push(TimedEvent {
                tick: current_tick,
                track: 0,
                event: MidiEvent::TimeSignature {
                    numerator: entry.numerator,
                    denominator: entry.denominator,
                },
                condition: None,
                step_index: None,
            });
        }
        prev_ts = Some(ts);

        if let Some(bars) = entry.bars {
            // Use per-bar ticks from bar_layout for correct tick advancement
            let segment_ticks: u64 = (0..bars)
                .map(|i| bar_layout.ticks_for_bar(current_bar + i))
                .sum();
            current_tick += segment_ticks;
            current_bar += bars;
        }
    }
}

/// Collect transform calls from a track's play expression (flattened).
fn collect_play_transforms(content: &TrackContent) -> Vec<crate::ast::TransformCall> {
    match content {
        TrackContent::Play(expr) => collect_expr_transforms(expr),
        TrackContent::Steps(_) => Vec::new(),
    }
}

/// Recursively collect transforms from a pattern expression.
fn collect_expr_transforms(expr: &PatternExpr) -> Vec<crate::ast::TransformCall> {
    match expr {
        PatternExpr::Transform { pattern, transform } => {
            let mut transforms = collect_expr_transforms(pattern);
            transforms.push(transform.clone());
            transforms
        }
        PatternExpr::Repeat { pattern, .. } | PatternExpr::RepeatSoft { pattern, .. } => {
            collect_expr_transforms(pattern)
        }
        PatternExpr::Concat { left, right } | PatternExpr::ConcatSoft { left, right } => {
            let mut t = collect_expr_transforms(left);
            t.extend(collect_expr_transforms(right));
            t
        }
        _ => Vec::new(),
    }
}

/// Canonical-order phase of a transform (spec §10.1): 1 = swing,
/// 2 = expressive performance transforms (§10.6), 3 = humanize.
///
/// Returns `None` for transforms that are orthogonal to the canonical
/// order:
/// - structural transforms (§10.2: reverse, invert, retrograde, rotate,
///   stretch, compress, transpose, shift_oct, subset, interleave, mirror)
///   are applied at pattern-resolution time, before any emission phase;
/// - the remaining emission transforms the spec does not sequence (vary,
///   evolve, euclid_gate, arp);
/// - stream transforms applied to the finalized event stream (echo,
///   vel_curve, gate_curve, scale_lock).
fn transform_order_phase(t: &crate::ast::TransformCall) -> Option<(u8, &'static str)> {
    use crate::ast::TransformCall::*;
    match t {
        Swing(..) => Some((1, "swing")),
        Rubato(..) => Some((2, "rubato")),
        Ritardando(..) => Some((2, "ritardando")),
        Accelerando(..) => Some((2, "accelerando")),
        Agogic(..) => Some((2, "agogic")),
        Breathe(..) => Some((2, "breathe")),
        Swell(..) => Some((2, "swell")),
        Phrase(..) => Some((2, "phrase")),
        Humanize(..) => Some((3, "humanize")),
        _ => None,
    }
}

/// Validate that one written transform pipeline follows the canonical
/// order swing → expressive → humanize (spec §10.1). A violation is a
/// compile error naming the misordered pair — never a silent reorder.
fn validate_transform_order(
    transforms: &[crate::ast::TransformCall],
    span: Span,
) -> CompileResult<()> {
    let mut max_seen: Option<(u8, &'static str)> = None;
    for t in transforms {
        if let Some((phase, name)) = transform_order_phase(t) {
            if let Some((prev_phase, prev_name)) = max_seen {
                if phase < prev_phase {
                    return Err(CompileError::ParseError {
                        message: format!(
                            "transform pipeline order: '{name}' must come before '{prev_name}' \
                             — canonical order is swing → expressive transforms → humanize"
                        ),
                        span,
                    });
                }
            }
            if max_seen.is_none_or(|(p, _)| phase >= p) {
                max_seen = Some((phase, name));
            }
        }
    }
    Ok(())
}

/// Validate canonical transform ordering for every contiguous `->` chain
/// in a pattern expression. Chains in different subexpressions (e.g. the
/// two sides of `>>`, or a parenthesized group) are independent pipelines
/// and are validated separately.
fn validate_expr_transform_order(expr: &PatternExpr, span: Span) -> CompileResult<()> {
    match expr {
        PatternExpr::Transform { .. } => {
            // Collect the contiguous chain in application order
            // (innermost transform first).
            let mut chain = Vec::new();
            let mut cur = expr;
            while let PatternExpr::Transform { pattern, transform } = cur {
                chain.push(transform.clone());
                cur = pattern;
            }
            chain.reverse();
            validate_transform_order(&chain, span)?;
            validate_expr_transform_order(cur, span)
        }
        PatternExpr::Repeat { pattern, .. } | PatternExpr::RepeatSoft { pattern, .. } => {
            validate_expr_transform_order(pattern, span)
        }
        PatternExpr::Concat { left, right } | PatternExpr::ConcatSoft { left, right } => {
            validate_expr_transform_order(left, span)?;
            validate_expr_transform_order(right, span)
        }
        PatternExpr::Ref { .. } => Ok(()),
    }
}

/// Get a human-readable name for a transform call.
fn transform_call_name(t: &crate::ast::TransformCall) -> String {
    use crate::ast::TransformCall::*;
    match t {
        Reverse => "reverse".into(),
        Invert => "invert".into(),
        Retrograde => "retrograde".into(),
        Rotate(n) => format!("rotate({n})"),
        Stretch(n, d) => format!("stretch({n}/{d})"),
        Compress(n, d) => format!("compress({n}/{d})"),
        Transpose(n) => format!("transpose({n})"),
        ShiftOct(n) => format!("shift_oct({n})"),
        Subset(indices) => format!("subset({indices:?})"),
        Interleave(name) => format!("interleave({name})"),
        Mirror => "mirror".into(),
        Humanize(timing, intensity) => format!("humanize({timing}, {intensity})"),
        Vary(p) => format!("vary({p})"),
        Swing(ratio, n, d) => format!("swing({ratio},{n}/{d})"),
        Rubato(depth, curve) => format!("rubato({depth}, {curve})"),
        Ritardando(factor) => format!("ritardando({factor})"),
        Accelerando(factor) => format!("accelerando({factor})"),
        Agogic(steps) => format!("agogic({steps:?})"),
        Breathe(pos, duration) => format!("breathe({pos}, {duration})"),
        Swell(peak, curve) => format!("swell({peak}, {curve})"),
        Phrase(tension, release) => format!("phrase({tension},{release})"),
        Evolve(prob) => format!("evolve({prob})"),
        EuclidGate(hits, steps) => format!("euclid({hits},{steps})"),
        Echo(rn, rd, repeats, decay) => format!("echo({rn}/{rd},{repeats},{decay})"),
        VelCurve(wave, min, max, repeat) => format!("vel_curve({wave:?},{min},{max},{repeat})"),
        GateCurve(wave, min, max, repeat) => format!("gate_curve({wave:?},{min},{max},{repeat})"),
        ScaleLock(scale, _, _) => {
            let name = scale.as_deref().unwrap_or("auto");
            format!("scale_lock({name})")
        }
        Arp { pattern, .. } => format!("arp({pattern:?})"),
    }
}

/// Compute swing parameters from track-level swing= and swingunit= settings.
/// Returns (swing_unit_ticks, swing_shift_ticks). Both 0 if no swing.
fn compute_swing_params(track: &TrackBlock, header: &GlobalHeader) -> (u64, i64) {
    match (track.swing, track.swing_unit) {
        (Some(ratio), Some((num, denom))) => {
            let su_ticks = compute_unit_ticks(header.ppq, num, denom);
            let shift = (su_ticks as f64 * (ratio - 0.5) * 2.0).round() as i64;
            (su_ticks, shift)
        }
        _ => (0, 0),
    }
}

/// Maximum humanize velocity deviation at full intensity.
///
/// Spec §10.5 defines `intensity` as a 0.0–1.0 value that "scales overall
/// strength" but does not fix an absolute velocity range; we map full
/// intensity to ±32 velocity units (a quarter of the MIDI velocity range) —
/// the simplest musically sane reading.
const HUMANIZE_VEL_RANGE: f64 = 32.0;

/// Correlation coefficient between humanize timing and velocity deviation
/// (spec §10.5, §11.5).
const HUMANIZE_CORRELATION: f64 = 0.4;

/// Draw the humanize deviation for one note: `(tick_offset, vel_offset)`.
///
/// Consumes exactly two RNG draws per note (two independent uniforms in
/// [-1, 1)). The timing deviation is symmetric with maximum magnitude equal
/// to the resolved `timing` parameter (T% of the step unit, a fraction, or
/// ms via the effective BPM at `tick` — spec §12). The velocity deviation
/// is correlated with the timing deviation per the spec §11.5 formula
/// (`vel_offset = 0.4 * timing_offset_normalized +
/// sqrt(1 - 0.4^2) * independent_vel`), applied with a negative sign so a
/// note pushed late (positive tick offset) tends to be softer (spec §10.5).
fn humanize_note_offsets(
    timing: &TimingValue,
    intensity: f64,
    unit_ticks: u64,
    ppq: u32,
    bpm_lookup: &[(u64, f64)],
    tick: u64,
    rng_state: &mut u64,
) -> (i64, i64) {
    let max_ticks = resolve_timing_value(timing, unit_ticks, ppq, bpm_lookup, tick).abs() as f64;
    let timing_norm = crate::transform::xorshift64_symmetric(rng_state);
    let independent_vel = crate::transform::xorshift64_symmetric(rng_state);
    let tick_offset = round_f64(timing_norm * max_ticks);
    let vel_offset_norm = HUMANIZE_CORRELATION * timing_norm
        + (1.0 - HUMANIZE_CORRELATION * HUMANIZE_CORRELATION).sqrt() * independent_vel;
    let vel_offset = -round_f64(vel_offset_norm * intensity.clamp(0.0, 1.0) * HUMANIZE_VEL_RANGE);
    (tick_offset, vel_offset)
}

/// Resolve a TimingValue to ticks.
fn resolve_timing_value(
    tv: &TimingValue,
    unit_ticks: u64,
    ppq: u32,
    bpm_lookup: &[(u64, f64)],
    tick: u64,
) -> i64 {
    match tv {
        TimingValue::Percent(pct) => {
            // shift_ticks = round((percent / 100) * unit_ticks)
            round_f64((*pct / 100.0) * unit_ticks as f64)
        }
        TimingValue::Fraction(num, den) => {
            // shift_ticks = round(ppq * 4 * (numerator / denominator))
            let ticks = ppq as f64 * 4.0 * (*num as f64 / *den as f64);
            round_f64(ticks)
        }
        TimingValue::Milliseconds(ms) => {
            // Use the effective BPM at the current tick (from the BPM timeline),
            // not the static initial BPM, so ms durations are accurate after tempo changes.
            let bpm = effective_bpm_at(bpm_lookup, tick);
            let ticks_per_ms = (ppq as f64 * bpm) / 60000.0;
            round_f64(*ms * ticks_per_ms)
        }
    }
}

/// Integer division with round-half-up.
fn round_div(numerator: u64, denominator: u64) -> u64 {
    (numerator + denominator / 2) / denominator
}

/// Round f64 to nearest i64 (round-half-up).
fn round_f64(x: f64) -> i64 {
    if x >= 0.0 {
        (x + 0.5) as i64
    } else {
        (x - 0.5) as i64
    }
}

// ── Expressive Curve Functions ─────────────────────────────────────────

/// Evaluate an expressive curve at normalized time t ∈ [0, 1].
fn eval_curve(curve: &crate::ast::ExpressiveCurve, t: f64) -> f64 {
    use crate::ast::ExpressiveCurve;
    match curve {
        ExpressiveCurve::EaseIn => t * t,
        ExpressiveCurve::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        ExpressiveCurve::EaseInOut => 3.0 * t * t - 2.0 * t * t * t,
        ExpressiveCurve::Arch => (std::f64::consts::PI * t).sin(),
    }
}

/// Compute per-step timing offsets for expressive transforms.
/// Returns a Vec of (tick_offset, velocity_scale) for each step.
fn compute_expressive_offsets(
    step_count: usize,
    unit_ticks: u64,
    transforms: &[crate::ast::TransformCall],
    ppq: u32,
    bpm_lookup: &[(u64, f64)],
) -> Vec<(i64, f64)> {
    let mut offsets: Vec<(i64, f64)> = vec![(0, 1.0); step_count];
    if step_count == 0 {
        return offsets;
    }

    for tf in transforms {
        match tf {
            crate::ast::TransformCall::Rubato(depth, curve) => {
                // Time envelope: redistribute time without changing total.
                // Cumulative curve gives time-warp; derivative gives per-step offset.
                let n = step_count as f64;
                for (i, offset) in offsets.iter_mut().enumerate() {
                    let t = i as f64 / n;
                    let curve_val = eval_curve(curve, t);
                    // Offset: curve deviates from linear; depth scales the deviation
                    let linear = t;
                    let warped = t + depth * (curve_val - linear) * 0.5;
                    let tick_shift = ((warped - linear) * n * unit_ticks as f64).round() as i64;
                    offset.0 += tick_shift;
                }
            }
            crate::ast::TransformCall::Ritardando(depth) => {
                // Gradual slowdown: step N takes (1 + depth * t) times its normal duration.
                // This stretches total duration.
                let n = step_count as f64;
                let mut cumulative_offset: f64 = 0.0;
                for (i, offset) in offsets.iter_mut().enumerate() {
                    offset.0 += cumulative_offset.round() as i64;
                    let t = i as f64 / n;
                    let stretch_factor = depth * t;
                    cumulative_offset += stretch_factor * unit_ticks as f64;
                }
            }
            crate::ast::TransformCall::Accelerando(depth) => {
                // Gradual speedup: step N takes (1 - depth * t) times its normal duration.
                let n = step_count as f64;
                let mut cumulative_offset: f64 = 0.0;
                for (i, offset) in offsets.iter_mut().enumerate() {
                    offset.0 += cumulative_offset.round() as i64;
                    let t = i as f64 / n;
                    let compress_factor = -depth * t;
                    cumulative_offset += compress_factor * unit_ticks as f64;
                }
            }
            crate::ast::TransformCall::Agogic(steps) => {
                // Emphasize specified steps by lengthening them 15%, shortening the next.
                let shift_amount = (unit_ticks as f64 * 0.15).round() as i64;
                for &step_1indexed in steps {
                    let idx = step_1indexed as usize - 1;
                    if idx + 1 < step_count {
                        offsets[idx + 1].0 += shift_amount;
                    }
                }
            }
            crate::ast::TransformCall::Breathe(position, duration) => {
                // Micro-pause at the given step: delay all subsequent steps.
                // Approximate the step's tick as position * unit_ticks to look up
                // the correct BPM for ms→tick conversion.
                let breathe_tick = (*position as u64).saturating_mul(unit_ticks);
                let pause_ticks =
                    resolve_timing_value(duration, unit_ticks, ppq, bpm_lookup, breathe_tick);
                let pos = *position as usize;
                for offset in offsets.iter_mut().skip(pos) {
                    offset.0 += pause_ticks;
                }
            }
            crate::ast::TransformCall::Swell(peak, curve) => {
                // Velocity envelope + subtle timing expansion at peak.
                let n = step_count as f64;
                for (i, offset) in offsets.iter_mut().enumerate() {
                    let t = i as f64 / n;
                    let curve_val = eval_curve(curve, t);
                    // Velocity: scale from 0.7 to 1.0 based on curve, peaking at `peak`
                    let dist = (t - peak).abs();
                    let proximity = 1.0 - dist.min(1.0);
                    let vel_scale = 0.7 + 0.3 * proximity * curve_val;
                    offset.1 *= vel_scale;
                    // Subtle timing push at peak (±2% of unit)
                    let timing_push =
                        (unit_ticks as f64 * 0.02 * curve_val * (t - peak)).round() as i64;
                    offset.0 += timing_push;
                }
            }
            crate::ast::TransformCall::Phrase(tension, release) => {
                // Composite: rubato-like push toward tension, relax at release.
                let n = step_count as f64;
                for (i, offset) in offsets.iter_mut().enumerate() {
                    let t = i as f64 / n;
                    // Push forward before tension, relax after release
                    let phase = if t < *tension {
                        // Building tension: slight accelerando
                        let progress = t / tension;
                        -0.03 * progress
                    } else if t < *release {
                        // Between tension and release: slight rit
                        let progress = (t - tension) / (release - tension);
                        0.03 * progress
                    } else {
                        // After release: settling back
                        0.02
                    };
                    offset.0 += (phase * unit_ticks as f64).round() as i64;
                    // Velocity emphasis at tension point
                    let dist = (t - tension).abs();
                    let emphasis = 1.0 - dist.min(0.5) * 2.0;
                    offset.1 *= 0.85 + 0.15 * emphasis.max(0.0);
                }
            }
            _ => {} // Non-expressive transforms handled elsewhere
        }
    }

    offsets
}

/// Compute per-step pitch offsets using the shift register (Turing Machine) algorithm.
/// Returns a Vec of semitone offsets for each step.
fn compute_evolve_offsets(step_count: usize, toggle: f64, rng_state: &mut u64) -> Vec<i8> {
    let mut offsets = vec![0i8; step_count];
    if step_count == 0 {
        return offsets;
    }

    // Initialize 16-bit shift register from current RNG state
    let mut register = (crate::transform::xorshift64(rng_state) & 0xFFFF) as u16;

    for offset in offsets.iter_mut() {
        // Map register value to a pitch offset in [-4, +4] semitones — the
        // documented range, 9 values, via modulo over the full register.
        // (The previous `& 0x07` 3-bit sampling gave [-4, +3]: +4 was
        // unreachable.) The register itself is the Turing-Machine rotate
        // spec §10.5 describes: shift left, the shifted-out MSB feeds back
        // into the LSB, flipped with probability `toggle`.
        let value = (register % 9) as i8 - 4;
        *offset = value;

        // Shift register: MSB feeds back to LSB
        let feedback_bit = (register >> 15) & 1;
        register <<= 1;

        // Toggle: with probability `toggle`, flip the feedback bit
        let rng_val = crate::transform::xorshift64(rng_state);
        let rng_norm = (rng_val & 0xFFFFFFFF) as f64 / u32::MAX as f64;
        if rng_norm < toggle {
            register |= feedback_bit ^ 1;
        } else {
            register |= feedback_bit;
        }
    }

    offsets
}

/// Bjorklund's algorithm: distribute `pulses` evenly across `steps`.
/// Returns a Vec<bool> of length `steps` where true = pulse, false = rest.
fn bjorklund(pulses: u32, steps: u32) -> Vec<bool> {
    if steps == 0 {
        return Vec::new();
    }
    if pulses >= steps {
        return vec![true; steps as usize];
    }
    if pulses == 0 {
        return vec![false; steps as usize];
    }

    // Build groups: start with `pulses` [true] groups and `steps-pulses` [false] groups
    let mut groups: Vec<Vec<bool>> = Vec::new();
    for _ in 0..pulses {
        groups.push(vec![true]);
    }
    for _ in 0..(steps - pulses) {
        groups.push(vec![false]);
    }

    // Iteratively distribute remainder
    loop {
        let group_count = groups.len();
        // Count how many groups share the same pattern as the first group
        let first = &groups[0];
        let same_count = groups.iter().take_while(|g| g == &first).count();
        let remainder = group_count - same_count;

        if remainder <= 1 {
            break;
        }

        let distribute = same_count.min(remainder);
        let mut new_groups = Vec::new();
        for i in 0..distribute {
            let mut combined = groups[i].clone();
            combined.extend_from_slice(&groups[same_count + i]);
            new_groups.push(combined);
        }
        // Remaining undistributed groups
        if same_count > distribute {
            for g in groups.iter().skip(distribute).take(same_count - distribute) {
                new_groups.push(g.clone());
            }
        }
        if remainder > distribute {
            for g in groups.iter().skip(same_count + distribute) {
                new_groups.push(g.clone());
            }
        }
        groups = new_groups;
    }

    groups.into_iter().flatten().collect()
}

/// Compute a euclidean gate mask for the step sequence.
/// Steps where the mask is false are silenced (treated as rests).
fn compute_euclid_gate_mask(step_count: usize, pulses: u32, euclid_steps: u32) -> Vec<bool> {
    let pattern = bjorklund(pulses, euclid_steps);
    if pattern.is_empty() {
        return vec![true; step_count];
    }
    // Cycle the euclidean pattern across all steps
    (0..step_count)
        .map(|i| pattern[i % pattern.len()])
        .collect()
}

/// Compute a wave value in [0, 1] for a given step index within a sequence.
///
/// `idx` is the current step index, `count` is total step count,
/// `repeat` is how many full wave cycles to fit, `rng_state` is for random wave.
fn compute_wave_value(
    wave: &crate::ast::WaveShape,
    idx: usize,
    count: usize,
    repeat: u32,
    rng_state: &mut u64,
) -> f64 {
    use crate::ast::WaveShape;
    if count <= 1 {
        return 0.0;
    }
    // Normalize position to [0, repeat] with repeat cycles
    let t = (idx as f64 / (count - 1) as f64) * repeat as f64;
    // Fractional position within current cycle [0, 1]
    // When t lands exactly on a cycle boundary (e.g. t=1.0, t=2.0),
    // treat it as the end of the previous cycle (1.0) not the start of the next (0.0).
    let t_frac = if t > 0.0 && (t - t.floor()).abs() < 1e-10 {
        1.0
    } else {
        t - t.floor()
    };
    match wave {
        WaveShape::Sine => {
            // Sine: 0→1→0 over one cycle
            (t_frac * std::f64::consts::PI).sin()
        }
        WaveShape::Tri => {
            // Triangle: 0→1→0 linearly
            if t_frac < 0.5 {
                t_frac * 2.0
            } else {
                2.0 - t_frac * 2.0
            }
        }
        WaveShape::Ramp => {
            // Ramp: 0→1 linearly within each cycle
            t_frac
        }
        WaveShape::Square => {
            // Square: 0 for first half, 1 for second half
            if t_frac < 0.5 {
                0.0
            } else {
                1.0
            }
        }
        WaveShape::Random => {
            // Seeded random [0, 1]
            let rng_val = crate::transform::xorshift64(rng_state);
            (rng_val & 0xFFFFFFFF) as f64 / u32::MAX as f64
        }
    }
}

// ── Step Emission ──────────────────────────────────────────────────────

/// Emit events for a single step line (may contain simultaneous tokens via `+`).
#[allow(clippy::too_many_arguments)]
fn emit_step_line(
    events: &mut EventStream,
    step_line: &StepLine,
    step_start: u64,
    unit_ticks: u64,
    ctx: &MusicalContext<'_>,
    prev_pitches: &mut Option<Vec<u8>>,
    ties: &mut TieState,
    rng_state: &mut u64,
    evolve_offset: i8,
    arp_config: Option<&(ArpPattern, u32, u32, u32)>,
    step_index: Option<usize>,
) -> CompileResult<()> {
    for token in &step_line.tokens {
        emit_token(
            events,
            token,
            step_start,
            unit_ticks,
            ctx,
            prev_pitches,
            ties,
            rng_state,
            evolve_offset,
            arp_config,
            step_index,
        )?;
    }
    Ok(())
}

/// Emit events for a single step token.
#[allow(clippy::too_many_arguments)]
fn emit_token(
    events: &mut EventStream,
    token: &StepToken,
    step_start: u64,
    unit_ticks: u64,
    ctx: &MusicalContext<'_>,
    prev_pitches: &mut Option<Vec<u8>>,
    ties: &mut TieState,
    rng_state: &mut u64,
    evolve_offset: i8,
    arp_config: Option<&(ArpPattern, u32, u32, u32)>,
    step_index: Option<usize>,
) -> CompileResult<()> {
    let track_number = ctx.track_number;
    let channel = ctx.channel;
    let default_vel = ctx.default_vel;
    let default_gate = ctx.default_gate;
    let default_octave = ctx.default_octave;
    let track_shift_ticks = ctx.track_shift_ticks;
    let track_lshift_ticks = ctx.track_lshift_ticks;
    let harmony_index = ctx.harmony_index;
    let track = ctx.track;
    let effective_inv = ctx.effective_inv;
    let header = ctx.header;
    let bpm_lookup = ctx.bpm_lookup;
    let bar_layout = ctx.bar_layout;

    match token {
        StepToken::Rest => {
            // A rest silences the tie context: a following `~` is an error
            // (spec §7.3.8). Sounding notes end at their scheduled (gated)
            // off; a carried note (soft-boundary carry-over) ends at the
            // rest's onset. Notes with an explicit [dur:] overlapping the
            // rest keep their full duration — a rest does not truncate
            // them (matches the pre-tie eager-emission behavior).
            settle_active_notes(
                events,
                ties,
                track_number,
                channel,
                Some(step_start),
                Some(step_start),
                None,
            );
            ties.tie_legal = false;
        }

        StepToken::Tie => {
            if !ties.tie_legal {
                return Err(CompileError::TieWithNoPriorNote {
                    name: ties.current_pattern.clone(),
                    span: ties.current_span.unwrap_or_else(|| Span::new(0, 0)),
                });
            }
            // Extend every sounding note by this step's (or subdivision
            // slot's) duration. Gate applies to the final extended duration
            // (see ActiveNote::scheduled_off). A tie also clears the
            // soft-boundary carry flag: the note's extent now covers the
            // tie step and normal gated settling resumes afterwards.
            //
            // If nothing is sounding — the prior note was suppressed by
            // [prob:N] or euclid_gate, or was emitted inline by a ratchet
            // or arp expansion — the tie extends nothing. This is
            // deliberately NOT an error: whether a note actually sounded
            // can depend on the seeded RNG, and compile errors must be
            // deterministic. A suppressed note is never resurrected.
            for an in &mut ties.active {
                an.nominal_ticks += unit_ticks;
                an.carried = false;
            }
        }

        StepToken::Subdivision { tokens } => {
            emit_subdivision(
                events,
                tokens,
                step_start,
                unit_ticks,
                ctx,
                prev_pitches,
                ties,
                rng_state,
                evolve_offset,
                arp_config,
                step_index,
            )?;
        }

        StepToken::Variant { alternatives } => {
            // Spec §7.11: variant pools are metadata consumed by `vary()` —
            // without it, the first alternative is always used (and no RNG
            // is consumed, so bare pools stay deterministic).
            //
            // Under `vary(p)` (spec §10.5): at each step containing a pool,
            // with probability p a random alternative is selected. Exactly
            // one RNG draw `u` is consumed per variant step per pattern
            // instance: when `u < p` the accepted draw is rescaled to
            // uniform [0,1) via `u / p` and indexes the alternatives
            // uniformly (the first alternative can be re-selected — the
            // simplest reading of "selects a random alternative").
            // A single-alternative pool has no variability, so no RNG draw
            // is consumed for it. This also covers `+` clusters inside
            // subdivision brackets, which the parser encodes as
            // single-alternative pools — they are plain simultaneous
            // chords, not variant steps.
            let chosen_idx = match ctx.vary {
                Some(p) if alternatives.len() > 1 => {
                    let u = crate::transform::xorshift64_f64(rng_state);
                    if p > 0.0 && u < p {
                        (((u / p) * alternatives.len() as f64) as usize).min(alternatives.len() - 1)
                    } else {
                        0
                    }
                }
                _ => 0,
            };
            if let Some(first_alt) = alternatives.get(chosen_idx) {
                let sub_line = StepLine {
                    token_spans: vec![None; first_alt.len()],
                    tokens: first_alt.clone(),
                    span: None,
                };
                emit_step_line(
                    events,
                    &sub_line,
                    step_start,
                    unit_ticks,
                    ctx,
                    prev_pitches,
                    ties,
                    rng_state,
                    evolve_offset,
                    arp_config,
                    step_index,
                )?;
            }
        }

        StepToken::DrumHit { name, annotations } => {
            // Drum hits count as prior-note context for `~` legality, but
            // their on/off pairs are emitted inline (never deferred), so a
            // following tie extends nothing.
            ties.tie_legal = true;
            if let Some(dm) = ctx.drummap {
                if let Some(&midi_note) = dm.get(name) {
                    let vel = annotation_vel(annotations).unwrap_or(default_vel);
                    let gate = annotation_gate(annotations).unwrap_or(default_gate);
                    let shift = annotation_shift(annotations)
                        .map(|tv| {
                            resolve_timing_value(
                                &tv, unit_ticks, header.ppq, bpm_lookup, step_start,
                            )
                        })
                        .unwrap_or(track_shift_ticks);

                    let note_on_tick = apply_shift(step_start, shift);
                    let note_duration = annotation_dur(annotations)
                        .map(|(n, d)| compute_unit_ticks(header.ppq, n, d))
                        .unwrap_or_else(|| (unit_ticks as f64 * gate) as u64);
                    let note_off_tick = note_on_tick + note_duration;

                    // Settle previously sounding notes (restrike truncation
                    // at this onset; `+` siblings at this slot keep sounding).
                    settle_active_notes(
                        events,
                        ties,
                        track_number,
                        channel,
                        Some(step_start),
                        Some(note_on_tick),
                        Some(&[midi_note]),
                    );

                    let cond = annotation_condition(annotations);
                    let ratch_count = annotation_ratch(annotations).unwrap_or(1);
                    let ratch_decay = annotation_ratch_decay(annotations);

                    if ratch_count <= 1 {
                        // Normal single hit. Pipeline humanize (spec §10.5)
                        // shifts the on/off pair together (duration and
                        // on/off ordering preserved) and offsets velocity.
                        let (h_tick, h_vel) =
                            if let Some((ref h_timing, h_intensity)) = ctx.humanize {
                                humanize_note_offsets(
                                    h_timing,
                                    h_intensity,
                                    unit_ticks,
                                    header.ppq,
                                    bpm_lookup,
                                    step_start,
                                    rng_state,
                                )
                            } else {
                                (0, 0)
                            };
                        let h_on_tick = apply_shift(note_on_tick, h_tick);
                        let h_velocity = (vel as i64 + h_vel).clamp(1, 127) as u8;
                        events.push(TimedEvent {
                            tick: h_on_tick,
                            track: track_number,
                            event: MidiEvent::NoteOn {
                                channel,
                                note: midi_note,
                                velocity: h_velocity,
                            },
                            condition: cond.clone(),
                            step_index,
                        });
                        events.push(TimedEvent {
                            tick: h_on_tick + (note_off_tick - note_on_tick),
                            track: track_number,
                            event: MidiEvent::NoteOff {
                                channel,
                                note: midi_note,
                            },
                            condition: cond,
                            step_index,
                        });
                    } else {
                        emit_ratchet_hits(
                            events,
                            note_on_tick,
                            unit_ticks,
                            ratch_count,
                            ratch_decay,
                            vel,
                            gate,
                            midi_note,
                            channel,
                            track_number,
                            &cond,
                            step_index,
                        );
                    }

                    // Emit lane events from annotations
                    let lshift = annotation_lshift(annotations)
                        .map(|tv| {
                            resolve_timing_value(
                                &tv, unit_ticks, header.ppq, bpm_lookup, step_start,
                            )
                        })
                        .unwrap_or(track_lshift_ticks);
                    emit_annotation_cc(
                        events,
                        annotations,
                        step_start,
                        unit_ticks,
                        lshift,
                        track_number,
                        channel,
                    );
                }
            }
        }

        // Note-producing tokens (Degree, AbsolutePitch, MidiNumber, ChordStep)
        _ => {
            let annotations = token_annotations(token);

            // This is structurally a note token, so a following `~` is
            // legal even if this step ends up suppressed ([prob:N] below) —
            // tie legality must never depend on the seeded RNG.
            ties.tie_legal = true;

            // [prob:N] — probabilistic step skip. Every annotated step
            // consumes exactly ONE RNG draw regardless of N (spec §11.1 /
            // Spec §11.2: "one xorshift64 value per annotated step, in step
            // order") — so adding or removing [prob:1.0] shifts downstream
            // draws exactly like any other probability. The `p < 1.0`
            // check only guards suppression: [prob:1.0] always plays, even
            // on the one-in-2^32 draw where rng_norm == 1.0.
            if let Some(p) = annotation_prob(annotations) {
                let rng_val = crate::transform::xorshift64(rng_state);
                let rng_norm = (rng_val & 0xFFFF_FFFF) as f64 / u32::MAX as f64;
                if p < 1.0 && rng_norm >= p {
                    // Suppressed: settle prior notes as a rest would. A
                    // following tie will find nothing sounding and
                    // extend nothing — a suppressed note is never
                    // resurrected by a tie.
                    settle_active_notes(
                        events,
                        ties,
                        track_number,
                        channel,
                        Some(step_start),
                        Some(step_start),
                        None,
                    );
                    return Ok(());
                }
            }

            let harmony_ctx = harmony_index.and_then(|idx| idx.query(step_start));

            // Compute the effective scale context for ^n degree resolution.
            // Bar number is 1-indexed.
            let (abs_bar, _) = bar_layout.bar_at_tick(step_start);
            let (base_mode_ivs, base_scale_root) = ctx.scale_timeline.context_at_bar(abs_bar);
            // track.mode= overrides the mode (but keeps the scale root) for ^n resolution.
            // Borrow slices directly — no allocation needed.
            let (effective_mode_ivs, effective_scale_root): (&[u8], u8) =
                if let Some(ref mode_name) = track.mode {
                    match crate::harmony::lookup_mode(mode_name) {
                        Some(ivs) => (ivs, base_scale_root),
                        None => (base_mode_ivs, base_scale_root),
                    }
                } else {
                    (base_mode_ivs, base_scale_root)
                };

            // Resolve pitches
            let pitches = resolve_step_pitches(
                token,
                harmony_ctx,
                effective_mode_ivs,
                effective_scale_root,
                default_octave,
            );

            if pitches.is_empty() {
                // Nothing resolvable to play (e.g. %n without harmony
                // context): treat like a silent step for tie purposes.
                settle_active_notes(
                    events,
                    ties,
                    track_number,
                    channel,
                    Some(step_start),
                    Some(step_start),
                    None,
                );
                return Ok(());
            }

            // Apply voicing for chord steps and current chord
            let final_pitches = if matches!(token, StepToken::ChordStep { .. }) {
                let chord = match token {
                    StepToken::ChordStep { chord, .. } => chord,
                    _ => unreachable!(),
                };
                let (voiced, new_prev) = voicing::voice_chord(
                    chord,
                    track.voice,
                    effective_inv, // resolved: harmony_block.inv < track.inv
                    default_octave,
                    prev_pitches.as_deref(),
                );
                *prev_pitches = Some(new_prev);
                voiced
            } else if matches!(token, StepToken::CurrentChord { .. }) {
                // Voice the current harmony chord with effective inversion (harmony < track)
                if let Some(hctx) = harmony_ctx {
                    let (voiced, new_prev) = voicing::voice_chord(
                        &hctx.chord,
                        track.voice,
                        effective_inv, // resolved: harmony_block.inv < track.inv
                        default_octave,
                        prev_pitches.as_deref(),
                    );
                    *prev_pitches = Some(new_prev);
                    voiced
                } else {
                    pitches
                }
            } else if matches!(token, StepToken::ChordOrdinal { .. }) {
                // Single chord tone — update voice leading state like single-note tokens
                if pitches.len() > 1 {
                    *prev_pitches = Some(pitches.clone());
                }
                pitches
            } else {
                // Update voice leading state for non-chord tokens too
                if pitches.len() > 1 {
                    *prev_pitches = Some(pitches.clone());
                }
                pitches
            };

            // Apply evolve pitch offset (shift register algorithm)
            let final_pitches: Vec<u8> = if evolve_offset != 0 {
                let harmony_ctx = harmony_index.and_then(|idx| idx.query(step_start));
                final_pitches
                    .into_iter()
                    .map(|p| {
                        let shifted = (p as i16 + evolve_offset as i16).clamp(0, 127) as u8;
                        if let Some(hctx) = harmony_ctx {
                            crate::voicing::snap_to_scale(
                                shifted,
                                &hctx.mode_intervals,
                                hctx.scale_root,
                            )
                        } else {
                            shifted
                        }
                    })
                    .collect()
            } else {
                final_pitches
            };

            // Get per-step overrides
            let vel = annotation_vel(annotations).unwrap_or(default_vel);
            let gate = annotation_gate(annotations).unwrap_or(default_gate);
            let shift = annotation_shift(annotations)
                .map(|tv| resolve_timing_value(&tv, unit_ticks, header.ppq, bpm_lookup, step_start))
                .unwrap_or(track_shift_ticks);

            let note_on_tick = apply_shift(step_start, shift);
            let explicit_dur =
                annotation_dur(annotations).map(|(n, d)| compute_unit_ticks(header.ppq, n, d));
            let note_duration = explicit_dur.unwrap_or((unit_ticks as f64 * gate) as u64);
            let note_off_tick = note_on_tick + note_duration;

            // Settle previously sounding notes: a restruck pitch is
            // truncated at this onset, a carried note (soft-boundary
            // carry-over) sustains legato up to this onset, and everything
            // else ends at its scheduled (gated) off tick. Simultaneous
            // `+` siblings already emitted at this slot keep sounding.
            settle_active_notes(
                events,
                ties,
                track_number,
                channel,
                Some(step_start),
                Some(note_on_tick),
                Some(&final_pitches),
            );

            // Extract condition and ratchet from annotations
            let cond = annotation_condition(annotations);
            let ratch_count = annotation_ratch(annotations).unwrap_or(1);
            let ratch_decay = annotation_ratch_decay(annotations);

            // [glide] — emit portamento CC before note-on (CC65=127 on, CC5=time)
            let glide = annotation_glide(annotations);
            if let Some(frac_opt) = &glide {
                if !track.is_drum {
                    let cc5_val = frac_opt.map_or(64, |f| (f * 127.0).round() as u8);
                    events.push(TimedEvent {
                        tick: note_on_tick,
                        track: track_number,
                        event: MidiEvent::CC {
                            channel,
                            controller: 65,
                            value: 127,
                        },
                        condition: None,
                        step_index,
                    });
                    events.push(TimedEvent {
                        tick: note_on_tick,
                        track: track_number,
                        event: MidiEvent::CC {
                            channel,
                            controller: 5,
                            value: cc5_val,
                        },
                        condition: None,
                        step_index,
                    });
                }
            }

            if ratch_count <= 1 {
                // Check for arp expansion (multi-note steps only)
                let arp_emitted = if let Some(&(ref arp_pat, rate_num, rate_den, octaves)) =
                    arp_config
                {
                    if final_pitches.len() > 1 {
                        let arp_onset = compute_unit_ticks(header.ppq, rate_num, rate_den);
                        if arp_onset > 0 {
                            // Expand tones with octave layers
                            let mut base_tones: Vec<u8> = final_pitches.clone();
                            base_tones.sort();
                            base_tones.dedup();
                            let mut tones = base_tones.clone();
                            for oct in 1..octaves {
                                for &p in &base_tones {
                                    let shifted = (p as u32).saturating_add(12 * oct);
                                    if shifted <= 127 {
                                        tones.push(shifted as u8);
                                    }
                                }
                            }
                            tones.sort();
                            tones.dedup();

                            let cycle = generate_arp_cycle(&tones, arp_pat, rng_state);
                            if !cycle.is_empty() {
                                let step_end = note_on_tick + unit_ticks;
                                // A rate coarser than the step unit
                                // (arp_onset >= unit_ticks) still emits
                                // at least one slot — the first arp
                                // tone for the full step, clamped at
                                // the step end — rather than silently
                                // swallowing the chord (spec §10.4
                                // defines rate as onset spacing, not a
                                // license for silence).
                                let n_slots = unit_ticks.checked_div(arp_onset).unwrap_or(0).max(1);
                                for k in 0..n_slots {
                                    let slot_on = note_on_tick + k * arp_onset;
                                    if slot_on >= step_end {
                                        break;
                                    }
                                    let slot_dur = (arp_onset as f64 * gate) as u64;
                                    let slot_off = (slot_on + slot_dur).min(step_end);
                                    let note = cycle[(k as usize) % cycle.len()];
                                    events.push(TimedEvent {
                                        tick: slot_on,
                                        track: track_number,
                                        event: MidiEvent::NoteOn {
                                            channel,
                                            note,
                                            velocity: vel,
                                        },
                                        condition: cond.clone(),
                                        step_index,
                                    });
                                    events.push(TimedEvent {
                                        tick: slot_off,
                                        track: track_number,
                                        event: MidiEvent::NoteOff { channel, note },
                                        condition: cond.clone(),
                                        step_index,
                                    });
                                }
                                // Arp slot on/offs are emitted inline —
                                // nothing is registered as sounding, so
                                // a following tie extends nothing.
                                true
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                if !arp_emitted {
                    // Normal note emission. The NoteOff is NOT emitted here:
                    // it is deferred via the active-note list so that a
                    // following `~` can extend the note. The off is settled
                    // (at the scheduled gated tick) when the next
                    // non-extending token, a hard boundary, or the end of
                    // the track is reached.
                    //
                    // Pipeline humanize (spec §10.5) applies per note: two
                    // RNG draws each, shifting the onset (clamped to tick 0;
                    // the deferred off follows the shifted onset, so a note
                    // can never end before it starts) and offsetting the
                    // velocity (clamped to 1–127). Ratchet and arp
                    // expansions are generative sequences and are not
                    // humanized.
                    for &note in &final_pitches {
                        let (h_tick, h_vel) =
                            if let Some((ref h_timing, h_intensity)) = ctx.humanize {
                                humanize_note_offsets(
                                    h_timing,
                                    h_intensity,
                                    unit_ticks,
                                    header.ppq,
                                    bpm_lookup,
                                    step_start,
                                    rng_state,
                                )
                            } else {
                                (0, 0)
                            };
                        let h_on_tick = apply_shift(note_on_tick, h_tick);
                        let h_velocity = (vel as i64 + h_vel).clamp(1, 127) as u8;
                        events.push(TimedEvent {
                            tick: h_on_tick,
                            track: track_number,
                            event: MidiEvent::NoteOn {
                                channel,
                                note,
                                velocity: h_velocity,
                            },
                            condition: cond.clone(),
                            step_index,
                        });
                        ties.active.push(ActiveNote {
                            note,
                            on_tick: h_on_tick,
                            slot: step_start,
                            nominal_ticks: explicit_dur.unwrap_or(unit_ticks),
                            gate,
                            gated: explicit_dur.is_none(),
                            carried: false,
                            condition: cond.clone(),
                            step_index,
                        });
                    }
                }
            } else {
                // Ratchet: emit multiple hits within the step. The hit
                // on/offs are emitted inline (never deferred), so a
                // following tie extends nothing.
                for &note in &final_pitches {
                    emit_ratchet_hits(
                        events,
                        note_on_tick,
                        unit_ticks,
                        ratch_count,
                        ratch_decay,
                        vel,
                        gate,
                        note,
                        channel,
                        track_number,
                        &cond,
                        step_index,
                    );
                }
            }

            // [glide] — emit portamento off after note-off (CC65=0)
            if glide.is_some() && !track.is_drum {
                events.push(TimedEvent {
                    tick: note_off_tick,
                    track: track_number,
                    event: MidiEvent::CC {
                        channel,
                        controller: 65,
                        value: 0,
                    },
                    condition: None,
                    step_index,
                });
            }

            // Emit lane events from annotations
            let lshift = annotation_lshift(annotations)
                .map(|tv| resolve_timing_value(&tv, unit_ticks, header.ppq, bpm_lookup, step_start))
                .unwrap_or(track_lshift_ticks);
            emit_annotation_cc(
                events,
                annotations,
                step_start,
                unit_ticks,
                lshift,
                track_number,
                channel,
            );

            // Emit pitch bend if present
            if let Some(pb) = annotation_pitch_bend(annotations) {
                let pb_tick = apply_shift(step_start, lshift);
                events.push(TimedEvent {
                    tick: pb_tick,
                    track: track_number,
                    event: MidiEvent::PitchBend { channel, value: pb },
                    condition: None,
                    step_index,
                });
            }

            // Emit aftertouch if present
            if let Some(at) = annotation_aftertouch(annotations) {
                let at_tick = apply_shift(step_start, lshift);
                events.push(TimedEvent {
                    tick: at_tick,
                    track: track_number,
                    event: MidiEvent::Aftertouch { channel, value: at },
                    condition: None,
                    step_index,
                });
            }
        }
    }

    Ok(())
}

// ── Subdivision Emission ───────────────────────────────────────────────

/// Emit events for a subdivision bracket. Divides the parent step duration
/// equally among the contained tokens, recursing for nested brackets.
#[allow(clippy::too_many_arguments)]
fn emit_subdivision(
    events: &mut EventStream,
    tokens: &[StepToken],
    parent_start: u64,
    parent_ticks: u64,
    ctx: &MusicalContext<'_>,
    prev_pitches: &mut Option<Vec<u8>>,
    ties: &mut TieState,
    rng_state: &mut u64,
    evolve_offset: i8,
    arp_config: Option<&(ArpPattern, u32, u32, u32)>,
    step_index: Option<usize>,
) -> CompileResult<()> {
    if tokens.is_empty() {
        return Ok(());
    }

    let n = tokens.len() as u64;
    let sub_ticks = parent_ticks / n;

    for (k, token) in tokens.iter().enumerate() {
        let token_start = parent_start + k as u64 * sub_ticks;
        // The LAST slot absorbs the integer-division remainder (mirroring
        // the harmony `steps:` remainder handling), so the bracket's total
        // is exactly `parent_ticks` — otherwise a 96-tick parent split 7
        // ways would lose 5 ticks (last note ends early, next step
        // overlaps/gaps). Nested brackets recurse with their slot's exact
        // duration, so the invariant holds at every depth.
        let slot_ticks = if k as u64 == n - 1 {
            parent_ticks - (n - 1) * sub_ticks
        } else {
            sub_ticks
        };
        emit_token(
            events,
            token,
            token_start,
            slot_ticks,
            ctx,
            prev_pitches,
            ties,
            rng_state,
            evolve_offset,
            arp_config,
            step_index,
        )?;
    }
    Ok(())
}

// ── Active Note Management (tie tracking) ──────────────────────────────

/// A note that has been struck but whose NoteOff has not yet been emitted.
///
/// NoteOffs are deferred until the note's full extent is known: a `~` (tie)
/// step extends the note by one step, so the off tick can only be computed
/// once a non-extending token (note, rest, hard boundary, or end of track)
/// is reached.
struct ActiveNote {
    /// MIDI note number.
    note: u8,
    /// Actual NoteOn tick (after shift).
    on_tick: u64,
    /// Slot identity: the (swung / expressive-adjusted) step or subdivision
    /// start this note was emitted at. Simultaneous tokens joined with `+`
    /// share a slot and must not settle each other.
    slot: u64,
    /// Nominal (un-gated) duration: the owning step's unit ticks plus one
    /// unit per extending tie step.
    nominal_ticks: u64,
    /// Gate ratio applied to `nominal_ticks`.
    gate: f64,
    /// False when an explicit `[dur:]` annotation set the base duration.
    /// `dur` overrides gate entirely (spec §13), so gate is not applied and
    /// tie extensions add raw step ticks.
    gated: bool,
    /// Set when the note crosses a soft pattern boundary (`*~` / `~>>`).
    /// A carried note sustains legato up to the next event's onset instead
    /// of ending at its gated off. A tie in the next instance clears the
    /// flag and extends the note normally.
    carried: bool,
    /// Condition from the originating step — the deferred NoteOff carries
    /// the same condition as its NoteOn.
    condition: Option<crate::ast::StepCondition>,
    /// Step index of the originating step.
    step_index: Option<usize>,
}

impl ActiveNote {
    /// The off tick implied by the note's current extent.
    ///
    /// Gate policy for tie chains: spec §7.3.8 says a tie "extends the
    /// duration of the previous active notes by one step" but does not
    /// specify gate handling. We apply gate to the *final extended* nominal
    /// duration, so `^1 ~ ~ ~` at gate 1.0 is exactly one whole note and at
    /// gate g ends at `on + 4·unit·g`. This reduces to the pre-tie formula
    /// `(unit_ticks as f64 * gate) as u64` for un-tied notes, keeping all
    /// existing output byte-identical.
    fn scheduled_off(&self) -> u64 {
        if self.gated {
            self.on_tick + (self.nominal_ticks as f64 * self.gate) as u64
        } else {
            self.on_tick + self.nominal_ticks
        }
    }
}

/// Per-track tie / active-note state threaded through step emission.
///
/// `active` is an insertion-ordered `Vec` (not a hash map): NoteOff emission
/// order must be deterministic, and `sort_event_stream` is a *stable* sort
/// keyed only on (tick, priority, track) — so the order NoteOffs are pushed
/// in is observable in the final stream. A Vec preserves the NoteOn order of
/// the originating step exactly (a BTreeMap keyed by pitch would reorder
/// chord voicings whose pitches are not ascending).
struct TieState {
    /// Currently sounding notes, in emission order.
    active: Vec<ActiveNote>,
    /// Whether a `~` at the current position has a prior note context.
    /// True after any note-producing token, false at track start, after a
    /// rest, and after a hard pattern boundary. Purely structural — it never
    /// depends on runtime/probabilistic suppression, so tie errors are
    /// deterministic regardless of seed.
    tie_legal: bool,
    /// Name of the pattern instance currently being emitted (for errors).
    current_pattern: String,
    /// Span of the step line currently being emitted (for errors).
    current_span: Option<Span>,
}

impl TieState {
    fn new(pattern_name: String) -> Self {
        Self {
            active: Vec::new(),
            tie_legal: false,
            current_pattern: pattern_name,
            current_span: None,
        }
    }
}

/// Settle (emit the deferred NoteOffs for) active notes.
///
/// - `current_slot`: notes registered at this slot are kept sounding — they
///   are simultaneous (`+`) siblings of the token currently being emitted.
///   Pass `None` to settle everything (hard boundary / end of track).
/// - `cut`: onset of the new event. Carried notes (soft-boundary carry-over)
///   end here (legato into the next instance); a restruck pitch (present in
///   `new_pitches`) is truncated here if it would still be sounding.
/// - `new_pitches`: pitches about to be struck, for restrike truncation.
///
/// Notes that are neither carried nor restruck end at their scheduled
/// (gated) off tick — which may lie beyond `cut` for explicit `[dur:]`
/// overlaps. Such notes are finalized here and are then beyond the reach of
/// later ties or restrikes (matching the pre-tie eager-emission behavior).
#[allow(clippy::too_many_arguments)]
fn settle_active_notes(
    events: &mut EventStream,
    ties: &mut TieState,
    track_number: usize,
    channel: u8,
    current_slot: Option<u64>,
    cut: Option<u64>,
    new_pitches: Option<&[u8]>,
) {
    let mut i = 0;
    while i < ties.active.len() {
        if current_slot == Some(ties.active[i].slot) {
            i += 1;
            continue;
        }
        let an = ties.active.remove(i);
        let scheduled = an.scheduled_off();
        let restruck = new_pitches.is_some_and(|ps| ps.contains(&an.note));
        let off_tick = if an.carried {
            cut.unwrap_or(scheduled)
        } else if restruck {
            cut.map_or(scheduled, |c| scheduled.min(c))
        } else {
            scheduled
        };
        events.push(TimedEvent {
            tick: off_tick,
            track: track_number,
            event: MidiEvent::NoteOff {
                channel,
                note: an.note,
            },
            condition: an.condition,
            step_index: an.step_index,
        });
    }
}

/// Find the index of the NoteOff paired with a NoteOn in the accumulated
/// stream, or `None` if the pair is incomplete.
///
/// The pair is identified by (track, channel, note, step_index) plus
/// `off.tick >= on_tick` — the deferred-settling TieState stamps each
/// deferred NoteOff with its NoteOn's `step_index`, and inline emitters
/// (ratchet, arp, drum, echo) copy it too, so step_index + pitch is the
/// pairing key. `claimed[j] == true` marks offs already paired to an
/// earlier NoteOn (ratchet hits share pitch and step_index; vec order
/// pairs them correctly). This replaces first-NoteOff-after-onset scans,
/// which could retarget a different note's off at the same pitch.
fn find_paired_note_off(
    events: &EventStream,
    claimed: &[bool],
    track_number: usize,
    channel: u8,
    note: u8,
    step_index: Option<usize>,
    on_tick: u64,
) -> Option<usize> {
    events.iter().enumerate().position(|(j, e)| {
        !claimed[j]
            && e.track == track_number
            && e.step_index == step_index
            && e.tick >= on_tick
            && matches!(
                &e.event,
                MidiEvent::NoteOff { channel: c, note: n } if *c == channel && *n == note
            )
    })
}

/// Apply a shift offset to a tick, clamping to 0.
fn apply_shift(tick: u64, shift: i64) -> u64 {
    let shifted = tick as i64 + shift;
    if shifted < 0 {
        0
    } else {
        shifted as u64
    }
}

/// Emit N ratchet hits within a step's duration.
///
/// Each hit occupies `unit_ticks / ratch_count` ticks. Gate is applied per hit.
/// Velocity decays by `ratch_decay` for each successive hit, clamped to 1–127.
#[allow(clippy::too_many_arguments)]
fn emit_ratchet_hits(
    events: &mut Vec<TimedEvent>,
    note_on_tick: u64,
    unit_ticks: u64,
    ratch_count: u32,
    ratch_decay: f64,
    base_vel: u8,
    gate: f64,
    note: u8,
    channel: u8,
    track_number: usize,
    cond: &Option<crate::ast::StepCondition>,
    step_index: Option<usize>,
) {
    let hit_duration = unit_ticks / ratch_count as u64;
    let mut current_vel = base_vel as f64;

    for k in 0..ratch_count {
        let hit_on = note_on_tick + k as u64 * hit_duration;
        let hit_note_dur = (hit_duration as f64 * gate) as u64;
        let hit_off = hit_on + hit_note_dur;
        let vel = (current_vel.round() as u8).clamp(1, 127);

        events.push(TimedEvent {
            tick: hit_on,
            track: track_number,
            event: MidiEvent::NoteOn {
                channel,
                note,
                velocity: vel,
            },
            condition: cond.clone(),
            step_index,
        });
        events.push(TimedEvent {
            tick: hit_off,
            track: track_number,
            event: MidiEvent::NoteOff { channel, note },
            condition: cond.clone(),
            step_index,
        });

        current_vel *= ratch_decay;
    }
}

// ── Annotation Extraction ──────────────────────────────────────────────

/// Get annotations from a token (returns empty slice for tokens without annotations).
fn token_annotations(token: &StepToken) -> &[Annotation] {
    match token {
        StepToken::Degree { annotations, .. }
        | StepToken::AbsolutePitch { annotations, .. }
        | StepToken::MidiNumber { annotations, .. }
        | StepToken::ChordStep { annotations, .. }
        | StepToken::DrumHit { annotations, .. }
        | StepToken::CurrentChord { annotations, .. }
        | StepToken::ChordOrdinal { annotations, .. } => annotations,
        StepToken::Rest
        | StepToken::Tie
        | StepToken::Subdivision { .. }
        | StepToken::Variant { .. } => &[],
    }
}

/// Extract velocity override from annotations.
fn annotation_vel(annotations: &[Annotation]) -> Option<u8> {
    annotations.iter().find_map(|a| match a {
        Annotation::Vel(v) => Some(*v),
        _ => None,
    })
}

/// Extract gate override from annotations.
fn annotation_gate(annotations: &[Annotation]) -> Option<f64> {
    annotations.iter().find_map(|a| match a {
        Annotation::Gate(g) => Some(*g),
        _ => None,
    })
}

/// Extract shift override from annotations.
fn annotation_shift(annotations: &[Annotation]) -> Option<TimingValue> {
    annotations.iter().find_map(|a| match a {
        Annotation::Shift(tv) => Some(tv.clone()),
        _ => None,
    })
}

/// Extract lshift override from annotations.
fn annotation_lshift(annotations: &[Annotation]) -> Option<TimingValue> {
    annotations.iter().find_map(|a| match a {
        Annotation::LShift(tv) => Some(tv.clone()),
        _ => None,
    })
}

/// Extract explicit duration from annotations.
fn annotation_dur(annotations: &[Annotation]) -> Option<(u32, u32)> {
    annotations.iter().find_map(|a| match a {
        Annotation::Dur(n, d) => Some((*n, *d)),
        _ => None,
    })
}

/// Extract an octave override from annotations.
fn annotation_octave(annotations: &[Annotation]) -> Option<u8> {
    annotations.iter().find_map(|a| match a {
        Annotation::Oct(o) => Some(*o),
        _ => None,
    })
}

/// Extract pitch bend from annotations.
fn annotation_pitch_bend(annotations: &[Annotation]) -> Option<i16> {
    annotations.iter().find_map(|a| match a {
        Annotation::PitchBend(pb) => Some(*pb),
        _ => None,
    })
}

/// Extract step condition from annotations.
fn annotation_condition(annotations: &[Annotation]) -> Option<crate::ast::StepCondition> {
    annotations.iter().find_map(|a| match a {
        Annotation::Condition(c) => Some(c.clone()),
        _ => None,
    })
}

/// Extract aftertouch from annotations.
fn annotation_aftertouch(annotations: &[Annotation]) -> Option<u8> {
    annotations.iter().find_map(|a| match a {
        Annotation::Aftertouch(at) => Some(*at),
        _ => None,
    })
}

/// Extract ratchet count from annotations.
fn annotation_ratch(annotations: &[Annotation]) -> Option<u32> {
    annotations.iter().find_map(|a| match a {
        Annotation::Ratch(n) => Some(*n),
        _ => None,
    })
}

/// Extract ratchet decay from annotations (default 1.0).
fn annotation_ratch_decay(annotations: &[Annotation]) -> f64 {
    annotations
        .iter()
        .find_map(|a| match a {
            Annotation::RatchDecay(d) => Some(*d),
            _ => None,
        })
        .unwrap_or(1.0)
}

/// Extract probability from annotations (None = always play).
fn annotation_prob(annotations: &[Annotation]) -> Option<f64> {
    annotations.iter().find_map(|a| match a {
        Annotation::Prob(p) => Some(*p),
        _ => None,
    })
}

/// Extract glide annotation (None = no glide, Some(None) = full-duration, Some(Some(f)) = fractional).
fn annotation_glide(annotations: &[Annotation]) -> Option<Option<f64>> {
    annotations.iter().find_map(|a| match a {
        Annotation::Glide(frac) => Some(*frac),
        _ => None,
    })
}

/// Generate an arp cycle from chord tones for the given pattern.
///
/// Returns a cycling sequence for use in `k % cycle.len()` indexing.
/// For `UpDown`: ascending then descending without repeating top or bottom.
fn generate_arp_cycle(tones: &[u8], pattern: &ArpPattern, rng_state: &mut u64) -> Vec<u8> {
    if tones.is_empty() {
        return Vec::new();
    }
    match pattern {
        ArpPattern::Up => tones.to_vec(),
        ArpPattern::Down => {
            let mut v = tones.to_vec();
            v.reverse();
            v
        }
        ArpPattern::UpDown => {
            // e.g. [C, E, G] → [C, E, G, E] (no repeat at top, next cycle starts C)
            // e.g. [C, E, G, B] → [C, E, G, B, G, E]
            let mut v = tones.to_vec(); // ascending half
            if v.len() > 1 {
                // descending half: middle tones reversed (skip first=bottom, skip last=top)
                let down: Vec<u8> = tones[1..tones.len() - 1].iter().rev().cloned().collect();
                v.extend(down);
            }
            v
        }
        ArpPattern::Random => {
            let mut v = tones.to_vec();
            // Fisher-Yates shuffle using rng_state
            for i in (1..v.len()).rev() {
                let j = (crate::transform::xorshift64(rng_state) as usize) % (i + 1);
                v.swap(i, j);
            }
            v
        }
    }
}

// ── CC / Lane Event Emission ───────────────────────────────────────────

/// Emit CC events from annotations at the step's lane-shifted position.
#[allow(clippy::too_many_arguments)]
fn emit_annotation_cc(
    events: &mut EventStream,
    annotations: &[Annotation],
    step_start: u64,
    unit_ticks: u64,
    lshift: i64,
    track_number: usize,
    channel: u8,
) {
    for ann in annotations {
        match ann {
            Annotation::Expr(cv) => emit_cc_value(
                events,
                11,
                cv,
                step_start,
                unit_ticks,
                lshift,
                track_number,
                channel,
            ),
            Annotation::Dyn(cv) => emit_cc_value(
                events,
                1,
                cv,
                step_start,
                unit_ticks,
                lshift,
                track_number,
                channel,
            ),
            Annotation::Sus(v) => {
                let tick = apply_shift(step_start, lshift);
                events.push(TimedEvent {
                    tick,
                    track: track_number,
                    event: MidiEvent::CC {
                        channel,
                        controller: 64,
                        value: *v,
                    },
                    condition: None,
                    step_index: None,
                });
            }
            Annotation::Pan(cv) => emit_cc_value(
                events,
                10,
                cv,
                step_start,
                unit_ticks,
                lshift,
                track_number,
                channel,
            ),
            Annotation::Vol(cv) => emit_cc_value(
                events,
                7,
                cv,
                step_start,
                unit_ticks,
                lshift,
                track_number,
                channel,
            ),
            Annotation::Cc(cc_num, cv) => emit_cc_value(
                events,
                *cc_num,
                cv,
                step_start,
                unit_ticks,
                lshift,
                track_number,
                channel,
            ),
            _ => {} // Non-CC annotations handled elsewhere
        }
    }
}

/// Emit a CC value — static or ramp (4 sample points).
#[allow(clippy::too_many_arguments)]
fn emit_cc_value(
    events: &mut EventStream,
    controller: u8,
    cv: &CcValue,
    step_start: u64,
    unit_ticks: u64,
    lshift: i64,
    track_number: usize,
    channel: u8,
) {
    match cv {
        CcValue::Static(v) => {
            let tick = apply_shift(step_start, lshift);
            events.push(TimedEvent {
                tick,
                track: track_number,
                event: MidiEvent::CC {
                    channel,
                    controller,
                    value: *v,
                },
                condition: None,
                step_index: None,
            });
        }
        CcValue::Ramp { start, end } => {
            // 4 sample points at [0, 0.25, 0.5, 0.75] of step duration
            for i in 0..4u32 {
                let fraction = i as f64 / 4.0;
                let sample_tick = step_start + (unit_ticks as f64 * fraction) as u64;
                let tick = apply_shift(sample_tick, lshift);
                let value = lerp_u8(*start, *end, fraction);
                events.push(TimedEvent {
                    tick,
                    track: track_number,
                    event: MidiEvent::CC {
                        channel,
                        controller,
                        value,
                    },
                    condition: None,
                    step_index: None,
                });
            }
        }
    }
}

/// Linear interpolation between two u8 values.
fn lerp_u8(start: u8, end: u8, t: f64) -> u8 {
    let result = start as f64 + (end as f64 - start as f64) * t;
    result.round().clamp(0.0, 127.0) as u8
}

// ── Pitch Resolution (from Phase 8) ───────────────────────────────────

/// Resolve a step token to zero or more MIDI note numbers.
///
/// Given a step token and the active chord context (from the harmony index),
/// returns the MIDI note numbers for that step. For multi-note steps (chords,
/// simultaneous tokens), returns all notes.
///
/// `default_octave` is the track's default octave. The harmony context may be
/// `None` if the track has no `follow=` directive (scale-relative fallback).
/// `scale_mode_ivs` and `scale_root` are always supplied by the caller from the
/// active `@scale` timeline entry (with any `track.mode=` override applied).
pub fn resolve_step_pitches(
    token: &StepToken,
    context: Option<&ChordContext>,
    scale_mode_ivs: &[u8],
    scale_root: u8,
    default_octave: u8,
) -> Vec<u8> {
    match token {
        StepToken::Degree {
            degree,
            accidental,
            octave,
            annotations,
        } => {
            // ^n is scale-absolute: always uses the passed scale context, never chord context.
            let oct = annotation_octave(annotations)
                .or(*octave)
                .unwrap_or(default_octave);
            let midi =
                voicing::resolve_degree(*degree, *accidental, oct, scale_mode_ivs, scale_root);
            vec![midi]
        }

        StepToken::ChordOrdinal {
            degree,
            octave,
            annotations,
        } => {
            // %n uses chord intervals from harmony context.
            let forced_oct = annotation_octave(annotations).or(*octave);
            if let Some(ctx) = context {
                let midi = voicing::resolve_chord_ordinal(
                    *degree,
                    default_octave,
                    forced_oct,
                    &ctx.chord.intervals,
                    ctx.chord.root,
                );
                vec![midi]
            } else {
                Vec::new()
            }
        }

        StepToken::AbsolutePitch { midi_note, .. } => {
            vec![*midi_note]
        }

        StepToken::MidiNumber { note, .. } => {
            vec![*note]
        }

        StepToken::ChordStep { chord, .. } => {
            // Chord symbol in step context — voice the chord
            let (pitches, _) = voicing::voice_chord(
                chord,
                VoicingStrategy::Close,
                Inversion::Fixed(0),
                default_octave,
                None,
            );
            pitches
        }

        StepToken::CurrentChord { .. } => {
            // Resolve from current harmony context
            if let Some(ctx) = context {
                let (pitches, _) = voicing::voice_chord(
                    &ctx.chord,
                    VoicingStrategy::Close,
                    Inversion::Fixed(0),
                    default_octave,
                    None,
                );
                pitches
            } else {
                Vec::new()
            }
        }

        StepToken::Rest | StepToken::Tie => Vec::new(),

        StepToken::DrumHit { .. } => {
            // Drum hits are resolved via drummap, not harmony
            Vec::new()
        }

        StepToken::Subdivision { tokens } => {
            // For pitch resolution, return pitches of first token
            // (timing subdivision is handled by the timing engine)
            if let Some(first) = tokens.first() {
                resolve_step_pitches(first, context, scale_mode_ivs, scale_root, default_octave)
            } else {
                Vec::new()
            }
        }

        StepToken::Variant { alternatives } => {
            // Default: use first alternative
            if let Some(first_alt) = alternatives.first() {
                if let Some(first) = first_alt.first() {
                    resolve_step_pitches(first, context, scale_mode_ivs, scale_root, default_octave)
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        }
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Annotation, ChordSymbol};
    use crate::harmony::ChordContext;

    fn cmaj7_context() -> ChordContext {
        ChordContext {
            chord: ChordSymbol {
                root: 0,
                intervals: vec![0, 4, 7, 11],
                slash_bass: None,
                roman: None,
            },
            mode_intervals: vec![0, 2, 4, 5, 7, 9, 11],
            scale_root: 0,
        }
    }

    fn dm7_context() -> ChordContext {
        ChordContext {
            chord: ChordSymbol {
                root: 2,
                intervals: vec![0, 3, 7, 10],
                slash_bass: None,
                roman: None,
            },
            mode_intervals: vec![0, 2, 3, 5, 7, 9, 10],
            scale_root: 2,
        }
    }

    // C major scale (root=0, intervals=[0,2,4,5,7,9,11])
    const C_MAJOR: [u8; 7] = [0, 2, 4, 5, 7, 9, 11];

    // ── Pitch resolution tests ──

    #[test]
    fn test_resolve_degree_root_with_context() {
        let ctx = cmaj7_context();
        let token = StepToken::Degree {
            degree: 1,
            accidental: 0,
            octave: None,
            annotations: Vec::new(),
        };
        let pitches = resolve_step_pitches(&token, Some(&ctx), &C_MAJOR, 0, 4);
        assert_eq!(pitches, vec![60]); // C4
    }

    #[test]
    fn test_resolve_degree_third() {
        let ctx = cmaj7_context();
        let token = StepToken::Degree {
            degree: 3,
            accidental: 0,
            octave: None,
            annotations: Vec::new(),
        };
        let pitches = resolve_step_pitches(&token, Some(&ctx), &C_MAJOR, 0, 4);
        assert_eq!(pitches, vec![64]); // E4
    }

    #[test]
    fn test_resolve_degree_flat_third() {
        let ctx = cmaj7_context();
        let token = StepToken::Degree {
            degree: 3,
            accidental: -1,
            octave: None,
            annotations: Vec::new(),
        };
        let pitches = resolve_step_pitches(&token, Some(&ctx), &C_MAJOR, 0, 4);
        assert_eq!(pitches, vec![63]); // Eb4
    }

    #[test]
    fn test_resolve_degree_different_root() {
        let ctx = dm7_context();
        // D dorian: mode_intervals=[0,2,3,5,7,9,10], scale_root=2
        let d_dorian: [u8; 7] = [0, 2, 3, 5, 7, 9, 10];
        let token = StepToken::Degree {
            degree: 1,
            accidental: 0,
            octave: None,
            annotations: Vec::new(),
        };
        let pitches = resolve_step_pitches(&token, Some(&ctx), &d_dorian, 2, 4);
        assert_eq!(pitches, vec![62]); // D4
    }

    #[test]
    fn test_resolve_degree_no_context_fallback() {
        let token = StepToken::Degree {
            degree: 5,
            accidental: 0,
            octave: None,
            annotations: Vec::new(),
        };
        // No harmony context — pass C major scale explicitly
        let pitches = resolve_step_pitches(&token, None, &C_MAJOR, 0, 4);
        assert_eq!(pitches, vec![67]); // G4
    }

    #[test]
    fn test_resolve_absolute_pitch() {
        let token = StepToken::AbsolutePitch {
            midi_note: 72,
            annotations: Vec::new(),
        };
        let pitches = resolve_step_pitches(&token, None, &C_MAJOR, 0, 4);
        assert_eq!(pitches, vec![72]);
    }

    #[test]
    fn test_resolve_midi_number() {
        let token = StepToken::MidiNumber {
            note: 48,
            annotations: Vec::new(),
        };
        let pitches = resolve_step_pitches(&token, None, &C_MAJOR, 0, 4);
        assert_eq!(pitches, vec![48]);
    }

    #[test]
    fn test_resolve_rest() {
        let pitches = resolve_step_pitches(&StepToken::Rest, None, &C_MAJOR, 0, 4);
        assert!(pitches.is_empty());
    }

    #[test]
    fn test_resolve_tie() {
        let pitches = resolve_step_pitches(&StepToken::Tie, None, &C_MAJOR, 0, 4);
        assert!(pitches.is_empty());
    }

    #[test]
    fn test_resolve_degree_octave_override() {
        let ctx = cmaj7_context();
        let token = StepToken::Degree {
            degree: 1,
            accidental: 0,
            octave: Some(5),
            annotations: Vec::new(),
        };
        let pitches = resolve_step_pitches(&token, Some(&ctx), &C_MAJOR, 0, 4);
        assert_eq!(pitches, vec![72]); // C5
    }

    #[test]
    fn test_resolve_degree_annotation_octave() {
        let ctx = cmaj7_context();
        let token = StepToken::Degree {
            degree: 1,
            accidental: 0,
            octave: None,
            annotations: vec![Annotation::Oct(6)],
        };
        let pitches = resolve_step_pitches(&token, Some(&ctx), &C_MAJOR, 0, 4);
        assert_eq!(pitches, vec![84]); // C6
    }

    #[test]
    fn test_resolve_degree_annotation_overrides_token_octave() {
        let ctx = cmaj7_context();
        let token = StepToken::Degree {
            degree: 1,
            accidental: 0,
            octave: Some(3),                       // token says 3
            annotations: vec![Annotation::Oct(6)], // annotation says 6
        };
        let pitches = resolve_step_pitches(&token, Some(&ctx), &C_MAJOR, 0, 4);
        assert_eq!(pitches, vec![84]); // annotation wins: C6
    }

    // ── Timing tests ──

    #[test]
    fn test_compute_unit_ticks_quarter() {
        // unit=1/4 with ppq=480: 480 * 4 * 1 / 4 = 480
        assert_eq!(compute_unit_ticks(480, 1, 4), 480);
    }

    #[test]
    fn test_compute_unit_ticks_eighth() {
        // unit=1/8 with ppq=480: 480 * 4 * 1 / 8 = 240
        assert_eq!(compute_unit_ticks(480, 1, 8), 240);
    }

    #[test]
    fn test_compute_unit_ticks_sixteenth() {
        // unit=1/16 with ppq=480: 480 * 4 * 1 / 16 = 120
        assert_eq!(compute_unit_ticks(480, 1, 16), 120);
    }

    #[test]
    fn test_compute_unit_ticks_half() {
        // unit=1/2 with ppq=480: 480 * 4 * 1 / 2 = 960
        assert_eq!(compute_unit_ticks(480, 1, 2), 960);
    }

    #[test]
    fn test_compute_bar_ticks_4_4() {
        let header = GlobalHeader {
            ppq: 480,
            ts_numerator: 4,
            ts_denominator: 4,
            ..Default::default()
        };
        assert_eq!(compute_bar_ticks(&header), 1920); // 480 * 4
    }

    #[test]
    fn test_compute_bar_ticks_3_4() {
        let header = GlobalHeader {
            ppq: 480,
            ts_numerator: 3,
            ts_denominator: 4,
            ..Default::default()
        };
        assert_eq!(compute_bar_ticks(&header), 1440); // 480 * 3
    }

    #[test]
    fn test_resolve_timing_percent() {
        // 5% of 480 ticks = 24
        let tv = TimingValue::Percent(5.0);
        assert_eq!(resolve_timing_value(&tv, 480, 480, &[(0, 120.0)], 0), 24);
    }

    #[test]
    fn test_resolve_timing_fraction() {
        // 1/32 note = ppq * 4 / 32 = 480 * 4 / 32 = 60
        let tv = TimingValue::Fraction(1, 32);
        assert_eq!(resolve_timing_value(&tv, 480, 480, &[(0, 120.0)], 0), 60);
    }

    #[test]
    fn test_resolve_timing_ms() {
        // At 120 BPM, ppq=480: ticks_per_ms = 480 * 120 / 60000 = 0.96
        // 10ms = round(10 * 0.96) = round(9.6) = 10
        let tv = TimingValue::Milliseconds(10.0);
        assert_eq!(resolve_timing_value(&tv, 480, 480, &[(0, 120.0)], 0), 10);
    }

    #[test]
    fn test_resolve_timing_negative_percent() {
        let tv = TimingValue::Percent(-5.0);
        assert_eq!(resolve_timing_value(&tv, 480, 480, &[(0, 120.0)], 0), -24);
    }

    #[test]
    fn test_apply_shift_positive() {
        assert_eq!(apply_shift(100, 10), 110);
    }

    #[test]
    fn test_apply_shift_negative_clamp() {
        assert_eq!(apply_shift(5, -10), 0);
    }

    #[test]
    fn test_lerp_u8() {
        assert_eq!(lerp_u8(0, 127, 0.0), 0);
        assert_eq!(lerp_u8(0, 127, 1.0), 127);
        assert_eq!(lerp_u8(0, 127, 0.5), 64);
        assert_eq!(lerp_u8(40, 88, 0.25), 52);
    }

    // ── Full compilation tests ──

    #[test]
    fn test_compile_simple_track() {
        let header = GlobalHeader {
            ppq: 480,
            bpm: 120.0,
            ts_numerator: 4,
            ts_denominator: 4,
            ..Default::default()
        };

        let pattern = PatternBlock {
            name: "melody".to_string(),
            steps: 2,
            unit: (1, 4),
            velocity: 100,
            gate: 0.9,
            octave: 4,
            transforms: Vec::new(),
            body: crate::ast::PatternBody::Steps(vec![
                StepLine {
                    tokens: vec![StepToken::AbsolutePitch {
                        midi_note: 60,
                        annotations: Vec::new(),
                    }],
                    token_spans: vec![None],
                    span: None,
                },
                StepLine {
                    tokens: vec![StepToken::AbsolutePitch {
                        midi_note: 64,
                        annotations: Vec::new(),
                    }],
                    token_spans: vec![None],
                    span: None,
                },
            ]),
            span: None,
        };

        let track = TrackBlock {
            name: "piano".to_string(),
            channel: 1,
            program: Some(0),
            unit: None,
            octave: 4,
            velocity: 100,
            gate: 0.9,
            shift: None,
            lshift: None,
            follow: None,
            voice: VoicingStrategy::Close,
            inv: Inversion::Fixed(0),
            seed: None,
            start: None,
            is_drum: false,
            drummap: None,
            mode: None,
            rate: None,
            swing: None,
            swing_unit: None,
            content: TrackContent::Play(PatternExpr::Ref {
                name: "melody".to_string(),
                rate: None,
            }),
            span: None,
        };

        let blocks = vec![Block::Pattern(pattern), Block::Track(track)];

        let output = compile(&header, &blocks).unwrap();

        // Should have: tempo, time_sig, track_name(title=none so no title), program_change, track_name,
        // bar_marker, note_on, note_off, note_on, note_off, final note_off cleanup
        let note_ons: Vec<&TimedEvent> = output
            .events
            .iter()
            .filter(|e| matches!(e.event, MidiEvent::NoteOn { .. }))
            .collect();
        assert_eq!(note_ons.len(), 2);

        // First note at tick 0, second at tick 480
        assert_eq!(note_ons[0].tick, 0);
        assert_eq!(note_ons[1].tick, 480);

        if let MidiEvent::NoteOn {
            note,
            velocity,
            channel,
        } = &note_ons[0].event
        {
            assert_eq!(*note, 60);
            assert_eq!(*velocity, 100);
            assert_eq!(*channel, 0); // 1-1=0
        }

        if let MidiEvent::NoteOn { note, .. } = &note_ons[1].event {
            assert_eq!(*note, 64);
        }

        // Check note-offs exist
        let note_offs: Vec<&TimedEvent> = output
            .events
            .iter()
            .filter(|e| matches!(e.event, MidiEvent::NoteOff { .. }))
            .collect();
        // 2 gated note-offs + potential cleanup
        assert!(note_offs.len() >= 2);

        // First note-off at tick 432 (480 * 0.9)
        let first_off = note_offs
            .iter()
            .find(|e| matches!(&e.event, MidiEvent::NoteOff { note, .. } if *note == 60));
        assert!(first_off.is_some());
        assert_eq!(first_off.unwrap().tick, 432);
    }

    #[test]
    fn test_compile_rest_ends_notes() {
        let header = GlobalHeader::default();

        let pattern = PatternBlock {
            name: "rest_test".to_string(),
            steps: 2,
            unit: (1, 4),
            velocity: 100,
            gate: 0.9,
            octave: 4,
            transforms: Vec::new(),
            body: crate::ast::PatternBody::Steps(vec![
                StepLine {
                    tokens: vec![StepToken::AbsolutePitch {
                        midi_note: 60,
                        annotations: Vec::new(),
                    }],
                    token_spans: vec![None],
                    span: None,
                },
                StepLine {
                    tokens: vec![StepToken::Rest],
                    token_spans: vec![None],
                    span: None,
                },
            ]),
            span: None,
        };

        let track = TrackBlock {
            name: "test".to_string(),
            channel: 1,
            program: None,
            unit: None,
            octave: 4,
            velocity: 100,
            gate: 0.9,
            shift: None,
            lshift: None,
            follow: None,
            voice: VoicingStrategy::Close,
            inv: Inversion::Fixed(0),
            seed: None,
            start: None,
            is_drum: false,
            drummap: None,
            mode: None,
            rate: None,
            swing: None,
            swing_unit: None,
            content: TrackContent::Play(PatternExpr::Ref {
                name: "rest_test".to_string(),
                rate: None,
            }),
            span: None,
        };

        let blocks = vec![Block::Pattern(pattern), Block::Track(track)];
        let output = compile(&header, &blocks).unwrap();

        // Rest should produce a note-off at step 1's start (tick 480)
        let note_offs: Vec<&TimedEvent> = output
            .events
            .iter()
            .filter(|e| matches!(e.event, MidiEvent::NoteOff { note, .. } if note == 60))
            .collect();
        assert!(!note_offs.is_empty());
    }

    #[test]
    fn test_compile_program_change() {
        let header = GlobalHeader::default();
        let pattern = PatternBlock {
            name: "p".to_string(),
            steps: 1,
            unit: (1, 4),
            velocity: 100,
            gate: 0.9,
            octave: 4,
            transforms: Vec::new(),
            body: crate::ast::PatternBody::Steps(vec![StepLine {
                tokens: vec![StepToken::Rest],
                token_spans: vec![None],
                span: None,
            }]),
            span: None,
        };
        let track = TrackBlock {
            name: "t".to_string(),
            channel: 2,
            program: Some(42),
            unit: None,
            octave: 4,
            velocity: 100,
            gate: 0.9,
            shift: None,
            lshift: None,
            follow: None,
            voice: VoicingStrategy::Close,
            inv: Inversion::Fixed(0),
            seed: None,
            start: None,
            is_drum: false,
            drummap: None,
            mode: None,
            rate: None,
            swing: None,
            swing_unit: None,
            content: TrackContent::Play(PatternExpr::Ref {
                name: "p".to_string(),
                rate: None,
            }),
            span: None,
        };

        let blocks = vec![Block::Pattern(pattern), Block::Track(track)];
        let output = compile(&header, &blocks).unwrap();

        let prog_changes: Vec<&TimedEvent> = output
            .events
            .iter()
            .filter(|e| matches!(e.event, MidiEvent::ProgramChange { .. }))
            .collect();
        assert_eq!(prog_changes.len(), 1);
        if let MidiEvent::ProgramChange { channel, program } = &prog_changes[0].event {
            assert_eq!(*channel, 1); // ch=2 → 0-indexed=1
            assert_eq!(*program, 42);
        }
    }

    #[test]
    fn test_rate_harmony_pitch_validation() {
        use crate::lexer::tokenize;
        use crate::parser::parse_header;

        let source = r#"
@bpm 120
@ts 4/4
@ppq 480
@seed 1

@scale root=C mode=major

@harmony main
| Cmaj7 | Dm7 | Em7 | Fmaj7 |

@pattern walk steps=4 unit=1/4
^1
^3
^5
^7

@track normal ch=1 follow=main vel=80 gate=0.9 oct=4
play: walk * 4

@track fast ch=2 follow=main vel=80 gate=0.9 oct=4 rate=2.0
play: walk * 4

@track slow ch=3 follow=main vel=80 gate=0.9 oct=4 rate=0.5
play: walk * 4
"#;

        // Parse and compile
        let (tokens, lex_errors) = tokenize(source);
        assert!(lex_errors.is_empty(), "lexer errors: {lex_errors:?}");
        let (header, mut parser) = parse_header(tokens).expect("header parse failed");

        let mut blocks = Vec::new();
        while parser.has_tokens() {
            parser.skip_newlines_pub();
            if !parser.has_tokens() {
                break;
            }
            if parser.peek_is_scale() {
                let tc = parser.parse_scale_block().expect("scale parse failed");
                blocks.push(Block::Scale(tc));
            } else if parser.peek_is_harmony() {
                let block = parser.parse_harmony_block().expect("harmony parse failed");
                blocks.push(Block::Harmony(block));
            } else if parser.peek_is_pattern() {
                let block = parser.parse_pattern_block().expect("pattern parse failed");
                blocks.push(Block::Pattern(block));
            } else if parser.peek_is_track() {
                let block = parser.parse_track_block().expect("track parse failed");
                blocks.push(Block::Track(block));
            } else {
                break;
            }
            parser.skip_newlines_pub();
        }

        // Build harmony index for validation
        let tonal = blocks
            .iter()
            .find_map(|b| {
                if let Block::Scale(tc) = b {
                    Some(tc.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let scale_timeline_test =
            ScaleTimeline::from_tonal_context(&tonal).expect("scale timeline build failed");
        let harmony_block = blocks
            .iter()
            .find_map(|b| {
                if let Block::Harmony(h) = b {
                    Some(h)
                } else {
                    None
                }
            })
            .expect("no harmony block found");
        let bar_layout = BarLayout::from_header(&header);
        let harmony_index =
            HarmonyIndex::build(harmony_block, &header, &scale_timeline_test, &bar_layout)
                .expect("harmony index build failed");

        // Compile
        let output = compile(&header, &blocks).expect("compile failed");

        // Validate every NoteOn pitch is a chord tone or scale tone
        for event in &output.events {
            if let MidiEvent::NoteOn { channel, note, .. } = &event.event {
                let ctx = harmony_index.query(event.tick).unwrap_or_else(|| {
                    panic!(
                        "no harmony context at tick {} (ch={}, note={})",
                        event.tick, channel, note
                    )
                });

                let pitch_class = note % 12;

                // Chord tones: root + each interval, mod 12
                let chord_tones: Vec<u8> = ctx
                    .chord
                    .intervals
                    .iter()
                    .map(|&i| (ctx.chord.root + i) % 12)
                    .collect();

                // Scale tones: scale_root + each mode interval, mod 12
                let scale_tones: Vec<u8> = ctx
                    .mode_intervals
                    .iter()
                    .map(|&i| (ctx.scale_root + i) % 12)
                    .collect();

                assert!(
                    chord_tones.contains(&pitch_class) || scale_tones.contains(&pitch_class),
                    "pitch {} (class={}) at tick {} ch={} is not a chord tone {:?} \
                     or scale tone {:?} (chord root={})",
                    note,
                    pitch_class,
                    event.tick,
                    channel,
                    chord_tones,
                    scale_tones,
                    ctx.chord.root
                );
            }
        }
    }

    #[test]
    fn transform_call_name_includes_all_params() {
        use crate::ast::{ExpressiveCurve, TimingValue, TransformCall};

        assert_eq!(
            transform_call_name(&TransformCall::Humanize(TimingValue::Percent(5.0), 0.5)),
            "humanize(5%, 0.5)"
        );
        assert_eq!(
            transform_call_name(&TransformCall::Humanize(TimingValue::Fraction(1, 32), 0.3)),
            "humanize(1/32, 0.3)"
        );
        assert_eq!(
            transform_call_name(&TransformCall::Rubato(0.1, ExpressiveCurve::Arch)),
            "rubato(0.1, arch)"
        );
        assert_eq!(
            transform_call_name(&TransformCall::Swell(0.6, ExpressiveCurve::EaseInOut)),
            "swell(0.6, ease_in_out)"
        );
        assert_eq!(
            transform_call_name(&TransformCall::Breathe(4, TimingValue::Fraction(1, 32))),
            "breathe(4, 1/32)"
        );
        assert_eq!(
            transform_call_name(&TransformCall::Rubato(0.2, ExpressiveCurve::EaseIn)),
            "rubato(0.2, ease_in)"
        );
        assert_eq!(
            transform_call_name(&TransformCall::Swell(0.8, ExpressiveCurve::EaseOut)),
            "swell(0.8, ease_out)"
        );
    }

    // ── Tie tests ──

    /// Parse and compile a full source string, returning the compile result.
    fn compile_src(source: &str) -> CompileResult<CompileOutput> {
        let mut program = crate::parse_only(source).expect("parse failed");
        program.header.resolved_seed = Some(1);
        compile(&program.header, &program.blocks)
    }

    /// Compile a source string that is expected to fail; returns the error.
    fn compile_err(source: &str, why: &str) -> CompileError {
        match compile_src(source) {
            Ok(_) => panic!("{why}: expected a compile error but compilation succeeded"),
            Err(e) => e,
        }
    }

    /// Collect (tick, is_on, note) triples for NoteOn/NoteOff events on user tracks.
    fn note_events(output: &CompileOutput) -> Vec<(u64, bool, u8)> {
        output
            .events
            .iter()
            .filter_map(|e| match &e.event {
                MidiEvent::NoteOn { note, .. } => Some((e.tick, true, *note)),
                MidiEvent::NoteOff { note, .. } => Some((e.tick, false, *note)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_tie_at_track_start_is_error() {
        let src = "@ppq 480\n\n@pattern p steps=2 unit=1/4\n~\nC4\n\n@track t ch=1\nplay: p\n";
        let err = compile_err(src, "leading tie at track start must error");
        assert!(
            matches!(err, CompileError::TieWithNoPriorNote { ref name, .. } if name == "p"),
            "expected TieWithNoPriorNote for pattern 'p', got: {err:?}"
        );
    }

    #[test]
    fn test_tie_at_steps_track_start_is_error() {
        let src = "@ppq 480\n\n@track t ch=1 unit=1/4\nsteps:\n  ~\n  C4\n";
        let err = compile_err(src, "leading tie in steps: must error");
        assert!(
            matches!(err, CompileError::TieWithNoPriorNote { ref name, .. } if name == "t"),
            "expected TieWithNoPriorNote for track 't', got: {err:?}"
        );
    }

    #[test]
    fn test_tie_after_hard_boundary_is_error() {
        // Instance 1 of `p` starts with C4 (so the first tie chain is fine),
        // but the hard `*` boundary resets the tie context — the leading
        // tie of instance 2 must error. Uses a pattern that both starts
        // with a tie and ends with a note via concat.
        let src = "@ppq 480\n\n@pattern a steps=2 unit=1/4\nC4\nE4\n\n\
                   @pattern b steps=2 unit=1/4\n~\nG4\n\n@track t ch=1\nplay: a >> b\n";
        let err = compile_err(src, "leading tie after hard boundary must error");
        assert!(
            matches!(err, CompileError::TieWithNoPriorNote { ref name, .. } if name == "b"),
            "expected TieWithNoPriorNote for pattern 'b', got: {err:?}"
        );
    }

    #[test]
    fn test_tie_after_hard_repeat_boundary_is_error() {
        // `hold` ends with a note and starts with a tie; `hold *~ 2` would
        // be legal mid-sequence, but a *hard* repeat cuts the context.
        let src = "@ppq 480\n\n@pattern a steps=1 unit=1/4\nC4\n\n\
                   @pattern hold steps=2 unit=1/4\n~\nG4\n\n@track t ch=1\nplay: a ~>> hold * 2\n";
        let err = compile_err(src, "leading tie after hard * boundary must error");
        assert!(
            matches!(err, CompileError::TieWithNoPriorNote { ref name, .. } if name == "hold"),
            "expected TieWithNoPriorNote for pattern 'hold', got: {err:?}"
        );
    }

    #[test]
    fn test_tie_after_rest_is_error() {
        let src = "@ppq 480\n\n@pattern p steps=3 unit=1/4\nC4\n.\n~\n\n@track t ch=1\nplay: p\n";
        let err = compile_err(src, "tie after rest must error (spec 7.3.8)");
        assert!(
            matches!(err, CompileError::TieWithNoPriorNote { .. }),
            "expected TieWithNoPriorNote, got: {err:?}"
        );
    }

    #[test]
    fn test_tie_after_soft_boundary_is_legal() {
        let src = "@ppq 480\n\n@pattern a steps=1 unit=1/4\nC4\n\n\
                   @pattern hold steps=2 unit=1/4\n~\nG4\n\n@track t ch=1\nplay: a ~>> hold\n";
        let output = compile_src(src).expect("leading tie after soft boundary is legal");
        // C4 is extended by hold's leading tie: nominal 2 steps * gate.
        let events = note_events(&output);
        let c4_off = events
            .iter()
            .find(|(_, is_on, n)| !*is_on && *n == 60)
            .expect("C4 off");
        assert!(
            c4_off.0 > 480,
            "C4 must sustain past its own step (off at {}), extended by the tie",
            c4_off.0
        );
    }

    #[test]
    fn test_tie_chain_duration_honors_gate() {
        // gate=0.5, 4 steps of 1/4 at ppq 480: nominal 1920, off at 960.
        let src = "@ppq 480\n\n@pattern p steps=4 unit=1/4 gate=0.5\nC4\n~\n~\n~\n\n\
                   @track t ch=1\nplay: p\n";
        let output = compile_src(src).expect("compile");
        assert_eq!(note_events(&output), vec![(0, true, 60), (960, false, 60)]);
    }

    #[test]
    fn test_tie_after_prob_suppressed_note_is_not_error_and_does_not_resurrect() {
        // [prob:0.0] always suppresses the step. The following tie is
        // structurally legal (errors must not depend on the seeded RNG)
        // but must not resurrect the suppressed E4 — and must not extend
        // the earlier C4 either, which was settled when the suppressed
        // step was reached.
        let src = "@ppq 480\n\n@pattern p steps=3 unit=1/4 gate=1.0\nC4\nE4[prob:0.0]\n~\n\n\
                   @track t ch=1\nplay: p\n";
        let output = compile_src(src).expect("tie after suppressed note must not error");
        assert_eq!(
            note_events(&output),
            vec![(0, true, 60), (480, false, 60)],
            "C4 sounds exactly one step; E4 never sounds; the tie extends nothing"
        );
    }

    #[test]
    fn test_tie_extends_conditional_note_off() {
        // The deferred NoteOff carries the same condition as its NoteOn.
        let src = "@ppq 480\n\n@pattern p steps=2 unit=1/4 gate=1.0\nC4[every:2]\n~\n\n\
                   @track t ch=1\nplay: p\n";
        let output = compile_src(src).expect("compile");
        let offs: Vec<_> = output
            .events
            .iter()
            .filter(|e| matches!(e.event, MidiEvent::NoteOff { .. }))
            .collect();
        assert_eq!(offs.len(), 1);
        assert_eq!(offs[0].tick, 960);
        assert!(
            offs[0].condition.is_some(),
            "deferred NoteOff must keep the originating step's condition"
        );
    }

    // ── Transform pipeline order enforcement (spec §10.1) ──

    #[test]
    fn test_order_humanize_before_swing_is_error() {
        let src = "@ppq 480\n\n@pattern p steps=2 unit=1/8\nC4\nE4\n\n@track t ch=1\n\
                   play: p -> humanize(5%, 0.5) -> swing(0.62, 1/8)\n";
        let err = compile_err(src, "humanize before swing must error");
        match err {
            CompileError::ParseError { message, .. } => {
                assert!(
                    message.contains("swing") && message.contains("humanize"),
                    "error must name the misordered pair, got: {message}"
                );
            }
            other => panic!("expected ParseError for transform order, got: {other:?}"),
        }
    }

    #[test]
    fn test_order_humanize_before_expressive_is_error() {
        let src = "@ppq 480\n\n@pattern p steps=2 unit=1/8\nC4\nE4\n\n@track t ch=1\n\
                   play: p -> humanize(5%, 0.5) -> rubato(0.3, arch)\n";
        let err = compile_err(src, "humanize before rubato must error");
        match err {
            CompileError::ParseError { message, .. } => {
                assert!(
                    message.contains("rubato") && message.contains("humanize"),
                    "error must name the misordered pair, got: {message}"
                );
            }
            other => panic!("expected ParseError for transform order, got: {other:?}"),
        }
    }

    #[test]
    fn test_order_expressive_before_swing_is_error() {
        let src = "@ppq 480\n\n@pattern p steps=2 unit=1/8\nC4\nE4\n\n@track t ch=1\n\
                   play: p -> rubato(0.3, arch) -> swing(0.62, 1/8)\n";
        let err = compile_err(src, "rubato before swing must error");
        assert!(
            matches!(err, CompileError::ParseError { .. }),
            "expected ParseError for transform order, got: {err:?}"
        );
    }

    #[test]
    fn test_order_canonical_pipeline_compiles() {
        // swing → expressive → humanize is the canonical order; structural
        // transforms (reverse) are orthogonal and may appear anywhere.
        let src = "@ppq 480\n\n@pattern p steps=2 unit=1/8\nC4\nE4\n\n@track t ch=1\n\
                   play: p -> reverse -> swing(0.62, 1/8) -> rubato(0.3, arch) -> humanize(5%, 0.5)\n";
        compile_src(src).expect("canonical order must compile");
    }

    #[test]
    fn test_order_separate_pipelines_are_independent() {
        // Chains on either side of `>>` are independent pipelines — a
        // humanize on the left does not constrain a swing on the right.
        let src = "@ppq 480\n\n@pattern a steps=2 unit=1/8\nC4\nE4\n\n\
                   @pattern b steps=2 unit=1/8\nG4\nC5\n\n@track t ch=1\n\
                   play: (a -> humanize(5%, 0.5)) >> (b -> swing(0.62, 1/8))\n";
        compile_src(src).expect("independent pipelines must compile");
    }

    // ── [prob:N] RNG-draw contract ──

    #[test]
    fn test_prob_one_consumes_rng_draw() {
        // Every [prob:N] annotation consumes exactly one draw, including
        // [prob:1.0]. Mirror the compiler's RNG stream: resolved_seed = 1
        // (set by compile_src), track index 0.
        let track_seed = transform::fnv1a_derive(1, 0);
        let mut st = transform::seed_state(track_seed);
        let mut norms = Vec::new();
        for _ in 0..5 {
            let v = transform::xorshift64(&mut st);
            norms.push((v & 0xFFFF_FFFF) as f64 / u32::MAX as f64);
        }
        // The final [prob:0.5] step uses the 5th draw when the four
        // [prob:1.0] steps consume draws, and the 1st draw when they are
        // unannotated. The chosen seed makes those land on opposite sides
        // of 0.5, so the two sources compile differently.
        assert!(
            (norms[4] < 0.5) != (norms[0] < 0.5),
            "test premise: draws 1 and 5 must fall on opposite sides of 0.5"
        );

        let src_annotated = "@ppq 480\n\n@pattern p steps=5 unit=1/4\n\
             C4[prob:1.0]\nC4[prob:1.0]\nC4[prob:1.0]\nC4[prob:1.0]\nE4[prob:0.5]\n\n\
             @track t ch=1\nplay: p\n";
        let src_bare = "@ppq 480\n\n@pattern p steps=5 unit=1/4\n\
             C4\nC4\nC4\nC4\nE4[prob:0.5]\n\n\
             @track t ch=1\nplay: p\n";

        let has_e4 = |output: &CompileOutput| {
            output
                .events
                .iter()
                .any(|e| matches!(e.event, MidiEvent::NoteOn { note, .. } if note == 64))
        };
        let out_a = compile_src(src_annotated).expect("compile annotated");
        let out_b = compile_src(src_bare).expect("compile bare");
        assert_eq!(
            has_e4(&out_a),
            norms[4] < 0.5,
            "[prob:1.0] steps must each consume one draw (E4 uses draw 5)"
        );
        assert_eq!(
            has_e4(&out_b),
            norms[0] < 0.5,
            "unannotated steps must consume no draw (E4 uses draw 1)"
        );
        assert_ne!(
            has_e4(&out_a),
            has_e4(&out_b),
            "[prob:1.0] must shift downstream draws"
        );
        // All four C4 steps always play in both sources.
        for out in [&out_a, &out_b] {
            let c4_count = out
                .events
                .iter()
                .filter(|e| matches!(e.event, MidiEvent::NoteOn { note, .. } if note == 60))
                .count();
            assert_eq!(c4_count, 4, "[prob:1.0] steps always play");
        }
    }

    // ── Pipeline humanize (spec §10.5) ──

    /// Collect (tick, note, velocity) for NoteOn events.
    fn note_ons(output: &CompileOutput) -> Vec<(u64, u8, u8)> {
        output
            .events
            .iter()
            .filter_map(|e| match &e.event {
                MidiEvent::NoteOn { note, velocity, .. } => Some((e.tick, *note, *velocity)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_pipeline_humanize_is_applied_and_deterministic() {
        let src = "@ppq 480\n\n@pattern p steps=4 unit=1/4\nC4\nE4\nG4\nC5\n\n\
                   @track t ch=1\nplay: p -> humanize(20%, 1.0)\n";
        let out1 = compile_src(src).expect("compile 1");
        let out2 = compile_src(src).expect("compile 2");
        assert_eq!(
            note_ons(&out1),
            note_ons(&out2),
            "humanize must be deterministic under a fixed seed"
        );

        // And it must actually do something: compare with the same source
        // without the pipe.
        let src_plain = "@ppq 480\n\n@pattern p steps=4 unit=1/4\nC4\nE4\nG4\nC5\n\n\
                         @track t ch=1\nplay: p\n";
        let out_plain = compile_src(src_plain).expect("compile plain");
        assert_ne!(
            note_ons(&out1),
            note_ons(&out_plain),
            "pipeline humanize must not be a no-op"
        );
    }

    #[test]
    fn test_two_tracks_without_seed_derive_different_streams() {
        // FNV-1a per-track derivation: two identical tracks (no seed=)
        // must not humanize in lockstep.
        let src = "@ppq 480\n\n@pattern p steps=4 unit=1/4\nC4\nE4\nG4\nC5\n\n\
                   @track a ch=1\nplay: p -> humanize(20%, 1.0)\n\n\
                   @track b ch=2\nplay: p -> humanize(20%, 1.0)\n";
        let out = compile_src(src).expect("compile");
        let track_a: Vec<(u64, u8, u8)> = out
            .events
            .iter()
            .filter(|e| e.track == 1)
            .filter_map(|e| match &e.event {
                MidiEvent::NoteOn { note, velocity, .. } => Some((e.tick, *note, *velocity)),
                _ => None,
            })
            .collect();
        let track_b: Vec<(u64, u8, u8)> = out
            .events
            .iter()
            .filter(|e| e.track == 2)
            .filter_map(|e| match &e.event {
                MidiEvent::NoteOn { note, velocity, .. } => Some((e.tick, *note, *velocity)),
                _ => None,
            })
            .collect();
        assert_eq!(track_a.len(), 4);
        assert_eq!(track_b.len(), 4);
        assert_ne!(
            track_a, track_b,
            "tracks without seed= must derive distinct per-track seeds"
        );
    }

    #[test]
    fn test_explicit_track_seed_wins_verbatim() {
        // Two tracks with the same explicit seed= produce identical
        // humanize streams (modulo channel).
        let src = "@ppq 480\n\n@pattern p steps=4 unit=1/4\nC4\nE4\nG4\nC5\n\n\
                   @track a ch=1 seed=99\nplay: p -> humanize(20%, 1.0)\n\n\
                   @track b ch=1 seed=99\nplay: p -> humanize(20%, 1.0)\n";
        let out = compile_src(src).expect("compile");
        let per_track = |tn: usize| -> Vec<(u64, u8, u8)> {
            out.events
                .iter()
                .filter(|e| e.track == tn)
                .filter_map(|e| match &e.event {
                    MidiEvent::NoteOn { note, velocity, .. } => Some((e.tick, *note, *velocity)),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            per_track(1),
            per_track(2),
            "explicit seed= must be used verbatim"
        );
    }

    #[test]
    fn test_humanize_correlation_late_notes_tend_softer() {
        // Spec §10.5: velocity variation is correlated with timing
        // variation at coefficient 0.4 — a note pushed late tends to be
        // slightly softer. Over many draws with a fixed seed the sample
        // correlation between tick offset and velocity offset must be
        // clearly negative.
        let mut st = transform::seed_state(42);
        let bpm_lookup = [(0u64, 120.0f64)];
        let mut ticks = Vec::new();
        let mut vels = Vec::new();
        for _ in 0..2000 {
            let (t, v) = humanize_note_offsets(
                &TimingValue::Percent(50.0),
                1.0,
                480,
                480,
                &bpm_lookup,
                0,
                &mut st,
            );
            ticks.push(t as f64);
            vels.push(v as f64);
        }
        let n = ticks.len() as f64;
        let mean = |xs: &[f64]| xs.iter().sum::<f64>() / n;
        let (mt, mv) = (mean(&ticks), mean(&vels));
        let mut cov = 0.0;
        let mut var_t = 0.0;
        let mut var_v = 0.0;
        for i in 0..ticks.len() {
            cov += (ticks[i] - mt) * (vels[i] - mv);
            var_t += (ticks[i] - mt).powi(2);
            var_v += (vels[i] - mv).powi(2);
        }
        let corr = cov / (var_t.sqrt() * var_v.sqrt());
        assert!(
            corr < -0.2,
            "late notes must tend softer (expected correlation near -0.4, got {corr:.3})"
        );
    }

    #[test]
    fn test_baked_humanize_rerolls_per_reference() {
        // Spec §10.5: a humanize baked into a @pattern declaration produces
        // different humanization for each reference in a play: expression.
        let src = "@ppq 480\n\n@pattern p steps=1 unit=1/4 -> humanize(50%, 1.0)\nC4\n\n\
                   @track t ch=1\nplay: p * 2\n";
        let out = compile_src(src).expect("compile");
        let ons = note_ons(&out);
        assert_eq!(ons.len(), 2);
        let offset_1 = ons[0].0 as i64;
        let offset_2 = ons[1].0 as i64 - 480;
        assert!(
            offset_1 != offset_2 || ons[0].2 != ons[1].2,
            "baked-in humanize must re-roll per reference \
             (got identical offsets {offset_1} and velocities {})",
            ons[0].2
        );
    }

    // ── Variant pools and vary() (spec §7.11, §10.5) ──

    #[test]
    fn test_variant_pool_without_vary_takes_first_deterministically() {
        // Spec §7.11: without vary(), the first alternative is always used.
        let src = "@ppq 480\n\n@pattern v steps=2 unit=1/4\n{C4, E4, G4}\n{E4, G4}\n\n\
                   @track t ch=1\nplay: v\n";
        let out = compile_src(src).expect("compile");
        let ons = note_ons(&out);
        assert_eq!(
            ons.iter().map(|(_, n, _)| *n).collect::<Vec<_>>(),
            vec![60, 64],
            "bare variant pools must take the first alternative"
        );
    }

    #[test]
    fn test_vary_selects_seeded_alternatives() {
        // vary(1.0): every variant step picks uniformly among its
        // alternatives, one RNG draw per variant step. Mirror the
        // compiler's stream: resolved_seed=1, track index 0.
        let mut st = transform::seed_state(transform::fnv1a_derive(1, 0));
        let expected: Vec<u8> = (0..4)
            .map(|_| {
                let u = transform::xorshift64_f64(&mut st);
                let idx = ((u * 3.0) as usize).min(2);
                [60u8, 64, 67][idx]
            })
            .collect();
        assert!(
            expected.iter().any(|&n| n != 60),
            "test premise: the seeded selection must pick a non-first alternative"
        );

        let src = "@ppq 480\n\n@pattern v steps=4 unit=1/4\n\
                   {C4, E4, G4}\n{C4, E4, G4}\n{C4, E4, G4}\n{C4, E4, G4}\n\n\
                   @track t ch=1\nplay: v -> vary(1.0)\n";
        let out = compile_src(src).expect("compile");
        let notes: Vec<u8> = note_ons(&out).iter().map(|(_, n, _)| *n).collect();
        assert_eq!(
            notes, expected,
            "vary(1.0) selection must match the seeded RNG stream"
        );
    }

    #[test]
    fn test_vary_low_probability_matches_seeded_rolls() {
        // vary(p): one draw u per variant step; u >= p keeps the first
        // alternative, u < p re-selects uniformly via u/p. Mirror the
        // compiler's stream for the exact expectation.
        let p = 0.1;
        let mut st = transform::seed_state(transform::fnv1a_derive(1, 0));
        let expected: Vec<u8> = (0..4)
            .map(|_| {
                let u = transform::xorshift64_f64(&mut st);
                if u < p {
                    let idx = (((u / p) * 3.0) as usize).min(2);
                    [60u8, 64, 67][idx]
                } else {
                    60
                }
            })
            .collect();

        let src = "@ppq 480\n\n@pattern v steps=4 unit=1/4\n\
                   {C4, E4, G4}\n{C4, E4, G4}\n{C4, E4, G4}\n{C4, E4, G4}\n\n\
                   @track t ch=1\nplay: v -> vary(0.1)\n";
        let out = compile_src(src).expect("compile");
        let notes: Vec<u8> = note_ons(&out).iter().map(|(_, n, _)| *n).collect();
        assert_eq!(
            notes, expected,
            "vary(0.1) must follow the seeded rolls exactly"
        );
    }

    // ── Pipeline swing scoping (spec §10.3) ──

    #[test]
    fn test_pipeline_swing_scopes_to_its_pattern() {
        // Track-level swing applies to `b`; pipeline swing overrides it for
        // `a` only. ppq 480, 1/8 unit: swing unit ticks = 240.
        // Track swing 0.6 → shift 48; pipeline swing 0.75 → shift 120.
        let src = "@ppq 480\n\n@pattern a steps=2 unit=1/8\nC4\nE4\n\n\
                   @pattern b steps=2 unit=1/8\nG4\nC5\n\n\
                   @track t ch=1 swing=0.6 swingunit=1/8\n\
                   play: (a -> swing(0.75, 1/8)) >> b\n";
        let out = compile_src(src).expect("compile");
        let ons = note_ons(&out);
        let ticks: Vec<u64> = ons.iter().map(|(t, _, _)| *t).collect();
        // a: steps at 0 and 240; the off-beat step gets the pipeline shift
        // (240 + 120 = 360). b: steps at 480 and 720; the off-beat step
        // gets the track-level shift (720 + 48 = 768).
        assert_eq!(
            ticks,
            vec![0, 360, 480, 768],
            "pipeline swing must override track swing for its pattern only"
        );
    }

    // ── Subdivision remainder (last slot absorbs) ──

    #[test]
    fn test_subdivision_last_slot_absorbs_remainder() {
        // 480 ticks / 7 tokens = 68 r 4: the last slot must be 72 ticks so
        // the bracket totals exactly 480 — no vanished ticks, no overlap
        // with the next step.
        let src = "@ppq 480\n\n@pattern p steps=2 unit=1/4 gate=1.0\n\
                   (C4 D4 E4 F4 G4 A4 B4)\nC5\n\n@track t ch=1\nplay: p\n";
        let out = compile_src(src).expect("compile");
        let ons: Vec<u64> = note_ons(&out).iter().map(|(t, _, _)| *t).collect();
        assert_eq!(
            ons,
            vec![0, 68, 136, 204, 272, 340, 408, 480],
            "slot onsets: 6 slots of 68 ticks, last slot starts at 408"
        );
        // Last subdivision note (B4=71) ends exactly at the next step start.
        let b4_off = note_events(&out)
            .into_iter()
            .find(|(_, is_on, n)| !*is_on && *n == 71)
            .expect("B4 off");
        assert_eq!(
            b4_off.0, 480,
            "last slot must absorb the remainder (68 + 4 = 72 ticks)"
        );
    }

    #[test]
    fn test_nested_subdivision_remainder_recurses() {
        // ((C4 D4 E4) F4): outer splits 480 → 240 + 240; the inner bracket
        // splits its 240-tick slot as 80/80/80 (exact); with a 5-way outer
        // the remainder handling recurses per level. Here verify a nested
        // odd split: outer (x y z) of 480 → 160,160,160; inner (a b) shares
        // exactly. Use 7 inner over the 160-tick middle slot: 160/7=22 r 6,
        // last inner slot = 160 - 6*22 = 28 → ends exactly at 320.
        let src = "@ppq 480\n\n@pattern p steps=1 unit=1/4 gate=1.0\n\
                   (C4 (C4 C4 C4 C4 C4 C4 C4) C4)\n\n@track t ch=1\nplay: p\n";
        let out = compile_src(src).expect("compile");
        let evs = note_events(&out);
        // Last inner note's off must land exactly where the outer's third
        // slot begins (320), and the final note's off exactly at 480.
        let ons: Vec<u64> = evs
            .iter()
            .filter(|(_, o, _)| *o)
            .map(|(t, _, _)| *t)
            .collect();
        assert_eq!(ons[0], 0);
        assert_eq!(ons[8], 320, "third outer slot starts at 320");
        let inner_last_off = evs
            .iter()
            .filter(|(t, o, _)| !*o && *t <= 320 && *t > 298)
            .map(|(t, _, _)| *t)
            .max()
            .expect("inner last off");
        assert_eq!(
            inner_last_off, 320,
            "inner bracket's last slot absorbs its remainder up to the outer slot end"
        );
        let final_off = evs.iter().filter(|(_, o, _)| !*o).map(|(t, _, _)| *t).max();
        assert_eq!(final_off, Some(480), "bracket total is exactly the step");
    }

    // ── Arp coarse rate (rate >= step unit) ──

    #[test]
    fn test_arp_coarse_rate_emits_one_slot_per_chord_step() {
        // unit 1/16 = 120 ticks, default arp rate 1/8 = 240 ticks: coarser
        // than the step. Must emit ONE slot per chord step (first arp tone,
        // clamped to the step), not silence.
        let src = "@ppq 480\n\n@scale root=C mode=major\n\n@harmony main\nI\n\n\
                   @pattern p steps=2 unit=1/16 gate=1.0\n$chord\n$chord\n\n\
                   @track t ch=1 follow=main\nplay: p -> arp(pattern=up)\n";
        let out = compile_src(src).expect("compile");
        assert_eq!(
            note_events(&out),
            vec![
                (0, true, 60),
                (120, false, 60),
                (120, true, 60),
                (240, false, 60)
            ],
            "each chord step sounds the first arp tone for the full step, clamped"
        );
    }

    // ── Echo clamps at pattern-instance boundaries ──

    #[test]
    fn test_echo_clamped_at_pattern_instance_boundary() {
        // Instance 1 covers [0, 1920), instance 2 [1920, 3840). The source
        // note at 1440 gets ONE echo at 1680 (inside its instance); the
        // echo at 1920 would cross the boundary and is dropped, and the
        // surviving echo's NoteOff is clamped at 1920. Instance 2's source
        // at 3360 echoes at 3600, clamped at 3840.
        let src = "@ppq 480\n\n@pattern p steps=4 unit=1/4 gate=1.0\n.\n.\n.\nC4\n\n\
                   @track t ch=1\nplay: p * 2 -> echo(1/8, 3, 0.6)\n";
        let out = compile_src(src).expect("compile");
        let ons: Vec<u64> = note_ons(&out).iter().map(|(t, _, _)| *t).collect();
        assert_eq!(
            ons,
            vec![1440, 1680, 3360, 3600],
            "echoes within the instance stay; copies crossing the boundary are dropped"
        );
        let max_inst1_off = note_events(&out)
            .iter()
            .filter(|(t, is_on, _)| !*is_on && *t <= 1920)
            .map(|(t, _, _)| *t)
            .max();
        assert_eq!(
            max_inst1_off,
            Some(1920),
            "instance-1 echo NoteOff clamps exactly at the instance end"
        );
        assert!(
            note_events(&out).iter().all(|(t, _, _)| *t <= 3840),
            "no event may cross the final instance end"
        );
    }

    // ── BarMarkers decoupled from step alignment ──

    /// Collect (tick, bar) for BarMarker events on a given track.
    fn bar_markers(output: &CompileOutput, track: usize) -> Vec<(u64, u32)> {
        output
            .events
            .iter()
            .filter_map(|e| match &e.event {
                MidiEvent::BarMarker { bar } if e.track == track => Some((e.tick, *bar)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_bar_markers_emitted_for_nonaligned_rate() {
        // rate=1.3 → effective unit 369 ticks: no step ever re-aligns to
        // the 1920-tick bar grid after tick 0, yet every bar boundary in
        // the track's range must still get a marker (RT hot-swap / bar
        // seeking depend on them).
        let src = "@ppq 480\n@ts 4/4\n\n@pattern p steps=8 unit=1/4\n\
                   C4\nD4\nE4\nF4\nG4\nA4\nB4\nC5\n\n\
                   @track t ch=1 rate=1.3\nplay: p\n";
        let out = compile_src(src).expect("compile");
        // Track total = 8 * 369 = 2952 ticks → bars 1 (tick 0) and 2 (1920).
        assert_eq!(bar_markers(&out, 1), vec![(0, 1), (1920, 2)]);
    }

    #[test]
    fn test_bar_markers_identical_for_aligned_track() {
        // A step-aligned track must produce exactly the markers the old
        // per-step check emitted: one per bar start in range, none at the
        // track end boundary.
        let src = "@ppq 480\n@ts 4/4\n\n@pattern p steps=4 unit=1/4\n\
                   C4\nE4\nG4\nC5\n\n@track t ch=1\nplay: p * 2\n";
        let out = compile_src(src).expect("compile");
        assert_eq!(bar_markers(&out, 1), vec![(0, 1), (1920, 2)]);
    }

    // ── Evolve offset range (documented [-4, +4]) ──

    #[test]
    fn test_evolve_offsets_within_documented_range_and_reach_extremes() {
        let mut state = crate::transform::seed_state(42);
        let offsets = compute_evolve_offsets(400, 1.0, &mut state);
        assert!(
            offsets.iter().all(|&o| (-4..=4).contains(&o)),
            "every evolve offset must lie in the documented [-4, +4] range"
        );
        assert!(
            offsets.contains(&4),
            "+4 must be reachable (the old 3-bit mask capped the range at +3)"
        );
        assert!(offsets.contains(&-4), "-4 must be reachable");
    }

    #[test]
    fn test_evolve_zero_toggle_is_deterministic() {
        let mut s1 = crate::transform::seed_state(7);
        let mut s2 = crate::transform::seed_state(7);
        assert_eq!(
            compute_evolve_offsets(64, 0.0, &mut s1),
            compute_evolve_offsets(64, 0.0, &mut s2),
            "toggle=0.0 locks the sequence (spec §10.5)"
        );
    }

    // ── gate_curve pairing and per-step unit ──

    #[test]
    fn test_gate_curve_uses_per_step_effective_unit() {
        // p >> p@2.0: instance 1's step has effective unit 480, instance
        // 2's has 240 (per-reference rate). A constant gate of 0.5 must
        // yield durations 240 and 120 respectively — the old code used the
        // track-level unit (480) for both.
        let src = "@ppq 480\n\n@pattern p steps=1 unit=1/4 gate=1.0\nC4\n\n\
                   @track t ch=1\nplay: p >> p@2.0 -> gate_curve(wave=ramp, min=0.5, max=0.5)\n";
        let out = compile_src(src).expect("compile");
        assert_eq!(
            note_events(&out),
            vec![
                (0, true, 60),
                (240, false, 60),
                (480, true, 60),
                (600, false, 60)
            ],
            "gate must be sized from each step's effective unit"
        );
    }

    #[test]
    fn test_gate_curve_retargets_each_notes_own_off() {
        // Tied C4 (2 steps) then E4: each NoteOn's OWN deferred NoteOff is
        // retargeted (paired via step_index), and the gate basis is the
        // owning step's unit — the tie extension is overridden by the curve.
        let src = "@ppq 480\n\n@pattern p steps=4 unit=1/4 gate=1.0\nC4\n~\nE4\n.\n\n\
                   @track t ch=1\nplay: p -> gate_curve(wave=ramp, min=0.5, max=0.5)\n";
        let out = compile_src(src).expect("compile");
        let evs = note_events(&out);
        assert_eq!(
            evs,
            vec![
                (0, true, 60),
                (240, false, 60),
                (960, true, 64),
                (1200, false, 64)
            ],
            "each note's own NoteOff is retargeted; no cross-note retarget"
        );
    }

    // ── scale_lock(mode=filter) removes only the filtered pair ──

    #[test]
    fn test_scale_lock_filter_removes_only_filtered_pairs() {
        // Two out-of-scale C#4 notes plus in-scale notes: exactly the two
        // C#4 on/off pairs disappear; every remaining NoteOn keeps its own
        // NoteOff (no orphaned offs, no stuck notes).
        let src = "@ppq 480\n\n@scale root=C mode=major\n\n\
                   @pattern p steps=4 unit=1/4 gate=1.0\nC4\nDb4\nE4\nDb4\n\n\
                   @track t ch=1\nplay: p -> scale_lock(mode=filter)\n";
        let out = compile_src(src).expect("compile");
        let evs = note_events(&out);
        assert_eq!(
            evs,
            vec![
                (0, true, 60),
                (480, false, 60),
                (960, true, 64),
                (1440, false, 64)
            ],
            "only the out-of-scale pairs are removed; in-scale pairs stay intact"
        );
        // Balance check: equal on/off counts per pitch.
        let mut on_count = 0i32;
        for (_, is_on, _) in &evs {
            on_count += if *is_on { 1 } else { -1 };
            assert!(on_count >= 0, "NoteOff without a prior NoteOn");
        }
        assert_eq!(on_count, 0, "every NoteOn must retain its NoteOff");
    }
}
