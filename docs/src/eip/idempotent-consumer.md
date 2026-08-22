# Idempotent Consumer

The Idempotent Consumer is a System Management pattern from Hohpe and Woolf. It detects duplicate exchanges by a message key and skips them. The route processes only the first delivery of each key.

```yaml
{{#include ../../../examples/idempotent-consumer/routes.yaml:idempotent-consumer-route}}
```

The `idempotent_consumer` step computes a key from each exchange with a `MessageIdExpression`. In the included route, the expression reads `${header.messageId}`. The step then asks its repository whether it has seen the key before. A new key runs the child steps, and the step records that key in the repository. A repeated key skips the child steps entirely. The route pins `messageId` to a fixed value across five timer ticks. Only the first tick runs the `log` and `to` steps. The other four are duplicates.

A duplicate does not raise an error. The segment returns `Completed` and the parent pipeline continues. This matters when a source retries delivery on failure. Without deduplication, each retry re-runs the child steps and produces duplicate side effects. With the Idempotent Consumer, the second and later deliveries of the same key return `Completed` and leave the recorded result intact.

Use the Idempotent Consumer when a route must tolerate redelivery without repeating work. Payment processing, order creation, and any at-least-once message source benefit from a deduplication gate. The example registers a memory-backed repository. A memory-backed repository loses its keys when the process exits. Two durable backends survive a restart. `"redb"` stores keys in an on-disk file. `"redis"` stores keys in a shared Redis keyspace ([ADR-0063](../adr/0063-redis-repository-service.md)). Configure the redis backend with `[default.idempotent_repo]`:

```toml
{{#include ../../../examples/redis-repositories/Camel.toml:idempotent-repo}}
```

The redis repository registers under the name `"redis"`, so a route selects it with `repository = "redis"`. A redis repository also shares deduplication keys across processes.

The Idempotent Consumer differs from the [Claim Check](claim-check.md). Both use a repository trait. The Idempotent Consumer stores only the key. The Claim Check stores the full payload. Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), the consumer is an outcome-aware segment. Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), it compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/idempotent-consumer`](https://github.com/kennycallado/rust-camel/tree/main/examples/idempotent-consumer).
