# Design: endpoints-created-counter

## Approach

Count at the resolver/factory choke point in
`crates/camel-core/src/lifecycle/adapters/endpoint_resolver_factory.rs`, immediately
after `Component::create_endpoint` succeeds. The resolver already holds
`rt: Arc<dyn RuntimeObservability>`; `rt.metrics()` returns the shared
`MetricsCollector` handle. This is the established late-bound pattern
(`context_tests.rs:1292`, `splitting.rs:344`), so no second handle mechanism and no
constructor changes are needed.

Recorded name: `camel.core.endpoints_created_total` — the fully qualified dot form,
consistent with the cache-EIP dotted convention (`camel.cache.misses`). Post-rc-oo2w
the exporter treats the `camel.` prefix as already-applied and sanitizes dots to
underscores, so the exported family is `camel_core_endpoints_created_total`.

The `component` label value is the URI scheme. Its value set is bounded by the
registered component schemes but is not a compile-time closed set, so the call site
carries the lint annotation `// allow-open-label rc-haik` (marker alone does not
suppress; the bd ref is required).

## Alternatives considered

- Count inside `Component::create_endpoint` (trait level): rejected per STAGE 0
  ruling — it would touch the trait contract, require every component to thread the
  handle, and miss the single choke point where every resolver-path creation flows.
- Record `core.endpoints_created_total` instead: exports to the same family
  (`camel_core_endpoints_created_total`); the fully qualified dot form was chosen
  for symmetry with `camel.cache.misses` and self-describing recorded names.

## Test strategy

Two camel-core unit-tier tests (TDD, red before the fix), harness adapted from
`RecMetrics` in `context_tests.rs`: a `CountingMetrics` recording collector plus a
`TwoSchemeContext` stub resolving `direct` and `mock`. Test 1 asserts one increment
per created endpoint under each component label. Test 2 asserts the recorded family
name is exactly the rc-haik dotted form (no cheap registry-export harness exists at
camel-core unit tier, so the name assertion runs against the recording collector).
The hexagonal architecture boundaries test guards layering.
