# Scenarios

A scenario is a workload (a route under measurement). A contender is
a system under test. One scenario × one contender = one cell, the
unit every run measures. The standard set is six contenders per
scenario: `rust-camel-lib`, `rust-camel-cli`,
`camel-standalone-dsl`, `camel-standalone-yaml`,
`camel-quarkus-dsl-native`, `camel-quarkus-yaml-native`.

- Coverage index: `COVERAGE.md` (which cells exist, which are
  measured).
- Run protocol and vocabulary: `../harness/CONTEXT.md`.
- Template scenario for everything below: `t2-json/` (newest,
  six-artifact, canonical payload).

## Adding a new scenario

Template: copy the SHAPE of `t2-json/` (never its content blindly).

1. **Fixtures** — create `scenarios/<name>/` with one directory per
   contender:
   - `rust-camel-lib/` — `Cargo.toml` + `src/` (fixture drives the
     lib directly).
   - `rust-camel-cli/` — `routes/*.yaml` (drives the CLI).
   - `camel-standalone/{dsl,yaml}/` — Maven projects.
   - `camel-quarkus/{dsl,yaml}(-native)/` — Gradle projects.
   - `README.md` — the route, the marker contract, and the input
     digest goldens.
2. **Marker contract** — the route prints ONE line on completion
   (e.g. `BENCH_ROUTE_READY items=100`). Register it in
   `harness/run.sh`:
   `SCENARIO_MARKER["<name>"]="BENCH_ROUTE_READY …"`.
3. **Cells** — in `harness/run.sh`, one `add_cell` per contender
   (see the t2-json block around `add_cell "$scenario"` calls), plus
   the scenario's round-protocol mapping (`["<name>"]="A"` or
   `"B"`).
4. **Canonical payload** — if the scenario consumes a deterministic
   input: add a builder in `harness/loadgen/src/payload.rs`, pin its
   sha256 with a golden unit test, and register the scenario in
   `scenario_payload_digest` (this is what fills `input_sha256` in
   records). If the scenario consumes no payload body: do nothing —
   the record carries a documented `null`.
5. **Workspace member** — add the rust fixture to the root
   `Cargo.toml` members: `benchmarks/scenarios/<name>/rust-camel-lib`.
6. **Smoke** — first `bash benchmarks/bench run --scenarios=<name>
   --dry-run` green (no JDK needed). Then the real smoke: every
   contender must produce the SAME input digest, byte-identical
   across all six.
7. **Paperwork** — a row in `COVERAGE.md` with status
   `? open-if: first published container-hosted run` (never claim a
   measurement that no run produced).

## Adding a new contender to an existing scenario

Much cheaper — the scenario already exists.

1. **Fixture** — `scenarios/<name>/<contender>/` following that
   scenario's existing fixtures (route semantics identical).
2. **Cell** — one `add_cell` line in `harness/run.sh` for the
   scenario (plus a builder only if the contender is a whole new
   runtime).
3. **Smoke** — its input digest must be byte-identical to the other
   contenders' in the same scenario.
4. **Paperwork** — update the scenario's row/column in
   `COVERAGE.md` with `open-if` until a published run measures it.

## Golden rules

- Every contender in a scenario processes the SAME canonical input;
  the record's `input_sha256` is the proof. A diverging digest is a
  broken fixture, not a faster contender.
- No human ever types a number into a report — everything under
  `records/` is generated and checksum-guarded.
- Cells are only comparable within one run; never mix runs in a
  ratio.
- Gauges stay ON in every measured cell (see ADR-0066).
