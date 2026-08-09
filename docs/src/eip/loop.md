# Loop

The Loop is a System Management pattern from Hohpe and Woolf. It repeats a block of steps a fixed number of times for each incoming exchange. The body or state can change one iteration at a time.

```rust,ignore
{{#include ../../../examples/loop/src/main.rs:loop-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: loop-count-demo
  from: timer:tick?period=3000&repeatCount=3
  steps:
    - set_body:
        value: hello
    - loop:
        count: 3
        steps:
          - to: log:loop-iteration
    - to: log:loop-result?level=info&showBody=true
```

</details>

The included route fires a timer three times. The `.set_body("hello")` step sets the body to a short string. The `.loop_count(3)` step opens the loop and `.end_loop()` closes it. The `.process(...)` step inside the loop appends one `!` to the body on each pass. After three iterations the body becomes `hello!!!`. The `log:loop-result` step after the loop logs the final body. In the YAML DSL the loop count is the `count` field under `loop`. The count is fixed before the route starts.

Each iteration runs the wrapped sub-pipeline against the same exchange. A mutating step builds on the result of the previous pass. The loop acts as a fold over a fixed range. Use a loop for retry-with-backoff sequences, batch enrichment, or pagination that fetches one page per iteration. The Loop differs from the Splitter. The Loop produces a single output exchange with accumulated state. The Splitter produces many output exchanges, one per fragment.

Per [ADR-0025](../adr/0025-outcome-aware-structural-eips.md), the loop is an outcome-aware structural EIP. A `Stopped` outcome from the inner sub-pipeline returns `Stopped(ex)` and skips the remaining iterations. The parent pipeline never sees a half-applied loop body. Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the loop compiles into a `Service<Exchange>` step. The per-iteration sub-pipeline compiles into child steps on the same route channel. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/loop`](https://github.com/kennycallado/rust-camel/tree/main/examples/loop).
