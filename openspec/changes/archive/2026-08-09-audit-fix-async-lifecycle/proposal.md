# Proposal: audit-fix-async-lifecycle

## Why

Five async-lifecycle defects across five crates cause detached tasks, leaked
resources, wrong error variants, skipped drains, and head-of-line blocking
under shutdown or backpressure. Each is a latent correctness bug that masks
under current happy-path conditions but violates documented ADRs and
lifecycle contracts.

- **camel-health**: `HealthServer::stop` detaches the probe-drain task on
  timeout without `.abort()`; `SHUTDOWN_TIMEOUT` (5s) < `handler_timeout`
  (6s) creates a 1s window where status reports stopped while the probe
  still drains, enabling a rebind race.
- **camel-function**: `FunctionRuntimeService::stop` short-circuits on the
  first `provider.shutdown` error via `?`, aborting the drain loop and
  orphaning remaining runners, health tasks, and containers.
- **camel-jms**: `LazyJmsProducer::poll_ready` returns
  `ProcessorError` on `BridgeState::Stopped` instead of
  `ConsumerStopping` (ADR-0024 §Decision), causing the route to treat
  clean shutdown as a generic 500 error and bypass exception policies.
- **camel-master**: `stop_delegate` has three early-return arms that skip
  the epoch-bridge drain, leaving a detached bridge that can stamp stale
  exchanges after the leader yields.
- **camel-auth**: `CachingTokenIntrospector` and
  `CachingPermissionEvaluator` hold a single `Mutex<()>` across the
  backend HTTP await, serializing all tokens/requests under a slow IdP.

## What Changes

- camel-health `server.rs`: abort handle on timeout, handle JoinError,
  derive shutdown timeout from handler timeout.
- camel-function `service.rs`: best-effort drain loop in `stop()`,
  collect first error, drain remaining before returning.
- camel-jms `component.rs`: `BridgeState::Stopped` arm returns
  `ConsumerStopping` instead of `ProcessorError`.
- camel-master `leadership.rs`: restructure `stop_delegate` so
  epoch-bridge drain runs on ALL paths, delegate error propagated after.
- camel-auth `introspection.rs` + `permission_cache.rs`: replace
  `Mutex<()>` with per-key dedup map, lazy cleanup in eviction.

## Acceptance criteria

- No detached JoinHandle on any shutdown path in the five affected crates.
- `FunctionRuntimeService::stop` drains all providers even on partial
  failure.
- `LazyJmsProducer::poll_ready` returns `ConsumerStopping` on Stopped.
- `stop_delegate` always drains epoch-bridge regardless of delegate outcome.
- Different tokens/requests no longer serialize on the auth cache.
- All existing tests pass; new regression tests cover each fix.

## Risk budget

Low risk — each fix is a targeted async-lifecycle correction in
well-tested crates. The auth per-key-lock change is the most invasive
(two structs) but follows an established pattern. No public API changes
beyond error-variant alignment (JMS).
