## MODIFIED Requirements

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

### Requirement: Fail-closed complete-record publish

A record is COMPLETE iff every cell of the EXPECTED registered roster
for the run's `meta.json.scenarios` is PRESENT: m1 data for every
expected cell AND m2 data for every expected cell whose scenario's
warm concept applies. An m2 warm-applicable cell is PRESENT when its
shape is valid AND it is either MEASURED (an `m2-summary.json` was
produced in at least one round — attempt evidence is then ignored) or
ATTEMPTED, where the SUMMARIZER derives the status from
harness-written evidence in the cell's round directories and the
PUBLISHER re-validates the derived shape:

- `unconverged`: a `protocol-a-summary.txt` contains BOTH
  `warmup failed-stability: MessageBoundUnconverged` AND
  `status=failed reason=measure-a-error`.
- `attempted-timeout`: an `exit-codes.txt` contains
  `# probe reason: no BENCH_LATENCY within 30s timeout`.

All other evidence — malformed sentinel files, unrecognized content,
or the two statuses conflicting across rounds — leaves the cell
MISSING with a loud warning naming cell and artifact; no publishable
"unknown" status exists. Warm applicability is declared scenario
vocabulary: `startup-minimal` = cold-only (`warm: n/a` — absence is
not a gap); `http-server` = applicable (Protocol A loadgen); t2-json,
split-aggregate, t2-realistic-eip, xsd-validation-bridge,
xslt-bridge = applicable (Protocol B ticks). The expected roster
follows the harness asymmetry (five full scenarios × 8 contenders +
two bridge scenarios × 6 = 52 cells) and SHALL be persisted in the
record or recomputed deterministically by the publisher; a wholly
absent cell (no directory, no JSON, no evidence) counts as MISSING.
`bench publish` SHALL fail closed on incomplete records: nonzero exit
and a list of every missing cell (scenario/contender/metric).

#### Scenario: complete record publishes clean

- **Given** a record whose full expected roster has m1 data and whose
  warm-applicable cells all have m2 data
- **When** `bench publish` executes
- **Then** it succeeds with no completeness complaint

#### Scenario: missing metric rejects publish

- **Given** a record where one warm-applicable cell has m1 but no m2
  data, no summary, and no recognizable evidence
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

#### Scenario: unconverged warmup counts as present with status

- **Given** a warm-applicable m2 cell whose round directories contain
  `protocol-a-summary.txt` with both the `MessageBoundUnconverged`
  line and `status=failed reason=measure-a-error`, and no
  `m2-summary.json` in any round
- **When** the run is summarized and then published (publish consumes
  the summarized `run.json`)
- **Then** the cell appears in `run.json` with
  `status: "unconverged"`, a nonempty `reason`, and no latency
  fields, and the publish gate counts it present (completeness is
  not blocked)

#### Scenario: probe timeout counts as present with status

- **Given** a warm-applicable m2 cell whose round directory contains
  an `exit-codes.txt` with
  `# probe reason: no BENCH_LATENCY within 30s timeout` and no
  `m2-summary.json` in any round
- **When** the run is summarized and then published
- **Then** the cell appears with `status: "attempted-timeout"` and
  the gate counts it present

#### Scenario: measured wins over attempt evidence

- **Given** a warm-applicable m2 cell with a valid `m2-summary.json`
  in round 2 and an unconverged sentinel in round 0
- **When** the run is summarized
- **Then** the cell is MEASURED from round 2 (no status field) and
  the attempt evidence is ignored

#### Scenario: conflicting statuses stay missing

- **Given** a warm-applicable m2 cell with an `unconverged` sentinel
  in one round dir and an `attempted-timeout` sentinel in another,
  and no `m2-summary.json` anywhere
- **When** the run is summarized
- **Then** a loud warning names the cell and the conflicting
  artifacts, and the cell remains MISSING

#### Scenario: malformed evidence stays missing

- **Given** a warm-applicable m2 cell whose round dir contains
  sentinel files with truncated or unrecognized content and no
  `m2-summary.json`
- **When** the run is summarized
- **Then** a loud warning names the cell and artifact, and the cell
  remains MISSING — no publishable unknown status exists

#### Scenario: status schema is additive and one-way

- **Given** records published under schema_version 1 (no status
  fields) alongside a new schema_version 2 record
- **When** the publisher validates the v1 record and the index is
  rebuilt over the mixed v1/v2 set
- **Then** the v1 record validates unchanged, the mixed rebuild
  succeeds, and `index_schema_version` remains 1
