# Splitter

The Splitter is a Message Routing pattern from Hohpe and Woolf. It takes one composite message and produces one exchange per fragment.

```rust,ignore
{{#include ../../../examples/splitter/src/main.rs:splitter-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: splitter-demo
  from: timer:batch?period=2000&repeatCount=3
  error_handler:
    retry:
      max_attempts: 1
  steps:
    - set_body:
        value: "alice,100\nbob,200\ncharlie,300"
    - split:
        expression: body_lines
        aggregation: collect_all
        steps:
          - to: log:fragment?showBody=true&showCorrelationId=true
    - to: log:aggregated?showBody=true&showCorrelationId=true
```

</details>

A split expression decides how to divide the body. The included route uses `split_body_lines()`, which returns one fragment per line. Each fragment becomes its own exchange that flows through the sub-pipeline. A fragment inherits the parent headers, properties, message pattern, and OpenTelemetry context, so its span is a child of the parent span. The `CamelSplitIndex`, `CamelSplitSize`, and `CamelSplitComplete` properties mark where each fragment sits in the batch. The `.map_body(...)` step inside the split scope turns each line into a JSON object, and `.end_split()` closes the scope.

When the split scope closes, an aggregation strategy combines the fragment outputs. `CollectAll` gathers every fragment body into a JSON array. That array then flows to the step after `.end_split()`. Per the aggregation contract, failed fragments reach the strategy as `Err(e)` entries in the vector, not as exchanges with an attached exception.

The Splitter differs from Multicast in how it produces children. Multicast sends the same exchange to several endpoints. The Splitter derives one exchange per fragment from a single input. It also pairs with the Aggregator in a split-process-aggregate flow. The Splitter divides one message into many. The Aggregator collects many correlated messages back into one.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the splitter compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The per-fragment sub-pipeline compiles into child steps on the same route channel. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), the split block is an outcome-aware structural EIP. A `Stopped` outcome from the sub-pipeline returns `Stopped(fragment_ex)` and skips aggregation. The parent pipeline never sees a partial batch. The per-fragment boundary keeps the fragment mutations made before the Stop visible to the outer pipeline.

The example source is at [`examples/splitter`](https://github.com/kennycallido/rust-camel/tree/main/examples/splitter).
