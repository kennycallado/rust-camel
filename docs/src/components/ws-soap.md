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
          key: "CamelWsSendToAll"
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

Auth rides the unified transport auth kernel (ADR-0061; see [Authentication and authorization](../services/auth.md)). When a route carries a compiled security plan, the upgrade handler authenticates per the plan's credential sources via `kernel_authenticate` and stamps the typed principal carrier onto every message Exchange; a non-Public route without the carrier is denied pre-pipeline by the strict dispatch check. Failed authentication returns `401` at the upgrade. Policy evaluation errors return `500` and increment the `e:ws:policy-eval` metric (ADR-0012 class e). The health pin becomes the operator signal for bind failures (ADR-0012 class g).

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
    from: "timer:hello-tick?period=10000"
    steps:
      - set_body:
          value: "<sayHello><name>World</name></sayHello>"
      - to: "cxf://http://localhost:8080/hello?wsdl=wsdl/hello.wsdl&service={http://example.com/hello}HelloService&port={http://example.com/hello}HelloPort&operation=sayHello&profile=hello"
      - log: "SOAP response: ${body}"
```

<details>
<summary>Consumer route (YAML)</summary>

```yaml
routes:
  - id: cxf-consumer-hello
    from: "cxf://http://0.0.0.0:9090/cxf/hello?wsdl=wsdl/hello.wsdl&service={http://example.com/hello}HelloService&port={http://example.com/hello}HelloPort&profile=hello"
    steps:
      - log: "Received SOAP request: ${body}"
      - set_body:
          simple: "<sayHelloResponse><message>Hello from rust-camel!</message></sayHelloResponse>"
```

The body of the response Exchange is what the bridge writes back as the SOAP body. The bridge wraps it in an envelope and signs it with the profile's certificates.

</details>

The example ships with `Camel.toml` that declares the profile:

```toml
[components.cxf]
version = "0.8.0"
bind_address = "http://0.0.0.0:9090/cxf"

[[components.cxf.profiles]]
name = "hello"
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

The producer dispatch runs `Service.Mode.PAYLOAD`. The Exchange body is the bare operation element. Do not wrap it in a SOAP envelope yourself: the bridge emits exactly one `Envelope` with exactly one `Body`, and the operation element is a direct child of that `Body`.

The SOAP version derives from the selected WSDL port binding, never from a request header. A `soap-version` header is ignored: a port bound to SOAP 1.1 emits a SOAP 1.1 envelope and a port bound to SOAP 1.2 emits a SOAP 1.2 envelope. An unrecognized binding ID fails the invocation. A payload whose root element is already a SOAP `Envelope` is rejected before the network call; no request is sent.

### One-way producer operations

A WSDL operation is one-way (input-only) when the operation in the selected service/port binding has an input and no output. The producer resolves the operation against that binding: an explicit `operation` name, or the bare payload root QName when the name is absent. An unknown or ambiguous selection fails before any HTTP request.

For an input-only operation the bridge calls `Dispatch.invokeOneWay` exactly once with the bare payload. An empty HTTP 200 or 202 acknowledgment completes successfully with empty output bytes. The bridge does not deserialize an absent response body. Resolved WSDL dispatches are cached per profile.

One-way producer dispatch is non-robust: `org.apache.cxf.oneway.robust` is fixed to `false`, and there is no `oneway.robust` option. A successful send does not assert remote business success and does not guarantee observation of a remote SOAP fault. A local bridge or transport failure remains an error, never an empty success, and does not replay or resend the invocation. Recovery is a subsequent explicit send, a separate action.

An operation with WSDL output stays request-reply. An empty response to a request-reply operation is an error: the producer never falls back to one-way, fabricates a payload, or resends.

## CXF consumer

`cxf://http://0.0.0.0:9090/...` hosts a SOAP endpoint. The bridge process publishes the endpoint under `<base>/<profile_name>` over Vert.x. Inbound SOAP requests arrive on the gRPC stream as `ConsumerRequest` messages. The Consumer builds an Exchange with the payload bytes as the body and the headers from the table below.

Each consumer route serves one profile. The consumer opens its own gRPC stream and announces the profile as the `x-cxf-profile` call metadata; the bridge delivers a request only to the stream of the profile in its path and never falls back to another profile's stream. A pool accepts one consumer route per profile: a second route on the same profile fails at start with a configuration error naming the profile. Stopping and restarting a route is allowed.

