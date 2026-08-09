# camel-direct

Direct component for rust-camel — synchronous, in-memory communication between routes sharing the same CamelContext. When a Producer sends to a `direct:name` endpoint, it blocks until the Consumer registered on the same name finishes processing the Exchange. No serialization overhead — Exchanges pass through an in-memory channel.

## Language

**DirectEndpoint**:
Endpoint for `direct:name` URIs; maintains an in-memory channel that pairs producers with the consumer registered to the same name.
_Avoid_: direct channel, direct address

**DirectConsumer**:
Event-driven Consumer bound to a `direct:name`; receives Exchanges synchronously from any DirectProducer on the same name within the CamelContext.
_Avoid_: direct listener, direct receiver

**DirectProducer**:
Producer that delivers an Exchange to the DirectConsumer synchronously via `send_and_wait`. Blocks the calling Route until the consumer's Pipeline completes; returns the (possibly transformed) Exchange or the failure if the consumer's Pipeline errors.
_Avoid_: direct sender, direct caller

## Log-level policy

Per ADR-0012.

**Outside-contract metric:**
- `b-prime:direct:send-and-wait` identifies the `DirectConsumer::start` `Err` branch after
  `send_and_wait`. ADR-0012 §Migration scope classifies it as category (b′)
  outside-contract. The normal-data call returned an error, so the route handler did not
  absorb the failure. This branch owns the ERROR signal. It calls
  `runtime.metrics().increment_errors(route_id, label)`, then logs at `error!` with
  `// log-policy: outside-contract`. Regression test:
  `tests::test_send_and_wait_error_increments_errors_metric`.

## Startup handshake

`DirectConsumer` declares `ConsumerStartupMode::Explicit`. Its `start()` calls
`ConsumerContext::mark_ready()` immediately after inserting into the shared
`DirectRegistry`, before entering the event loop. The runtime's `start_context`
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
