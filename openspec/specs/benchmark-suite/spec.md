# benchmark-suite Specification

## Purpose
TBD - created by archiving change bench-missing-cells. Update Purpose after archive.
## Requirements
### Requirement: Payload-size axis

The benchmark harness SHALL support selecting a transport-body size class
(1024, 32768, 262144, or 1048576 bytes — other values rejected) for
load-driven measurements (Protocol A and M3 throughput), and timer-driven
fixtures (Protocol B) SHALL honor a `BENCH_PAYLOAD_BYTES` environment
variable constructing a byte-identical canonical body in every artifact.
Fixture-count references below mean the completeness-declaring fixture
set of the scenario (registered contenders; eight with the node family).

#### Scenario: Transport body builder exactness

- **GIVEN** the loadgen body builder unit test
- **WHEN** bodies are built for 1024, 32768, 262144, 1048576 bytes
- **THEN** each body's length is exactly the requested size and its
  SHA-256 matches the recorded golden digest for that size

#### Scenario: Invalid size rejected

- **GIVEN** `measure-throughput --payload-size 2048`
- **WHEN** the CLI parses arguments
- **THEN** it exits with a usage error naming the four valid sizes

#### Scenario: Protocol-B fixture canonical body and size assert

- **GIVEN** a t2-json fixture started with `BENCH_PAYLOAD_BYTES=32768`
  and tick 0
- **WHEN** the route builds its body
- **THEN** the fixture logs `BENCH_INPUT_SHA256=<golden hex for
  (32768, tick 0)>` and asserts the body length equals 32768 before
  processing and the exact output length equals 32768 + 13 bytes after
  marshaling, emitting NO marker (failing the cell) on any mismatch

#### Scenario: Artifacts byte-equivalent

- **GIVEN** the same `BENCH_PAYLOAD_BYTES` and tick passed to the
  completeness-declaring fixture set of a scenario
- **WHEN** each fixture builds its body
- **THEN** every fixture produces byte-identical canonical inputs (golden
  digest equality), so rankings measure the framework, not payload skew

### Requirement: t2-json scenario

The suite SHALL provide a `t2-json` scenario exercising
unmarshal("json") → jsonpath validate → transform (append field) →
marshal("json") with the scenario's registered contender set (six
artifact fixtures, eight cells with the node family), registered in the
harness marker and protocol maps, marker-emitting exactly once per suite
contract.

#### Scenario: rust-lib fixture passes marker contract

- **GIVEN** the harness runs the `t2-json` rust-camel-lib cell
- **WHEN** the route completes one cycle
- **THEN** stdout contains exactly one `BENCH_ROUTE_READY` marker, the
  marshaled output has the exact asserted length (input_size + 13
  bytes), and parsed semantic equality holds (id="bench", original seq,
  fill, appended `"bench": true` present)

#### Scenario: Cross-runtime input equivalence

- **GIVEN** the completeness-declaring fixture set (six artifact
  fixtures, eight cells with the node family) runs with the same payload
  class and tick
- **WHEN** each logs its input digest
- **THEN** all report the same `BENCH_INPUT_SHA256` value for that
  (size, tick); output bytes may differ in field order (documented
  caveat), inputs never do

### Requirement: split-aggregate scenario

The suite SHALL provide a `split-aggregate` scenario exercising the
one-to-many EIP surface via two routes joined through `direct:` — an
outer route that splits a fixed-count canonical JSON array (N=100 items)
into correlated fragments sent to an aggregate route with
`completion_size=100` and `force_completion_on_stop=false` — where the
marker fires only on true bucket completion asserting the aggregated
item count.

#### Scenario: Aggregate completes with exactly N items

- **GIVEN** the outer route splits an array of 100 items into fragments
  forwarded to `direct:agg-in`
- **WHEN** the aggregate bucket reaches completion_size=100
- **THEN** the completion path asserts the aggregated collection holds
  exactly 100 items and emits `BENCH_ROUTE_READY items=100` exactly once

#### Scenario: Incomplete bucket emits no marker

- **GIVEN** a split cycle where fragments never reach 100 (simulated in
  a fixture unit test by stopping at 99)
