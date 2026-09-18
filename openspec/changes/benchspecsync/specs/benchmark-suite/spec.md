## MODIFIED Requirements

### Requirement: Zone contract

The `benchmarks/` directory SHALL contain exactly one README, the
`bench` facade, the zones `attic/`, `audits/`, `contenders/`,
`harness/`, `records/`, `runner/`, `scenarios/`, and the pinned
level-1 investigation doc `docs-investigation-strategy.md` (the
live-defect link for the warmup policy, referenced from
`harness/CONTEXT.md` §2 and required by
`harness/test_warmup_policy.py`), with no loose spike directories,
reports, or results trees outside their zone. The `contenders/` zone
holds consolidated contender builds and their shared runtime assets
only (today: `rust-camel-lib/` single crate, `node/` shared runtime
dir); per-scenario data never lives there. The `audits/` zone holds
the bench program's tracked decision artifacts (fixture-fairness
audit, canonical-shape proposal, re-run manifest, ruling proposals);
bench artifacts under `benchmarks/` stay tracked — it is only the
`docs/benchmarks/` period memos that the owner's untracking ruling
(ef337c0f, bd rc-mq1sh, 2026-09-17) removes from version control.

#### Scenario: Level-1 audit

- **GIVEN** the repository after consolidation and the audits-zone
  addition (rc-h42s6, 2026-09-16)
- **WHEN** listing `benchmarks/` at level 1
- **THEN** the listing contains only `README.md`, `.gitignore`,
  `bench`, `audits/`, `attic/`, `contenders/`, `harness/`,
  `records/`, `runner/`, `scenarios/`, and
  `docs-investigation-strategy.md`
- **AND** no file matching `spike-*`, `results/`, or a historical
  report remains at level 1 (audit artifacts live under `audits/`)

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

### Requirement: Era-1 freeze

The system SHALL preserve era-1 evidence without tracking it in the
working tree: the v2/v3/v4/addendum/consultation reports under
`docs/benchmarks/` (including `history/`) are untracked by the
owner's governance ruling (ef337c0f, bd rc-mq1sh, 2026-09-17 — period
evidence does not belong in version control; on-disk copies remain,
gitignored, in the owner checkout), the full history stays reachable
via the git tag `bench/era-1-final` and git history, and historical
results trees stay tracked under `benchmarks/attic/results-era-1/`.
The durable numbers behind standing decisions SHALL remain quoted in
tracked artifacts — the gauges-ON A/B verdict in ADR-0066 and in
`benchmarks/runner/RUNBOOK.md` — and tracked references to the
untracked reports SHALL name the tag retrieval so no link is
presented as live in a fresh clone.

#### Scenario: Reports reachable after freeze

- **GIVEN** the era-1 reports untracked from the working tree
- **WHEN** a reader checks out tag `bench/era-1-final` (or runs
  `git show bench/era-1-final:<path>`)
- **THEN** the reports are present at `docs/benchmarks/` with
  byte-identical content
- **AND** tracked references to the untracked reports (e.g.
  `scenarios/COVERAGE.md`) name the tag retrieval, so no link is
  presented as live in a fresh clone

#### Scenario: Gauge premise preserved

- **GIVEN** the v4 addendum reachable via tag `bench/era-1-final`
- **WHEN** a reader traces the gauges-ON decision
- **THEN** ADR-0066 cites the gauge A/B verdict (point 0.9890,
  CI [0.9785, 1.0126], interval includes 1.0) and
  `benchmarks/runner/RUNBOOK.md` records the lever-study ratio with
  the same bounds

### Requirement: Ratio confidence intervals

The harness SHALL compute paired bootstrap confidence intervals for M3
throughput ratios between two cells from the same run (identical
measurement-order provenance, equal round count, round indices 0..n−1),
reusing the existing PRNG machinery (a single `SplitMix64` stream per
invocation, controlled by `--seed`); the CI method for ratios is the
percentile bootstrap — BCa is deliberately not applied to a ratio of
two medians (undefined for that statistic; rationale recorded in the
loadgen ratios module) — exposed as an `aggregate-ratios` loadgen
subcommand that hard-errors on any validation mismatch.

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

### Requirement: Payload-size axis

The benchmark harness SHALL support selecting a transport-body size class
(1024, 32768, 262144, or 1048576 bytes — other values rejected) for
load-driven measurements (Protocol A and M3 throughput). Among
timer-driven fixtures (Protocol B), a `BENCH_PAYLOAD_BYTES` environment
variable constructing a byte-identical canonical body applies to the
payload-carrying fixture family — today `t2-json`, in every artifact;
`split-aggregate` (a fixed 100-item canonical JSON array) and
`t2-realistic-eip` (no canonical byte payload) are outside the payload
axis by design.
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
- **Then** the per-scenario entry scripts resolve directly from their
  committed `.mjs` sources (no build), and the family's single shared
  `package.json` at `benchmarks/contenders/node/` reports one
  `<would-build:npm-ci>` marker without failing the dry-run

