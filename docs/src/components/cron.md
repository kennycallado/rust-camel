# Cron

The cron component fires Exchanges on a Unix 5-field cron schedule. Use it for source routes that run at calendar times, not fixed intervals. The component mirrors the timer structure (Component to Endpoint to Consumer) but delegates scheduling to a pluggable `CronService`.

The cron-example fires a route every minute:

```rust,ignore
{{#include ../../../examples/cron-example/src/main.rs:cron-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: cron-demo
    from: "cron:tick?schedule=*+*+*+*+*"
    steps:
      - set_body: "cron-fired"
      - set_header:
          key: source
          value: "cron"
      - to: "log:cron-result?level=info&showBody=true&showHeaders=true"
```

</details>

## URI

```text
cron:<name>?schedule=<5-field-expr>[&timeZone=<IANA>&includeMetadata=true]
```

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `schedule` | yes | — | Unix 5-field cron expression. Use `+` as the space separator |
| `timeZone` | no | `UTC` | IANA timezone identifier (e.g. `America/New_York`) |
| `includeMetadata` | no | `true` | Attach `CronFire` metadata to each Exchange |

## Schedule format

The schedule is a standard Unix 5-field cron expression:

```text
minute hour day-of-month month day-of-week
```

The URI uses `+` as the space separator (Apache Camel convention). The expression `*+*+*+*+*` fires every minute. Each field supports the standard operators: `*`, ranges (`1-5`), lists (`1,3,5`), and step values (`*/2`).

## Consumer

`cron:tick?schedule=*+*+*+*+*` fires an Exchange on the schedule. The Consumer is a pure source. It reads from no external system. The first fire happens at the next matching schedule time, not at startup.

The Consumer delegates scheduling to `Arc<dyn CronService>`. The default implementation is `TokioCronService`. The service picks the fire time. The `CronConsumer` callback submits the Exchange.

## Misfire behavior

A missed fire is not replayed. When the process is down at a scheduled fire time, the Consumer recomputes the next fire from the current time. This behavior is misfire-skip. Automatic catch-up of batch jobs is dangerous, so the component skips missed fires instead.

## Metadata

Set `includeMetadata=true` to attach `CronFire` metadata to each Exchange:

- `scheduled_at`. The time the fire was scheduled.
- `fired_at`. The actual time the Consumer submitted the Exchange. This differs from `scheduled_at` under load.
- `counter`. The number of fires since the Consumer started.

## Error propagation

The `CronCallback` is an async, fallible closure. On `Err`, the error propagates to Route supervision (ADR-0007). The Route records the failure and, when a supervision policy is configured, restarts the Route.

**Reference**: [cron crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-cron/CONTEXT.md). Example source: [`examples/cron-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/cron-example).
