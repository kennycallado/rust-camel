# Routes and pipelines

A Route is a source endpoint followed by an ordered list of steps. Each step is a Processor that receives an [Exchange](exchange-message.md), transforms it, and returns it. The final step forwards the result to a sink.

## Route

A Route is a named message-processing pipeline. It pairs a source endpoint that emits Exchanges with a sequence of steps that transform or route them. The [Runtime](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-core/CONTEXT.md) owns the definition.

```rust,ignore
{{#include ../../../examples/hello-world/src/main.rs:first-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: "hello-world"
    from: "timer:tick?period=1000&repeatCount=5"
    steps:
      - set_header:
          key: "source"
          value: "timer"
      - to: "log:info?showHeaders=true&showCorrelationId=true"
```

</details>

The include shows both ends of a Route. `RouteBuilder::from("timer:tick?...")` is the source endpoint. `.to("log:info?...")` is the sink. The `.set_header(...)` and `.to(...)` calls between them are the ordered steps. The source fires a new Exchange per timer tick. Each step runs in order. The sink receives the final state.

## Pipeline

A Pipeline is the compiled assembly of Processors that processes an Exchange through a Route. Each Processor is a single processing unit. It can be an EIP pattern (filter, choice, split, setBody) or a custom step that receives and returns an Exchange.

The data plane runs on Tower ([ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md)). Every processor and producer is a `Service<Exchange>`. The Tower `Layer` trait composes these services into a chain at build time. This is the architectural advantage over Apache Camel. Middleware, backpressure, timeout, and cancellation compose through one uniform trait instead of ad-hoc hooks.

## Pipeline outcome and flow control

The pipeline executor produces a `PipelineOutcome` ([ADR-0024](../adr/0024-pipeline-outcome-replaces-camel-error-stopped.md)) with three variants:

| Variant | Means | Reply channel sees |
|---|---|---|
| `Completed(Exchange)` | The pipeline ran to the end | `Ok(ex)` |
| `Stopped(Exchange)` | A step ended the route early with `Stop`. Successful control flow, not an error | `Ok(ex)` |
| `Failed(CamelError)` | A step returned an error and no handler absorbed it | `Err(err)` |

`PipelineOutcome` sits one layer above Tower. Tower `Service<Exchange>` responses stay `Result<Exchange, CamelError>`. A single adapter inside `SequentialPipeline::call` translates `PipelineOutcome` to `Result`. `Completed` and `Stopped` both become `Ok`. The consumer reply channel cannot distinguish them. It builds the response from the Exchange state in both cases.

`Stop` is successful control flow, not a failure ([ADR-0024](../adr/0024-pipeline-outcome-replaces-camel-error-stopped.md)). The Exchange carries every mutation made before the Stop step. The error handler never runs for a Stop.

## Structural EIPs

Steps come in two shapes. A leaf EIP (setBody, log, marshal) is one Processor that maps an Exchange to an Exchange. A structural EIP (Filter, Choice, Loop, Throttle, doTry, Split, Multicast, LoadBalance) contains child steps that form a sub-pipeline.

```rust,ignore
{{#include ../../../examples/content-based-router/src/main.rs:cbr-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: "content-based-router-demo"
    from: "timer:tick?period=1000&repeatCount=6"
    error_handler:
      on_exceptions:
        - retry:
            max_attempts: 1
    steps:
      # The Rust closure that rotates the body becomes a registered bean.
      - bean:
          name: "set-rotating-priority"
          method: "process"
      - choice:
          when:
            - simple: "${body} == 'high'"
              steps:
                - to: "log:high-priority?showBody=true&showCorrelationId=true"
            - simple: "${body} == 'medium'"
              steps:
                - to: "log:medium-priority?showBody=true&showCorrelationId=true"
          otherwise:
            - to: "log:low-priority?showBody=true&showCorrelationId=true"
```

</details>

The include shows a Choice segment with three branches. Each `.when(...)` arm compiles to a sub-pipeline. The Choice evaluates predicates in order. It runs the first matching branch and skips the rest.

Structural EIPs return `PipelineOutcome` directly through the `OutcomePipeline` trait ([ADR-0025](../adr/0025-outcome-aware-structural-eips.md)). Each wraps its body as an `OutcomeSegment`, stored in a `CompiledStep::Segment` variant. This lets `Stopped(ex)` propagate out of the sub-pipeline with Exchange state intact. A `.stop()` inside any branch halts the entire route, not just the branch.

For the full processor catalog, see the [Processor crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

**Reference**: [Runtime](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-core/CONTEXT.md) · [Processor crate](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md)
