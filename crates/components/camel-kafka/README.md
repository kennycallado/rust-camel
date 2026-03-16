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
| `allowManualCommit` | `false` | Enable manual offset commit via `KafkaManualCommit` |
| `securityProtocol` | — | `PLAINTEXT`/`SSL`/`SASL_PLAINTEXT`/`SASL_SSL` |
| `saslAuthType` | — | `PLAIN`/`SCRAM_SHA_256`/`SCRAM_SHA_512`/`SSL` |
| `saslUsername` | — | SASL username (required when `saslAuthType` is PLAIN/SCRAM) |
| `saslPassword` | — | SASL password (required when `saslAuthType` is PLAIN/SCRAM) |
| `sslKeystoreLocation` | — | Path to client keystore (PEM/PKCS12) |
| `sslTruststoreLocation` | — | Path to CA truststore |

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
| `kafka.manual_commit` | `KafkaManualCommit` handle (present when `allowManualCommit=true`); call `.commit()` or `.commit_async()` to ack the offset |

## Security

```
# SASL/SCRAM over TLS
kafka:orders?securityProtocol=SASL_SSL&saslAuthType=SCRAM_SHA_512&saslUsername=user&saslPassword=pass

# TLS client certificate
kafka:orders?securityProtocol=SSL&sslKeystoreLocation=/certs/client.pem&sslTruststoreLocation=/certs/ca.pem
```

## Manual Offset Commit

```rust
let route = RouteBuilder::from("kafka:orders?brokers=localhost:9092&allowManualCommit=true")
    .process(|exchange| Box::pin(async move {
        // ... process exchange ...

        // Commit only after successful processing
        if let Some(mc) = exchange.extensions.get("kafka.manual_commit") {
            let commit: KafkaManualCommit = mc.clone().downcast().unwrap();
            commit.commit_async().await?;
        }
        Ok(exchange)
    }))
    .build()?;
```

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

## Global Configuration

Configure default Kafka settings in `Camel.toml` that apply to all Kafka endpoints:

```toml
[default.components.kafka]
brokers = "localhost:9092"           # Bootstrap servers (default: localhost:9092)
group_id = "camel"                   # Consumer group ID (default: camel)
session_timeout_ms = 45000           # Consumer session timeout (default: 45000)
request_timeout_ms = 30000           # Producer request timeout (default: 30000)
auto_offset_reset = "latest"         # Offset reset policy (default: latest)
security_protocol = "plaintext"      # Security protocol (default: plaintext)
```

URI parameters always override global defaults:

```rust
// Uses global brokers (localhost:9092) but overrides groupId
.to("kafka:orders?groupId=my-service")

// Overrides both global settings
.to("kafka:orders?brokers=prod-kafka:9092&groupId=my-service")
```

### Profile-Specific Configuration

```toml
[default.components.kafka]
brokers = "localhost:9092"
group_id = "camel-dev"

[production.components.kafka]
brokers = "kafka-prod-1:9092,kafka-prod-2:9092"
group_id = "camel-prod"
security_protocol = "SASL_SSL"
```

## Known Limitations

- Batch consumption mode not implemented
- Multiple parallel consumers (`consumersCount`) not implemented