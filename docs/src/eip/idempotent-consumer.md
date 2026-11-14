# Idempotent Consumer

The Idempotent Consumer is a System Management pattern from Hohpe and Woolf. It detects duplicate exchanges by a message key and skips them. The route processes only the first delivery of each key.

```yaml
{{#include ../../../examples/idempotent-consumer/routes.yaml:idempotent-consumer-route}}
```

The `idempotent_consumer` step computes a key from each exchange with a message-id expression. In the included route, the expression reads `${header.messageId}`. The step then asks its repository whether it has seen the key before. A new key runs the child steps, and the step records that key in the repository. A repeated key skips the child steps entirely. The route pins `messageId` to a fixed value across five timer ticks. Only the first tick runs the `log` and `to` steps. The other four are duplicates.

A duplicate does not raise an error. The segment returns `Completed` and the parent pipeline continues. This matters when a source retries delivery on failure. Without deduplication, each retry re-runs the child steps and produces duplicate side effects. With the Idempotent Consumer, the second and later deliveries of the same key return `Completed` and leave the recorded result intact.

The step fails an exchange whose key is absent or empty, and the error names the step and the repository. An empty key counts as absent, and the step never registers it. Set `on_missing_key: process` to forward such an exchange to the child steps with a warning and no repository access. The default policy is `fail`, because a key comes from untrusted exchange data ([ADR-0099](../adr/0099-idempotent-reservation-semantics.md)).

ADR-0099 also sets the eager default. With `eager` enabled, `remove_on_failure` defaults to `true`. The step removes the reservation when the child fails, so a route-level retry runs the child again. An explicit `remove_on_failure: false` keeps an at-most-once contract, and the route load emits a warning. A failed removal fails the exchange and returns the child error unchanged. The next delivery of that key retries the removal before it checks the repository. One delivery owns that recovery, and a concurrent delivery of the same key fails with an error ([ADR-0099](../adr/0099-idempotent-reservation-semantics.md)).

Duplicate skips are observable. A skipped exchange carries the `camel.idempotent.duplicate` property set to `true`, and the step increments the `camel.idempotent.duplicates` counter. The step reports both duplicate paths: a repository contains hit and a lost eager reservation race. A failed non-eager repository add keeps the exchange successful and increments `camel.idempotent.add_failures` ([ADR-0099](../adr/0099-idempotent-reservation-semantics.md)).

Use the Idempotent Consumer when a route must tolerate redelivery without repeating work. Payment processing, order creation, and any at-least-once message source benefit from a deduplication gate. The example registers a memory-backed repository. A memory-backed repository loses its keys when the process exits. Two durable backends survive a restart. `"redb"` stores keys in an on-disk file. `"redis"` stores keys in a shared Redis keyspace ([ADR-0063](../adr/0063-redis-repository-service.md)). Configure the redis backend with `[default.idempotent_repo]`:

```toml
{{#include ../../../examples/redis-repositories/Camel.toml:idempotent-repo}}
```

The redis repository registers under the name `"redis"`, so a route selects it with `repository = "redis"`. A redis repository also shares deduplication keys across processes.

The Idempotent Consumer differs from the [Claim Check](claim-check.md). Both use a repository trait. The Idempotent Consumer stores only the key. The Claim Check stores the full payload. Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), the consumer is an outcome-aware segment: it compiles into an `OutcomePipeline` segment (`CompiledStep::Segment`) executed inside the route's Tower service. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/idempotent-consumer`](https://github.com/kennycallado/rust-camel/tree/main/examples/idempotent-consumer).
