# language-value-boundary Delta

## MODIFIED Requirements

### Requirement: Predicate results are strictly boolean

A predicate result SHALL be a boolean. A non-boolean result SHALL be a
`type-mismatch` error. Truthiness coercion SHALL NOT be applied (owner
decision 2026-10-02: strict, no staging).

JSONPath predicates SHALL additionally support an explicit `predicate_mode`
with values `strict` and `matches`; `strict` SHALL be the default and SHALL
behave exactly as above. In `matches` mode, evaluation SHALL inspect the raw
match sequence before value-source normalization: zero matches (including a
missing path) SHALL produce `false`; exactly one boolean match SHALL produce
that boolean (a literal `false` SHALL stay `false`); every other non-empty
match sequence SHALL produce `true`. `matches` mode SHALL NOT import XPath
`boolean()` value rules: an empty string and `0` SHALL produce `true`.
JSONPath value-source (expression) typing SHALL remain unchanged in both
modes, and XPath behavior SHALL remain unchanged. Evaluation failures SHALL
remain typed errors in every mode and SHALL NOT be coerced to `false`.
Error messages SHALL name the expression and the value kind only, never
payload values.

#### Scenario: Non-boolean predicate result fails

- **GIVEN** a `filter` predicate whose script returns the string `"false"`
- **WHEN** the predicate evaluates
- **THEN** the step SHALL fail with a `type-mismatch` error; the string SHALL
  NOT be coerced to a boolean

#### Scenario: Explicit JSONPath matches mode coerces by cardinality

- **GIVEN** a JSONPath predicate created with `predicate_mode: matches`
- **WHEN** it evaluates a query returning, in turn: an object match, a
  multi-match sequence, a single empty string, the string `"false"`, the
  number `0`, an existing `null` value, a missing path, and a literal
  `false` boolean
- **THEN** the result SHALL be `true` for every non-empty match sequence
  (including the empty string, `"false"`, `0`, and the existing `null`),
  `false` for the missing path, and `false` for the sole literal `false`
  boolean

#### Scenario: Matches mode parity with XPath boolean()

- **GIVEN** equivalent JSON and XML fixtures with zero, one, and multiple
  item matches
- **WHEN** a `matches`-mode JSONPath predicate and an XPath
  `boolean(/order/items/item[price>10])` predicate evaluate the same fixture
  set
- **THEN** both SHALL return the same boolean for every fixture, including
  a sole `false` boolean equal to `boolean(false())`
