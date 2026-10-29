## ADDED Requirements

### Requirement: XPath number projection refuses non-finite values

An XPath expression result of `+Inf`, `-Inf`, or `NaN` SHALL fail the step
with a typed `EvalFailure` carrying class `arithmetic` and a payload-free
detail naming the non-finite class. The conversion SHALL NOT emit
`JsonValue::Null` for a non-finite number and SHALL NOT stringify the value.

#### Scenario: Positive infinity from division is refused

- **GIVEN** an XML body and the XPath expression `1 div 0`
- **WHEN** the expression is evaluated
- **THEN** the result is an `EvalFailure` with class `arithmetic` whose
  detail names `+Inf`, and no `JsonValue::Null` is produced

#### Scenario: Negative infinity from division is refused

- **GIVEN** an XML body and the XPath expression `-1 div 0`
- **WHEN** the expression is evaluated
- **THEN** the result is an `EvalFailure` with class `arithmetic` whose
  detail names `-Inf`

#### Scenario: NaN is refused

- **GIVEN** an XML body and the XPath expression `0 div 0`
- **WHEN** the expression is evaluated
- **THEN** the result is an `EvalFailure` with class `arithmetic` whose
  detail names `NaN`

#### Scenario: Predicate verb refuses a non-finite result

- **GIVEN** an XML body and an XPath predicate whose result is `1 div 0`
- **WHEN** the predicate is matched
- **THEN** the match fails with the same typed `arithmetic` refusal and no
  `false` is returned in its place

### Requirement: XPath number projection preserves integral fidelity

Finite XPath numbers SHALL project to JSON preserving their integral form:
an integral value exactly representable as a 64-bit integer SHALL project to
JSON integer form (never a degraded float form), using exactness-checked
intervals only; fractional values and integral values outside the exact
64-bit integer intervals SHALL project to float form. The rule SHALL be
documented next to the conversion. No lossy integer cast is permitted.

#### Scenario: count yields integer form

- **GIVEN** an XML body with two matching nodes and the XPath expression
  `count(/root/item)`
- **WHEN** the expression is evaluated
- **THEN** the result is the JSON number `2` in integer form

#### Scenario: Fractional value stays float form

- **GIVEN** an XPath expression whose result is `2.5`
- **WHEN** the expression is evaluated
- **THEN** the result is the JSON number `2.5` in float form

#### Scenario: Integral value outside the exact 64-bit intervals stays float form

- **GIVEN** an XPath expression whose result is an integral value of
  magnitude `2^64` or larger (outside both the exact signed and unsigned
  64-bit intervals)
- **WHEN** the expression is evaluated
- **THEN** the result is the JSON number in float form with the same
  mathematical value, never a saturated or truncated integer

### Requirement: XPath boundary regression kit pins the audit findings

The XPath language crate SHALL carry a deterministic, bounded regression kit:
a depth-bounded roundtrip property test over the conversion surface, and a
per-verb conformance harness seeded from the language-boundary audit
repros R1, R2, R4, and R5 where they apply to XPath. The kit SHALL NOT
introduce new dependencies, sleeps, or unbounded inputs.

#### Scenario: Audit R2 refusal is pinned on both verbs

- **GIVEN** the conformance harness entry seeded from audit repro R2
- **WHEN** the harness runs the non-finite cases against the expression and
  predicate verbs
- **THEN** both verbs produce the typed `arithmetic` refusal

#### Scenario: Audit R1 predicate errors propagate

- **GIVEN** the conformance harness entry seeded from audit repro R1
- **WHEN** a predicate evaluation fails (for example, a body that is not
  parseable XML)
- **THEN** the predicate returns the typed error and not `false`

#### Scenario: Audit R4 strict bool has no truthiness

- **GIVEN** the conformance harness entry seeded from audit repro R4
- **WHEN** a predicate whose result is a number or string is matched
- **THEN** the result is a `TypeMismatch` error, never a truthiness coercion

#### Scenario: Audit R5 refusal text excludes document content

- **GIVEN** the conformance harness entry seeded from audit repro R5 with a
  body containing a sentinel string
- **WHEN** a non-finite or coercion refusal is produced
- **THEN** the error text does not contain the sentinel

#### Scenario: Property roundtrip stays bounded and deterministic

- **GIVEN** the depth-bounded property test with its fixed seed, fixed
  iteration count, and bounded generator
- **WHEN** the property test runs
- **THEN** finite values convert successfully and roundtrip, non-finite
  values are refused, and generated XML content roundtrips through
  document, nodeset, and string-value forms unchanged

## MODIFIED Requirements

### Requirement: Unrepresentable values are refused

Conversion from a Language value to an engine value SHALL be fallible. Values
the engine cannot represent — not-a-number, positive or negative infinity,
function pointers, closures, timestamps, and custom types — SHALL yield a
typed `conversion` error that names the source type and the target (property
name, header name, or body). A `to_string()` fallback SHALL NOT be used as a
conversion.

Exception (XPath number projection): where the unrepresentable value is a
non-finite XPath number (`+Inf`, `-Inf`, `NaN`) produced by an XPath
expression or predicate, the refusal SHALL instead use the `arithmetic` class
with a payload-free detail naming the non-finite class, per the "XPath number
projection refuses non-finite values" requirement. Every other refusal
follows the `conversion` rule above.

#### Scenario: NaN property is refused

- **GIVEN** a rhai script step whose result is `0.0/0.0`
- **WHEN** the engine converts the result into a property
- **THEN** the step SHALL fail with a `conversion` error naming the type and
  target; the property SHALL NOT hold `null` or a string

#### Scenario: No debug-string fallback

- **GIVEN** a rhai value of a custom or function type returned at the boundary
- **WHEN** the engine converts it
- **THEN** the engine SHALL NOT store the value's `Display` or `Debug` text
