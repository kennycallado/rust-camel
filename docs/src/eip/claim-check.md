# Claim Check

The Claim Check is a Message Translator from Hohpe and Woolf. It stores the full message payload in a repository and replaces the body with a lightweight claim key. The route carries only the key until a later step retrieves the original body.

```yaml
{{#include ../../../examples/claim-check/routes.yaml:claim-check-route}}
```

The `claim_check` step delegates to a `ClaimCheckRepository` registered on the context. The `set` operation stashes the current body under the given key and replaces the body with that key. The `get` operation reads the key, retrieves the stashed body from the repository, and restores it as the body. The included route sets a placeholder payload, pins the `claimKey` header, then calls `set` to stash the body. The log step after `set` shows the claim key as the new body. The second `claim_check` step calls `get` with the same key, and the final log step shows the restored body.

Use the Claim Check when the payload is large and the route does not need the full body at every step. Store-and-forward flows stash the payload at the ingress. The route carries only the lightweight key through the pipeline. A later step retrieves the payload at the egress before delivery. This keeps the in-memory footprint low across long routes. The repository also supports `get_and_remove`, `push`, and `pop` for stack and queue access patterns.

The Claim Check differs from the [Idempotent Consumer](idempotent-consumer.md). Both patterns use a repository trait for their state. The Idempotent Consumer stores only the deduplication key. The Claim Check stores the full payload. A route that needs to deduplicate and stash uses both patterns. Claim Check alone keeps a heavy payload out of the pipeline.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the claim check step compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The repository trait is defined in [ADR-0028](../adr/0028-claimcheck-repository-trait.md). The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/claim-check`](https://github.com/kennycallado/rust-camel/tree/main/examples/claim-check).
