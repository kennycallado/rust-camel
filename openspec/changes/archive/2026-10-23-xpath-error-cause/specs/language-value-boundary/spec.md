## MODIFIED Requirements

### Requirement: Evaluation errors carry typed diagnostics

A language evaluation error SHALL carry: the language name, the route id, the
step id or verb plus index, the script position (`line:col` where the Language
reports one), and an error class. The class set SHALL include at least:
`runtime`, `arithmetic`, `type-mismatch`, `function-not-found`, `limit`,
`timeout`, `conversion`, `parse`. The xpath language SHALL classify its
failure paths as: input-size failure → `limit`, body coercion failure →
`conversion` (the `Conversion` class covers result conversion and input-body
coercion), malformed XML body → `parse` (the `Parse` class covers expression
and document sources), genuine evaluation failure → `runtime`.
When a language error carries a contractually redacted payload
(`EvalFailure.detail`, or the type-name-only `TypeMismatch`/`ConversionError`
payloads), `to_expression_failed` SHALL carry that fixed detail as the
mapped `ExpressionFailed`'s `cause` instead of `None`, wrapped in a
`ProcessorErrorWithSource` carrier with explicit mapper provenance (a
private marker behind its `#[source]`), so the cause renders as
`Processor error: <detail>`. Mapper provenance SHALL be identifiable
exactly — by marker downcast (`is_expression_detail_cause`), never by
variant or string shape.
Free-form payloads (`EvalError`, `ParseError` reason) SHALL keep
`cause: None`. Newly generated language diagnostics SHALL never place the
raw body or provider error text into the cause; an original route error
chained by catch handling keeps its own provenance and is not re-sanitized.
`chain_predicate_error` SHALL replace the cause with the original route
error when it is absent or mapper-generated; any caller-supplied cause
chain arriving through `PredicateSource::Async` SHALL pass through
untouched.

#### Scenario: Error content is complete

- **GIVEN** a route `r1` whose step `s2` evaluates a rhai expression that
  raises `ErrorArithmetic` at a known position
- **WHEN** the step fails
- **THEN** the route error SHALL identify language `rhai`, route `r1`, step
  `s2` with its verb, the position, and class `arithmetic`

#### Scenario: xpath size failure classifies as limit with a fixed cause

- **GIVEN** an xpath expression with `max_input_bytes` configured below the
  body size
- **WHEN** the step evaluates against an oversized well-formed XML body
- **THEN** the mapped `ExpressionFailed` SHALL carry class `limit`, and its
  cause payload SHALL be exactly `input exceeds maximum allowed size`
  (rendered through the carrier as `Processor error: input exceeds maximum
  allowed size`)

#### Scenario: xpath body coercion classifies as conversion

- **GIVEN** an xpath expression and a body that is neither XML nor coercible
  to XML
- **WHEN** the step evaluates
- **THEN** the mapped `ExpressionFailed` SHALL carry class `conversion` and
  the fixed cause payload `body is not XML and cannot be coerced`, with no
  provider error text appended

#### Scenario: malformed XML is distinguishable from a size failure without logs

- **GIVEN** a configured xpath input limit `L` near 100 KB, a well-formed
  XML body of length greater than `L`, and a malformed body of length less
  than `L` (both roughly 100 KB)
- **WHEN** the same xpath configuration evaluates both bodies
- **THEN** the size-rejected failure SHALL carry class `limit` with cause
  payload `input exceeds maximum allowed size`, the malformed failure SHALL
  carry class `parse` with cause payload `body is not valid XML`, and the
  two SHALL be tellable apart by class and cause without enabling `RUST_LOG`

#### Scenario: genuine evaluation failure keeps the runtime class

- **GIVEN** an xpath expression that compiles but fails during sxd evaluation
- **WHEN** the step evaluates against a valid XML body
- **THEN** the mapped `ExpressionFailed` SHALL keep class `runtime`

#### Scenario: free-form error text keeps an empty cause

- **GIVEN** a language failure carried as a free-form `EvalError` payload
- **WHEN** `to_expression_failed` maps it
- **THEN** the `cause` SHALL remain `None`

#### Scenario: redacted body content never enters the cause

- **GIVEN** a body containing a sentinel string and any reclassified xpath
  failure path triggered on that body
- **WHEN** the mapped `ExpressionFailed` cause is rendered, including through
  `Error::source()` traversal
- **THEN** the sentinel SHALL NOT appear; only the fixed class payloads SHALL

#### Scenario: catch-predicate failure still chains the original route error

- **GIVEN** a doTry route whose doCatch `onWhen` xpath predicate fails to
  evaluate (malformed body)
- **WHEN** the try body raises the original route error and the predicate
  evaluation fails
- **THEN** the surfaced `ExpressionFailed` SHALL carry the predicate's class
  and its `cause` chain SHALL still render the original route error,
  reachable via `Error::source()` traversal

#### Scenario: caller-supplied cause chains pass through untouched

- **GIVEN** a `PredicateSource::Async` closure that fails with an
  `ExpressionFailed` carrying its own non-synthetic cause chain — including
  a hand-built `ProcessorError` or `ProcessorErrorWithSource` without mapper
  provenance
- **WHEN** doTry chains the original route error
- **THEN** the caller-supplied cause SHALL remain unchanged
