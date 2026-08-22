//! Compiles every example program under `examples/` at the workspace root.
//!
//! Every `*.interval` file (outside `expected-failures/`) must parse and
//! compile cleanly. Every `*.invalid.interval` file under
//! `examples/expected-failures/` must fail to compile.
//!
//! Run with `cargo test --test examples_compile -p interval-core`.

use std::fs;
use std::path::{Path, PathBuf};

/// Example files that are known not to compile against the current compiler.
///
/// FIXME(pre-release): fix these examples (they were hand-tested against
/// older language versions) and empty this list. Entries are file names,
/// not paths.
const SKIP: &[&str] = &[];

/// Parse and compile one source string, mimicking the CLI pipeline.
/// Seed resolution normally happens in `interval-cli`; tests pin it to 1.
fn compile_source(source: &str) -> Result<(), String> {
    let mut program = interval_core::parse_only(source).map_err(|e| e.to_string())?;
    program.header.resolved_seed = Some(1);
    interval_core::compiler::compile(&program.header, &program.blocks)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn examples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples")
}

/// Recursively collect `*.interval` files, skipping `expected-failures/`.
fn collect_examples(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        fs::read_dir(dir).unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display())); // safe: test code
    for entry in entries {
        let path = entry.expect("dir entry").path(); // safe: test code
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "expected-failures") {
                continue;
            }
            collect_examples(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "interval") {
            out.push(path);
        }
    }
}

#[test]
fn all_examples_compile() {
    let root = examples_root();
    assert!(
        root.is_dir(),
        "examples directory not found at {}",
        root.display()
    );

    let mut files = Vec::new();
    collect_examples(&root, &mut files);
    files.sort();
    assert!(
        !files.is_empty(),
        "no .interval files found under {} — the examples corpus is missing",
        root.display()
    );

    let mut failures = Vec::new();
    for path in &files {
        let name = path
            .file_name()
            .unwrap_or_default() // safe: test code
            .to_string_lossy()
            .into_owned();
        let source = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display())); // safe: test code

        match compile_source(&source) {
            Ok(()) => {
                assert!(
                    !SKIP.contains(&name.as_str()),
                    "{name} is on the skip list but compiles — remove it from SKIP"
                );
            }
            Err(err) => {
                if SKIP.contains(&name.as_str()) {
                    eprintln!("SKIP (known-broken) {name}: {err}");
                } else {
                    failures.push(format!("{}: {err}", path.display()));
                }
            }
        }
    }

    if !failures.is_empty() {
        panic!(
            "{} example(s) failed to compile:\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }
}

#[test]
fn expected_failures_fail_to_compile() {
    let dir = examples_root().join("expected-failures");
    assert!(
        dir.is_dir(),
        "expected-failures directory not found at {}",
        dir.display()
    );

    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", dir.display())) // safe: test code
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let name = path.file_name()?.to_string_lossy().into_owned();
            (path.is_file() && name.ends_with(".invalid.interval")).then_some(path)
        })
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no .invalid.interval files found under {}",
        dir.display()
    );

    for path in &files {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display())); // safe: test code
        assert!(
            compile_source(&source).is_err(),
            "{} compiled successfully but is expected to fail",
            path.display()
        );
    }
}
