# Proposal: jsonpath-predicate-matches-mode

## Why

`jsonpath` predicates reject any query result that is not a literal boolean:
`$.items[?@.price>10]` fails with `TypeMismatch 'object'` and `$.missing` with
`TypeMismatch 'null'` (bd rc-lmfjx). `xpath` reaches the same "there is at
least one match" intent through its core `boolean()` function, so the two
language surfaces disagree on the same route shape — and JSONPath (RFC 9535,
no function-call extension) has no escape hatch an XPath author could reach
for.

## What Changes

- Add `JsonPathPredicateMode` (`Strict` | `Matches`) to `JsonPathConfig`.
  `Strict` stays the default: the sealed strict-bool predicate behavior
  (owner decision 2026-10-02) is unchanged unless the operator opts in.
- `Matches` mode coerces the raw match sequence before value-source
  normalization: zero matches (missing path included) are `false`, a sole
  boolean match keeps its value, and every other non-empty sequence is
  `true`. It is cardinality-only — XPath `boolean()` value rules (empty
  string, `0`, NaN are false) are deliberately NOT imported.
- JSONPath value-source (expression) reads keep their current typing.
  XPath behavior is unchanged.
- Oracle ruling (e_gpt, 2026-10-24, mission 399): option (b) — explicit
  opt-in configuration, not automatic coercion in predicate position;
  automatic coercion (a) was rejected because it silently changes the
  meaning of equivalent filters when their match count changes and leaves
  singleton object matches broken with no escape.
- Parity with XPath `boolean()` is pinned by a table test
  (`camel-test/tests/jsonpath_xpath_boolean_parity_test.rs`).

## Impact

- `crates/languages/camel-language-jsonpath`: new public enum + config field,
  predicate branch, unit table tests.
- `crates/camel-test`: cross-language parity table test (both crates already
  dependencies).
- `openspec/specs/language-value-boundary/spec.md`: "Predicate results are
  strictly boolean" requirement is amended (delta in this change).
- `docs/src/languages/jsonpath.md`: predicate modes documented.
