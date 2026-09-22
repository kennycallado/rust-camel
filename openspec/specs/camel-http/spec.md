# camel-http Specification

## Purpose
TBD - created by archiving change jsonreply. Update Purpose after archive.
## Requirements
### Requirement: JSON error replies preserve existing behavior

The HTTP error finalizer SHALL construct JSON replies for
`TypeConversionFailed`, `ValidationError`, `UnsupportedMediaType`, and
`NotAcceptable` through one private helper without changing observable reply
behavior.

#### Scenario: Type conversion failure remains a bad request

- **GIVEN** `pipeline_error_to_reply` receives `TypeConversionFailed("bad")`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 400, `Content-Type: application/json`, and
  JSON fields `error: "bad_request"` and `message: "bad"`

#### Scenario: Validation failure remains a bad request

- **GIVEN** `pipeline_error_to_reply` receives `ValidationError("invalid")`
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 400, `Content-Type: application/json`, and
  JSON fields `error: "validation_error"` and `message: "invalid"`

#### Scenario: Unsupported media remains 415

- **GIVEN** `pipeline_error_to_reply` receives `UnsupportedMediaType` with
  consumed and declared media values
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 415, `Content-Type: application/json`, and
  message `consumed {consumed}, declared {declared}`

#### Scenario: Unacceptable media remains 406

- **GIVEN** `pipeline_error_to_reply` receives `NotAcceptable` with accept and
  produced media values
- **WHEN** it maps the error to an HTTP reply
- **THEN** the reply has status 406, `Content-Type: application/json`, and
  message `accept {accept}, produced {produced}`

#### Scenario: Empty messages remain valid JSON

- **GIVEN** the helper receives an empty message
- **WHEN** it serializes the reply
- **THEN** the JSON message field is an empty string and the reply remains
  application/json

### Requirement: HTTP maxInflightRequests upper bound

The system SHALL reject `maxInflightRequests` values above
`tokio::sync::Semaphore::MAX_PERMITS` with a typed configuration
error (`CamelError::Config`) that names the parameter, the configured
value, and the limit. The rejection SHALL occur at URI parse, at
`create_consumer`, and at consumer start before shared-server
registry interaction, listener binding, consumer envelope-channel
construction, or inflight-semaphore construction; `spawn_entry` SHALL
additionally validate before any listener side effect and before the
semaphore primitive as defense-in-depth. The system SHALL NOT panic
during route startup for any representable `maxInflightRequests`
value. The value `0` SHALL remain accepted with its existing
reject-everything (503) semantics; no zero normalization SHALL be
introduced.

#### Scenario: oversized value rejected at URI parse

- **GIVEN** an http consumer URI with `maxInflightRequests` greater than `tokio::sync::Semaphore::MAX_PERMITS`
- **WHEN** the URI is parsed into `HttpServerConfig` (including via `from_uri_with_defaults`)
- **THEN** parsing fails with a typed configuration error naming `maxInflightRequests`, the configured value, and the limit

#### Scenario: oversized value rejected at consumer creation even when constructed directly

- **GIVEN** an `HttpComponent` whose `HttpServerConfig` was constructed directly (not via URI parse) with `maxInflightRequests` greater than the limit
- **WHEN** `create_consumer` is invoked
- **THEN** it returns a typed configuration error before any `HttpConsumer` is constructed

#### Scenario: oversized value rejected at consumer start before side effects

- **GIVEN** an `HttpConsumer` constructed directly with `maxInflightRequests` greater than the limit
- **WHEN** `start` is invoked
- **THEN** it returns a typed configuration error before shared-server registry interaction, listener binding, envelope-channel construction, or semaphore construction, and no panic occurs

#### Scenario: defense-in-depth before the semaphore primitive

- **GIVEN** `spawn_entry` invoked with `max_inflight_requests` greater than the limit
- **WHEN** it runs
- **THEN** it returns a typed configuration error before any listener side effect and before `tokio::sync::Semaphore::new` is called, and no panic occurs

