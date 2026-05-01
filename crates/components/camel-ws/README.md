# camel-component-ws

> WebSocket client/server component for rust-camel

## Overview

`camel-component-ws` adds WebSocket endpoints to rust-camel for both inbound server consumers and outbound client producers.

## URI Format

```text
ws://host:port/path[?options]
wss://host:port/path[?options]
```

## Features

- **Server Consumer**: accept incoming WebSocket connections and feed route exchanges
- **Client Producer**: connect to remote WebSocket endpoints and send/receive frames
- **Server-send mode**: send to all local clients (broadcast) or specific connections
- **TLS**: secure server/client URIs via `wss://` (rustls, no OpenSSL dependency)
- **Runtime enforcement**: max connections, message size, heartbeat, and idle timeout

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
camel-component-ws = "*"
```

## URI Parameters

| Parameter | Default | Description |
|-----------|---------|-------------|
| `maxConnections` | `100` | Maximum concurrent connections for the endpoint path |
| `maxMessageSize` | `65536` | Maximum inbound message size (bytes) |
| `sendToAll` | `false` | Default producer behavior for local server-send mode |
| `heartbeatIntervalMs` | `0` | Ping interval in milliseconds (`0` disables heartbeat) |
| `idleTimeoutMs` | `0` | Idle timeout in milliseconds (`0` disables timeout) |
| `connectTimeoutMs` | `10000` | Client connect timeout in milliseconds (10s) |
| `responseTimeoutMs` | `30000` | Client response timeout in milliseconds |
| `allowOrigin` | `*` | Allowed Origin header for WS upgrade requests |
| `tlsCert` | _(none)_ | TLS certificate path (required for `wss` consumer) |
| `tlsKey` | _(none)_ | TLS private key path (required for `wss` consumer) |

## Exchange Headers

| Header | Direction | Description |
|--------|-----------|-------------|
| `CamelWsConnectionKey` | Consumer → route | Unique connection key for the current client |
| `CamelWsPath` | Consumer → route | Request path that received the message |
| `CamelWsRemoteAddress` | Consumer → route | Remote peer socket address |
| `CamelWsSendToAll` | Route → producer | If `true`, producer broadcasts to all local clients |
| `sendToAll` | Route → producer | Fallback for `CamelWsSendToAll` (checked if not set) |
| `CamelWsMessageType` | Route → producer | Outbound type: `text` (default) or `binary` |

## Usage (DSL)

### WebSocket Consumer Route

```rust
use camel_builder::RouteBuilder;
use camel_component_ws::WsComponent;
use camel_core::CamelContext;

let mut ctx = CamelContext::new();
ctx.register_component("ws", Box::new(WsComponent));

ctx.add_route(
    RouteBuilder::from("ws://0.0.0.0:9000/chat?maxConnections=200")
        .process(|ex| async move {
            // ex.input contains CamelWsConnectionKey / CamelWsPath / CamelWsRemoteAddress
            Ok(ex)
        })
        .build()?
).await?;
```

### WebSocket Client Producer

```rust
let route = RouteBuilder::from("direct:send")
    .to("ws://127.0.0.1:9000/chat?responseTimeoutMs=5000")
    .build()?;
```

### Local Server Broadcast

```rust
use serde_json::Value;

let route = RouteBuilder::from("direct:broadcast")
    .set_header("CamelWsSendToAll", Value::Bool(true))
    .set_header("CamelWsMessageType", Value::String("text".into()))
    .to("ws://127.0.0.1:9000/chat")
    .build()?;
```

### Local Server Targeted Send

```rust
use serde_json::Value;

let route = RouteBuilder::from("direct:target")
    .set_header(
        "CamelWsConnectionKey",
        Value::String("conn-a,conn-b".to_string())
    )
    .to("ws://127.0.0.1:9000/chat")
    .build()?;
```

### Secure WSS Consumer

```rust
let route = RouteBuilder::from(
    "wss://0.0.0.0:9443/secure?tlsCert=/etc/certs/server.pem&tlsKey=/etc/certs/server.key"
)
.build()?;
```

## Camel.toml Example

```toml
[default.components.ws]
max_connections = 200
max_message_size = 131072
heartbeat_interval_ms = 15000
idle_timeout_ms = 60000
connect_timeout_ms = 5000
response_timeout_ms = 10000
```

URI parameters still take precedence over global defaults.

> **Note:** Global config from Camel.toml is parsed but not yet applied to endpoints. Currently only URI parameters and code defaults are active. Full 3-tier override (defaults → Camel.toml → URI) is planned.

## Documentation

- [API Documentation](https://docs.rs/camel-component-ws)
- [Repository](https://github.com/kennycallado/rust-camel)

## License

Apache-2.0

## Contributing

Contributions are welcome! Please see the [main repository](https://github.com/kennycallado/rust-camel) for details.
