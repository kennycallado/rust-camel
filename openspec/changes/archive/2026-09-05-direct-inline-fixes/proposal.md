# Proposal: direct-inline-fixes

## Why

Post-land review of `623cca62` (perf(direct): inline dispatch) found one
critical and one important defect, both fix-forward small:

1. **rc-2sba (P0, silent wrong results)**: aggregate-split routes
   publish the inline dispatcher over an EMPTY pipeline. For
   `from("direct:X").aggregate(...)`, `detect_and_validate_route_split`
   returns `(Some(split), vec![])` → `compose_pipeline(vec![])` →
   `IdentityProcessor`; `start_aggregate_route` runs pre/agg/post from
   the split pipelines and never swaps `managed.pipeline`. Every
   dispatched fragment returns unprocessed, unaggregated, without
   error or metric — on the exact split-aggregate topology that
   motivated the perf change. No test covers direct-entry aggregates;
   `inline_dispatcher_tests.rs:676` actively mandates the buggy
   publication, and the synced spec codifies it by omission.
2. **rc-y5nn (P1, full-tests red on main)**: the b′ emission site
   moved from the (deleted) consumer receive loop to the producer
   runtime handle (`camel-direct/src/lib.rs:518-532`);
   `late_registration_after_routes_observed` builds its producer with
   a NoOp handle, so an unhandled dispatch failure is no longer
   operator-visible. The lookup `?` (`lib.rs:483`) also exits before
   the producer emission block. The originally proposed fix (route
   lookup failures through the traced wrapper) would not turn the
   test green — this is a b′-visibility regression, not a tracing
   ordering issue.

## What Changes

- Suppress dispatcher publication for aggregate-split routes (they
  keep today's channel path, which is correct); invert the test that
  mandated publication; add an end-to-end regression test
  (direct-entry aggregate: N fragments → 1 aggregated reply).
- Restore b′ visibility for unhandled dispatch failures (lookup,
  admission, in-pipeline): exactly once via the producer's
  context-threaded runtime handle (`Arc::clone(&component_ctx)` at
  route compile), `ConsumerStopping` excluded, timeout branch
  included, channel path unchanged.
  
- Amend the synced `direct-dispatch` spec: aggregate exclusion in the
  selection requirement + the inline-failure visibility contract.

**Out of scope**: rc-e2r9 (channel-path b′ drop on producer-timeout
abandonment — pre-existing, separate bd); aggregate INLINE dispatch
(dispatcher over pre+agg+post — not justified now); depth>64 channel
degradation (deliberate, spec'd behavior); fairness counter counting
errors (cosmetic).

## Acceptance criteria

- `from("direct:X").aggregate(timeout)` end-to-end returns ONE
  aggregated reply for N fragments (test, red before the guard).
- Aggregate routes do NOT publish the dispatcher capability (inverted
  assertion); non-aggregate Sequential publication unchanged.
- `cargo test -p camel-test --test metrics_wiring_test` green
  (late_registration case).
- No b′ double-emission for inline dispatches (test).
- `cargo fmt/clippy` clean; full camel-core + camel-direct +
  camel-test suites green.

## Risk budget

- Acceptable: one behavior toggle (aggregate → channel) restoring
  pre-623cca62 semantics; additive dispatcher constructor arg.
- Out of bounds: touching drain/stop semantics, Concurrent fallback,
  admission, or timeout structure; any perf regression of the gated
  9.3x plain-hop win (bench re-run to confirm no fallback leakage).

Affected: camel-core (lifecycle adapters, controller publication),
camel-direct (dispatcher construction/emission), camel-test (red test
turns green), openspec/specs/direct-dispatch. Bd: rc-2sba, rc-y5nn.
