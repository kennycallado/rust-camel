# Endpoint-Created PollingConsumer for pollEnrich

**Status**: Proposed (2026-06-07)

The `pollEnrich` DSL verb resolves its URI to an `Endpoint`, calls the
existing `Endpoint::polling_consumer() -> Option<Box<dyn PollingConsumer>>`
opt-in method, and invokes `receive(timeout)` on the result. We rejected the
producer-side alternatives (`file:...?mode=read` bridge, new `resource:`
component) because they invert the Tower `Service<Exchange>` producer invariant
(write/send only) and would constitute fake URI reuse across the data plane,
violating ADR-0001 and ADR-0014. The `PollingConsumer` trait already lives in
`crates/components/camel-component-api/src/endpoint.rs`; this ADR records the
decision to wire it up for the EIP-7 Content Enricher use case rather than
introduce a new mechanism.

## Considered Options

- **A. Endpoint-created PollingConsumer (chosen)** — reuse the existing
  `PollingConsumer` trait; endpoints opt in by overriding `polling_consumer()`.
  File/SEDA/JMS can return `Some(...)`; HTTP server, Kafka return `None`.
- **B. Producer `file:...?mode=read` bridge** — REJECTED: violates ADR-0014
  (fake URI reuse) and inverts the producer semantic.
- **C. New `ReadEndpoint` trait** — REJECTED: component-specific, doesn't
  reuse the existing consumer logic (filters, idempotency, path traversal).
- **D. `resource:` standalone component** — REJECTED: same flaws as B plus
  duplicates filesystem logic.
- **E. Consumer downcast** — REJECTED: ugly, doesn't generalize, requires
  every pollable component to share a concrete `Consumer` shape.

## Consequences

- Adding `timeout: Duration` to `PollingConsumer::receive` is a **breaking
  change** for any external implementor of the trait. Acceptable while the
  trait has zero implementors in-tree (all current `Endpoint::polling_consumer`
  overrides return `None`). Bump `camel-component-api` minor version.
- `FileEndpoint::polling_consumer()` is the first non-`None` implementation
  in the workspace; it must establish the eager-finalization lifecycle
  (delete/move/idempotency-mark happen inside `receive`, before the Exchange
  is returned) that future implementors will follow.
- WASM gains a sibling host function `camel_poll(uri, timeout_ms)`. Plugins
  remain backward compatible (additive WIT imports). No versioned worlds for
  v1 — carto-kit is the only consumer and is pre-production.
- The DSL strategy trait is named `EnrichmentStrategy`, not
  `AggregationStrategy`, to avoid collision with the existing EIP-22
  `AggregateStrategyDef` family.