- **WHEN** the process is asked to complete
- **THEN** no marker is emitted (force_completion_on_stop=false) and the
  cell fails by marker deadline

#### Scenario: Harness registration

- **GIVEN** `run.sh --scenarios=split-aggregate`
- **WHEN** cells resolve
- **THEN** the scenario is recognized (marker + Protocol B in the harness
  maps) and does not fail scenario resolution as unknown

### Requirement: Ratio confidence intervals

The harness SHALL compute paired bootstrap confidence intervals for M3
throughput ratios between two cells from the same run (identical
measurement-order provenance, equal round count, round indices 0..n−1),
reusing the existing BCa/PRNG machinery, exposed as an `aggregate-ratios`
loadgen subcommand that hard-errors on any validation mismatch.

#### Scenario: Ratio CI on published data

- **GIVEN** two `m3-summary.json` files from the same published run with
  5 per-round means each
- **WHEN** `bench-loadgen aggregate-ratios <cellA> <cellB>` runs
- **THEN** output states `RATIO <A>/<B> point=<median-ratio>
  lo=<lower> hi=<upper>` derived from jointly resampled round indices

#### Scenario: Deterministic across invocations

- **GIVEN** the same inputs and seed
- **WHEN** the subcommand runs twice
- **THEN** the output is identical

#### Scenario: Unrelated or malformed summaries rejected

- **GIVEN** inputs failing any validation: summaries from DIFFERENT runs
  (mismatched provenance identity), a non-M3 summary (e.g. m2), a summary
  with missing provenance, malformed `per_round_means` (empty or
  non-numeric), or duplicate/missing/noncontiguous round indices
- **WHEN** `aggregate-ratios` validates its inputs
- **THEN** it exits nonzero with an error naming the specific mismatch
  (metric, provenance, round count, round indices, or means format)

### Requirement: Metric-family overhead measurement

The suite SHALL measure the T3 http-server throughput cost of the metric
families as a lever study on the rust-cli artifact: both arms register
the same Prometheus backend; arm A enables exchange, duration, and
components families; arm B sets only the master `enabled=false`; 5
rounds × 30 s per arm; the published result is the throughput ratio with
its bootstrap confidence interval.

#### Scenario: A/B produces a bounded ratio

- **GIVEN** T3 M3 runs (5 rounds × 30 s) for the rust-cli fixture with
  arm-A and arm-B Camel.tomls, both exporting Prometheus on the same
  port across separate runs
- **WHEN** `aggregate-ratios` compares arm A to arm B
- **THEN** the report states point ratio with lo/hi bounds, labeled
  "lever study", not a contender row

### Requirement: CI bench subset

CI SHALL run a fast criterion subset (`bench-smoke` job: camel-bench
`pipeline` and `body_coercion` benches in quick mode) on ubuntu with
`timeout-minutes: 10`, and SHALL smoke the restructured suite
entrypoint by invoking `bench run --dry-run` (no JDK required) in
the same job, keeping the bench entrypoints green without the
container matrix.

#### Scenario: bench-smoke job

- **GIVEN** a PR touching bench code or CI
- **WHEN** the `bench-smoke` job runs
- **THEN** both criterion benches execute in quick mode and
  `bench run --scenarios=t2-json,split-aggregate --dry-run` exits 0
  through the restructured paths
- **AND** the job completes within its 10-minute timeout without
  container services

### Requirement: Zone contract

The `benchmarks/` directory SHALL contain exactly one README, the
`bench` facade, and the zones `harness/`, `scenarios/`, `runner/`,
`records/`, `attic/` at level 1, with no loose spike directories,
reports, or results trees outside their zone.

#### Scenario: Level-1 audit

- **GIVEN** the repository after restructure
- **WHEN** listing `benchmarks/` at level 1
- **THEN** the listing contains only `README.md`, `bench`,
  `harness/`, `scenarios/`, `runner/`, `records/`, `attic/` (and
  `.gitignore` if needed)
- **AND** no file matching `spike-*`, `results/`, or a historical
  report remains at level 1

#### Scenario: Harness moves without modification

- **GIVEN** the pre-change `harness` sources (run.sh, loadgen
  crates) at their old paths
