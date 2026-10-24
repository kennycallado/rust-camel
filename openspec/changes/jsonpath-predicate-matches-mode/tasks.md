# Tasks

## 1. Implementation

- [x] 1.1 Add `JsonPathPredicateMode` (`Strict` default | `Matches`) and the
  `predicate_mode` field to `JsonPathConfig`; branch `JsonPathPredicate::matches`
  on the mode, coercing the raw match sequence in `Matches` (zero → false, sole
  boolean → its value, other non-empty → true) and keeping the strict path
  byte-for-byte
- [x] 1.2 Unit tests: full kind table for `Matches` (object, arrays, empty and
  non-empty strings, zero and non-zero numbers, booleans, existing `null`,
  missing path), strict-default repro cases stay `TypeMismatch`, evaluation
  failure stays a typed error in `Matches`
- [x] 1.3 Parity table test vs XPath `boolean()` in `camel-test` (zero, one,
  multiple matches; sole `false` boolean vs `boolean(false())`)

## 2. Docs and spec

- [x] 2.1 Update `docs/src/languages/jsonpath.md` with predicate modes
- [x] 2.2 Update `camel-language-jsonpath/CONTEXT.md` (predicate modes section,
  public-enum statement)
- [x] 2.3 Delta for `language-value-boundary` "Predicate results are strictly
  boolean" requirement
