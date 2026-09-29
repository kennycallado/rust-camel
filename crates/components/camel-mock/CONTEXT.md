Mock component for rust-camel — records every exchange sent to `mock:name`
endpoints and verifies them at assertion time. Producer-only: a `to:
mock:name` step records the exchange. Consumer creation is rejected.

## Language

**MockComponent**:
Component for `mock:name` URIs. Registered into the `CamelContext` for
tests. Creates mock endpoints from parsed `MockConfig` values.
_Avoid_: stub component, fake endpoint

**MockConfig**:
Defaults for mock endpoints: `max_retained`, `copy_on_exchange`,
`fail_fast`, and `any_order`. URI parameters override one endpoint at a
time. `copy=true` deep-copies the recorded body.
_Avoid_: mock settings, mock options

**expectedCount**:
URI parameter. It registers an exact count expectation when the endpoint
is first created. The expectation is assertion-time only. It never rejects
live traffic.
_Avoid_: count gate, arrival limit

**failFast**:
URI parameter and `MockConfig` default. A body mismatch trips a latch.
After the latch trips, the endpoint rejects new exchanges.
_Avoid_: bail-out mode

**anyOrder**:
URI parameter. Expected bodies match without their configured order.
_Avoid_: unordered mode

## Boundary

The mock records what the route under test actually sent. Wire-level
proof belongs to the integration tier (ADR-0069, harness HTTP partners).
Use the mock for in-process route assertions, not wire proofs.
