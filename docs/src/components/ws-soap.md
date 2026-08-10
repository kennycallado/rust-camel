# WebSocket and SOAP (CXF)

The WebSocket component (`ws`, `wss`) serves and connects to real-time bidirectional endpoints. The CXF component (`cxf`) calls and hosts SOAP services through a supervised Java bridge. Both speak plain URI syntax. No code generation. No SOAP engine in the Rust data plane.

The ws-server example wires a chat broadcast and a per-connection echo against the same port:

```rust,ignore
use camel_api::{CamelError, Value};
use camel_builder::{RouteBuilder, StepAccumulator};
use camel_component_ws::WsComponent;
use camel_core::context::CamelContext;

#[tokio::main]
async fn main() -> Result<(), CamelError> {
    let mut ctx = CamelContext::builder().build().await.unwrap();
    ctx.register_component(WsComponent::new());

    // Echo: CamelWsConnectionKey arrives on the inbound message; the producer
    // targets the same key on the same path, so the reply reaches the sender.
    let echo_route = RouteBuilder::from("ws://0.0.0.0:9000/echo")
        .route_id("ws-echo")
        .to("ws://0.0.0.0:9000/echo")
        .build()?;

    // Chat: CamelWsSendToAll=true tells the producer to fan out to every
    // local client connected to /chat.
    let chat_route = RouteBuilder::from("ws://0.0.0.0:9000/chat")
        .route_id("ws-chat")
        .set_header("CamelWsSendToAll", Value::Bool(true))
        .to("ws://0.0.0.0:9000/chat")
        .build()?;

    ctx.add_route_definition(echo_route).await?;
    ctx.add_route_definition(chat_route).await?;
    ctx.start().await?;
    Ok(())
}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: ws-echo
    from: "ws://0.0.0.0:9000/echo"
    steps:
      - to: "ws://0.0.0.0:9000/echo"
  - id: ws-chat
    from: "ws://0.0.0.0:9000/chat"
    steps:
      - set_header:
          name: "CamelWsSendToAll"
          value: "true"
      - to: "ws://0.0.0.0:9000/chat"
```

The echo route replies to the sender. The chat route broadcasts to every local client on `/chat`. The two routes share one port because `ServerRegistry` keys servers by port and paths register independently.

</details>

## WebSocket URI

```text
ws://host:port/path[?options]
wss://host:port/path[?options]
```

| Parameter | Default | Description |
| --- | --- | --- |
| `maxConnections` | `100` | Maximum concurrent connections for the path |
| `maxMessageSize` | `65536` | Inbound message size limit in bytes |
| `heartbeatIntervalMs` | `0` | Ping interval in ms; `0` disables heartbeat |
| `idleTimeoutMs` | `0` | Idle close timeout in ms; `0` disables it |
| `connectTimeoutMs` | `10000` | Client connect timeout in ms |
| `responseTimeoutMs` | `30000` | Client response timeout in ms |
| `allowOrigin` | `*` | Allowed `Origin` header for upgrade requests |
| `tlsCert` | required for `wss` | Path to TLS certificate |
| `tlsKey` | required for `wss` | Path to TLS private key |

TLS uses rustls. No OpenSSL dependency.

## WebSocket consumer

`ws://0.0.0.0:9000/chat` accepts inbound upgrade requests and submits one Exchange per received frame. The Consumer sets three headers on every inbound message:

| Header | Value |
| --- | --- |
| `CamelWsConnectionKey` | UUID that identifies the connection for targeted replies |
| `CamelWsPath` | URL path that received the frame |
| `CamelWsRemoteAddress` | Peer socket address as a string |

A `ServerRegistry` keys servers by port. The first Consumer to register on a port fixes the host and TLS mode. A later registration with the other TLS mode fails (`Server on port N already running with different TLS mode`). Multiple Consumers on the same port share one server and register independent paths. Consumer shutdown removes the path, the security policy, and the connection registry. The server stays alive until the process exits.

The upgrade handler (`dispatch_handler`) checks `Origin` against `allowOrigin` before any auth step. When a `SecurityContext` is attached, the handler fails closed on missing credentials, denied policy decisions, and future `AuthorizationDecision` variants. Query-token values are redacted before logging (ADR-0051). TLS certificate and key paths in `WsEndpointConfig::fmt` are redacted as well. The crate applies a stricter diagnostic policy than ADR-0051 because the cert and key paths sit on the same boundary.

