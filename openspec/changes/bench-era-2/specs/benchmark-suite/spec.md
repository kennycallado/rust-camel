## ADDED Requirements

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

## MODIFIED Requirements

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
