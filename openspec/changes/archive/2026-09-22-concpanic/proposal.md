# Proposal: concpanic

## Why

v0.52.0 (e0a2bace, bd rc-ey6v) made the gRPC consumer `consumerConcurrency`
URI parameter configurable. The parsed `usize` flows, unbounded, into
`tokio::sync::mpsc::channel` and `tokio::sync::Semaphore::new` in
`GrpcConsumer::start_inner`. `mpsc::channel` delegates its capacity to an
internal semaphore, and semaphore construction panics beyond
`tokio::sync::Semaphore::MAX_PERMITS` (`usize::MAX >> 3`, i.e. 2^61 - 1
on 64-bit): `consumerConcurrency=2305843009213693952` (2^61, the limit
plus one) passes config parsing and then
panics route startup instead of returning a configuration error
(bd rc-9kgtm, retro520 blind-spot class: "values valid to parse but
invalid downstream").

## What Changes

- Reject oversized `consumerConcurrency` at configuration validation time,
  fail closed with a typed `CamelError::Config` naming the configured value
  and the limit. Warn-and-clamp was considered and rejected per ADR-0033
  (warn-and-continue leaves a silent misconfiguration live).
- Single source of truth: `consumer_concurrency_limit` becomes fallible —
  clamps 0 to 1 (unchanged rc-ey6v behavior) and rejects values above
  `tokio::sync::Semaphore::MAX_PERMITS` (referenced directly inside the
  helper, no local re-definition that could drift from the primitive).
- Enforced at four seams: URI parse (`parse_grpc_query_params`),
  `create_consumer` (component funnel, covers directly constructed
  `GrpcEndpoint`s per the C2 precedent), consumer startup entry
  (`start()`: before shared-server registry mutation, listener
  binding, or readiness signaling; `start_with_listener()`: before
  registry mutation — its listener is bound by the caller before
  invocation — covering all direct `GrpcConsumer::new` construction
  paths with no observable side effects), and `start_inner`
  (defense-in-depth structural guard immediately before channel
  construction — no construction path can panic).
- Boundary tests at limit-1, limit, limit+1, plus a primitive-level
  no-panic proof (channel + semaphore accept exactly the limit).
- Affected crates: `camel-component-grpc` only.
- Docs: field doc comment and `CONTEXT.md` consumer paragraph updated.
- Explicitly EXCLUDED: camel-http `maxInflightRequests` has the same
  unbounded `Semaphore::new` exposure (lib.rs:1256) — same-family finding
  filed as a bd follow-up, out of scope here.

## Acceptance criteria

- `consumerConcurrency` above `Semaphore::MAX_PERMITS` never reaches Tokio
  channel/semaphore construction; every path returns a typed
  `CamelError::Config` naming `consumerConcurrency`, the configured value,
  and the limit.
- Boundary values: limit-1 accepted, limit accepted, limit+1 rejected.
- `consumerConcurrency=0` still normalizes to 1 (rc-ey6v behavior
  unchanged); default 64 unchanged.
- Existing component tests and integration tests pass; `cargo fmt`,
  `cargo clippy -p camel-component-grpc --all-targets -- -D warnings`
  clean.

## Risk budget

Low. Purely additive validation; the only behavior change is that values
which previously panicked route startup now fail with a typed config
error. No public API signature changes (`GrpcConsumer::new` untouched).
Fleet protocol: no merge, no push; park in fleet inbox on completion.
