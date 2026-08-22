# Examples

Curated Interval v0.9 example programs and failure fixtures.

Runnable examples live in one directory per piece and are intended to pass
`interval check` and `interval compile`.

Known-piece showcase examples:

- `bach-prelude-c-major/`
- `fur-elise-opening/`
- `fur-elise-full-study/`
- `pachelbel-canon/`
- `when-the-saints/`

Expected failures live under `expected-failures/` and are intentionally invalid
regression cases for current syntax and semantics.

Useful commands:

```sh
find examples -name '*.interval' ! -path '*/expected-failures/*'
interval check examples/<dir>/<file>.interval
interval compile examples/<dir>/<file>.interval
interval check examples/expected-failures/<file>.invalid.interval
```

Every example here is also compiled by the test suite
(`cargo test --test examples_compile -p interval-core`).
