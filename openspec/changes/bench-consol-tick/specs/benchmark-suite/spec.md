# benchmark-suite delta — bench-consol-tick

## REMOVED Requirements

### Requirement: Canonical v1 baseline run

**Reason**: subset machinery retired (owner ruling 2026-08-31, main
commit `7db78627`): the canonical run is the FULL matrix (every active
scenario × every registered contender), one command, no `BENCH_SUBSET`.
Replaced by "Canonical full-matrix run" below, which carries forward
the removed requirement's surviving guarantees: memory gauges ON in
every measured cell and randomized measurement order (order seed
recorded in the record's `protocol`). No owner ruling retires either.

## MODIFIED Requirements

### Requirement: Zone contract

The `benchmarks/` directory SHALL contain exactly one README, the
`bench` facade, and the zones `contenders/`, `harness/`, `scenarios/`,
`runner/`, `records/`, `attic/` at level 1, with no loose spike
directories, reports, or results trees outside their zone. The
`contenders/` zone holds consolidated contender builds and their
shared runtime assets only (today: `rust-camel-lib/` single crate,
`node/` shared runtime dir); per-scenario data never lives there.

#### Scenario: Level-1 audit

- **GIVEN** the repository after consolidation
- **WHEN** listing `benchmarks/` at level 1
- **THEN** the listing contains only `README.md`, `bench`,
  `contenders/`, `harness/`, `scenarios/`, `runner/`, `records/`,
  `attic/` (and `.gitignore` if needed)
- **AND** no file matching `spike-*`, `results/`, or a historical
  report remains at level 1

#### Scenario: Harness moves without modification

- **GIVEN** the pre-change `harness` sources (`benchmarks/harness/run.sh`, loadgen
  crates) at their old paths
- **WHEN** the zone move completes
- **THEN** `git diff` of each moved source shows path changes only
- **AND** every golden-digest and harness test passes unmodified

#### Scenario: Contenders zone holds builds, not data

- **GIVEN** the consolidated contender builds
- **WHEN** auditing `benchmarks/contenders/`
- **THEN** it contains only build sources and shared runtime assets
  (crate sources, `node_modules`, entry scripts)
- **AND** no scenario payload, golden, or parity-hash asset lives
  there (those stay under `scenarios/<scn>/`)

## ADDED Requirements

### Requirement: Canonical full-matrix run

The system SHALL treat `bash benchmarks/bench run-all` (no env vars, no
flags) as the canonical run: EVERY active scenario × every registered
contender — the asymmetric **52-cell matrix** (five full scenarios × 8
contenders + two bridge scenarios × 6: the 4-artifact core + 2 node) —
cold + warm-where-applicable, with memory gauges ON in every measured
cell and randomized measurement order (order seed recorded in the
record's `protocol`), recorded as one run-level record. `--scenarios=`
remains a harness-level developer knob only. Scenario discovery is
automatic and ACTIVE-ONLY (registered scenarios; `spike-*` and
non-registered dirs like `multi-step` excluded — also from the
`meta.json` `scenarios` list).

#### Scenario: one command full coverage

- **Given** the digest-pinned runner image present and a host meeting
  the quiet-host criteria in `benchmarks/harness/CONTEXT.md`
- **When** `bash benchmarks/bench run-all` executes
- **Then** the run measures the full 52-cell matrix and writes raw
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
per-scenario route-builder modules) and the node contenders as one
runtime dir `benchmarks/contenders/node/` (shared `node_modules`,
per-scenario entry scripts). The consolidated lib crate MUST be a
root-workspace member (replacing the seven per-scenario crate entries;
non-default member, shared root `Cargo.lock` — Pair A/B dep-version
parity per `benchmarks/harness/CONTEXT.md` §2/§4) and MUST preserve the
fixture-local `target-dir` pin and `env -u CARGO_TARGET_DIR` build
semantics. Per-scenario DATA (shared payloads, goldens,
`BENCH_INPUT_SHA256` parity hashes) stays under `scenarios/<scn>/`.
Explicit exemptions, stated in `benchmarks/scenarios/COVERAGE.md` as requirements:
`rust-camel-cli` (already one build + YAML routes — the copied
pattern), `camel-standalone-*` (classpath-isolation fairness is
load-bearing, `benchmarks/harness/CONTEXT.md` §3), `camel-quarkus-*-native`
(AOT bakes the route; per-scenario artifacts ARE the measurement).
`benchmarks/scenarios/COVERAGE.md` MUST record these exemptions and
the consolidated locations.

No scenario-dispatch work executes before the marker: parse argv →
build ONLY the selected route → marker → (tick loop).

#### Scenario: single build, all scenarios

- **Given** the consolidated lib crate
- **When** cargo builds it once
- **Then** the resulting binary serves every active scenario via argv
  dispatch, and no per-scenario fixture crates remain under
  `scenarios/*/rust-camel-lib/`
- **And** `cargo metadata` resolves the crate as a root-workspace
  member sharing the root `Cargo.lock`

#### Scenario: smoke parity after the move

- **Given** all 52 expected cells after consolidation
- **When** the smoke harness runs each cell
- **Then** every cell emits its `SCENARIO_MARKER` exactly once and each
  scenario's `BENCH_INPUT_SHA256` digests are byte-identical to the
  pre-move values

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

### Requirement: Warm tick mode

The t2-json, split-aggregate and t2-realistic-eip fixtures in EVERY
runtime — camel-standalone-dsl, camel-standalone-yaml,
camel-quarkus-dsl-native, camel-quarkus-yaml-native, rust-camel-lib,
rust-camel-cli (including its wrapper latency-file plumbing),
node-native, node-fastify — SHALL run a tick loop AFTER the readiness
marker, writing `BENCH_LATENCY` records to the cell's latency file
under the existing Protocol B contract (10 ms timer period,
saturation escalates via won't-measure, not period adaptation). The
loop starts strictly after the `SCENARIO_MARKER`. When tick emission
lands, `t2-realistic-eip` is REMOVED from the Protocol B m2 skip list
— all 24 warm cells (3 scenarios × 8 runtimes) must measure.

#### Scenario: protocol B records exist for tick scenarios

- **Given** a post-change run including m2
- **When** a t2-json / split-aggregate / t2-realistic-eip cell runs
- **Then** `parse-protocol-b` parses n>0 latency records for that cell
  (no more "not-measured: pending fixture emission"; no m2 skip for
  `t2-realistic-eip`)

#### Scenario: marker timing unperturbed, quantitative gate

- **Given** the same fixtures before and after the tick change
- **When** M1 measures time-to-marker with n≥30 samples per cell
- **Then** every cell's post-change median is within max(±15%, ±3 ms)
  of its pre-change median; ANY cell outside tolerance blocks the
  phase exit

#### Scenario: tick parity across runtimes

- **Given** the three tick scenarios
- **When** their cells register
- **Then** every runtime of the scenario ticks, including
  rust-camel-cli via its wrapper latency-file plumbing (a scenario
  whose fixtures tick asymmetrically across runtimes is a hard error,
  not a partial warm matrix)
