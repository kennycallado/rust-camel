# camel-direct

Direct component for rust-camel — synchronous, in-memory communication between routes sharing the same CamelContext. When a Producer sends to a `direct:name` endpoint, it blocks until the Consumer registered on the same name finishes processing the Exchange. The Producer submits each Exchange to the consumer route without any per-message channel or oneshot inside camel-direct: when the registered consumer is live and Sequential, the dispatch runs INLINE in the producer's task through the `InlineRouteDispatcher` capability (zero inter-task handoffs, JVM `direct:` parity); every other case (Concurrent consumer, capability unavailable) falls back to the consumer context's `send_and_wait` channel submission.

## Language

**DirectEndpoint**:
Endpoint for `direct:name` URIs; holds the registry entry that pairs producers with the consumer's route submission context registered under the same name.
_Avoid_: direct channel, direct address

**DirectConsumer**:
Event-driven Consumer bound to a `direct:name`; publishes its `ConsumerContext` under the name and stays live for dispatch. Producers submit Exchanges into its route pipeline synchronously through that context.
_Avoid_: direct listener, direct receiver

**DirectProducer**:
Producer that delivers an Exchange to the consumer route's pipeline synchronously. Path selection: registered dispatcher capability present (Sequential, live) → inline dispatch on the producer's own task, guarded by the task-local cycle/depth stack; otherwise → `ConsumerContext::send_and_wait`. Blocks the calling Route until the consumer's Pipeline completes; returns the (possibly transformed) Exchange or the failure if the consumer's Pipeline errors.
_Avoid_: direct sender, direct caller

## Log-level policy

Per ADR-0012.

**Outside-contract metric:**
- `b-prime:direct:send-and-wait` identifies the `DirectProducer::call` `Err` branch after
  path selection (inline dispatch or `send_and_wait`). ADR-0012 §Migration scope classifies it as category (b′)
  outside-contract. The normal-data call returned an error, so the route handler did not
  absorb the failure. This branch owns the ERROR signal. It calls
  `runtime.metrics().increment_errors(route_id, label)` — with the consumer route's id
  (from the registry entry's context) as the b′ owner — then logs at `error!` with
  `// log-policy: outside-contract`. Regression test:
  `tests::test_send_and_wait_error_increments_errors_metric`.

## Startup handshake

`DirectConsumer` declares `ConsumerStartupMode::Explicit`. Its `start()` calls
`ConsumerContext::mark_ready()` immediately after inserting into the shared
`DirectRegistry`, before parking on the cancellation token. There is no receive
loop: producers dispatch inline when the capability is present, else through the registered context's `send_and_wait`
directly, so the consumer task only owns lifecycle (registration, readiness,
cleanup) and a `CloseGuard` that marks the registry entry closed on any exit
path (normal return, panic, or task abort) — a crashed consumer's entry is
therefore overwritable by a replacement consumer, and `poll_ready` reports a
closed entry not-ready. The runtime's `start_context`
starts routes sequentially by `startup_order`; a producer route with a higher
`startup_order` than the consumer route will not be driven until the consumer's
`StartRoute` completes (registration visible + `mark_ready` resolved).

**Residual operator window:** if a producer route and its consumer route share
the same `startup_order` (default 1000), ordering within the tier is by the
controller's stable list order. Operators who need a strict guarantee set the
consumer's `startup_order` lower so it starts first. This matches Apache
Camel's own guidance (start direct consumers before their producers).

## Example dialogue

> "How is direct different from SEDA?"
> "Direct is synchronous — the producer blocks until the consumer finishes processing. SEDA is asynchronous — the producer sends to a bounded queue and returns immediately. Direct has lower overhead; SEDA decouples route lifetimes."

> "What happens if the consumer's route has no error handler?"
> "The producer's `send_and_wait` returns an Err. The DirectConsumer records the failure via `increment_errors` with label `b-prime:direct:send-and-wait` and logs at `error!`. The error propagates back to the producer, which must handle it in its own route."

## Residual: b′ signal for producer-abandoned dispatches (Hook B)

Since the channel collapse, the b′ error emission for a failed dispatch lives
in `DirectProducer::call` inside the `tokio::time::timeout` wrapper. If the
producer timeout drops the future while the exchange is already enqueued to
the consumer route, a later unhandled pipeline failure replies into a dropped
oneshot receiver (`route_controller.rs` reply site, `let _ = tx.send(Err(e))`)
and emits NO operator signal. The inline fast path (Phase 3) does not have
this window (no queue between producer and pipeline; a dropped future drops
the whole operation). Tracked for the channel path in beads; fix belongs at
the reply-drop observation site in camel-core.
