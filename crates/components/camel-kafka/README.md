# camel-component-kafka

Kafka component for [rust-camel](https://github.com/kennycallado/rust-camel).

## URI Format

```
kafka:topic[?param=value&...]
```

## Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `brokers` | `localhost:9092` | Bootstrap servers |
| `groupId` | `camel` | Consumer group ID |
| `autoOffsetReset` | `latest` | `earliest`/`latest`/`none` |
| `sessionTimeoutMs` | `45000` | Consumer session timeout (ms) |
| `pollTimeoutMs` | `5000` | Poll timeout (ms) |
| `maxPollRecords` | `500` | Max records per poll |
| `acks` | `all` | Producer acknowledgment: `0`/`1`/`all` |
| `requestTimeoutMs` | `30000` | Producer delivery timeout (ms) |

## Headers

| Header | Direction | Description |
|--------|-----------|-------------|
| `CamelKafkaTopic` | In/Out | Topic name |
| `CamelKafkaPartition` | In/Out | Partition number |
| `CamelKafkaOffset` | Out (consumer) | Message offset |
| `CamelKafkaKey` | In/Out | Message key (UTF-8 only; binary keys are logged as warnings and dropped) |
| `CamelKafkaTimestamp` | Out (consumer) | Epoch milliseconds timestamp |
| `CamelKafkaGroupId` | Out (consumer) | Consumer group ID |
| `CamelKafkaRecordMetadata` | Out (producer) | `{topic, partition, offset}` delivery ack |

## Exchange Properties

| Key | Description |
|-----|-------------|
| `kafka.manual.commit` | JSON object: `{topic, partition, offset}` of the consumed record |

## Quick Start

```rust
use camel_component_kafka::KafkaComponent;

ctx.register_component(KafkaComponent::new());

// Consumer
let route = RouteBuilder::from("kafka:orders?brokers=localhost:9092&groupId=my-service")
    .to("log:info")
    .build()?;

// Producer
let route = RouteBuilder::from("timer:tick?period=1000")
    .set_body(Value::String("hello".to_string()))
    .to("kafka:events?brokers=localhost:9092")
    .build()?;
```

## Running the Example

```bash
# Start Kafka
docker run -p 9092:9092 apache/kafka:latest

# Run the example
cargo run -p kafka-example
```

## Integration Tests

```bash
KAFKA_BROKERS=localhost:9092 cargo test -p camel-component-kafka -- --ignored
```

## Known Limitations (Phase 2)

- SSL/SASL authentication not supported
- Batch consumption mode not implemented
- Manual offset commit (beyond metadata storage) is Phase 2
- Multiple parallel consumers (`consumersCount`) not implemented