# Resequencer

The Resequencer is a Message Router from Hohpe and Woolf. It reorders a stream of exchanges into a defined sequence. The route downstream sees the exchanges in order even when the upstream side delivers them out of order.

```yaml
{{#include ../../../examples/resequencer/routes.yaml:resequencer-route}}
```

The batch policy buffers exchanges per correlation key. Each exchange enters the bucket that matches its correlation header. The bucket completes when it reaches its size limit or when the timeout window elapses. At completion, the policy sorts the buffered exchanges by the sort expression. It emits the sorted exchanges as an ordered burst. The included route copies the timer counter into a `seq` header and tags each exchange with a `region` correlation key. The `resequence.batch` step releases three sorted exchanges at a time.

The sort expression extracts a comparable value from each exchange. The route uses `${header.seq}`, so the release order matches the sequence numbers. Exchanges that arrive out of order wait in the buffer until the window completes. The policy then sorts them into the correct sequence before release.

The Resequencer differs from the [Sort](sort.md). The Sort orders the elements of one body array in place. The Resequencer buffers separate out-of-order exchanges across many messages, then releases them in sequence. Use Sort when one exchange carries a collection. Use Resequencer when many exchanges arrive out of sequence. The Resequencer also differs from the [Aggregator](aggregator.md). The Aggregator fuses many exchanges into one. The Resequencer keeps one exchange per output and only changes the order. A route that needs both patterns places a Resequencer before an Aggregator.

Per [ADR-0029](../adr/0029-resequencer-continuation-boundary.md), the resequencer is a continuation boundary. The main pipeline ends at the resequencer. A post-driver task runs the steps after the resequencer on each sorted emission. Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), the batch and stream policies propagate `PipelineOutcome::Stopped` through the post-continuation. Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the pre-steps compile into the main `Service<Exchange>` pipeline and the post-steps compile into a continuation processor. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/resequencer`](https://github.com/kennycallado/rust-camel/tree/main/examples/resequencer).
