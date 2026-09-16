# Components

Components connect routes to external systems. Each Component owns a URI scheme and creates Endpoints that produce Consumers, Producers, or both. The vocabulary for Component, Endpoint, Consumer, and Producer lives in [`crates/components/CONTEXT.md`](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md).

## Catalog

| Scheme | Direction | Authority |
| --- | --- | --- |
| `timer` | consumer | [camel-timer](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-timer/CONTEXT.md) |
| `log` | producer | [camel-log](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-log/CONTEXT.md) |
| `direct` | both | [camel-direct](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-direct/CONTEXT.md) |
| `seda` | both | [camel-component-seda](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-seda/CONTEXT.md) |
| `controlbus` | producer | [camel-controlbus](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-controlbus/CONTEXT.md) |
| `mock` | producer | [parent](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md) |
| `file` | both | [camel-file](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-file/CONTEXT.md) |
| `http`, `https`, `http-static` | both | [camel-http](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-http/CONTEXT.md) |
| `ws`, `wss` | both | [camel-ws](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-ws/CONTEXT.md) |
| `grpc` | both | [camel-component-grpc](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-grpc/CONTEXT.md) |
| `cron` | consumer | [camel-cron](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-cron/CONTEXT.md) |
| `kafka` | both | [camel-kafka](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-kafka/CONTEXT.md) |
| `jms`, `activemq`, `artemis` | both | [camel-jms](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-jms/CONTEXT.md) |
| `mqtt` | both | [camel-mqtt](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-mqtt/CONTEXT.md) |
| `redis`, `redis-sentinel`, `rediss-sentinel` | both | [camel-redis](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-redis/CONTEXT.md) |
| `sql` | both | [camel-sql](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-sql/CONTEXT.md) |
| `surrealdb` | both | [camel-component-surrealdb](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-component-surrealdb/CONTEXT.md) |
| `opensearch`, `opensearchs` | producer | [camel-opensearch](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-opensearch/CONTEXT.md) |
| `master` | both | [parent](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md) |
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

The table covers every crate under `crates/components/` and every registered URI scheme. The contract crate `camel-component-api` defines the Component SPI and the Consumer, Producer, and Endpoint traits. It registers no URI scheme. The `mock` and `template` components have no per-crate CONTEXT.md; their parent entry is the available authority.

## Direction

**consumer** marks an inbound Component. It starts a Consumer that submits Exchanges into the Route. **producer** marks an outbound Component. It creates a Producer that sends Exchanges to an external system. **both** means the Component supports either direction, one per Endpoint.

`master` wraps a delegate Consumer in a leadership gate. The bridge exposes inbound traffic only while this node holds the leadership lock ([ADR-0035](../adr/0035-leader-epoch-fencing-token.md)).

## Families

- [In-process routing](in-process.md). Components that move Exchanges between routes inside one process.
- [Network endpoints](network.md). Components that connect routes to remote systems over a network.
- [Messaging brokers](brokers.md). Components that exchange messages through a broker.
- [Data stores](datastores.md). Components that read from and write to external data stores.
- [Files, scheduling, and processes](local.md). Components that work with the local machine.
- [AI and extension](ai.md). Components that add AI and extension capabilities.
- [Validation, testing, and rendering](testing.md). Components that validate, test, or render Exchange content.
