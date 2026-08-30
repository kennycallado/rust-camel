# Design: consumer-cancel-token-conformance

## Approach

ADR-0007 keeps consumer lifecycle Runtime-owned. camel-core implements
this: each route consumer receives a supervision token. The Runtime
builds it as a child of the managed consumer token
(`route_controller_trait.rs:352,806`). Route stop cancels the parent.
The supervision task waits on the consumer context token, then aborts
the background task when the consumer does not exit
(`consumer_management.rs:493-540`). A consumer that selects only on a
fresh token is deaf to this chain. It hangs until the abort.

The fix is one rule: the consumer-lifetime token comes from
`ConsumerContext::cancel_token()`
(`crates/components/camel-component-api/src/consumer.rs:272`). Reference
implementation: `crates/components/camel-mqtt/src/consumer.rs:118`.

Three wiring shapes cover the five consumers:

1. Direct replacement (kafka, jms, cxf). `start()` replaces
   `CancellationToken::new()` with `ctx.cancel_token()`. Each loop keeps
   its current select arms.
2. Child link (redis). `start()` stores
   `ctx.cancel_token().child_token()`. `RedisConsumer::stop()`
   (`consumer.rs:205-207`) cancels the stored token to break local
   sessions. A direct replacement is wrong here: all clones share cancel
   state, so a local `stop()` would fire the Runtime-owned token out
   from under supervision. A child token keeps `stop()` local. The
   parent cascade still reaches the loop on route stop.
3. Overwrite (wasm-source). The token is created in `new()`
   (`source_consumer.rs:78`) and cloned into `SourceHostState` (lines
   129-137). `start()` overwrites `self.cancel_token` with
   `ctx.cancel_token()` before the state is built.

Per-request child tokens (for example HttpConsumer fan-out) stay legal
and unchanged.

Tests run offline. No testcontainers, brokers, or bridges:

- kafka: start with an unreachable broker URL and short reconnect
  delays. Cancel the context token. `stop()` completes within 2 s.
- jms: `start()` pre-flight needs a bridge binary. Mirror the injected
  handle harness (consumer tests at line 694). A spawned task waits on
  the context token and is wired as a task handle. Cancel the token.
  `stop()` joins within 1 s.
- cxf: use the `spawn_mock_bridge` harness. Cancel after start. The
  handle completes within 2 s.
- redis: start against an unreachable Redis URI. Cancel the context
  token. The poll loop exits within 2 s.
- wasm: reuse the `tests/source_integration.rs:103` harness. It builds
  a `ConsumerContext` with an external token. Cancel after start. The
  run task completes within 2-3 s.

## Affected crates

- `camel-kafka`: `src/consumer.rs` token source, one test.
- `camel-jms`: `src/consumer.rs` token source, one test.
- `camel-cxf`: `src/consumer.rs` token source, one test.
- `camel-component-wasm`: `src/source_consumer.rs` token overwrite, one
  test.
- `camel-redis`: `src/consumer.rs` child link, one test.

## Architecture boundaries

Components layer only. No Runtime, DSL, or Services change. camel-core
supervision keeps its current behavior. The change conforms components
to an existing Runtime contract. The data/control plane boundary is
untouched.

Accepted patterns, no change:

- master `supervision.rs:74`: the loop selects its own token and the
  context token (dual select). Route stop is observed today.
- mqtt `producer.rs:134`: driver task with Drop-based teardown. Not a
  route consumer. Something cancels it on teardown.

## Alternatives considered

- API accessor on `ComponentContext` for producers (wasm
  `producer.rs:141`, `bean.rs:52`): rejected for this change. It is an
  API addition and no `ConsumerContext` exists there. bd rc-515m tracks
  the accessor as follow-up work.
- Fix the camel-core supervision select race (the unbiased select can
  abort a consumer mid graceful drain): rejected. Pre-existing Runtime
  behavior, out of scope.
- Xtask ratchet lint for consumer-lifetime `CancellationToken::new()`:
  rejected here. bd rc-pu2s tracks it as follow-up work and seeds its
  counter from this change.
