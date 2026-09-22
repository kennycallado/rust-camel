# Design: concpanic

## Approach

Fail-closed upper-bound validation for the gRPC consumer
`consumerConcurrency` parameter, with one source of truth.

1. `pub(crate) fn consumer_concurrency_limit(configured: usize) -> Result<usize, CamelError>`
   replaces the infallible clamp (visibility raised from private to
   `pub(crate)` — `config.rs` and `component.rs` are sibling modules that
   must call it; public API surface unchanged): `Ok(configured.max(1))`
   for values in
   range (0 → 1 keeps rc-ey6v normalization), `Err(CamelError::Config(..))`
   above the limit — message names `consumerConcurrency`, the configured
   value, and the limit. The bound is `tokio::sync::Semaphore::MAX_PERMITS`
   (public const since tokio 1.36; workspace pins 1.53.1), referenced
   directly inside the helper — no local alias constant that could drift
   from the primitive. Both enforcement points derive from this one
   constant: the bounded-channel capacity ceiling (`mpsc::channel`
   delegates capacity to an internal semaphore) and the dispatcher
   semaphore's `Semaphore::new` permit bound (`usize::MAX >> 3`, i.e.
   2^61 - 1 on 64-bit) — oversized values panic during semaphore
   construction on either path.
2. Enforcement seams, all calling the same function (no drift):
   - `config.rs::parse_grpc_query_params` — earliest error, consistent
     with existing numeric-param validation;
   - `component.rs::create_consumer` — component funnel; covers
     directly constructed `GrpcEndpoint`s (C2 precedent: parse defers,
     create_consumer hard-errors) and any future serde path
     (`GrpcConfig` derives `Deserialize`);
   - `consumer.rs::start()` and `start_with_listener()` — at entry,
     beside `validate_route_credential_sources()`, BEFORE any
     shared-server registry mutation: `start()` additionally precedes
     listener binding and readiness signaling;
     `start_with_listener()` receives an already-bound listener (bound
     by its caller before invocation), so its guarantee starts at
     registry mutation. A direct-constructed `GrpcConsumer` (the
     integration tests construct `GrpcConsumer::new` directly) fails
     with the typed error and no observable startup side effects;
   - `consumer.rs::start_inner` — retained as defense in depth: the
     structural guard immediately before `mpsc::channel`/
     `Semaphore::new` guarantees no construction path can panic.
3. `GrpcConsumer::new` signature is unchanged (its many integration-test
   call sites); it stores the value, validation happens at the seams above.

Test design (named tests, exact assertions; `L = tokio::sync::Semaphore::MAX_PERMITS`, formatted dynamically — never hardcoded, so 32-bit targets stay valid):

1. `consumer_concurrency_limit_normalizes_zero_and_rejects_beyond_limit`
   (consumer.rs unit, plain `#[test]`): `Ok(0)==1`, `Ok(1)==1`, `Ok(7)==7`,
   `Ok(L-1)==L-1`, `Ok(L)==L`; `L+1` → `Err(CamelError::Config)` whose
   message contains `consumerConcurrency`, `format!("{}", L + 1)`, and
   `format!("{L}")`.
2. `tokio_primitives_accept_exactly_the_limit` (consumer.rs unit):
   `tokio::sync::Semaphore::new(L)` and `mpsc::channel::<GrpcRequestEnvelope>(L)`
   both construct without panicking (both O(1)); pins our bound to the
   primitives' real bound.
3. `test_parse_grpc_uri_consumer_concurrency_at_limit_accepted`
   (config.rs): URI with `consumerConcurrency={L}` → Ok, stored value `L`.
4. `test_parse_grpc_uri_consumer_concurrency_above_limit_rejected`
   (config.rs): URI with `consumerConcurrency={L+1}` → Err
   `CamelError::Config`, message contains `consumerConcurrency`, the
   value, and the limit.
5. `test_parse_grpc_uri_consumer_concurrency_zero_normalizes_to_one`
   (config.rs): `consumerConcurrency=0` → Ok, stored value `1`.
6. `test_create_consumer_rejects_oversized_direct_endpoint`
   (component.rs; mirrors the C2 fixture): direct `GrpcEndpoint` with
   `consumer_concurrency = L+1` → `create_consumer` returns
   `Err(CamelError::Config)` naming value and limit; no panic.
7. `start_rejects_oversized_concurrency_before_registry_bind_or_readiness`
   (consumer.rs, `#[tokio::test]`): direct `GrpcConsumer::new` with `L+1`
   on an unused `(127.0.0.1, port)`; ctx built via
   `ConsumerContext::new(..)` with a held `StartupSignal::pair()` receiver
   (`with_startup`). Assert: `start(ctx).await` → `Err(CamelError::Config)`
   with value+limit in message; `GrpcServerRegistry::global()
   .contains_server(&host, port)? == false` (no registry mutation, and
   `get_or_spawn` never ran so no bind either); `receiver.await_ready().await`
   is `Err` (readiness was never signalled `Ready`).
8. `start_with_listener_rejects_oversized_concurrency_before_registry_mutation`
   (consumer.rs, `#[tokio::test]`): caller binds `TcpListener` on
   `127.0.0.1:0`, constructs the consumer for that `(host, port)` with
   `L+1`; `start_with_listener(ctx, listener).await` → same error
   assertions + `contains_server == false` + readiness-not-signalled.
   (The listener is bound by the caller by API contract — the test
   proves the error precedes registry mutation and channel/semaphore
   construction.)

`GrpcServerRegistry.inner` is private to `server.rs`; add a
`#[cfg(test)] pub(crate) fn contains_server(&self, host: &str, port: u16)
-> Result<bool, CamelError>` query method there (locks `inner`, checks
the `(host, port)` key; poison propagates as `EndpointCreationFailed`
like the existing lock sites). No other widening.

Supersession note: existing
`consumer_concurrency_limit_clamps_zero_and_follows_config` is
superseded by test 1; update/remove accordingly.

## Affected crates

- `camel-component-grpc`: `src/consumer.rs`, `src/config.rs`,
  `src/component.rs`, `src/server.rs` (test-only `contains_server`
  probe), `CONTEXT.md`.

## Architecture boundaries

Component layer only (camel-component-grpc). No Runtime, DSL, or
Services changes; the config type surface is unchanged. Follows
ADR-0033 (fail-closed startup validation: safety-primitive bounds
enforced before route start; warn-and-continue explicitly rejected)
and the rc-3y6j channel==semaphore invariant (one value still derives
both primitives; the invariant function now also guards the bound).
Data/control plane boundary untouched.

## Alternatives considered

- **Clamp to limit with a warn** (mission's option B): rejected —
  ADR-0033 rejects warn-and-continue for startup validation; a
  semaphore silently live at the platform limit is a hidden
  misconfiguration, not a fixed one. Also contradicts bd rc-9kgtm acceptance ("Reject
  unsupported values with a typed CamelError").
- **Validate at URI parse only**: rejected — blind-spot checklist
  fallback-path probe; `GrpcEndpoint` can be constructed directly
  (C2 test does exactly this) and `GrpcConfig` derives `Deserialize`,
  so parse-only validation leaves panic paths open.
- **Local constant `(usize::MAX >> 3)`**: rejected — drifts if the
  pinned tokio ever changes; referencing
  `tokio::sync::Semaphore::MAX_PERMITS` makes the primitive itself
  the definition, and the primitive-level proof test pins it.
