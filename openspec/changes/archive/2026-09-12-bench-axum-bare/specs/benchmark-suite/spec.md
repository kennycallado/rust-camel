## ADDED Requirements

### Requirement: axum-bare reference contender

The suite SHALL measure an `axum-bare` reference contender — a minimal
axum+hyper+tokio HTTP server with NO camel dependency — as a registered
`http-server`-only roster cell, so the m3/m2 tables situate a stack-tax
reference between the devnull ceiling and `rust-camel-lib` (camel tax vs
HTTP-stack tax becomes a recorded cell instead of a one-off profile, bd
rc-u034). Adding the contender MUST NOT alter any published record, devnull
ceiling semantics, Pair A/B membership, or warm-gate arithmetic; its
measurement happens through the generic per-cell machinery at the next
canonical run.

#### Scenario: marker after bind, flushed

- **GIVEN** the axum-bare fixture launched by the harness (stdout piped)
- **WHEN** its listener binds on 0.0.0.0:8080 (or `BENCH_AXUM_BARE_PORT`)
- **THEN** stdout receives exactly one bare `BENCH_ROUTE_READY` line with an
  explicit stdout flush, emitted before the serve loop blocks
- **AND** the harness drives the loadgen only after observing the marker

#### Scenario: T3 route contract with observable drain

- **GIVEN** the fixture serving and a client holding one keep-alive
  connection, sending two sequential POSTs to `/bench`, each carrying the
  canonical T3 payload (~32 KiB) issued as several separate client writes
- **WHEN** both requests complete
- **THEN** each response is `200` `text/plain; charset=utf-8` with body
  `pong`
- **AND** the second request succeeds on the SAME connection — successful
  keep-alive reuse is the mechanical proof the first request's body (and
  EOS) was fully consumed before its response completed
- **AND** stdout carries `BENCH_HTTP_REQUEST received` and
  `BENCH_HTTP_REQUEST id=1` then `id=2` (smoke contract, rc-am22)

#### Scenario: roster registration is http-server-only

- **GIVEN** the harness roster constants after this change
- **WHEN** `expected_roster` derives identities for all 7 active scenarios
- **THEN** `http-server/axum-bare` is expected (53 identities total:
  5 full scenarios × 8 + 2 bridge × 6 + 1 reference)
- **AND** axum-bare appears in NO other scenario's roster and in NEITHER
  Pair A nor Pair B

#### Scenario: roster drift guard covers the reference tuple

- **GIVEN** run.sh declares `REFERENCE_CONTENDERS` and summarize.py declares
  the equivalent mapping
- **WHEN** the roster-mirror drift test greps both sources
- **THEN** they are equal in both directions (run.sh projection ↔ python
  mapping), and the drift test fails if either gains or loses an entry

#### Scenario: no-camel isolation with lock parity

- **GIVEN** the fixture crate `benchmarks/contenders/axum-bare`
- **WHEN** its dependency graph resolves
- **THEN** no `camel-*` crate appears in it; it is a root-workspace
  non-default member sharing the root `Cargo.lock` whose only lock change
  is the fixture's own package entry — every third-party version stays
  pinned at the versions the lock already carries

#### Scenario: smoke case with committed evidence

- **GIVEN** the http-server smoke harness extended with the axum-bare case
  (runnable standalone via an artifact filter, launched on a free port via
  the env override)
- **WHEN** the smoke runs
- **THEN** it asserts the marker, `200`/`pong`, and `id=1`, and commits the
  resulting `axum-bare.log` containing no timing-like numbers

#### Scenario: published records stay byte-identical

- **GIVEN** records published before this change (52-cell persisted rosters)
- **WHEN** `summarize.py --check` regenerates their summaries after this
  change lands
- **THEN** every regenerated summary.md is byte-identical to the committed
  one (completeness validates against each record's persisted
  `expected_cells`, never current harness constants)

## MODIFIED Requirements

### Requirement: Canonical full-matrix run

The system SHALL treat `bash benchmarks/bench run-all` (no env vars, no
flags) as the canonical run: EVERY active scenario × every registered
contender — the asymmetric **53-cell matrix** (five full scenarios × 8
contenders + two bridge scenarios × 6: the 4-artifact core + 2 node, plus
the `axum-bare` reference contender for `http-server`) — cold +
warm-where-applicable, with memory gauges ON in every measured cell and
randomized measurement order (order seed recorded in the record's
`protocol`), recorded as one run-level record. `--scenarios=` remains a
harness-level developer knob only. Scenario discovery is automatic and
ACTIVE-ONLY (registered scenarios; `spike-*` and non-registered dirs like
`multi-step` excluded — also from the `meta.json` `scenarios` list).

#### Scenario: one command full coverage

- **Given** the digest-pinned runner image present and a host meeting
  the quiet-host criteria in `benchmarks/harness/CONTEXT.md`
- **When** `bash benchmarks/bench run-all` executes
- **Then** the run measures the full 53-cell matrix and writes raw
  artifacts plus a launch-time `meta.json` carrying `run_id` (launch
  timestamp) and `scenarios` (the 7 active scenario names, and nothing
  else)

