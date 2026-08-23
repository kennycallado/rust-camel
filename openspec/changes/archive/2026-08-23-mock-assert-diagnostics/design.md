# Design: mock-assert-diagnostics

## Context

`evaluate_expectations` (`assert.rs:191`) holds the received-exchange
snapshot when it constructs the error variants. All diagnostic data is
therefore available at construction time; no sampling changes.

## Goals and Non-Goals

- Goal: every failing header/body expectation reports what was received,
  bounded in size.
- Non-goal: pairing semantics between header expectations and their body's
  exchange (rc-3g4f); richer matchers (rc-3kwt); body-content dumps in
  `BodyNotFound` (the count already separates "never arrived").

## Decisions

- **Fields, not a message blob.** The spec locks named payloads; diagnostics
  stay structured (`received_count: usize`, `actual_values: Vec<String>`,
  `last_headers: Option<String>`) so future tooling can read them without
  parsing prose.
- **Caps.** `actual_values` and `last_headers` cap at 8 entries; overflow
  appends `+N more`. Failure diagnostics must stay one-screen.
- **`last_headers` is keys-only.** Values of unrelated headers are noise;
  the keys answer "did the exchange arrive with a different shape".
- **No boxing of diagnostic payloads.** Boxing (the `do_try_segment.rs` `result_large_err` precedent boxes its Err) was considered and rejected: camel-mock is a non-hot-path test harness, and boxing would thread `Box<MockAssertionError>` through the shared `latch_err` chain for every branch. The lint is allowed at the three `Result` sites instead.
- **Parity kept structurally.** The panicking surface formats the same
  `MockAssertionError` value, so `Display` == panic message continues to
  hold by construction; tests assert it.

## Risks / Trade-offs

- Payload additions are a spec-visible shape change; the delta spec records
  them as a MODIFIED requirement (accepted breaking change for a
  `#[non_exhaustive]` enum with no external constructors).

## Migration Plan

None needed: no workspace code constructs or matches these variants outside
`camel-mock`.

## Open Questions

None.
