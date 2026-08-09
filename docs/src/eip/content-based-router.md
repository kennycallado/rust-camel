# Content-Based Router

The Content-Based Router is a Message Router from Hohpe and Woolf. It inspects the exchange and routes it to one destination from a fixed set of branches.

```rust,ignore
{{#include ../../../examples/content-based-router/src/main.rs:cbr-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: content-based-router-demo
  from: timer:tick?period=1000&repeatCount=6
  error_handler:
    retry:
      max_attempts: 1
  steps:
    - choice:
        when:
          - simple: "${body} == 'high'"
            steps:
              - to: log:high-priority?showBody=true&showCorrelationId=true
          - simple: "${body} == 'medium'"
            steps:
              - to: log:medium-priority?showBody=true&showCorrelationId=true
        otherwise:
          - to: log:low-priority?showBody=true&showCorrelationId=true
```

</details>

The included route fires a timer that assigns one of three priority strings to the body. The `.choice()` call opens a routing block. Each `.when(predicate).to(endpoint).end_when()` chain defines one branch. The `.otherwise().to(endpoint).end_otherwise()` chain defines the fallback. The `.end_choice()` call closes the block so the `error_handler` attaches to the route, not to the choice.

The router evaluates the `when` predicates in order and runs the first branch that matches. It short-circuits on the first match, so it skips the remaining branches. The order of your `when` clauses matters when predicates overlap. Put the most specific predicate first. Each predicate is a closure of type `Fn(&Exchange) -> bool`. It can read the body, headers, and properties. The example reads `ex.input.body.as_text()` to dispatch on the string the previous `process` step produced.

If no predicate matches and you omit `otherwise`, the exchange passes through unchanged. It does not stop, and it raises no error. The route continues to the next step. Add an `otherwise` branch when an unmatched exchange must not proceed silently. The choice block composes with other steps. A `process` step inside a branch mutates the body, and the `to` endpoint receives that mutated body. An error handler attached after `end_choice()` catches errors raised inside the chosen branch.

Use the Content-Based Router when the set of destinations is fixed at build time. Use the [Dynamic Router](dynamic-router.md) when the destination is itself data. A header value, a registry lookup, or a computation decides where the exchange goes next. CBR chooses among branches you wrote. The Dynamic Router computes an endpoint at runtime.

Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), the choice compiles into a `ChoiceSegment` that returns `PipelineOutcome` directly, so `Stop` propagates with exchange state intact. Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), that segment runs as a `Service<Exchange>` in the Tower middleware pipeline. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

For how a filter feeds a choice inside the same route, see the [Message Filter](filter.md) page.

The example source is at [`examples/content-based-router`](https://github.com/kennycallado/rust-camel/tree/main/examples/content-based-router).