- **WHEN** the zone move completes
- **THEN** `git diff` of each moved source shows path changes only
- **AND** every golden-digest and harness test passes unmodified

### Requirement: Single facade

The system SHALL expose one `bench` entrypoint whose `run`
subcommand passes through to the existing `harness/run.sh` with
identical semantics (same flags, same env vars), plus `summarize`
and `publish` subcommands delegating to the records layer.

#### Scenario: Run passthrough parity

- **GIVEN** any run.sh invocation used before the change (e.g.
  `--scenarios=http-server --metric=m3 --rounds=5`)
- **WHEN** the same arguments are given to `bench run`
- **THEN** run.sh receives the arguments and environment unchanged
- **AND** the marker contract and result digests are byte-identical
  to a direct run.sh invocation

#### Scenario: Dry run via facade

- **GIVEN** the t2-json and split-aggregate fixtures registered
- **WHEN** `bench run --scenarios=t2-json,split-aggregate --dry-run`
  executes without a JDK
- **THEN** the dry-run gates pass and no JVM toolchain is required

### Requirement: Canonical v1 baseline run

The system SHALL produce a first canonical container-hosted run
(`records/` v1 baseline) over the validated subset
(startup-minimal, http-server, t2-json, split-aggregate, plus the
payload axis on two reference contenders), with memory gauges ON
and randomized measurement order, recorded as a run-level record.

#### Scenario: v1 record lands

- **GIVEN** the digest-pinned runner image and a host meeting the
  quiet-host criteria defined in `harness/CONTEXT.md` (devnull
  baseline within its stability bound; host load below the
  recorded ceiling)
- **WHEN** the v1 subset run completes
- **THEN** `records/` contains a run dir with `run.json`
  (`schema_version: 1`), `summary.md`, and per-cell JSON
- **AND** `run.json` records `container_digest` (never `:latest`),
  `git_commit`, and the order seed
- **AND** every cell carries its canonical `input_sha256`

#### Scenario: Gauges stay on

- **GIVEN** the gauge A/B verdict (cost unresolved from zero)
- **WHEN** the v1 run is configured
- **THEN** memory gauges are enabled in every measured cell

#### Scenario: Human-invoked execution

- **GIVEN** the agent has prepared and validated the runner image,
  fixtures, quiet-host criteria, and run configuration (dry-run
  gates green)
- **WHEN** the v1 run is to execute (hours-long, quiet-host
  predicate that an agent cannot guarantee)
- **THEN** a human invokes the run command and the agent's
  deliverable ends at preparation and post-run record validation

### Requirement: Era-1 freeze

The system SHALL preserve era-1 evidence instead of deleting it:
v2/v3/v4/addendum/consultation reports under
`docs/benchmarks/history/`, historical results trees under
`benchmarks/attic/results-era-1/`, and a git tag
`bench/era-1-final` freezing the last commit where the reports
lived at `docs/benchmarks/`. The gauge A/B verdict that justifies
gauges-ON SHALL be cited in ADR-0066 before the addendum moves.

#### Scenario: Reports reachable after freeze

- **GIVEN** the era-1 reports moved to `docs/benchmarks/history/`
- **WHEN** a reader checks out tag `bench/era-1-final`
- **THEN** the reports are present at `docs/benchmarks/` with
  byte-identical content
- **AND** `docs/benchmarks/` on the restructured main contains no
  stale era-1 comparative claims outside `history/`
- **AND** no link in `scenarios/COVERAGE.md` points to a moved
  report at its pre-move path

#### Scenario: Gauge premise preserved

- **GIVEN** the v4 addendum moved to `docs/benchmarks/history/`
- **WHEN** a reader traces the gauges-ON decision
- **THEN** ADR-0066 cites the gauge A/B verdict (point 0.9890,
  CI [0.9785, 1.0126], interval includes 1.0)

### Requirement: Public terminology confinement

Owner-facing benchmark prose SHALL use only the public vocabulary
(corrida, escenario, contendiente, fecha, registro), and technical
vocabulary (M1-M4, T-families, pairing, seeds) SHALL be confined
to `harness/CONTEXT.md` and technical artifacts.

#### Scenario: README diet

