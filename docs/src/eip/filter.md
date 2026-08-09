# Message Filter

The Message Filter is a Message Router from Hohpe and Woolf. It drops exchanges that fail a predicate. A filter step evaluates a predicate on each exchange and runs its inner steps only when the predicate holds.

```rust,ignore
{{#include ../../../examples/content-based-routing/src/main.rs:filter-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: content-based-routing-demo
  from: timer:tick?period=1000&repeatCount=10
  error_handler:
    retry:
      max_attempts: 1
  steps:
    - filter:
        simple: "${body} == 'important'"
        steps:
          - to: log:filtered?showBody=true&showCorrelationId=true
```

</details>

The `.filter(predicate)` call takes a closure of type `Fn(&Exchange) -> bool`. In the included route the predicate reads `ex.input.body.as_text()` and keeps only exchanges whose body is the string `important`. The `.to(...)` call inside the filter scope is the inner step. It runs for exchanges the predicate accepts. The `.end_filter()` call closes the scope. The `error_handler` that follows then attaches to the route, not to the filter.

When the predicate returns `false`, the filter does not raise an error. It returns `PipelineOutcome::Completed` with the original exchange, and the inner step is skipped. The exchange then continues to any step after `.end_filter()`. In this route the filter is the last step before the error handler, so a filtered exchange simply ends with no log line written.

This is the rule that separates filter from choice. A filter has one branch that runs or skips. A Content-Based Router selects one branch from several. Use filter to gate a single sub-route on a yes-or-no condition. Use [Content-Based Router](content-based-router.md) when you must dispatch to one of many destinations.

Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), the filter compiles into a `FilterSegment` that operates on `PipelineOutcome` directly. The processor contract for the filter is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/content-based-routing`](https://github.com/kennycallado/rust-camel/tree/main/examples/content-based-routing).