#### Scenario: boundary values

- **GIVEN** the limit `L = tokio::sync::Semaphore::MAX_PERMITS`
- **WHEN** bound validation runs with `L-1`, `L`, and `L+1`
- **THEN** `L-1` and `L` are accepted unchanged and `L+1` is rejected with a typed configuration error

#### Scenario: zero-value semantics retained

- **GIVEN** a consumer configured with `maxInflightRequests=0`
- **WHEN** it starts and receives a request
- **THEN** the request is rejected with HTTP 503 as before (rc-3y6j reject-everything semantics); validation does not normalize or reject the zero value

#### Scenario: accepted boundary is constructible in Tokio primitives

- **GIVEN** the limit `L = tokio::sync::Semaphore::MAX_PERMITS`
- **WHEN** a semaphore of `L` permits is constructed
- **THEN** construction succeeds without panicking (the accepted upper bound is exactly the primitive's bound)

### Requirement: Strict TLS material survives the CA-less fallback

When the platform-verifier client build fails and the webpki fallback
triggers, and `tls.enabled` and `tls.strict` are both true, and custom
CA and/or mTLS material is configured, the effective fallback client
SHALL carry that material: the custom CA SHALL be merged into the
Mozilla webpki root store (trust-union parity with the primary path's
`add_root_certificate` semantics) and the mTLS cert/key SHALL be the
client identity. If any material-loading or rustls-configuration step
fails — unreadable file, zero parseable PEM CERTIFICATE sections, zero
roots accepted by the root store, unparseable private key, or
client-auth configuration rejection — `build_client` SHALL return
`CamelError::EndpointCreationFailed` naming the conflict. The fallback
SHALL NOT silently substitute Mozilla roots alone for configured
strict material.

#### Scenario: Custom CA carried into forced fallback

- **GIVEN** a forced-fallback environment (empty platform CA store),
  `tls` with `enabled=true`, `strict=true`, and a `ca_cert_path`
  whose PEM signs a local TLS server's certificate
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds and the response is received

#### Scenario: Material-free client fails against custom-CA server

- **GIVEN** the same forced-fallback environment and local
  custom-CA-certified server
- **WHEN** `build_client` builds a client with strict on but NO
  `ca_cert_path` and requests the same URL
- **THEN** the TLS handshake fails (the CA material is causally
  carried, not incidental)

#### Scenario: Custom CA merged as trust union, not replacement

- **GIVEN** a custom CA PEM containing exactly one root certificate
- **WHEN** the fallback root store is assembled with that CA
- **THEN** the store contains the bundled Mozilla webpki anchors
  AND the custom root (union), not the custom root alone — an
  inspectable root-store construction seam makes the contents
  assertable

#### Scenario: mTLS identity carried into forced fallback

- **GIVEN** a forced-fallback environment, a local TLS server that
  REQUIRES a client certificate, and `tls` with `enabled=true`,
  `strict=true`, a valid `ca_cert_path`, and a matching
  `client_cert_path`/`client_key_path` pair
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds including client-auth and the
  response is received

#### Scenario: mTLS-less client rejected by identity-requiring server

- **GIVEN** the same forced-fallback environment and
  client-cert-requiring server
- **WHEN** `build_client` builds a client with strict on, valid
  `ca_cert_path`, but NO client cert/key pair and requests the same
  URL
- **THEN** the server rejects the TLS handshake

#### Scenario: CA with zero PEM certificate sections fails closed

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, and a `ca_cert_path` whose file
  contains no parseable PEM CERTIFICATE section (e.g. only a private
  key or plain text)
- **WHEN** `build_client` is called
- **THEN** it returns `Err(CamelError::EndpointCreationFailed)` whose
  message names the strict/webpki-fallback conflict, and no fallback
  client is returned

#### Scenario: CA accepted by zero roots fails closed

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, and a `ca_cert_path` whose
  certificate bytes parse as PEM but are rejected by the root store
  (zero roots accepted)
- **WHEN** `build_client` is called
- **THEN** it returns `Err(CamelError::EndpointCreationFailed)` whose
  message names the strict/webpki-fallback conflict, and no fallback
  client is returned

#### Scenario: mTLS key with no parseable private-key section fails closed

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, a valid `client_cert_path`, and a
  `client_key_path` with no parseable private-key section
- **WHEN** `build_client` is called
- **THEN** it returns `Err(CamelError::EndpointCreationFailed)` whose
  message names the strict/webpki-fallback conflict, and no fallback
  client is returned

#### Scenario: PEM-valid but rustls-rejected mTLS key fails closed

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, a valid `client_cert_path`, and a
  `client_key_path` holding a PEM private-key section that parses at
  the PEM layer but is rejected by rustls at client-auth
  configuration (no usable key material)
- **WHEN** `build_client` is called
- **THEN** it returns `Err(CamelError::EndpointCreationFailed)` whose
  message names the strict/webpki-fallback conflict, and no fallback
  client is returned

#### Scenario: Unreadable strict material fails closed with typed error

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, and a `ca_cert_path` or mTLS path
  that cannot be read
- **WHEN** `build_client` is called
- **THEN** it returns `Err(CamelError::EndpointCreationFailed)` whose
  message names the strict/webpki-fallback conflict, and no fallback
  client is returned

#### Scenario: Material-free strict fallback retained

- **GIVEN** a forced-fallback environment and `tls` with
  `enabled=true`, `strict=true`, and no material configured
- **WHEN** `build_client` is called
- **THEN** the client builds successfully on Mozilla webpki roots
  with no configured-material warning

#### Scenario: Non-strict valid material carried into fallback

- **GIVEN** a forced-fallback environment, `tls` with `strict=false`,
  a valid `ca_cert_path` whose PEM signs a local TLS server's
  certificate
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds — valid material is carried
  regardless of `strict`; no degradation warn fires for loadable
  material

#### Scenario: Non-strict material failure downgrades permissively, item-wise

- **GIVEN** a forced-fallback environment, `tls` with `strict=false`,
  a VALID `ca_cert_path` whose PEM signs a local TLS server's
  certificate (server does not require a client certificate), and an
  INVALID mTLS item (`client_key_path` unreadable)
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds — the valid CA item is carried
  and enforces peer trust despite the identity item failing — and the
  existing configured-material warn fires for the failed mTLS item
  (permissive item-wise F2-7 degradation, matching the primary
  path's independent load sites)

#### Scenario: Default construction never panics on CA-less platforms

- **GIVEN** a forced-fallback environment
- **WHEN** `HttpComponent::new()` runs (default config, no TLS
  material)
- **THEN** construction completes without panic and the component
  serves endpoints on the material-free webpki fallback

#### Scenario: Strict error folding surfaces at endpoint creation without panic

- **GIVEN** a forced-fallback environment
- **WHEN** `HttpComponent::with_config()` runs with strict-on
  configuration whose material fails to fold
- **THEN** construction completes without panic and
  `create_endpoint` returns the folded `EndpointCreationFailed`
  error instead of exposing an endpoint

### Requirement: CA-less fallback honors disabled verification

When the webpki fallback triggers and verification is disabled
(`tls.insecure=true` or `tls.verify_peer=false`), the fallback client
SHALL disable certificate verification — parity with the primary
path's `danger_accept_invalid_certs` — instead of silently re-enabling
verification against Mozilla roots.

#### Scenario: Insecure client handshakes with self-signed server in forced fallback

- **GIVEN** a forced-fallback environment, a local TLS server with a
  self-signed certificate, and `tls` with `enabled=true`,
  `insecure=true`
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds and the response is received

#### Scenario: verify_peer=false parity in forced fallback

- **GIVEN** a forced-fallback environment, a local TLS server with a
  self-signed certificate, and `tls` with `enabled=true`,
  `verify_peer=false`
- **WHEN** `build_client` builds the client and it requests `https://`
  from that server
- **THEN** the TLS handshake succeeds and the response is received

