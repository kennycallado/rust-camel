# Proposal: mock-assert-diagnostics

## Why

`MockAssertionError` failure messages print only the expected side. The
received side is absent, so a failing expectation cannot distinguish "the
header was never set on the exchange" from "the exchange never arrived".
This hits multi-route `camel test` documents hardest, where inputs traverse
several routes before reaching the mock endpoint: debugging one failed
header expectation takes tens of minutes of guesswork.

Reported during demo prep (2026-08-22). bd issue: rc-5lp5.

## What Changes

- `BodyNotFound` gains `received_count` (number of received exchanges at
  evaluation). `Display` appends `(received N exchanges)`.
- `HeaderNotFound` and `HeaderRegexNotMatched` gain `received_count`,
  `actual_values` (`{:?}`-formatted values of the expected key across
  received exchanges that carry it, capped at 8 with `+N more`), and
  `last_headers` (pre-formatted key list of the last received exchange,
  capped at 8, `None` when no exchanges arrived).
- `Display` for both header variants appends a received-state clause:
  `received 0 exchanges` when nothing arrived; the key's actual values when
  present under other values; the last exchange's header keys when the key
  is absent everywhere but exchanges arrived.
- The error taxonomy is unchanged: one variant per assertion branch. The
  `Display`-equals-panic-message parity is preserved (the panicking
  surface formats the same error value).

## Impact

- Code: `crates/components/camel-mock/src/assert.rs` (enum payloads,
  `Display`, construction sites inside `evaluate_expectations`, tests).
- Spec: `mock-component-correctness` requirement "non-panicking assertion
  surface" payload lists gain the diagnostic fields; new scenarios for the
  three received-state clauses.
- No consumers outside `camel-mock` name these variants (`#[non_exhaustive]`
  enum, surfaced via `Display`), so the payload additions break no caller.
- Related, not duplicated: rc-3kwt (matcher expressiveness), rc-3g4f
  (any_order pairing semantics).
