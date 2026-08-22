//! Golden file test harness for Interval.
//!
//! Discovers all directories under `tests/golden/`, reads `input.interval`,
//! compiles it, and compares the output to `expected.json`.
//!
//! Run with `cargo test --test golden_tests`.
//!
//! To update expected outputs after intentional changes:
//! `cargo test --test golden_tests -- --ignored`
//! (runs the `update_golden` test which overwrites expected.json files)

use interval_core::ast::{
    Block, DrumMapBlock, GlobalHeader, PatternBlock, TonalContext, TrackBlock,
};
use interval_core::compiler::{self, resolve_step_pitches, BarLayout};
use interval_core::harmony::{HarmonyIndex, ScaleTimeline};
use interval_core::lexer::tokenize;
use interval_core::parser::{parse_header, Parser};
use interval_core::pattern::{resolve_all, ResolvedPattern};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

// ── Output types for golden tests ────────────────────────────────────

/// Output format for header-only golden tests (Phase 1).
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct HeaderOutput {
    ppq: u32,
    bpm: f64,
    ts_numerator: u8,
    ts_denominator: u8,
    title: Option<String>,
    seed: Option<u64>,
}

impl From<&GlobalHeader> for HeaderOutput {
    fn from(h: &GlobalHeader) -> Self {
        Self {
            ppq: h.ppq,
            bpm: h.bpm,
            ts_numerator: h.ts_numerator,
            ts_denominator: h.ts_denominator,
            title: h.title.clone(),
            seed: h.seed,
        }
    }
}

/// Output format for harmony golden tests (Phase 2).
/// Each span records the tick range and chord root + intervals.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct HarmonyOutput {
    header: HeaderOutput,
    harmony_name: String,
    total_ticks: u64,
    spans: Vec<HarmonySpanOutput>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
struct HarmonySpanOutput {
    start_tick: u64,
    end_tick: u64,
    chord_root: u8,
    intervals: Vec<u8>,
    mode_intervals: Vec<u8>,
    scale_root: u8,
}

/// Generic expected result: success (JSON value) or error substring.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum ExpectedResult {
    Error { error: String },
    Value(serde_json::Value),
}

// ── Test discovery ───────────────────────────────────────────────────

fn discover_golden_dirs() -> Vec<PathBuf> {
    let golden_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    if !golden_dir.exists() {
        return Vec::new();
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(&golden_dir)
        .expect("failed to read golden directory") // safe: test code
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                Some(path)
            } else {
                None
            }
        })
        .collect();
    dirs.sort();
    dirs
}

/// Determine test type from directory name prefix.
fn test_type(dir: &Path) -> &'static str {
    let name = dir.file_name().unwrap_or_default().to_string_lossy(); // safe: test code
    if name.contains("header") {
        "header"
    } else if name.contains("event") || name.contains("timing") {
        "event"
    } else if name.contains("harmony") {
        "harmony"
    } else if name.contains("compiler") || name.contains("resolve") || name.contains("voicing") {
        "compiler"
    } else if name.contains("compose") {
        "compose"
    } else if name.contains("drummap") {
        "drummap"
    } else if name.contains("drum_track") || name.contains("track") {
        "track"
    } else if name.contains("pattern") {
        "pattern"
    } else {
        "header" // default fallback
    }
}

// ── Compilation helpers ──────────────────────────────────────────────

fn compile_header(source: &str) -> Result<(GlobalHeader, Parser), String> {
    let (tokens, lex_errors) = tokenize(source);
    if !lex_errors.is_empty() {
        return Err(format!("lexer errors at: {lex_errors:?}"));
    }
    parse_header(tokens).map_err(|e| e.to_string())
}

