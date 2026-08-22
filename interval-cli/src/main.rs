//! Interval CLI tool.
//!
//! Subcommands:
//! - `compile input.Interval -o output.mid` — compile to SMF
//! - `play input.Interval` — real-time playback with file-watch hot-swap
//! - `check input.Interval` — validate and report errors
//! - `dump input.Interval` — print event stream as text for debugging

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use codespan_reporting::diagnostic::{Diagnostic, Label};
use codespan_reporting::files::SimpleFiles;
use codespan_reporting::term;
use codespan_reporting::term::termcolor::{ColorChoice, StandardStream};
use interval_core::compiler;
use interval_core::error::CompileError;
use interval_core::event::MidiEvent;
use interval_rt::hotswap::{CompiledStream, HotSwapSlot};
use interval_rt::scheduler::{Scheduler, SwapMode};
use interval_smf::renderer;
use notify::{EventKind, RecursiveMode, Watcher};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "interval", version, about = "Interval compiler and player")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Hot-swap seek mode for `play`.
#[derive(Clone, Copy, clap::ValueEnum)]
enum SwapModeArg {
    /// Apply at the next bar boundary, seeking to the bar start
    Immediate,
    /// Apply at the next bar boundary, seeking to the same beat in the next bar
    Next,
}

impl From<SwapModeArg> for SwapMode {
    fn from(arg: SwapModeArg) -> Self {
        match arg {
            SwapModeArg::Immediate => SwapMode::Immediate,
            SwapModeArg::Next => SwapMode::Next,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Compile a Interval file to MIDI (.mid)
    Compile {
        /// Input .interval file
        input: PathBuf,
        /// Output .mid file
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Override random seed (deterministic output)
        #[arg(long)]
        seed: Option<u64>,
    },
    /// Play a Interval file in real time with hot-swap on file changes
    Play {
        /// Input .interval file
        input: PathBuf,
        /// MIDI output port index (0-based). If omitted, lists ports for selection.
        #[arg(short, long)]
        port: Option<usize>,
        /// Override random seed (deterministic output)
        #[arg(long)]
        seed: Option<u64>,
        /// Hot-swap seek mode
        #[arg(long, value_enum, default_value_t = SwapModeArg::Immediate)]
        swap_mode: SwapModeArg,
    },
    /// Check a Interval file for errors without producing output
    Check {
        /// Input .interval file
        input: PathBuf,
    },
    /// Dump the event stream as text for debugging
    Dump {
        /// Input .interval file
        input: PathBuf,
        /// Override random seed (deterministic output)
        #[arg(long)]
        seed: Option<u64>,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Command::Compile {
            input,
            output,
            seed,
        } => cmd_compile(&input, output.as_deref(), seed),
        Command::Play {
            input,
            port,
            seed,
            swap_mode,
        } => cmd_play(&input, port, seed, swap_mode.into()),
        Command::Check { input } => cmd_check(&input),
        Command::Dump { input, seed } => cmd_dump(&input, seed),
    };

    if let Err(e) = result {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

// ── Shared compilation pipeline ──────────────────────────────────────

/// Resolve the effective seed for compilation.
/// Priority: CLI --seed flag > @seed directive > session seed (kept stable
/// across hot-swap recompiles) > ephemeral time-derived seed.
fn resolve_seed(header_seed: Option<u64>, cli_seed: Option<u64>, session_seed: Option<u64>) -> u64 {
    cli_seed
        .or(header_seed)
        .or(session_seed)
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(0x517cc1b727220a95)
        })
}

/// Parse and compile a source file. Returns the compiled output plus the
/// resolved seed, or the compile error for the caller to render.
fn compile_source(
    source: &str,
    _filename: &str,
    cli_seed: Option<u64>,
    session_seed: Option<u64>,
) -> Result<(compiler::CompileOutput, u64), CompileError> {
    let mut program = interval_core::parse_only(source)?;

    // Resolve seed: CLI flag > @seed directive > session > ephemeral
    let resolved_seed = resolve_seed(program.header.seed, cli_seed, session_seed);
    // Always make the resolved seed available for random transforms.
    program.header.resolved_seed = Some(resolved_seed);
    // Only embed seed as TextMeta in output when explicitly provided (via @seed
    // or --seed). OS-random seeds are ephemeral — embedding them would make .mid
    // files non-deterministic even when no random transforms are used.
    let seed_is_explicit = cli_seed.is_some() || program.header.seed.is_some();
    if !seed_is_explicit {
        program.header.seed = None; // already None, but be explicit
    }
    eprintln!(
        "seed: {resolved_seed}{}",
        if seed_is_explicit { "" } else { " (ephemeral)" }
    );

    compiler::compile(&program.header, &program.blocks).map(|out| (out, resolved_seed))
}

/// Render a CompileError with codespan-reporting.
fn render_error(err: &CompileError, source: &str, filename: &str) {
    let mut files = SimpleFiles::new();
    let file_id = files.add(filename, source);

    let span = error_span(err);
    let diagnostic = Diagnostic::error()
        .with_message(err.to_string())
        .with_labels(vec![
            Label::primary(file_id, span.start..span.end).with_message(err.to_string())
        ]);

    let writer = StandardStream::stderr(ColorChoice::Auto);
    let config = term::Config::default();
    let _ = term::emit(&mut writer.lock(), &config, &files, &diagnostic);
}

/// Extract the span from a CompileError.
fn error_span(err: &CompileError) -> interval_core::error::Span {
    match err {
        CompileError::StepCountMismatch { span, .. }
        | CompileError::TieWithNoPriorNote { span, .. }
        | CompileError::UndefinedHarmonyBlock { span, .. }
        | CompileError::DrumTrackWithFollow { span, .. }
        | CompileError::PlayAndSteps { span, .. }
        | CompileError::NeitherPlayNorSteps { span, .. }
        | CompileError::UnitMismatch { span, .. }
        | CompileError::ChannelOutOfRange { span, .. }
        | CompileError::VelocityOutOfRange { span, .. }
        | CompileError::GateOutOfRange { span, .. }
        | CompileError::InversionExceedsChordTones { span, .. }
        | CompileError::BeatAssignmentMismatch { span, .. }
        | CompileError::UndefinedPattern { span, .. }
        | CompileError::InterleaveMismatch { span, .. }
        | CompileError::ForwardReference { span, .. }
        | CompileError::SectionBarNotIncreasing { span, .. }
        | CompileError::SectionBarExceedsTotal { span, .. }
        | CompileError::CurrentChordWithoutFollow { span, .. }
        | CompileError::DeprecatedPipeOperator { span, .. }
        | CompileError::DeprecatedVariantPipe { span, .. }
        | CompileError::DeprecatedCurrentChordToken { span, .. }
        | CompileError::ChordOrdinalWithoutFollow { span, .. }
        | CompileError::MultipleHarmonyBlocksRequireNames { span, .. }
        | CompileError::DeprecatedTempo { span, .. }
        | CompileError::ParseError { span, .. } => *span,
    }
}

// ── Subcommands ──────────────────────────────────────────────────────

fn cmd_compile(
    input: &PathBuf,
    output: Option<&std::path::Path>,
    cli_seed: Option<u64>,
) -> Result<()> {
    let source =
        fs::read_to_string(input).with_context(|| format!("failed to read {}", input.display()))?;
    let filename = input.display().to_string();

    let compiled = match compile_source(&source, &filename, cli_seed, None) {
        Ok((c, _seed)) => c,
        Err(e) => {
            render_error(&e, &source, &filename);
            anyhow::bail!("compilation failed");
        }
    };

    let output_path = output
        .map(PathBuf::from)
        .unwrap_or_else(|| input.with_extension("mid"));

    if output_path == *input {
        anyhow::bail!(
            "output path {} would overwrite the input file; pass a different path with -o",
            output_path.display()
        );
    }

    let mut file = fs::File::create(&output_path)
        .with_context(|| format!("failed to create {}", output_path.display()))?;

    renderer::render(&compiled.events, compiled.ppq, &mut file)
        .with_context(|| "failed to write SMF")?;

    eprintln!("wrote {}", output_path.display());
    Ok(())
}

fn cmd_check(input: &PathBuf) -> Result<()> {
    let source =
        fs::read_to_string(input).with_context(|| format!("failed to read {}", input.display()))?;
    let filename = input.display().to_string();

    match compile_source(&source, &filename, None, None) {
        Ok((compiled, _seed)) => {
            let note_count = compiled
                .events
                .iter()
                .filter(|e| matches!(e.event, MidiEvent::NoteOn { .. }))
                .count();
            // Track 0 is the tempo/meta track — count only user tracks.
            let track_count = compiled
                .events
                .iter()
                .map(|e| e.track)
                .filter(|&t| t != 0)
                .collect::<std::collections::HashSet<_>>()
                .len();
            eprintln!(
                "ok: {note_count} notes, {track_count} tracks, ppq={}",
                compiled.ppq
            );
            Ok(())
        }
        Err(e) => {
            render_error(&e, &source, &filename);
            anyhow::bail!("check failed");
        }
    }
}

fn cmd_dump(input: &PathBuf, cli_seed: Option<u64>) -> Result<()> {
    let source =
        fs::read_to_string(input).with_context(|| format!("failed to read {}", input.display()))?;
    let filename = input.display().to_string();

    let compiled = match compile_source(&source, &filename, cli_seed, None) {
        Ok((c, _seed)) => c,
        Err(e) => {
            render_error(&e, &source, &filename);
            anyhow::bail!("compilation failed");
        }
    };

    println!("ppq: {}", compiled.ppq);
    println!("events: {}", compiled.events.len());
    println!("---");
    for event in &compiled.events {
        println!(
            "tick={:>6}  track={}  {}",
            event.tick,
            event.track,
            format_event(&event.event)
        );
    }
    Ok(())
}

fn format_event(event: &MidiEvent) -> String {
    match event {
        MidiEvent::NoteOn {
            channel,
            note,
            velocity,
        } => {
            format!("NoteOn  ch={channel} note={note} vel={velocity}")
        }
        MidiEvent::NoteOff { channel, note } => {
            format!("NoteOff ch={channel} note={note}")
        }
        MidiEvent::CC {
            channel,
            controller,
            value,
        } => {
            format!("CC      ch={channel} cc={controller} val={value}")
        }
        MidiEvent::ProgramChange { channel, program } => {
            format!("ProgChg ch={channel} prog={program}")
        }
        MidiEvent::PitchBend { channel, value } => {
            format!("PBend   ch={channel} val={value}")
        }
        MidiEvent::Aftertouch { channel, value } => {
            format!("AfterT  ch={channel} val={value}")
        }
        MidiEvent::Tempo { bpm } => format!("Tempo   bpm={bpm}"),
        MidiEvent::TimeSignature {
            numerator,
            denominator,
        } => {
            format!("TimeSig {numerator}/{denominator}")
        }
        MidiEvent::TrackName { name } => format!("TrkName \"{name}\""),
        MidiEvent::TextMeta { text } => format!("Text    \"{text}\""),
        MidiEvent::BarMarker { bar } => format!("Bar     {bar}"),
        MidiEvent::PatternBoundary {
            track,
            pattern_name,
        } => {
            format!("PatBnd  trk={track} pat=\"{pattern_name}\"")
        }
    }
}

fn cmd_play(
    input: &PathBuf,
    port_arg: Option<usize>,
    cli_seed: Option<u64>,
    swap_mode: SwapMode,
) -> Result<()> {
    let source =
        fs::read_to_string(input).with_context(|| format!("failed to read {}", input.display()))?;
    let filename = input.display().to_string();

    let (compiled, session_seed) = match compile_source(&source, &filename, cli_seed, None) {
        Ok(pair) => pair,
        Err(e) => {
            render_error(&e, &source, &filename);
            anyhow::bail!("compilation failed");
        }
    };

    // Open MIDI output port via interval-rt device API.
    use interval_rt::midi_devices::{connect_midi_output, list_midi_outputs};

    let ports = list_midi_outputs().context("failed to enumerate MIDI ports")?;
    if ports.is_empty() {
        anyhow::bail!("no MIDI output ports available");
    }

    let port_index = match port_arg {
        Some(idx) => {
            if idx >= ports.len() {
                eprintln!("available MIDI ports:");
                for p in &ports {
                    eprintln!("  {}: {}", p.index, p.name);
                }
                anyhow::bail!("port index {idx} out of range (0-{})", ports.len() - 1);
            }
            idx
        }
        None if ports.len() == 1 => 0,
        None => {
            eprintln!("available MIDI ports:");
            for p in &ports {
                eprintln!("  {}: {}", p.index, p.name);
            }
            eprint!("select port (0-{}): ", ports.len() - 1);
            let mut input_buf = String::new();
            std::io::stdin()
                .read_line(&mut input_buf)
                .context("failed to read port selection")?;
            input_buf
                .trim()
                .parse::<usize>()
                .map_err(|_| anyhow::anyhow!("invalid port number"))?
        }
    };

    let port_name = ports
        .get(port_index)
        .map(|p| p.name.as_str())
        .unwrap_or("?");
    eprintln!("using MIDI port {port_index}: {port_name}");

    let conn = connect_midi_output(port_index).map_err(|e| anyhow::anyhow!("{e}"))?;

    let bpm = compiled
        .events
        .iter()
        .find_map(|e| {
            if let MidiEvent::Tempo { bpm } = &e.event {
                Some(*bpm)
            } else {
                None
            }
        })
        .unwrap_or(120.0);

    let stream = CompiledStream::new(compiled.events, compiled.ppq, bpm);
    let hot_swap = Arc::new(HotSwapSlot::new());
    let scheduler = Scheduler::new(stream, conn, Arc::clone(&hot_swap), true, swap_mode);

    // Set up file watcher for hot-swap. Watch the parent directory, not the
    // file itself: editors that save atomically (write temp + rename) replace
    // the inode, which silently kills a direct file watch after the first save.
    let watch_path = input.canonicalize().unwrap_or_else(|_| input.clone());
    let watch_dir = watch_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let watch_file_name = watch_path.file_name().map(|n| n.to_os_string());
    let watch_hot_swap = Arc::clone(&hot_swap);
    let (tx, rx) = std::sync::mpsc::channel();

    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            let is_relevant = matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            ) && event
                .paths
                .iter()
                .any(|p| p.file_name().map(|n| n.to_os_string()) == watch_file_name);
            if is_relevant {
                let _ = tx.send(());
            }
        }
    })
    .context("failed to create file watcher")?;

    watcher
        .watch(&watch_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("failed to watch {}", watch_dir.display()))?;

    eprintln!(
        "playing {} (Ctrl+C to stop, file changes will hot-swap)",
        input.display()
    );
    scheduler.play();

    // File watch loop.
    let watch_filename = filename.clone();
    let _watch_thread = std::thread::spawn(move || {
        while rx.recv().is_ok() {
            // Debounce: drain any queued events (30ms is sufficient for
            // atomic file writes from all major editors).
            std::thread::sleep(Duration::from_millis(30));
            while rx.try_recv().is_ok() {}

            eprintln!("file changed, recompiling...");
            match fs::read_to_string(&watch_path) {
                Ok(new_source) => match compile_source(
                    &new_source,
                    &watch_filename,
                    cli_seed,
                    // Keep the session's resolved seed stable across hot-swap
                    // recompiles so unrelated edits don't reshuffle random
                    // transforms (an explicit @seed / --seed still wins).
                    Some(session_seed),
                ) {
                    Ok((new_compiled, _seed)) => {
                        let new_bpm = new_compiled
                            .events
                            .iter()
                            .find_map(|e| {
                                if let MidiEvent::Tempo { bpm } = &e.event {
                                    Some(*bpm)
                                } else {
                                    None
                                }
                            })
                            .unwrap_or(120.0);
                        let new_stream =
                            CompiledStream::new(new_compiled.events, new_compiled.ppq, new_bpm);
                        watch_hot_swap.stage(new_stream);
                        eprintln!("hot-swap staged, will apply at next bar boundary");
                    }
                    Err(e) => {
                        render_error(&e, &new_source, &watch_filename);
                        eprintln!("recompilation failed, continuing with current stream");
                    }
                },
                Err(e) => {
                    eprintln!("failed to read file: {e}");
                }
            }
        }
    });

    // Block until the user presses Enter or hits Ctrl+C. Both paths funnel
    // into one channel so we always shut the scheduler down cleanly (NoteOff
    // for all active notes) — the default SIGINT disposition would kill the
    // process without running any cleanup.
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let ctrlc_tx = stop_tx.clone();
    ctrlc::set_handler(move || {
        let _ = ctrlc_tx.send(());
    })
    .context("failed to install Ctrl+C handler")?;
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        let _ = stop_tx.send(());
    });

    eprintln!("press Enter or Ctrl+C to stop playback...");
    let _ = stop_rx.recv();

    eprintln!("shutting down...");
    scheduler.shutdown();
    Ok(())
}