#### Scenario: no subset escape hatch on the owner surface

- **Given** `run-all.sh`
- **When** invoked with any `BENCH_SUBSET` value
- **Then** the variable is ignored (no subset concept exists); the run
  covers the full matrix

#### Scenario: gauges and order survive the subset retirement

- **Given** the canonical run configuration
- **When** the run executes
- **Then** memory gauges are enabled in every measured cell
- **And** measurement order is randomized with the seed recorded in
  the record's `protocol.order_seed`

#### Scenario: human-invoked execution

- **Given** the agent has prepared and validated the runner image,
  fixtures, quiet-host criteria, and run configuration
- **When** the run is to execute (hours-long, quiet-host predicate)
- **Then** a human invokes the one command; the agent's deliverable
  ends at preparation and post-run record validation

### Requirement: Consolidated contender builds

The suite SHALL build one parametrized artifact per contender family
where the runtime allows it: `rust-camel-lib` as a single crate at
`benchmarks/contenders/rust-camel-lib/` (argv scenario dispatch to
per-scenario route-builder modules), the node contenders as one
runtime dir `benchmarks/contenders/node/` (shared `node_modules`,
per-scenario entry scripts), and the `axum-bare` reference contender
as its own crate at `benchmarks/contenders/axum-bare/`. The
consolidated lib crate MUST be a root-workspace member (replacing the
seven per-scenario crate entries; non-default member, shared root
`Cargo.lock` — Pair A/B dep-version parity per
`benchmarks/harness/CONTEXT.md` §2/§4) and MUST preserve the
fixture-local `target-dir` pin and `env -u CARGO_TARGET_DIR` build
semantics; the axum-bare crate follows the same membership semantics
(non-default member, shared root `Cargo.lock`, fixture-local
`target-dir` pin). Per-scenario DATA (shared payloads, goldens,
`BENCH_INPUT_SHA256` parity hashes) stays under `scenarios/<scn>/`.
Explicit exemptions, stated in `benchmarks/scenarios/COVERAGE.md` as
requirements: `rust-camel-cli` (already one build + YAML routes — the
copied pattern), `camel-standalone-*` (classpath-isolation fairness is
load-bearing, `benchmarks/harness/CONTEXT.md` §3),
`camel-quarkus-*-native` (AOT bakes the route; per-scenario artifacts
ARE the measurement). `benchmarks/scenarios/COVERAGE.md` MUST record
these exemptions and the consolidated locations.

No scenario-dispatch work executes before the marker: parse argv →
build ONLY the selected route → marker → (tick loop). The axum-bare
fixture has no scenario dispatch (single-purpose binary): bind →
marker flushed → serve.

#### Scenario: single build, all scenarios

- **Given** the consolidated lib crate
- **When** cargo builds it once
- **Then** the resulting binary serves every active scenario via argv
  dispatch, and no per-scenario fixture crates remain under
  `scenarios/*/rust-camel-lib/`
- **And** `cargo metadata` resolves the crate as a root-workspace
  member sharing the root `Cargo.lock`

#### Scenario: reference contender builds standalone

- **Given** the axum-bare fixture crate
- **When** `builder/build-all.sh` runs (or the crate is built alone
  with `env -u CARGO_TARGET_DIR cargo build --release -p
  axum-bare-fixture` from its directory)
- **Then** the release binary lands at the fixture-local
  `target/release/axum-bare-fixture` path the harness resolves, and the
  build's only `Cargo.lock` change (if any, on first workspace resolve)
  is the `axum-bare-fixture` package entry — no third-party version
  changes

#### Scenario: smoke parity after the move

- **Given** all 53 expected cells after consolidation (including the
  reference cell)
- **When** the smoke harness runs each cell
- **Then** every cell emits its `SCENARIO_MARKER` exactly once
- **And** every pre-existing digest-emitting cell preserves its
  pre-consolidation `BENCH_INPUT_SHA256`; `axum-bare` has no pre-move
  digest baseline and is validated by its marker and T3 route-contract
  scenarios instead

#### Scenario: dispatch does not perturb M1

- **Given** the consolidated lib fixture launched for one scenario
- **When** it starts
- **Then** only the selected route builder executes before the marker;
  non-selected route builders never run (lazy — RSS stays honest)

#### Scenario: shared node runtime

- **Given** the node contenders' runtime dir
- **When** fixtures resolve
- **Then** `node-fastify` resolves ONE shared `node_modules` tree
  (fastify installed once) and per-scenario entry scripts run against
  it; node-native scripts run dependency-free except the XML scenarios

#### Scenario: completeness guard survives the layout change

- **Given** the completeness guard and expected-cell count after the
  node fixtures move out of `scenarios/<scn>/`
- **When** a completeness-declaring family registers only a subset of
  the selected active scenarios
- **Then** the run aborts with a hard error naming the family and each
  missing scenario (RED-proof: unregistering one node fixture aborts)
- **And** the guard's evidence is registered cells (or an explicit
  family registration map), never `scenarios/<scn>/<member>` directory
  presence
- **And** a fixture present for an inactive scenario triggers a
  warning, not an error (behavior unchanged)