fn compile_harmony(source: &str) -> Result<(HeaderOutput, HarmonyOutput), String> {
    let (header, mut parser) = compile_header(source)?;
    let header_out = HeaderOutput::from(&header);

    parser.skip_newlines_pub();

    // Parse @scale block if present
    let tonal = if parser.peek_is_scale() {
        let tc = parser.parse_scale_block().map_err(|e| e.to_string())?;
        parser.skip_newlines_pub();
        tc
    } else {
        TonalContext::default()
    };

    let block = parser.parse_harmony_block().map_err(|e| e.to_string())?;
    let scale_tl = ScaleTimeline::from_tonal_context(&tonal).map_err(|e| e.to_string())?;
    let bar_layout = BarLayout::from_header(&header);
    let index =
        HarmonyIndex::build(&block, &header, &scale_tl, &bar_layout).map_err(|e| e.to_string())?;

    let spans: Vec<HarmonySpanOutput> = index
        .spans()
        .iter()
        .map(|s| HarmonySpanOutput {
            start_tick: s.start_tick,
            end_tick: s.end_tick,
            chord_root: s.context.chord.root,
            intervals: s.context.chord.intervals.clone(),
            mode_intervals: s.context.mode_intervals.clone(),
            scale_root: s.context.scale_root,
        })
        .collect();

    Ok((
        header_out,
        HarmonyOutput {
            header: HeaderOutput::from(&header),
            harmony_name: index.name.clone(),
            total_ticks: index.total_ticks,
            spans,
        },
    ))
}

/// Output format for compose golden tests (Phase 4).
#[derive(Debug, Serialize)]
struct ComposeOutput {
    patterns: Vec<ComposePatternOutput>,
}

#[derive(Debug, Serialize)]
struct ComposePatternOutput {
    name: String,
    resolved: ResolvedPattern,
}

fn compile_pattern(source: &str) -> Result<PatternBlock, String> {
    let (header, mut parser) = compile_header(source)?;
    let _ = header; // pattern tests don't need header output
    parser.skip_newlines_pub();
    parser.parse_pattern_block().map_err(|e| e.to_string())
}

fn compile_compose(source: &str) -> Result<ComposeOutput, String> {
    let (_header, mut parser) = compile_header(source)?;
    parser.skip_newlines_pub();

    // Parse all pattern blocks
    let mut blocks = Vec::new();
    while parser.has_tokens() {
        parser.skip_newlines_pub();
        if !parser.has_tokens() {
            break;
        }
        let block = parser.parse_pattern_block().map_err(|e| e.to_string())?;
        blocks.push(block);
        parser.skip_newlines_pub();
    }

    let resolved = resolve_all(&blocks).map_err(|e| e.to_string())?;

    // Output in declaration order
    let patterns = blocks
        .iter()
        .map(|b| ComposePatternOutput {
            name: b.name.clone(),
            resolved: resolved[&b.name].clone(),
        })
        .collect();

    Ok(ComposeOutput { patterns })
}

/// Output format for compiler golden tests (Phase 8).
/// Resolves each step in each pattern against the harmony context.
#[derive(Debug, Serialize)]
struct CompilerOutput {
    patterns: Vec<CompilerPatternOutput>,
}

#[derive(Debug, Serialize)]
struct CompilerPatternOutput {
    name: String,
    steps: Vec<CompilerStepOutput>,
}

#[derive(Debug, Serialize)]
struct CompilerStepOutput {
    pitches: Vec<u8>,
}

