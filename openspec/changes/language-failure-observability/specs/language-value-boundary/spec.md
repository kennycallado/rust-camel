## MODIFIED Requirements

### Requirement: Evaluation failure metrics

Each language evaluation failure SHALL increment a counter metric labeled with
at least `{language, route, verb, class}`, per the project's metric label
rules. The counter SHALL be named `language_eval_failures_total` (exported as
`camel_language_eval_failures_total`) and SHALL increment exactly once per
top-level `CamelError::ExpressionFailed` surfaced by the route step loop. The
`class` label value SHALL come from the typed `ExpressionErrorClass` stable
name (`runtime`, `arithmetic`, `type-mismatch`, `function-not-found`, `limit`,
`timeout`, `conversion`, `parse`), never from message text. The `route` label
SHALL carry the route id. Label values SHALL be limited to the language, the
route id, the verb and the class; the cause, the conversion detail, the
position and any exchange data SHALL NOT be used as label values. A failure
that a route error handler recovers after the step failed SHALL still be
counted, because the evaluation failed. Retries performed inside the route
error handler SHALL NOT increment the counter again: one exhausted retry
sequence counts once. An `ExpressionFailed` wrapped inside
another error's cause chain SHALL NOT be counted. An `ExpressionFailed`
raised by the error handler itself (a catch or onWhen predicate failure) is
outside this requirement and SHALL NOT be counted. Emission SHALL use the
existing `MetricsCollector` counter surface and SHALL NOT require a new trait
method, so every existing collector implementation keeps compiling. The
counter SHALL NOT be gated by a metric-family lever, matching the
non-disableable error family.

#### Scenario: Metric records class and verb

- **GIVEN** a route whose `set_property` rhai expression fails with class
  `arithmetic`
- **WHEN** the step fails
- **THEN** the evaluation-failure metric SHALL increment with labels language
  `rhai`, the route id, verb `set_property`, class `arithmetic`

#### Scenario: Handler-recovered failure still counts

- **GIVEN** a route whose language step fails and whose error handler handles
  the failure so the exchange continues
- **WHEN** the step fails
- **THEN** the evaluation-failure metric SHALL increment once for that step

#### Scenario: Segment predicate failure is counted

- **GIVEN** a `filter` step whose predicate language fails to evaluate
- **WHEN** the segment step fails
- **THEN** the evaluation-failure metric SHALL increment with the filter verb
  and the typed class, exactly once

#### Scenario: Retry does not double count

- **GIVEN** a route with a retry policy whose language step fails on every
  attempt
- **WHEN** the retry sequence is exhausted
- **THEN** the evaluation-failure metric SHALL increment once, not once per
  attempt

#### Scenario: Label values exclude exchange data

- **GIVEN** a language expression that fails while operating on a body
  containing a sentinel string
- **WHEN** the metric is emitted
- **THEN** no label value SHALL contain the sentinel, the cause text, the
  conversion detail, or the script position

### Requirement: Evaluation failure span attributes

The failing step's span SHALL record the error class and the script position as
span attributes. The class attribute SHALL be `camel.expression.class` carrying
the typed class's stable lowercase name. The position attribute SHALL be
`camel.expression.position` carrying the `line:column` rendering of the
1-based script position. The position attribute SHALL be absent when the
language reports no position; no empty or zero placeholder SHALL be written.
The local `tracing` step span SHALL expose the same values through the
`expression_class` and `expression_position` fields. The attributes SHALL ride
the existing step span; no parallel span tree SHALL be created.

#### Scenario: Span carries class and position

- **GIVEN** tracing enabled and a step whose expression fails at line 3,
  column 8
- **WHEN** the step fails
- **THEN** the step span SHALL carry the class and position `3:8` as
  attributes

#### Scenario: No position attribute when the language reports none

- **GIVEN** tracing enabled and a step whose language error carries no position
- **WHEN** the step fails
- **THEN** the step span SHALL carry the class attribute and SHALL NOT carry a
  position attribute

#### Scenario: Segment step span carries the attributes

- **GIVEN** tracing enabled and a `filter` predicate that fails at a known
  position
- **WHEN** the segment step fails
- **THEN** the segment step span SHALL carry the class and position attributes

### Requirement: camel test shows typed evaluation errors

`camel test` output for a failed exchange SHALL show the full typed evaluation
error: class, position, and step identification, not a generic failure line. A
propagated evaluation failure SHALL render through the typed `CamelError`
`Display`, naming the class, language, verb, step id, route id and position
when the language reports one. The change
SHALL NOT alter assertion verdicts or the exit-code precedence. Rendered output
SHALL NOT contain exchange data.

#### Scenario: Test output names the failing expression

- **GIVEN** a test route whose `filter` expression fails at a known position
- **WHEN** `camel test` runs the route
- **THEN** the failure output SHALL include the class, the position, and the
  step id

#### Scenario: Output stays redacted

- **GIVEN** a failing language expression whose input body contains a sentinel
  string
- **WHEN** `camel test` renders the failure
- **THEN** the rendered output SHALL NOT contain the sentinel
