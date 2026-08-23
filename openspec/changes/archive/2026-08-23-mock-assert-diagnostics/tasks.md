# Tasks: mock-assert-diagnostics

## Task 1.1: received-state diagnostics in MockAssertionError

**Files:**

- `crates/components/camel-mock/src/assert.rs` (enum payloads, `Display`,
  `evaluate_expectations` construction sites, `#[cfg(test)]` tests)

**Steps:**

1. Write the tests listed below first; run `cargo test -p camel-mock` and
   confirm each new test fails (payloads/clause absent).
2. Extend `MockAssertionError` payloads: `BodyNotFound` gains
   `received_count: usize`; `HeaderNotFound` and `HeaderRegexNotMatched`
   gain `received_count: usize`, `actual_values: Vec<String>`,
   `last_headers: Option<String>`. Doc-comment each field (caps: 8 entries,
   `+N more` suffix; `last_headers` is `None` when no exchange received).
3. Compute the diagnostic fields at the construction sites inside
   `evaluate_expectations` from the received-exchange snapshot. Shared
   helper for the two header variants is preferred over duplication.
4. Extend `Display`: append the received-state clause per the delta spec
   (zero-exchanges / actual-values / absent-key-with-last-headers forms).
   Keep `BodyMismatch`-style phrasing consistent (STE, no prose drift).
5. Verify the panicking surface formats the same error value (parity by
   construction); adjust any existing Display-string tests to the new
   expected text.
6. Run `cargo fmt --check` and `cargo clippy -p camel-mock -- -D warnings`
   in the worktree; fix findings.

**Tests (write first, name/arrange/act/assert):**

1. `header_not_found_zero_exchanges_message_contains_received_0` —
   arrange: endpoint with `expect_header("k","v")`, no sends; act:
   `try_assert_satisfied().await`; assert: `Err` Display contains
   "received 0 exchanges".
2. `header_not_found_wrong_values_message_contains_actual_values` —
   arrange: `expect_header("k","expected")`, send 2 exchanges with `k` =
   "actual-1"/"actual-2"; act: evaluate; assert: Display contains
   "received 2 exchanges", "actual-1", "actual-2".
3. `header_not_found_absent_key_message_contains_last_exchange_headers` —
   arrange: `expect_header("k","v")`, send 1 exchange with headers
   `a`,`b` only; act: evaluate; assert: Display reports the key absent and
   contains `a` and `b`.
4. `body_not_found_message_contains_received_count` — arrange: any-order
   `expect_body("x")`, send 1 exchange with a different body; act:
   evaluate; assert: Display contains "received 1" and the expected body.
5. `header_regex_not_matched_message_contains_actual_values` — arrange:
   `expect_header_regex("k","^pre")`, send 1 exchange with `k`="other";
   act: evaluate; assert: Display contains "other".
6. `diagnostic_lists_cap_at_eight_entries` — arrange:
   `expect_header("k","v")`, send 10 exchanges with `k` under 10 distinct
   values; act: evaluate; assert: Display lists at most 8 values and
   contains "+2 more".

**Acceptance Criteria:**

- All six tests pass; existing camel-mock tests pass (updated Display
  expectations where the message text changed).
- `cargo test -p camel-mock` green; `cargo check -p camel-test -p
  camel-cli` green (no external constructor/match of the variants).
- `cargo fmt --check` and `cargo clippy -p camel-mock -- -D warnings`
  green.
- Error taxonomy unchanged: one variant per assertion branch; latch and
  `InvalidHeaderPattern` bypass behavior untouched.

- [x] 1.1
