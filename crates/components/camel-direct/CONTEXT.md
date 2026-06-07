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

**Labels wired in Phase B (commit 2cd7ae9d):**
- `b-prime:direct:send-and-wait` (`src/lib.rs:315` DirectProducer failure path) — category (b′) outside-contract: producer's `send_and_wait` returned Err, meaning the consumer's Pipeline did NOT absorb the failure; this is the only ERROR signal. Calls `runtime.metrics().increment_errors(route_id, label)` then logs at `error!` with `// log-policy: outside-contract`. Regression test: `test_send_and_wait_error_increments_errors_metric`.

## Example dialogue

> "How is direct different from SEDA?"
> "Direct is synchronous — the producer blocks until the consumer finishes processing. SEDA is asynchronous — the producer sends to a bounded queue and returns immediately. Direct has lower overhead; SEDA decouples route lifetimes."

> "What happens if the consumer's route has no error handler?"
> "The producer's `send_and_wait` returns an Err. The DirectConsumer records the failure via `increment_errors` with label `b-prime:direct:send-and-wait` and logs at `error!`. The error propagates back to the producer, which must handle it in its own route."
