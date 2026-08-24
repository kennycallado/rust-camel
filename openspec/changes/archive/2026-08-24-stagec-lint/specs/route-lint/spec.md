## ADDED Requirements

### Requirement: R-MOCK-IN-PRODUCTION warns on intercept-replaceable mock sends

The lint catalog SHALL include a rule with stable code `R-MOCK-IN-PRODUCTION`
that emits a `Warning` diagnostic for every endpoint whose origin key is
`to` or `endpoints` (the send surfaces that compile to interceptable
`To` steps) and whose URI value starts with `mock:`. Endpoints emitted from
other URI-bearing keys (`from`, `uri`, `wire_tap`, `enrich`, `poll_enrich`,
`dead_letter_channel`) SHALL NOT be flagged: no declarative interception
replacement exists for them. The diagnostic message SHALL point at the
migration: declare `intercepts:` (`skipTo`/`divertCopyTo`) in a
`*.test.yaml` instead (see the testing guide). Warning severity SHALL NOT
affect the `camel lint` exit code. The CLI SHALL suppress
`R-MOCK-IN-PRODUCTION` (and only that code) for files under a
`tests/fixtures/` path component — the fixture exemption of ADR-0064 §5
("inline `mock:` stays legitimate in pure test-fixture routes that
`camel run` never loads"); other rules' diagnostics SHALL be unaffected by
the suppression. Escalation to Error severity is a documented future change
gated on ecosystem conversion (ADR-0064 §5 lazy migration, no flag-day); it
SHALL NOT be encoded in this rule's initial severity.

#### Scenario: to mock send warns

- **GIVEN** a route file with a step `to: mock:out`
- **WHEN** the engine runs the rule over the document
- **THEN** exactly one `R-MOCK-IN-PRODUCTION` Warning is emitted with a span at the `mock:out` URI and a message naming the intercepts migration

#### Scenario: endpoints recipient list mock warns per occurrence

- **GIVEN** a route file with a recipient list `endpoints: [mock:a, mock:b]`
- **WHEN** the engine runs the rule over the document
- **THEN** two `R-MOCK-IN-PRODUCTION` Warnings are emitted, one per mock occurrence, each with its own span

#### Scenario: mock with query parameters warns

- **GIVEN** a route file with a step `to: mock:out?count=2`
- **WHEN** the engine runs the rule over the document
- **THEN** one `R-MOCK-IN-PRODUCTION` Warning is emitted for the occurrence

#### Scenario: two to-mock steps warn twice

- **GIVEN** a route file with steps `to: mock:first` and `to: mock:second`
- **WHEN** the engine runs the rule over the document
- **THEN** two `R-MOCK-IN-PRODUCTION` Warnings are emitted with distinct spans

#### Scenario: non-mock send is silent

- **GIVEN** a route file with a step `to: kafka:orders`
- **WHEN** the engine runs the rule over the document
- **THEN** no `R-MOCK-IN-PRODUCTION` diagnostic is emitted

#### Scenario: non-interceptable origins are silent

- **GIVEN** a route file containing `wire_tap: mock:tap`, `enrich: mock:enr`, `poll_enrich: mock:poll`, `enrich: {uri: mock:uri}` (object form), `from: mock:src`, and `dead_letter_channel: mock:dlq`
- **WHEN** the engine runs the rule over the document
- **THEN** no `R-MOCK-IN-PRODUCTION` diagnostic is emitted for any of them

#### Scenario: warning does not affect exit code

- **GIVEN** a route file whose only finding is an `R-MOCK-IN-PRODUCTION` Warning
- **WHEN** `camel lint <file>` runs
- **THEN** the diagnostic is printed and the exit code is 0

#### Scenario: fixture path suppresses only the mock rule

- **GIVEN** a route file under a `tests/fixtures/` path containing both `to: mock:result` and an unknown option for a known scheme
- **WHEN** `camel lint <file>` runs
- **THEN** no `R-MOCK-IN-PRODUCTION` diagnostic is emitted and the unknown-option Error diagnostic is emitted (suppression is code-scoped)

#### Scenario: test documents are skipped

- **GIVEN** a `*.test.yaml` containing an `intercepts:` block with `mock:` targets
- **WHEN** `camel lint` is invoked on it
- **THEN** the file is skipped with the existing info diagnostic and no `R-MOCK-IN-PRODUCTION` is emitted

## MODIFIED Requirements

### Requirement: Lint engine is runtime-free and produces span-exact diagnostics

The `camel-lint` crate SHALL expose a `LintEngine` that parses YAML/JSON source itself via
the `noyalib` CST (it SHALL NOT depend on `camel-dsl`, `camel-core`, or `camel-cli`),
constructs a span-carrying `LintRoute` view, and runs `Rule` implementations over it. Each
emitted `Diagnostic` SHALL carry a stable `DiagnosticCode`, a `Severity`, a byte-exact
`Span` (start/end offsets into the source text), a human message, and an optional `Fix`. The
engine SHALL accept the component catalog as `Arc<dyn ComponentMetadataCatalog>` and SHALL
expose no catalog constructor and no dependency on `Registry`. The engine SHALL tolerate
partial/malformed input: a document that fails syntax parsing SHALL still be reported without
panicking, and later tiers SHALL be skipped for the unparseable document. The workspace
hexagonal-architecture test SHALL be extended to assert that `camel-lint` does not depend on
`camel-core` or `camel-dsl`.

#### Scenario: Valid document yields no diagnostics

- **GIVEN** a syntactically valid, schema-valid route file whose URIs and options are all known to the catalog and whose `to`/`endpoints` sends target no `mock:` endpoint
- **WHEN** the engine runs all six rules over the document
- **THEN** the engine returns an empty diagnostic list

#### Scenario: Diagnostic span is byte-exact, not a line range

- **GIVEN** a route file with a step `timer:foo?bogus=1` where the unknown option key `bogus` starts at byte offset 42, and a catalog that knows `timer` (so the option is validated, not the scheme)
- **WHEN** R-URI-known runs over the document
- **THEN** the emitted `RUriKnown(UnknownOption)` diagnostic's `Span` start offset equals 42 and its end offset equals the byte after the last byte of `bogus` (not the start of the line, not the whole URI, not the whole file)

#### Scenario: Partial input does not crash the engine

- **GIVEN** a route file with a YAML syntax error that prevents construction of the route view
- **WHEN** the engine runs over the document
- **THEN** R-SYN emits a syntax diagnostic and the engine returns without panicking; R-SCHEMA and the semantic rules are skipped for the unparseable document

#### Scenario: Engine does not depend on camel-core or camel-dsl

- **GIVEN** the workspace hexagonal-architecture test is run
- **WHEN** the test checks `camel-lint`'s dependency edges
- **THEN** neither `camel-core` nor `camel-dsl` appears as a dependency of `camel-lint`

### Requirement: Zero false positives over a discovered in-tree corpus

The `camel-cli` integration test `tests/lint_corpus.rs` SHALL discover every route file in the
repository by a glob rule (covering `examples/**/*.{yaml,json}` and
`crates/**/tests/fixtures/**/*.{yaml,json}`, plus any route fixtures referenced by the
schema-validation corpus), run the engine with the production catalog over each, apply the
same `R-MOCK-IN-PRODUCTION` fixture-path suppression the CLI applies (files under a
`tests/fixtures/` path component contribute no `R-MOCK-IN-PRODUCTION` expectations), and compare
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

- **GIVEN** the engine built with the production catalog and all six rules active, the fixture-path suppression, and the checked-in baseline
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
