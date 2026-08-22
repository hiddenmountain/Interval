# Contributing to Interval

Thanks for helping out. This is a small, focused codebase — the rules below
exist to keep it that way.

## Building and testing

```bash
cargo build                                # build all crates
cargo test                                 # run everything, including golden tests
cargo clippy -- -D clippy::all             # must be clean
cargo fmt --all                            # format before committing
```

CI builds with `-D warnings`, so any warning is a build failure. Run clippy
and `cargo fmt --all -- --check` locally before pushing.

## Golden tests

Most compiler behavior is pinned by golden-file tests in
`interval-core/tests/golden/` — one directory per case, each containing an
`input.interval` source and an `expected.json` of the compiled output. The
harness in `interval-core/tests/golden_tests.rs` auto-discovers every
directory and diffs actual vs. expected.

If you intentionally change compiler output:

```bash
cargo test --test golden_tests -- --ignored
```

This runs the `update_golden` test, which overwrites every `expected.json`.
Review the diff carefully before committing — a re-bless that changes files
you didn't mean to touch usually means you broke something.

There is also `interval-core/tests/examples_compile.rs`, which compiles every
example under `examples/` and asserts that everything under
`examples/expected-failures/` fails. New syntax should usually come with an
example, and new error conditions with a failure fixture.

## The WASM constraint

`interval-core` and `interval-smf` must compile to `wasm32-unknown-unknown`:

```bash
cargo build --target wasm32-unknown-unknown -p interval-core
cargo build --target wasm32-unknown-unknown -p interval-smf
```

That means **no** `std::fs`, `std::time`, `std::thread`, or
`println!`/`eprintln!` in those crates, and no dependencies that pull in
native system libraries. All error reporting is by return value; the caller
(CLI, RT, WASM wrapper) does the I/O. This also covers randomness: seeds are
resolved in `interval-cli`/`interval-rt` and passed in fully resolved —
the core never touches an OS random source or clock.

## Crate dependency rules

- `interval-smf` and `interval-rt` must never depend on each other.
- `interval-rt` is native-only (`midir`); nothing WASM-facing may depend on it.
- Don't add `rand` — deterministic seeded output uses a hand-rolled xorshift64
  and FNV-1a precisely so results stay stable across dependency bumps.
- Don't add `nom`/`pest` — the parser is deliberately hand-written recursive
  descent for the sake of error messages.

See `ARCHITECTURE.md` for the reasoning behind these boundaries.

## Scope

The language spec in `docs/spec/` is the source of truth for behavior.
Features listed as deferred/reserved there (and in `docs/design.md`) are
intentionally not implemented — please open an
issue to discuss before starting work on new language surface.