fn compile_compiler(source: &str) -> Result<CompilerOutput, String> {
    let (header, mut parser) = compile_header(source)?;
    parser.skip_newlines_pub();

    // Parse @scale block if present
    let tonal = if parser.peek_is_scale() {
        let tc = parser.parse_scale_block().map_err(|e| e.to_string())?;
        parser.skip_newlines_pub();
        tc
    } else {
        TonalContext::default()
    };

    // Build scale timeline for pitch resolution and harmony building
    let scale_tl = ScaleTimeline::from_tonal_context(&tonal).map_err(|e| e.to_string())?;
    let (scale_mode_ivs_owned, base_scale_root): (Vec<u8>, u8) = {
        let (ivs, root) = scale_tl.context_at_bar(1);
        (ivs.to_vec(), root)
    };

    // Parse harmony block if present
    let bar_layout = BarLayout::from_header(&header);
    let harmony_index = if parser.peek_is_harmony() {
        let block = parser.parse_harmony_block().map_err(|e| e.to_string())?;
        let index = HarmonyIndex::build(&block, &header, &scale_tl, &bar_layout)
            .map_err(|e| e.to_string())?;
        parser.skip_newlines_pub();
        Some(index)
    } else {
        None
    };

    // Parse all pattern blocks
    let mut blocks = Vec::new();
    while parser.has_tokens() {
        parser.skip_newlines_pub();
        if !parser.has_tokens() {
            break;
        }
        if parser.peek_is_pattern() {
            let block = parser.parse_pattern_block().map_err(|e| e.to_string())?;
            blocks.push(block);
        } else {
            break;
        }
        parser.skip_newlines_pub();
    }

    let resolved = resolve_all(&blocks).map_err(|e| e.to_string())?;
    let default_octave = 4u8;

    let patterns = blocks
        .iter()
        .map(|b| {
            let res = &resolved[&b.name];
            let steps: Vec<CompilerStepOutput> = res
                .steps
                .iter()
                .map(|step| {
                    // Use the first token in the step for pitch resolution
                    let pitches = if let Some(token) = step.tokens.first() {
                        // Query harmony at tick 0 for simplicity (single-bar tests)
                        let context = harmony_index.as_ref().and_then(|idx| idx.query(0));
                        resolve_step_pitches(
                            token,
                            context,
                            &scale_mode_ivs_owned,
                            base_scale_root,
                            default_octave,
                        )
                    } else {
                        Vec::new()
                    };
                    CompilerStepOutput { pitches }
                })
                .collect();
            CompilerPatternOutput {
                name: b.name.clone(),
                steps,
            }
        })
        .collect();

    Ok(CompilerOutput { patterns })
}

/// Output format for event stream golden tests (Phase 9).
#[derive(Debug, Serialize)]
struct EventOutput {
    ppq: u32,
    events: Vec<EventEntry>,
    /// Warning messages, omitted from JSON when empty (so existing tests stay unchanged).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct EventEntry {
    tick: u64,
    track: usize,
    event: interval_core::event::MidiEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    condition: Option<interval_core::ast::StepCondition>,
}

fn compile_events(source: &str) -> Result<EventOutput, String> {
    let (header, mut parser) = compile_header(source)?;
    parser.skip_newlines_pub();

    // Parse all blocks
    let mut blocks = Vec::new();
    while parser.has_tokens() {
        parser.skip_newlines_pub();
        if !parser.has_tokens() {
            break;
        }
        if parser.peek_is_scale() {
            let tc = parser.parse_scale_block().map_err(|e| e.to_string())?;
            blocks.push(Block::Scale(tc));
        } else if parser.peek_is_harmony() {
            let block = parser.parse_harmony_block().map_err(|e| e.to_string())?;
            blocks.push(Block::Harmony(block));
        } else if parser.peek_is_pattern() {
            let block = parser.parse_pattern_block().map_err(|e| e.to_string())?;
            blocks.push(Block::Pattern(block));
        } else if parser.peek_is_track() {
            let block = parser.parse_track_block().map_err(|e| e.to_string())?;
            blocks.push(Block::Track(block));
        } else if parser.peek_is_drummap() {
            let block = parser.parse_drummap_block().map_err(|e| e.to_string())?;
            blocks.push(Block::DrumMap(block));
        } else if parser.peek_is_tempo() {
            // @tempo is a hard error everywhere in v0.5+ (mirrors parse_only).
            return Err(interval_core::error::CompileError::DeprecatedTempo {
                span: parser.current_span(),
            }
            .to_string());
        } else {
            return Err(parser.error_unexpected_block().to_string());
        }
        parser.skip_newlines_pub();
    }

    let output = compiler::compile(&header, &blocks).map_err(|e| e.to_string())?;

    let events = output
        .events
        .into_iter()
        .map(|e| EventEntry {
            tick: e.tick,
            track: e.track,
            event: e.event,
            condition: e.condition,
        })
        .collect();

    let warnings: Vec<String> = output.warnings.iter().map(|w| w.to_string()).collect();
    Ok(EventOutput {
        ppq: output.ppq,
        events,
        warnings,
    })
}

