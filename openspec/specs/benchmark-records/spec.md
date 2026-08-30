# benchmark-records Specification

## Purpose
TBD - created by archiving change bench-era-2. Update Purpose after archive.
## Requirements
### Requirement: Run-level record schema

The records layer SHALL define a `run.json` schema
(`schema_version: 1`) capturing per run: `run_id`, `date`, `era`,
`git_commit`, `container_digest`, `host_provenance`, `protocol`
(rounds, duration_secs, warmup_secs, order_seed), `cells`
(scenario, contender, variant, payload_class, metric, round
values, median, unit, input digest), and `ratios` (numerator,
denominator, metric, point estimate, CI bounds, method).

#### Scenario: Record generated from per-cell JSON

- **GIVEN** a finished run directory with per-cell JSON outputs
- **WHEN** `bench summarize` executes
- **THEN** it emits `run.json` with `schema_version: 1` and every
  cell populated from the per-cell files
- **AND** the JSON is deterministic (sorted keys, fixed float
  representation; two invocations are byte-identical)

#### Scenario: Schema rejects hand-typed drift

- **GIVEN** a `summary.md` hand-edited after generation (a number
  no longer derivable from `run.json`)
- **WHEN** the checksum guard runs (regenerate from `run.json`,
  diff)
- **THEN** the guard fails, detecting the hand-edited number

### Requirement: Records index

The system SHALL maintain `records/index.json` as an array of run
records (one entry per published run with run_id, date, era,
git_commit, subset, and pointer to the run dir), serving as the
dated-tables view that is fetchable as a static file from the
docs infrastructure.

#### Scenario: Index updated on publish

- **GIVEN** a validated run record not yet published
- **WHEN** `bench publish` executes
- **THEN** the run dir appears under `records/`
- **AND** `records/index.json` contains one new entry, ordered by
  date, with a relative pointer to the run dir

#### Scenario: Static fetch

- **GIVEN** the docs site build including `records/`
- **WHEN** a client fetches the index URL
- **THEN** it receives valid JSON parseable without execution
  environment

### Requirement: Generated summaries only

Per-run human-readable summaries SHALL be generated from
`run.json` (tables per metric, ratio table with CI columns), never
hand-authored; the ratio math SHALL reuse the existing
`aggregate-ratios` implementation as the single source.

#### Scenario: Summary derived from record

- **GIVEN** a `run.json` with two contenders on one scenario
- **WHEN** the summary generates
- **THEN** it contains a per-metric table with both contenders and
  a ratio row with point estimate and CI
- **AND** every number in the summary appears verbatim in
  `run.json`

### Requirement: Digest-pinned runner

The canonical runner SHALL be built from `runner/Dockerfile` and
recorded by image digest; mutable tags (e.g. `:latest`) SHALL NOT
appear in any record or canonical run configuration.

#### Scenario: Digest recorded, tag rejected

- **GIVEN** the runner image built from `runner/Dockerfile`
- **WHEN** the v1 run record is written
- **THEN** `container_digest` holds a `sha256:` digest
- **AND** the summary-checksum records guard in CI fails if any
  record field references a mutable tag

