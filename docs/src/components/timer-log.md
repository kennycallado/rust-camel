# Timer and log

The timer and log components form the smallest working route. Timer is a pure source. It fires Exchanges on a schedule and reads from no external system. Log is a pure sink. It formats Exchange state and writes the result through `tracing`. Together they exercise the source-to-sink path with no network and no extra dependencies.

The hello-world example wires both components and produces one Exchange per tick:

```rust,ignore
{{#include ../../../examples/hello-world/src/main.rs:first-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: hello-world
    from: "timer:tick?period=1000&repeatCount=5"
    steps:
      - set_header:
          key: source
          value: "timer"
      - to: "log:info?showHeaders=true&showCorrelationId=true"
```

</details>

## Timer

`timer:tick?period=1000&repeatCount=5` fires an Exchange every `period` milliseconds. The first tick fires immediately. The Consumer stops after `repeatCount` fires. Omit `repeatCount` to fire until the Route stops. Set `repeatCount=0` and the timer fires never.

| Parameter | Default | Description |
| --- | --- | --- |
| `period` | `1000` | Interval between ticks in milliseconds |
| `delay` | `0` | Wait before the first tick in milliseconds |
| `repeatCount` | omitted | Tick limit. Omit for infinite. `0` fires never |
| `fixedRate` | `false` | `true` skips missed ticks. `false` fires all missed ticks at once |
| `includeMetadata` | `true` | Attach `CamelTimer*` headers to each Exchange |

Timer is a consumer-only Component. Its Consumer submits one Exchange per tick. The Exchange body holds a short label. When `includeMetadata=true`, the Consumer sets four Exchange headers. They carry the timer name, the tick counter, the ISO-8601 fire time, and the epoch timestamp. Timer does not poll a directory and does not implement `PollingConsumer`. It is event-driven and runs inside the Route lifecycle.

## Log

`log:info?showHeaders=true&showCorrelationId=true` writes the Exchange to the configured `tracing` level. The query parameters select what the Producer prints. `showHeaders=true` adds the Exchange header map. `showCorrelationId=true` prefixes the line with the correlation id, so several Routes can share one log output.

Log is a producer-only Component. Its Producer formats the Exchange body, headers, and correlation id, then returns the Exchange unchanged. The Producer composes with every other pipeline step (ADR-0001). Calling `log:` from a `from:` position returns an error at Endpoint creation.

The Producer exposes five levels: trace, debug, info, warn, error. Each level selects a `tracing` macro. Routes that handle sensitive data should set `logMask=true`. The option replaces the body and matching header values with a redaction marker.

## Putting it together

The example registers both components before the Route starts. The Route flows `from: timer:tick` to `to: log:info`. Each Exchange passes through one Timer Consumer, one `.set_header` step, and one Log Producer. The result is one log line per tick for five ticks.

This pair is the recommended starting template. Once a route reads a header, transforms the body, or routes to more than one sink, replace log with a real sink and timer with a real source. The registration shape and the Endpoint URIs stay the same.

The timer URI grammar lives in the [parent Components authority](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md). The log contract surface lives in the [camel-log CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-log/CONTEXT.md). The example source is at [`examples/hello-world`](https://github.com/kennycallado/rust-camel/tree/main/examples/hello-world).