fn compile_drummap(source: &str) -> Result<DrumMapBlock, String> {
    let (_header, mut parser) = compile_header(source)?;
    parser.skip_newlines_pub();
    parser.parse_drummap_block().map_err(|e| e.to_string())
}

fn compile_track(source: &str) -> Result<TrackBlock, String> {
    let (_header, mut parser) = compile_header(source)?;
    parser.skip_newlines_pub();
    parser.parse_track_block().map_err(|e| e.to_string())
}

// ── Test runner ──────────────────────────────────────────────────────

fn run_golden_test(dir: &Path) -> Result<(), String> {
    let input_path = dir.join("input.interval");
    let expected_path = dir.join("expected.json");
    let test_name = dir.file_name().unwrap_or_default().to_string_lossy(); // safe: test code

    if !input_path.exists() {
        return Err(format!("{test_name}: missing input.interval"));
    }
    if !expected_path.exists() {
        return Err(format!("{test_name}: missing expected.json"));
    }

    let source = fs::read_to_string(&input_path)
        .map_err(|e| format!("{test_name}: failed to read input: {e}"))?;
    let expected_json = fs::read_to_string(&expected_path)
        .map_err(|e| format!("{test_name}: failed to read expected.json: {e}"))?;
    let expected: ExpectedResult = serde_json::from_str(&expected_json)
        .map_err(|e| format!("{test_name}: failed to parse expected.json: {e}"))?;

    let tt = test_type(dir);

    match tt {
        "header" => run_header_test(&test_name, &source, &expected, &expected_json),
        "harmony" => run_harmony_test(&test_name, &source, &expected, &expected_json),
        "pattern" => run_pattern_test(&test_name, &source, &expected, &expected_json),
        "compose" => run_compose_test(&test_name, &source, &expected, &expected_json),
        "compiler" => run_compiler_test(&test_name, &source, &expected, &expected_json),
        "event" => run_event_test(&test_name, &source, &expected, &expected_json),
        "track" => run_track_test(&test_name, &source, &expected, &expected_json),
        "drummap" => run_drummap_test(&test_name, &source, &expected, &expected_json),
        _ => Err(format!("{test_name}: unknown test type '{tt}'")),
    }
}

fn run_header_test(
    test_name: &str,
    source: &str,
    expected: &ExpectedResult,
    expected_json: &str,
) -> Result<(), String> {
    match compile_header(source) {
        Ok((header, _)) => {
            let actual = HeaderOutput::from(&header);
            let actual_value = serde_json::to_value(&actual)
                .map_err(|e| format!("{test_name}: serialization error: {e}"))?;
            check_success(test_name, &actual_value, expected, expected_json)
        }
        Err(err_msg) => check_error(test_name, &err_msg, expected),
    }
}

fn run_harmony_test(
    test_name: &str,
    source: &str,
    expected: &ExpectedResult,
    expected_json: &str,
) -> Result<(), String> {
    match compile_harmony(source) {
        Ok((_, harmony_out)) => {
            let actual_value = serde_json::to_value(&harmony_out)
                .map_err(|e| format!("{test_name}: serialization error: {e}"))?;
            check_success(test_name, &actual_value, expected, expected_json)
        }
        Err(err_msg) => check_error(test_name, &err_msg, expected),
    }
}