## WebSocket producer

The Producer has two modes. The mode is selected by the inbound Exchange and the URI:

- **Outbound client mode** when no local Consumer matches the URI. The Producer opens a Tokio-tungstenite connection, sends the body, and reads the first reply.
- **Server-send mode** when a local Consumer matches the URI. The Producer never opens a new socket. It writes to the local connection registry.

The mode picker inspects three signals: the `CamelWsSendToAll` header, the `CamelWsConnectionKey` header, and the presence of a local Consumer on `(host, port, path)`. Any one of them selects server-send mode. The producer then targets the keys in `CamelWsConnectionKey` (comma-separated) or, with `CamelWsSendToAll=true`, every active connection on the path.

The body becomes a `Text` frame by default. Set `CamelWsMessageType` to `binary` for a `Binary` frame. The URI parameter `binaryPayload=true` makes binary the default for that endpoint.

The client mode reconnects on transient failures with a shared `NetworkRetryPolicy`. The producer also surfaces backpressure: when a server-send channel is full, it sets `CamelWsDeliveryDropped` and returns `Err` on the next `poll_ready` call.

## WebSocket security

Auth rides on the per-route `SecurityContext` (see [Authentication and authorization](../services/auth.md)). When a path has a context, the upgrade handler authenticates the bearer token and evaluates the route's `SecurityPolicy` before completing the upgrade. Failed auth returns `401`. Failed policy returns `403`. Policy evaluation errors return `500` and increment the `e:ws:policy-eval` metric (ADR-0012 class e). The health pin becomes the operator signal for bind failures (ADR-0012 class g).

## CXF URI

```text
cxf://http://host:port/path?wsdl=file.wsdl&service={ns}Name&port={ns}Port[&operation=opName][&profile=profile_name][&timeout_ms=N][&mtom_enabled=true|false]
```

| Parameter | Required | Description |
| --- | --- | --- |
| `wsdl` | yes | Path to the WSDL file |
| `service` | yes | Service name in `{namespace}Name` form |
| `port` | yes | Port name in `{namespace}Name` form |
| `operation` | no | SOAP operation; falls back to the `CamelCxfOperation` header |
| `profile` | yes (resolver) | Profile name from `Camel.toml` |
| `timeout_ms` | no | Per-request timeout in ms |
| `mtom_enabled` | no | `true` sets `multipart/related` and `SOAPAction` headers; MTOM encoding is partial ([CXF-014]) |
| `address` (path) | yes | Producer: SOAP target URL. Consumer: bind address override |

The Rust component treats the SOAP envelope and WSDL as opaque bytes. No XML parser runs in this crate. The supervised Java bridge owns parsing, DTD handling, and entity resolution. The XXE boundary lives in the bridge process (ADR-0032).

The cxf-example ships a producer that fires every ten seconds and a consumer that returns a fixed response:

```yaml
routes:
  - id: cxf-producer-hello
    from:
      uri: "timer:hello-tick?period=10000"
    steps:
      - set-body:
          constant: "<sayHello><name>World</name></sayHello>"
      - to: "cxf://http://localhost:8080/hello?wsdl=wsdl/hello.wsdl&service={http://example.com/hello}HelloService&port={http://example.com/hello}HelloPort&operation=sayHello"
      - log: "SOAP response: ${body}"
```

<details>
<summary>Consumer route (YAML)</summary>

```yaml
routes:
  - id: cxf-consumer-hello
    from:
      uri: "cxf://http://0.0.0.0:9090/hello?wsdl=wsdl/hello.wsdl&service={http://example.com/hello}HelloService&port={http://example.com/hello}HelloPort"
    steps:
      - log: "Received SOAP request: ${body}"
      - set-body:
          simple: "<sayHelloResponse><message>Hello from rust-camel!</message></sayHelloResponse>"
```

The body of the response Exchange is what the bridge writes back as the SOAP body. The bridge wraps it in an envelope and signs it with the profile's certificates.

</details>

The example ships with `Camel.toml` that declares the profile:

```toml
[components.cxf]
version = "0.8.1"

[[components.cxf.profiles]]
wsdl_path = "wsdl/hello.wsdl"
service_name = "{http://example.com/hello}HelloService"
port_name = "{http://example.com/hello}HelloPort"
```

