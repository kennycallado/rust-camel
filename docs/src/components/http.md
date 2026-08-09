# HTTP

The HTTP component covers both directions in one crate. The server side binds a TCP listener and consumes inbound requests. The client side produces outbound requests and forwards the response into the Route. The same `http:` scheme serves both. The direction follows the Endpoint position: `from:` for a server, `to:` for a client.

The http-server example shows the server direction with a process step that returns a JSON body:

```rust,ignore
{{#include ../../../examples/http-server/src/main.rs:http-health-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: health-check
    from: "http://0.0.0.0:8080/health"
    steps:
      - to: "log:health?showHeaders=true&showBody=true&showCorrelationId=true"
      - bean:
          name: health-handler
          method: build_status
```

The `.process()` closure reads runtime state. It reads the system clock, a request counter, and an in-memory store. Register that logic as a bean and call it with a `bean:` step.

</details>

The http-client example shows the client direction. A timer drives a GET request to a remote service, and a log step records the response:

```rust,ignore
{{#include ../../../examples/http-client/src/main.rs:http-client-route}}
```

<details>
<summary>YAML equivalent</summary>

```yaml
routes:
  - id: http-client
    from: "timer:http-poll?period=5000&repeatCount=3"
    error_handler:
      dead_letter_channel: "log:http-dlc?showBody=true"
      retry:
        max_attempts: 2
    steps:
      - bean:
          name: request-id-setter
          method: apply
      - to: "https://httpbin.org/get?source=rust-camel&allowInternal=false"
      - bean:
          name: response-printer
          method: apply
      - to: "log:http-response?showHeaders=true&showBody=true&showCorrelationId=true"
```

Both `.process()` closures have no YAML step. The first sets a UUID request header. The second parses and prints the response. Register each as a bean and call them with `bean:` steps. The Rust `on_exception(|_| true)` matches every exception. The YAML `retry.max_attempts` mirrors the Rust `.retry(2)`.

</details>

## Server

`http://0.0.0.0:8080/health` is a Consumer Endpoint. The Runtime binds a TCP listener on the address and port from the URI, then dispatches each inbound request to the Route. The Consumer submits one Exchange per request. The Exchange body carries the request payload. The Exchange headers carry the request headers, the path, the query string, and the request metadata.

The example binds `0.0.0.0` to listen on every interface. Use `127.0.0.1` when the server must stay on the loopback. The component rejects a partial server TLS configuration that supplies only a certificate or only a key. Operators who terminate TLS at a separate proxy need no server TLS in the URI.

The Consumer bounds resource use. It enforces a 2 MiB default request-body limit, a read timeout, and an in-flight request semaphore. The reply finaliser maps the Exchange body shape to a response. A `Body::Json` value serializes to JSON. A `Body::Stream` value streams the response. The `CamelHttpResponseCode` header sets the HTTP status. A missing header produces `200 OK`.

## Client

`to: http://example.org/api/data` is a Producer Endpoint. The Producer builds an outbound request, sends it through the configured transport, and returns the response as the new Exchange body. The Producer follows redirects, applies the configured `NetworkRetryPolicy`, and surfaces transport failures as `CamelError`.

The component validates each outbound URL and each redirect hop. `allow_internal=false` rejects internal addresses by default. The Producer pins DNS resolution to validated addresses to prevent DNS rebinding. Cross-origin redirects drop the `Authorization` and `Cookie` headers. The Producer has a 10 MiB default response-body limit.

`TlsConfig` verifies peer certificates by default. An operator can opt out with `insecure=true` or `verify_peer=false`. The opt-out emits a warning. The component forbids cleartext HTTP to public addresses even with `allow_internal=true`.

## Direction choice

A route that serves an API uses HTTP as a source. A route that calls another service uses HTTP as a sink. A route that does both needs two Endpoints, one in each position. The second Endpoint belongs to a second Route. The component supports both directions because the same crate owns the connection plumbing. Each Endpoint still picks one direction at creation time.

The inbound and outbound contract surface lives in the [camel-http CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-http/CONTEXT.md). The diagnostic endpoints (`/healthz`, `/readyz`, `/metrics`) live in the [Operations authority](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md). The example sources are at [`examples/http-server`](https://github.com/kennycallado/rust-camel/tree/main/examples/http-server) and [`examples/http-client`](https://github.com/kennycallado/rust-camel/tree/main/examples/http-client).