fn run_pattern_test(
    test_name: &str,
    source: &str,
    expected: &ExpectedResult,
    expected_json: &str,
) -> Result<(), String> {
    match compile_pattern(source) {
        Ok(pattern) => {
            let actual_value = serde_json::to_value(&pattern)
                .map_err(|e| format!("{test_name}: serialization error: {e}"))?;
            check_success(test_name, &actual_value, expected, expected_json)
        }
        Err(err_msg) => check_error(test_name, &err_msg, expected),
    }
}

fn run_compose_test(
    test_name: &str,
    source: &str,
    expected: &ExpectedResult,
    expected_json: &str,
) -> Result<(), String> {
    match compile_compose(source) {
        Ok(output) => {
            let actual_value = serde_json::to_value(&output)
                .map_err(|e| format!("{test_name}: serialization error: {e}"))?;
            check_success(test_name, &actual_value, expected, expected_json)
        }
        Err(err_msg) => check_error(test_name, &err_msg, expected),
    }
}

fn run_compiler_test(
    test_name: &str,
    source: &str,
    expected: &ExpectedResult,
    expected_json: &str,
) -> Result<(), String> {
    match compile_compiler(source) {
        Ok(output) => {
            let actual_value = serde_json::to_value(&output)
                .map_err(|e| format!("{test_name}: serialization error: {e}"))?;
            check_success(test_name, &actual_value, expected, expected_json)
        }
        Err(err_msg) => check_error(test_name, &err_msg, expected),
    }
}

fn run_event_test(
    test_name: &str,
    source: &str,
    expected: &ExpectedResult,
    expected_json: &str,
) -> Result<(), String> {
    match compile_events(source) {
        Ok(output) => {
            let actual_value = serde_json::to_value(&output)
                .map_err(|e| format!("{test_name}: serialization error: {e}"))?;
            check_success(test_name, &actual_value, expected, expected_json)
        }
        Err(err_msg) => check_error(test_name, &err_msg, expected),
    }
}

fn run_drummap_test(
    test_name: &str,
    source: &str,
    expected: &ExpectedResult,
    expected_json: &str,
) -> Result<(), String> {
    match compile_drummap(source) {
        Ok(dm) => {
            let actual_value = serde_json::to_value(&dm)
                .map_err(|e| format!("{test_name}: serialization error: {e}"))?;
            check_success(test_name, &actual_value, expected, expected_json)
        }
        Err(err_msg) => check_error(test_name, &err_msg, expected),
    }
}

fn run_track_test(
    test_name: &str,
    source: &str,
    expected: &ExpectedResult,
    expected_json: &str,
) -> Result<(), String> {
    match compile_track(source) {
        Ok(track) => {
            let actual_value = serde_json::to_value(&track)
                .map_err(|e| format!("{test_name}: serialization error: {e}"))?;
            check_success(test_name, &actual_value, expected, expected_json)
        }
        Err(err_msg) => check_error(test_name, &err_msg, expected),
    }
}

fn check_success(
    test_name: &str,
    actual_value: &serde_json::Value,
    expected: &ExpectedResult,
    expected_json: &str,
) -> Result<(), String> {
    match expected {
        ExpectedResult::Value(expected_value) => {
            if actual_value != expected_value {
                let actual_json = serde_json::to_string_pretty(actual_value).unwrap_or_default(); // safe: test code
                Err(format!(
                    "{test_name}: output mismatch\n--- expected ---\n{expected_json}\n--- actual ---\n{actual_json}"
                ))
            } else {
                Ok(())
            }
        }
        ExpectedResult::Error { error } => Err(format!(
            "{test_name}: expected error containing '{error}', but compilation succeeded"
        )),
    }
}

fn check_error(test_name: &str, err_msg: &str, expected: &ExpectedResult) -> Result<(), String> {
    match expected {
        ExpectedResult::Error {
            error: expected_msg,
        } => {
            if err_msg.contains(expected_msg.as_str()) {
                Ok(())
            } else {
                Err(format!(
                    "{test_name}: expected error containing '{expected_msg}', got: {err_msg}"
                ))
            }
        }
        ExpectedResult::Value(_) => Err(format!(
            "{test_name}: expected success, got error: {err_msg}"
        )),
    }
}

