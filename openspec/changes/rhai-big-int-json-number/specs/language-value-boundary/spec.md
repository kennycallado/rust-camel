# Delta specs: rhai-big-int-json-number

Scope: **rhai only**. Other languages keep refusing JSON integers above
`i64::MAX`; no scenario here applies to them.

## MODIFIED Requirements

### Requirement: Structured values round-trip natively

For every value `v` composed of strings, booleans, integers, finite floats,
unit/null, arrays, and objects (recursively), storing `v` through the boundary
and reading it back in a later evaluation of the same Language SHALL return an
equal value with its type preserved. The same SHALL hold for converting any
JSON value into the Language and back, except JSON integers greater than
`i64::MAX`, which the inbound conversion SHALL refuse with a typed error
(owner decision 2026-10-02, sealed Q4: no lossy float conversion) — in every
language except rhai, whose refinement follows.

Rhai refinement (bd rc-141om; refines owner decision 2026-10-02, sealed Q4 —
it does not reverse it): a JSON integer in `(i64::MAX, u64::MAX]` SHALL bind
on every rhai inbound path (body, headers, properties, nested) as the
`json number` wrapper carrying its exact decimal token. The inbound conversion
SHALL NOT refuse such an integer, SHALL NOT convert it to float, and SHALL NOT
convert it to string. A header or property the script never reads SHALL NOT
cause a failure on any eval path (`snapshot`, `snapshot_maps`,
`eval_mut_sync`). The explicit lossy path SHALL remain `x.to_float()`.
Comparisons against `i64`, `f64`, `String`, or `bool` SHALL keep refusing with
the `type-mismatch` class; no implicit coercion SHALL exist.

#### Scenario: Map survives a property round-trip

- **GIVEN** a rhai script step that stores `#{"a": 1, "b": [2, 3]}` into a
  property
- **WHEN** a later rhai script reads that property and reports its type
- **THEN** the later script SHALL see a map equal to the stored value, and the
  reported type SHALL be `map`

#### Scenario: Large unsigned integer binds as json number

- **GIVEN** an exchange property holding a JSON integer greater than
  `i64::MAX` and at most `u64::MAX`
- **WHEN** a rhai script reads that property
- **THEN** the value SHALL bind as `json number` (`type_of(x)` is
  `json number`), `x.to_string()` SHALL yield the exact decimal token, the
  evaluation SHALL NOT fail, and the engine SHALL NOT store a float or a
  string

#### Scenario: Untouched large header does not fail a later step

- **GIVEN** an exchange header holding `u64::MAX`
- **WHEN** a first rhai script step assigns another header and a second rhai
  script step assigns the body, neither step reading `big`
- **THEN** both steps SHALL succeed, the `big` header SHALL keep its original
  bit-identical `u64` value, and the assigned header SHALL hold the new value

#### Scenario: Comparison with a native scalar refuses

- **GIVEN** a `json number` header holding a value above `i64::MAX`
- **WHEN** a rhai script evaluates `header("big") == 1`
- **THEN** the evaluation SHALL fail with the `type-mismatch` class; no
  implicit coercion SHALL exist

### Requirement: Rhai host JSON helpers leave the exchange converter unchanged

`parse_json` and `to_json` SHALL be script-local host helpers. Their own
contracts SHALL NOT change: `to_json(parse_json(s))` keeps its semantic
round-trip and the rc-qka42 deferral stays (direct outbound conversion of a
wrapper other than the exact inbound image SHALL remain refused, so
persisting a parsed wrapper SHALL require `to_json` or a native leaf — except
a `json number` whose token is a plain decimal integer in canonical spelling
without leading zeros, in `(i64::MAX, u64::MAX]`, which this change permits
outbound). The exchange
converter SHALL change exactly as the rc-141om refinement requires: the
inbound conversion binds a JSON integer in `(i64::MAX, u64::MAX]` as the
`json number` wrapper with its exact decimal token (refining owner decision
2026-10-02, sealed Q4 — no lossy float), and the outbound
`dynamic_to_value` converter SHALL convert a `json number` back to
`Value::Number(u64)` only when its token is a plain decimal integer in
canonical spelling (no leading zeros) within
`(i64::MAX, u64::MAX]` — the exact inbound image, so untouched values
round-trip bit-identically. Every other `json number` SHALL refuse with the
stable label `json number`, and `JsonValue` SHALL refuse with `json value`.
The transaction diff SHALL compare `json number` values by token: an untouched
entry SHALL produce no delta and SHALL keep its original value handle.

#### Scenario: Exchange inbound behavior binds large integers as json number

- **GIVEN** an exchange property holding a JSON integer greater than
  `i64::MAX` and at most `u64::MAX`
- **WHEN** a rhai script reads that property
- **THEN** the value SHALL bind as `json number` with its exact decimal
  token, and the evaluation SHALL NOT fail

#### Scenario: Outbound refusal uses stable labels

- **GIVEN** a Rhai script whose expression result is a `JsonValue`, or a
  `JsonNumber` whose token is not a plain decimal integer in
  `(i64::MAX, u64::MAX]`
- **WHEN** the outbound conversion runs
- **THEN** it SHALL refuse with a conversion error whose source label is
  `json value` (respectively `json number`) and no Rust type path

#### Scenario: Explicit serialization yields a text body

- **GIVEN** a script that parses JSON and assigns `body = to_json(j)`
- **WHEN** the script runs
- **THEN** the body SHALL be a text body holding valid JSON

#### Scenario: Exact inbound image round-trips bit-identically

- **GIVEN** an exchange header holding a JSON integer greater than
  `i64::MAX` and at most `u64::MAX`, bound inbound as `json number`
- **WHEN** a mutating rhai script that never touches that header commits
- **THEN** the committed header SHALL hold the identical `u64` value with no
  delta applied to the entry

#### Scenario: Changed entry converts to the exact u64

- **GIVEN** an exchange property `big` holding a `json number` above
  `i64::MAX`
- **WHEN** a mutating rhai script copies that value into a new property with
  `properties.copy = property("big")`
- **THEN** the transaction SHALL convert the changed entry through the
  outbound converter, and the committed `copy` property SHALL hold the exact
  `Value::Number(u64)` image

#### Scenario: Untouched json number entry keeps its value handle

- **GIVEN** a mutating rhai script whose header map contains a `json number`
  entry and which mutates only a different entry of the same map
- **WHEN** the transaction diff compares the entry by token
- **THEN** the `json number` entry SHALL produce no delta and SHALL keep its
  original value handle
