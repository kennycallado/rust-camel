# benchmark-records delta — bench-consol-tick

## MODIFIED Requirements

### Requirement: Run-level record schema

The records layer SHALL define a `run.json` schema
(`schema_version: 1`) capturing per run: `run_id`, `date`, `era`,
`git_commit`, `container_digest`, `host_provenance`, `protocol`
(rounds, duration_secs, warmup_secs, order_seed), `cells`
(scenario, contender, variant, payload_class, metric, round
values, median, unit, input digest), and `ratios` (numerator,
denominator, metric, point estimate, CI bounds, method). `run_id` is
the launch timestamp `<YYYYMMDDTHHMMSSZ>` — chronological, no
sequence numbering; legacy era-1/pre-2026-08-31 ids keep the
`<YYYYMMDD>-v<N>` shape and remain readable (legacy `run_seq` metas
still compose their old id; never emitted for new runs). The launch
`meta.json` records `run_id` and `scenarios` (comma-joined ACTIVE
scenario names — inactive scenarios like `multi-step` never appear);
legacy `subset` metas remain readable.

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

#### Scenario: timestamp run_id round-trip

- **GIVEN** a `meta.json` with `"run_id": "20260905T142601Z"`
- **WHEN** `bench summarize` builds the record
- **THEN** `run.json.run_id` equals `20260905T142601Z` and no `-v<N>`
  suffix exists anywhere in the record

#### Scenario: legacy meta still summarizes

- **GIVEN** a pre-2026-08-31 `meta.json` with `run_seq: 4`
- **WHEN** summarized
- **THEN** the legacy `<YYYYMMDD>-v4` id composes (old run dirs stay
  readable; no new run emits `run_seq`)

### Requirement: Records index

The system SHALL maintain `records/index.json` as an array of run
records (one entry per published run with run_id, date, era,
git_commit, `scenarios` — comma-joined, derived from the cells —,
and pointer to the run dir), serving as the dated-tables view that is
fetchable as a static file from the docs infrastructure. Entries
written before 2026-08-31 carry `subset` with the same shape (legacy
vocabulary; readers tolerate both keys).

#### Scenario: Index updated on publish

- **GIVEN** a validated run record not yet published
- **WHEN** `bench publish` executes
- **THEN** the run dir appears under `records/`
- **AND** `records/index.json` contains one new entry, ordered by
  date, with a relative pointer to the run dir
- **AND** the entry's `scenarios` field lists the run's scenario
  names comma-joined and sorted (no `subset` key written for new
  entries)

#### Scenario: Static fetch

- **GIVEN** the docs site build including `records/`
- **WHEN** a client fetches the index URL
- **THEN** it receives valid JSON parseable without execution
  environment

## ADDED Requirements

### Requirement: Fail-closed complete-record publish

A record is COMPLETE iff every cell of the EXPECTED registered roster
for the run's `meta.json.scenarios` is measured: m1 data for every
expected cell AND m2 data for every expected cell whose scenario's
warm concept applies. Warm applicability is declared scenario
vocabulary: `startup-minimal` = cold-only (`warm: n/a` — absence is
not a gap); `http-server` = applicable (Protocol A loadgen); t2-json,
split-aggregate, t2-realistic-eip, xsd-validation-bridge,
xslt-bridge = applicable (Protocol B ticks). The expected roster
follows the harness asymmetry (five full scenarios × 8 contenders +
two bridge scenarios × 6 = 52 cells) and SHALL be persisted in the
record or recomputed deterministically by the publisher; a wholly
absent cell (no directory, no JSON) counts as MISSING. `bench publish`
SHALL fail closed on incomplete records: nonzero exit and a list of
every missing cell (scenario/contender/metric).

#### Scenario: complete record publishes clean

- **Given** a record whose full expected roster has m1 data and whose
  warm-applicable cells all have m2 data
- **When** `bench publish` executes
- **Then** it succeeds with no completeness complaint

#### Scenario: missing metric rejects publish

- **Given** a record where one warm-applicable cell has m1 but no m2
  data
- **When** `bench publish` executes
- **Then** it exits nonzero listing that cell (scenario, contender,
  metric m2)

#### Scenario: wholly missing cell rejects publish

- **Given** a record where one expected cell produced no directory
  and no per-cell JSON at all
- **When** `bench publish` executes
- **Then** it exits nonzero listing that cell — validation is against
  the EXPECTED roster, not the observed cells

#### Scenario: n/a warm is not a gap

- **Given** a record where `startup-minimal` cells have m1 data and no
  m2 data
- **When** `bench publish` executes
- **Then** no completeness complaint is raised for those cells
  (`startup-minimal` warm is `n/a` by design)