// ── Test entry points ────────────────────────────────────────────────

#[test]
fn golden_file_tests() {
    let dirs = discover_golden_dirs();
    if dirs.is_empty() {
        eprintln!("WARNING: no golden test directories found");
        return;
    }

    let mut failures = Vec::new();
    for dir in &dirs {
        if let Err(msg) = run_golden_test(dir) {
            failures.push(msg);
        }
    }

    if !failures.is_empty() {
        let count = failures.len();
        let details = failures.join("\n\n");
        panic!("{count} golden test(s) failed:\n\n{details}");
    }
}

#[test]
#[ignore]
fn update_golden() {
    eprintln!("WARNING: Updating golden expected files. Review changes carefully!");

    let dirs = discover_golden_dirs();
    for dir in &dirs {
        let input_path = dir.join("input.interval");
        let expected_path = dir.join("expected.json");
        let test_name = dir.file_name().unwrap_or_default().to_string_lossy(); // safe: test code

        if !input_path.exists() {
            eprintln!("  SKIP {test_name}: no input.interval");
            continue;
        }

        let source = fs::read_to_string(&input_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", input_path.display())); // safe: test code

        let tt = test_type(dir);
        let json = match tt {
            "header" => match compile_header(&source) {
                Ok((header, _)) => {
                    let out = HeaderOutput::from(&header);
                    serde_json::to_string_pretty(&out).unwrap_or_else(|e| panic!("JSON error: {e}"))
                    // safe: test code
                }
                Err(err) => {
                    serde_json::to_string_pretty(&serde_json::json!({"error": err}))
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
            },
            "harmony" => match compile_harmony(&source) {
                Ok((_, harmony_out)) => {
                    serde_json::to_string_pretty(&harmony_out)
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
                Err(err) => {
                    serde_json::to_string_pretty(&serde_json::json!({"error": err}))
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
            },
            "pattern" => match compile_pattern(&source) {
                Ok(pattern) => {
                    serde_json::to_string_pretty(&pattern)
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
                Err(err) => {
                    serde_json::to_string_pretty(&serde_json::json!({"error": err}))
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
            },
            "compose" => match compile_compose(&source) {
                Ok(output) => {
                    serde_json::to_string_pretty(&output)
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
                Err(err) => {
                    serde_json::to_string_pretty(&serde_json::json!({"error": err}))
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
            },
            "event" => match compile_events(&source) {
                Ok(output) => {
                    serde_json::to_string_pretty(&output)
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
                Err(err) => {
                    serde_json::to_string_pretty(&serde_json::json!({"error": err}))
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
            },
            "compiler" => match compile_compiler(&source) {
                Ok(output) => {
                    serde_json::to_string_pretty(&output)
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
                Err(err) => {
                    serde_json::to_string_pretty(&serde_json::json!({"error": err}))
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
            },
            "track" => match compile_track(&source) {
                Ok(track) => {
                    serde_json::to_string_pretty(&track)
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
                Err(err) => {
                    serde_json::to_string_pretty(&serde_json::json!({"error": err}))
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
            },
            "drummap" => match compile_drummap(&source) {
                Ok(dm) => {
                    serde_json::to_string_pretty(&dm).unwrap_or_else(|e| panic!("JSON error: {e}"))
                    // safe: test code
                }
                Err(err) => {
                    serde_json::to_string_pretty(&serde_json::json!({"error": err}))
                        .unwrap_or_else(|e| panic!("JSON error: {e}")) // safe: test code
                }
            },
            _ => {
                eprintln!("  SKIP {test_name}: unknown type '{tt}'");
                continue;
            }
        };

        fs::write(&expected_path, &json)
            .unwrap_or_else(|e| panic!("failed to write {}: {e}", expected_path.display())); // safe: test code
        eprintln!("  UPDATED {test_name}");
    }
}
