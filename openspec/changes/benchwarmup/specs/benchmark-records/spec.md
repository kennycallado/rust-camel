## MODIFIED Requirements

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
