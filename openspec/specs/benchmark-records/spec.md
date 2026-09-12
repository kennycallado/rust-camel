# benchmark-records Specification

## Purpose
TBD - created by archiving change bench-era-2. Update Purpose after archive.
## Requirements
### Requirement: Run-level record schema

The records layer SHALL define a `run.json` schema
(`schema_version: 2`) capturing per run: `run_id`, `date`, `era`,
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

An m2 cell is either MEASURED (today's shape: latency fields, no
`status` field) or ATTEMPTED (`status` ∈ {`unconverged`,
`attempted-timeout`}, nonempty `reason` string, `rounds` count, and
NO latency fields). No other cell shape is valid: a cell with an
unknown `status`, empty/missing `reason`, or `status` mixed with
latency fields is invalid and counts as MISSING. Measured-cell shape
is identical to schema_version 1 — the extension is additive and
one-way compatible: v2-reading tooling SHALL read v1 and v2 records;
older tooling is not required to read v2.

#### Scenario: Record generated from per-cell JSON

- **GIVEN** a finished run directory with per-cell JSON outputs
- **WHEN** `bench summarize` executes
- **THEN** it emits `run.json` with `schema_version: 2` and every
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

#### Scenario: attempted cell shape is closed

- **GIVEN** an m2 cell emitted with `status: "unconverged"`, a
  nonempty `reason`, and no latency fields
- **WHEN** the publisher validates the record
- **THEN** the cell shape is valid
- **WHEN** a cell carries `status: "weird"`, or an empty `reason`,
  or `status` together with latency fields
- **THEN** the shape is invalid and the cell counts as MISSING

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

### Requirement: Fail-closed complete-record publish

A record SHALL be COMPLETE only when every cell of the EXPECTED registered
roster for `meta.json.scenarios` is PRESENT: m1 data for every expected cell
and m2 data for every warm-applicable cell. A warm-applicable m2 cell is
PRESENT when shape is valid and it is MEASURED (an `m2-summary.json` exists in
at least one round; attempt evidence is ignored) or ATTEMPTED. The SUMMARIZER
derives status from harness evidence and the PUBLISHER re-validates the shape.

- `unconverged`: `protocol-a-summary.txt` contains a `warmup failed-stability:`
  line with reason `MessageBoundUnconverged`, `TimeBoundUnconverged`, or
  `InsufficientSamples`, and `status=failed reason=measure-a-error`.
- `attempted-timeout`: `exit-codes.txt` contains
  `# probe reason: no BENCH_LATENCY within 30s timeout`.

All malformed, unrecognized, incomplete, or conflicting evidence SHALL leave
the cell MISSING with a loud warning naming cell and artifact. No publishable
`unknown` status exists. `startup-minimal` remains warm `n/a`; `http-server`
is applicable, as are t2-json, split-aggregate, t2-realistic-eip,
xsd-validation-bridge, and xslt-bridge; the expected roster is five full
scenarios times eight contenders plus two bridge scenarios times six, with
axum-bare making http-server 53 cells versus 52 before it joined. The roster
SHALL be persisted in the record or recomputed deterministically, and validation
SHALL use that record-wide roster rather than postdating harness constants;
wholly absent cells are MISSING. `bench publish`
SHALL fail closed with nonzero exit and every missing cell. Measured data SHALL
override attempt evidence. Historical schema-version 1 records SHALL remain
valid beside additive status fields in schema-version 2, and mixed index rebuild
SHALL preserve `index_schema_version: 1`.

#### Scenario: complete record publishes clean
- **GIVEN** every expected m1 and warm-applicable m2 cell is present
- **WHEN** `bench publish` executes
- **THEN** it succeeds without a completeness complaint

#### Scenario: pre-reference records stay complete
- **GIVEN** a record persists its 52-cell roster before `axum-bare`
- **WHEN** validation runs after that contender joins
- **THEN** validation uses persisted `expected_cells` and remains unchanged

#### Scenario: missing metric rejects publish
- **GIVEN** a warm-applicable cell has m1 but no m2, summary, or evidence
- **WHEN** `bench publish` executes
- **THEN** it exits nonzero and lists the cell

#### Scenario: wholly missing cell rejects publish
- **GIVEN** an expected cell has no directory, JSON, or evidence
- **WHEN** `bench publish` executes
- **THEN** it exits nonzero and lists the cell

#### Scenario: n/a warm is not a gap
- **GIVEN** `startup-minimal` has m1 but no m2
- **WHEN** `bench publish` executes
- **THEN** no completeness complaint is raised

#### Scenario: unconverged warmup counts as present with status
- **GIVEN** a warm-applicable cell has a historical `MessageBoundUnconverged`
  line plus the required failure status and no m2 summary
- **WHEN** it is summarized and published
- **THEN** it has status `unconverged`, no latency fields, and counts present

#### Scenario: trailing-window warmup failure counts as present
- **GIVEN** a warm-applicable cell has `TimeBoundUnconverged` or
  `InsufficientSamples` plus the required failure status
- **WHEN** it is summarized and published
- **THEN** it has status `unconverged`, no latency fields, and counts present

#### Scenario: probe timeout counts as present with status
- **GIVEN** a cell has the required probe-timeout sentinel and no m2 summary
- **WHEN** it is summarized and published
- **THEN** it has status `attempted-timeout` and counts present

#### Scenario: measured wins over attempt evidence
- **GIVEN** a later round has valid m2 data and an earlier round has an
  unconverged sentinel
- **WHEN** the run is summarized
- **THEN** measured data wins and status is absent

#### Scenario: conflicting statuses stay missing
- **GIVEN** rounds contain unconverged and attempted-timeout evidence without
  m2 data
- **WHEN** the run is summarized
- **THEN** a loud warning is emitted and the cell remains MISSING

#### Scenario: malformed evidence stays missing
- **GIVEN** a round has truncated or unrecognized sentinel content and no m2
  summary
- **WHEN** the run is summarized
- **THEN** a loud warning is emitted and the cell remains MISSING

#### Scenario: status schema is additive and one-way
- **GIVEN** schema-version 1 and schema-version 2 records coexist
- **WHEN** validation and index rebuild run
- **THEN** v1 remains valid and additive v2 statuses do not rewrite v1

