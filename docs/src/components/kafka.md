# Kafka

The Kafka component produces to and consumes from Apache Kafka topics. One crate covers both directions. The Consumer subscribes to topics and submits one Exchange per record. The Producer publishes the Exchange body to a topic.

The kafka-example wires a timer-driven producer and a log consumer against a testcontainers broker:

```rust,ignore
{{#include ../../../examples/kafka-example/src/main.rs:kafka-producer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: kafka-producer
    from: "timer:tick?period=3000"
    steps:
      - set_body: '{"event":"heartbeat","source":"rust-camel"}'
      - to: "kafka:orders?brokers=127.0.0.1:9092&acks=all"
```

The Rust example reads the broker port from a testcontainers container. Substitute your real broker address in `brokers`.

</details>

```rust,ignore
{{#include ../../../examples/kafka-example/src/main.rs:kafka-consumer-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: kafka-consumer
    from: "kafka:orders?brokers=127.0.0.1:9092&groupId=example-group&autoOffsetReset=earliest"
    steps:
      - to: "log:info?showHeaders=true"
```

The Rust example reads the broker port from a testcontainers container. Substitute your real broker address in `brokers`.

</details>

## URI

```
kafka:<topic>?brokers=<host:port>[&groupId=<group>][&autoOffsetReset=<policy>][&partitionAssignmentStrategy=<strategy>][&acks=<level>][&securityProtocol=<protocol>]
```

| Parameter | Required | Default | Description |
| --- | --- | --- | --- |
| `brokers` | yes | `localhost:9092` | Comma-separated `host:port` broker addresses |
| `groupId` | consumer | `camel` | Consumer group ID for coordinated consumption |
| `autoOffsetReset` | consumer | `latest` | Offset reset policy: `earliest`, `latest`, or `none` |
| `partitionAssignmentStrategy` | consumer | `range` | `range`, `roundRobin`, or `cooperativeSticky` |
| `acks` | producer | `all` | Durability level: `all`, `1`, or `0` |
| `securityProtocol` | no | `PLAINTEXT` | `PLAINTEXT`, `SSL`, `SASL_PLAINTEXT`, or `SASL_SSL` |

## Consumer

`kafka:<topic>?brokers=localhost:9092&groupId=my-group` subscribes to a topic. The Consumer submits one Exchange per record. The Exchange body carries the record value. The headers carry the topic, partition, offset, key, and timestamp (`CamelKafkaTopic`, `CamelKafkaPartition`, `CamelKafkaOffset`, `CamelKafkaKey`, `CamelKafkaTimestamp`).

The `groupId` coordinates consumption across instances. Consumers that share a group ID split the topic partitions. The `partitionAssignmentStrategy` picks how the broker assigns partitions across members: `range` (default), `roundRobin`, or `cooperativeSticky`. The `autoOffsetReset` policy picks the start position when the group has no committed offset: `latest` (default), `earliest`, or `none`.

Offset commit has two modes. Auto-commit is the default. The Consumer commits offsets on its auto-commit interval. Set `allowManualCommit=true` to commit from the route instead. The route reads a `KafkaManualCommit` handle from the `kafka.manual_commit` exchange property and calls `commit_async()` after it processes the record.

The Consumer uses the standard push model. It does not implement `PollingConsumer`. The consumer task starts with the Route and runs until the Route stops or reconnect attempts exhaust.

## Producer

`kafka:<topic>?brokers=localhost:9092&acks=all` sends the Exchange body to a topic. The `acks` parameter controls durability. `all` waits for every in-sync replica. `1` waits for the leader only. `0` fires and forgets. On success the Producer returns the Exchange unchanged and writes delivery metadata to the `CamelKafkaRecordMetadata` header.

A send failure returns `Err`. The pipeline catches it and the route `ErrorHandler` owns the operational signal.

## Security

The component supports four security protocols:

- **PLAINTEXT** (default). No encryption. The component warns at startup.
- **SSL**. TLS encryption. Gated behind the `ssl` or `ssl-vendored` cargo feature.
- **SASL_PLAINTEXT**. SASL authentication without encryption. Gated behind the `sasl` feature.
- **SASL_SSL**. SASL authentication with TLS. Requires both the `sasl` and `ssl` features.

The component redacts SASL and SSL passwords to `[REDACTED]` in `Debug` output. A missing feature gate stops startup with the required `cargo add` command.

## Error handling

The Consumer logs at `error!` for auto-commit failures, manual-commit handler failures, and exhausted reconnect attempts. ADR-0012 classifies these as outside-contract (b') and system-broken (c). The Producer logs send failures at `warn!`. The route handler owns these failures (ADR-0012 category a).

**Reference**: [Kafka crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-kafka/CONTEXT.md). Example source: [`examples/kafka-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/kafka-example).