- **GIVEN** the restructured `benchmarks/README.md`
- **WHEN** scanned for technical terms (M1-M4, T2j, T-family,
  paired, bootstrap)
- **THEN** none appear outside quoted references to
  `harness/CONTEXT.md`

### Requirement: Contender completeness

The suite MUST enforce contender completeness for completeness-declaring
families: a family that declares completeness SHALL implement every
ACTIVE scenario selected for the run. Enforcement is selection-scoped —
a family with at least one fixture among the run's selected active
scenarios that does not cover ALL selected active scenarios is a hard
error listing the family and each missing scenario. Families that do not
declare completeness are outside the rule (today: the YAML artifact-set
pair excluded from bridge scenarios by the documented scenario-side
`SCENARIO_ARTIFACT_SET` reduction). The node family declares
completeness across all 7 active scenarios. Inactive scenarios (no
harness wiring, no contender set — currently `multi-step`) are outside
the rule until activated; a fixture present for an inactive scenario
triggers a warning, not an error.

#### Scenario: contender family registered with a missing fixture

- **Given** the node contender family (completeness-declaring) and a run
  selecting all 7 active scenarios
- **When** the harness wires cells and `node-fastify` lacks a fixture
  directory for `split-aggregate`
- **Then** the run aborts before any measurement with an error listing
  `node-fastify` and every scenario it is missing (here
  `split-aggregate`)

#### Scenario: fixture for an inactive scenario

- **Given** a fixture directory `scenarios/multi-step/node-native/`
- **When** the harness wires cells
- **Then** the run emits a warning naming `multi-step` as inactive and
  continues without registering a cell for it

### Requirement: Node contender family

The suite SHALL measure two Node.js contenders, `node-native` (no web
framework; external libraries only where the Node stdlib has no
equivalent capability, i.e. the XML scenarios) and `node-fastify` (same
route logic behind Fastify), across all active scenarios. Both honor the
scenario's existing contract: shared canonical payload, marker line,
latency file, and per-scenario protocol identical to the JVM contenders
in the same scenario. In the six protocol-B scenarios `node-fastify`
boots the Fastify application WITHOUT binding a listener — the module
and init cost is the measured framework tax; only `http-server` binds a
listener. Node XML fixtures execute their libraries in-process
(`saxon-js`, `xmllint-wasm`), reuse only the scenario's shared
`bench-payload`/`.xsl`/`.xsd` assets (digest parity by construction),
and are exempt from the compiled-bridge subprocess/wrapper/PID
contract.

#### Scenario: cell registration

- **Given** a host with the pinned Node runtime resolved
- **When** the harness wires a scenario that has `node-native` and
  `node-fastify` fixtures
- **Then** two cells register with launch commands invoking `node` on the
  fixture entry scripts, the scenario's marker contract, and the
  scenario's protocol mapping

#### Scenario: digest parity

- **Given** the smoke harness for a scenario with a canonical payload
- **When** the node contenders' smoke runs
- **Then** their input digests equal the digests the existing contenders
  produced for the same scenario (byte-identical canonical input)

#### Scenario: dry-run without Node on the host

- **Given** a host without `node` on PATH and `--dry-run`
- **When** the node cells resolve
- **Then** fixtures without `package.json` resolve from their committed
  scripts (no build) and fixtures with `package.json` report a
  would-build marker without failing the dry-run

### Requirement: Pinned Node runtime

The runner image SHALL install Node from the official `nodejs.org/dist`
tarball with its SHA256 pinned by `pin.sh` alongside the existing runner-image
digest record; the runner refuses mutable tags and unpinned versions.

#### Scenario: pin verification

- **Given** a runner image build with `NODE_VERSION` and `NODE_SHA256`
  recorded by `pin.sh`
- **When** the build downloads the tarball
- **Then** the build verifies the SHA256 before install and fails closed
  on mismatch

#### Scenario: XML engine auditability

- **Given** the `xsd-validation-bridge` and `xslt-bridge` node fixtures
  (which require external libraries because Node stdlib has no XML)
- **When** a reviewer reads the fixture
- **Then** the fixture README names the library and engine in use
  (`saxon-js`, `xmllint-wasm`) and the counterpart engine on the JVM
  side, so the cross-engine comparison is auditable

