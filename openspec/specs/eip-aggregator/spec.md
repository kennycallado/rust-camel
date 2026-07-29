# eip-aggregator Specification

## Purpose
TBD - created by archiving change aggregator-adr0046-divergences. Update Purpose after archive.
## Requirements
### Requirement: Aggregator divergence documentation in tracked CONTEXT.md

The system SHALL document every forced divergence between the rust-camel
Aggregator EIP and Apache Camel as tracked content in
`crates/camel-processor/CONTEXT.md`, per ADR-0046 §Anti-pattern §5 (no
gitignored spike artifacts). Each documented divergence SHALL name (a) the
divergence statement, (b) the forcing ADR or contract shape, and (c) the
observable consequence for an operator migrating from Apache Camel.

#### Scenario: D-A1 binary-fold strategy contract documented

- **GIVEN** the spike classified D-A1 (Camel `aggregate(oldExchange, newExchange)`
  passes null on the first bucket message; rust-camel `AggregationFn =
  Fn(Exchange, Exchange) -> Exchange` always receives two exchanges, first
  message sits untouched in the bucket)
- **WHEN** a contributor reads `crates/camel-processor/CONTEXT.md`
- **THEN** they find a sub-section stating the binary-fold contract, the
  no-null forcing (Rust discipline + bucket model), and that strategies
  needing initialize-on-first must handle the first message via the bucket
  state rather than a null-oldExchange branch

#### Scenario: D-A2 strategy-cannot-signal-failure documented

- **GIVEN** the spike classified D-A2 (Camel strategy can throw; rust-camel
  `AggregationFn -> Exchange` returns no `Result`, so a strategy cannot
  signal failure except by panic; `aggregate() -> Result` covers only
  internal body-extraction errors)
- **WHEN** a contributor reads the CONTEXT.md divergence section
- **THEN** they find a sub-section stating the no-Result return, that this is
  the D2-family for the Aggregate EIP specifically (distinct from the
  Splitter `Vec<Result>` D2), and the consequence (error-aware strategies
  cannot be expressed in the return type)

#### Scenario: D-A3 force-completion channel path documented

- **GIVEN** the spike classified D-A3 (Camel flows force-completed buckets
  synchronously through the downstream pipeline on `context.stop()`;
  rust-camel emits via a bounded `late_tx` mpsc channel drained into
  `post_pipeline`; boolean semantics are EQUAL but under late-channel-full
  pressure force-completed exchanges are dropped with a warn log)
- **WHEN** a contributor reads the CONTEXT.md divergence section
- **THEN** they find a sub-section stating the channel-mediated path, the
  drop-under-pressure edge (with the source line range), and that this is
  forced by Tower shutdown lifecycle + bounded-channel DoS protection

#### Scenario: D-A4 timeout mechanism and knob divergence documented

- **GIVEN** the spike classified D-A4 (Camel uses a single
  completion-timeout-checker thread polling every
  `completionTimeoutCheckerInterval`; rust-camel uses per-bucket
  `spawn_timeout_task` + `bucket_ttl` sweep + `max_timeout_tasks` cap;
  observable completion is EQUAL, mechanism and exposed knobs differ)
- **WHEN** a contributor reads the CONTEXT.md divergence section
- **THEN** they find a sub-section stating the per-bucket-task mechanism, the
  knob mapping (rust-camel `max_timeout_tasks`/`bucket_ttl` ↔ Camel
  `completionTimeoutCheckerInterval`), and the graceful-degradation behavior
  when the task cap is reached

#### Scenario: D-A5 mandatory memory bounds documented

- **GIVEN** the spike classified D-A5 (Camel default in-memory repository is
  unbounded; rust-camel `AggregatorConfig::validate()` rejects unbounded
  configs with `ConfigValidationError::AggregatorMissingMemoryBound` /
  `AggregatorTimeoutRequiresTtl` per ADR-0033; builder defaults
  `max_buckets=10_000` + `bucket_ttl=300s`)
- **WHEN** a contributor reads the CONTEXT.md divergence section
- **THEN** they find a sub-section stating the mandatory-bound rule, the
  forcing ADRs (0033 security defaults, 0044 admission), and the operator
  consequence (a config valid in Camel may be rejected here)

### Requirement: Native tests pin rust-camel Aggregator semantics

The system SHALL include native tests that pin rust-camel's Aggregator
semantics for the divergences where behavior is testable, asserting what
rust-camel DOES (not what Apache Camel does), per ADR-0046 §Anti-pattern §2.

#### Scenario: D-A1 semantic pin — first message preserved unchanged in bucket

- **GIVEN** an `AggregationFn` custom strategy and a bucket receiving its
  first exchange
- **WHEN** the bucket later completes (e.g. on size 2) and the strategy runs
- **THEN** the strategy receives `(first_exchange, second_exchange)` with
  both non-null, and the first exchange's body is the value pushed (not
  transformed by a null-oldExchange branch)

#### Scenario: D-A2 semantic pin — strategy return type is Exchange not Result

- **GIVEN** the `AggregationFn` type alias (`Arc<dyn Fn(Exchange, Exchange) -> Exchange + Send + Sync>`)
- **WHEN** a contributor attempts to write a strategy that signals failure via the return type
- **THEN** a compile-time type witness confirms the contract: a function with
  signature `Fn(Exchange, Exchange) -> Result<Exchange, CamelError>` does NOT
  type-check as `AggregationFn` (documented via a `compile_fail` doc-test or
  an explicit type-alias assertion), proving strategy-signaled failure is not
  expressible in the return type

#### Scenario: D-A3 semantic pin — force-completion emits via late channel and drops under pressure

- **GIVEN** an aggregator with `force_completion_on_stop(true)` and a saturated
  late channel (capacity filled before `force_complete_all()` runs)
- **WHEN** `force_complete_all()` attempts to emit additional completed buckets
- **THEN** the overflow exchange is dropped — observable via the late-channel
  receiver seeing fewer items than `force_complete_all()` attempted to emit,
  with a warn log recorded — rather than blocking or panicking. The test
  asserts this locally-observable channel-saturation/drop behavior (it aligns
  with the existing `test_late_channel_full_drops_with_warning`); downstream
  post-pipeline routing is verified by route-controller integration elsewhere
  and is out of scope for this inline unit test

#### Scenario: D-A5 semantic pin — validate rejects unbounded config

- **GIVEN** an `AggregatorConfig` with no `max_buckets`, no `Timeout`
  completion, and no `bucket_ttl`
- **WHEN** `validate()` is called
- **THEN** it returns `Err(CamelError::ConfigValidation(
  ConfigValidationError::AggregatorMissingMemoryBound))`

### Requirement: Gap-coverage note for completionSize-as-expression

The system SHALL document as a known coverage gap (not a forced divergence)
that rust-camel's `CompletionCondition::Size(usize)` is static, whereas
Apache Camel supports `completionSize(expression)` evaluated per exchange.

#### Scenario: G-A1 gap note present

- **GIVEN** the spike identified G-A1 as a coverage gap (no ADR forbids it)
- **WHEN** a contributor reads the CONTEXT.md divergence section
- **THEN** they find a note labeled gap-coverage stating the missing
  expression-based completion-size surface, distinct from the forced
  divergences above

