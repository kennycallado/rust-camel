# Stop

The Stop step halts the pipeline. No further steps run after a Stop. The
exchange does not raise an error. The pipeline reports the outcome as
`PipelineOutcome::Stopped`. This is a successful termination, not a failure.

```rust,ignore
RouteBuilder::from("timer:tick?period=1000")
    .route_id("stop-demo")
    .filter(|ex| ex.input.body.as_text().map(|t| t.contains("urgent")).unwrap_or(false))
    .to("log:urgent?showBody=true")
    .end_filter()
    .stop()
    .build()?;
```

<details>
<summary>YAML equivalent</summary>

```yaml
- id: stop-demo
  from: timer:tick?period=1000
  steps:
    - filter:
        simple: "${body} contains 'urgent'"
        steps:
          - to: log:urgent?showBody=true
    - stop: true
```

</details>

The `.stop()` call adds a terminal step. When the exchange reaches this step,
the pipeline returns `PipelineOutcome::Stopped` and the route stops processing
that exchange. The exchange state is preserved as-is. The filter in the example
shows the common pattern. Process exchanges that match a condition, then stop.
Exchanges that fail the filter skip the filter block, reach the Stop step, and
halt.

The Stop step is not an EIP. It is a flow control utility. It differs from a
[Message Filter](../eip/filter.md). The Filter drops exchanges that fail the
predicate. Stop halts every exchange that reaches it. A route that needs to stop
conditionally places a filter or choice block before the Stop.

Stop is modeled as `PipelineOutcome::Stopped`, not as `CamelError`. This
distinction matters for error handling. A stopped exchange does not trigger the
route error handler. Place any log or retry step before the Stop step, not in an
error handler.

Per [ADR-0024](../adr/0024-pipeline-outcome-replaces-camel-error-stopped.md),
Stop is a `CompiledStep::Stop` variant. The executor converts it into
`PipelineOutcome::Stopped` without invoking a Tower service. The outcome layer
sits above Tower so the runtime can distinguish a stop from a failure.
