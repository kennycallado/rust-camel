# Sampling

The Sampling is a Message Router from Hohpe and Woolf. It passes one exchange out of every N and drops the rest. The route downstream sees a subsample of the upstream traffic.

```yaml
{{#include ../../../examples/sampling/routes.yaml:sampling-route}}
```

The sample period is an integer. `sampling: 3` accepts one exchange out of every three and drops the other two. The counter starts at zero. The step increments it before the modulo check. Exchange number three is the first one that passes, then six, then nine. A period of one passes every exchange. The step rejects a period of zero at build time.

The step sets the `CamelStop` header on a dropped exchange. The surrounding pipeline translates that header into `PipelineOutcome::Stopped`. The steps after the sampling step do not run for that exchange. The included route fires a timer nine times. The log step runs three times.

Use the Sampling when the downstream rate is more than the route needs. Load shedding drops traffic before it reaches expensive steps. A monitoring or debugging route can sample a feed so the logs stay readable under high volume. A period-N sampler reduces the rate by an integer factor without the state of a rate limiter.

The Sampling differs from the [Throttler](throttler.md). The Sampling is stateless and uses a counter. The Throttler paces the rate over a sliding time window and queues excess exchanges. A route that needs both patterns samples first, then throttles.

Per [ADR-0001](../adr/0001-tower-data-plane-split-from-control-plane.md), the sampling step compiles into a `Service<Exchange>` step in the Tower middleware pipeline. The processor contract is documented in [camel-processor/CONTEXT.md](https://github.com/kennycallado/rust-camel/blob/main/crates/camel-processor/CONTEXT.md).

The example source is at [`examples/sampling`](https://github.com/kennycallado/rust-camel/tree/main/examples/sampling).
