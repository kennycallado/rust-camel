# Tasks: expects-matcher-growth

## Phase 1: one algebra, two projections

### Task 1.1 — camel-mock delegates string/json verbs to camel-matchers

**Files:**
- `crates/components/camel-mock/Cargo.toml` (modified — add `camel-matchers = { workspace = true }`)
- `crates/components/camel-mock/src/matcher.rs` (modified)
- `crates/components/camel-mock/src/matcher_tests.rs` or existing test module (modified — inspect where matcher unit tests live first; if tests are inline `#[cfg(test)]`, extend there)

**Steps:**
1. Add the `camel-matchers` workspace dependency to camel-mock's Cargo.toml.
2. In `matcher.rs`, add the text projection: `fn text_only(body: &Body) -> Option<serde_json::Value>` — `Body::Text(s) => Some(Value::String(s.clone()))`, every other variant `=> None`.
3. Rewrite `BodyMatcher::matches` arms for `Regex`, `Contains`, `StartsWith`, `EndsWith` to: project `text_only(actual)`; `None => false` (fail closed, byte-identical to today's non-text `_ => false`); `Some(v) => camel_matchers::expectation_matches(&<this matcher as core Expectation>, &v)` where the arm maps to `Expectation::Regex/Contains/StartsWith/EndsWith(pattern.clone())`.
4. Rewrite the `JsonSubset` arm: keep the `pattern.is_object()` guard FIRST (non-object pattern => false, exactly today's behavior), then `json_value(actual)` projection (`None => false`), then delegate to `camel_matchers::expectation_matches(&Expectation::JsonSubset(pattern.clone()), &received)`.
5. DELETE the local `json_subset` function (and its test, moving the equivalent coverage to a delegation-equivalence test).
6. Leave `Equals` (`body_eq`) and `Exists` (`!Body::Empty`) arms untouched — observation-typed evaluation stays.
7. Leave `mismatch_note` and `Display` untouched (byte-identical diagnostics).
8. Add/extend unit tests per the specs below; run `cargo test -p camel-component-mock` and confirm the pre-existing suite is green with zero expectation-text edits.
9. `cargo fmt`; `cargo clippy -p camel-component-mock --all-targets -- -D warnings`.

**Tests:**
- `string_verbs_delegate_through_text_projection` — setup: matcher `Regex("^order-[0-9]+$")`; action: `matches(&Body::Text("order-42"))`; assert: true, and `camel_matchers::expectation_matches(&Expectation::Regex(same), &Value::String("order-42".into()))` returns the same verdict; also `Contains("total")` over a `Body::Json` body. command: `cargo test -p camel-component-mock`. expected: pass after implementation.
- `non_text_bodies_fail_closed_for_string_verbs` — setup: `Contains("x")`, `StartsWith("x")`, `EndsWith("x")`, `Regex("x")` vs `Body::Json`, `Body::Bytes(..)`, `Body::Empty` as applicable to the enum's real variants; action: `matches(...)`; assert: all false; `mismatch_note` still reports "body is not text" where it did before. same command.
- `json_subset_local_duplicate_deleted` — action: `grep -n "fn json_subset" crates/components/camel-mock/src/matcher.rs`; assert: zero hits.
- `json_subset_delegation_preserves_verdicts` — setup: pattern `{"status":"ok","meta":{"seq":3}}` vs a JSON superset body and vs a non-matching body; action: `matches`; assert: pass/fail identical to pre-change semantics (superset passes, non-match fails); a scalar pattern (`JsonSubset(5)`) fails regardless of body. same command.

**Acceptance:**
- `cargo test -p camel-component-mock` green, zero test-expectation edits outside the moved json_subset coverage.
- `cargo clippy -p camel-component-mock --all-targets -- -D warnings` exit 0.
- `cargo xtask lint-component-deps` exit 0 (new mock→camel-matchers edge accepted; if the gate rejects it, STOP and report `BLOCKED: lint-component-deps rejects the edge` — do not weaken the gate).

- [x] 1.1

### Task 1.2 — bounds completion: CountBound on mock + maxCount grammar

**Files:**
- `crates/components/camel-mock/src/inner.rs` (modified — setters)
- `crates/components/camel-mock/src/expectations.rs` (modified — count state)
- `crates/components/camel-mock/src/assert.rs` (modified — evaluation + error variants)
- `crates/camel-cli/src/commands/test/document.rs` (modified)
- `crates/camel-cli/src/commands/test/runner.rs` (modified — `set_expectations` mapping)
- `crates/camel-cli/src/commands/test/document_tests/parsing.rs` (modified — new parse-error cases)
- `crates/camel-cli/src/commands/test/driver_tests.rs` (modified — end-to-end bound scenarios)

**Steps:**
1. In `expectations.rs`: replace the count state (`expected_count`/`minimum_count`) with `count_bound: Option<camel_matchers::CountBound>`; setters in `inner.rs` become `pub fn expect_bound(&self, bound: CountBound)` plus `expect_maximum_count(n)` (AtMost sugar); keep `expect_count`/`expect_minimum_count` as Exact/AtLeast sugar over the same state.
2. In `assert.rs`: evaluate via `camel_matchers::bound_holds(bound, actual)` on the current snapshot (the runner settles before asserting — no polling). Failure text composes `camel_matchers::render_bound(bound)` with the existing suffix shape: Exact/AtLeast failures keep the current variants and bytes ("expected N exchanges, got M"); at-most renders "expected at most {n} exchanges, got {m}"; range "expected between {min} and {max} exchanges, got {m}". New `MockAssertionError` variants as needed (the enum is `#[non_exhaustive]`).
3. camel-cli `ExpectSet` (document.rs :262): add `pub max_count: Option<usize>` (serde camelCase `maxCount` via the struct's existing rename); extend validation with new error variants in the family's verb shape: `count`+`maxCount` => "expects entry `{endpoint}` must not set both count and maxCount" (exit 2, mirroring the existing count+minCount variant); `minCount > maxCount` (both set) => "expects entry `{endpoint}` must not set minCount {a} above maxCount {b}" (exit 2).
4. runner.rs `set_expectations`: map `min_count`+`max_count` => `expect_bound(Range(min,max))`; `max_count` alone => `expect_maximum_count(n)`; existing mappings unchanged.
5. Tests per specs below; `cargo test -p camel-cli` green; `cargo fmt`; `cargo clippy -p camel-cli -- -D warnings`.

**Tests:**
- `range_bound_passes_inside` (driver-style or mock-unit per existing house pattern) — Given route emitting 2 exchanges, `expects: {mock:r: {minCount: 1, maxCount: 2}}` — evaluation passes. command: `cargo test -p camel-cli`.
- `range_bound_fails_above` — same but 3 exchanges — FAIL with text containing "expected at most 2 exchanges, got 3" (or the range form — pin the exact chosen wording in the test).
- `max_count_zero_asserts_absence` — Given `maxCount: 0` and no arrivals after settle — PASS; a same-window arrival — FAIL with at-most text.
- `count_with_maxcount_rejected` (parsing test) — `{count: 1, maxCount: 2}` => exit-2 parse error stating mutual exclusion.
- `min_exceeds_max_rejected` (parsing test) — `{minCount: 3, maxCount: 2}` => exit-2 parse error stating the range is empty / min exceeds max.

**Acceptance:**
- `cargo test -p camel-cli` green; `cargo test -p camel-component-mock` green.
- `cargo clippy -p camel-cli -- -D warnings` and `-p camel-component-mock --all-targets -- -D warnings` exit 0.
- Existing `.test.yaml` fixtures untouched and passing.

- [x] 1.2

### Task 1.3 — camel-test re-exports + ADR-0072 dated amendment + docs

**Files:**
- `crates/camel-test/src/lib.rs` (modified — re-exports)
- `docs/adr/0072-test-pyramid-v2.md` (modified — amendment note)
- `crates/components/camel-mock/CONTEXT.md` (modified, if the crate has one — inspect first; else skip)
- `crates/camel-test/CONTEXT.md` (modified — inspect first; else skip)

**Steps:**
1. camel-test lib.rs: `pub use camel_matchers::{Expectation, CountBound};` following the crate's existing `pub use` style; add the `camel-matchers` workspace dep if not present.
2. ADR-0072: append a dated amendment note section (match ADR-0050's amendment-note precedent) — "Amendment (2026-09-07): the Context's statement that the unit tier's `expects` is endpoint-to-count only overstated the gap; camel-mock already carried the full seven-key matcher vocabulary over `Body` (mirrored from the mock-testkit rules), and the scenario tier's grammar mirrored the same keys. The real defects were the duplicated ad-hoc algebra and the missing upper bounds. Step 2 (landed as this change) delegates the mock's string/json verbs to the shared core through the `text_only`/`json_value` projections — the worked example of per-tier observation this ADR prescribes." Decision sections unchanged.
3. Update the crate CONTEXT.md files if they exist, citing the shared algebra (keep it minimal).
4. `cargo xtask lint-context-citations` exit 0.

**Tests:**
- `kit_reexports_shared_types` — setup: a `dev-dependency`-style check or compile assertion: a tiny `#[test]` in camel-test asserting the re-export path resolves: `let _: Option<camel_matchers::Expectation> = None;` via the kit's own namespace (`crate::Expectation`). command: `cargo test -p camel-test`.
- `adr_amendment_present` — `grep -c "Amendment 2026-09-07" docs/adr/0072-test-pyramid-v2.md` >= 1 (ADR-0050 precedent format: `## Amendment 2026-08-09 — …`, no parentheses).

**Acceptance:**
- `cargo test -p camel-test` green; lint-context-citations exit 0; ADR amendment present.

- [x] 1.3
