# Proposal: itest-partner-faults-and-asserts

## Why

The integration tier can express happy-path flows against scripted partners,
but not failure paths. A scenario author cannot make a partner slow down,
break the connection, or answer the same way twice, so retry, timeout, and
circuit-breaker logic in routes under test stays untested. At the same time
the partner recorder already captures every request that reaches the wire
(method, path, headers, body), but the grammar cannot reach that data: a
scenario cannot assert "the route called the partner exactly three times",
which is the assertion that proves a retry policy works (bd rc-8f6r, rc-7x0l).

Failure-path testing is the main reason a Camel adopter reaches for a test
harness. Without it the tier demos well but does not earn a place in CI.

## What Changes

- Extend the `partners:` script grammar (camel-integration-test,
  `document.rs` + `adapters/http.rs`) with three response behaviors:
  - `delay`: hold the response for a humantime duration before serving.
  - `fault: close`: drop the connection without an HTTP response, so the
    client sees a transport error rather than a polite 5xx.
  - `times: N`: serve one script entry to the first N matching requests
    before it is spent (absent keeps today's serve-once semantics).
- Extend `validate` with a `partner` target: assert on the recorder's
  snapshot of received requests. V1 assertions: exact `count` with optional
  `method` and `path` filters, and an optional `deadline` that polls until
  the count matches (retry scenarios settle asynchronously).
- Specs: MODIFIED "Scripted partner declarations" (delay, fault, times,
  backward compatibility) and ADDED "Partner request verification"
  (partner target, count filters, poll-or-immediate semantics, failure
  class).
- Docs: README grammar section, book testing page, one runnable example
  exercising a retry route against a faulting then healthy partner.

Excluded: response-body assertions on recorded history beyond count filters
(follow-up once count proves out), non-HTTP partner faults, fault
probability/distributions (deterministic scripts only), and any change to
product crates.

## Acceptance criteria

- A partner script with `delay: 500ms` holds the response measurably.
- A `fault: close` entry produces a client-side transport error, and the
  request is still recorded.
- `times: 2` serves two matching requests, then the entry is spent.
- Existing documents (no `delay`/`fault`/`times`) load and run unchanged.
- `validate` with a `partner` target asserts filtered counts, immediately
  or polled until an optional deadline; mismatch fails with the existing
  `validation-mismatch` verdict class naming the partner.
- Unknown fields in the new grammar positions are load errors naming the
  key, consistent with the existing taxonomy.

## Risk budget

Risk stays inside the harness: camel-integration-test is the only crate
expected to change (camel-cli only if feature-gated tests require it); no
product crate is touched. The serve loop
gains a delay and a fault path; both must keep the one-request-per-entry
invariants and the recorder untouched-by-assertions property. Worst case is
a regression in partner scripting, covered by the existing scripting e2e
suite plus new tests for each behavior.
