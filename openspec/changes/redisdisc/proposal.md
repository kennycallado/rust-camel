# Proposal: redisdisc

## Why

Two field-verified defects in `camel-component-redis` (bd `rc-2un5k` p0, `rc-n64k3` p1, e_opus ruling 2026-11-14 attached to both):

1. **Dead connection cached forever.** `MultiplexedExecutor` caches one
   `MultiplexedConnection` and only drops it inside `reconnect()`, which runs
   before a *retry*. Non-idempotent commands get `NetworkRetryPolicy::disabled()`,
   so the first transport error returns immediately and every later command keeps
   drawing the dead connection from the cache. `SET` inherited the failure mode
   when `41df2e33e` reclassified it option-blind (correct change — do not revert).
   `check_connection` has the same hole and additionally maps the PING error to a
   source-less `ProcessorError`, so no classifier can read it; health stays red
   for good.
2. **Retry budget never resets.** No Redis consumer loop resets its attempt
   counter after a proven round trip (`pubsub.rs`, `queue.rs`, `streams.rs` read
   task, `streams.rs` recovery). A recovered blip permanently consumes budget;
   after ~10 `CLIENT KILL` events the consumer dies with "retry budget
   exhausted". Kafka/MQTT/JMS reset on success; Redis is the outlier.

## What Changes

- `executor.rs`: the cache becomes generation-tagged
  (`Option<(u64, MultiplexedConnection)>`). On command failure inside
  `MultiplexedExecutor::execute_command` and `check_connection`, invalidate with
  compare-and-clear (clear only if the failing generation is still cached).
  Never dial in the error path; the next `get_conn` rebuilds lazily under the
  existing `connect_gate` and re-resolves sentinel. The original error is
  rethrown unchanged. `reconnect()` becomes generation-aware (per-instance
  last-used generation) so N clones failing on one socket do not clobber each
  other's fresh connection. No new trait method. `refresh()` keeps its
  unconditional-clear contract.
- `transport_error.rs`: new `invalidates_connection(&CamelError)` reusing the
  bounded chain walk. Invalidates on Io (reset/abort/broken pipe/refused),
  driver connection-dropped, `Server(ReadOnly)` (the one server reply that
  invalidates), and the `TransportTimeout` marker. All other server error
  replies never invalidate. It MUST be a strict subset of
  `is_transient_redis_error`, pinned by a property test. No new `CamelError`
  variant.
- `producer.rs`: `check_connection` invalidates on the new classifier and
  preserves the PING error's source chain.
- The four consumer loops reset the consecutive-failure counter exactly at the
  e_opus reset points (streams: `BatchDone` after a failure-free read round
  trip; recovery: after a successful `XAUTOCLAIM` pass; queue: on pop
  `Ok(Some)`/`Ok(None)`; pubsub: stream end after uptime ≥ `policy.max_delay`).
  Never reset on connect.
- ADR-0013 amended with the invalidation rule and the budget semantics.
- Docs fixes: `concepts/error-handling.md` (supervision is NOT "Rust API only";
  `[supervision]` exists in `Camel.toml`) and `configuration/schema.md`
  (`max_attempts` default 5, but an omitted value with the table declared means
  restart forever).
- Tests: headline kill-connection regression (next command reconnects), health
  recovery after a kill, per-loop consecutive-budget reset tests, classifier
  subset property test.

Affected crates: `camel-component-redis` (code + tests), `camel-test`
(integration suites), `docs` (ADR + guide). bd: `rc-2un5k`, `rc-n64k3`.

## Acceptance criteria

- Kill the producer connection (`CLIENT KILL`) → the NEXT command succeeds via a
  rebuilt connection; error is rethrown unchanged on the failing command.
- Health check recovers after a kill (connection invalidated on the failing
  PING).
- N kill/recover cycles do not exhaust any consumer budget; a run of consecutive
  failures still exhausts it.
- `invalidates_connection(e)` implies `is_transient_redis_error(e)` for the full
  corpus; only `Server(ReadOnly)` invalidates among server replies.
- ADR-0013 amendment and both docs corrections merged; `mdbook test` green.

## Risk budget

In scope: the invalidation seam, the classifier, the four reset points, ADR and
docs. Out of scope: reverting the `SET` reclassification, `rc-nukx2`
(bitmaps/HLL/WAIT), `rc-grcgu` (builds on the classifier later), idempotent
consumer work (mission 588), new trait methods, new `CamelError` variants.
