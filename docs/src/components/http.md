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

Endpoints declared through a `rest:` block are `http:` consumers after lowering, and they are secured the same way: the block-level `security_policy` is copied onto every lowered route, so its roles or scopes policy runs at the request boundary. See [Route structure](../yaml-dsl/route-structure.md) for the block key and [`examples/rest-crud`](https://github.com/kennycallado/rust-camel/tree/main/examples/rest-crud) for the runnable secured variant. For non-JSON payloads such as protobuf, the operation declares `binding: raw` with explicit data-format steps; see [REST usage (raw binding)](../data-formats/protobuf.md#rest-usage-raw-binding).

## Client

`to: http://example.org/api/data` is a Producer Endpoint. The Producer builds an outbound request, sends it through the configured transport, and returns the response as the new Exchange body. The Producer applies the configured `NetworkRetryPolicy` and surfaces transport failures as `CamelError`. By default the Producer does not follow redirects: a 3xx response is returned as-is, and `throwExceptionOnFailure=true` (the default) fails the exchange with an error that names `followRedirects`.

The component validates each outbound URL and each redirect hop. `allow_internal=false` rejects internal addresses by default. The Producer pins DNS resolution to validated addresses to prevent DNS rebinding. Cross-origin redirects drop the `Authorization` and `Cookie` headers. The Producer has a 10 MiB default response-body limit.

`TlsConfig` verifies peer certificates by default. An operator can opt out with `insecure=true` or `verify_peer=false`. The opt-out emits a warning. The component forbids cleartext HTTP to public addresses even with `allow_internal=true`.

### Expired-certificate allowlist (per-host pinned)

`TlsConfig` carries an opt-in allowlist that tolerates one expired certificate per pinned host + fingerprint. `tls.expired_allowlist` lists `{host, port, fingerprint}` entries. The default rejects expired certificates. The allowlist is the scoped alternative to `insecure=true` and `verify_peer=false`.

```yaml
components:
  http:
    tls:
      enabled: true
      expired_allowlist:
        - host: backend.internal.example
          port: 8443
          fingerprint: "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"
```

Each entry pins one `host`, one `port`, and one end-entity certificate fingerprint. The `fingerprint` is `sha256:` followed by exactly 64 hex characters of the DER-encoded certificate. Hex case is normalized. The `host` is an exact DNS name or IP literal: no wildcard, no subdomain pattern, no whitespace. Host case is normalized. Duplicate entries (same normalized host, port, and fingerprint) are rejected. The list has no URI parameter form: component YAML only, and never read from exchange headers.

Enforcement is narrow. Only certificate-expiry errors are tolerated. The hostname is independently re-verified against the presented end-entity certificate. Every other verification error still fails on a listed host: unknown issuer, hostname mismatch, wrong fingerprint. An expired certificate with a fingerprint not on the list fails like an unlisted host. The `port` is declarative: it feeds validation and duplicate detection only. The TLS verifier layer carries no port, so enforcement matches host + fingerprint.

The allowlist path builds trust on the bundled Mozilla roots plus `tls.ca_cert_path`. It does not consult the OS trust store. Operators whose chain issues from an OS-store CA must add that CA with `tls.ca_cert_path`.

Expiry anywhere on the presented chain is tolerated while the pinned end-entity certificate is the leaf. An expired intermediate therefore does not fail the handshake when the pinned leaf is presented.

Each accepted expired certificate emits one WARN that names the redacted host and the fingerprint. The WARN repeats at most once per 60 s window per host + fingerprint pair.

Prefer the allowlist over `insecure=true` and `verify_peer=false` when one backend has an expired certificate. The allowlist scopes the risk to one host and one certificate. `insecure` disables all verification for every host the endpoint talks to. The allowlist has its own risk: it pins trust to that specific certificate. Pin only certificates the operator controls or has verified.

A non-empty allowlist combined with any of the following fails endpoint creation:

| Conflict | Effect |
| --- | --- |
| `insecure=true` | Endpoint creation fails. |
| `verify_peer=false` | Endpoint creation fails. |
| `enabled=false` | Endpoint creation fails. |
| Malformed entry: missing `sha256:` prefix, non-64-hex fingerprint, wildcard, whitespace, or empty host, `port` 0 | Endpoint creation fails. |
| Duplicate entry (same normalized host, port, fingerprint) | Endpoint creation fails. |

### Redirects

The Producer does not follow redirects by default. A 3xx response is returned as-is, and the exchange fails when `throwExceptionOnFailure=true`.

| Option | Default | Accepted | Effect |
| --- | --- | --- | --- |
| `followRedirects` | `false` | `true`, `false` | Follows 301, 302, 303, 307, and 308 responses through the SSRF-validated redirect loop. |
| `maxRedirects` | `10` | `0` to `20` | Hop budget for the redirect loop. `0` sends once and returns the redirect as-is. Has no effect unless `followRedirects=true`. |

Following redirects widens egress: each hop can name a new target, so the request reaches hosts beyond the one in the URI. That trade-off is why the default is off. Every hop passes the same SSRF validation as the initial request; see [ADR-0071](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0071-http-outbound-url-policy.md) for the outbound URL policy.

### HTTP method selection

The Producer resolves the outbound method in this order: the `httpMethod` URI option, then the `CamelHttpMethod` header, then a body-based fallback. An empty body selects `GET`. A non-empty body selects `POST`.

An exchange from a non-HTTP consumer (timer, cron, file, quartz) carries no `CamelHttpMethod` header and usually a non-empty default body. A timer exchange, for example, holds a tick label. Driving an `http:` producer from such a source therefore selects `POST` by default. A GET-only upstream then rejects the request, often with a message that does not name the method as the cause.

Set an explicit method when the source is not HTTP. Append `httpMethod=GET` to the destination URI, or set the header:

```yaml
- to: "http://wfs.example.org/ows?httpMethod=GET"
```

```yaml
- set_header:
    key: CamelHttpMethod
    value: GET
- to: "http://wfs.example.org/ows"
```

### URL header inheritance and bridgeEndpoint

The HTTP consumer sets `CamelHttpMethod`, `CamelHttpPath`, and `CamelHttpQuery` on the inbound Exchange. An `http:` producer in the same Route reads these headers to build the outbound request. This inheritance is the default state (`bridgeEndpoint=false`).

The inherited path appends to the endpoint path. This route sends the request to `/_actions/contact/_forms/contact`:

```yaml
routes:
  - id: http-inherited-path-append
    from: "http://0.0.0.0:8080/_forms/contact"
    steps:
      - to: "http://host/_actions/contact"
```

Remove the header before the producer call to keep the configured path:

```yaml
routes:
  - id: http-inherited-path-remove
    from: "http://0.0.0.0:8080/_forms/contact"
    steps:
      - remove_header:
          key: CamelHttpPath
      - to: "http://host/_actions/contact"
```

The request path is now `/_actions/contact`. The `remove_header` step takes the exact header name. It has no wildcard support. Remove `CamelHttpQuery` in the same way when the inherited query is unwanted.

`bridgeEndpoint=true` is the other option. The producer then ignores `CamelHttpUri`, `CamelHttpPath`, and `CamelHttpQuery`, and sends the request to the endpoint base URL with only the endpoint query. Bridging also skips the configured auth headers, the configured `userAgent` header, and the OTel W3C trace-context headers when the `otel` feature is enabled. It is not a drop-in proxy switch. When only one header is unwanted, prefer `remove_header`.

The Producer resolves the trio in this order: the `httpMethod` option beats `CamelHttpMethod`. In the no-override case, endpoint query pairs beat `CamelHttpQuery` pairs on collision. With a `CamelHttpUri` override, the override URI's own query is the higher-precedence source and the endpoint query does not ride. `CamelHttpPath` appends to the configured path, or to a `CamelHttpUri` override path.

Default path inheritance and `bridgeEndpoint=true` semantics are Apache Camel parity. The `CamelHttpQuery` composition rules are a rust-camel divergence, documented in [ADR-0071](https://github.com/kennycallado/rust-camel/blob/main/docs/adr/0071-http-outbound-url-policy.md). Neither is a defect. The component emits no runtime warning for it.

## Direction choice

A route that serves an API uses HTTP as a source. A route that calls another service uses HTTP as a sink. A route that does both needs two Endpoints, one in each position. The second Endpoint belongs to a second Route. The component supports both directions because the same crate owns the connection plumbing. Each Endpoint still picks one direction at creation time.

The inbound and outbound contract surface lives in the [camel-http CONTEXT](https://github.com/kennycallado/rust-camel/blob/main/crates/components/camel-http/CONTEXT.md). The diagnostic endpoints (`/healthz`, `/readyz`, `/metrics`) live in the [Operations authority](https://github.com/kennycallado/rust-camel/blob/main/crates/components/CONTEXT.md). The example sources are at [`examples/http-server`](https://github.com/kennycallado/rust-camel/tree/main/examples/http-server) and [`examples/http-client`](https://github.com/kennycallado/rust-camel/tree/main/examples/http-client).
