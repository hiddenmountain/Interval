# Expected Failure Corpus

These files are intentionally invalid Interval v0.9 inputs kept as parser
and compiler regression cases.

Suggested manual checks:

```sh
interval check examples/expected-failures/invalid-old-pipe.invalid.interval
interval check examples/expected-failures/invalid-multiple-unnamed-harmony.invalid.interval
interval check examples/expected-failures/invalid-chord-ordinal-no-follow.invalid.interval
```

The test suite asserts that every `*.invalid.interval` file here fails to
compile (`cargo test --test examples_compile -p interval-core`).