## CXF bridge

The component spawns a single GraalVM native Java bridge process on first use. The Rust side calls `ensure_binary_for_spec` to download (or reuse) the bridge binary, then starts it with `CXF_PROFILES` and `CXF_PROFILE_<NAME>_*` environment variables. The bridge prints a JSON readiness line on stdout; the Rust side reads the ephemeral gRPC port and the health endpoint URL from that line.

Profile names must match `[a-z0-9_]+`. The validator rejects uppercase, hyphens, and spaces at config load time. Each profile owns its own WSDL, service, port, and optional keystore and truststore. Multiple profiles share one bridge process.

A background health monitor probes the bridge on a configurable interval (default 5s). On a failed health check the monitor moves the slot to `Degraded` and triggers a restart with exponential backoff (capped at 30s). After 10 failed restart attempts the monitor transitions the slot to permanent `Degraded`. The route startup fails fast when the initial probe returns `Degraded` or `Stopped`.

On shutdown the order matters. Call `pool.begin_shutdown()` before `ctx.stop()` so the health monitor does not race a restart against shutdown. The CLI handles this for you with Ctrl+C.

## CXF producer

`cxf://http://host:port/path?...` sends the Exchange body as a SOAP request. The body bytes go over gRPC with the configured `security_profile` selector. The bridge resolves the profile, signs the envelope with the profile's certificate when WS-Security is configured, and POSTs the request to the SOAP target. The response bytes come back as the Exchange body.

The `profile` query parameter is required. The endpoint resolver looks up the profile in `CxfBridgePool::configured_profiles`; an unknown name fails endpoint creation with `unknown profile 'X'`. The `operation` parameter is optional when the WSDL declares a single operation. Use the `CamelCxfOperation` header to override the operation per Exchange.

## CXF consumer

`cxf://http://0.0.0.0:9090/...` hosts a SOAP endpoint. The bridge process publishes the endpoint under `<base>/<profile_name>` over Vert.x. Inbound SOAP requests arrive on the gRPC stream as `ConsumerRequest` messages. The Consumer builds an Exchange with the payload bytes as the body and the headers from the table below.

The response body is what the route writes to `exchange.input.body` (or `exchange.output.body` if the route set it). The bridge wraps the bytes in a SOAP envelope and signs with the profile's certificates.

A failed route handler returns a `soap:Server` fault with the error string. A response-marshalling failure (for example a `Stream` body, which CXF does not support) also returns a `soap:Server` fault, increments `b-prime:cxf:response-marshalling`, and logs at `error!` (ADR-0012 class b').

The inbound Exchange carries these headers from the bridge:

| Header | Value |
| --- | --- |
| `CxfRequestId` | Bridge-generated correlation ID |
| `CxfOperation` | Operation name from the SOAP body |
| `CxfSoapAction` | Value of the `SOAPAction` header |
| `CxfSecurityProfile` | Profile name that handled the request |

Other HTTP headers from the request land in the Exchange header map.

## CXF security

Credentials flow through per-profile environment variables. `CxfProfileEnvVars` wraps keystore, truststore, and signature passwords in `Redacted` while it builds the child-process configuration. gRPC requests carry only the `security_profile` selector. They do not carry password bytes. `CxfSecurityFields::fmt` prints `<redacted>` for every password field. The redaction policy follows ADR-0051.

The XML parser boundary lives in the Java bridge. The Rust component does not run an XML parser. Audit parser hardening in the bridge process. WSDL files also live in the bridge. The Rust side reads them as paths and forwards them to the bridge over the environment.

[CXF-014]: the MTOM multipart body is not yet fully implemented. The flag sets headers but the binary part is not encoded.

**Reference**: [WebSocket crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-ws/CONTEXT.md), [CXF crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-cxf/CONTEXT.md), [ADR-0032 trust boundary](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0032-exchange-data-trust-boundary.md), [ADR-0012 log levels](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0012-log-level-convention-handler-contract-boundaries.md), [ADR-0051 credential redaction](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0051-credential-redaction-at-diagnostic-boundaries.md). Example source: [`examples/ws-server`](https://github.com/kennycallado/rust-camel/tree/main/examples/ws-server), [`examples/cxf-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/cxf-example).
