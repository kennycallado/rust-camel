# Proposal: fix-fanout-claim-split

## Why

The camel test driver settles shutdown on the context-global in-flight
gauge (`InFlightGauge`) reaching zero. `Exchange::clone` deliberately
sets `in_flight_claim: None` (`crates/camel-api/src/exchange.rs:229-246`)
because `InFlightClaim` is deliberately not `Clone` (a duplicated claim
would double-release on drop). Fanout sites are therefore required to
mint sibling claims via `InFlightClaim::split`
(`crates/camel-api/src/in_flight.rs:111`) so every detached branch copy
stays counted.

The fanout clone sites under `crates/camel-processor/src` (wire_tap,
load_balancer, multicast, multicast_segment, recipient_list, splitter,
split_segment, streaming_splitter, streaming_split_segment,
content_enricher, error_handler, do_try, do_try_segment) and the
camel-core adapter fanout sites (`step_compilers/splitting.rs`,
`exchange_uow.rs`) do **not** split. A detached divert/wiretap/split
branch copy is invisible to the gauge; when the real exchange completes
the gauge hits zero while the copy is still in flight, the drain wait
returns early, and the copy's completion expectation fails
(`#audit expected 1 exchanges, got 0` with `#drained PASS`). This is the
root cause of the intermittent flake family tracked in **bd rc-ulmo6**
(related rc-ykha2, rc-np32x).

## What Changes

- Add `Exchange::branch_clone()` in `camel-api`: a full `Exchange::clone`
  plus a `InFlightClaim::split()` sibling onto the copy when the source
  carries a claim. The original claim stays live; `InFlightClaim` stays
  non-`Clone`.
- Route every fanout clone site in `camel-processor` and the camel-core
  adapter fanout sites through `branch_clone()`, with a per-site
  classification pass (branch-escape copies vs in-scope copies that
  complete before the parent's claim drops).
- Add a deterministic unit test on the helper (sibling counts, original
  unaffected) and integration tests over divert/wiretap plus one
  multicast/split path proving the gauge does not reach zero while a
  branch copy is pending.
- Out of scope: making `InFlightClaim` `Clone` (explicitly forbidden),
  changing gauge counters, drain/settle logic, and canonical specs
  (`openspec/specs/**` — deltas only).

## Acceptance criteria

- `Exchange::branch_clone()` exists; a unit test proves a copy made from
  a claimed exchange carries its own split claim and the original claim
  is unaffected.
- Every listed fanout clone site compiles through `branch_clone()`.
- Integration tests: wiretap/divert plus multicast/split paths where
  the branch copy outlives the parent claim release —
  `total_in_flight() > 0` (held only by the copy's split claim) until
  the copy completes.
- RED witness (unpatched) and GREEN run logs recorded under
  `/home/shared/tmp/rc-ulmo6/`.

## Risk budget

Low. The helper is additive; routing is mechanical. Risk: over-counting
in-scope copies delays settle slightly (correct, not a defect).
Reverting is a per-site call-site swap.

Affected crates: `camel-api`, `camel-processor`, `camel-core`.