The response body is what the route writes to `exchange.input.body` (or `exchange.output.body` if the route set it). The bridge wraps the bytes in a SOAP envelope and signs with the profile's certificates.

A failed route handler returns a `soap:Server` fault with the error string. A response-marshalling failure (for example a `Stream` body, which CXF does not support) also returns a `soap:Server` fault, increments `b-prime:cxf:response-marshalling`, and logs at `error!` (ADR-0012 class b'). This holds for an input-only operation too: a marshalling failure stays a Receiver fault and never becomes a successful ACK.

The inbound Exchange carries these headers from the bridge:

| Header | Value |
| --- | --- |
| `CxfRequestId` | Bridge-generated correlation ID |
| `CxfOperation` | Operation name from the SOAP body |
| `CxfSoapAction` | Value of the `SOAPAction` header |
| `CxfSecurityProfile` | Profile name that handled the request |

Other HTTP headers from the request land in the Exchange header map.

### One-way consumer operations

Startup builds a per-profile operation catalog from the selected WSDL binding before the HTTP listener binds. Misconfigured or unloadable WSDL metadata fails listener startup before a ready port is announced. Every request resolves the SOAP `Body` root QName against the profile's startup catalog. A supplied SOAPAction must agree with the resolved operation.

An unknown, conflicting, or ambiguous operation faults Sender (SOAP 1.1 HTTP 500, SOAP 1.2 HTTP 400) with zero route dispatch. The WSS-only `fault_status` override never applies to these faults.

For a resolved input-only operation the listener waits for the correlated, non-fault response, including successful Rust response marshalling. It then returns HTTP 202 with a zero-length body and discards the marshalled payload. It writes no outbound ACK envelope and no ACK signature. Route success alone does not bypass marshalling: a route that returns `Body::Stream` marshals to nothing, and that marshalling failure is a Receiver fault (HTTP 500, never 202). Stream unavailability stays HTTP 503 with `Retry-After` and a plain body.

### Consumer fault status

The listener renders a version-matched SOAP fault for every SOAP-processing
failure and maps the fault class to an HTTP status. Malformed XML and a missing
SOAP `Body` are rejected before WS-Security processing, so they never reach a
security override.

| Failure | Class | SOAP 1.1 | SOAP 1.2 |
| --- | --- | --- | --- |
| WS-Security verification | Sender | 500 | 400 |
| Malformed envelope | Sender | 500 | 400 |
| Missing SOAP `Body` | Sender | 500 | 400 |
| Rust `soap:Client` fault code | Sender | 500 | 400 |
| Signer-descriptor validation | Receiver | 500 | 500 |
| Route error | Receiver | 500 | 500 |
| Timeout | Receiver | 500 | 500 |
| Rust `soap:Server` fault code | Receiver | 500 | 500 |
| Any other SOAP 1.2 fault code | Receiver | 500 | 500 |

A successful request-reply response is HTTP 200, wrapped and signed once. A successful input-only operation returns HTTP 202 with a zero-length body only after the correlated, non-fault route completion. A fault never returns 200 or 202.

Transport rejections are not SOAP faults. They never enter the table above:

| Transport rejection | Response | Order |
| --- | --- | --- |
| Body over `CXF_MAX_BODY_BYTES` | HTTP 413, plaintext `request body exceeds CXF_MAX_BODY_BYTES`, connection closed | First: during body accumulation — before XML parsing, WS-Security, and the availability gate |
| No consumer stream for the addressed profile (pre-stream, or lost after the gate) | HTTP 503 + `Retry-After`, plaintext `routes unavailable` | Availability gate for the profile in the request path — before XML parsing and WS-Security |

`security.fault_status` overrides the HTTP status for **security (WSS) faults
only**, on the profile that sets it, on both SOAP versions. It is an integer in
400–599, validated at startup. It does not apply to any other fault class, and
it never applies to the transport rejections above (413, 503). There is no
global override.

```toml
[[components.cxf.profiles]]
name = "esap"
wsdl_path = "wsdl/hello.wsdl"
service_name = "{http://example.com/hello}HelloService"
port_name = "{http://example.com/hello}HelloPort"

[components.cxf.profiles.security]
keystore_path = "/etc/cxf/esap.jks"
keystore_password = "changeit"
truststore_path = "/etc/cxf/esap.jks"
truststore_password = "changeit"
fault_status = 403
```

With this profile, a WSS verification failure returns HTTP 403. A route error still
returns 500, and a malformed envelope still follows the matrix (400 on SOAP 1.2).
The override is not an authentication decision: the listener writes no principal
and no `camel.auth.*` property.

### Consumer signer identity

When inbound WS-Security verification succeeds, the listener exports the signer
of the request as the `SignerIdentity` field of the `ConsumerRequest`. The
descriptor has exactly four string fields and never carries certificate bytes:

| Field | Value |
| --- | --- |
| `subject` | RFC 2253 distinguished name of the certificate subject |
| `issuer` | RFC 2253 distinguished name of the certificate issuer |
| `serial` | Certificate serial number as lowercase hex |
| `fingerprint_sha256` | 64 lowercase hex characters: SHA-256 of the DER certificate |

The certificate must come from a validated, trusted `SIGN` engine result. The
bridge never reads identity from message headers, a `UsernameToken`, or an
unchecked certificate. A certificate whose DER encoding exceeds 64 KiB is
rejected, never truncated. At most 8 `SIGN` results are inspected; a 9th `SIGN`
result rejects the request on the budget alone. Certificates that share one
fingerprint count as one signer; two distinct Body-covering certificates reject
the request as ambiguous. When no signature is present the field stays absent,
meaning unknown. The metadata is signer identity, not authentication: no
principal is written and no `camel.auth.*` property is set.

**Intended breaking change:** a profile whose inbound actions include
`Signature` now requires a signature that covers the SOAP `Body`. Clients that
sign only the timestamp or only a header are now rejected. Profiles without a
`Signature` inbound action are not affected: the bridge runs no signer inspection
or signer policy on them and dispatches with no signer.

## CXF security

Credentials flow through per-profile environment variables. `CxfProfileEnvVars` wraps keystore, truststore, and signature passwords in `Redacted` while it builds the child-process configuration. gRPC requests carry only the `security_profile` selector. They do not carry password bytes. `CxfSecurityFields::fmt` prints `<redacted>` for every password field. The redaction policy follows ADR-0051.

### Producer transport TLS

A producer profile can present a client certificate and validate the server over HTTPS. The material is a nested, all-or-nothing table under `[components.cxf.profiles.security.tls]`:

```toml
[[components.cxf.profiles]]
name = "baleares"
wsdl_path = "wsdl/hello.wsdl"
service_name = "{http://example.com/hello}HelloService"
port_name = "{http://example.com/hello}HelloPort"

[components.cxf.profiles.security.tls]
keystore_path = "/etc/cxf/keystore.p12"
keystore_password = "changeit"
keystore_type = "PKCS12"
truststore_path = "/etc/cxf/truststore.p12"
truststore_password = "changeit"
truststore_type = "PKCS12"
```

| Key | Description |
| --- | --- |
| `keystore_path` | Path to the client key store. |
| `keystore_password` | Password for the client key store. Required with `keystore_path`. |
| `keystore_type` | `PKCS12` (default) or `JKS`. |
| `truststore_path` | Path to the trust store that validates the server certificate. |
| `truststore_password` | Password for the trust store. Required with `truststore_path`. |
| `truststore_type` | `PKCS12` (default) or `JKS`. |

Rules:

- `*_path` and `*_password` are supplied together. A path without a password fails pool validation at startup with an error that names the missing pair member.
- An explicitly supplied empty or whitespace-only path or password fails validation in both layers. Only an omitted key stays unset. A nonblank password keeps its spaces. Rust rejects the value at config load. The Java bridge rejects it before it announces ready.
- The store type defaults to `PKCS12`. Any other spelling is rejected at configuration load.
- A store that a password does not open aborts bridge startup before the `{"status":"ready",...}` line.
- A trust-store-only profile validates the server certificate and sends no client certificate.
- Hostname verification stays enabled. No option disables it. A certificate whose SAN does not match the endpoint host fails the handshake.
- Each profile uses only its own TLS material. Material from one profile never applies to another profile.

Transport TLS is independent of the WS-Security message-layer stores. Transport TLS encrypts the HTTP connection and authenticates the peer at the TLS layer. WS-Security signs and encrypts the SOAP envelope at the message layer. A profile can use either, both, or neither; the two store pairs do not share values.

Transport TLS applies to the producer (`cxf://https://...`) profiles. The consumer listener has its own pool-level transport TLS (Phase 2, operational).

### Consumer listener (pool-level, Phase 2)

The consumer SOAP listener is configured with FLAT keys on the pool (`[components.cxf]`), never per profile:

| Key | Meaning |
| --- | --- |
| `bind_address` | `http://<host>:<port>/<base-path>` or `https://<host>:<port>/<base-path>`. Port `0` binds an OS-assigned port. |
| `soap_tls_cert_path` + `soap_tls_key_path` | Listener server identity (PEM). XOR with the PKCS#12 form. |
| `soap_tls_pkcs12_path` + `soap_tls_pkcs12_password` | Listener server identity (PKCS#12). XOR with the PEM form. |
| `soap_tls_client_ca_path` | Client CA. Always required when listener TLS is configured: client auth is mandatory (mutual TLS). |

Rules:

- Exactly one key-material form (PEM XOR PKCS#12). A partial pair or both forms fails pool validation at startup.
- An explicitly supplied empty or whitespace-only `soap_tls_*` value fails validation in both layers. Only an omitted key stays unset. A nonblank password keeps its spaces. Rust rejects the value at config load. The Java bridge rejects it before the listener binds.
- An `https` bind requires key material; material requires a bind. An `http` bind without material stays valid: ingress termination is still optional and plaintext is an interim deployment form.
- Listener TLS is independent of the WS-Security message-layer stores. A pool with listener TLS can use either, both, or neither WSS store set.
- Required client auth: a client without a certificate, or with a certificate from another CA, is rejected at the TLS handshake — before any SOAP processing.

Startup protocol: the bridge announces `{"status":"ready","port":N}` for gRPC and `{"status":"soap-ready","port":N}` for the SOAP listener, in any order. `N` is the ACTUAL bound port held by the JVM from the first moment it exists (bind port `0` never leaks into the announce). The supervisor (`BridgeProcess`) awaits both lines under one `bridge_start_timeout_ms` budget whenever a SOAP bind is configured; a malformed or out-of-range announced port fails startup. The announced SOAP port is the only port source for clients — no bind-read-drop probe.

Pre-stream availability gate: the gate is per profile. A request to a profile path whose consumer stream is not open receives HTTP 503 with a `Retry-After` header and a plaintext body (`routes unavailable`) — never a SOAP fault — even when another profile has a connected consumer. The same response serves a stream lost after the gate.

Diagnosis: if a profile keeps answering 503 while its route is started, check the log for `CXF bridge rejected consumer profile with INVALID_ARGUMENT`. The bridge rejected the profile the consumer announced (crate and bridge versions out of step, or a profile missing from the bridge configuration); the consumer logged that one error, stopped and does not reconnect. Fix the version pair or profile configuration, then restart the route.

Signer metadata (Body `SIGN` coverage) is an integrity signal, not authentication: the exported `SignerIdentity` fields are exchange properties only and never a principal or authenticated identity (ADR-0032). One-way input-only operations are supported and return an empty HTTP 202 ACK after correlated completion (see [One-way consumer operations](#one-way-consumer-operations)).

CXF and XML bridge release pins are `0.8.1` (CXF) and `0.8.0` (XML). One-way acceptance and the mission-361 regressions run against the pinned `0.8.1` bridge rebuilt from source: set `CAMEL_CXF_BRIDGE_BINARY_PATH` to the rebuilt `cxf-bridge` binary to use it. Publication of bridge tags and public release assets is an owner-only action (the `*-bridge-release` workflows build and publish on tag push).

The XML parser boundary lives in the Java bridge. The Rust component does not run an XML parser. Audit parser hardening in the bridge process. WSDL files also live in the bridge. The Rust side reads them as paths and forwards them to the bridge over the environment.

[CXF-014]: the MTOM multipart body is not yet fully implemented. The flag sets headers but the binary part is not encoded.

**Reference**: [WebSocket crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-ws/CONTEXT.md), [CXF crate CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-cxf/CONTEXT.md), [ADR-0032 trust boundary](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0032-exchange-data-trust-boundary.md), [ADR-0012 log levels](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0012-log-level-convention-handler-contract-boundaries.md), [ADR-0051 credential redaction](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0051-credential-redaction-at-diagnostic-boundaries.md). Example source: [`examples/ws-server`](https://github.com/kennycallado/rust-camel/tree/main/examples/ws-server), [`examples/cxf-example`](https://github.com/kennycallado/rust-camel/tree/main/examples/cxf-example).