### Requirement: Canonical full-matrix run

The system SHALL treat `bash benchmarks/bench run-all` (no env vars, no
flags) as the canonical run: EVERY active scenario × every registered
contender — the asymmetric **53-cell matrix** (five full scenarios × 8
contenders + two bridge scenarios × 6: the 4-artifact core + 2 node, plus
the `axum-bare` reference contender for `http-server`) — cold +
warm-where-applicable, with memory gauges ON in every measured cell and
randomized measurement order (order seed recorded in the record's
`protocol`), recorded as one run-level record. The m3/m4 measured set
excludes the node family (rc-h42s6 phase-2 alignment, e_opus ruling D5,
2026-09-16: `M3_EXCLUDED_CONTENDERS` in `harness/run.sh`, mirrored as
`M3_M4_EXCLUDED_CONTENDERS` in `summarize.py`, the equality
drift-guarded by `test_summarize.py::test_roster_mirror_no_drift`);
node cells still register and measure m1/m2 — only the m3/m4 arms drop
them, and any lift of the exclusion lands as its own spec delta.
`--scenarios=` remains a harness-level developer knob only. Scenario
discovery is automatic and ACTIVE-ONLY (registered scenarios; `spike-*`
and non-registered dirs like `multi-step` excluded — also from the
`meta.json` `scenarios` list).

#### Scenario: one command full coverage

- **Given** the digest-pinned runner image present and a host meeting
  the quiet-host criteria in `benchmarks/harness/CONTEXT.md`
- **When** `bash benchmarks/bench run-all` executes
- **Then** the run registers the full 53-cell matrix (m1/m2 for every
  cell; m3/m4 for the non-node measured set) and writes raw
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
- **Then** memory gauges are enabled in every m3/m4 measured cell
  (the node family is outside the m3/m4 measured set per the D5
  exclusion; its cells keep m1/m2)
- **And** measurement order is randomized with the seed recorded in
  the record's `protocol.order_seed`

#### Scenario: human-invoked execution

- **Given** the agent has prepared and validated the runner image,
  fixtures, quiet-host criteria, and run configuration
- **When** the run is to execute (hours-long, quiet-host predicate)
- **Then** a human invokes the one command; the agent's deliverable
  ends at preparation and post-run record validation

### Requirement: Warm tick mode

The t2-json, split-aggregate and t2-realistic-eip fixtures in EVERY
runtime — camel-standalone-dsl, camel-standalone-yaml,
camel-quarkus-dsl-native, camel-quarkus-yaml-native, rust-camel-lib,
rust-camel-cli (via the harness's direct argv latency-file plumbing:
`BENCH_LATENCY_FILE` and `BENCH_LATENCY_MODE=route` injected at cell
launch), node-native, node-fastify — SHALL run a tick loop as part of
the measured route, writing `BENCH_LATENCY` records to the cell's
latency file under the existing Protocol B contract (10 ms timer
period, first fire immediate, saturation escalates via won't-measure,
not period adaptation). The readiness marker latches inside the FIRST
completed tick: the timer starts at route start with zero initial
delay and the marker fires exactly once on that code-path position —
pinning the marker to real measured work rather than to a pre-loop
handshake. When tick emission lands, `t2-realistic-eip` is REMOVED
from the Protocol B m2 skip list — all 24 warm cells (3 scenarios × 8
runtimes) must measure.

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
  rust-camel-cli via its argv latency-file plumbing (a scenario
  whose fixtures tick asymmetrically across runtimes is a hard error,
  not a partial warm matrix)

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
- **AND** stdout carries NO per-request trace lines (`BENCH_HTTP_REQUEST`
  is absent) — the minimal-bare fixture shape (e_opus ruling D1,
  2026-09-16, bd rc-h42s6) removed the rc-am22 smoke-trace contract from
  the measured route; per-request observability is asserted nowhere in
  the smoke (id checks are WARN-only, ruling D2)

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

#### Scenario: smoke case with owner-run evidence regeneration

- **GIVEN** the http-server smoke harness extended with the axum-bare case
  (runnable standalone via an artifact filter, launched on a free port via
  the `BENCH_AXUM_BARE_PORT` env override)
- **WHEN** the smoke runs
- **THEN** it asserts the marker and `200`/`pong` as HARD requirements
  (minimal-bare shape, e_opus ruling D2, 2026-09-16: the fixture emits no
  per-request trace lines), while `id=1` verification is WARN-only
  observability; the resulting transcript log carries no timing-like
  numbers
- **AND** committed transcripts exist only after an owner-run smoke — the
  pre-alignment transcripts were deleted (585858c6) and benchmark runs,
  smoke evidence regeneration included, are owner-exclusive

#### Scenario: published records stay byte-identical

- **GIVEN** records published before this change (52-cell persisted rosters)
- **WHEN** `summarize.py --check` regenerates their summaries after this
  change lands
- **THEN** every regenerated summary.md is byte-identical to the committed
  one (completeness validates against each record's persisted
  `expected_cells`, never current harness constants)
