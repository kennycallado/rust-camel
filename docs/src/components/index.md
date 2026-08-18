# Components

Components connect routes to external systems. Each Component owns a URI scheme and creates Endpoints that produce Consumers, Producers, or both. The vocabulary for Component, Endpoint, Consumer, and Producer lives in [`crates/components/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md).

## Catalog

| Scheme | Direction | Authority |
| --- | --- | --- |
| `timer` | consumer | [parent](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md) |
| `log` | producer | [camel-log](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-log/CONTEXT.md) |
| `direct` | both | [camel-direct](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-direct/CONTEXT.md) |
| `seda` | both | [camel-component-seda](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-seda/CONTEXT.md) |
| `controlbus` | producer | [camel-controlbus](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-controlbus/CONTEXT.md) |
| `mock` | both | [parent](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md) |
| `file` | both | [camel-file](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-file/CONTEXT.md) |
| `http`, `http-static` | both | [camel-http](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-http/CONTEXT.md) |
| `ws`, `wss` | both | [camel-ws](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-ws/CONTEXT.md) |
| `grpc`, `grpcs` | both | [camel-component-grpc](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-grpc/CONTEXT.md) |
| `cron` | consumer | [camel-cron](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-cron/CONTEXT.md) |
| `kafka` | both | [camel-kafka](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-kafka/CONTEXT.md) |
| `jms` | both | [camel-jms](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-jms/CONTEXT.md) |
| `mqtt` | both | [camel-mqtt](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-mqtt/CONTEXT.md) |
| `redis`, `rediss` | both | [camel-redis](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-redis/CONTEXT.md) |
| `sql` | both | [camel-sql](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-sql/CONTEXT.md) |
| `surrealdb` | both | [camel-component-surrealdb](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-surrealdb/CONTEXT.md) |
| `opensearch`, `opensearchs` | producer | [camel-opensearch](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-opensearch/CONTEXT.md) |
| `master` | consumer | [parent](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md) |
| `container` | both | [camel-container](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-container/CONTEXT.md) |
| `llm` | producer | [camel-component-llm](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-llm/CONTEXT.md) |
| `mcp` | both | [camel-component-mcp](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-mcp/CONTEXT.md) |
| `exec` | producer | [camel-component-exec](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-exec/CONTEXT.md) |
| `validator` | producer | [camel-validator](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-validator/CONTEXT.md) |
| `xslt` | producer | [camel-xslt](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-xslt/CONTEXT.md) |
| `xj` | producer | [camel-xj](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-xj/CONTEXT.md) |
| `cxf` | both | [camel-cxf](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-cxf/CONTEXT.md) |
| `keycloak` | both | [camel-component-keycloak](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-keycloak/CONTEXT.md) |
| `wasm` | both | [camel-component-wasm](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-wasm/CONTEXT.md) |
| `template` | producer | [parent](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md) |

The table covers every crate under `crates/components/`. The contract crate `camel-component-api` defines the Component SPI and the Consumer, Producer, and Endpoint traits. It registers no URI scheme.

## Direction

**consumer** marks an inbound Component. It starts a Consumer that submits Exchanges into the Route. **producer** marks an outbound Component. It creates a Producer that sends Exchanges to an external system. **both** means the Component supports either direction, one per Endpoint.

`master` wraps a delegate Consumer in a leadership gate. The bridge exposes inbound traffic only while this node holds the leadership lock ([ADR-0035](../adr/0035-leader-epoch-fencing-token.md)).

## Narrative pages

- [Timer and log](timer-log.md). The smallest working route.
- [File](file.md). Directory poller and disk writer.
- [HTTP](http.md). Server Consumer and response handling.
- [gRPC](grpc.md). Service consumer and producer.
- [WebSocket and SOAP](ws-soap.md). Bidirectional WebSocket traffic and SOAP calls through the Java bridge.
- [Kafka](kafka.md). Broker producer and consumer.
- [JMS](jms.md). Java bridge consumer and producer.
- [MQTT](mqtt.md). MQTT 3.1.1 broker producer and consumer.
- [Redis](redis.md). Datastore and pub/sub.
- [Database](database.md). SQL access.
- [SurrealDB](surrealdb.md). Multi-model database.
- [OpenSearch](opensearch.md). Search and indexing.
- [LLM](llm.md). Chat completions and embeddings.
- [MCP](mcp.md). Model Context Protocol server and client.
- [WASM](wasm.md). Sandboxed plugins with capability model.
- [Cron](cron.md). Scheduled message generation.
- [Direct](direct.md). Synchronous in-process routing.
- [SEDA](seda.md). Asynchronous staging between routes.
- [ControlBus](controlbus.md). Runtime control messages.
- [Master](master.md). Leader-only route execution.
- [Template](template.md). External template rendering.
- [Validator](validator.md). Schema validation.
- [Exec](exec.md). External process execution.
- [Keycloak](keycloak.md). OIDC auth and JWKS validation.
- [XML transform](xml-transform.md). XSLT and JSON-XML conversion.
- [Mock](mock.md). Testing assertions.
