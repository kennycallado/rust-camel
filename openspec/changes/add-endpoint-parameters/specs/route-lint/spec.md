## MODIFIED Requirements

### Requirement: LintRoute captures every URI-bearing location with spans

The engine SHALL construct `LintRoute` by walking the CST (or using noyalib's span-preserving
deserialization) and SHALL capture, each with a byte-exact `Spanned<T>`: the route-level
`from` URI; each endpoint URI (`to` / `uri` leaves); and every URI option key and value
(parsed out of the URI query string, the step option map, or the endpoint `parameters` map —
`parameters:` entries SHALL be captured as options attached to the same endpoint, each key
and value with its own byte-exact span into the source). Structural containers that hold
children but carry no URI themselves (`choice` with `when`/`otherwise` branches, `multicast`,
`scatter_gather.endpoints` — the containers present in `route-schema.json`; `pipeline` does
not exist in the schema) SHALL be traversed recursively so that endpoint URIs nested at any
depth are captured. The traversal SHALL be driven by the schema: lint resolves which node
types may contain `to`/`from`/`uri` or nested children by reading the `route-schema.json`
definition, so adding a new step container in the schema requires no lint code change beyond
re-syncing the embedded copy. Capturing a location SHALL NOT require `camel-dsl`.

#### Scenario: Route-level from URI is captured with a span

- **GIVEN** a route file with `from: direct:start` where `direct:start` starts at byte offset 12
- **WHEN** the engine builds `LintRoute`
- **THEN** the captured `from` value has a span whose start offset is 12

#### Scenario: Nested child step URIs are captured with spans

- **GIVEN** a route file with a `choice`/`when` branch (or a `multicast`) containing a child step `to: log:nested`
- **WHEN** the engine builds `LintRoute`
- **THEN** the child step's `to` value is present in the captured steps with its own byte-exact span, distinct from the parent step's span

#### Scenario: scatter_gather endpoint URIs are captured with spans

- **GIVEN** a route file with a `scatter_gather` step whose `endpoints` array contains `direct:a` and `direct:b`
- **WHEN** the engine builds `LintRoute`
- **THEN** both endpoint URIs are captured as URI-bearing locations, each with its own byte-exact span

#### Scenario: Option keys and values are captured with spans

- **GIVEN** a step `timer:foo?period=1s` where `period` starts at byte offset 30 and `1s` at 37
- **WHEN** the engine builds `LintRoute`
- **THEN** the captured option key `period` has start offset 30 and the option value `1s` has start offset 37

#### Scenario: parameters map entries are captured as options with spans

- **GIVEN** a step with `to: kafka:orders` and a `parameters:` map entry `brokers: my-host:9092` where the key starts at byte offset 42 and the value at 51
- **WHEN** the engine builds `LintRoute`
- **THEN** the captured endpoint carries an option `brokers` with key span start 42 and value span start 51, indistinguishable for rule purposes from a query-string option

#### Scenario: Step-level and inner parameters both reach a nested endpoint

- **GIVEN** a step with object-form `enrich: {uri: db:query, parameters: {dataSource: customers}}` and a sibling step-level `parameters: {timeoutS: "5000"}`
- **WHEN** the engine builds `LintRoute`
- **THEN** the nested `db:query` endpoint carries BOTH options `dataSource` and `timeoutS` — the two maps are concatenated, never either/or, so no entry is dropped from rule evaluation

#### Scenario: Route-level from parameters are captured as options

- **GIVEN** a route with `from: timer:tick` and a route-level `parameters: {period: "2500"}`
- **WHEN** the engine builds `LintRoute`
- **THEN** the `from` endpoint carries the option `period` with byte-exact key and value spans, indistinguishable for rule purposes from a query-string option

### Requirement: Zero false positives over a discovered in-tree corpus

The `camel-cli` integration test `tests/lint_corpus.rs` SHALL discover every route file in the
repository by a glob rule (covering `examples/**/*.{yaml,json}` and
`crates/**/tests/fixtures/**/*.{yaml,json}`, plus any route fixtures referenced by the
schema-validation corpus), run the engine with the production catalog over each, and compare
the emitted diagnostics against a checked-in baseline file
`tests/fixtures/lint-corpus-baseline.ron` (parsed with the `ron` crate, a `camel-cli`
dev-dependency). The test SHALL fail if any emitted diagnostic is absent from the baseline (a
false positive) or any baseline diagnostic is missing (a regression). Baseline updates are
reviewed diffs. The corpus file count is discovered at test-run time (not hardcoded). The
corpus SHALL include at least one fixture exercising a secret inside a `parameters:` map, and
its baseline entry SHALL pin the emitted R-SECRET diagnostic. This is
the merge gate: a rule that cannot meet zero false positives on the corpus SHALL be gated (the
`unverified-scheme` guard) or cut before merge.

#### Scenario: Corpus run matches the checked-in baseline

- **GIVEN** the engine built with the production catalog and all five rules active, and the checked-in baseline
- **WHEN** `tests/lint_corpus.rs` runs over the discovered corpus
- **THEN** the set of emitted diagnostics equals the baseline set exactly; the test passes

#### Scenario: A new false positive fails the gate

- **GIVEN** a change to the engine that emits a diagnostic against a corpus file not present in the baseline
- **WHEN** `tests/lint_corpus.rs` runs
- **THEN** the test fails, naming the file and diagnostic code that is outside the baseline

#### Scenario: Secret in parameters map is diagnosed with an in-map span

- **GIVEN** a corpus fixture where a catalog-secret option is set inside a `parameters:` map
- **WHEN** the engine lints the fixture
- **THEN** R-SECRET is emitted with a span pointing at the value inside the `parameters:` map, and the baseline contains that entry
